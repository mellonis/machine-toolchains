//! branch-fold (spec §8 pass 6): a `check` whose MF is statically known
//! goes unconditional. Sound only on coupled paths where the cell value
//! is proven (then MF == (cell == 1) by the coupling invariant); the
//! reset-MF trap (a check before any tape instruction) stays untouched
//! because such paths are `Uncoupled`.

use crate::ir::{IrFunction, IrTerm};
use crate::optimizer::dataflow;

pub fn run(f: &mut IrFunction) -> u32 {
    let entries = dataflow::block_entry_facts(f);
    let mut changes = 0;
    for b in &mut f.blocks {
        let Some(&entry_fact) = entries.get(&b.id) else {
            continue;
        };
        let mut fact = entry_fact;
        for op in &b.ops {
            fact = dataflow::transfer_op(fact, op);
        }
        if let IrTerm::Check { marked, blank } = b.term
            && let Some(sym) = fact.cell()
        {
            b.term = IrTerm::Goto {
                to: if sym == 1 { marked } else { blank },
            };
            changes += 1;
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrTerm, lower};
    use crate::lexer::lex;
    use crate::parser::parse;

    fn fold_fn(src: &str) -> crate::ir::IrFunction {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        ir.functions.remove(0)
    }

    #[test]
    fn known_written_value_decides_the_branch() {
        // wr 1 then check: marked arm (label 1) is statically taken.
        let f = fold_fn("f() { mark; check(1, 2); 1: left(!); 2: right; }");
        assert_eq!(f.blocks[0].term, IrTerm::Goto { to: 1 });
    }

    #[test]
    fn reset_mf_check_is_never_folded() {
        let f = fold_fn("f() { check(1, 2); 1: left(!); 2: right; }");
        assert!(matches!(f.blocks[0].term, IrTerm::Check { .. }));
    }

    #[test]
    fn moves_defeat_folding() {
        let f = fold_fn("f() { mark; right; check(1, 2); 1: left(!); 2: right; }");
        assert!(matches!(f.blocks[0].term, IrTerm::Check { .. }));
    }
}
