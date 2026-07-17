//! The phase-5 milestone: **three-mode equivalence**. Mono, frames, and
//! hybrid lower the same declarative bound-call programs three different ways —
//! stamped copies, a runtime compose table, or a per-site mix — yet they MUST
//! be observably identical on the same programs and tapes (docs/formats.md
//! (frames profile), the mode semantics summary). This harness mirrors
//! `crates/post-machine/tests/opt_equivalence.rs`: `build`/`run`/
//! `assert_equivalent`, one `.tma` source per program, the full behavioral
//! tuple compared across modes.
//!
//! The behavioral tuple is `(outcome kind, per-tape snapshots, heads)`. Two
//! things are deliberately EXCLUDED from the compare:
//!
//! - a trap's `at` offset — mono and frames lay code out differently, so the
//!   faulting address legitimately differs; the trap KIND is the invariant
//!   (the trap-taxonomy claim, GC5);
//! - `stats`/`ip`/`stack` — a stamp and a compose-table lookup cost different
//!   tacts by design (the O(1)-per-call frame overhead is measured separately,
//!   in the depth-independence test below).
//!
//! Snapshots and heads are compared STRICTLY: a divergence there is a real
//! engine bug this harness exists to catch, never something to loosen away.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions, LinkOutput, LinkReport};
use mtc_core::vm::{
    ArchRegistry, Machine, Outcome, RunLimits, RunOptions, RunStats, Tape, Trap, WideTape,
};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{assemble, link};

// ── harness ────────────────────────────────────────────────────────────────

/// Assemble + link `src` under `mech`; returns the whole link output so a
/// caller can read the executable, the sidecar map, or the report.
fn build_full(src: &str, mech: CallMech) -> LinkOutput {
    let obj = assemble(src, false).expect("assembles");
    link(
        &[obj],
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the {mech} link failed: {e}"))
}

/// The executable + report — the common `build` shape (the map is dropped).
fn build(src: &str, mech: CallMech) -> (Executable, LinkReport) {
    let out = build_full(src, mech);
    (out.executable, out.report)
}

/// A trap's KIND, stripped of its `at` offset (docs/formats.md (frames
/// profile), GC5). Exhaustive on purpose: a new `Trap` variant must be named
/// here rather than silently folded into a catch-all, which could mask a
/// cross-mode divergence into two distinct kinds reading as one.
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

/// The mode-independent behavioral outcome: `stopped`/`halted`, or a trap KIND.
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
/// snapshots, per-tape final heads. (Heads are also inside the snapshots; kept
/// separate to make the compare's intent explicit.)
struct Observed {
    outcome: String,
    snaps: Vec<TapeSnapshot>,
    heads: Vec<i64>,
    stats: RunStats,
}

/// Run `exe` on `seeds`, one seeded `WideTape` per physical tape (width from
/// the image's per-tape alphabet cardinalities).
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
                    max_steps: Some(100_000),
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
        stats: result.stats,
    }
}

