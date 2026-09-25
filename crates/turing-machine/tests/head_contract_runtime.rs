//! The RUNTIME half of a head contract (docs/tmt/language.md (head-position
//! clauses)): a debug build plants a check that traps when the head is not
//! where an `enters`/`leaves` clause promised, and `--strip-asserts` removes
//! the check without touching anything else the object says.
//!
//! The static half — the compile errors a body's own rules already disprove
//! a clause with — lives in its own file; nothing here re-checks it.
//!
//! Every expectation about a run is DERIVED from the source below (the
//! routine writes nothing and moves its head right exactly once), never
//! read back from a run's output.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, Trap, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::ir::{IrProgram, IrTransition};
use mtc_turing_machine::optimizer::OptLevel;

// ── the fixture ──────────────────────────────────────────────────────────

/// One tape parameter carrying BOTH clauses, each a PROPER SUBSET of the
/// alphabet (a clause naming every glyph would make its check trivially
/// satisfiable, and a reader could not tell a live check from a missing
/// one). The body writes nothing and moves the head right once, so the
/// leaving glyph is whatever the seed put one cell to the right — not
/// statically known, which is exactly the shape the static `leaves` check
/// declines and leaves to run time.
const CONTRACTED: &str = "\
alphabet sym { '_', 'a', 'b' }

routine walk(tape num: sym enters { '_' } leaves { 'b' }) {
  entry state go {
    [*] -> move [>] return;
  }
}

machine {
  tape t: sym;
  entry state s { [*] -> call walk(num = t) then done; }
  state done { [*] -> stop; }
}
";

/// [`CONTRACTED`] with the two clauses — and nothing else — deleted.
const UNCONTRACTED: &str = "\
alphabet sym { '_', 'a', 'b' }

routine walk(tape num: sym) {
  entry state go {
    [*] -> move [>] return;
  }
}

machine {
  tape t: sym;
  entry state s { [*] -> call walk(num = t) then done; }
  state done { [*] -> stop; }
}
";

/// The 1-based line the tape parameter is declared on — the line a debug
/// build maps a contract trap to.
const SIGNATURE_LINE: u32 = 3;

/// Head on `'a'`, which `enters { '_' }` does not name: the check at the
/// routine's entry is the one that fires.
const ENTERS_VIOLATION: &[u8] = &[1];
/// Head on `'_'` (accepted on entry) with `'a'` one cell right: the move
/// lands the head outside `leaves { 'b' }`, so the check before the
/// return is the one that fires.
const LEAVES_VIOLATION: &[u8] = &[0, 1];
/// The near miss — the same shape seeded so BOTH clauses hold: entry on
/// `'_'`, the move landing on `'b'`.
const SATISFIED: &[u8] = &[0, 2];

// ── harness ──────────────────────────────────────────────────────────────

