//! The phase-6a MILESTONE: the spec's six Appendix A `.tmc` examples
//! (docs/superpowers/specs/2026-07-16-tm1-and-tmt-design.md, Appendix A)
//! compile, link, and RUN, plus a nested-graft case beyond Appendix A. Each
//! fixture is the VERBATIM spec text (comments and doc lines included), kept
//! as a durable teaching file under `tests/golden/`.
//!
//! Discipline, mirroring the PM-1 `golden_programs.rs` and the UTM
//! `golden_programs.rs`: every expected final state is DERIVED BY HAND in
//! this test (with the derivation spelled out in comments), the run must
//! reproduce the derivation, and the committed `.tmt` golden is byte-identical
//! to the derived block. Goldens are regenerated FROM the derivation
//! (`regen_goldens`, `#[ignore]`), never captured from run output.
//!
//! A.5 is exercised THREE WAYS — a happy path plus the two holey-map trap
//! seeds (`'a'` / `'b'` under the call, both `unmapped-read`) — across ALL
//! THREE `--call-mech` modes. The compiler's object is mode-independent: one
//! `.tmo`, three links, identical observable behaviour (spec §11.1).

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, Trap, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, compile};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Compile a `.tmc` fixture, asserting it compiles with ZERO diagnostics —
/// every Appendix A example (and the nested-graft case) is warning-free
/// (verified: no unused imports/routines/graphs, no shadowed rows, no
/// unreachable states). Returns the object.
fn object(fixture: &str) -> ObjectFile {
    let src = fs::read_to_string(golden_dir().join(fixture)).expect("fixture present");
    let out = compile(&src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("{fixture} must compile: {e}"));
    assert!(
        out.report.diagnostics.is_empty(),
        "{fixture} must compile warning-free, got {:?}",
        out.report.diagnostics
    );
    out.object
}

/// Link one object with the given options.
fn link_with(obj: &ObjectFile, options: LinkOptions) -> Executable {
    link(&[obj.clone()], &[], options)
        .unwrap_or_else(|e| panic!("link failed: {e}"))
        .executable
}

/// A concrete tape snapshot (`alphabet: None` — the wire shape `WideTape`
/// round-trips to).
fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

/// Run `exe` on `(seed, width)` tape bands built directly (no CLI round-trip,
/// matching the UTM golden's approach), returning the outcome and the final
/// per-tape snapshots.
fn run(exe: &Executable, seeds: &[(TapeSnapshot, u32)]) -> (Outcome, Vec<TapeSnapshot>) {
    let mut tapes: Vec<WideTape> = seeds
        .iter()
        .map(|(s, w)| WideTape::from_snapshot(s, *w).expect("seed fits its width"))
        .collect();
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
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
    let snaps = tapes.iter().map(WideTape::to_snapshot).collect();
    (result.outcome, snaps)
}

/// Wrap derived final snapshots into an MT block for byte comparison against
/// the committed `.tmt`. The block-level alphabet is decimal labels `0..W`
/// (W = the widest band); the snapshots carry no per-tape override, so the
/// block serializes in the v1 shared-alphabet shape (mirrors the UTM golden's
/// `block`). Deterministic from `widths`, so regen and assert agree byte-wise.
fn block(snaps: &[TapeSnapshot], widths: &[u32]) -> TapeBlockFile {
    let max = widths.iter().copied().max().unwrap_or(2);
    TapeBlockFile {
        alphabet: (0..max).map(|i| i.to_string()).collect(),
        tapes: snaps.to_vec(),
    }
}