/// Build all three modes and assert the behavioral tuple is identical across
/// mono/frames/hybrid on every case. This is the milestone contract.
fn assert_equivalent(src: &str, cases: &[Case]) {
    let (mono, _) = build(src, CallMech::Mono);
    let (frames, _) = build(src, CallMech::Frames);
    let (hybrid, _) = build(src, CallMech::Hybrid);
    for (i, case) in cases.iter().enumerate() {
        let m = run(&mono, case);
        let f = run(&frames, case);
        let h = run(&hybrid, case);
        assert_eq!(
            (&m.outcome, &m.snaps, &m.heads),
            (&f.outcome, &f.snaps, &f.heads),
            "MONO vs FRAMES diverged on case {i} ({case:?})"
        );
        assert_eq!(
            (&f.outcome, &f.snaps, &f.heads),
            (&h.outcome, &h.snaps, &h.heads),
            "FRAMES vs HYBRID diverged on case {i} ({case:?})"
        );
    }
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

// ── program (a): the cross-alphabet holey one-way call ───────────────────────

/// A delimited-world caller (5-symbol alphabet: 0 blank, 1/2 data, 3/4 boundary
/// markers) declaratively calls a bare-representation callee (3-symbol: 0 blank,
/// 1/2 data) through a HOLEY ONE-WAY binding. The two markers collapse onto the
/// callee's blank (`3=>0, 4=>0`) — the canonical "run a bare routine inside a
/// delimited region, markers read as blank, left intact" shape from the spec's
/// §5. The callee walks right transforming data (1→2, 2→1) until it reads a
/// blank (a marker or a real blank), then returns.
const CROSS_ALPHABET: &str = "\
.routine main, tapes=1, alpha=(5)
.routine bare, tapes=1, alpha=(3)
.section tables
T:  .row [0]
    .row [1]
    .row [2]
D:  .targets fin, wa, wb
.section code
.func main
        call    bare [0{1->1, 2->2, 3=>0, 4=>0}]
        stp
.func bare
scan:   rd
        mtc     T
        djmp    D
wa:     wr      [2]
        mov     [>]
        jmp     scan
wb:     wr      [1]
        mov     [>]
        jmp     scan
fin:    ret
";

#[test]
fn cross_alphabet_holey_one_way_is_equivalent() {
    assert_equivalent(
        CROSS_ALPHABET,
        &[
            &[(&[1], 0)],          // one data cell 'a'
            &[(&[2], 0)],          // one data cell 'b'
            &[(&[1, 2], 0)],       // two data cells → swap walk
            &[(&[3], 0)],          // head starts on a marker → returns intact
            &[(&[3, 1, 2, 4], 1)], // ^ a b $, head on first data
            &[(&[], 0)],           // blank tape
        ],
    );
}

#[test]
fn cross_alphabet_happy_path_matches_the_hand_derived_tape() {
    // GC "all three wrong together" guard: pin the happy path to an oracle the
    // cross-mode compare can't provide. Seed one data cell 'a' (physical 1),
    // head 0:
    //   head 0: physical 1 → rmap 1->1 → virtual 1 → MR 2 → wa: wr virtual 2
    //           (wmap identity → physical 2), mov right. head → 1.
    //   head 1: blank → virtual 0 → MR 1 → fin → ret. main: stp.
    // Final tape: cell 0 = physical 2, head at 1, everything else blank.
    let (frames, _) = build(CROSS_ALPHABET, CallMech::Frames);
    let obs = run(&frames, &[(&[1], 0)]);
    assert_eq!(obs.outcome, "stopped");
    assert_eq!(
        cell_at(&obs.snaps[0], 0),
        2,
        "'a' transformed to physical 2"
    );
    assert_eq!(obs.snaps[0].head, 1, "walked one cell right, then returned");

    // And the two-cell swap walk, derived independently: [1,2] → [2,1], head 2.
    let obs = run(&frames, &[(&[1, 2], 0)]);
    assert_eq!(obs.outcome, "stopped");
    assert_eq!(cell_at(&obs.snaps[0], 0), 2);
    assert_eq!(cell_at(&obs.snaps[0], 1), 1);
    assert_eq!(obs.snaps[0].head, 2);

    // The marker-collapse case: head on a marker reads as blank → immediate
    // return, marker left intact (the one-way collapse never writes it).
    let obs = run(&frames, &[(&[3], 0)]);
    assert_eq!(obs.outcome, "stopped");
    assert_eq!(
        cell_at(&obs.snaps[0], 0),
        3,
        "the boundary marker is intact"
    );
    assert_eq!(obs.snaps[0].head, 0);
}

// ── program (b): two-level nested composition, a row-varying column ──────────

/// `main` calls `R` under two different contexts (swap12, swap13); `R` binds
/// `Q` (swap23), `Q` binds `S` (swap12). So the chain composites through three
/// levels and the SAME site — Q's `call S` — is reached under two distinct
/// active frames, making its compose-table column ROW-VARYING (the T4-review
/// coverage item, exercised end to end at run time). Every composite here is a
/// product of an ODD number of transpositions, so NONE collapses to identity —
/// every level stays a real `call.m` in frames mode.
const NESTED_TWO_LEVEL: &str = "\
.routine main, tapes=1, alpha=(4)
.routine R,    tapes=1, alpha=(4)
.routine Q,    tapes=1, alpha=(4)
.routine S,    tapes=1, alpha=(4)
.section code
.func main
        call    R [0{1->2, 2->1}]
        mov     [>]
        call    R [0{1->3, 3->1}]
        stp
.func R
        call    Q [0{2->3, 3->2}]
        ret
.func Q
        call    S [0{1->2, 2->1}]
        ret
.func S
        wr      [1]
        ret
";

#[test]
fn nested_two_level_composition_is_equivalent() {
    assert_equivalent(NESTED_TWO_LEVEL, &[&[(&[0], 0)]]);
}

#[test]
fn nested_two_level_row_varying_column_selects_per_context() {
    // In frames mode Q's single `call S` site is reached under two active
    // frames; a row-varying compose column must pick a DIFFERENT composite for
    // each, so S writes a different physical symbol per context — observable at
    // run time. main writes context 1's result at cell 0, steps right, then
    // context 2's at cell 1.
    let (frames, report) = build(NESTED_TWO_LEVEL, CallMech::Frames);
    assert!(
        report.composites >= 3,
        "the chain enumerates several distinct composites: {}",
        report.composites
    );
    let obs = run(&frames, &[(&[0], 0)]);
    assert_eq!(obs.outcome, "stopped");
    let c0 = cell_at(&obs.snaps[0], 0);
    let c1 = cell_at(&obs.snaps[0], 1);
    assert_ne!(c0, 0, "context 1 wrote a symbol");
    assert_ne!(c1, 0, "context 2 wrote a symbol");
    assert_ne!(
        c0, c1,
        "the two contexts select different composites (row-varying column)"
    );
}

// ── program (c): an equal-size bijection (hybrid stamps it) ──────────────────

/// A completed bijection — `swap` maps 1↔2 over a 4-symbol alphabet, equal-size
/// with the caller. Hybrid classifies this as stampable (base-profile mono);
/// pure frames runs it through the compose table. The two produce the same
/// tape but a DIFFERENT `LinkReport.instantiations`: frames stamps nothing,
/// hybrid stamps one copy.
const EQUAL_SIZE_BIJECTION: &str = "\
.routine main, tapes=1, alpha=(4)
.routine swap, tapes=1, alpha=(4)
.section code
.func main
        call    swap [0{1->2, 2->1}]
        stp
.func swap
        wr      [1]
        ret
";

#[test]
fn equal_size_bijection_is_equivalent() {
    assert_equivalent(EQUAL_SIZE_BIJECTION, &[&[(&[0], 0)]]);
}

#[test]
fn hybrid_stamps_the_bijection_frames_does_not() {
    let (_, frames) = build(EQUAL_SIZE_BIJECTION, CallMech::Frames);
    let (_, hybrid) = build(EQUAL_SIZE_BIJECTION, CallMech::Hybrid);
    assert_eq!(
        frames.instantiations, 0,
        "frames mode stamps nothing — it uses the compose table"
    );
    assert!(
        hybrid.instantiations >= 1,
        "hybrid stamps the completed bijection: {}",
        hybrid.instantiations
    );
    // The bijection is holeless, so hybrid emits no frames region at all.
    let (hybrid_exe, _) = build(EQUAL_SIZE_BIJECTION, CallMech::Hybrid);
    assert_eq!(
        hybrid_exe.frames_offset, 0,
        "an all-stamped hybrid image carries no frames region"
    );
}

// ── program (d): the trap taxonomy — one program, three trap kinds ───────────

/// A 2-tape caller drives a 2-tape callee across the whole trap taxonomy. Tape
/// 0 is caller-wider through a SWAP (1↔2) binding — physical 3 has no virtual
/// image, so it is a read hole; the swap keeps the composite non-identity so
/// the frame is real (an all-identity binding would relax to a plain call and
/// drop the cardinality hole). Tape 1 is callee-wider — virtual 3 has no
/// physical image, a write hole. The callee reads both heads, matches the
/// virtual pair, and dispatches. The three trap kinds are reached by seed
/// (physical symbols; the swap sends physical 1→virtual 2, physical 2→virtual
/// 1 on tape 0):
///
/// | seed (tape0, tape1) | virtual read | what happens                        |
/// |---------------------|--------------|-------------------------------------|
/// | (3, 0)              | —            | `rd` on tape-0 read hole → **UnmappedRead** |
/// | (2, 0)              | [1, 0]       | MR 1 → B writes virtual 3 → tape-1 write hole → **UnmappedWrite** |
/// | (1, 1)              | [2, 1]       | matches no row → **NoTransition**   |
/// | (1, 0)              | [2, 0]       | MR 2 → C writes virtual 1 → physical 2 (ok) |
///
/// The trap KIND must be identical across all three modes: mono raises it
/// through synthesized rows / `trap` stubs, frames through map sentinels — the
/// deepest claim of the design (GC5).
const TRAP_TAXONOMY: &str = "\
.routine main, tapes=2, alpha=(4, 3)
.routine sub,  tapes=2, alpha=(3, 4)
.section tables
T:  .row [1, 0]
    .row [2, 0]
D:  .targets B, C
.section code
.func main
        call    sub [0{1->2, 2->1}, 1{1->1, 2->2}]
        stp
.func sub
        rd
        mtc     T
        djmp    D
B:      wr      [-, 3]
        ret
C:      wr      [1, -]
        ret
";

#[test]
fn trap_taxonomy_is_equivalent_across_modes() {
    assert_equivalent(
        TRAP_TAXONOMY,
        &[
            &[(&[3], 0), (&[0], 0)], // UnmappedRead
            &[(&[2], 0), (&[0], 0)], // UnmappedWrite
            &[(&[1], 0), (&[1], 0)], // NoTransition
            &[(&[1], 0), (&[0], 0)], // happy path
        ],
    );
}

#[test]
fn trap_taxonomy_kinds_are_distinct_and_mode_invariant() {
    // Assert the ACTUAL kinds (not just cross-mode agreement) so the taxonomy
    // is pinned: each seed reaches exactly the intended trap, in every mode.
    let cases: &[(Case, &str)] = &[
        (&[(&[3], 0), (&[0], 0)], "trapped:unmapped-read"),
        (&[(&[2], 0), (&[0], 0)], "trapped:unmapped-write"),
        (&[(&[1], 0), (&[1], 0)], "trapped:no-transition"),
        (&[(&[1], 0), (&[0], 0)], "stopped"),
    ];
    for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
        let (exe, _) = build(TRAP_TAXONOMY, mech);
        for (seed, expected) in cases {
            let obs = run(&exe, seed);
            assert_eq!(
                &obs.outcome, expected,
                "{mech}: seed {seed:?} must be {expected}"
            );
        }
    }
    // Distinctness: the three trap kinds are pairwise different names.
    assert_ne!("trapped:unmapped-read", "trapped:unmapped-write");
    assert_ne!("trapped:unmapped-read", "trapped:no-transition");
    assert_ne!("trapped:unmapped-write", "trapped:no-transition");
}

