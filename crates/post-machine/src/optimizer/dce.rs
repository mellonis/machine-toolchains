//! dce (spec §8 pass 3): delete blocks unreachable from the entry.
//! Reachability-only deletion cannot dangle a reachable terminator, so
//! the closed-targets invariant is preserved by construction.

use std::collections::HashSet;

use crate::ir::{IrFunction, IrTerm};

pub fn run(f: &mut IrFunction) -> u32 {
    let index: std::collections::HashMap<u32, usize> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id, i))
        .collect();
    let mut seen = HashSet::new();
    let mut work = vec![f.blocks[0].id];
    while let Some(id) = work.pop() {
        if !seen.insert(id) {
            continue;
        }
        match f.blocks[index[&id]].term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => work.push(to),
            IrTerm::Check { marked, blank } => {
                work.push(marked);
                work.push(blank);
            }
            IrTerm::Return | IrTerm::Halt => {}
        }
    }
    let before = f.blocks.len();
    f.blocks.retain(|b| seen.contains(&b.id));
    (before - f.blocks.len()) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    #[test]
    fn unreachable_block_is_deleted_and_entry_survives() {
        let (mut ir, warnings) =
            lower(&parse(&lex("f() { goto 1; right; 1: left; }").unwrap()).unwrap()).unwrap();
        assert_eq!(warnings.len(), 1); // lowering still warns
        let f = &mut ir.functions[0];
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(run(f), 1);
        assert_eq!(f.blocks.len(), 2);
        crate::ir::validate_function(f).unwrap();
    }
}
