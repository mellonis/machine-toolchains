//! The optimizer equivalence harness (the equivalence contract —
//! crates/turing-machine/src/optimizer/mod.rs). Every `-O1` pass is guarded by
//! running the SAME program across the full 2×3 matrix — `-O0`/`-O1` × the
//! three call mechanisms (mono / frames / hybrid) — and comparing observables:
//! the outcome (INCLUDING trap kind), every per-tape final snapshot, and every
//! head. A pass that changes behaviour on any seed fails here.
//!
//! This combines the two 6a axes: the `mode_equivalence.rs` mechanism axis (×3)
//! and a new optimization-level axis (×2). Trap `at` offsets are excluded (they
//! differ by layout — the trap KIND is the invariant); snapshots and heads are
//! compared strictly.
//!
//! Programs at this stage: the six Appendix A examples plus the nested-graft
//! case (read from `tests/golden/*.tmc`), a debugger-bearing barrier program,
//! the do-no-harm floor (`-O1` with every pass disabled reproduces `-O0`
//! byte-for-byte), and per-pass fixtures added as passes land.

use std::fs;
use std::path::Path;

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, Trap, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, CompileOutput, compile};
use mtc_turing_machine::optimizer::{OptLevel, pass_names};

// ── harness ──────────────────────────────────────────────────────────────

/// Compile a `.tmc` source at `level`, disabling the named passes
/// (`--fno-<pass>`). Returns the whole output so a caller can read the object,
/// the optimizer report (was a pass real?), or the final IR (did a barrier
/// hold?).
fn object_of(src: &str, level: OptLevel, disabled: &[&str]) -> CompileOutput {
    compile(
        src,
        CompileOptions {
            opt_level: level,
            disabled_passes: disabled.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
    )
    .expect("the program compiles")
}

/// Compile at `level` (disabling `disabled`) then link under `mech` — the
/// common path to a runnable image. The compiler's object is mode-independent
/// (one `.tmo`, three links), so `mech` selects only how bound calls lower.
fn build(src: &str, level: OptLevel, mech: CallMech, disabled: &[&str]) -> Executable {
    let out = object_of(src, level, disabled);
    link_mech(&out.object, mech)
}

fn link_mech(obj: &ObjectFile, mech: CallMech) -> Executable {
    link(
        std::slice::from_ref(obj),
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the {mech} link failed: {e}"))
    .executable
}

/// A trap's KIND, stripped of its `at` offset (mono and frames lay code out
/// differently, so the faulting address legitimately differs; the KIND is the
/// invariant). Exhaustive on purpose — a new `Trap` variant must be named here,
/// not folded into a catch-all that could mask a cross-configuration divergence.
fn trap_kind(t: Trap) -> &'static str {
    match t {
        Trap::InvalidOpcode { .. } => "invalid-opcode",
        Trap::CodeOutOfBounds { .. } => "code-out-of-bounds",
        Trap::BadOperand { .. } => "bad-operand",
        Trap::CallTargetNotEntry { .. } => "call-target-not-entry",
        Trap::StackOverflow => "stack-overflow",
        Trap::StackUnderflow => "stack-underflow",
        Trap::StepLimit => "step-limit",
        Trap::TactLimit => "tact-limit",
        Trap::Device { .. } => "device",
        Trap::NoTransition { .. } => "no-transition",
        Trap::TableOutOfBounds { .. } => "table-out-of-bounds",
        Trap::DispatchOutOfRange { .. } => "dispatch-out-of-range",
        Trap::UnmappedRead { .. } => "unmapped-read",
        Trap::UnmappedWrite { .. } => "unmapped-write",
        Trap::ExitOutOfRange { .. } => "exit-out-of-range",
        Trap::ProfileViolation { .. } => "profile-violation",
    }
}

/// The configuration-independent behavioral outcome: `stopped`/`halted`, or a
/// trap KIND.
fn outcome_kind(o: Outcome) -> String {
    match o {
        Outcome::Stopped => "stopped".to_string(),
        Outcome::Halted => "halted".to_string(),
        Outcome::Trapped(t) => format!("trapped:{}", trap_kind(t)),
    }
}

/// One tape's initial contents: `cells` laid at origin 0, head at the given
/// coordinate. A `Case` is one such spec per physical tape, in tape order.
type Case = &'static [(&'static [u8], i64)];