// ── program (e): a narrower-callee identity binding (the cardinality hole) ───

/// An ALL-IDENTITY binding (`[0]`, no explicit pairs) into a NARROWER callee:
/// the caller's 4-symbol alphabet passes straight through to a 3-symbol
/// `bare`, so caller symbol 3 has no image in the callee — a read hole. This
/// is the shape that must NOT collapse to a plain call: collapsing (the
/// pre-fix, cardinality-blind behavior) would let physical 3 flow into `bare`
/// raw and silently miss the `UnmappedRead` trap the hole owes. Because that
/// miss is mode-consistent — all three modes would collapse identically — the
/// cross-mode compare alone cannot catch it, so the kind-pinning test below is
/// the oracle. `bare` walks right transforming data (1→2, 2→1) until a blank
/// read returns; a symbol-3 read traps.
const NARROWER_IDENTITY: &str = "\
.routine main, tapes=1, alpha=(4)
.routine bare, tapes=1, alpha=(3)
.section tables
T:  .row [0]
    .row [1]
    .row [2]
D:  .targets fin, wa, wb
.section code
.func main
        call    bare [0]
        stp
.func bare
scan:   rd
        mtc     T
        djmp    D
wa:     wr      [2]
        mov     [>]
        jmp     scan
wb:     wr      [1]
        mov     [>]
        jmp     scan
