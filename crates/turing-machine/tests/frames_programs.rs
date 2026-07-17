//! The phase-5a milestone: a hand-written `.tma` program exercises the whole
//! frames stack end to end — `.frame`/`.map`/`.exits` authoring, the framed
//! call `call.m`, per-tape projection + symbol translation, the trap taxonomy,
//! and the multi-exit return `retx`.
//!
//! The program is a **test-local string** rather than a `docs/examples/`
//! artifact: it is a focused mechanism proof (every branch exists to reach one
//! observable), not polished teaching material like the brainfuck UTM.
//!
//! ## The program
//!
//! A 4-tape `main` calls a 2-arity `helper` through the frame `Fh`, which
//! projects the caller's virtual tapes `(0, 1)` onto physical tapes `(2, 0)`:
//!
//! - virtual tape 0 → physical tape 2, read through `rmap=(1->1, 3=>0)`:
//!   physical `0`→virtual `0` (the pinned blank), `1`→`1`, `2`→**hole**
//!   (reading it is an unmapped read), `3`→`0` (a foreign marker folded onto
//!   blank — the canonical one-way collapse, spelled `=>`);
//! - virtual tape 1 → physical tape 0, written through `wmap=(2->1)`: virtual
//!   `0`→physical `0`, virtual `1`→**hole** (writing it is an unmapped write),
//!   virtual `2`→physical `1`.
//!
//! `helper` reads both virtual tapes (`rd`), matches the pair against `TH`,
//! dispatches through `DH`, then writes/moves and returns through one of two
//! exits: `retx #0` → `done` (main's `stp`, Stopped), `retx #1` → `alt`
//! (main's `hlt`, Halted). The match rows steer the outcome by seed:
//!
//! | virtual read `[v0, v1]` | MR | branch  | effect                       |
//! |-------------------------|----|---------|------------------------------|
//! | `[0, 1]`                | 1  | b_alt   | writes virtual 1 → wmap hole → UnmappedWrite |
//! | `[1, 0]`                | 2  | b_halt  | valid write, `retx #1` → Halted |
//! | `[1, 1]`                | 3  | b_happy | valid write, `retx #0` → Stopped |
//! | anything else           | 0  | —       | `djmp` on MR=0 → NoTransition |

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::PROFILE_FRAMES;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::vm::{DebugEvent, Outcome, PauseCause, RunLimits, RunOptions, Tape, Trap, WideTape};
use mtc_turing_machine::asm::{assemble, link};
use mtc_turing_machine::cli::{execute, execute_with};

/// The milestone program. `.frame Fh` projects virtual `(0,1)` → physical
/// `(2,0)` with a collapse-onto-blank (`3=>0`) and a hole (physical `2`) in
/// the read map and a hole (virtual `1`) in the write map; two exits.
const MILESTONE: &str = "\
.routine main, tapes=4, alpha=(2, 2, 4, 2)
.routine helper, tapes=2, alpha=(4, 4)
.section tables
TH: .row    [0, 1]
    .row    [1, 0]
    .row    [1, 1]
DH: .targets b_alt, b_halt, b_happy
Fh: .frame  tapes=(2, 0)
    .map    0, rmap=(1->1, 3=>0)
    .map    1, wmap=(2->1)
    .exits  done, alt
.section code
.func main
        call.m  helper, Fh
done:   stp
alt:    hlt
.func helper
        rd
        mtc     TH
        djmp    DH
b_alt:  wr      [-, 1]
        retx    #1
b_halt: wr      [1, -]
        mov     [>, .]
        retx    #1
b_happy:
        wr      [1, 2]
        mov     [>, <]
        retx    #0
";

/// Per-tape alphabet cardinalities, from `main`'s `.routine`.
const WIDTHS: [u32; 4] = [2, 2, 4, 2];

/// A width-`w` tape carrying a single seeded cell at the origin (symbol `0`
/// yields a blank tape — the erase in `WideTape::set`).
fn seeded(w: u32, sym: u8) -> WideTape {
    WideTape::from_snapshot(
        &TapeSnapshot {
            origin: 0,
            cells: vec![sym],
            head: 0,
            alphabet: None,
        },
        w,
    )
    .expect("seed fits the width")
}