/// The observable behavioral tuple of one run: outcome kind, per-tape final
/// snapshots, per-tape final heads.
struct Observed {
    outcome: String,
    snaps: Vec<TapeSnapshot>,
    heads: Vec<i64>,
}

/// Run `exe` on `seeds`, one seeded `WideTape` per physical tape (width from
/// the image's per-tape alphabet cardinalities). Mirrors `tmc_golden.rs` /
/// `mode_equivalence.rs`.
fn run(exe: &Executable, seeds: Case) -> Observed {
    assert_eq!(
        seeds.len(),
        exe.tape_count as usize,
        "a case must seed exactly one tape per machine tape"
    );
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tapes: Vec<WideTape> = seeds
        .iter()
        .zip(&exe.alphabet_cardinalities)
        .map(|(&(cells, head), &width)| {
            WideTape::from_snapshot(
                &TapeSnapshot {
                    origin: 0,
                    cells: cells.to_vec(),
                    head,
                    alphabet: None,
                },
                width,
            )
            .expect("the seed fits the tape width")
        })
        .collect();
    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(1_000_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    drop(devices);
    let snaps: Vec<TapeSnapshot> = tapes.iter().map(WideTape::to_snapshot).collect();
    let heads = snaps.iter().map(|s| s.head).collect();
    Observed {
        outcome: outcome_kind(result.outcome),
        snaps,
        heads,
    }
}

/// The milestone contract: build the full 2×3 matrix (`-O0`/`-O1` × mono /
/// frames / hybrid) and assert the behavioral tuple is identical across all six
/// configurations on every case.
fn assert_equivalent(src: &str, cases: &[Case]) {
    let mut exes: Vec<((OptLevel, CallMech), Executable)> = Vec::new();
    for level in [OptLevel::O0, OptLevel::O1] {
        for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
            exes.push(((level, mech), build(src, level, mech, &[])));
        }
    }
    for (i, case) in cases.iter().enumerate() {
        let results: Vec<((OptLevel, CallMech), Observed)> = exes
            .iter()
            .map(|((level, mech), exe)| ((*level, *mech), run(exe, case)))
            .collect();
        let (ref_key, r0) = &results[0];
        for (key, obs) in &results[1..] {
            assert_eq!(
                (&r0.outcome, &r0.snaps, &r0.heads),
                (&obs.outcome, &obs.snaps, &obs.heads),
                "matrix divergence on case {i} ({case:?}): {ref_key:?} vs {key:?}"
            );
        }
    }
}

/// Read a committed `.tmc` fixture from `tests/golden/`.
fn golden_src(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{name}: {e}"))
}

// ── the six Appendix A examples + nested graft (the equivalence floor) ──────

#[test]
fn a1_replace_b_is_equivalent() {
    // Walk right, 'b'→'a', stop at blank. Seed "bab" (cells [2,1,2]).
    assert_equivalent(&golden_src("a1_replace_b.tmc"), &[&[(&[2, 1, 2], 0)]]);
}

#[test]
fn a2_binary_plus_one_is_equivalent() {
    // Increment "11", head on the LSB (position 1); the carry extends leftward.
    assert_equivalent(&golden_src("a2_binary_plus_one.tmc"), &[&[(&[2, 2], 1)]]);
}

#[test]
fn a3_two_tape_copy_is_equivalent() {
    // src "10" (cells [2,1]) copied cell-by-cell onto a blank dst tape.
    assert_equivalent(
        &golden_src("a3_two_tape_copy.tmc"),
        &[&[(&[2, 1], 0), (&[], 0)]],
    );
}

#[test]
fn a4_byte_increment_is_equivalent() {
    // Normal (5→6, stop), overflow (126→halt), and blank (0→1, stop).
    assert_equivalent(
        &golden_src("a4_byte_increment.tmc"),
        &[&[(&[5], 0)], &[(&[126], 0)], &[(&[], 0)]],
    );
}