fin:    ret
";

#[test]
fn narrower_identity_binding_is_equivalent() {
    assert_equivalent(
        NARROWER_IDENTITY,
        &[
            &[(&[1], 0)],       // data 'a' → transformed to physical 2, stops
            &[(&[2], 0)],       // data 'b' → transformed to physical 1, stops
            &[(&[], 0)],        // blank tape → immediate return
            &[(&[3], 0)],       // the read hole → UnmappedRead in every mode
            &[(&[1, 2, 3], 0)], // walk two data cells, then hit the hole → trap
        ],
    );
}

#[test]
fn narrower_identity_binding_reading_the_hole_traps_unmapped_read() {
    // The oracle the cross-mode compare cannot provide (the miss would be
    // mode-consistent): the out-of-range read must trap UnmappedRead — NOT
    // collapse to a plain call and read the symbol raw. Pin the ACTUAL kind in
    // every mode. A happy-path seed still stops, proving it is the hole symbol
    // specifically that traps, not the binding as a whole.
    for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
        let (exe, _) = build(NARROWER_IDENTITY, mech);
        assert_eq!(
            run(&exe, &[(&[3], 0)]).outcome,
            "trapped:unmapped-read",
            "{mech}: caller symbol 3 has no callee image — the hole must trap"
        );
        assert_eq!(
            run(&exe, &[(&[1], 0)]).outcome,
            "stopped",
            "{mech}: an in-range data symbol still runs to a stop"
        );
    }
}

