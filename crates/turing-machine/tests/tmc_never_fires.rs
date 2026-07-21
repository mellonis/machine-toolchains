//! The never-fires family (docs/tmt/language.md (rules)): a rule whose
//! expansion produces zero rows (`empty-expansion`) and a rule shadowed by an
//! earlier all-wildcard catch-all (`unreachable-rule`) both WARN and compile —
//! never an internal compiler error. A state whose rules all vanished is a
//! valid zero-row state: entering it traps exactly like a runtime no-match.
//!
//! Reproductions of the two "internal-error on plausible source" bugs: a range
//! whose alternatives all fall outside the alphabet, and two catch-alls in one
//! state. Result cases are derivation-first.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, Trap, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

/// Compile `src`, returning the diagnostic codes it produced (compilation must
/// succeed — the whole point is that these defects warn, never error).
fn diag_codes(src: &str) -> Vec<String> {
    let out = compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("must compile (no internal error): {e}"));
    out.report
        .diagnostics
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

/// Compile, link, and run `src` on the given seed bands with a step cap,
/// returning the outcome and final per-tape snapshots. Warnings are allowed.
/// Defaults to `-O0`; [`run_capped_at`] pins the level explicitly.
fn run_capped(
    src: &str,
    seeds: &[(TapeSnapshot, u32)],
    max_steps: u64,
) -> (Outcome, Vec<TapeSnapshot>) {
    run_capped_at(src, seeds, max_steps, OptLevel::O0)
}

/// [`run_capped`] at a chosen optimization level.
fn run_capped_at(
    src: &str,
    seeds: &[(TapeSnapshot, u32)],
    max_steps: u64,
    opt_level: OptLevel,
) -> (Outcome, Vec<TapeSnapshot>) {
    let out = compile(
        src,
        CompileOptions {
            opt_level,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("must compile: {e}"));
    let exe: Executable = link(
        std::slice::from_ref(&out.object),
        &[],
        LinkOptions::default(),
    )
    .unwrap_or_else(|e| panic!("link failed: {e}"))
    .executable;
    let mut tapes: Vec<WideTape> = seeds
        .iter()
        .map(|(s, w)| WideTape::from_snapshot(s, *w).expect("seed fits its width"))
        .collect();
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");
    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(max_steps),
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

#[test]
fn empty_expansion_warns_and_compiles() {
    // `small` has no numeric labels, so every alternative of `[0..5]` is absent
    // and the rule expands to ZERO rows — a warning, not an error. The state
    // keeps a working `['a']` rule, so the machine still does something: seed
    // 'a' (index 1) fires the surviving rule (write 'b', stop); the [0..5] rule
    // never fires. Derived final tape: "b" (cell [2]).
    let src = "\
alphabet small { '_', 'a', 'b' }
machine {
  tape t: small;
  entry state s {
    [0..5] -> write ['b'] move [>] goto s;
    ['a']  -> write ['b'] stop;
  }
}
";
    let codes = diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "empty-expansion"),
        "expected empty-expansion, got {codes:?}"
    );
    let (outcome, snaps) = run_capped(src, &[(snap(0, &[1], 0), 3)], 1_000);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(snaps, vec![snap(0, &[2], 0)]);
}

#[test]
fn unreachable_after_catch_all_warns_and_compiles() {
    // Two catch-alls in one state: the second `[*]` can never fire (the first
    // matches everything). It WARNS (`unreachable-rule`) and is dropped before
    // codegen, so the compile succeeds — no assembler "all-wildcard must be
    // last" internal error. The surviving program is byte-for-byte the
    // single-rule loop, so their runs must be identical.
    let two = "\
alphabet ab { '_', 'a', 'b' }
machine {
  tape t: ab;
  entry state s {
    [*] -> move [>] goto s;
    [*] -> stop;
  }
}
";
    let one = "\
alphabet ab { '_', 'a', 'b' }
machine {
  tape t: ab;
  entry state s {
    [*] -> move [>] goto s;
  }
}
";
    let codes = diag_codes(two);
    assert!(
        codes.iter().any(|c| c == "unreachable-rule"),
        "expected unreachable-rule, got {codes:?}"
    );
    // Derive both, compare: the drop leaves exactly the single-rule loop, so
    // both step-cap identically (move right forever, no writes → tape unchanged,
    // head advanced by the cap, StepLimit trap).
    let seed = &[(snap(0, &[1, 1, 0], 0), 3)];
    let got_two = run_capped(two, seed, 8);
    let got_one = run_capped(one, seed, 8);
    assert_eq!(got_two, got_one);
    assert_eq!(got_two.0, Outcome::Trapped(Trap::StepLimit));
}

