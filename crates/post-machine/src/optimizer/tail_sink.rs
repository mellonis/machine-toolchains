//! tail-sink: when two arms of a `check` join back into the same
//! successor, a common OP SUFFIX shared by both arms sinks past the join —
//! it moves out of each arm and becomes a shared prefix of the join block
//! instead. Part of the `-O1` pipeline (optimizer/mod.rs), registered
//! after branch-fold and before tail-call.
//!
//! # Sinking, not hoisting
//!
//! This is the suffix-level dual of tail-merge's whole-block dedup
//! (tail_merge.rs): where tail-merge collapses two blocks that are
//! identical END TO END, tail-sink handles the more common partial case —
//! two arms differ at the front (the code that made them worth branching
//! on in the first place) but converge on identical trailing work. Only
//! the SUFFIX moves; nothing sinks past a point where the arms still
//! disagree, because that would require choosing which arm's now-earlier
//! ops to keep. The move is one-directional (down into the join, never up
//! into a predecessor) — a join can have many predecessors in general, but
//! this pass fires only on the exactly-two-jump-preds shape, so "the
//! join's new prefix" always has an unambiguous single home to move to.
//!
//! # Why identical ops may share one copy: the dynamic-identity argument
//!
//! `same_op` (reused from tail_merge.rs, made `pub(super)` there) compares
//! ops by instruction identity — same opcode, same operand — ignoring
//! source line. Two ops that are the same instruction perform the exact
//! same transaction regardless of which arm's control-flow history led to
//! them: a `wr 1` writes 1 whether MF took the marked or the blank path.
//! So replacing two static copies with one dynamic execution changes
//! nothing about what runs — every path still performs the identical
//! sequence of accesses, in the identical order, exactly as before. Only
//! the STATIC copy count shrinks (fewer emitted instructions); the
//! DYNAMIC access sequence on any one run is untouched.
//!
//! # Volatile-clean
//!
//! Because no access is added, dropped, merged, split, or reordered on any
//! executed path, tail-sink stays outside `gated_pass_names()`
//! (optimizer/mod.rs) — it runs the same in the volatile column as the
//! normal one. This is the same standard the other clean passes are held
//! to: `check-fold`/`jump-threading`/`tail-call`/`tail-merge` rewire
//! control flow between accesses without touching the accesses themselves,
//! and `dce`/`inline` delete or splice code without changing what a live
//! path executes. Tail-sink's own move is symmetric with tail-merge's:
//! both relocate already-identical code to run once instead of twice.
//!
//! # Fall-through layout is untouched
//!
//! This pass only moves `IrOp`s between existing blocks' `ops` vectors —
//! it never adds, removes, reorders, or retargets a block or a terminator.
//! So the physical block order codegen lays out from (docs/pmt/optimizer.md
//! (tail sinking); the layout invariant itself is documented at
//! crates/post-machine/src/codegen.rs) is exactly as it was before this
//! pass ran, and which terminators codegen elides as fall-through is
//! decided the same way it always was.
//!
//! # The join shape this pass recognizes
//!
//! A block J qualifies as a join when it has exactly two PREDECESSORS that
//! reach it by a `Goto` or `FallThrough` terminator (a "jump" edge), those
//! two predecessors are distinct from each other and from J itself (no
//! self-loop), and NO OTHER edge reaches J — not a `Check` arm, not being
//! the function's entry (called from outside the function, modeled as one
//! "other" edge on the entry block's own id so it can never qualify as a
//! join of its own predecessors). A third way to reach J — even a `Check`
//! arm that never fires at runtime — means some execution could observe J
//! without having executed the sunk suffix from A or B, so it must be
//! ruled out statically, not just dynamically.
//!
//! Given a qualifying J with jump predecessors A and B, the pass compares
//! A's and B's op lists from the end (`common_suffix_len`, driven by
//! `same_op`), stopping at any `Brk` on either side — a breakpoint is an
//! observability barrier (optimizer/mod.rs's equivalence contract) and the
//! op's exact block-relative position is part of what a debugger attached
//! there sees, so no motion may cross it even when both arms carry the
//! identical `Brk`. A run of at least two matching ops sinks: it's cut
//! from the tail of both A and B and spliced onto the front of J's ops.
//!
//! Only one join sinks per call; the fixpoint driver in optimizer/mod.rs
//! reruns the pipeline to convergence, so a chain of joins (or a deeper
//! shared suffix exposed by an earlier sink) is handled by successive
//! calls rather than one pass trying to do it all at once.

