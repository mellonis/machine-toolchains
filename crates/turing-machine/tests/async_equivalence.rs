//! sync ≡ pump equivalence over real TM-1 images (docs/core.md (async
//! session)): a pumped run through always-ready adapters must match
//! `Machine::run_tapes` bit-exactly — outcome, stats, ip, stack, AND every
//! band's final tape — at -O0 and -O1, on a single-tape program and a
//! two-tape program. Mirrors the PM-1 corpus check
//! (`crates/post-machine/tests/async_equivalence.rs`), carried over to the
//! multi-tape shape (`async_session_tapes` — the table-ROM-carrying,
//! no-initial-latch mirror of `run_tapes`).

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{
    ArchRegistry, AsyncTapeDevice, Machine, Outcome, PumpEvent, RunOptions, RunResult, SyncAsAsync,
    Tape, WideTape,
};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;

/* local helpers, copied per cli_programs.rs's / opt_equivalence.rs's build
pattern (source -> linked Executable + Machine construction against the
TM-1 arch registry) — each integration test file defines its own, repo
convention. */

fn build(src: &str, opt: OptLevel) -> Executable {
    let out = compile(
        src,
        CompileOptions {
            opt_level: opt,
            ..Default::default()
        },
    )
    .expect("compiles");
    link(
        std::slice::from_ref(&out.object),
        &[],
        LinkOptions::default(),
    )
    .expect("links")
    .executable
}

fn machine<'a>(exe: &Executable, registry: &'a ArchRegistry) -> Machine<'a> {
    Machine::from_executable(exe, registry).expect("loads")
}

/// A registry with one `Tm1` sized to `exe`'s own tape count, so it always
/// matches the image it is used against (mirrors `opt_equivalence.rs::run`).
fn registry_for(exe: &Executable) -> ArchRegistry {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    registry
}

/// One blank `WideTape` per physical tape, sized from the image's per-tape
/// alphabet cardinalities.
fn blank_tapes(exe: &Executable) -> Vec<WideTape> {
    exe.alphabet_cardinalities
        .iter()
        .map(|&width| WideTape::new(width))
        .collect()
}

/// Write `cells` (glyph indices) at consecutive positions starting from the
/// tape's initial head position, then walk the head back to that starting
/// cell (position 0, origin 0, matching every golden fixture's seed shape)
/// — the shared seeding step run identically against the sync device set
/// and the to-be-wrapped pumped device set, so both runs start from
/// IDENTICAL tape contents.
fn seed(tape: &mut WideTape, cells: &[u32]) {
    for (i, &index) in cells.iter().enumerate() {
        if i > 0 {
            tape.right();
        }
        tape.write(index).expect("seed cell fits the tape width");
    }
    for _ in 1..cells.len() {
        tape.left();
    }
}

/// Pump `exe` to completion over one `SyncAsAsync<WideTape>` per band,
/// seeded from `seeds` (one cell list per tape, empty for a blank tape).
/// Returns the `RunResult` alongside every band's final snapshot — the
/// `RunResult` alone carries no tape data, so a write mis-routed to the
/// wrong device index would be invisible to it.
fn pump_tapes_to_end(
    exe: &Executable,
    registry: &ArchRegistry,
    seeds: &[&[u32]],
) -> (RunResult, Vec<TapeSnapshot>) {
    let m = machine(exe, registry);
    let mut inner = blank_tapes(exe);
    for (tape, cells) in inner.iter_mut().zip(seeds) {
        seed(tape, cells);
    }
    let mut devices: Vec<SyncAsAsync<WideTape>> = inner.into_iter().map(SyncAsAsync::new).collect();
    let mut session = m.async_session_tapes(RunOptions::default());
    let mut slice: Vec<&mut dyn AsyncTapeDevice> =
        devices.iter_mut().map(|d| d as &mut _).collect();
    let result = loop {
        match session.pump(&mut slice, None) {
            PumpEvent::Finished(result) => break result,
            PumpEvent::DeviceWait | PumpEvent::BudgetSpent => continue,
            other => panic!("unexpected: {other:?}"),
        }
    };
    drop(slice);
    let snaps = devices
        .into_iter()
        .map(|d| d.into_inner().to_snapshot())
        .collect();
    (result, snaps)
}

/// Run `exe` synchronously over one `WideTape` per band, seeded from
/// `seeds` the same way `pump_tapes_to_end` seeds its own device set.
/// Returns the `RunResult` alongside every band's final snapshot (see
/// `pump_tapes_to_end`).
fn run_tapes_to_end(
    exe: &Executable,
    registry: &ArchRegistry,
    seeds: &[&[u32]],
) -> (RunResult, Vec<TapeSnapshot>) {
    let m = machine(exe, registry);
    let mut tapes = blank_tapes(exe);
    for (tape, cells) in tapes.iter_mut().zip(seeds) {
        seed(tape, cells);
    }
    let result = {
        let mut devices: Vec<&mut dyn Tape> =
            tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
        m.run_tapes(&mut devices, RunOptions::default())
            .expect("run set-up ok")
    };
    let snaps = tapes.iter().map(WideTape::to_snapshot).collect();
    (result, snaps)
}