#[test]
fn zero_row_state_traps_like_no_match() {
    // The entry state's only rule `[0..5]` empty-expands, leaving a ZERO-ROW
    // state. Entering it must trap exactly like a runtime no-match — the same
    // NoTransition kind a genuine unmatched read produces.
    let zero_row = "\
alphabet small { '_', 'a', 'b' }
machine {
  tape t: small;
  entry state s { [0..5] -> move [>] goto s; }
}
";
    let codes = diag_codes(zero_row);
    assert!(
        codes.iter().any(|c| c == "empty-expansion"),
        "expected empty-expansion, got {codes:?}"
    );
    let (outcome, _) = run_capped(zero_row, &[(snap(0, &[1], 0), 3)], 1_000);
    assert!(
        matches!(outcome, Outcome::Trapped(Trap::NoTransition { .. })),
        "zero-row state should trap NoTransition, got {outcome:?}"
    );

    // A genuine no-match: `s` matches only 'a'(1); seed '_'(0) → nothing
    // matches → NoTransition. Same trap kind as the zero-row state.
    let no_match = "\
alphabet small { '_', 'a', 'b' }
machine {
  tape t: small;
  entry state s { ['a'] -> stop; }
}
";
    let (nm_outcome, _) = run_capped(no_match, &[(snap(0, &[0], 0), 3)], 1_000);
    assert!(
        matches!(nm_outcome, Outcome::Trapped(Trap::NoTransition { .. })),
        "genuine no-match should trap NoTransition, got {nm_outcome:?}"
    );
    assert_eq!(
        std::mem::discriminant(&outcome),
        std::mem::discriminant(&nm_outcome),
        "zero-row and no-match outcomes must be the same kind"
    );
}

#[test]
fn graft_instantiated_empty_expansion_warns_not_errors() {
    // A graph written generically with a `[0..9]` rule, grafted onto a tape
    // whose alphabet (`marks`) has no numeric labels: the `[0..9]` rule
    // empty-expands (a warning, not an error) while the instance's other rule
    // still works. Seed "x_": the surviving `['x'] -> found` reaches `win`,
    // which stops. Derived: tape unchanged, head 0.
    let src = "\
alphabet marks { '_', 'x', 'y' }

graph findX(tape t: marks, state found, state missing) {
  entry state scan {
    [0..9] -> move [>] goto scan;
    ['x']  -> found;
    ['_']  -> missing;
    [*]    -> move [>] goto scan;
  }
}

machine {
  tape work: marks;
  entry graft findX(t = work, found = win, missing = lose) as g;
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";
    let codes = diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "empty-expansion"),
        "expected empty-expansion, got {codes:?}"
    );
    // 'x' (index 1) at head 0 → `['x'] -> found = win` → win stops. No writes
    // or moves, so the tape is unchanged (the lone 'x' cell, head 0).
    let (outcome, snaps) = run_capped(src, &[(snap(0, &[1], 0), 3)], 1_000);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(snaps, vec![snap(0, &[1], 0)]);
}

// -- Sweep (requirement 4): neighbouring zero-of-something shapes -------------

#[test]
fn graft_whose_only_rule_vanishes_becomes_a_zero_row_instance() {
    // A grafted graph whose single rule empty-expands: the spliced instance is
    // a ZERO-ROW state. As the entry graft it is entered immediately, and must
    // trap NoTransition exactly like a hand-written zero-row state — the splice
    // path reaches the same sound codegen.
    let src = "\
alphabet marks { '_', 'x', 'y' }

graph dead(tape t: marks, state done) {
  entry state s { [0..9] -> move [>] goto s; }
}

machine {
  tape work: marks;
  entry graft dead(t = work, done = win) as g;
  state win { [*] -> stop; }
}
";
    let codes = diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "empty-expansion"),
        "expected empty-expansion, got {codes:?}"
    );
    let (outcome, _) = run_capped(src, &[(snap(0, &[1], 0), 3)], 1_000);
    assert!(
        matches!(outcome, Outcome::Trapped(Trap::NoTransition { .. })),
        "grafted zero-row instance should trap NoTransition, got {outcome:?}"
    );
}