use std::collections::HashMap;

use super::tail_merge::same_op;
use crate::ir::{IrFunction, IrOp, IrTerm};

/// The length of the shared suffix of `a` and `b`, comparing from the end
/// with `same_op`. Stops (without counting the pair at that position) as
/// soon as either side's op at the current position is a `Brk` — a
/// breakpoint never sinks, even when both arms carry an identical one, and
/// nothing before it may sink either, since a debugger attached there must
/// still see it in its own arm.
fn common_suffix_len(a: &[IrOp], b: &[IrOp]) -> usize {
    let mut n = 0;
    while n < a.len() && n < b.len() {
        let x = &a[a.len() - 1 - n];
        let y = &b[b.len() - 1 - n];
        if matches!(x, IrOp::Brk { .. }) || matches!(y, IrOp::Brk { .. }) {
            break;
        }
        if !same_op(x, y) {
            break;
        }
        n += 1;
    }
    n
}

pub fn run(f: &mut IrFunction) -> u32 {
    let entry_id = f.blocks[0].id;
    let index: HashMap<u32, usize> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id, i))
        .collect();

    // Predecessor census, built in one walk over terminators. A jump pred
    // (Goto/FallThrough) is tracked by id per target; a Check edge or the
    // entry's own implicit external caller is tracked as an "other" edge
    // count per target — either disqualifies that target as a join.
    let mut jump_preds: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut other_edges: HashMap<u32, u32> = HashMap::new();
    other_edges.insert(entry_id, 1); // the function's own entry point counts as one "other" edge on itself, so blocks[0] never qualifies as a join.
    for b in &f.blocks {
        match &b.term {
            IrTerm::Goto { to } | IrTerm::FallThrough { to } => {
                jump_preds.entry(*to).or_default().push(b.id);
            }
            IrTerm::Check { marked, blank } => {
                *other_edges.entry(*marked).or_insert(0) += 1;
                *other_edges.entry(*blank).or_insert(0) += 1;
            }
            IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
        }
    }

    let mut found: Option<(usize, usize, usize, usize)> = None; // (a block idx, b block idx, join block idx, suffix len)
    for j in &f.blocks {
        let j_id = j.id;
        if other_edges.get(&j_id).copied().unwrap_or(0) != 0 {
            continue;
        }
        let Some(preds) = jump_preds.get(&j_id) else {
            continue;
        };
        if preds.len() != 2 {
            continue;
        }
        let (a_id, b_id) = (preds[0], preds[1]);
        if a_id == b_id || a_id == j_id || b_id == j_id {
            continue;
        }
        let (Some(&ai), Some(&bi), Some(&ji)) =
            (index.get(&a_id), index.get(&b_id), index.get(&j_id))
        else {
            continue;
        };
        let n = common_suffix_len(&f.blocks[ai].ops, &f.blocks[bi].ops);
        if n >= 2 {
            found = Some((ai, bi, ji, n));
            break;
        }
    }

    let Some((ai, bi, ji, n)) = found else {
        return 0;
    };

    let a_len = f.blocks[ai].ops.len();
    let suffix: Vec<IrOp> = f.blocks[ai].ops[a_len - n..].to_vec();
    f.blocks[ai].ops.truncate(a_len - n);
    let b_len = f.blocks[bi].ops.len();
    f.blocks[bi].ops.truncate(b_len - n);
    f.blocks[ji].ops.splice(0..0, suffix);
    n as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn sunk(src: &str) -> crate::ir::IrFunction {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        ir.functions.remove(0)
    }

    fn block_labeled(f: &crate::ir::IrFunction, label: u32) -> &crate::ir::IrBlock {
        f.blocks
            .iter()
            .find(|b| b.labels == vec![label])
            .unwrap_or_else(|| panic!("no block labeled {label}"))
    }

    #[test]
    fn identical_two_op_suffixes_sink_past_the_join() {
        // A: [Wr 1, Rgt, Rgt] -> Goto J; B: [Lft, Rgt, Rgt] -> Goto J
        // (fallthrough); J has exactly these two preds. After: A [Wr 1],
        // B [Lft], J.ops starts [Rgt, Rgt].
        let src =
            "f() { check(1, 2); 1: mark; right; right; goto 3; 2: left; right; right; 3: unmark; }";
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        let n = run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        let f = &ir.functions[0];
        assert_eq!(n, 2);
        assert_eq!(
            block_labeled(f, 1).ops,
            vec![IrOp::Wr { index: 1, line: 1 }]
        );
        assert_eq!(block_labeled(f, 2).ops, vec![IrOp::Lft { line: 1 }]);
        assert_eq!(
            block_labeled(f, 3).ops,
            vec![
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Wr { index: 0, line: 1 },
            ]
        );
    }

    #[test]
    fn a_one_op_suffix_is_below_threshold() {
        let src = "f() { check(1, 2); 1: mark; right; goto 3; 2: left; right; 3: unmark; }";
        let f = sunk(src);
        assert_eq!(
            block_labeled(&f, 1).ops,
            vec![IrOp::Wr { index: 1, line: 1 }, IrOp::Rgt { line: 1 }]
        );
        assert_eq!(
            block_labeled(&f, 2).ops,
            vec![IrOp::Lft { line: 1 }, IrOp::Rgt { line: 1 }]
        );
        assert_eq!(
            block_labeled(&f, 3).ops,
            vec![IrOp::Wr { index: 0, line: 1 }]
        );
    }

    #[test]
    fn a_brk_stops_the_upward_scan() {
        // Suffix [Brk, Rgt, Rgt] on both arms: only [Rgt, Rgt] sinks.
        let src = "f() { check(1, 2); 1: debugger; right; right; goto 3; 2: debugger; right; right; 3: unmark; }";
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        let n = run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        let f = &ir.functions[0];
        assert_eq!(n, 2);
        assert_eq!(block_labeled(f, 1).ops, vec![IrOp::Brk { line: 1 }]);
        assert_eq!(block_labeled(f, 2).ops, vec![IrOp::Brk { line: 1 }]);
        assert_eq!(
            block_labeled(f, 3).ops,
            vec![
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Wr { index: 0, line: 1 },
            ]
        );
    }

    #[test]
    fn a_third_predecessor_blocks_sinking() {
        // J also reachable from a Check edge: nothing moves.
        let src = "f() { check(4, 3); 4: check(1, 2); 1: mark; right; right; goto 3; 2: left; right; right; goto 3; 3: unmark; }";
        let f = sunk(src);
        assert_eq!(
            block_labeled(&f, 1).ops,
            vec![
                IrOp::Wr { index: 1, line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
            ]
        );
        assert_eq!(
            block_labeled(&f, 2).ops,
            vec![
                IrOp::Lft { line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
            ]
        );
        assert_eq!(
            block_labeled(&f, 3).ops,
            vec![IrOp::Wr { index: 0, line: 1 }]
        );
    }

    #[test]
    fn the_entry_block_never_gains_a_prefix() {
        // J == blocks[0]: skip, even though it otherwise looks like a
        // qualifying join (two distinct jump preds, matching suffix).
        let src =
            "f() { 1: check(2, 3); 2: mark; right; right; goto 1; 3: left; right; right; goto 1; }";
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        let n = run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        let f = &ir.functions[0];
        assert_eq!(n, 0);
        assert!(f.blocks[0].ops.is_empty());
        assert_eq!(
            block_labeled(f, 2).ops,
            vec![
                IrOp::Wr { index: 1, line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
            ]
        );
        assert_eq!(
            block_labeled(f, 3).ops,
            vec![
                IrOp::Lft { line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
            ]
        );
    }

    #[test]
    fn self_loop_join_is_skipped() {
        // A == J: block 2's own self-loop counts as a jump pred of itself,
        // disqualifying it even though the other pred's suffix matches.
        let src = "f() { check(1, 3); 1: mark; right; right; goto 2; 2: left; right; right; goto 2; 3: unmark; }";
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        let n = run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        let f = &ir.functions[0];
        assert_eq!(n, 0);
        assert_eq!(
            block_labeled(f, 1).ops,
            vec![
                IrOp::Wr { index: 1, line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
            ]
        );
        assert_eq!(
            block_labeled(f, 2).ops,
            vec![
                IrOp::Lft { line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
            ]
        );
    }
}
