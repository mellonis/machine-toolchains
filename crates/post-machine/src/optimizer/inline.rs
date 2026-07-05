//! inline (spec §8 pass 5): splice small leaf callees into their call
//! sites, intra-module. Dissolving the call barrier is what unlocks the
//! dataflow across it — the other passes then see through the old
//! boundary. Candidates never contain `brk` (inlining would erase the
//! call frame a debugger shows) and never contain calls of their own.

use std::collections::HashMap;

use crate::ir::{IrBlock, IrFunction, IrOp, IrProgram, IrTerm};

const INLINE_MAX_OPS: usize = 6;

fn is_leaf_without_brk(f: &IrFunction) -> bool {
    f.blocks.iter().all(|b| {
        !matches!(b.term, IrTerm::TailCall { .. })
            && b.ops
                .iter()
                .all(|op| !matches!(op, IrOp::Call { .. } | IrOp::Brk { .. }))
    })
}

fn op_count(f: &IrFunction) -> usize {
    f.blocks.iter().map(|b| b.ops.len()).sum()
}

pub fn run(ir: &mut IrProgram) -> u32 {
    // Candidate set is fixed from the pre-pass program state.
    let mut call_counts: HashMap<&str, u32> = HashMap::new();
    for f in &ir.functions {
        for b in &f.blocks {
            for op in &b.ops {
                if let IrOp::Call { name, .. } = op {
                    *call_counts.entry(name.as_str()).or_insert(0) += 1;
                }
            }
            if let IrTerm::TailCall { name } = &b.term {
                *call_counts.entry(name.as_str()).or_insert(0) += 1;
            }
        }
    }
    let candidates: HashMap<String, IrFunction> = ir
        .functions
        .iter()
        .filter(|f| {
            // main's Return means stp — splicing it would erase the machine stop.
            f.name != "main"
                && is_leaf_without_brk(f)
                && (op_count(f) <= INLINE_MAX_OPS
                    || call_counts.get(f.name.as_str()).copied().unwrap_or(0) == 1)
        })
        .map(|f| (f.name.clone(), f.clone()))
        .collect();

    let mut changes = 0;
    for f in &mut ir.functions {
        while let Some((bi, oi)) = find_site(f, &candidates) {
            splice(f, bi, oi, &candidates);
            changes += 1;
        }
    }
    changes
}

fn find_site(f: &IrFunction, candidates: &HashMap<String, IrFunction>) -> Option<(usize, usize)> {
    for (bi, b) in f.blocks.iter().enumerate() {
        for (oi, op) in b.ops.iter().enumerate() {
            if let IrOp::Call { name, .. } = op
                && name != &f.name
                && candidates.contains_key(name)
            {
                return Some((bi, oi));
            }
        }
    }
    None
}

