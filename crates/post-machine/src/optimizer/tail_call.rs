//! tail-call (spec §8 pass 8): a call in tail position emits `jmp` to
//! the callee's `ent` (legal for jumps) instead of `call` + `ret` —
//! saves a stack slot and the return trip. Never applied in `main`,
//! whose return is `stp`: the callee's `ret` would underflow.

use crate::ir::{IrFunction, IrOp, IrTerm};

pub fn run(f: &mut IrFunction) -> u32 {
    if f.name == "main" {
        return 0;
    }
    let mut changes = 0;
    for b in &mut f.blocks {
        if matches!(b.term, IrTerm::Return) && matches!(b.ops.last(), Some(IrOp::Call { .. })) {
            let Some(IrOp::Call { name, .. }) = b.ops.pop() else {
                unreachable!("just matched a trailing call")
            };
            b.term = IrTerm::TailCall { name };
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

    fn tc(src: &str) -> crate::ir::IrProgram {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        for f in &mut ir.functions {
            run(f);
            crate::ir::validate_function(f).unwrap();
        }
        ir
    }

    #[test]
    fn trailing_call_becomes_a_tail_jump() {
        let ir = tc("g() { left; @f(!); }");
        let b = &ir.functions[0].blocks[0];
        assert_eq!(b.ops.len(), 1); // the call op is gone
        assert_eq!(b.term, IrTerm::TailCall { name: "f".into() });
    }

    #[test]
    fn implicit_return_after_call_also_converts() {
        let ir = tc("g() { @f(); }"); // falls off the end
        assert_eq!(
            ir.functions[0].blocks[0].term,
            IrTerm::TailCall { name: "f".into() }
        );
    }

    #[test]
    fn main_is_exempt_and_non_tail_calls_survive() {
        let ir = tc("main() { @f(!); }");
        assert!(matches!(ir.functions[0].blocks[0].term, IrTerm::Return));
        let ir = tc("g() { @f(); left; }"); // call not in tail position
        assert_eq!(ir.functions[0].blocks[0].ops.len(), 2);
    }
}
