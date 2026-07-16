//! Shared forward dataflow for cell-state and branch-fold.
//!
//! # The MF-coupling invariant (soundness backbone — read before editing)
//!
//! Every PM-1 tape instruction latches MF from the cell at the resulting
//! head position (`lft`/`rgt` the destination cell, `wr` the written
//! value); nothing else latches MF or moves the head. Hence AFTER at
//! least one tape instruction, `MF == (cell_at_head == 1)` — and this
//! survives jumps, `ent`, `brk`, and whole `call`s (a callee either
//! re-establishes it with its own tape ops or disturbs neither MF nor
//! head). BEFORE any tape instruction executes, MF is the reset value 0,
//! DECOUPLED from the tape: a `check` on such a path branches on 0, not
//! on the cell. The lattice therefore tracks coupledness explicitly, and
//! check-edge refinement applies only on provably coupled paths.

use std::collections::HashMap;

use crate::ir::{IrFunction, IrOp, IrTerm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact {
    /// Some path may reach here with no tape instruction executed yet:
    /// MF may still be the reset value. No cell knowledge, no folding.
    Uncoupled,
    /// The coupling invariant holds; the symbol under the head, if known.
    Coupled(Option<u32>),
}

impl Fact {
    pub fn merge(self, other: Fact) -> Fact {
        match (self, other) {
            (Fact::Coupled(a), Fact::Coupled(b)) => Fact::Coupled(if a == b { a } else { None }),
            _ => Fact::Uncoupled,
        }
    }

    /// The symbol under the head, when provable.
    pub fn cell(self) -> Option<u32> {
        match self {
            Fact::Coupled(c) => c,
            Fact::Uncoupled => None,
        }
    }
}

pub fn transfer_op(fact: Fact, op: &IrOp) -> Fact {
    match op {
        // Moves — bare, or the tail of a fused write+move — couple MF to
        // the (unknown) destination cell; a fused op's pre-move write is
        // not the cell under the new head, so its post-op fact is identical
        // to a bare move's.
        IrOp::Lft { .. } | IrOp::Rgt { .. } | IrOp::WrLft { .. } | IrOp::WrRgt { .. } => {
            Fact::Coupled(None)
        }
        IrOp::Wr { index, .. } => Fact::Coupled(Some(*index)),
        // Opaque: callee clobbers head/cells, but preserves coupledness
        // (see module docs) — value knowledge only is lost.
        IrOp::Call { .. } => match fact {
            Fact::Coupled(_) => Fact::Coupled(None),
            Fact::Uncoupled => Fact::Uncoupled,
        },
        // Observability barrier: no fact-based elimination may reach
        // across it, so knowledge degrades; machine state is untouched,
        // so coupledness survives.
        IrOp::Brk { .. } => match fact {
            Fact::Coupled(_) => Fact::Coupled(None),
            Fact::Uncoupled => Fact::Uncoupled,
        },
    }
}

/// Entry fact for every reachable block: worklist to fixpoint. The
/// function entry is `Uncoupled` (reset MF / unknown caller history).
/// Check edges refine (marked → cell 1, blank → cell 0 — sound because
/// the PM-1 alphabet has exactly two symbols) ONLY from coupled paths.
pub fn block_entry_facts(f: &IrFunction) -> HashMap<u32, Fact> {
    let index: HashMap<u32, usize> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id, i))
        .collect();
    let mut entry: HashMap<u32, Fact> = HashMap::new();
    let entry_id = f.blocks[0].id;
    entry.insert(entry_id, Fact::Uncoupled);
    let mut work = vec![entry_id];

    while let Some(id) = work.pop() {
        let b = &f.blocks[index[&id]];
        let mut fact = entry[&id];
        for op in &b.ops {
            fact = transfer_op(fact, op);
        }
        let mut push = |target: u32, edge_fact: Fact, work: &mut Vec<u32>| {
            let merged = match entry.get(&target) {
                Some(&old) => old.merge(edge_fact),
                None => edge_fact,
            };
            if entry.get(&target) != Some(&merged) {
                entry.insert(target, merged);
                work.push(target);
            }
        };
        match &b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => push(*to, fact, &mut work),
            IrTerm::Check { marked, blank } => {
                let (m, bl) = match fact {
                    Fact::Coupled(_) => (Fact::Coupled(Some(1)), Fact::Coupled(Some(0))),
                    Fact::Uncoupled => (Fact::Uncoupled, Fact::Uncoupled),
                };
                push(*marked, m, &mut work);
                push(*blank, bl, &mut work);
            }
            IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
        }
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn facts_of(src: &str) -> (crate::ir::IrProgram, HashMap<u32, Fact>) {
        let ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        let facts = block_entry_facts(&ir.functions[0]);
        (ir, facts)
    }

    #[test]
    fn entry_is_uncoupled_and_first_check_refines_nothing() {
        // check BEFORE any tape op: reset-MF trap — edges stay Uncoupled.
        let (_, facts) = facts_of("f() { check(1, 2); 1: mark(!); 2: unmark; }");
        assert_eq!(facts[&1], Fact::Uncoupled);
        assert_eq!(facts[&2], Fact::Uncoupled);
    }

    #[test]
    fn tape_op_couples_and_check_edges_refine() {
        // rgt couples; marked edge knows cell 1, blank edge cell 0.
        let (_, facts) = facts_of("f() { right; check(1, 2); 1: mark(!); 2: unmark; }");
        assert_eq!(facts[&1], Fact::Coupled(Some(1)));
        assert_eq!(facts[&2], Fact::Coupled(Some(0)));
    }

    #[test]
    fn wr_yields_exact_knowledge_and_moves_erase_it() {
        let f = Fact::Uncoupled;
        let f = transfer_op(f, &crate::ir::IrOp::Wr { index: 1, line: 1 });
        assert_eq!(f, Fact::Coupled(Some(1)));
        let f = transfer_op(f, &crate::ir::IrOp::Rgt { line: 1 });
        assert_eq!(f, Fact::Coupled(None));
    }

    #[test]
    fn call_and_brk_degrade_but_do_not_uncouple() {
        let coupled = Fact::Coupled(Some(1));
        assert_eq!(
            transfer_op(
                coupled,
                &crate::ir::IrOp::Call {
                    name: "g".into(),
                    line: 1
                }
            ),
            Fact::Coupled(None)
        );
        assert_eq!(
            transfer_op(coupled, &crate::ir::IrOp::Brk { line: 1 }),
            Fact::Coupled(None)
        );
        assert_eq!(
            transfer_op(
                Fact::Uncoupled,
                &crate::ir::IrOp::Call {
                    name: "g".into(),
                    line: 1
                }
            ),
            Fact::Uncoupled
        );
    }

    #[test]
    fn merge_disagreement_degrades_to_unknown_value() {
        assert_eq!(
            Fact::Coupled(Some(1)).merge(Fact::Coupled(Some(0))),
            Fact::Coupled(None)
        );
        assert_eq!(
            Fact::Coupled(Some(1)).merge(Fact::Uncoupled),
            Fact::Uncoupled
        );
    }

    #[test]
    fn loop_facts_reach_fixpoint() {
        // goToEnd shape: 1: right; check(1, 2); 2: left;
        let (_, facts) = facts_of("f() { 1: right; check(1, 2); 2: left; }");
        // Block 0 is re-entered from its own marked edge: entry merges
        // Uncoupled (function entry) with Coupled(Some(1)) -> Uncoupled.
        assert_eq!(facts[&0], Fact::Uncoupled);
        // The blank edge is only reachable AFTER rgt -> refined.
        assert_eq!(facts[&1], Fact::Coupled(Some(0)));
    }

    #[test]
    fn back_edge_coupled_disagreement_merges_to_unknown() {
        // Loop header fed by entry (after `mark`: Coupled(Some(1))) and a
        // back edge (after `unmark`: Coupled(Some(0))) — must merge to
        // Coupled(None), the only nontrivial worklist re-push path.
        let (_, facts) =
            facts_of("f() { mark; 1: right; check(2, 3); 2: unmark; goto 1; 3: left; }");
        assert_eq!(facts[&1], Fact::Coupled(None));
    }
}
