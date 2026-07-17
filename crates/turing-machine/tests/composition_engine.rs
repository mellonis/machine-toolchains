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
