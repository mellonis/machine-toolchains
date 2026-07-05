//! jump-threading (spec §8 pass 2): a jump to an EMPTY block that only
//! jumps onward retargets to the final destination. Chains collapse in
//! one application; a cycle of empty forwarders is a deliberate infinite
//! loop (`1: goto 1;`) and is preserved untouched.

use std::collections::{HashMap, HashSet};

use crate::ir::{IrBlock, IrFunction, IrTerm};

fn forwards_to(b: &IrBlock) -> Option<u32> {
    if b.ops.is_empty()
        && let IrTerm::Goto { to } | IrTerm::FallThrough { to } = b.term
    {
        Some(to)
    } else {
        None
    }
}

pub fn run(f: &mut IrFunction) -> u32 {
    let forward: HashMap<u32, u32> = f
        .blocks
        .iter()
        .filter_map(|b| forwards_to(b).map(|t| (b.id, t)))
        .collect();
    let resolve = |start: u32| -> u32 {
        let mut seen = HashSet::new();
        let mut cur = start;
        while let Some(&next) = forward.get(&cur) {
            if !seen.insert(cur) {
                return start; // cycle: preserve the loop as written
            }
            cur = next;
        }
        cur
    };

    let mut changes = 0;
    for b in &mut f.blocks {
        let mut retarget = |t: &mut u32| {
            let new = resolve(*t);
            if new != *t {
                *t = new;
                changes += 1;
            }
        };
        match &mut b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => retarget(to),
            IrTerm::Check { marked, blank } => {
                retarget(marked);
                retarget(blank);
            }
            IrTerm::Return | IrTerm::Halt => {}
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

    fn ir_of(src: &str) -> crate::ir::IrProgram {
        lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0
    }

    #[test]
    fn goto_chain_collapses_to_final_target() {
        // 1 -> 2 -> 3 -> mark: blocks 0(goto 1), 1(goto 2), 2(mark).
        let mut ir = ir_of("f() { goto 1; 1: goto 2; 2: goto 3; 3: mark; }");
        let f = &mut ir.functions[0];
        assert!(run(f) > 0);
        // Entry now targets the mark block directly.
        assert_eq!(f.blocks[0].term, IrTerm::Goto { to: 3 });
    }

    #[test]
    fn empty_self_loop_is_preserved() {
        let mut ir = ir_of("f() { 1: goto 1; }");
        assert_eq!(run(&mut ir.functions[0]), 0);
        assert_eq!(ir.functions[0].blocks[0].term, IrTerm::Goto { to: 0 });
    }

    #[test]
    fn blocks_with_ops_are_not_threaded_through() {
        let mut ir = ir_of("f() { goto 1; 1: mark(2); 2: left; }");
        assert_eq!(run(&mut ir.functions[0]), 0);
    }
}