/// Assert a committed golden is byte-identical to a derived block.
fn assert_golden(name: &str, derived: &TapeBlockFile) {
    let bytes = fs::read(golden_dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(bytes, derived.to_bytes(), "{name} drifted");
}

// ── the six Appendix A examples ─────────────────────────────────────────────

#[test]
fn a1_replace_b() {
    // ab = {'_'=0, 'a'=1, 'b'=2}. `scan` walks right, 'b'→'a', 'a' passes,
    // '_' stops. Seed "bab" (cells [2,1,2], head 0):
    //   [0]='b' → write 'a', >   ⇒ cell0='a'(1), head 1
    //   [1]='a' → >               ⇒ head 2
    //   [2]='b' → write 'a', >    ⇒ cell2='a'(1), head 3
    //   [3]='_' → stop            ⇒ head 3 (blank)
    let exe = link_with(&object("a1_replace_b.tmc"), LinkOptions::default());
    let (outcome, snaps) = run(&exe, &[(snap(0, &[2, 1, 2], 0), 3)]);
    assert_eq!(outcome, Outcome::Stopped);
    // Final "aaa" with the head on the trailing blank (span 0..3).
    let derived = [snap(0, &[1, 1, 1, 0], 3)];
    assert_eq!(snaps, derived);
    assert_golden("a1_replace_b.expected.tmt", &block(&derived, &[3]));
}

#[test]
fn a2_binary_plus_one() {
    // bits = {'_'=0, '0'=1, '1'=2}. `inc` starts on the LSB, carries LEFT.
    // Seed "11" = 3 (cells [2,2], head on the LSB = position 1):
    //   [1]='1' → write '0', <   ⇒ cell1='0'(1), head 0
    //   [0]='1' → write '0', <   ⇒ cell0='0'(1), head -1
    //   [-1]='_' → write '1', stop⇒ cell-1='1'(2), head -1
    // "100" = 4 = 3 + 1.
    let exe = link_with(&object("a2_binary_plus_one.tmc"), LinkOptions::default());
    let (outcome, snaps) = run(&exe, &[(snap(0, &[2, 2], 1), 3)]);
    assert_eq!(outcome, Outcome::Stopped);
    let derived = [snap(-1, &[2, 1, 1], -1)];
    assert_eq!(snaps, derived);
    assert_golden("a2_binary_plus_one.expected.tmt", &block(&derived, &[3]));
}

#[test]
fn a3_two_tape_copy() {
    // bits card 3. `copy` reads src ('0'..'1' bound to c), writes c to dst,
    // both heads step right, until src blank. Seed src "10" (cells [2,1],
    // head 0), dst blank:
    //   src[0]='1' → c=2 → dst[0]='1', >>  ⇒ heads 1
    //   src[1]='0' → c=1 → dst[1]='0', >>  ⇒ heads 2
    //   src[2]='_' → stop                   ⇒ heads 2
    let exe = link_with(&object("a3_two_tape_copy.tmc"), LinkOptions::default());
    let (outcome, snaps) = run(&exe, &[(snap(0, &[2, 1], 0), 3), (snap(0, &[], 0), 3)]);
    assert_eq!(outcome, Outcome::Stopped);
    // src unchanged (head on trailing blank); dst now carries the copy.
    let derived = [snap(0, &[2, 1, 0], 2), snap(0, &[2, 1, 0], 2)];
    assert_eq!(snaps, derived);
    assert_golden("a3_two_tape_copy.expected.tmt", &block(&derived, &[3, 3]));
}

#[test]
fn a4_byte_increment() {
    // bytes = 0..126 (127 symbols; index == value). `inc`:
    //   1..125 → write v+1, stop ; 126 → halt (overflow) ; 0 → write 1, stop.
    let obj = object("a4_byte_increment.tmc");
    let exe = link_with(&obj, LinkOptions::default());

    // Normal: value 5 → 6, stop (the committed golden).
    let (outcome, snaps) = run(&exe, &[(snap(0, &[5], 0), 127)]);
    assert_eq!(outcome, Outcome::Stopped);
    let derived = [snap(0, &[6], 0)];
    assert_eq!(snaps, derived);
    assert_golden("a4_byte_increment.expected.tmt", &block(&derived, &[127]));

    // Overflow: value 126 → halt, tape unchanged (the CLI halt-exit case).
    let (outcome, snaps) = run(&exe, &[(snap(0, &[126], 0), 127)]);
    assert_eq!(outcome, Outcome::Halted);
    assert_eq!(snaps, [snap(0, &[126], 0)]);

    // Blank cell = value 0 → write 1, stop.
    let (outcome, snaps) = run(&exe, &[(snap(0, &[], 0), 127)]);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(snaps, [snap(0, &[1], 0)]);
}

#[test]
fn a6_graph_graft_multi_exit_runs_both_exits() {
    // marks = {'_'=0, 'x'=1, 'y'=2, 'z'=3}. The entry graft `seek` (= findX's
    // walk) walks right: 'x' → celebrate (write '_', stop); '_' → giveUp
    // (halt); else step right.
    let obj = object("a6_graph_graft_multi_exit.tmc");
    let exe = link_with(&obj, LinkOptions::default());

    // x-found: seed "zx" (cells [3,1], head 0):
    //   [0]='z' → >                    ⇒ head 1
    //   [1]='x' → celebrate: write '_' ⇒ cell1='_'(0), stop, head 1
    let (outcome, snaps) = run(&exe, &[(snap(0, &[3, 1], 0), 4)]);
    assert_eq!(outcome, Outcome::Stopped);
    let derived = [snap(0, &[3, 0], 1)];
    assert_eq!(snaps, derived);
    assert_golden("a6_graph_graft_multi_exit.expected.tmt", &block(&derived, &[4]));

    // blank-found: seed "y" (cells [2], head 0):
    //   [0]='y' → >     ⇒ head 1
    //   [1]='_' → giveUp: halt (tape unchanged), head 1
    let (outcome, snaps) = run(&exe, &[(snap(0, &[2], 0), 4)]);
    assert_eq!(outcome, Outcome::Halted);
    assert_eq!(snaps, [snap(0, &[2, 0], 1)]);
}

// ── A.5: three ways × three call-mechs ──────────────────────────────────────

/// bits = {'_'=0, '0'=1, '1'=2}; wide = {'_'=0, 'a'=1, 'b'=2, '0'=3, '1'=4}.
/// `main` finds a '1' under `ctl`, then calls `plusOne` on `data` through the
/// map {'0'->'0', '1'->'1'} (wide 3↔bits 1, wide 4↔bits 2). 'a'/'b' (wide
/// 1/2) have NO preimage — reading one inside the call is `unmapped-read`.
const A5_WIDTHS: [u32; 2] = [3, 5];

#[test]
fn a5_happy_path_is_mode_independent() {
    // One object, three links. Seed ctl "1" (index 2, triggers the call) and
    // data "1" (wide index 4). plusOne increments through the map:
    //   data[0]='1'(w4→b'1') → write b'0'(→w3), < ⇒ cell0='0'(3), head -1
    //   data[-1]='_' (b'_')  → write b'1'(→w4), ret⇒ cell-1='1'(4), head -1
    // then `then done` → stop. ctl is unbound: keep+stay, unchanged.
    // "1" → "10" (=2) on the wide tape.
    let obj = object("a5_call_across_alphabets.tmc");
    let ctl = snap(0, &[2], 0);
    let data = snap(0, &[4], 0);
    let derived = [snap(0, &[2], 0), snap(-1, &[4, 3], -1)];

    for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
        let exe = link_with(
            &obj,
            LinkOptions {
                call_mech: mech,
                ..Default::default()
            },
        );
        let (outcome, snaps) = run(
            &exe,
            &[(ctl.clone(), A5_WIDTHS[0]), (data.clone(), A5_WIDTHS[1])],
        );
        assert_eq!(outcome, Outcome::Stopped, "{mech:?} happy path stops");
        assert_eq!(snaps, derived, "{mech:?} happy path tapes");
    }

    // The committed golden pins the mode-independent final block.
    assert_golden(
        "a5_call_across_alphabets.expected.tmt",
        &block(&derived, &A5_WIDTHS),
    );
}