#[test]
fn graft_rule_that_vanishes_only_at_the_splice_warns_and_the_rest_works() {
    // The graph body expands cleanly in graph-space (`digits` has '0'/'1'), but
    // the binding maps no host symbol to graph '1', so the `['1']` rule has an
    // empty host preimage and vanishes at the SPLICE (not range expansion). It
    // warns `empty-expansion` while the instance's other rules still work.
    // Seed 'a': graph '0' → move right to the blank → graph '_' → done = win.
    let src = "\
alphabet digits  { '_', '0', '1' }
alphabet letters { '_', 'a' }

graph foo(tape t: digits, state done) {
  entry state s {
    ['1'] -> done;
    ['0'] -> move [>] goto s;
    ['_'] -> done;
  }
}

machine {
  tape work: letters;
  entry graft foo(t = work with map { 'a' -> '0' }, done = win) as g;
  state win { [*] -> stop; }
}
";
    let codes = diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "empty-expansion"),
        "expected empty-expansion from the splice, got {codes:?}"
    );
    // 'a'(1) → move right → blank(0) → win → stop. No writes; head ends at 1.
    // `letters` has cardinality 2, so the tape band width is 2.
    let (outcome, snaps) = run_capped(src, &[(snap(0, &[1], 0), 2)], 1_000);
    assert_eq!(outcome, Outcome::Stopped);
    // The head rests on the blank it stepped onto, so the snapshot spans it.
    assert_eq!(snaps, vec![snap(0, &[1, 0], 1)]);
}

#[test]
fn duplicate_exact_rows_are_a_clean_diagnostic_not_an_internal_error() {
    // Two identical wildcard-free rows match the same tuple — the assembler
    // disciplines that pairing. The compiler must reject it at the source with
    // a proper diagnostic, never surface it as an assembler internal error.
    let src = "\
alphabet ab { '_', 'a', 'b' }
machine {
  tape t: ab;
  entry state s {
    ['a'] -> stop;
    ['a'] -> halt;
  }
}
";
    let err =
        compile(src, CompileOptions::default()).expect_err("duplicate exact rows must be rejected");
    let rendered = err.to_string();
    assert!(
        !rendered.contains("internal"),
        "must be a source diagnostic, not an internal error: {rendered}"
    );
}

#[test]
fn dropped_unreachable_rule_keeps_the_call_list_aligned() {
    // The dropped unreachable rule is a SECOND catch-all carrying a `call`; the
    // surviving reachable rule carries a DIFFERENT call. A world's source-order
    // call list is filtered in tandem with the dropped rule — if it misaligned,
    // the reachable rule would resolve to the wrong routine. Seed 'a' fires the
    // reachable exact `['a']` rule (sorts ahead of the catch-all) → `call mark`
    // (writes 'b'); a misalignment would instead run `erase` (writes '_').
    let src = "\
alphabet ab { '_', 'a', 'b' }
namespace lib {
  export routine mark(tape t: ab) {
    entry state m { [*] -> write ['b'] return; }
  }
  export routine erase(tape t: ab) {
    entry state e { [*] -> write ['_'] return; }
  }
}
use lib::mark;
use lib::erase;
machine {
  tape t: ab;
  entry state s {
    ['a'] -> call mark(t = t) then done;
    [*]   -> goto done;
    [*]   -> call erase(t = t) then done;
  }
  state done { [*] -> stop; }
}
";
    let codes = diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "unreachable-rule"),
        "expected unreachable-rule, got {codes:?}"
    );
    // 'a' (index 1) → exact `['a']` fires → `call mark` writes 'b' (index 2) at
    // head 0 → done → stop.
    let (outcome, snaps) = run_capped(src, &[(snap(0, &[1], 0), 3)], 1_000);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(snaps, vec![snap(0, &[2], 0)]);
}

#[test]
fn zero_row_state_is_sound_at_both_opt_levels() {
    // A zero-row state is a NEW IR shape (rules: []) the optimizer had never
    // seen before this task; the optimizer runs on it at -O1 before codegen.
    // The -O0/-O1 equivalence floor must hold: entering the zero-row state
    // traps NoTransition at BOTH levels (no pass panics on the empty rule
    // vector, none transforms it so codegen's zero-row branch is skipped).
    let src = "\
alphabet small { '_', 'a', 'b' }
machine {
  tape t: small;
  entry state s { [0..5] -> move [>] goto s; }
}
";
    for level in [OptLevel::O0, OptLevel::O1] {
        let (outcome, _) = run_capped_at(src, &[(snap(0, &[1], 0), 3)], 1_000, level);
        assert!(
            matches!(outcome, Outcome::Trapped(Trap::NoTransition { .. })),
            "zero-row state must trap NoTransition at {level:?}, got {outcome:?}"
        );
    }
}