/// Assemble + link the milestone (or a variant), returning the executable.
fn build(src: &str) -> mtc_core::formats::executable::Executable {
    let obj = assemble(src, false).expect("the milestone assembles");
    link(&[obj], &[], mtc_core::linker::LinkOptions::default())
        .expect("the milestone links")
        .executable
}

/// Run `src` over four seeded tapes; `None` seeds a blank tape. Returns the
/// outcome and the four final snapshots.
fn run(src: &str, seeds: [Option<u8>; 4]) -> (Outcome, [TapeSnapshot; 4]) {
    let exe = build(src);
    let registry = {
        let mut r = mtc_core::vm::ArchRegistry::new();
        r.register(Box::new(mtc_turing_machine::arch::Tm1::new(exe.tape_count)));
        r
    };
    let machine = mtc_core::vm::Machine::from_executable(&exe, &registry).expect("loads");

    let mut t0 = seeds[0].map_or_else(|| WideTape::new(WIDTHS[0]), |s| seeded(WIDTHS[0], s));
    let mut t1 = seeds[1].map_or_else(|| WideTape::new(WIDTHS[1]), |s| seeded(WIDTHS[1], s));
    let mut t2 = seeds[2].map_or_else(|| WideTape::new(WIDTHS[2]), |s| seeded(WIDTHS[2], s));
    let mut t3 = seeds[3].map_or_else(|| WideTape::new(WIDTHS[3]), |s| seeded(WIDTHS[3], s));
    let mut devices: Vec<&mut dyn Tape> = vec![&mut t0, &mut t1, &mut t2, &mut t3];
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
    (
        result.outcome,
        [
            t0.to_snapshot(),
            t1.to_snapshot(),
            t2.to_snapshot(),
            t3.to_snapshot(),
        ],
    )
}

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

#[test]
fn milestone_links_to_the_frames_profile() {
    // A frame descriptor + a framed call ⇒ the linker emits PROFILE_FRAMES.
    assert_eq!(build(MILESTONE).profile, PROFILE_FRAMES);
    assert_eq!(build(MILESTONE).tape_count, 4);
}

#[test]
fn milestone_happy_path_stops_with_the_derived_tapes() {
    // Seed: physical 2 = 1 (→ virtual 0 = 1), physical 0 = 1 (→ virtual 1 = 1)
    // ⇒ read [1, 1] ⇒ MR 3 ⇒ b_happy. wr [1, 2]: virtual 0 ← 1 (identity wmap
    // → physical 1 on tape 2); virtual 1 ← 2 (wmap 2→1, so physical 1 on tape
    // 0 — the non-identity write translation). mov [>, <]: tape 2 head right,
    // tape 0 head left. retx #0 → done → stp.
    let (outcome, tapes) = run(MILESTONE, [Some(1), None, Some(1), None]);
    assert_eq!(outcome, Outcome::Stopped);
    // Hand-derived, independent of the run:
    //  t0 (phys 0): wrote 1 at 0 (seed already 1), head moved left to -1.
    //  t1: untouched blank.
    //  t2 (phys 2): wrote 1 at 0 (seed already 1), head moved right to 1.
    //  t3: untouched blank.
    assert_eq!(tapes[0], snap(-1, &[0, 1], -1));
    assert_eq!(tapes[1], snap(0, &[0], 0));
    assert_eq!(tapes[2], snap(0, &[1, 0], 1));
    assert_eq!(tapes[3], snap(0, &[0], 0));
}

#[test]
fn milestone_halt_path_returns_through_the_second_exit() {
    // Seed: physical 2 = 1, physical 0 = 0 ⇒ read [1, 0] ⇒ MR 2 ⇒ b_halt.
    // wr [1, -]: virtual 0 ← 1 (physical 1 on tape 2); virtual 1 kept.
    // mov [>, .]: tape 2 head right. retx #1 → alt → hlt (Halted) — the
    // second, distinct exit vector entry.
    let (outcome, tapes) = run(MILESTONE, [Some(0), None, Some(1), None]);
    assert_eq!(outcome, Outcome::Halted);
    assert_eq!(tapes[0], snap(0, &[0], 0)); // blank, untouched
    assert_eq!(tapes[1], snap(0, &[0], 0));
    assert_eq!(tapes[2], snap(0, &[1, 0], 1));
    assert_eq!(tapes[3], snap(0, &[0], 0));
}

