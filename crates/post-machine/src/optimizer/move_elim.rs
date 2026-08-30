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
//! must each be computed FRESH, over the CURRENT ops, immediately before
//! every deletion. A stale set of facts reused across several deletions
//! risks a cross-block circularity: two pairs in different blocks could
//! license each other off a snapshot neither of their deletions has
//! actually happened against yet — pair A deleted as MF-dead, relying on
//! pair B's own leading op as the re-latch witness; pair B deleted as
//! already-`Coupled`, where that coupling fact was produced by pair A.
//! Each proof holds against the pre-deletion IR in isolation, but
//! applying both off the SAME stale scan would leave neither witness
//! standing. So a single SCAN (`find_first_sound_pair`) licenses only the
//! first sound pair it finds, in block then position order, and stops
//! there. `run()` loops that scan-then-delete step to a local fixpoint,
//! recomputing facts from the current IR before every scan, so every
//! decision is always checked against the IR as it actually stands. This
//! converges an unbounded number of independent pairs within ONE call to
//! `run()` — the crate-level fixpoint driver in optimizer/mod.rs no
//! longer spends a round per pair, only the usual closing round that
//! finds nothing further to do.
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

/// One scan over the CURRENT ops: the first sound deletion, in block then
/// position order, or `None` if a full scan finds nothing to delete. See
/// the module doc for why a single scan may license only one pair (the
/// cross-block circularity a batch of deletions checked against one stale
/// snapshot risks).
fn find_first_sound_pair(f: &IrFunction) -> Option<(usize, usize)> {
    let entry_facts = block_entry_facts(f);
    let index: HashMap<u32, usize> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id, i))
        .collect();

    for (bi, b) in f.blocks.iter().enumerate() {
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
                    return Some((bi, i));
                }
            }
            fact = transfer_op(fact, &b.ops[i]);
            i += 1;
        }
    }
    None
}

pub fn run(f: &mut IrFunction) -> u32 {
    let mut changes = 0;
    // Recompute facts on the CURRENT cfg every iteration — see the
    // module doc: a scan's decisions must never rest on a snapshot a
    // sibling deletion in this same call has already invalidated.
    while let Some((bi, i)) = find_first_sound_pair(f) {
        f.blocks[bi].ops.drain(i..=i + 1);
        changes += 1;
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::parser::parse;

    fn opt_fn(src: &str) -> IrFunction {
        let mut ir = lower(&parse(src).unwrap()).unwrap().0;
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
    fn many_independent_pairs_all_converge_within_one_call() {
        // Twelve independent, sound pairs — each one licensed purely by
        // its OWN preceding `mark` (proof 1, `Coupled`), with no
        // dependency on any sibling pair's deletion: a move resets the
        // dataflow's known-cell fact to `Coupled(None)` (dataflow.rs), so
        // cell-state's idempotent-write rule never fires between them
        // either, leaving exactly move-elim as the pass that can act
        // here. More pairs than MAX_ROUNDS (optimizer/mod.rs) has rounds
        // — the old one-pair-per-call discipline could delete at most
        // MAX_ROUNDS of these across the whole fixpoint driver (exactly
        // ten, since every other pass here makes zero changes and never
        // extends the round count), always leaving two behind at the
        // round cap. One `run()` call must delete every one of them on
        // its own.
        let mut src = String::from("main() {\n");
        for _ in 0..12 {
            src.push_str("    mark; right; left;\n");
        }
        src.push_str("}\n");
        let mut ir = lower(&parse(&src).unwrap()).unwrap().0;
        let f = &mut ir.functions[0];
        assert_eq!(run(f), 12, "one call eliminates all twelve pairs");
        crate::ir::validate_function(f).unwrap();
        let expected: Vec<IrOp> = (2..=13).map(|line| IrOp::Wr { index: 1, line }).collect();
        assert_eq!(f.blocks[0].ops, expected);
    }

    #[test]
    fn compiled_at_o1_has_no_remaining_inverse_pairs() {
        // End-to-end pin, through the full pipeline rather than an
        // isolated `run()` call: the twelve pairs from the test above are
        // gone after move-elim's own work, not just from the FINAL fused
        // output. Checking only the final IR is too weak a pin —
        // fuse-tape-ops (which runs right after move-elim in the same
        // round) folds any surviving `wr; rgt` into `wrr`, which
        // `is_inverse_pair` does not match, so a leftover, unconverged
        // pair would silently read as "gone" once fused rather than
        // failing the check. Reading the LAST `after:move-elim` snapshot
        // (`capture_ir`, the `--emit-ir` backing) inspects the op stream
        // exactly as move-elim itself left it, before fuse-tape-ops gets
        // a chance to mask anything.
        let mut src = String::from("main() {\n");
        for _ in 0..12 {
            src.push_str("    mark; right; left;\n");
        }
        src.push_str("}\n");
        let out = crate::compiler::compile(
            &src,
            crate::compiler::CompileOptions {
                opt_level: super::super::OptLevel::O1,
                capture_ir: true,
                ..Default::default()
            },
        )
        .expect("compiles");
        let (_, after_move_elim) = out
            .ir_snapshots
            .iter()
            .rev()
            .find(|(stage, _)| stage == "after:move-elim")
            .expect("move-elim made at least one change");
        for f in &after_move_elim.functions {
            for b in &f.blocks {
                for w in b.ops.windows(2) {
                    assert!(
                        !is_inverse_pair(&w[0], &w[1]),
                        "inverse pair survived move-elim's own work in `{}`: {w:?}",
                        f.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_cross_block_pair_no_longer_licenses_the_pair_it_depends_on() {
        // The circularity the final review caught: block 0's pair escapes
        // through `goto 2` into block 1, whose OWN leading `rgt` is the
        // re-latch witness `mf_dead_after` finds for block 0's pair (proof
        // 2). Block 1's own pair, in turn, only reaches `Coupled` at its
        // entry because block 0's pair ran (proof 1) — the two would
        // license each other if a single SCAN found both sound off one
        // stale pre-deletion snapshot.
        //
        // `run()` now converges within this one call, but the trace is the
        // same as it was under the old one-scan-per-call driver, just
        // internal to a single call instead of spread across two calls to
        // `run()`: scan 1 finds block 0's pair sound (still sound on its
        // own) and deletes it; the loop recomputes facts on the now-empty
        // block 0 and scans again — block 1's entry fact is `Uncoupled`
        // once block 0 is actually empty, and its own pair sits right
        // before the `check` that reads MF, so neither proof licenses it;
        // scan 2 finds nothing and the loop stops. So one `run()` call
        // returns 1 (not 2), and a further call returns 0 — the FINAL
        // state (block 0 empty, block 1's pair surviving) is identical to
        // what the old shape produced across two separate `run()` calls.
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
        let mut ir = lower(&parse(src).unwrap()).unwrap().0;
        let f = &mut ir.functions[0];
        assert_eq!(
            run(f),
            1,
            "one call converges: only the one legitimately-sound pair goes"
        );
        crate::ir::validate_function(f).unwrap();
        assert!(f.blocks[0].ops.is_empty(), "block 0's pair is gone");
        assert_eq!(
            f.blocks[1].ops,
            vec![IrOp::Rgt { line: 5 }, IrOp::Lft { line: 6 }],
            "block 1's pair MUST survive: deleting it too would leave the \
             `check` reading MF with no tape op left in the function to \
             have latched it"
        );
        assert_eq!(
            run(f),
            0,
            "already converged: a further call deletes nothing"
        );
    }
}