#[test]
fn a5_call_across_alphabets_is_equivalent() {
    // Happy path (ctl "1", data "1"→"10") plus the two holey reads ('a'/'b'
    // under the data head → unmapped-read). Trap KIND is compared, so the
    // holes exercise the trap-taxonomy axis across all six configurations.
    assert_equivalent(
        &golden_src("a5_call_across_alphabets.tmc"),
        &[
            &[(&[2], 0), (&[4], 0)], // happy: increments through the map
            &[(&[2], 0), (&[1], 0)], // data head on 'a' (wide 1) → unmapped-read
            &[(&[2], 0), (&[2], 0)], // data head on 'b' (wide 2) → unmapped-read
        ],
    );
}

#[test]
fn a6_graph_graft_multi_exit_is_equivalent() {
    // x-found (seed "zx", stop) and blank-found (seed "y", halt).
    assert_equivalent(
        &golden_src("a6_graph_graft_multi_exit.tmc"),
        &[&[(&[3, 1], 0)], &[(&[2], 0)]],
    );
}

#[test]
fn nested_graft_is_equivalent() {
    // happy (seed "xy", stop) and lose (seed "x" with no following 'y', halt).
    assert_equivalent(
        &golden_src("nested_graft.tmc"),
        &[&[(&[1, 2], 0)], &[(&[1], 0)]],
    );
}

// ── the brk barrier ─────────────────────────────────────────────────────────

/// A forwarder state that carries a `debugger` (`brk`) row. It has the shape a
/// motion pass would love to thread through — a single all-wildcard row with no
/// write and no move — but the `brk` makes it an observability barrier: a
/// debugger attached at `-O1` must still pause there. `brk` is inert in a plain
/// run, so the cross-configuration compare below CANNOT see a barrier violation
/// on its own; the structural assertion is the real check for this pass.
const BRK_BARRIER: &str = "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go     { [*] -> goto brkfwd; }
  state brkfwd       { [*] -> debugger goto done; }
  state done         { [*] -> stop; }
}";

#[test]
fn brk_barrier_holds_across_the_matrix() {
    // Observables agree across the whole 2×3 matrix on a blank and an 'a' tape.
    assert_equivalent(BRK_BARRIER, &[&[(&[0], 0)], &[(&[1], 0)]]);

    // The real barrier check: the debugger row must survive `-O1`. No motion
    // pass may thread through it (jump-threading), and no pass may delete the
    // state that carries it while it is reachable.
    let out = object_of(BRK_BARRIER, OptLevel::O1, &[]);
    assert!(
        out.ir
            .worlds
            .iter()
            .any(|w| w.states.iter().any(|s| s.rules.iter().any(|r| r.debugger))),
        "the brk barrier must keep the debugger row at -O1"
    );
}

// ── jump-threading + dce, and the do-no-harm floor ──────────────────────────

/// A terminating program with an off-adjacency empty forwarder: `scan` walks
/// right over 'a's; on a blank it hops to `hop`, an empty forwarder to
/// `finish`. `hop` is not the physically-next block, so `-O0` emits a real jump
/// to it that jump-threading removes (retargeting scan's blank edge straight to
/// `finish`) and dce then deletes the now-unreachable `hop`. So `-O1` is a
/// STRICTLY smaller object than `-O0` here — which makes both the pass-fired
/// fixture and the do-no-harm floor byte-observable, not vacuous.
const FORWARDER_HOP: &str = "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state scan {
    ['a'] -> move [>] goto scan;
    ['_'] -> goto hop;
  }
  state finish { [*] -> write ['a'] stop; }
  state hop    { [*] -> goto finish; }
}";