fn splice(f: &mut IrFunction, bi: usize, oi: usize, candidates: &HashMap<String, IrFunction>) {
    let next_id = f.blocks.iter().map(|b| b.id).max().unwrap_or(0) + 1;
    let IrOp::Call { name, line } = f.blocks[bi].ops[oi].clone() else {
        unreachable!("find_site returned a call site")
    };
    let callee = &candidates[&name];

    // Split the site block: ops after the call + the original terminator
    // move to a fresh continuation block.
    let tail_ops = f.blocks[bi].ops.split_off(oi + 1);
    f.blocks[bi].ops.pop(); // the call itself
    let cont_id = next_id;
    let mut id_map: HashMap<u32, u32> = HashMap::new();
    for (k, cb) in callee.blocks.iter().enumerate() {
        id_map.insert(cb.id, next_id + 1 + k as u32);
    }
    let cont = IrBlock {
        id: cont_id,
        labels: vec![],
        line,
        ops: tail_ops,
        term: f.blocks[bi].term.clone(),
        term_line: f.blocks[bi].term_line,
    };
    f.blocks[bi].term = IrTerm::Goto {
        to: id_map[&callee.blocks[0].id],
    };
    f.blocks[bi].term_line = line;

    let clones: Vec<IrBlock> = callee
        .blocks
        .iter()
        .map(|cb| {
            let mut nb = cb.clone();
            nb.id = id_map[&cb.id];
            nb.labels = vec![]; // callee label names are meaningless here
            nb.term = match &cb.term {
                IrTerm::FallThrough { to } => IrTerm::FallThrough { to: id_map[to] },
                IrTerm::Goto { to } => IrTerm::Goto { to: id_map[to] },
                IrTerm::Check { marked, blank } => IrTerm::Check {
                    marked: id_map[marked],
                    blank: id_map[blank],
                },
                // The callee's return continues after the call site.
                IrTerm::Return => IrTerm::Goto { to: cont_id },
                IrTerm::Halt => IrTerm::Halt,
                IrTerm::TailCall { .. } => unreachable!("candidates are leaves"),
            };
            nb
        })
        .collect();

    // Insertion order: callee body, then continuation, right after the
    // site block — preserves fall-through layout quality.
    let mut at = bi + 1;
    for c in clones {
        f.blocks.insert(at, c);
        at += 1;
    }
    f.blocks.insert(at, cont);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn inlined(src: &str) -> IrProgram {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        run(&mut ir);
        for f in &ir.functions {
            crate::ir::validate_function(f).unwrap();
        }
        ir
    }

    #[test]
    fn small_leaf_is_spliced_and_the_call_disappears() {
        let ir = inlined("f() { right; } main() { @f(); mark; }");
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        assert!(
            main.blocks
                .iter()
                .all(|b| b.ops.iter().all(|op| !matches!(op, IrOp::Call { .. })))
        );
        // site block + callee clone + continuation = 3 blocks.
        assert_eq!(main.blocks.len(), 3);
    }

    #[test]
    fn brk_and_non_leaf_callees_are_never_inlined() {
        let ir = inlined("f() { debugger; right; } main() { @f(); }");
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        assert!(
            main.blocks[0]
                .ops
                .iter()
                .any(|op| matches!(op, IrOp::Call { .. }))
        );

        let ir = inlined("f() { @g(); } g() { right; } main() { @f(); }");
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        // f calls g → f is no leaf; the call to f survives. (g gets
        // inlined INTO f, which is fine — candidates come from the
        // pre-pass state where f was not a leaf.)
        assert!(
            main.blocks[0]
                .ops
                .iter()
                .any(|op| matches!(op, IrOp::Call { name, .. } if name == "f"))
        );
    }

    #[test]
    fn recursion_is_never_inlined() {
        let ir = inlined("f() { @f(); } main() { @f(); }");
        // f is not a leaf (calls itself) → nothing inlines anywhere.
        for f in &ir.functions {
            let calls: usize = f
                .blocks
                .iter()
                .map(|b| {
                    b.ops
                        .iter()
                        .filter(|op| matches!(op, IrOp::Call { .. }))
                        .count()
                })
                .sum();
            assert_eq!(calls, 1, "{}", f.name);
        }
    }

    #[test]
    fn single_call_site_admits_a_large_callee() {
        // 8 ops > INLINE_MAX_OPS, but exactly one call site module-wide.
        let ir = inlined(
            "big() { right; right; right; right; left; left; left; left; } main() { @big(); }",
        );
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        assert!(
            main.blocks
                .iter()
                .all(|b| b.ops.iter().all(|op| !matches!(op, IrOp::Call { .. })))
        );
    }

    #[test]
    fn check_arms_inside_the_callee_are_remapped() {
        let ir = inlined("f() { 1: right; check(1, 2); 2: left; } main() { @f(); mark; }");
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        crate::ir::validate_function(main).unwrap(); // remapped targets resolve
        assert!(
            main.blocks
                .iter()
                .any(|b| matches!(b.term, IrTerm::Check { .. }))
        );
    }

    #[test]
    fn main_is_never_an_inline_candidate() {
        // Splicing main would rewrite its stp-Return into a Goto (final
        // review I2): "stop the machine" must not become "keep running".
        let ir = inlined("main() { right; } f() { @main(); left; }");
        let f = ir.functions.iter().find(|f| f.name == "f").unwrap();
        assert!(
            f.blocks[0]
                .ops
                .iter()
                .any(|op| matches!(op, IrOp::Call { name, .. } if name == "main"))
        );
    }
}