// ── determinism: re-link is byte-identical (image + sidecar) ─────────────────

#[test]
fn every_program_relinks_byte_identically_in_every_mode() {
    // Reproducible builds: the closure BFS is deterministic, so linking the
    // same program under the same mechanism twice yields byte-identical bytes
    // AND an identical sidecar JSON (docs/formats.md (frames profile)).
    for src in [
        CROSS_ALPHABET,
        NESTED_TWO_LEVEL,
        EQUAL_SIZE_BIJECTION,
        TRAP_TAXONOMY,
        NARROWER_IDENTITY,
    ] {
        for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
            let a = build_full(src, mech);
            let b = build_full(src, mech);
            assert_eq!(
                a.executable.to_bytes(),
                b.executable.to_bytes(),
                "the {mech} image is not reproducible"
            );
            assert_eq!(
                a.map.to_json(),
                b.map.to_json(),
                "the {mech} sidecar is not reproducible"
            );
        }
    }
}

// ── depth-independence (§5.2): O(1) frame overhead per composed call ─────────

/// A pure forwarder ladder: `main → … → S`, each level a `call.m` under a
/// 4-cycle permutation of the 5-symbol alphabet (`(1 2 3 4)`). The 4-cycle has
/// order 4, so the composites accumulated over a ≤3-deep chain (p, p², p³) are
/// all NON-identity — every level stays a real `call.m` (an identity composite
/// would relax to a plain `call`, breaking the measurement). `S` does exactly
/// one write in every ladder, so its device stall cancels in the differences.
const PERM: &str = "0{1->2, 2->3, 3->4, 4->1}";

fn ladder(levels: usize) -> String {
    // levels == 1: main → S. levels == 3: main → R → Q → S.
    let names = ["R", "Q"];
    let mut routines = String::from(".routine main, tapes=1, alpha=(5)\n");
    for name in names.iter().take(levels - 1) {
        routines.push_str(&format!(".routine {name}, tapes=1, alpha=(5)\n"));
    }
    routines.push_str(".routine S, tapes=1, alpha=(5)\n.section code\n");

    // main calls the first forwarder (or S directly at level 1).
    let first = if levels == 1 { "S" } else { names[0] };
    routines.push_str(&format!(
        ".func main\n        call {first} [{PERM}]\n        stp\n"
    ));
    // Each forwarder calls the next; the last forwarder calls S.
    for i in 0..levels.saturating_sub(1) {
        let here = names[i];
        let next = if i + 2 < levels { names[i + 1] } else { "S" };
        routines.push_str(&format!(
            ".func {here}\n        call {next} [{PERM}]\n        ret\n"
        ));
    }
    routines.push_str(".func S\n        wr [1]\n        ret\n");
    routines
}

#[test]
fn composed_call_overhead_is_constant_per_depth() {
    // Frames mode only — the compose table + directory + descriptor loads are
    // the frame machinery whose per-call cost the O(1) claim is about. Measure
    // total stall tacts at nesting depths 1, 2, 3.
    let stall = |levels: usize| -> u64 {
        let (exe, _) = build(&ladder(levels), CallMech::Frames);
        run(&exe, &[(&[0], 0)]).stats.stall_tacts
    };
    let s1 = stall(1);
    let s2 = stall(2);
    let s3 = stall(3);

    // Derive the per-call constant from the first two runs, then assert the
    // depth-3 total is exactly depth-1 + 2×constant (the plan's linearity
    // formula). Because S's single device write is present at every depth, the
    // differences isolate the frame machinery: one extra `call.m` (compose +
    // directory + descriptor load) plus one extra return-restore reload per
    // added level. If the per-call cost grew with depth, s3 would exceed the
    // linear prediction.
    let per_call = s2 - s1;
    assert!(per_call > 0, "a composed call has real frame overhead");
    assert_eq!(
        s3,
        s1 + 2 * per_call,
        "frame overhead is O(1) per call, depth-invariant: \
         s1={s1}, s2={s2}, s3={s3}, per-call={per_call}"
    );
}

