//! cell-state: the historic redundant mark/unmark elimination,
//! generalized to `wr`. Part of the `-O1` pipeline (optimizer/mod.rs).
//! Two rules, both MF-safe by the coupling invariant (see dataflow module
//! docs):
//!
//! 1. Idempotent write: `wr i` when the cell provably holds `i` on a
//!    COUPLED path — the write changes neither the tape nor MF (both
//!    the skipped latch and the current MF equal `i == 1`).
//! 2. Block-local dead store: a `wr` overwritten by a later `wr` in the
//!    same block with nothing in between that could observe the value —
//!    moves make it tape-visible, `call` may read it, `brk` is an
//!    observability barrier, and MF observation only happens at the
//!    terminator (after the last write re-latches).

use crate::ir::{IrFunction, IrOp};
use crate::optimizer::dataflow;

pub fn run(f: &mut IrFunction) -> u32 {
    let entries = dataflow::block_entry_facts(f);
    let mut changes = 0u32;
    for b in &mut f.blocks {
        // Unreachable blocks have no entry fact; they are dce's job.
        let Some(&entry_fact) = entries.get(&b.id) else {
            continue;
        };

        // Rule 1: idempotent writes.
        let mut fact = entry_fact;
        let mut kept: Vec<IrOp> = Vec::with_capacity(b.ops.len());
        for op in std::mem::take(&mut b.ops) {
            if let IrOp::Wr { index, .. } = &op
                && fact.cell() == Some(*index)
            {
                changes += 1;
                continue;
            }
            fact = dataflow::transfer_op(fact, &op);
            kept.push(op);
        }

        // Rule 2: dead stores.
        let mut dead: Vec<usize> = Vec::new();
        let mut pending: Option<usize> = None;
        for (i, op) in kept.iter().enumerate() {
            match op {
                IrOp::Wr { .. } => {
                    if let Some(p) = pending {
                        dead.push(p);
                    }
                    pending = Some(i);
                }
                // A fused write+move ends the window like a bare move: the
                // move makes the pre-move cell tape-visible and shifts the
                // head. Fully conservative — its own pre-move write is never
                // tracked as a droppable pending store.
                IrOp::Lft { .. }
                | IrOp::Rgt { .. }
                | IrOp::WrLft { .. }
                | IrOp::WrRgt { .. }
                | IrOp::Call { .. }
                | IrOp::Brk { .. } => {
                    pending = None;
                }
            }
        }
        if !dead.is_empty() {
            changes += dead.len() as u32;
            let mut i = 0usize;
            kept.retain(|_| {
                let drop = dead.contains(&i);
                i += 1;
                !drop
            });
        }
        b.ops = kept;
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrOp, lower};
    use crate::lexer::lex;
    use crate::parser::parse;

    fn opt_fn(src: &str) -> crate::ir::IrFunction {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        while run(&mut ir.functions[0]) > 0 {}
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        ir.functions.remove(0)
    }

    #[test]
    fn double_mark_keeps_one_write() {
        let f = opt_fn("f() { mark; mark; }");
        assert_eq!(f.blocks[0].ops.len(), 1);
    }

    #[test]
    fn overwritten_write_is_a_dead_store() {
        let f = opt_fn("f() { mark, unmark; }");
        assert_eq!(f.blocks[0].ops, vec![IrOp::Wr { index: 0, line: 1 }]);
    }

    #[test]
    fn check_arm_knowledge_kills_the_confirming_write() {
        // On the marked edge the cell provably holds 1 — `mark` is a no-op.
        let f = opt_fn("f() { right; check(1, 2); 1: mark(!); 2: unmark; }");
        let marked_block = f.blocks.iter().find(|b| b.labels == vec![1]).unwrap();
        assert!(marked_block.ops.is_empty());
    }

    #[test]
    fn moves_calls_and_brk_protect_writes() {
        let f = opt_fn("f() { mark; right; }");
        assert_eq!(f.blocks[0].ops.len(), 2); // move makes the value visible
        let f = opt_fn("f() { mark; debugger; mark; }");
        assert_eq!(f.blocks[0].ops.len(), 3); // barrier: nothing dropped
        let f = opt_fn("f() { mark; @g(); mark; }");
        assert_eq!(f.blocks[0].ops.len(), 3); // call may observe/clobber
    }

    #[test]
    fn uncoupled_entry_never_licenses_a_drop() {
        // No tape op before `mark`: cell unknown, write must stay.
        let f = opt_fn("f() { mark; }");
        assert_eq!(f.blocks[0].ops.len(), 1);
    }
}
