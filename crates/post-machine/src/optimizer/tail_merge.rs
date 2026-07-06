//! tail-merge, v1 scope: (a) whole-block dedup — semantically identical
//! blocks (same ops modulo line numbers, same terminator) collapse to
//! one, references retargeted; (b) return-chaining — a Return block
//! physically followed by an EMPTY Return block falls through to share
//! the terminal instruction (`jm Lstp; wr 1; Lstp: stp`: one `stp` serves
//! both paths). Suffix-level merging (partial tails) is a future
//! refinement. Part of the `-O1` pipeline (optimizer/mod.rs).

use crate::ir::{IrFunction, IrOp, IrTerm};

fn same_op(a: &IrOp, b: &IrOp) -> bool {
    match (a, b) {
        (IrOp::Lft { .. }, IrOp::Lft { .. })
        | (IrOp::Rgt { .. }, IrOp::Rgt { .. })
        | (IrOp::Brk { .. }, IrOp::Brk { .. }) => true,
        (IrOp::Wr { index: x, .. }, IrOp::Wr { index: y, .. }) => x == y,
        (IrOp::Call { name: x, .. }, IrOp::Call { name: y, .. }) => x == y,
        _ => false,
    }
}

fn same_block(a: &crate::ir::IrBlock, b: &crate::ir::IrBlock) -> bool {
    a.ops.len() == b.ops.len()
        && a.ops.iter().zip(&b.ops).all(|(x, y)| same_op(x, y))
        && a.term == b.term
}

pub fn run(f: &mut IrFunction) -> u32 {
    let mut changes = 0;

    // (a) dedup to the earliest identical block; the duplicate is
    // deleted immediately (all references just moved), which also keeps
    // this loop terminating.
    loop {
        let mut found: Option<(u32, u32)> = None; // (dup id, keeper id)
        'outer: for i in 0..f.blocks.len() {
            for j in (i + 1)..f.blocks.len() {
                if same_block(&f.blocks[i], &f.blocks[j]) {
                    found = Some((f.blocks[j].id, f.blocks[i].id));
                    break 'outer;
                }
            }
        }
        let Some((dup, keeper)) = found else { break };
        for b in &mut f.blocks {
            let r = |t: &mut u32| {
                if *t == dup {
                    *t = keeper;
                }
            };
            match &mut b.term {
                IrTerm::FallThrough { to } | IrTerm::Goto { to } => r(to),
                IrTerm::Check { marked, blank } => {
                    r(marked);
                    r(blank);
                }
                IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
            }
        }
        f.blocks.retain(|b| b.id != dup); // j > i ≥ 0, so never the entry
        changes += 1;
    }

    // (b) return-chaining: share the physically-next terminal.
    for i in 0..f.blocks.len().saturating_sub(1) {
        if matches!(f.blocks[i].term, IrTerm::Return)
            && f.blocks[i + 1].ops.is_empty()
            && matches!(f.blocks[i + 1].term, IrTerm::Return)
        {
            f.blocks[i].term = IrTerm::FallThrough {
                to: f.blocks[i + 1].id,
            };
            changes += 1;
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn merged(src: &str) -> crate::ir::IrFunction {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        ir.functions.remove(0)
    }

    #[test]
    fn identical_blocks_dedup_and_check_arms_converge() {
        // Arms 2 and 3 are the same code — dedup makes check(2,3) a
        // check(k,k), which check-fold will collapse next.
        let f = merged("f() { 1: check(2, 3); 2: mark, right(!); 3: mark, right(!); }");
        assert_eq!(f.blocks.len(), 2);
        let (m, b) = match &f.blocks[0].term {
            crate::ir::IrTerm::Check { marked, blank } => (*marked, *blank),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(m, b);
    }

    #[test]
    fn return_chaining_shares_the_adjacent_terminal() {
        // This pass's own module-doc example: 1: check(!, 2); 2: mark(!);
        // blocks: b0 Check{exit, b1}, b1 [wr1] Return, exit [] Return.
        let f = merged("f() { 1: check(!, 2); 2: mark(!); }");
        assert!(matches!(
            f.blocks[1].term,
            crate::ir::IrTerm::FallThrough { .. }
        ));
    }

    #[test]
    fn non_adjacent_and_non_empty_returns_stay() {
        let f = merged("f() { 1: check(2, 3); 2: mark(!); 3: unmark(!); }");
        // b1 [wr1] Ret, b2 [wr0] Ret: different ops, not empty — no merge.
        assert!(matches!(f.blocks[1].term, crate::ir::IrTerm::Return));
    }
}