// ── coverage: in-stamp closure descent (mono, run-tested) ────────────────────

/// A mono stamp whose body contains BOTH a live plain call and a live bound
/// call. The plain callee (`P`) must stamp under the SAME composite the body
/// runs under (it inherits the frame); the bound callee (`B`) stamps under the
/// COMPOSED composite. If the stamper failed to descend into `P` under the
/// inherited composite, `P` would write the wrong physical symbol — caught by
/// both the pinned oracle and the cross-mode compare.
const IN_STAMP_DESCENT: &str = "\
.routine main, tapes=1, alpha=(4)
.routine M,    tapes=1, alpha=(4)
.routine P,    tapes=1, alpha=(4)
.routine B,    tapes=1, alpha=(4)
.section code
.func main
        call    M [0{1->2, 2->1}]
        stp
.func M
        call    P
        mov     [>]
        call    B [0{1->3, 3->1}]
        ret
.func P
        wr      [1]
        ret
.func B
        wr      [1]
        ret
";

#[test]
fn in_stamp_closure_descent_is_equivalent() {
    assert_equivalent(IN_STAMP_DESCENT, &[&[(&[0], 0)]]);
}

#[test]
fn in_stamp_plain_callee_inherits_the_body_composite() {
    // Oracle for the plain-call descent: `P` runs under M's composite (swap12),
    // so its `wr [1]` writes virtual 1 → physical 2 at cell 0. This value comes
    // from swap12 alone (no further composition), so it is certain regardless
    // of composition direction. The bound `B` write lands at cell 1; the two
    // callees write different cells, both non-blank.
    let (mono, _) = build(IN_STAMP_DESCENT, CallMech::Mono);
    let obs = run(&mono, &[(&[0], 0)]);
    assert_eq!(obs.outcome, "stopped");
    assert_eq!(
        cell_at(&obs.snaps[0], 0),
        2,
        "the plain callee stamped under the inherited swap12 composite"
    );
    assert_ne!(cell_at(&obs.snaps[0], 1), 0, "the bound callee wrote too");
}

// ── the same program through the CLI in all three modes ──────────────────────

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// End to end through the real `tmt` CLI (in-process): assemble program (a)
/// once, then `link --call-mech mono|frames|hybrid` and `run` each image over
/// the same seeded tape. The exit codes must be identical — the CLI path
/// carries the same three-mode equivalence the library harness proves, and
/// `--call-mech` is wired end to end (docs/cli.md).
#[test]
fn program_a_runs_identically_via_the_cli_in_all_three_modes() {
    use mtc_turing_machine::cli::execute;

    let dir = scratch("mode_equivalence_cli");
    let src = dir.join("a.tma");
    std::fs::write(&src, CROSS_ALPHABET).unwrap();
    let obj = dir.join("a.tmo");
    execute(&args(&[
        "asm",
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]))
    .unwrap();

    let mut codes = Vec::new();
    for mech in ["mono", "frames", "hybrid"] {
        let exe = dir.join(format!("a-{mech}.tmx"));
        let link = execute(&args(&[
            "link",
            obj.to_str().unwrap(),
            "--call-mech",
            mech,
            "-o",
            exe.to_str().unwrap(),
        ]))
        .unwrap_or_else(|e| panic!("link --call-mech {mech}: {e}"));
        assert_eq!(link.code, 0, "link {mech} exits 0: {}", link.stdout);

        // Mint a tape from this image and seed one data cell 'a' (physical 1)
        // on tape 0 — the happy path that stops.
        let tape = dir.join(format!("a-{mech}.tmt"));
        execute(&args(&[
            "tape",
            "new",
            "--from",
            exe.to_str().unwrap(),
            "-o",
            tape.to_str().unwrap(),
        ]))
        .unwrap();
        execute(&args(&[
            "tape",
            "set",
            tape.to_str().unwrap(),
            "--in-place",
            "--tape",
            "0",
            "--cells",
            "1",
        ]))
        .unwrap();

        let out = execute(&args(&[
            "run",
            exe.to_str().unwrap(),
            "--tape",
            tape.to_str().unwrap(),
        ]))
        .unwrap();
        codes.push(out.code);
    }
    assert_eq!(codes, vec![0, 0, 0], "all three modes stop and exit 0");
}