#[test]
fn jump_threading_and_dce_collapse_the_forwarder() {
    // Equivalent to -O0 on a blank tape and an 'a' tape across the whole 2×3
    // matrix.
    assert_equivalent(FORWARDER_HOP, &[&[(&[], 0)], &[(&[1], 0)]]);

    // Non-vacuous: -O1 shrinks the object, and the report names both passes as
    // having changed the IR — the equivalence above is a real transform, not a
    // no-op agreeing with itself.
    let o0 = object_of(FORWARDER_HOP, OptLevel::O0, &[]);
    let o1 = object_of(FORWARDER_HOP, OptLevel::O1, &[]);
    assert!(
        o1.object.to_bytes().len() < o0.object.to_bytes().len(),
        "-O1 must shrink the forwarder object: {} -> {}",
        o0.object.to_bytes().len(),
        o1.object.to_bytes().len()
    );
    let fired: Vec<&str> = o1.report.opt.changes.iter().map(|c| c.pass).collect();
    assert!(
        fired.contains(&"jump-threading"),
        "jump-threading fired: {fired:?}"
    );
    assert!(fired.contains(&"dce"), "dce fired: {fired:?}");
}

#[test]
fn fno_every_pass_restores_the_do_no_harm_floor() {
    // Disabling every registered pass returns the optimizer to identity: the
    // `-O1` object is byte-identical to `-O0` (the pmc do-no-harm floor
    // transposed). Real because `-O1` WOULD otherwise shrink this program (see
    // above). Reads `pass_names()` so it stays correct as passes land.
    let disabled = pass_names();
    let o0 = object_of(FORWARDER_HOP, OptLevel::O0, &[]);
    let o1 = object_of(FORWARDER_HOP, OptLevel::O1, &disabled);
    assert_eq!(
        o0.object, o1.object,
        "disabling every pass must reproduce -O0 byte-for-byte"
    );
}

// ── tail_call: cross-unit bindless tail calls ───────────────────────────────

/// A chain of `n` routines, each in its OWN compilation unit so its call is
/// EXTERNAL — and therefore BINDLESS (an in-unit call to a tape-bearing routine
/// requires binding args, which the front end enforces and which excludes it
/// from `tail_call`). `main` bindless-calls `cl::step1 then stop` (a real call,
/// never tail-converted — a machine cannot `return`); `step{k}` bindless-
/// tail-calls `cl::step{k+1} then return`; `step{n}` writes '1' and returns.
/// The whole chain runs on the one shared tape (identity projection). Returned
/// unit 0 is `main`; units 1..=n are the steps.
fn tail_chain_units(n: usize) -> Vec<String> {
    let mut units = vec![
        "alphabet bits { '_', '0', '1' }\n\
         machine { tape t: bits; entry state go { [*] -> call cl::step1() then stop; } }"
            .to_string(),
    ];
    for k in 1..n {
        units.push(format!(
            "alphabet bits {{ '_', '0', '1' }}\n\
             namespace cl {{ export routine step{k}(tape t: bits) {{\n\
               entry state s {{ [*] -> call cl::step{next}() then return; }}\n\
             }} }}",
            next = k + 1
        ));
    }
    units.push(format!(
        "alphabet bits {{ '_', '0', '1' }}\n\
         namespace cl {{ export routine step{n}(tape t: bits) {{\n\
           entry state s {{ [*] -> write ['1'] return; }}\n\
         }} }}"
    ));
    units
}

