//! Phase-5b end-to-end: `.tma` programs using DECLARATIVE bound calls link
//! under `--call-mech=mono` (and `hybrid`) and RUN on the real TM-1 arch.
//! Mono stamps a specialized base-profile copy of the callee with the
//! binding's symbol maps folded in; a crossed map hole traps exactly as the
//! frames path would, but through a synthesized `trap` instruction rather
//! than a runtime map lookup.

use mtc_core::formats::PROFILE_FRAMES;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::trap::Trap;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{assemble, link};

fn build(src: &str, mech: CallMech) -> Executable {
    let obj = assemble(src, false).expect("assembles");
    link(
        &[obj],
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .expect("the composition engine links the bound calls")
    .executable
}

/// Run `exe` on blank tapes of the given per-tape alphabet widths.
fn run(exe: &Executable, widths: &[u32]) -> (Outcome, Vec<TapeSnapshot>) {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tapes: Vec<WideTape> = widths.iter().map(|&w| WideTape::new(w)).collect();
    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
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
    let snaps = tapes.iter().map(WideTape::to_snapshot).collect();
    (result.outcome, snaps)
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

/// A 2-tape machine mono-calls a 1-tape `sub` through a HOLEY, one-way
/// binding: physical 1 and 2 both read as virtual 1 (a collapse); physical 3
/// has no virtual image (a read hole). `sub` reads, matches, and dispatches
/// to a per-virtual-symbol write. `main` seeds physical `{seed}` on tape 0
/// before the call. Branch B (virtual 1) writes virtual 2 → physical 2.
fn read_program(seed: u8) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=1, alpha=(3)
.section tables
T0: .row [0]
    .row [1]
    .row [2]
D0: .targets A, B, C
.section code
.func main
        wr   [{seed}, -]
        call sub [0{{1=>1, 2=>1}}]
        stp
.func sub
        rd
        mtc  T0
        djmp D0
A:      wr [0]
        ret
B:      wr [2]
        ret
C:      wr [1]
        ret
"
    )
}

#[test]
fn mono_links_to_the_base_profile() {
    let exe = build(&read_program(1), CallMech::Mono);
    assert_ne!(exe.profile, PROFILE_FRAMES, "mono ⇒ base profile");
    assert_eq!(exe.frames_offset, 0, "no frames region");
}

#[test]
fn mono_happy_path_reads_dispatches_and_writes() {
    // Seed physical 1: reads as virtual 1 → branch B → writes virtual 2,
    // which maps back to physical 2. The stamp read, matched, dispatched,
    // and wrote entirely on the base profile.
    let exe = build(&read_program(1), CallMech::Mono);
    let (outcome, snaps) = run(&exe, &[4, 4]);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(cell_at(&snaps[0], 0), 2, "virtual 1 → B → physical 2");
}

#[test]
fn mono_collapse_maps_two_physical_symbols_to_one_branch() {
    // Seed physical 2: the one-way collapse reads it ALSO as virtual 1, so
    // it dispatches to the same branch B and produces the same physical 2 —
    // the row expansion pointed both preimages at one target.
    let exe = build(&read_program(2), CallMech::Mono);
    let (outcome, snaps) = run(&exe, &[4, 4]);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(
        cell_at(&snaps[0], 0),
        2,
        "physical 2 collapses onto virtual 1"
    );
}

#[test]
fn mono_read_hole_traps_unmapped_read() {
    // Seed physical 3: no virtual symbol reads as it, so the synthesized
    // first-match trap row fires — `trap #0` on the base profile raises
    // UnmappedRead, the same trap KIND the frames path would.
    let exe = build(&read_program(3), CallMech::Mono);
    let (outcome, _snaps) = run(&exe, &[4, 4]);
    assert!(
        matches!(outcome, Outcome::Trapped(Trap::UnmappedRead { .. })),
        "read hole traps UnmappedRead, got {outcome:?}"
    );
}

/// A 1-tape machine mono-calls a wider `w` through a swap binding; `w`
/// writes virtual 3, which has no physical image (the machine alphabet is
/// narrower) — a write hole the stamp lowers to `trap #1`.
const WRITE_HOLE: &str = "\
.routine main, tapes=1, alpha=(3)
.routine w, tapes=1, alpha=(4)
.section code
.func main
        call w [0{1->2, 2->1}]
        stp
.func w
        wr [3]
        ret
";

#[test]
fn mono_write_hole_traps_unmapped_write() {
    let exe = build(WRITE_HOLE, CallMech::Mono);
    assert_ne!(exe.profile, PROFILE_FRAMES, "still base profile");
    let (outcome, _snaps) = run(&exe, &[3]);
    assert!(
        matches!(outcome, Outcome::Trapped(Trap::UnmappedWrite { .. })),
        "write hole traps UnmappedWrite, got {outcome:?}"
    );
}

/// One HYBRID image with both paths: `main` calls `swap` through an
/// equal-size bijection (mono-stamped) and `holey` through a one-way collapse
/// (frames). The bijection writes on the base-profile stamp; the frames site
/// runs through the compose table. Both land their symbol on tape 0 in turn.
const HYBRID_MIXED: &str = "\
.routine main, tapes=1, alpha=(4)
.routine swap, tapes=1, alpha=(4)
.routine holey, tapes=1, alpha=(4)
.section code
.func main
        call swap  [0{1->2, 2->1}]
        call holey [0{2=>3, 3=>3}]
        stp
.func swap
        wr [1]
        ret
.func holey
        wr [3]
        ret
";

#[test]
fn hybrid_mixed_image_is_frames_and_runs_both_paths() {
    let exe = build(HYBRID_MIXED, CallMech::Hybrid);
    // A frames site survives, so the image is FRAMES and carries a region.
    assert_eq!(exe.profile, PROFILE_FRAMES);
    assert_ne!(exe.frames_offset, 0);
    let (outcome, snaps) = run(&exe, &[4]);
    assert_eq!(outcome, Outcome::Stopped);
    // swap (mono stamp): writes virtual 1 → physical 2. Then holey (frames):
    // writes virtual 3, which maps to physical 3. The second write lands on
    // the same cell (no motion), so the final cell is physical 3.
    assert_eq!(
        cell_at(&snaps[0], 0),
        3,
        "the frames write ran after the stamp"
    );
}
