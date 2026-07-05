//! Equivalence harness (spec §11): every optimizer pass is tested by
//! running -O0 and -O1 builds of the same program on the same tapes and
//! comparing observables — outcome kind, final tape, final head.

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, RunLimits, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::link;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;

fn build(src: &str, level: OptLevel) -> mtc_core::formats::executable::Executable {
    let out = compile(
        src,
        CompileOptions {
            opt_level: level,
            ..Default::default()
        },
    )
    .expect("compiles");
    link(&[out.object], &[], LinkOptions::default())
        .expect("links")
        .executable
}

fn run_tape(
    exe: &mtc_core::formats::executable::Executable,
    cells: &[bool],
    head: i64,
) -> (mtc_core::vm::Outcome, Vec<i64>, i64) {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells(cells.iter().copied(), 0, head);
    let options = RunOptions {
        limits: RunLimits {
            max_steps: Some(10_000),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = machine.run(&mut tape, options);
    (result.outcome, tape.marked_cells(), tape.head())
}

/// Assert -O0 and -O1 agree on every tape; return (o0_len, o1_len).
pub fn assert_equivalent(src: &str, tapes: &[(&[bool], i64)]) -> (usize, usize) {
    let o0 = build(src, OptLevel::O0);
    let o1 = build(src, OptLevel::O1);
    for (cells, head) in tapes {
        let r0 = run_tape(&o0, cells, *head);
        let r1 = run_tape(&o1, cells, *head);
        assert_eq!(
            r0, r1,
            "observables diverged on tape {cells:?}/{head}: {src}"
        );
    }
    (o0.code.len(), o1.code.len())
}

const TAPES: &[(&[bool], i64)] = &[
    (&[false], 0),
    (&[true], 0),
    (&[true, true, true], 0),
    (&[false, true, true], 0),
    (&[true, false, true], 1),
];

#[test]
fn check_fold_shrinks_and_preserves() {
    let src = "main() { right; check(5, 5); 5: mark; }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(o1 < o0, "fold must shrink: {o0} -> {o1}");
}

#[test]
fn jump_threading_shrinks_and_preserves() {
    // NOT a forward-adjacent chain (codegen's fall-through layout already
    // eats those at -O0 — Task-2 BLOCKED finding, controller-ratified).
    // Here the hop is backward: -O0 emits `jmp L2; wr 1; stp; L2: jmp L1`
    // (8 bytes); -O1 threads goto-2 through the empty forwarder to the
    // mark block, dce deletes the forwarder, fall-through absorbs the
    // rest: `ent, wr 1, stp` (4 bytes).
    let src = "main() { goto 2; 1: mark(!); 2: goto 1; }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert_eq!((o0, o1), (8, 4));
}

#[test]
fn dce_removes_dead_code_bytes() {
    let src = "main() { goto 9; right; left; right; left; 9: mark; }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(o1 < o0);
}

#[test]
fn empty_infinite_loop_still_loops_at_o1() {
    let src = "main() { 1: goto 1; }";
    let o1 = build(src, OptLevel::O1);
    let (outcome, _, _) = run_tape(&o1, &[true], 0);
    assert!(
        matches!(
            outcome,
            mtc_core::vm::Outcome::Trapped(mtc_core::vm::Trap::StepLimit)
        ),
        "the loop must survive optimization: {outcome:?}"
    );
}

#[test]
fn cell_state_shrinks_and_preserves() {
    let (o0, o1) = assert_equivalent("main() { mark; mark; right; mark, unmark; }", TAPES);
    assert!(o1 < o0);
}

#[test]
fn brk_barrier_blocks_elimination() {
    let (o0, o1) = assert_equivalent("main() { mark; debugger; mark; }", TAPES);
    assert_eq!(o0, o1, "no elimination across an observability barrier");
}

#[test]
fn branch_fold_cascades_into_dce_and_preserves() {
    let src = "main() { mark; check(1, 2); 1: unmark(!); 2: right; }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(
        o1 < o0,
        "folded branch + dead arm must shrink: {o0} -> {o1}"
    );
}

#[test]
fn reset_mf_semantics_survive_o1() {
    // First instruction is a check: MF is the reset 0 on EVERY tape,
    // including marked ones. -O1 must not "know better".
    let src = "main() { check(1, 2); 1: mark(!); 2: unmark(!); }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert_eq!(o0, o1, "an unfoldable program must be byte-stable");
}

#[test]
fn dropped_confirming_write_still_feeds_later_mf_observations() {
    // Task-4 review follow-up (controller-ratified): on the marked arm,
    // `mark` is a confirming write cell-state drops — the SECOND check
    // then observes MF that the dropped write would have latched. The
    // coupling invariant says dropping is invisible; this runs it.
    let src = "main() { right; check(1, 2); 1: mark; check(3, 2); 3: right(!); 2: left; }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(o1 < o0, "drop + fold must shrink: {o0} -> {o1}");
}

/// The program the whole optimizer story was started for in 2002:
/// redundant marks, a decided branch, a dead arm, a confirming write.
const FLAGSHIP: &str = "\
main() {
    mark;
    mark;
    right;
    mark, mark, unmark;
    check(1, 2);
1:  mark(!);
2:  unmark;
}
";

