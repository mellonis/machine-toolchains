//! move-elim: delete an immediately adjacent inverse move pair — `rgt;
//! lft` or `lft; rgt` — that provably makes no observable difference to
//! the run. Part of the `-O1` pipeline (optimizer/mod.rs), inserted
//! immediately before fuse-tape-ops so fuse-tape-ops sees the op stream
//! this pass has already settled.
//!
//! # The two soundness proofs
//!
//! A pair at some position is deleted when the MF-coupling dataflow fact
//! (optimizer/dataflow.rs) just BEFORE it proves one of:
//!
//! 1. **The path is already `Coupled`.** By the coupling invariant, MF
//!    already equals `cell_at_head == 1` before the pair runs. The pair
//!    steps the head away and back to that SAME cell with no write in
//!    between, so it re-latches MF from the identical cell it started
//!    at — post-pair MF, and every later fact, is unchanged whether the
//!    pair ran or not.
//! 2. **MF is dead after the pair (`mf_dead_after`).** On an `Uncoupled`
//!    path the pair's own re-latch DOES change what MF holds (nothing
//!    coupled it before), but deleting is still sound if no reachable
//!    path reads that MF before some later tape instruction re-latches
//!    it from scratch first. `mf_dead_after` walks forward from just
//!    past the pair: a move or write ends the walk successfully (the
//!    next latch moots the pair's own); a `Check` terminator, `Call`, or
//!    `Brk` is a possible reader/observer and ends the walk
//!    unsuccessfully; `Return`/`Halt` end the function with nothing left
//!    to read; `Goto`/`FallThrough` continue the walk into the
//!    successor, with a visited-block guard treating a revisited block
//!    as dead (a latch-free, read-free cycle never reads MF on that path
//!    either).
//!
//! Both proofs are evaluated at the pair's own position, using the fact
//! BEFORE it — but `block_entry_facts` and `mf_dead_after`'s forward walk
//! are each computed once, function-wide, at the top of `run()`, reading
//! the ops as they stood when `run()` was CALLED. A single call therefore
//! deletes AT MOST ONE pair — the first sound one found, in block then
//! position order — and returns immediately: the same one-application
//! discipline `tail_sink::run()` uses (tail_sink.rs). Deleting more than
//! one pair per call risks a cross-block circularity two pairs in
//! different blocks could license each other on a snapshot neither of
//! their deletions has actually happened against yet: pair A deleted as
//! MF-dead, relying on pair B's own leading op as the re-latch witness;
//! pair B deleted as already-`Coupled`, where that coupling fact was
//! produced by pair A. Each proof holds against the pre-deletion IR in
//! isolation, but applying both AT ONCE would leave neither witness
//! standing. The fixpoint driver in optimizer/mod.rs reruns the pass with
//! FRESH facts after every deletion, so each decision is always checked
//! against the IR as it actually stands, and a run with no cascades still
//! converges in as many rounds as it has pairs.
//!
//! # Gated in the volatile column
//!
//! Every tape access is externally observable on a volatile band
//! (docs/pmt/optimizer.md (volatile builds)) — a move is one such access,
//! so eliminating the pair drops two of them regardless of which proof
//! licensed it. docs/pmt/optimizer.md (move elimination) is the durable
//! writeup.

use std::collections::{HashMap, HashSet};

use super::dataflow::{Fact, block_entry_facts, transfer_op};
use crate::ir::{IrFunction, IrOp, IrTerm};

fn is_inverse_pair(a: &IrOp, b: &IrOp) -> bool {
    matches!(
        (a, b),
        (IrOp::Rgt { .. }, IrOp::Lft { .. }) | (IrOp::Lft { .. }, IrOp::Rgt { .. })
    )
}

/// MF-dead: from `block`'s ops after position `i` (the pair already
/// considered removed), every reachable path re-latches MF (a tape op)
/// before any MF read (a `Check` terminator), observation (`Brk`), or
/// opaque reader (`Call`).
///
/// Checking only the first op suffices: every `IrOp` variant either
/// re-latches or blocks, so the match below is exhaustive over the whole
/// enum — a future variant added to `IrOp` fails to compile here rather
/// than silently falling through unclassified.
fn mf_dead_after(
    f: &IrFunction,
    index: &HashMap<u32, usize>,
    block: usize,
    i: usize,
    seen: &mut HashSet<u32>,
) -> bool {
    if let Some(op) = f.blocks[block].ops[i..].first() {
        return match op {
            IrOp::Lft { .. }
            | IrOp::Rgt { .. }
            | IrOp::Wr { .. }
            | IrOp::WrLft { .. }
            | IrOp::WrRgt { .. } => true,
            IrOp::Brk { .. } | IrOp::Call { .. } => false,
        };
    }
    match &f.blocks[block].term {
        IrTerm::Check { .. } => false,
        IrTerm::Return | IrTerm::Halt => true,
        IrTerm::TailCall { .. } => false,
        IrTerm::FallThrough { to } | IrTerm::Goto { to } => {
            if !seen.insert(*to) {
                return true; // latch-free, read-free cycle: no read on this path
            }
            index
                .get(to)
                .is_some_and(|&b| mf_dead_after(f, index, b, 0, seen))
        }
    }
}