/// Compile every unit at `level` and link them together under `mech`.
fn build_chain(n: usize, level: OptLevel, mech: CallMech) -> Executable {
    let units = tail_chain_units(n);
    let objs: Vec<ObjectFile> = units
        .iter()
        .map(|u| object_of(u, level, &[]).object)
        .collect();
    link(
        &objs,
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the {mech} chain link failed: {e}"))
    .executable
}

/// Run a linked chain image on one blank `bits` tape with an explicit stack
/// depth, collecting the same observable tuple as `run`.
fn run_chain(exe: &Executable, stack_depth: usize) -> Observed {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = WideTape::new(exe.alphabet_cardinalities[0]);
    let result = {
        let mut devices: Vec<&mut dyn Tape> = vec![&mut tape as &mut dyn Tape];
        machine
            .run_tapes(
                &mut devices,
                RunOptions {
                    stack_depth,
                    limits: RunLimits {
                        max_steps: Some(1_000_000),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .expect("run set-up ok")
    };
    let snap = tape.to_snapshot();
    let heads = vec![snap.head];
    Observed {
        outcome: outcome_kind(result.outcome),
        snaps: vec![snap],
        heads,
    }
}

#[test]
fn tail_call_chain_is_equivalent_across_the_matrix() {
    // A terminating bindless tail-call chain runs identically across the full
    // 2×3 matrix (a generous stack, so -O0 does not overflow — the resource
    // divergence is the SEPARATE test below). The observable tuple must agree.
    let n = 6;
    let mut results: Vec<((OptLevel, CallMech), Observed)> = Vec::new();
    for level in [OptLevel::O0, OptLevel::O1] {
        for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
            let exe = build_chain(n, level, mech);
            results.push(((level, mech), run_chain(&exe, 1024)));
        }
    }
    let (ref_key, r0) = &results[0];
    for (key, obs) in &results[1..] {
        assert_eq!(
            (&r0.outcome, &r0.snaps, &r0.heads),
            (&obs.outcome, &obs.snaps, &obs.heads),
            "tail-call matrix divergence: {ref_key:?} vs {key:?}"
        );
    }
    // The chain actually ran to the end: it stopped with `step{n}`'s '1' (bits
    // index 2) on the tape.
    assert_eq!(r0.outcome, "stopped");
    assert_eq!(
        r0.snaps[0].cells.first().copied(),
        Some(2),
        "step{n} wrote '1'"
    );
}

#[test]
fn tail_call_shrinks_the_step_object_and_fires() {
    // A step unit's `call cl::stepK+1; ret` (6 bytes) collapses to `jmp
    // @cl::stepK+1` (5 bytes) — the object shrinks and the pass is reported.
    // The `--fno-<every-pass>` floor reproduces -O0 byte-for-byte, so the
    // shrink is a real transform, not the optimizer disagreeing with itself.
    let units = tail_chain_units(6);
    let step = &units[1]; // step1: a bindless-tail-calling step unit
    let o0 = object_of(step, OptLevel::O0, &[]);
    let o1 = object_of(step, OptLevel::O1, &[]);
    assert!(
        o1.object.to_bytes().len() < o0.object.to_bytes().len(),
        "-O1 must shrink the tail-calling step: {} -> {}",
        o0.object.to_bytes().len(),
        o1.object.to_bytes().len()
    );
    let fired: Vec<&str> = o1.report.opt.changes.iter().map(|c| c.pass).collect();
    assert!(fired.contains(&"tail-call"), "tail-call fired: {fired:?}");

    let floor = object_of(step, OptLevel::O1, &pass_names());
    assert_eq!(
        o0.object, floor.object,
        "disabling every pass reproduces -O0 on the step unit"
    );
}

#[test]
fn tail_call_eliminates_the_return_stack_growth() {
    // The equivalence contract (crates/turing-machine/src/optimizer/mod.rs):
    // "step counts and resource-limit outcomes may change; no motion across an
    // un-stripped brk." A tail call removes the return-stack push, so a deep
    // chain that StackOverflows at -O0 on a tiny stack COMPLETES at -O1 on the
    // same stack. That divergence (trap vs stop) is a resource-limit outcome —
    // explicitly excluded from the equivalence contract — so it is asserted
    // here, OUTSIDE `assert_equivalent`, on a stack too small for -O0's frames.
    let n = 6;
    let stack = 3; // < n: -O0's per-call return pushes overflow it
    let o0 = build_chain(n, OptLevel::O0, CallMech::Mono);
    let o1 = build_chain(n, OptLevel::O1, CallMech::Mono);
    assert_eq!(
        run_chain(&o0, stack).outcome,
        "trapped:stack-overflow",
        "-O0 keeps a return frame per call and overflows a depth-{stack} stack"
    );
    assert_eq!(
        run_chain(&o1, stack).outcome,
        "stopped",
        "-O1 tail-jumps with no push and completes on the same tiny stack"
    );
}
