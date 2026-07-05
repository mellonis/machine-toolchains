//! check-fold (spec §8 pass 1): a check with identical arms decides
//! nothing — replace with an unconditional goto. The single-arm jm/jnm
//! specialization is codegen's adjacency selection, not an IR rewrite.

use crate::ir::{IrFunction, IrTerm};

pub fn run(f: &mut IrFunction) -> u32 {
    let mut changes = 0;
    for b in &mut f.blocks {
        if let IrTerm::Check { marked, blank } = b.term
            && marked == blank
        {
            b.term = IrTerm::Goto { to: marked };
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

    #[test]
    fn identical_arms_fold_to_goto() {
        let (mut ir, _) = lower(&parse(&lex("f() { 1: check(1, 1); }").unwrap()).unwrap()).unwrap();
        assert_eq!(run(&mut ir.functions[0]), 1);
        assert_eq!(ir.functions[0].blocks[0].term, IrTerm::Goto { to: 0 });
    }

    #[test]
    fn distinct_arms_untouched() {
        let (mut ir, _) = lower(&parse(&lex("f() { 1: check(1, !); }").unwrap()).unwrap()).unwrap();
        assert_eq!(run(&mut ir.functions[0]), 0);
    }
}