#[test]
fn flagship_optimizes_to_exact_bytes() {
    use mtc_post_machine::arch::opcodes::*;
    // Derivation (task-6 BLOCKED ruling: everything lands in r1 —
    // block_entry_facts is computed over the WHOLE function per pass
    // call, and the still-standing check's edge refinement already
    // tells both arms their cell): cell-state r1: b0 [wr1,wr1,rgt,
    // wr1,wr1,wr0] -> idempotent-drop 2nd+4th wr1, dead-store the wr1
    // before wr0 -> [wr1, rgt, wr0]; b1's confirming wr1 (marked edge,
    // Coupled(Some(1))) and b2's confirming wr0 (blank edge,
    // Coupled(Some(0))) drop in the SAME call. branch-fold r1: fact
    // Coupled(Some(0)) at the check -> goto blank arm. dce r1: block
    // `1:` dies. r2: zero changes — fixpoint. rounds == 2.
    // Codegen: ent, wr 1, rgt, wr 0, stp = 7 bytes.
    let out = compile(
        FLAGSHIP,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(
        linked.executable.code,
        vec![ENT, WR, 0x81, RGT, WR, 0x80, STP]
    );
    assert_eq!(out.report.opt.rounds, 2);

    // -O0 reference: 20 bytes (ent + 11 op bytes + jnm.s 2 + wr/stp 3 + wr/stp 3).
    let o0 = compile(FLAGSHIP, CompileOptions::default()).unwrap();
    let l0 = link(&[o0.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(l0.executable.code.len(), 20);
}

#[test]
fn flagship_is_equivalent_on_all_tapes() {
    let (o0, o1) = assert_equivalent(FLAGSHIP, TAPES);
    assert_eq!((o0, o1), (20, 7));
}

#[test]
fn fno_disables_a_single_pass() {
    let out = compile(
        FLAGSHIP,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["cell-state".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !out.report
            .opt
            .changes
            .iter()
            .any(|c| c.pass == "cell-state"),
        "{:?}",
        out.report.opt.changes
    );
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    assert!(linked.executable.code.len() > 7);
}

#[test]
fn capture_ir_records_the_pass_stages() {
    let out = compile(
        FLAGSHIP,
        CompileOptions {
            opt_level: OptLevel::O1,
            capture_ir: true,
            ..Default::default()
        },
    )
    .unwrap();
    let stages: Vec<&str> = out.ir_snapshots.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(stages.first().copied(), Some("lowered"));
    assert_eq!(stages.last().copied(), Some("final"));
    assert!(stages.contains(&"after:cell-state"), "{stages:?}");
    assert!(stages.contains(&"after:branch-fold"), "{stages:?}");
    assert!(stages.contains(&"after:tail-merge"), "{stages:?}");
    assert_ne!(out.ir_snapshots.first(), out.ir_snapshots.last());
    assert_eq!(out.ir, out.ir_snapshots.last().unwrap().1);
}

#[test]
fn spec_sample_inlines_at_o1() {
    // 6a's "already optimal" golden is obsolete BY DESIGN: with inline,
    // main absorbs goToEnd (leaf, 2 ops) and the linker then drops the
    // now-uncalled goToEnd. Derivation of the 14-byte -O1 executable:
    // main after splice: B[](goto g0'), g0'[rgt](check{g0',g1'}),
    // g1'[lft](goto C), C[rgt](check{b1,b2}), b1[wr0](ret), b2[wr1](ret)
    // → ent, rgt, jm.s -3, lft, rgt, jnm.s +3, wr 0, stp, wr 1, stp
    // = 1+1+2+1+1+2+2+1+2+1 = 14. -O0 linked = 18 (Plan 5 golden).
    let src = "\
goToEnd() {
1:  right;
    check(1, 2);
2:  left;
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert_eq!((o0, o1), (18, 14));

    // And the linker confirms the callee died:
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(linked.report.dropped, Vec::<String>::new());
}

#[test]
fn fno_inline_restores_the_do_no_harm_floor() {
    // With inline off, nothing in the 6b pipeline fires on the spec
    // sample (no tail position, no duplicate blocks, no empty-return
    // adjacency) — the old 6a byte-stability golden, behind the flag.
    let src = "\
goToEnd() {
1:  right;
    check(1, 2);
2:  left;
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
";
    let o0 = compile(src, CompileOptions::default()).unwrap();
    let o1 = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["inline".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(o0.object, o1.object);
}

#[test]
fn tail_call_preserves_behavior_and_shrinks() {
    // g tail-calls f; inline would dissolve the call first, so pin the
    // tail-call transform in isolation via --fno-inline (Task 4 adds
    // inline; this test is written to be correct both before and after).
    let src = "f() { right(!); } g() { left, @f(!); } main() { @g(); mark; }";
    let o0 = build(src, OptLevel::O0);
    let out1 = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["inline".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    let o1 = link(&[out1.object], &[], LinkOptions::default())
        .unwrap()
        .executable;
    for (cells, head) in TAPES {
        let r0 = run_tape(&o0, cells, *head);
        let r1 = run_tape(&o1, cells, *head);
        assert_eq!(r0, r1, "tape {cells:?}/{head}");
    }
    assert!(
        o1.code.len() < o0.code.len(),
        "{} -> {}",
        o0.code.len(),
        o1.code.len()
    );
}

#[test]
fn self_recursive_tail_call_becomes_an_in_place_loop() {
    // THE documented resource exception (spec §8 as amended): at -O0 the
    // recursion overflows the return stack; at -O1 the tail call is a
    // self-jump — an infinite loop that hits the step limit instead.
    // Termination KIND changes; that is sanctioned for resource traps.
    let src = "spin() { @spin(!); } main() { @spin(); }";
    let o0 = build(src, OptLevel::O0);
    let o1 = build(src, OptLevel::O1);
    let (outcome0, _, _) = run_tape(&o0, &[true], 0);
    let (outcome1, _, _) = run_tape(&o1, &[true], 0);
    assert!(
        matches!(
            outcome0,
            mtc_core::vm::Outcome::Trapped(mtc_core::vm::Trap::StackOverflow)
        ),
        "{outcome0:?}"
    );
    assert!(
        matches!(
            outcome1,
            mtc_core::vm::Outcome::Trapped(mtc_core::vm::Trap::StepLimit)
        ),
        "{outcome1:?}"
    );
}

#[test]
fn tail_call_emits_a_relaxed_jump() {
    use mtc_post_machine::arch::opcodes::*;
    let src = "f() { right(!); } g() { left, @f(!); } main() { @g(); mark; }";
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["inline".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    // Layout (BFS from main): main, g, f.
    // main: ent0, call.s g +? , wr 1, stp | g: ent, lft, jmp.s @f | f: ent, rgt, ret.
    // main = [0D][1B off][06 81][02] = 6 bytes → g at 6: [0D][04][18 off] = 4 → f at 10: [0D][05][0C].
    // call.s: end 3, g at 6 → +3. jmp.s: at 8, end 10, f at 10 → 0.
    assert_eq!(
        linked.executable.code,
        vec![
            ENT, CALL_S, 0x03, WR, 0x81, STP, // main
            ENT, LFT, JMP_S, 0x00, // g
            ENT, RGT, RET, // f
        ]
    );
}

#[test]
fn tail_merge_shares_the_stp_exactly_as_the_spec_promises() {
    use mtc_post_machine::arch::opcodes::*;
    // Spec §8 pass 7's own example. -O0: jm B2; wr 1; stp; B2: stp = 7
    // bytes (two stp). -O1: return-chaining drops the first stp — 6.
    let src = "main() { 1: check(!, 2); 2: mark(!); }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert_eq!((o0, o1), (7, 6));
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    // ent, jm.s +2, wr 1, stp — one stp serves both paths.
    assert_eq!(linked.executable.code, vec![ENT, JM_S, 0x02, WR, 0x81, STP]);
}

#[test]
fn flagship_is_untouched_by_the_6b_passes() {
    // The 6a crown jewel must not move: no calls, no duplicate blocks,
    // no empty-return adjacency (b0 ends Goto, not Return).
    let out = compile(
        FLAGSHIP,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    use mtc_post_machine::arch::opcodes::*;
    assert_eq!(
        linked.executable.code,
        vec![ENT, WR, 0x81, RGT, WR, 0x80, STP]
    );
}

#[test]
fn inline_then_tail_call_compose() {
    // step() is inlined into walk(); walk()'s own trailing call to
    // itself is NOT inlined (recursion) but IS tail-converted — the
    // classic loop-from-recursion, verified terminating identically.
    let src = "\
step() { right; }
walk() { @step(); check(1, !); 1: @walk(!); }
main() { @walk(); mark; }
";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(o1 < o0, "{o0} -> {o1}");
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            capture_ir: true,
            ..Default::default()
        },
    )
    .unwrap();
    let stages: Vec<&str> = out.ir_snapshots.iter().map(|(s, _)| s.as_str()).collect();
    assert!(stages.contains(&"after:inline"), "{stages:?}");
    assert!(stages.contains(&"after:tail-call"), "{stages:?}");
    // The recursive call became a tail jump:
    let walk = out.ir.functions.iter().find(|f| f.name == "walk").unwrap();
    assert!(walk.blocks.iter().any(|b| matches!(
        &b.term,
        mtc_post_machine::ir::IrTerm::TailCall { name } if name == "walk"
    )));
}

#[test]
fn fno_tail_call_keeps_calls() {
    let src = "f() { right(!); } g() { left, @f(!); } main() { @g(); }";
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["inline".to_string(), "tail-call".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!out.report.opt.changes.iter().any(|c| c.pass == "tail-call"));
    let g = out.ir.functions.iter().find(|f| f.name == "g").unwrap();
    assert!(
        g.blocks
            .iter()
            .all(|b| !matches!(b.term, mtc_post_machine::ir::IrTerm::TailCall { .. }))
    );
}