#[test]
fn a5_holey_map_read_traps_unmapped_read_in_all_modes() {
    // The mandatory trap-path golden. Seed ctl "1" (triggers the call), then a
    // holey wide symbol under the data head: 'a' (wide 1) or 'b' (wide 2),
    // neither in the map's domain. plusOne's first read faults `unmapped-read`
    // — the same trap KIND on every mechanism (the `at` offset differs by
    // layout and is deliberately not compared).
    let obj = object("a5_call_across_alphabets.tmc");
    let ctl = snap(0, &[2], 0);
    for hole in [1u8 /* 'a' */, 2u8 /* 'b' */] {
        let data = snap(0, &[hole], 0);
        for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
            let exe = link_with(
                &obj,
                LinkOptions {
                    call_mech: mech,
                    ..Default::default()
                },
            );
            let (outcome, _) = run(
                &exe,
                &[(ctl.clone(), A5_WIDTHS[0]), (data.clone(), A5_WIDTHS[1])],
            );
            assert!(
                matches!(outcome, Outcome::Trapped(Trap::UnmappedRead { .. })),
                "wide symbol {hole} under {mech:?} must trap unmapped-read, got {outcome:?}"
            );
        }
    }
}

// ── nested graft (beyond Appendix A — the T5-review two-level splice) ────────