#[test]
fn milestone_trap_taxonomy_is_distinct() {
    // Four different seeds drive the same program to four distinct trap
    // outcomes — the spec-critical taxonomy invariant.

    // (1) Unmapped READ: physical 2 = 2 is a rmap hole; `rd` traps on settle,
    //     before any match. The write map is never consulted.
    let (read_trap, _) = run(MILESTONE, [Some(0), None, Some(2), None]);
    assert!(
        matches!(read_trap, Outcome::Trapped(Trap::UnmappedRead { .. })),
        "physical 2 (a read hole) must trap UnmappedRead: {read_trap:?}"
    );

    // (2) Unmapped WRITE: physical 2 = 3 folds onto blank (virtual 0), physical
    //     0 = 1 ⇒ read [0, 1] ⇒ MR 1 ⇒ b_alt, which writes virtual 1 — a wmap
    //     hole. The read (through the collapse) succeeds; the write traps.
    let (write_trap, _) = run(MILESTONE, [Some(1), None, Some(3), None]);
    assert!(
        matches!(write_trap, Outcome::Trapped(Trap::UnmappedWrite { .. })),
        "writing virtual 1 (a write hole) must trap UnmappedWrite: {write_trap:?}"
    );

    // (3) No transition: read [0, 0] matches no `TH` row ⇒ MR 0 ⇒ `djmp` traps.
    //     Distinct from the two unmapped traps.
    let (no_row, _) = run(MILESTONE, [Some(0), None, Some(0), None]);
    assert!(
        matches!(no_row, Outcome::Trapped(Trap::NoTransition { .. })),
        "an unmatched read must trap NoTransition: {no_row:?}"
    );

    // Distinctness: the three trap kinds are pairwise different.
    assert_ne!(read_trap, write_trap);
    assert_ne!(read_trap, no_row);
    assert_ne!(write_trap, no_row);
}

#[test]
fn milestone_retx_past_the_exit_vector_traps_exit_out_of_range() {
    // A variant whose happy branch returns through `retx #2` — the exit
    // vector has only two entries (0, 1), so index 2 is out of range.
    let variant = MILESTONE.replace("retx    #0", "retx    #2");
    let (outcome, _) = run(&variant, [Some(1), None, Some(1), None]);
    assert!(
        matches!(outcome, Outcome::Trapped(Trap::ExitOutOfRange { .. })),
        "retx #2 past a 2-entry exit vector must trap ExitOutOfRange: {outcome:?}"
    );
}

#[test]
fn milestone_debug_shows_fr_rise_and_fall_with_call_depth() {
    // Stepping the happy path: fr() is 0 outside the frame, non-zero inside
    // the framed call, and 0 again after retx; the return depth mirrors a
    // plain call/ret (rises to exactly 1, falls back to 0 once).
    let exe = build(MILESTONE);
    let mut registry = mtc_core::vm::ArchRegistry::new();
    registry.register(Box::new(mtc_turing_machine::arch::Tm1::new(exe.tape_count)));
    let machine = mtc_core::vm::Machine::from_executable(&exe, &registry).expect("loads");
    let mut session = machine.debug_tapes(RunOptions::default());

    let mut t0 = seeded(WIDTHS[0], 1);
    let mut t1 = WideTape::new(WIDTHS[1]);
    let mut t2 = seeded(WIDTHS[2], 1);
    let mut t3 = WideTape::new(WIDTHS[3]);
    let mut devs: [&mut dyn Tape; 4] = [&mut t0, &mut t1, &mut t2, &mut t3];

    // Before the first step: identity frame, empty stack.
    assert_eq!(session.fr(), 0);
    assert_eq!(session.depth(), 0);

    let mut fr_seq = Vec::new();
    let mut depth_seq = Vec::new();
    loop {
        match session.step_in_tapes(&mut devs) {
            DebugEvent::Paused(PauseCause::Step) => {
                fr_seq.push(session.fr());
                depth_seq.push(session.depth());
            }
            DebugEvent::Finished(outcome) => {
                assert_eq!(outcome, Outcome::Stopped);
                break;
            }
            other => panic!("unexpected debug event: {other:?}"),
        }
    }

    // fr() starts at 0 (main's ent), rises inside the frame, and is 0 again
    // after retx.
    assert_eq!(fr_seq.first(), Some(&0), "identity frame before the call");
    assert_eq!(
        fr_seq.last(),
        Some(&0),
        "identity frame restored after retx"
    );
    assert!(
        fr_seq.iter().any(|&f| f != 0),
        "FR non-zero inside the framed call: {fr_seq:?}"
    );
    // Depth rises to exactly 1 (one call frame) and returns to 0 — retx pops
    // one entry, exactly like a plain ret.
    assert_eq!(depth_seq.first(), Some(&0));
    assert_eq!(depth_seq.last(), Some(&0));
    assert_eq!(
        *depth_seq.iter().max().unwrap(),
        1,
        "the framed call nests exactly one deep: {depth_seq:?}"
    );
}