/// Single-tape bit-flipper: walk right over `0`/`1`, flipping each, stop at
/// the first blank.
const BIT_FLIPPER: &str = "\
alphabet bits { '_', '0', '1' }

machine {
  tape t: bits;

  entry state scan {
    ['0'] -> write ['1'] move [>] goto scan;
    ['1'] -> write ['0'] move [>] goto scan;
    ['_'] -> stop;
  }
}
";

/// Two-tape copier: copy tape `a`'s run of marks onto tape `b`, stopping at
/// the first blank on `a`. Modeled on `a3_two_tape_copy.tmc`
/// (docs/tmt/language.md (substitution)), narrowed to a single mark glyph
/// so no substitution binding is needed.
const MARK_COPIER: &str = "\
alphabet marks { '_', '*' }

machine {
  tape a: marks;
  tape b: marks;

  entry state copy {
    ['*', *] -> write [-, '*'] move [>, >] goto copy;
    ['_', *] -> stop;
  }
}
";

#[test]
fn pumped_tm_runs_match_run_tapes() {
    // `BIT_FLIPPER` is single-tape, one band, cardinality 3 (bits: '_' '0'
    // '1'). Seed "010" (cells [1,2,1]).
    for opt in [OptLevel::O0, OptLevel::O1] {
        let exe = build(BIT_FLIPPER, opt);
        let registry = registry_for(&exe);
        let seeds: [&[u32]; 1] = [&[1, 2, 1]]; // "010"
        let (sync, sync_snaps) = run_tapes_to_end(&exe, &registry, &seeds);
        // Baseline pin (mirrors session.rs's own equivalence tests): a
        // program that traps before doing anything would make the
        // pumped/sync equality below pass vacuously.
        assert_eq!(
            sync.outcome,
            Outcome::Stopped,
            "bit_flipper at {opt:?} baseline"
        );
        assert!(sync.stats.steps > 0, "bit_flipper at {opt:?} baseline");
        // The flip actually happened: "010" -> "101", head walked one cell
        // past onto the blank that stopped it.
        assert_eq!(
            sync_snaps[0].cells,
            vec![2, 1, 2, 0],
            "bit_flipper at {opt:?} baseline: the sync run must genuinely flip every bit"
        );
        let (pumped, pumped_snaps) = pump_tapes_to_end(&exe, &registry, &seeds);
        assert_eq!(pumped, sync, "bit_flipper at {opt:?}: RunResult");
        assert_eq!(
            pumped_snaps, sync_snaps,
            "bit_flipper at {opt:?}: final tape"
        );
    }

    // `MARK_COPIER` is two-tape, both bands cardinality 2 ('_' '*'). Seed
    // tape `a` with a run of 3 marks, tape `b` blank.
    for opt in [OptLevel::O0, OptLevel::O1] {
        let exe = build(MARK_COPIER, opt);
        let registry = registry_for(&exe);
        // The compiled program dispatches through `mtc`/`djmp` on a
        // non-blank alphabet's match table — a real per-band table ROM, the
        // half of `async_session_tapes` (vs. `async_session`) this test set
        // exists to exercise. An empty ROM would trap `mtc` under the
        // pumped path (TableOutOfBounds) and diverge from the sync run, so
        // this pins that the exercise is genuine rather than accidental.
        assert!(
            !exe.tables.is_empty(),
            "mark_copier at {opt:?}: the linked image must carry a non-empty table ROM"
        );
        let seeds: [&[u32]; 2] = [&[1, 1, 1], &[]]; // a: "***", b: blank
        let (sync, sync_snaps) = run_tapes_to_end(&exe, &registry, &seeds);
        assert_eq!(
            sync.outcome,
            Outcome::Stopped,
            "mark_copier at {opt:?} baseline"
        );
        assert!(sync.stats.steps > 0, "mark_copier at {opt:?} baseline");
        // The copy actually happened: tape `a` untouched, tape `b` now
        // carries the same run of 3 marks (head walked one cell past onto
        // the blank that stopped the copy).
        assert_eq!(
            sync_snaps[1].cells,
            vec![1, 1, 1, 0],
            "mark_copier at {opt:?} baseline: the sync run must genuinely copy the mark run"
        );
        let (pumped, pumped_snaps) = pump_tapes_to_end(&exe, &registry, &seeds);
        assert_eq!(pumped, sync, "mark_copier at {opt:?}: RunResult");
        assert_eq!(
            pumped_snaps, sync_snaps,
            "mark_copier at {opt:?}: final tapes"
        );
    }
}