fn object(src: &str, level: OptLevel, strip_asserts: bool) -> ObjectFile {
    compile(
        src,
        CompileOptions {
            opt_level: level,
            strip_asserts,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the program compiles: {e}"))
    .object
}

fn ir(src: &str, level: OptLevel, strip_asserts: bool) -> IrProgram {
    compile(
        src,
        CompileOptions {
            opt_level: level,
            strip_asserts,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the program compiles: {e}"))
    .ir
}

fn build(src: &str, level: OptLevel, mech: CallMech, strip_asserts: bool) -> Executable {
    link(
        std::slice::from_ref(&object(src, level, strip_asserts)),
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the {mech} link failed: {e}"))
    .executable
}

/// Run an image on one seed per physical tape — each laid at origin 0 with
/// its head at 0 — and return the outcome with every band's final snapshot.
fn run(exe: &Executable, seeds: &[&[u8]]) -> (Outcome, Vec<TapeSnapshot>) {
    assert_eq!(
        seeds.len(),
        exe.tape_count as usize,
        "a case seeds exactly one tape per machine tape"
    );
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tapes: Vec<WideTape> = seeds
        .iter()
        .zip(&exe.alphabet_cardinalities)
        .map(|(cells, &width)| {
            WideTape::from_snapshot(
                &TapeSnapshot {
                    origin: 0,
                    cells: cells.to_vec(),
                    head: 0,
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
                    max_steps: Some(10_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    drop(devices);
    (
        result.outcome,
        tapes.iter().map(WideTape::to_snapshot).collect(),
    )
}

/// `run` on a one-tape image, unwrapping the single band.
fn run_one(exe: &Executable, cells: &[u8]) -> (Outcome, TapeSnapshot) {
    let (outcome, mut snaps) = run(exe, &[cells]);
    (outcome, snaps.remove(0))
}

/// A snapshot's cell at an ABSOLUTE tape coordinate — the total view of the
/// infinite tape, so an assertion never depends on how a snapshot happens
/// to trim its blank margins.
fn cell_at(snap: &TapeSnapshot, pos: i64) -> u8 {
    let index = pos - snap.origin;
    if index < 0 {
        return 0;
    }
    snap.cells.get(index as usize).copied().unwrap_or(0)
}

/// The derivation every case below shares: the routine writes nothing, so
/// each cell of the final tape still holds what the seed put there (and
/// blank everywhere the seed said nothing).
fn assert_nothing_was_written(snap: &TapeSnapshot, seed: &[u8]) {
    for pos in -2..=4i64 {
        let expected = if pos >= 0 {
            seed.get(pos as usize).copied().unwrap_or(0)
        } else {
            0
        };
        assert_eq!(cell_at(snap, pos), expected, "cell at {pos}");
    }
}

/// Every configuration a behavioural claim here is made across: both opt
/// levels × all three call mechanisms.
fn matrix() -> Vec<(OptLevel, CallMech)> {
    let mut out = Vec::new();
    for level in [OptLevel::O0, OptLevel::O1] {
        for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
            out.push((level, mech));
        }
    }
    out
}

// ── the checks fire ──────────────────────────────────────────────────────

/// Mutation this catches: not synthesizing the `enters` chain at all (or
/// synthesizing it without making it the world's entry) — the run then
/// stops normally with the head one cell right, exactly as it does at the
/// commit before this one.
#[test]
fn an_entry_outside_the_enters_clause_traps() {
    for (level, mech) in matrix() {
        let exe = build(CONTRACTED, level, mech, false);
        let (outcome, snap) = run_one(&exe, ENTERS_VIOLATION);
        assert!(
            matches!(outcome, Outcome::Trapped(Trap::Contract { .. })),
            "{level:?}/{mech}: expected a contract trap, got {outcome:?}"
        );
        // The check runs BEFORE the body, so the head has not moved.
        assert_eq!(snap.head, 0, "{level:?}/{mech}");
        assert_nothing_was_written(&snap, ENTERS_VIOLATION);
    }
}

/// Mutation this catches: leaving `IrTransition::Return` alone instead of
/// routing it through the `leaves` chain — the run then stops normally.
/// Distinct from the `enters` guard above: the two halves are synthesized
/// by separate code, and this seed enters on a glyph the `enters` clause
/// DOES name, so an `enters`-only implementation stays green on it.
#[test]
fn a_return_outside_the_leaves_clause_traps() {
    for (level, mech) in matrix() {
        let exe = build(CONTRACTED, level, mech, false);
        let (outcome, snap) = run_one(&exe, LEAVES_VIOLATION);
        assert!(
            matches!(outcome, Outcome::Trapped(Trap::Contract { .. })),
            "{level:?}/{mech}: expected a contract trap, got {outcome:?}"
        );
        // The check runs AFTER the move, so it reads the real cell.
        assert_eq!(snap.head, 1, "{level:?}/{mech}");
        assert_nothing_was_written(&snap, LEAVES_VIOLATION);
    }
}

/// The near miss: the same program, seeded so both clauses hold. Mutation
/// this catches: a check state whose rows trap on the DECLARED set (the
/// polarity inverted), or a catch-all that traps unconditionally — either
/// leaves the two guards above green and only this one red.
#[test]
fn a_kept_contract_runs_to_its_ordinary_stop() {
    for (level, mech) in matrix() {
        let exe = build(CONTRACTED, level, mech, false);
        let (outcome, snap) = run_one(&exe, SATISFIED);
        assert_eq!(outcome, Outcome::Stopped, "{level:?}/{mech}");
        assert_eq!(snap.head, 1, "{level:?}/{mech}");
        assert_nothing_was_written(&snap, SATISFIED);
    }
}

/// Mutation this catches: synthesizing the checks regardless of
/// `strip_asserts` — both seeds then trap instead of running through.
#[test]
fn stripping_the_asserts_restores_the_unchecked_run() {
    for (level, mech) in matrix() {
        let exe = build(CONTRACTED, level, mech, true);
        for seed in [ENTERS_VIOLATION, LEAVES_VIOLATION, SATISFIED] {
            let (outcome, snap) = run_one(&exe, seed);
            assert_eq!(outcome, Outcome::Stopped, "{level:?}/{mech} on {seed:?}");
            assert_eq!(snap.head, 1, "{level:?}/{mech} on {seed:?}");
            assert_nothing_was_written(&snap, seed);
        }
    }
}

/// A routine that returns through a call's resume point rather than a
/// `return` row of its own. The static half declines this shape (the glyph
/// a callee leaves behind is not statically known), which is exactly why
/// the run-time check has to cover it: `then return` IS the moment control
/// goes back to the caller.
///
/// It is also the one shape where a planted state is the SPLICE TARGET's
/// own entry: at `-O1` `inline` splices `inner` into `walk`, whose entry
/// by then is the check state rather than the state the source wrote.
/// Planting runs before the optimizer, so this ordering has to hold.
const THEN_RETURN: &str = "\
alphabet sym { '_', 'a', 'b' }

routine inner(tape num: sym) {
  entry state s {
    [*] -> move [>] return;
  }
}

routine walk(tape num: sym enters { '_' } leaves { 'b' }) {
  entry state go {
    [*] -> call inner(num = num) then return;
  }
}

machine {
  tape t: sym;
  entry state s { [*] -> call walk(num = t) then done; }
  state done { [*] -> stop; }
}
";

/// Mutation this catches: rewriting only a rule's own `return` terminal and
/// leaving a call's `then return` alone — the seed below then runs through
/// with the head on a glyph the clause never named.
#[test]
fn a_call_resuming_at_a_return_is_checked_too() {
    for (level, mech) in matrix() {
        let exe = build(THEN_RETURN, level, mech, false);

        let (outcome, snap) = run_one(&exe, LEAVES_VIOLATION);
        assert!(
            matches!(outcome, Outcome::Trapped(Trap::Contract { .. })),
            "{level:?}/{mech}: expected a contract trap, got {outcome:?}"
        );
        assert_eq!(snap.head, 1, "{level:?}/{mech}");

        let (outcome, snap) = run_one(&exe, SATISFIED);
        assert_eq!(outcome, Outcome::Stopped, "{level:?}/{mech}");
        assert_eq!(snap.head, 1, "{level:?}/{mech}");
    }
}

/// TWO tape parameters, with clauses on BOTH — so the `enters` chain has
/// two links — and the second tape carrying a clause of its own, so a check
/// state's row has to name the column the clause was declared on rather
/// than the first one. `p` never moves; `q` moves right once, which leaves
/// its exit glyph statically unknown and so leaves it to run time.
const TWO_TAPES: &str = "\
alphabet sym { '_', 'a', 'b' }

routine pair(tape p: sym enters { '_' }, tape q: sym enters { 'a' } leaves { 'b' }) {
  entry state go {
    [*, *] -> move [., >] return;
  }
}

machine {
  tape x: sym;
  tape y: sym;
  entry state s { [*, *] -> call pair(p = x, q = y) then done; }
  state done { [*, *] -> stop; }
}
";

/// Each case is `(p's seed, q's seed, the outcome, q's final head)`. `p`'s
/// head never moves, and neither tape is ever written, so those two facts
/// are asserted for every case rather than listed per case.
///
/// The derivation, from the source alone: the chain reads `p`'s `enters`,
/// then `q`'s, then the body moves `q` right once, then `q`'s `leaves` is
/// read on the cell the move landed on.
///
/// Two mutations this catches, one per unguarded path:
/// * a check row naming the FIRST column instead of the declaring tape's —
///   case 4 then traps on `p`'s blank against `q`'s `enters { 'a' }`
///   instead of running clean, and case 3 traps before the move rather
///   than after it, so its `q` head is wrong;
/// * a chain that plants only one of the clauses — case 1 or case 2 then
///   runs through, depending on which link is dropped.
const TWO_TAPE_CASES: &[(&[u8], &[u8], bool, i64)] = &[
    // `p` enters on 'a', outside `enters { '_' }`: the FIRST link fires,
    // before `q` is ever read.
    (&[1], &[1, 2], true, 0),
    // `p` is fine; `q` enters on 'b', outside `enters { 'a' }`: the SECOND
    // link fires, still before the move.
    (&[0], &[2, 2], true, 0),
    // Both entries fine; the move lands `q` on 'a', outside
    // `leaves { 'b' }`.
    (&[0], &[1, 1], true, 1),
    // The near miss: every clause held.
    (&[0], &[1, 2], false, 1),
];

#[test]
fn a_clause_on_a_second_tape_reads_that_tape() {
    for (level, mech) in matrix() {
        let exe = build(TWO_TAPES, level, mech, false);
        for (i, (p, q, traps, q_head)) in TWO_TAPE_CASES.iter().enumerate() {
            let (outcome, snaps) = run(&exe, &[p, q]);
            if *traps {
                assert!(
                    matches!(outcome, Outcome::Trapped(Trap::Contract { .. })),
                    "{level:?}/{mech} case {i}: expected a contract trap, got {outcome:?}"
                );
            } else {
                assert_eq!(outcome, Outcome::Stopped, "{level:?}/{mech} case {i}");
            }
            assert_eq!(snaps[0].head, 0, "{level:?}/{mech} case {i}: p's head");
            assert_eq!(
                snaps[1].head, *q_head,
                "{level:?}/{mech} case {i}: q's head"
            );
            assert_nothing_was_written(&snaps[0], p);
            assert_nothing_was_written(&snaps[1], q);
        }
    }
}

/// A routine leaving through one of its own `state` parameters rather than
/// returning. The language rule is that a routine's exit parameters are
/// NOT `leaves` rows — only `return` is — and the static half reads it
/// that way, so the run-time half must too: a check planted here would
/// trap on a moment the language promises nothing about.
const EXIT_PARAMETER: &str = "\
alphabet sym { '_', 'a', 'b' }

routine walk(tape num: sym leaves { 'b' }, state out) {
  entry state go {
    [*] -> move [>] goto out;
  }
}

machine {
  tape t: sym;
  entry state s { [*] -> call walk(num = t, out = done) then other; }
  state done  { [*] -> stop; }
  state other { [*] -> stop; }
}
";

/// Mutation this catches: treating a `goto <state parameter>` (and a call
/// resuming at one) as a way the clause governs — the seed below, whose
/// head ends on a glyph outside the clause, then traps instead of running
/// through.
#[test]
fn an_exit_parameter_is_not_a_leaves_row() {
    for (level, mech) in matrix() {
        let exe = build(EXIT_PARAMETER, level, mech, false);
        let (outcome, snap) = run_one(&exe, LEAVES_VIOLATION);
        assert_eq!(outcome, Outcome::Stopped, "{level:?}/{mech}");
        assert_eq!(snap.head, 1, "{level:?}/{mech}");
        assert_nothing_was_written(&snap, LEAVES_VIOLATION);
    }
}

// ── the stripping identity ───────────────────────────────────────────────

/// The user-visible half: a stripped build of the contracted source is the
/// plain build of the same source with the clause text deleted — in
/// everything the object says EXCEPT the interface section, which carries
/// the DECLARED contract and must survive stripping (a consumer links
/// against the declaration; removing a run-time check is not permission to
/// stop publishing the promise). Both sides compile WITHOUT `-g`, since the
/// two sources differ in text and a debug line table would differ for a
/// reason that has nothing to do with this property.
///
/// Mutation this catches: any residue of the clauses in the code, symbol,
/// relocation, table or bound-call sections of a stripped object. The two
/// interface assertions guard the exemption itself from both sides — the
/// positive one is the claim (stripping publishes exactly what an
/// unstripped build publishes), the negative one is what makes exempting
/// the section from the byte compare legitimate.
#[test]
fn a_stripped_object_is_the_clause_free_object() {
    for level in [OptLevel::O0, OptLevel::O1] {
        let mut stripped = object(CONTRACTED, level, true);
        let plain = object(UNCONTRACTED, level, false);
        assert_eq!(
            stripped.interface,
            object(CONTRACTED, level, false).interface,
            "{level:?}: stripping the check keeps the published contract"
        );
        assert_ne!(
            stripped.interface, plain.interface,
            "{level:?}: the declared contract is published either way"
        );
        stripped.interface = plain.interface.clone();
        assert_eq!(
            stripped.to_bytes(),
            plain.to_bytes(),
            "{level:?}: a stripped build differs from the clause-free build \
             outside the interface section"
        );
    }
}

/// THE discriminating guard. The object comparison above cannot see the
/// difference between "the compiler never synthesized the states" and "the
/// compiler synthesized them and codegen filtered them out" — a clean
/// filter leaves the bytes identical either way. Reading the IR by the
/// TRANSITION VARIANT the checks lower their catch-all to makes the
/// distinction a fact: under `--strip-asserts` there is no check state and
/// no contract terminal in the IR at all.
///
/// Both arms are asserted so the negative one cannot go vacuous: an
/// implementation that synthesizes nothing anywhere fails the debug arm.
///
/// Mutation this catches: synthesizing unconditionally and dropping the
/// states at codegen.
#[test]
fn stripping_leaves_no_check_state_in_the_ir() {
    // The lowered shape, read at `-O0` where the optimizer has not run:
    // exactly one state and one contract terminal per declared clause,
    // each named for the tape that declared it. At `-O1` the `inline` pass
    // legitimately splices the routine — checks included — into its
    // caller, so the counts there are the optimizer's business, not this
    // property's.
    let lowered = ir(CONTRACTED, OptLevel::O0, false);
    assert_eq!(
        contract_terminals(&lowered),
        2,
        "one contract terminal per declared clause"
    );
    assert_eq!(
        check_state_names(&lowered),
        vec!["enters_num".to_string(), "leaves_num".to_string()],
        "one check state per declared clause, named for its tape"
    );

    for level in [OptLevel::O0, OptLevel::O1] {
        assert!(
            contract_terminals(&ir(CONTRACTED, level, false)) > 0,
            "{level:?}: the stripped arm below is only a claim if a debug \
             build carries checks at all"
        );
        let stripped = ir(CONTRACTED, level, true);
        assert_eq!(
            contract_terminals(&stripped),
            0,
            "{level:?}: a stripped build carries no contract terminal"
        );
        assert!(
            check_state_names(&stripped).is_empty(),
            "{level:?}: a stripped build carries no check state"
        );
    }
}

/// Every rule in `program` whose transition is the contract terminal.
fn contract_terminals(program: &IrProgram) -> usize {
    program
        .worlds
        .iter()
        .flat_map(|w| &w.states)
        .flat_map(|s| &s.rules)
        .filter(|r| r.transition == IrTransition::TrapContract)
        .count()
}

/// Every state named for a head clause, sorted — the state names a reader
/// sees in `tmt dis` and in a debug stack.
fn check_state_names(program: &IrProgram) -> Vec<String> {
    let mut names: Vec<String> = program
        .worlds
        .iter()
        .flat_map(|w| &w.states)
        .map(|s| s.name.clone())
        .filter(|n| n.starts_with("enters_") || n.starts_with("leaves_"))
        .collect();
    names.sort();
    names
}

/// Synthesis is strictly per-clause opt-in: a routine that declares
/// neither clause compiles to the same bytes whether or not asserts are
/// stripped. This is what keeps every byte pin in the suite still.
///
/// Mutation this catches: synthesizing a check for every tape parameter,
/// or for a tape whose clause is absent.
#[test]
fn a_clause_free_routine_is_untouched_by_the_option() {
    for level in [OptLevel::O0, OptLevel::O1] {
        assert_eq!(
            object(UNCONTRACTED, level, false).to_bytes(),
            object(UNCONTRACTED, level, true).to_bytes(),
            "{level:?}"
        );
    }
}

// ── the two reporting fidelities ─────────────────────────────────────────

fn scratch(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("head-contract-{name}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// `tmt build` the contracted source in `dir` under `flags`, seed a
/// one-band `.tmt`, and return `tmt run`'s stdout.
fn run_cli(dir: &Path, flags: &[&str], seed: &[u8]) -> String {
    let src = dir.join("walk.tmc");
    fs::write(&src, CONTRACTED).unwrap();
    let exe = dir.join("walk.tmx");
    let mut build_args = vec![
        "build".to_string(),
        src.to_str().unwrap().to_string(),
        "-o".to_string(),
        exe.to_str().unwrap().to_string(),
    ];
    build_args.extend(flags.iter().map(|f| f.to_string()));
    execute(&build_args).expect("the build succeeds");

    let block = TapeBlockFile {
        alphabet: vec!["_".into(), "a".into(), "b".into()],
        tapes: vec![TapeSnapshot {
            origin: 0,
            cells: seed.to_vec(),
            head: 0,
            alphabet: None,
        }],
    };
    let tape = dir.join("seed.tmt");
    fs::write(&tape, block.to_bytes().unwrap()).unwrap();

    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
        tape.to_str().unwrap(),
    ]))
    .expect("the run itself is not a tool error");
    assert_eq!(out.code, 3, "a trapped run exits 3");
    out.stdout
}

/// The full fidelity — the routine, the tape, the clause, and the source
/// position the clause was declared at — which needs the check's own
/// dispatch block to survive as a LABEL in the sidecar. A `-g` build at
/// `-O0` is where that holds; the next test is the `-O1` build where it
/// does not, and the claim is deliberately not made here for both.
///
/// Mutation this catches: rendering the outcome through the default
/// `Debug` formatting (the reader then gets `Trapped(Contract { at: 12 })`
/// and nothing else).
#[test]
fn a_debug_build_names_the_routine_the_tape_and_the_clause() {
    let dir = scratch("debug");
    let stdout = run_cli(&dir, &["-g"], LEAVES_VIOLATION);
    let first = stdout.lines().next().expect("an outcome line");
    assert!(first.contains("contract"), "got: {first}");
    assert!(first.contains("walk"), "got: {first}");
    assert!(first.contains("num"), "got: {first}");
    assert!(first.contains("leaves"), "got: {first}");
    assert!(
        first.contains(&format!("walk.tmc:{SIGNATURE_LINE}")),
        "the trap maps to the signature line; got: {first}"
    );
}

/// The `-O1` degradation, with debug info present throughout. `inline`
/// splices the check into its caller and `dispatch-select` lowers it to
/// the two-row branch form, which emits the trap with no dispatch block
/// ahead of it — so the label that carried the tape and the clause is gone
/// even though the line table is not. What survives is the kind, the
/// address, the function the check now physically lives in, and the
/// signature line.
///
/// A recorded limit rather than a defect: the trap itself is correct and
/// nothing is guessed in the label's place. Keeping a label for the branch
/// form purely so this message could be richer is a codegen question, not
/// this check's.
///
/// Mutation this catches: resolving the tape and the clause through
/// anything other than a real label in the sidecar — the same unconditional
/// resolution the no-debug-info test catches, which panics or renders
/// nonsense here too.
#[test]
fn an_optimized_build_loses_the_tape_and_the_clause() {
    let dir = scratch("optimized");
    let stdout = run_cli(&dir, &["-g", "-O1"], LEAVES_VIOLATION);
    let first = stdout.lines().next().expect("an outcome line");
    assert!(first.contains("contract"), "got: {first}");
    assert!(first.contains("0x"), "the faulting address; got: {first}");
    assert!(
        first.contains("main"),
        "the function the check was spliced into; got: {first}"
    );
    assert!(
        first.contains(&format!("walk.tmc:{SIGNATURE_LINE}")),
        "the line table survives the splice; got: {first}"
    );
    assert!(
        !first.contains("leaves") && !first.contains("enters") && !first.contains("num"),
        "no label is left to name the tape or the clause; got: {first}"
    );
}

/// The degradation: without `-g` a linked function carries no labels and no
/// lines at all, so the tape and the clause — which live in the check
/// state's LABEL — are simply not there to report. What survives is the
/// trap kind, the faulting address, and the routine's own name (which
/// comes from the function range, populated either way).
///
/// Mutation this catches: resolving the clause through `labels`
/// unconditionally, which renders an empty or garbled tape/clause pair
/// here rather than falling back. Without this arm a reader would believe
/// the richer message is always available.
#[test]
fn a_build_without_debug_info_reports_the_reduced_message() {
    let dir = scratch("nodebug");
    let stdout = run_cli(&dir, &[], LEAVES_VIOLATION);
    let first = stdout.lines().next().expect("an outcome line");
    assert!(first.contains("contract"), "got: {first}");
    assert!(first.contains("0x"), "the faulting address; got: {first}");
    assert!(first.contains("walk"), "the routine; got: {first}");
    assert!(
        !first.contains("leaves") && !first.contains("enters"),
        "no clause can be named without a label table; got: {first}"
    );
    assert!(
        !first.contains("num"),
        "no tape can be named without a label table; got: {first}"
    );
}
