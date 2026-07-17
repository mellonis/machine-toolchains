//! Phase-5b end-to-end: a `.tma` program using DECLARATIVE bound calls
//! links through the composition engine and RUNS on the real TM-1 arch, the
//! runtime compose lookup selecting a different frame descriptor per active
//! context. This proves the engine's emission against the T2 VM path.

use mtc_core::formats::PROFILE_FRAMES;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::{Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::asm::{assemble, link};

/// `main` calls the SAME 1-tape `writer` under two different bindings: the
/// first swaps symbols 1 and 2, the second swaps 1 and 3. `writer` writes
/// virtual symbol 1 and steps right. So the generic writer code, run under
/// two composites, lands physical symbol 2 in cell 0 and physical symbol 3
/// in cell 1 — the compose lookup selects the context.
const TWO_CONTEXT: &str = "\
.routine main, tapes=1, alpha=(4)
.routine writer, tapes=1, alpha=(4)
.section code
.func main
        call    writer [0{1->2, 2->1}]
        call    writer [0{1->3, 3->1}]
        stp
.func writer
        wr      [1]
        mov     [>]
        ret
";

fn frames_opts() -> LinkOptions {
    LinkOptions {
        call_mech: CallMech::Frames,
        ..Default::default()
    }
}

fn build(src: &str) -> mtc_core::formats::executable::Executable {
    let obj = assemble(src, false).expect("assembles");
    link(&[obj], &[], frames_opts())
        .expect("the composition engine links the bound calls")
        .executable
}

fn run(exe: &mtc_core::formats::executable::Executable, width: u32) -> (Outcome, TapeSnapshot) {
    let mut registry = mtc_core::vm::ArchRegistry::new();
    registry.register(Box::new(mtc_turing_machine::arch::Tm1::new(exe.tape_count)));
    let machine = mtc_core::vm::Machine::from_executable(exe, &registry).expect("loads");
    let mut t0 = WideTape::new(width);
    let mut devices: Vec<&mut dyn Tape> = vec![&mut t0];
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(100_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    drop(devices);
    (result.outcome, t0.to_snapshot())
}

/// The cell at absolute position `pos` in a snapshot (blank past the ends).
fn cell_at(snap: &TapeSnapshot, pos: i64) -> u8 {
    let idx = pos - snap.origin;
    if idx < 0 || idx as usize >= snap.cells.len() {
        0
    } else {
        snap.cells[idx as usize]
    }
}

#[test]
fn a_declarative_binding_program_links_to_the_frames_profile() {
    assert_eq!(build(TWO_CONTEXT).profile, PROFILE_FRAMES);
}

#[test]
fn the_compose_lookup_selects_a_different_descriptor_per_context() {
    let exe = build(TWO_CONTEXT);
    let (outcome, snap) = run(&exe, 4);
    assert_eq!(outcome, Outcome::Stopped);
    // Derived independently: writer under the swap-1-2 frame writes physical
    // 2 at cell 0; under the swap-1-3 frame it writes physical 3 at cell 1.
    // The SAME `wr [1]` produced two different physical symbols — the runtime
    // compose lookup chose the context.
    assert_eq!(cell_at(&snap, 0), 2, "context 1: virtual 1 -> physical 2");
    assert_eq!(cell_at(&snap, 1), 3, "context 2: virtual 1 -> physical 3");
}

/// Re-linking the same declarative program is byte-identical — the closure
/// is deterministic (reproducible builds).
#[test]
fn a_declarative_binding_link_is_reproducible() {
    let a = build(TWO_CONTEXT).to_bytes();
    let b = build(TWO_CONTEXT).to_bytes();
    assert_eq!(a, b);
}

/// An intra-function jump crossing a widened bound-call site must be
/// re-encoded through the offset map: the jump lands on `A` (the stop),
/// skipping the framed call, so `sub` never writes and the tape stays blank.
/// A mis-shifted offset would land inside the widened framed call.
const JUMP_OVER_SITE: &str = "\
.routine main, tapes=1, alpha=(4)
.routine sub, tapes=1, alpha=(4)
.section code
.func main
        jmp     A
        call    sub [0{1->2, 2->1}]
A:      stp
.func sub
        wr      [1]
        ret
";

#[test]
fn a_jump_over_a_widened_site_is_re_encoded() {
    let exe = build(JUMP_OVER_SITE);
    assert_eq!(exe.profile, PROFILE_FRAMES, "the bound call still frames");
    let (outcome, snap) = run(&exe, 4);
    assert_eq!(outcome, Outcome::Stopped);
    // The jump skipped the framed call; `sub` never wrote — a blank tape.
    assert_eq!(cell_at(&snap, 0), 0, "sub was skipped, tape stays blank");
}

/// A hand-authored `call.m` inside an engine-composed routine keeps activating
/// its authored descriptor (absolute placement), unaffected by the composite
/// `r` runs under. `main` bound-calls `r` under a swap-1-3 frame; `r`'s raw
/// `call.m leaf, Fr` writes virtual 1 through `Fr`'s wmap (1->2), so `leaf`
/// lands physical 2 — NOT physical 3 (which the enclosing frame would give).
const NESTED_RAW: &str = "\
.routine main, tapes=1, alpha=(4)
.routine r, tapes=1, alpha=(4)
.routine leaf, tapes=1, alpha=(4)
.section tables
Fr: .frame  tapes=(0)
    .map    0, wmap=(1->2)
.section code
.func main
        call    r [0{1->3, 3->1}]
        stp
.func r
        call.m  leaf, Fr
        ret
.func leaf
        wr      [1]
        ret
";

#[test]
fn a_raw_call_m_in_a_composed_routine_activates_its_own_descriptor() {
    let exe = build(NESTED_RAW);
    assert_eq!(exe.profile, PROFILE_FRAMES);
    let (outcome, snap) = run(&exe, 4);
    assert_eq!(outcome, Outcome::Stopped);
    // Fr's wmap sends virtual 1 -> physical 2, regardless of the swap-1-3
    // frame `r` runs under (which would send virtual 1 -> physical 3).
    assert_eq!(cell_at(&snap, 0), 2, "the authored descriptor Fr activated");
}