// -- CLI: the same program through `tmt asm` / `link` / `run` in-process ----

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// asm MILESTONE.tma → link → mint a fresh tape band; returns (exe, tape).
fn cli_setup(dir: &Path) -> (PathBuf, PathBuf) {
    let src = dir.join("milestone.tma");
    fs::write(&src, MILESTONE).unwrap();
    let obj = dir.join("milestone.tmo");
    execute(&args(&[
        "asm",
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]))
    .unwrap();
    let exe = dir.join("milestone.tmx");
    execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "-o",
        exe.to_str().unwrap(),
    ]))
    .unwrap();
    let tape = dir.join("milestone.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    (exe, tape)
}

/// Seed physical tape 2 = 1 and physical tape 0 = 1 (the happy [1, 1] read).
fn seed_happy(tape: &Path) {
    for band in ["2", "0"] {
        execute(&args(&[
            "tape",
            "set",
            tape.to_str().unwrap(),
            "--in-place",
            "--tape",
            band,
            "--cells",
            "1",
        ]))
        .unwrap();
    }
}

#[test]
fn cli_happy_path_runs_and_exits_zero() {
    let dir = scratch("frames_cli_happy");
    let (exe, tape) = cli_setup(&dir);
    seed_happy(&tape);
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0, "framed happy path exits 0:\n{}", out.stdout);
    assert!(out.stdout.contains("Stopped"), "{}", out.stdout);
}

#[test]
fn cli_blank_tape_traps_no_transition_and_exits_three() {
    let dir = scratch("frames_cli_trap");
    let (exe, tape) = cli_setup(&dir);
    // A blank band reads [0, 0] inside the frame ⇒ NoTransition ⇒ exit 3. The
    // frame is still entered (call.m activates it before the helper's djmp).
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 3, "trapped run exits 3:\n{}", out.stdout);
    assert!(out.stdout.contains("Trapped"), "{}", out.stdout);
}

#[test]
fn cli_trace_shows_fr_zero_outside_and_non_zero_inside_the_frame() {
    let dir = scratch("frames_cli_trace");
    let (exe, tape) = cli_setup(&dir);
    seed_happy(&tape);
    let mut trace = Vec::new();
    let out = execute_with(
        &args(&[
            "run",
            exe.to_str().unwrap(),
            "--tape",
            tape.to_str().unwrap(),
            "--trace",
        ]),
        &mut trace,
    )
    .unwrap();
    assert_eq!(out.code, 0, "traced happy run exits 0:\n{}", out.stdout);
    let trace = String::from_utf8(trace).unwrap();
    // The frames profile appends ` FR=<n>` to every trace line.
    assert!(trace.contains(" FR="), "frames trace carries FR=:\n{trace}");
    // FR is 0 on the main-side lines (identity frame) …
    assert!(
        trace.contains(" FR=0"),
        "FR is 0 outside the framed call:\n{trace}"
    );
    // … and non-zero on at least one helper-side line (inside the frame).
    assert!(
        trace
            .lines()
            .any(|l| l.contains(" FR=") && !l.contains(" FR=0")),
        "FR is non-zero inside the framed call:\n{trace}"
    );
}