#[test]
fn nested_graft_two_levels_runs() {
    // marks = {'_'=0, 'x'=1, 'y'=2}. The machine grafts findXthenY, which
    // itself grafts findX: two splice levels flatten into one world. `run`
    // walks to an 'x', then (without moving) `run__seekY` walks on to a 'y'
    // → win (write '_', stop); a blank on either leg → lose (halt).
    let obj = object("nested_graft.tmc");
    let exe = link_with(&obj, LinkOptions::default());

    // happy: seed "xy" (cells [1,2], head 0):
    //   run[0]='x' → jmp seekY (no move)
    //   seekY[0]='x' → > ⇒ head 1
    //   seekY[1]='y' → win: write '_' ⇒ cell1='_'(0), stop, head 1
    let (outcome, snaps) = run(&exe, &[(snap(0, &[1, 2], 0), 3)]);
    assert_eq!(outcome, Outcome::Stopped);
    let derived = [snap(0, &[1, 0], 1)];
    assert_eq!(snaps, derived);
    assert_golden("nested_graft.expected.tmt", &block(&derived, &[3]));

    // lose: seed "x" (cells [1], head 0) — an 'x' but no following 'y':
    //   run[0]='x' → jmp seekY ; seekY[0]='x' → > ; seekY[1]='_' → lose: halt
    let (outcome, snaps) = run(&exe, &[(snap(0, &[1], 0), 3)]);
    assert_eq!(outcome, Outcome::Halted);
    assert_eq!(snaps, [snap(0, &[1, 0], 1)]);
}

// ── golden regeneration (derivation-first; explicit) ─────────────────────────

/// Regenerate the committed `.tmt` goldens FROM THE HAND DERIVATIONS (never
/// from run output). Keep the derived blocks here in lockstep with the tests
/// above. Run explicitly:
///   cargo test -p mtc-turing-machine --test tmc_golden regen -- --ignored
#[test]
#[ignore = "writes the golden files; run explicitly"]
fn regen_goldens() {
    let write = |name: &str, b: &TapeBlockFile| {
        fs::write(golden_dir().join(name), b.to_bytes()).unwrap();
    };
    write(
        "a1_replace_b.expected.tmt",
        &block(&[snap(0, &[1, 1, 1, 0], 3)], &[3]),
    );
    write(
        "a2_binary_plus_one.expected.tmt",
        &block(&[snap(-1, &[2, 1, 1], -1)], &[3]),
    );
    write(
        "a3_two_tape_copy.expected.tmt",
        &block(&[snap(0, &[2, 1, 0], 2), snap(0, &[2, 1, 0], 2)], &[3, 3]),
    );
    write(
        "a4_byte_increment.expected.tmt",
        &block(&[snap(0, &[6], 0)], &[127]),
    );
    write(
        "a5_call_across_alphabets.expected.tmt",
        &block(&[snap(0, &[2], 0), snap(-1, &[4, 3], -1)], &A5_WIDTHS),
    );
    write(
        "a6_graph_graft_multi_exit.expected.tmt",
        &block(&[snap(0, &[3, 0], 1)], &[4]),
    );
    write(
        "nested_graft.expected.tmt",
        &block(&[snap(0, &[1, 0], 1)], &[3]),
    );
}