pub fn run(f: &mut IrFunction) -> u32 {
    let entry_facts = block_entry_facts(f);
    let index: HashMap<u32, usize> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id, i))
        .collect();

    // Find the FIRST sound deletion, in block then position order, and stop
    // — see the module doc for why a call may license only one (the
    // cross-block circularity a batch of deletions checked against one
    // stale snapshot risks).
    let mut found: Option<(usize, usize)> = None; // (block idx, op idx of pair start)
    'search: for (bi, b) in f.blocks.iter().enumerate() {
        let Some(&entry) = entry_facts.get(&b.id) else {
            continue; // unreachable block
        };
        let mut fact = entry;
        let mut i = 0;
        while i + 1 < b.ops.len() {
            if is_inverse_pair(&b.ops[i], &b.ops[i + 1]) {
                let sound = matches!(fact, Fact::Coupled(_))
                    || mf_dead_after(f, &index, bi, i + 2, &mut HashSet::new());
                if sound {
                    found = Some((bi, i));
                    break 'search;
                }
            }
            fact = transfer_op(fact, &b.ops[i]);
            i += 1;
        }
    }

    let Some((bi, i)) = found else {
        return 0;
    };
    f.blocks[bi].ops.drain(i..=i + 1);
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn opt_fn(src: &str) -> IrFunction {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        ir.functions.remove(0)
    }

    #[test]
    fn coupled_pair_is_eliminated_even_before_a_check() {
        // wr 1; rgt; lft; term Check — fact at the pair is Coupled(the
        // wr), so the pair goes; the check reads identical MF.
        let f = opt_fn("f() { mark; right; left; check(1, 2); 1: mark(!); 2: unmark(!); }");
        assert_eq!(f.blocks[0].ops, vec![IrOp::Wr { index: 1, line: 1 }]);
    }

    #[test]
    fn uncoupled_pair_before_a_check_is_kept() {
        // Entry block: rgt; lft; term Check — entry fact Uncoupled, no
        // later latch: kept.
        let f = opt_fn("f() { right; left; check(1, 2); 1: mark(!); 2: unmark(!); }");
        assert_eq!(
            f.blocks[0].ops,
            vec![IrOp::Rgt { line: 1 }, IrOp::Lft { line: 1 }]
        );
    }

    #[test]
    fn uncoupled_pair_is_eliminated_when_a_latch_dominates_the_next_read() {
        // rgt; lft; wr 0; term Check — MF-dead: the wr re-latches.
        let f = opt_fn("f() { right; left; unmark; check(1, 2); 1: mark(!); 2: unmark(!); }");
        assert_eq!(f.blocks[0].ops, vec![IrOp::Wr { index: 0, line: 1 }]);
    }

    #[test]
    fn a_call_between_pair_and_check_blocks_mf_dead() {
        // rgt; lft; call f; term Check — callee may read MF at entry:
        // kept (built uncoupled — the entry fact of the function).
        let f = opt_fn("f() { right; left; @g(); check(1, 2); 1: mark(!); 2: unmark(!); } g() { }");
        assert_eq!(
            f.blocks[0].ops,
            vec![
                IrOp::Rgt { line: 1 },
                IrOp::Lft { line: 1 },
                IrOp::Call {
                    name: "g".into(),
                    line: 1
                },
            ]
        );
    }

    #[test]
    fn a_brk_after_the_pair_blocks_mf_dead() {
        // Brk = observation.
        let f = opt_fn("f() { right; left; debugger; check(1, 2); 1: mark(!); 2: unmark(!); }");
        assert_eq!(
            f.blocks[0].ops,
            vec![
                IrOp::Rgt { line: 1 },
                IrOp::Lft { line: 1 },
                IrOp::Brk { line: 1 },
            ]
        );
    }

    #[test]
    fn cross_successor_mf_dead_walks_goto_chains() {
        // Pair in block A (term Goto B); B starts with a tape op:
        // eliminated.
        let f = opt_fn("f() { right; left; goto 2; 2: right; }");
        assert!(f.blocks[0].ops.is_empty());
        let target = f.blocks.iter().find(|b| b.labels == vec![2]).unwrap();
        assert_eq!(target.ops, vec![IrOp::Rgt { line: 1 }]);
    }

    #[test]
    fn lft_rgt_order_also_matches() {
        let f = opt_fn("f() { mark; left; right; check(1, 2); 1: mark(!); 2: unmark(!); }");
        assert_eq!(f.blocks[0].ops, vec![IrOp::Wr { index: 1, line: 1 }]);
    }

    #[test]
    fn a_cross_block_pair_no_longer_licenses_the_pair_it_depends_on() {
        // The circularity the final review caught: block 0's pair escapes
        // through `goto 2` into block 1, whose OWN leading `rgt` is the
        // re-latch witness `mf_dead_after` finds for block 0's pair (proof
        // 2). Block 1's own pair, in turn, only reaches `Coupled` at its
        // entry because block 0's pair ran (proof 1) — the two would
        // license each other if a single `run()` deleted both from one
        // stale pre-deletion snapshot. One call now deletes AT MOST ONE
        // pair: block 0's pair goes (still sound on its own), block 1's
        // does NOT — recomputed fresh, block 1's entry fact is `Uncoupled`
        // once block 0 is actually empty, and its own pair sits right
        // before the `check` that reads MF, so neither proof licenses it.
        // A second `run()` call converges at zero further deletions.
        let src = "\
main() {
    right;
    left;
    goto 2;
2:  right;
    left;
    check(3, !);
3:  unmark;
}
";
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        let f = &mut ir.functions[0];
        assert_eq!(run(f), 1, "exactly one pair per call");
        crate::ir::validate_function(f).unwrap();
        assert!(f.blocks[0].ops.is_empty(), "block 0's pair is gone");
        assert_eq!(
            f.blocks[1].ops,
            vec![IrOp::Rgt { line: 5 }, IrOp::Lft { line: 6 }],
            "block 1's pair MUST survive: deleting it too would leave the \
             `check` reading MF with no tape op left in the function to \
             have latched it"
        );
        assert_eq!(run(f), 0, "converged: nothing further to delete");
    }
}
