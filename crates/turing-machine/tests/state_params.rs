//! A routine's `state` parameters, end to end: the `.tmc` front end lowers
//! `goto <state parameter>` to `retx #k`, a call site supplying resume
//! states emits the `exits=(…)` operand, and the linked image leaves
//! through the exit the callee chose — under every call mechanism
//! (docs/tmt/language.md (reuse), docs/formats.md (bound calls)).
//!
//! The exit NUMBER is the parameter's position in the callee's signature,
//! and both ends read that one list: the callee's `retx #k` and the call
//! site's exits vector. The fixtures here are seeded so the two exits are
//! observably different, which is what makes reversing either end alone a
//! failing mutation rather than a no-op.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::{ObjectFile, SymbolDef};
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, Trap, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{assemble, disassemble_object, link};
use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;
use mtc_turing_machine::stdlib;

// ── harness ────────────────────────────────────────────────────────────────

const MECHS: [CallMech; 3] = [CallMech::Mono, CallMech::Frames, CallMech::Hybrid];

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter — copied verbatim from
/// `tests/mode_equivalence.rs::scratch`: a literal, non-unique directory
/// name is shared by every concurrently running `cargo test` invocation
/// that targets the same `CARGO_TARGET_TMPDIR`, so two such invocations
/// racing on the identical path can interleave their non-atomic writes.
fn scratch(name: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Compile `src` at `level`, returning the generated assembly.
fn assembly(src: &str, level: OptLevel) -> String {
    compile(
        src,
        CompileOptions {
            opt_level: level,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile: {e}"))
    .tma
}

/// Compile `src` at `level`, link under `mech`, run on blank tapes of the
/// given per-tape alphabet widths, and return the outcome with the final
/// tapes.
fn run_program(
    src: &str,
    level: OptLevel,
    mech: CallMech,
    widths: &[u32],
) -> (Outcome, Vec<TapeSnapshot>) {
    let object = compile(
        src,
        CompileOptions {
            opt_level: level,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile: {e}"))
    .object;
    let exe = link(
        &[object],
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the {mech} link failed: {e}"))
    .executable;
    run_image(&exe, widths)
}

/// Run a linked image on blank tapes of the given per-tape alphabet widths.
fn run_image(exe: &Executable, widths: &[u32]) -> (Outcome, Vec<TapeSnapshot>) {
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

// ── fixtures ───────────────────────────────────────────────────────────────

/// A routine with two `state` parameters, called from a two-tape machine.
/// `sub` reads the machine's first tape: a blank leaves through `hit` (the
/// FIRST state parameter, exit 0), anything else through `miss` (exit 1).
/// The two exit handlers write different glyphs to the SECOND tape, which
/// `sub` never touches — so the final `out` cell names the exit taken, and
/// the seed decides which one that is.
///
/// `then done` halts: `sub` leaves only through its exits, so a normal
/// return would show up as a different termination kind rather than as a
/// tape that merely looks wrong.
fn two_exits(seed: &str) -> String {
    format!(
        "\
alphabet ab {{ '_', '0', '1' }}

routine sub(tape t: ab, state hit, state miss) {{
  entry state s {{
    ['_'] -> goto hit;
    [*]   -> goto miss;
  }}
}}

machine {{
  tape d: ab;
  tape out: ab;
  entry state go {{ [*, *] -> write [{seed}, -] call sub(t = d, hit = won, miss = lost) then done; }}
  state won  {{ [*, *] -> write [-, '0'] stop; }}
  state lost {{ [*, *] -> write [-, '1'] stop; }}
  state done {{ [*, *] -> halt; }}
}}
"
    )
}

/// The exit the two seeds must reach: a blank first tape takes exit 0 and
/// writes `'0'` (index 1) on the second tape; a seeded `'1'` takes exit 1
/// and writes `'1'` (index 2).
const SEEDS: [(&str, u8); 2] = [("-", 1), ("'1'", 2)];

/// The facade: `outer` declares the two exits the machine binds, does no
/// work of its own, and hands them straight to `inner` — the delegation
/// shape a `graft` spells with `done = …` and a routine spells by passing
/// its own `state` parameters on. The observable is the same as
/// [`two_exits`]'s, so the same seeds decide it, but control now leaves
/// `inner` through `outer`'s exit and then `outer` through the machine's
/// state.
fn facade(seed: &str) -> String {
    format!(
        "\
alphabet ab {{ '_', '0', '1' }}

routine inner(tape t: ab, state hit, state miss) {{
  entry state s {{
    ['_'] -> goto hit;
    [*]   -> goto miss;
  }}
}}

routine outer(tape t: ab, state won, state lost) {{
  entry state s {{ [*] -> call inner(t = t, hit = won, miss = lost) then back; }}
  state back {{ [*] -> return; }}
}}

machine {{
  tape d: ab;
  tape out: ab;
  entry state go {{ [*, *] -> write [{seed}, -] call outer(t = d, won = w, lost = l) then done; }}
  state w    {{ [*, *] -> write [-, '0'] stop; }}
  state l    {{ [*, *] -> write [-, '1'] stop; }}
  state done {{ [*, *] -> halt; }}
}}
"
    )
}

/// Three exits, two of them TERMINATORS: a blank leaves through the
/// machine's own state, `'0'` through a `stop` argument and `'1'` through
/// a `halt` argument. The three seeds are told apart by the termination
/// kind as well as the tape, which is what makes swapping two terminator
/// kinds a failing mutation rather than an invisible one.
fn terminator_arguments(seed: &str) -> String {
    format!(
        "\
alphabet ab {{ '_', '0', '1' }}

routine pick(tape t: ab, state blank, state zero, state one) {{
  entry state s {{
    ['_'] -> goto blank;
    ['0'] -> goto zero;
    [*]   -> goto one;
  }}
}}

machine {{
  tape d: ab;
  tape out: ab;
  entry state go {{ [*, *] -> write [{seed}, -] call pick(t = d, blank = w, zero = stop, one = halt) then done; }}
  state w    {{ [*, *] -> write [-, '0'] stop; }}
  state done {{ [*, *] -> halt; }}
}}
"
    )
}

/// A `then` that names one of the enclosing routine's own `state`
/// parameters: `outer` calls a leaf that RETURNS normally, and the
/// continuation leaves `outer` through its own exit instead of resuming
/// in `outer`. `then never` in the machine halts, so a continuation that
/// wrongly printed `ret` would come back there and change the
/// termination kind.
const THEN_EXIT: &str = "\
alphabet ab { '_', '0', '1' }

routine leaf(tape t: ab) {
  entry state s { [*] -> return; }
}

routine outer(tape t: ab, state done) {
  entry state s { [*] -> call leaf(t = t) then done; }
}

machine {
  tape d: ab;
  tape out: ab;
  entry state go { [*, *] -> call outer(t = d, done = fin) then never; }
  state fin   { [*, *] -> write [-, '0'] stop; }
  state never { [*, *] -> halt; }
}
";

// ── lowering and emission ──────────────────────────────────────────────────

/// Mutation: lowering `goto <state parameter>` to a plain `return`; the
/// `retx` assertions go red. Emitting no `exits=` operand on the call, or
/// no `exits=` clause on the `.routine`, is caught by the same test.
#[test]
fn a_state_parameter_lowers_to_retx() {
    let tma = assembly(&two_exits("-"), OptLevel::O0);
    assert!(
        tma.contains(".routine sub, tapes=1, alpha=(3), exits=2"),
        "the callee publishes its exit count:\n{tma}"
    );
    assert!(
        tma.contains("retx    #0"),
        "exit 0 leaves through retx:\n{tma}"
    );
    assert!(
        tma.contains("retx    #1"),
        "exit 1 leaves through retx:\n{tma}"
    );
    let call = tma
        .lines()
        .find(|l| l.contains("call    sub"))
        .unwrap_or_else(|| panic!("no call line in:\n{tma}"))
        .trim();
    assert_eq!(call, "call    sub [0] exits=(won, lost)", "{tma}");
}

/// The exit number IS the parameter's position, and the call site builds
/// its vector in that same order. Run under all three mechanisms, with
/// both seeds, so the claim is executed rather than read off the text.
///
/// Mutation: reverse ONE end — the index `ir::lower_rule` assigns a
/// `state` parameter, or the order the call site's exits vector is built
/// in — and the seeded run takes the other branch. Reversing the
/// signature's parameter list itself is a no-op: both ends read that one
/// list, so it moves neither.
#[test]
fn the_exit_index_is_the_parameter_position() {
    for (seed, expected) in SEEDS {
        for mech in MECHS {
            let (outcome, snaps) = run_program(&two_exits(seed), OptLevel::O0, mech, &[3, 3]);
            assert_eq!(
                outcome,
                Outcome::Stopped,
                "seed {seed} under {mech} left through an exit, not a normal return"
            );
            assert_eq!(
                cell_at(&snaps[1], 0),
                expected,
                "seed {seed} under {mech} took the wrong exit"
            );
        }
    }
}

/// The three mechanisms agree on the tape AND the termination kind for an
/// exit-bearing call the compiler emitted — mono splices a per-site copy,
/// frames puts the vector in the site's descriptor, hybrid decides per
/// fold group.
///
/// Mutation: dropping the exits vector when the linker rewrites a site;
/// the mechanisms diverge.
#[test]
fn the_three_mechanisms_agree() {
    for (seed, _) in SEEDS {
        for level in [OptLevel::O0, OptLevel::O1] {
            let results: Vec<_> = MECHS
                .iter()
                .map(|&m| run_program(&two_exits(seed), level, m, &[3, 3]))
                .collect();
            for (m, r) in MECHS.iter().zip(&results).skip(1) {
                assert_eq!(
                    (&results[0].0, &results[0].1),
                    (&r.0, &r.1),
                    "mono vs {m} diverged on an exit-bearing program (seed {seed})"
                );
            }
        }
    }
}

/// Everything the compiler can put in an object is expressible in
/// hand-written assembly, proven by dis → asm byte-identity. The exit
/// vocabulary is the newest thing it emits — `exits=K` on the signature,
/// the `exits=(…)` operand, `retx #k` — and this fixture exports no
/// alphabet, so none of the declared exceptions to that gate applies.
///
/// Mutation: print an exit clause or operand the assembler cannot read
/// back (a bare number for a label, a missing binding group); the
/// reassemble step fails or the bytes differ.
#[test]
fn an_exit_bearing_object_disassembles_back_to_itself() {
    let object = compile(&two_exits("-"), CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile: {e}"))
        .object;
    let text = disassemble_object(&object);
    assert!(text.contains("exits=2"), "{text}");
    assert!(text.contains("retx    #0"), "{text}");
    assert!(text.contains("exits=("), "{text}");
    let again = assemble(&text, false).expect("the disassembly reassembles");
    assert_eq!(again.to_bytes(), object.to_bytes(), "{text}");
}

/// A RECURSIVE facade — `walk` forwards its own exits to itself — is the
/// one shape the copy paths refuse: a per-site copy cannot close that
/// loop. BOTH copy paths name it (hybrid delegates the shape to mono),
/// and frames links it, because there the vector lives in the site's
/// descriptor rather than in a splice. The distinctive wording is asserted
/// rather than the advice, which every copy-path refusal carries.
///
/// The frames image is then RUN: the tape walks one cell of `'0'` before
/// the blank, so the recursion goes one level deep and the exit has to
/// thread back out through both frames.
///
/// Mutation: drop the exits vector at the recursive site; the copy paths
/// stop refusing and link a program whose inner call resumes wherever the
/// outer one did.
#[test]
fn a_recursive_facade_is_refused_by_the_copy_paths() {
    const RECURSIVE: &str = "\
alphabet ab { '_', '0', '1' }

routine walk(tape t: ab, state hit, state miss) {
  entry state s {
    ['_'] -> goto hit;
    ['0'] -> move [>] call walk(t = t, hit = hit, miss = miss) then done;
    [*]   -> goto miss;
  }
  state done { [*] -> return; }
}

machine {
  tape d: ab;
  tape out: ab;
  entry state seed { [*, *] -> write ['0', -] move [>, .] goto back; }
  state back { [*, *] -> move [<, .] call walk(t = d, hit = won, miss = lost) then fin; }
  state won  { [*, *] -> write [-, '0'] stop; }
  state lost { [*, *] -> write [-, '1'] stop; }
  state fin  { [*, *] -> halt; }
}
";
    let object = compile(RECURSIVE, CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile: {e}"))
        .object;
    for mech in [CallMech::Mono, CallMech::Hybrid] {
        let err = link(
            std::slice::from_ref(&object),
            &[],
            LinkOptions {
                call_mech: mech,
                ..Default::default()
            },
        )
        .err()
        .unwrap_or_else(|| panic!("{mech} must refuse a recursive facade"))
        .to_string();
        assert!(
            err.contains("reaches `walk` again through its exits"),
            "{mech}: {err}"
        );
    }

    let exe = link(
        std::slice::from_ref(&object),
        &[],
        LinkOptions {
            call_mech: CallMech::Frames,
            ..Default::default()
        },
    )
    .expect("frames carries the vector in the descriptor and links")
    .executable;
    let (outcome, snaps) = run_image(&exe, &[3, 3]);
    assert_eq!(outcome, Outcome::Stopped, "the frames image runs");
    assert_eq!(
        cell_at(&snaps[1], 0),
        1,
        "the blank past the seeded cell leaves through exit 0, twice over"
    );
}

// ── the three resume shapes ────────────────────────────────────────────────

/// Forwarding a continuation: `outer` hands its own exits to `inner`, so
/// control leaves `inner` through `outer`'s exit and `outer` through the
/// machine's state. Run on both seeds, under all three mechanisms, at both
/// opt levels.
///
/// The resume states this mints disturb no tape and move no head, which the
/// absolute assertions below pin: the seeded cell survives and the head
/// stays where the call left it.
///
/// Mutations: point the exits vector at the wrong resume state (the
/// seeded run takes the other branch); give the minted state a write or a
/// move (the seed or the head assertion goes red).
#[test]
fn a_forwarded_continuation_leaves_through_the_facades_exit() {
    for (seed, expected) in SEEDS {
        let seeded_cell = if expected == 1 { 0 } else { 2 };
        for level in [OptLevel::O0, OptLevel::O1] {
            for mech in MECHS {
                let (outcome, snaps) = run_program(&facade(seed), level, mech, &[3, 3]);
                assert_eq!(
                    outcome,
                    Outcome::Stopped,
                    "seed {seed} under {mech} at {level:?} left through an exit"
                );
                assert_eq!(
                    cell_at(&snaps[1], 0),
                    expected,
                    "seed {seed} under {mech} at {level:?} took the wrong exit"
                );
                assert_eq!(
                    cell_at(&snaps[0], 0),
                    seeded_cell,
                    "the resume states write nothing ({seed}, {mech}, {level:?})"
                );
                assert_eq!(
                    snaps[0].head, 0,
                    "the resume states move no head ({seed}, {mech}, {level:?})"
                );
            }
        }
    }
}

/// A `state` argument may be a terminator, exactly as a `graft`'s exit
/// may: `zero = stop` ends the run where it is, `one = halt` ends it
/// abnormally, and the third exit resumes at a state as usual. Told apart
/// by the termination KIND, so swapping the two terminator kinds is a
/// failing mutation.
#[test]
fn a_terminator_state_argument_ends_the_run() {
    // (seed, outcome, the `out` cell the run leaves behind)
    let cases: [(&str, Outcome, u8); 3] = [
        ("-", Outcome::Stopped, 1),
        ("'0'", Outcome::Stopped, 0),
        ("'1'", Outcome::Halted, 0),
    ];
    for (seed, want, out) in cases {
        for level in [OptLevel::O0, OptLevel::O1] {
            for mech in MECHS {
                let (outcome, snaps) =
                    run_program(&terminator_arguments(seed), level, mech, &[3, 3]);
                assert_eq!(
                    outcome, want,
                    "seed {seed} under {mech} at {level:?} ended the wrong way"
                );
                assert_eq!(cell_at(&snaps[1], 0), out, "seed {seed} under {mech}");
            }
        }
    }
}

/// A `then` naming one of the enclosing routine's `state` parameters is
/// the instruction after the call — `retx #k` where a `return`
/// continuation prints `ret` — so it needs no resume state at all.
///
/// Mutation: print `ret` there; the instruction after the call changes.
#[test]
fn a_then_that_names_a_state_parameter_prints_retx() {
    let tma = assembly(THEN_EXIT, OptLevel::O0);
    let call_then: Vec<&str> = tma
        .lines()
        .skip_while(|l| !l.contains("call    leaf"))
        .take(2)
        .map(|l| l.trim())
        .collect();
    assert_eq!(call_then, vec!["call    leaf [0]", "retx    #0"], "{tma}");
}

/// The behavioural half, standing on its own so the consequence is
/// observed and not merely implied by the text: leaving through the exit
/// reaches the machine's `fin`, which stops. A continuation that returned
/// normally instead would come back to `then never`, which halts.
///
/// Mutation: print `ret` for such a `then`; every run halts instead of
/// stopping, and the tape stays blank.
#[test]
fn a_then_that_names_a_state_parameter_returns_through_its_exit() {
    for level in [OptLevel::O0, OptLevel::O1] {
        for mech in MECHS {
            let (outcome, snaps) = run_program(THEN_EXIT, level, mech, &[3, 3]);
            assert_eq!(
                outcome,
                Outcome::Stopped,
                "under {mech} at {level:?} the continuation left through the exit"
            );
            assert_eq!(cell_at(&snaps[1], 0), 1, "under {mech} at {level:?}");
        }
    }
}

// ── the two optimizer guards ───────────────────────────────────────────────

/// A bindless exit-bearing site must never become a tail call: the callee
/// leaves through `retx #k`, which indexes the SITE's exit vector — and a
/// tail jump carries no site at all, so the callee would index the
/// original caller's.
///
/// Mutation: drop the `exits.is_empty()` guard in the `tail-call` pass;
/// the site becomes `jmp @lib::pick` and both assertions go red.
#[test]
fn an_exit_bearing_site_is_never_tail_called() {
    let dir = scratch("tail_call_guard");
    let header = write_file(
        &dir,
        "lib.tmh",
        "\
namespace lib {
  export routine pick(state hit, state miss);
}
",
    );
    let src = write_file(
        &dir,
        "app.tmc",
        "\
alphabet ab { '_', '0', '1' }

use lib::pick;

routine go(tape d: ab) {
  entry state s { [*] -> call pick(hit = won, miss = lost) then return; }
  state won  { [*] -> write ['0'] return; }
  state lost { [*] -> write ['1'] return; }
}

machine {
  tape d: ab;
  entry state m { [*] -> call go(d = d) then done; }
  state done { [*] -> stop; }
}
",
    );
    let out = execute(&args(&[
        "compile",
        src.to_str().unwrap(),
        "-O1",
        "-S",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("app.tma").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let tma = std::fs::read_to_string(dir.join("app.tma")).unwrap();
    assert!(
        tma.contains("call    lib::pick [] exits=(won, lost)"),
        "the site stays a call carrying its exits:\n{tma}"
    );
    assert!(
        !tma.contains("jmp     @lib::pick"),
        "the site must not be tail-jumped:\n{tma}"
    );
}

/// Splicing an exit-bearing callee into its site would have to replace the
/// callee's `retx #k` rows with the site's own exit targets. That fold is
/// not attempted here, so `inline` refuses the callee outright — a stated
/// conservatism, pinned so it stays a decision.
///
/// The eligibility rule lives in ONE place — the candidate set — so this
/// pins that one guard.
///
/// Mutation: drop the `w.exits == 0` test from `inline`'s candidate
/// filter; the callee becomes splice-eligible and the call disappears
/// from the `-O1` assembly (in practice the splice hits the callee's own
/// `retx` row first, which is the same failure one step earlier).
#[test]
fn an_exit_bearing_callee_is_not_inlined() {
    let tma = assembly(&two_exits("-"), OptLevel::O1);
    assert!(
        tma.contains("call    sub [0] exits=(won, lost)"),
        "the exit-bearing call survives -O1:\n{tma}"
    );
}

// ── the no-state-parameter floor ───────────────────────────────────────────

/// A program that declares no `state` parameter emits exactly what it
/// emitted before routines could have one: `exits=` prints only for a
/// nonzero count, and no row lowers to `retx`. The embedded standard
/// library is the corpus program — its compiled object is byte-pinned by
/// its own golden, so this test's job is the printer, at both opt levels.
///
/// Mutation: print `exits=0` on every `.routine` (or print the clause
/// unconditionally); both halves go red, and the stdlib object's golden
/// moves with them.
#[test]
fn a_program_without_state_parameters_is_byte_identical() {
    for level in [OptLevel::O0, OptLevel::O1] {
        let tma = assembly(stdlib::SOURCE, level);
        assert!(
            !tma.contains("exits="),
            "no routine declares an exit, so no clause prints:\n{tma}"
        );
        assert!(
            !tma.contains("retx"),
            "nothing leaves through an exit:\n{tma}"
        );
    }
}

// ── the exit-count ceiling ─────────────────────────────────────────────────

/// A routine with `n` `state` parameters, generated rather than written
/// out: the wire's exit count is one byte, so 255 is the ceiling and 256
/// is the diagnostic. Each parameter sits on its OWN line, so the span a
/// diagnostic reports names one parameter and not merely the signature.
fn many_state_params(n: usize) -> String {
    let params: Vec<String> = (0..n).map(|i| format!("  state p{i},\n")).collect();
    format!(
        "\
alphabet ab {{ '_', '0' }}

routine wide(
  tape t: ab,
{}) {{
  entry state s {{ [*] -> goto p0; }}
}}

machine {{
  tape d: ab;
  entry state m {{ [*] -> stop; }}
}}
",
        // The last parameter carries no trailing comma.
        params.join("").trim_end().trim_end_matches(',')
    )
}

/// The ceiling is ONE conversion, where a signature is resolved — so the
/// count a compiled world publishes and the count a merely DECLARED one
/// carries are the same check.
///
/// Mutation: narrow that conversion with a bare `as u8`; the
/// 256-parameter routine publishes `exits=0` while its body leaves
/// through exit 0, and the compile fails as `internal-error` (the world
/// invariants catch it) rather than naming the ceiling.
#[test]
fn more_than_255_state_parameters_is_a_typed_error() {
    let err = compile(&many_state_params(256), CompileOptions::default())
        .expect_err("256 state parameters is one too many");
    assert_eq!(err.kind.code(), "too-many-state-params", "{err}");
    // The OFFENDING PARAMETER, not the routine's name: `wide(` opens on
    // line 3, the tape parameter is line 4, so the 256th `state`
    // parameter is line 260, at the column its name starts.
    assert_eq!(
        (err.span.start.line, err.span.start.col),
        (260, 9),
        "the diagnostic points at the parameter past the ceiling: {err}"
    );
}

/// The near miss: exactly 255 is the widest signature the wire can carry,
/// and it compiles and publishes its count.
#[test]
fn exactly_255_state_parameters_compiles() {
    let tma = assembly(&many_state_params(255), OptLevel::O0);
    assert!(
        tma.contains(".routine wide, tapes=1, alpha=(2), exits=255"),
        "{tma}"
    );
}

/// A header declaring `n` exits, and a caller that binds every one of
/// them — the pair that reaches the exit count through DECLARATIONS
/// rather than through a compiled body.
fn header_with_exits(n: usize) -> String {
    let params: Vec<String> = (0..n).map(|i| format!("state e{i}")).collect();
    format!(
        "\
namespace lib {{
  export alphabet bits {{ '_', '0', '1' }}
  export routine big(tape n: bits, {});
}}
",
        params.join(", ")
    )
}

fn caller_binding_exits(n: usize) -> String {
    let args: Vec<String> = (0..n).map(|i| format!("e{i} = done")).collect();
    format!(
        "\
use lib::bits;

machine {{
  tape d: bits;
  entry state go {{ [*] -> call lib::big(n = d, {}) then done; }}
  state done {{ [*] -> stop; }}
}}
",
        args.join(", ")
    )
}

/// The ceiling covers a signature that is only DECLARED, not compiled:
/// the wire cannot hold the count either way, and the exits vector a
/// caller builds from that declaration is what would carry it. Reported
/// against the HEADER's own path, since that is the file to fix.
///
/// Mutation: check the ceiling only where a world is lowered; this pair
/// then reaches the object writer with a 256-entry exit vector and
/// panics there instead of reporting anything.
#[test]
fn a_header_declaring_too_many_state_parameters_is_a_typed_error() {
    let dir = scratch("header_exit_ceiling");
    let header = write_file(&dir, "lib.tmh", &header_with_exits(256));
    let app = write_file(&dir, "app.tmc", &caller_binding_exits(256));
    let err = execute(&args(&[
        "compile",
        app.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("app.tmo").to_str().unwrap(),
    ]))
    .expect_err("a declared signature is held to the same ceiling");
    assert!(err.contains("[too-many-state-params]"), "{err}");
    assert!(
        err.contains("lib.tmh"),
        "the header is the file to fix: {err}"
    );
}

/// The near miss on the declarations path: 255 declared exits compile,
/// link-ready, through the same route.
#[test]
fn a_header_declaring_exactly_255_state_parameters_compiles() {
    let dir = scratch("header_exit_ceiling_near");
    let header = write_file(&dir, "lib.tmh", &header_with_exits(255));
    let app = write_file(&dir, "app.tmc", &caller_binding_exits(255));
    let out = execute(&args(&[
        "compile",
        app.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("app.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("255 declared exits is the ceiling, not past it: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
}

// ── out-of-unit callees ────────────────────────────────────────────────────

/// A `.tmc` library whose exported routine takes a `state` parameter, and
/// the header that declares it.
const LIB_SRC: &str = "\
namespace lib {
  export alphabet bits { '_', '0', '1' }

  export routine pick(tape n: bits, state hit, state miss) {
    entry state s {
      ['_'] -> goto hit;
      [*]   -> goto miss;
    }
  }
}
";

const LIB_HEADER: &str = "\
namespace lib {
  export alphabet bits { '_', '0', '1' }
  export routine pick(tape n: bits, state hit, state miss);
}
";

/// The caller: a symbolic (out-of-unit) binding plus an exits vector the
/// callee's DECLARED parameter order fixes.
const APP_SRC: &str = "\
use lib::bits;

machine {
  tape d: bits;
  tape out: bits;
  entry state go { [*, *] -> call lib::pick(n = d, hit = won, miss = lost) then done; }
  state won  { [*, *] -> write [-, '0'] stop; }
  state lost { [*, *] -> write [-, '1'] stop; }
  state done { [*, *] -> halt; }
}
";

/// With the callee's declarations in hand the compiler can order the
/// exits vector — the wire's vector is positional and carries no names —
/// so the site emits a symbolic binding AND an exits operand, links
/// against the real library object and runs to the right exit.
///
/// Mutation: emit the exits vector in the call site's SOURCE order rather
/// than the callee's declared order; a site that names them the other way
/// round takes the wrong exit.
#[test]
fn an_external_exit_bearing_call_links_and_runs() {
    let dir = scratch("external_exits");
    let header = write_file(&dir, "lib.tmh", LIB_HEADER);
    let app = write_file(&dir, "app.tmc", APP_SRC);
    let lib = write_file(&dir, "lib.tmc", LIB_SRC);

    let out = execute(&args(&[
        "compile",
        app.to_str().unwrap(),
        "-S",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("app.tma").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile app: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let tma = std::fs::read_to_string(dir.join("app.tma")).unwrap();
    let call = tma
        .lines()
        .find(|l| l.contains("lib::pick"))
        .unwrap_or_else(|| panic!("no call line in:\n{tma}"))
        .trim();
    assert_eq!(call, "call    lib::pick [n: 0] exits=(won, lost)", "{tma}");

    // Compile both units for real and link them. The library compiles
    // against no declarations of its own — it IS the declared unit.
    let out = execute(&args(&[
        "compile",
        app.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("app.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile app object: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let out = execute(&args(&[
        "compile",
        lib.to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("lib.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile lib object: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let out = execute(&args(&[
        "link",
        dir.join("app.tmo").to_str().unwrap(),
        dir.join("lib.tmo").to_str().unwrap(),
        "--nostdlib",
        "--call-mech",
        "frames",
        "-o",
        dir.join("app.tmx").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("link: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);

    let tape = dir.join("blank.tmt");
    let out = execute(&args(&[
        "tape-block",
        "new",
        "--from",
        dir.join("app.tmx").to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("tape-block new: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let saved = dir.join("out.tmt");
    let out = execute(&args(&[
        "run",
        dir.join("app.tmx").to_str().unwrap(),
        "--tape-block",
        tape.to_str().unwrap(),
        "--save-tape-block",
        saved.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("run: {e}"));
    assert_eq!(
        out.code, 0,
        "the blank run stops through an exit: {}",
        out.stderr
    );
    let shown = show(&saved);
    assert!(
        shown.contains("tape 1: origin 0, head 0 reads '1'"),
        "the blank tape takes exit 0, whose handler writes index 1:\n{shown}"
    );
}

/// `tmt tape-block show FILE` as text.
fn show(path: &std::path::Path) -> String {
    let out = execute(&args(&["tape-block", "show", path.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("tape-block show: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    out.stdout
}

/// Without the callee's declarations the compiler cannot order the exits
/// vector at all — the vector is positional on the wire and carries no
/// names, and the linker's own fix-up reorders NAMED binding entries, not
/// exits. So the call is refused here rather than emitted in source order
/// and silently mis-resumed.
///
/// Mutation: fall back to source order when the declarations are missing;
/// this compile succeeds and the diagnostic disappears.
#[test]
fn an_external_exit_bearing_call_without_declarations_is_refused() {
    let dir = scratch("external_exits_undeclared");
    let app = write_file(&dir, "app.tmc", APP_SRC_LOCAL_ALPHABET);
    let err = execute(&args(&[
        "compile",
        app.to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("app.tmo").to_str().unwrap(),
    ]))
    .expect_err("a call passing state arguments needs the callee's declarations");
    assert!(err.contains("[state-args-need-declarations]"), "{err}");
}

/// The same caller with its own local alphabet, so the missing
/// declarations bite on the exits vector rather than on `use lib::bits`.
const APP_SRC_LOCAL_ALPHABET: &str = "\
alphabet bits { '_', '0', '1' }

machine {
  tape d: bits;
  tape out: bits;
  entry state go { [*, *] -> call lib::pick(n = d, hit = won, miss = lost) then done; }
  state won  { [*, *] -> write [-, '0'] stop; }
  state lost { [*, *] -> write [-, '1'] stop; }
  state done { [*, *] -> halt; }
}
";

// ── `noreturn`: inferred, declarable, exported ──────────────────────────────

/// The [`RoutineInterface`](mtc_core::formats::object::RoutineInterface)
/// for the routine/machine named `name`, found by its symbol's blob index —
/// mirrors `interface_emission.rs::routine_interface`, kept local rather
/// than shared: each integration test file in this crate defines its own
/// local helpers instead of depending on a shared test-support module.
fn returns_bit(object: &ObjectFile, name: &str) -> bool {
    let symbol = object
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol named `{name}` in {:?}", object.symbols));
    let blob = match symbol.def {
        SymbolDef::Defined { blob } | SymbolDef::Local { blob } => blob,
        SymbolDef::External => panic!("`{name}` is external, not defined in this object"),
    };
    object
        .interface
        .as_ref()
        .unwrap_or_else(|| panic!("object carries no interface section"))
        .routines[blob as usize]
        .returns
}

/// `forever`'s only `return` sits in `dead` — a state nothing in the body
/// ever `goto`s or resumes at, so it survives to `-O0` codegen but is
/// deleted by `-O1`'s `dce` pass. Built with EXACTLY this shape because a
/// fact computed AFTER the optimizer would see the `return` at `-O0`
/// (`dce` never runs) and not at `-O1` (already deleted) — the one shape
/// that makes "compute it post-optimizer" and "compute it pre-optimizer"
/// disagree.
const DEAD_RETURN_NORETURN: &str = "\
alphabet ab { '_', 'a' }

export routine forever(tape t: ab) {
  entry state s { [*] -> goto s; }
  state dead { [*] -> return; }
}

machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";

/// The inferred fact never depends on `-O`: `forever`'s dead `return` is
/// counted conservatively either way, so both levels agree — here, both
/// say "can return" (`returns == true`), since a dead state still counts.
///
/// Mutation: compute `IrWorld::returns` from `ir::lower_world` AFTER
/// `optimizer::optimize` runs instead of before (feed the ALREADY-lowered,
/// then separately optimized-per-level IR into `body_can_return`'s
/// equivalent post-hoc): at `-O0` `dce` never runs, so `dead` and its
/// `return` survive and the fact still reads `true`; at `-O1` `dce` has
/// already deleted `dead` by the time the fact is read, so it flips to
/// `false` — verified by hand: reordering `ir.rs`'s `returns` field to be
/// filled from a post-optimize scan reds this test at `-O1` while `-O0`
/// stays green, the exact asymmetry this assertion exists to catch.
#[test]
fn noreturn_is_inferred_from_the_body_independently_of_opt_level() {
    let o0 = compile(
        DEAD_RETURN_NORETURN,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile -O0: {e}"))
    .object;
    let o1 = compile(
        DEAD_RETURN_NORETURN,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile -O1: {e}"))
    .object;
    assert_eq!(
        returns_bit(&o0, "forever"),
        returns_bit(&o1, "forever"),
        "the `returns` bit must not depend on the optimization level"
    );
    // Both must additionally read `true`: the dead `return` counts.
    assert!(
        returns_bit(&o0, "forever"),
        "the dead `return` still counts at -O0"
    );
    assert!(
        returns_bit(&o1, "forever"),
        "the dead `return` still counts at -O1"
    );
}

/// A routine declared `noreturn` whose body carries a live `return` is
/// refused — the declared-and-wrong shape.
#[test]
fn a_declared_noreturn_that_returns_is_refused() {
    let src = "\
alphabet ab { '_', 'a' }

routine liar(tape t: ab) noreturn {
  entry state s { [*] -> return; }
}

machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";
    let err = compile(src, CompileOptions::default()).unwrap_err();
    assert!(err.to_string().contains("[noreturn-violated]"), "{err}");
}

/// The same body without the `return` — the near miss: a truthful
/// `noreturn` declaration compiles clean.
#[test]
fn a_truthful_one_compiles() {
    let src = "\
alphabet ab { '_', 'a' }

routine honest(tape t: ab) noreturn {
  entry state s { [*] -> goto s; }
}

machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";
    compile(src, CompileOptions::default()).unwrap_or_else(|e| panic!("compile: {e}"));
}

/// A routine whose ONLY way out is handing `return` to a callee as a
/// `state` ARGUMENT — never its own `return`, never a `then return` —
/// still counts as a way to return. `outer` calls `inner` with
/// `hit = return`; `inner` itself leaves only through its own exit
/// (`goto hit`), so `inner` is separately inferred `noreturn`, and the
/// call's own `then` is deliberately `stop`, a DIFFERENT terminator, and
/// therefore dead code (`inner` never returns normally to reach it) — so
/// the `then`-side check (`matches!(then, Some(Continuation::Return))`)
/// cannot be what makes this pass; only `args_return` scanning the
/// call's own arguments can.
///
/// Mutation: neutralize `args_return` (`ir.rs`) to `return false;`
/// unconditionally — `outer` would then be wrongly inferred `noreturn`,
/// and BOTH assertions below would fail: the interface bit would read
/// `false`, and declaring `outer` `noreturn` would compile clean instead
/// of failing `noreturn-violated`. Verified by hand: applying that exact
/// mutation reds this test while leaving `--test state_params` otherwise
/// green (`crates/turing-machine/src/ir.rs::args_return`).
#[test]
fn return_as_a_state_argument_on_a_direct_call_counts_as_a_way_out() {
    let src = "\
alphabet ab { '_', 'a' }

routine inner(tape t: ab, state hit) {
  entry state s { [*] -> goto hit; }
}

export routine outer(tape t: ab) {
  entry state s { [*] -> call inner(t = t, hit = return) then stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile: {e}"))
        .object;
    assert!(
        returns_bit(&object, "outer"),
        "handing `return` to a callee as a state argument must count as a way to return"
    );

    let lying = "\
alphabet ab { '_', 'a' }

routine inner(tape t: ab, state hit) {
  entry state s { [*] -> goto hit; }
}

export routine outer(tape t: ab) noreturn {
  entry state s { [*] -> call inner(t = t, hit = return) then stop; }
}
";
    let err = compile(lying, CompileOptions::default()).unwrap_err();
    assert!(err.to_string().contains("[noreturn-violated]"), "{err}");
}

/// The same arm, through a `bind` declaration's FIXED arguments instead of
/// a direct call's own — the other half of `args_return`'s two callers
/// (a bind's own args are looked up once, by the bind's declaration, not
/// re-read per call site). `outer` here calls the bind `b()` with no
/// arguments of its own at all; every argument, `hit = return` included,
/// comes from `bind inner(...) as b;`.
///
/// Mutation: the same `args_return` neutralization; the assertion on
/// `outer`'s `returns` bit fails identically, through the OTHER call
/// site `args_return` is reached from (`ir.rs`'s `BindCall` arm).
#[test]
fn return_as_a_state_argument_fixed_in_a_bind_counts_as_a_way_out() {
    let src = "\
alphabet ab { '_', 'a' }

routine inner(tape t: ab, state hit) {
  entry state s { [*] -> goto hit; }
}

export routine outer(tape t: ab) {
  bind inner(t = t, hit = return) as b;
  entry state s { [*] -> call b() then stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile: {e}"))
        .object;
    assert!(
        returns_bit(&object, "outer"),
        "a bind's own return-bound state argument must count as a way to return"
    );
}

/// The compiled object's interface carries the inferred fact as its
/// `returns` bit — `false` for a genuinely `noreturn` routine.
#[test]
fn the_interface_carries_the_returns_bit() {
    let src = "\
alphabet ab { '_', 'a' }

export routine forever(tape t: ab) noreturn {
  entry state s { [*] -> goto s; }
}

machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile: {e}"))
        .object;
    assert!(
        !returns_bit(&object, "forever"),
        "a `noreturn` routine's interface must carry `returns: false`"
    );
}

/// `tmt interface` prints `noreturn` on BOTH arms, from the same fact
/// reached two different ways: the source arm infers it from the body, the
/// object arm reads the wire's `returns` bit codegen wrote from that same
/// inference — so the two arms agree by construction.
#[test]
fn tmt_interface_prints_noreturn() {
    let dir = scratch("interface_noreturn");
    // `ab` is EXPORTED so the object arm's alphabet-matching rule (1)
    // reaches it by its own short name too, exactly like the source
    // arm — otherwise the object arm synthesizes a private name
    // (`resolve_object_alphabet`'s rule (4)) and the two arms'
    // signature LINES would legitimately differ in the alphabet name
    // alone, which is not what this test is checking.
    const SRC: &str = "\
export alphabet ab { '_', 'a' }

export routine forever(tape t: ab) noreturn {
  entry state s { [*] -> goto s; }
}

machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";
    let src_path = write_file(&dir, "src.tmc", SRC);

    let source_arm = execute(&args(&["interface", src_path.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("interface: {e}"));
    assert_eq!(source_arm.code, 0, "{}", source_arm.stderr);
    assert!(
        source_arm
            .stdout
            .contains("export routine forever(tape t: ab writes {}) noreturn;"),
        "source arm: {}",
        source_arm.stdout
    );

    let obj_path = dir.join("src.tmo");
    let compiled = execute(&args(&[
        "compile",
        src_path.to_str().unwrap(),
        "-o",
        obj_path.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile: {e}"));
    assert_eq!(compiled.code, 0, "{}", compiled.stderr);

    let object_arm = execute(&args(&["interface", obj_path.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("interface: {e}"));
    assert_eq!(object_arm.code, 0, "{}", object_arm.stderr);
    assert!(
        object_arm
            .stdout
            .contains("export routine forever(tape t: ab writes {}) noreturn;"),
        "object arm: {}",
        object_arm.stdout
    );
}

/// A bodiless (`.tmh`) declarations-only reading of a `noreturn` routine
/// round-trips: there is no body to infer from, so the declared clause is
/// the only source of truth, and it must reprint unchanged.
#[test]
fn a_bodiless_noreturn_declaration_round_trips() {
    let dir = scratch("interface_noreturn_header");
    let header_path = write_file(
        &dir,
        "src.tmh",
        "\
alphabet ab { '_', 'a' }

export routine forever(tape t: ab writes {}) noreturn;
",
    );
    let out = execute(&args(&["interface", header_path.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("interface: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout
            .contains("export routine forever(tape t: ab writes {}) noreturn;"),
        "{}",
        out.stdout
    );
}

// ── `then` optional against a known `noreturn` callee, end to end ──────────

/// `pick` never carries a `return`, only its two `state` parameters as
/// exits, so it is inferred `noreturn` — the call site's own `then` may
/// therefore be omitted, putting the call in TAIL position
/// (docs/tmt/language.md (reuse)). Seeded so the exit that fires is
/// unambiguous: `go` writes `'1'` before calling, so `pick`'s dispatch
/// takes `hit`, not the blank-reading `miss`.
const NORETURN_TAIL: &str = "\
alphabet ab { '_', '0', '1' }

routine pick(tape t: ab, state hit, state miss) noreturn {
  entry state s {
    ['1'] -> hit;
    [*]   -> miss;
  }
}

machine {
  tape d: ab;
  entry state go { [*] -> write ['1'] call pick(t = d, hit = won, miss = lost); }
  state won  { [*] -> write ['0'] stop; }
  state lost { [*] -> write ['1'] stop; }
}
";

/// A tail-position call prints a synthesized `trap #0` right after it,
/// never `ret`, `retx`, `stp`, `hlt`, or `jmp` — the resume shapes a
/// WRITTEN `then` would print. An honest program never reaches this
/// trap (the callee never returns), but it turns a LYING `noreturn`
/// (a callee that returns anyway) into a controlled stop instead of
/// falling through into whatever the linker placed next
/// (docs/tmt/isa.md (explicit traps)).
///
/// Mutation: falling back to some default resume `Then` (e.g. always
/// synthesizing `stp`) when `IrTransition::CallThen.then` is `None`;
/// the line right after `call` would then be `stp` instead of `trap #0`.
#[test]
fn an_exit_bearing_tail_call_prints_a_trap() {
    let tma = assembly(NORETURN_TAIL, OptLevel::O0);
    let after_call: &str = tma
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("call    pick"))
        .nth(1)
        .map(|l| l.trim())
        .unwrap_or_else(|| panic!("no line after the call:\n{tma}"));
    assert_eq!(
        after_call, "trap    #0",
        "the tail call carries no safety trap:\n{tma}"
    );
}

/// The tail-position `noreturn` shape, executed end to end: an exit-bearing
/// call whose `then` is omitted because its callee is inferred `noreturn`
/// links and runs correctly under all three call mechanisms, at both
/// optimization levels — the `.tmc`-level counterpart to the hand-written
/// `.tma` fixture `link_matrix.rs::TAIL_POSITION_NORETURN` proves at the
/// object level. None of the three mechanisms refuses it.
///
/// Mutation: refuse a `then`-omitted call unconditionally in
/// `ir::lower_rule` (never checking `known_noreturn`); this fixture stops
/// compiling at all, at either level.
#[test]
fn a_noreturn_exits_only_callee_may_omit_then_and_still_links_and_runs() {
    for level in [OptLevel::O0, OptLevel::O1] {
        for mech in MECHS {
            let (outcome, snaps) = run_program(NORETURN_TAIL, level, mech, &[3]);
            assert_eq!(
                outcome,
                Outcome::Stopped,
                "under {mech} at {level:?} the tail-position call did not run to a stop"
            );
            assert_eq!(
                cell_at(&snaps[0], 0),
                1,
                "under {mech} at {level:?}: exit `hit` should have fired, leaving '0' \
                 (index 1) behind"
            );
        }
    }
}

/// A `then` omitted against a callee that is NOT known to be `noreturn` —
/// here, one this unit cannot see at all (no declarations for `outside`) —
/// stays a compile error, never silently accepted. `then` also stays
/// mandatory against a callee that CAN return; `an_exit_bearing_site_is_
/// never_tail_called`'s own fixture and every other `then …` call in this
/// file already exercise that half continuously.
#[test]
fn an_omitted_then_against_an_unknown_callee_is_refused() {
    let src = "\
alphabet ab { '_', 'a' }

machine {
  tape t: ab;
  entry state go { [*] -> call outside(t = t); }
}
";
    let err = compile(src, CompileOptions::default()).unwrap_err();
    assert!(err.to_string().contains("[then-required]"), "{err}");
}

// ── a lying `noreturn` traps instead of falling through ────────────────────

/// `liar`'s header — the only declaration the CALLER ever sees.
const LIAR_HEADER: &str = "\
alphabet ab { '_', 'a' }

export routine liar(tape t: ab writes {}) noreturn;
";

/// The real definition: the header LIED. `liar` returns.
const LIAR_LIB: &str = "\
alphabet ab { '_', 'a' }

export routine liar(tape t: ab writes {}) {
  entry state s { [*] -> return; }
}
";

/// The caller trusts the header and omits `then` — legally, as far as it
/// can tell.
const LIAR_CALLER: &str = "\
alphabet ab { '_', 'a' }

use liar;

machine {
  tape t: ab;
  entry state go { [*] -> call liar(t = t); }
}
";

/// A `noreturn` declaration that LIES — the header says `noreturn`, the
/// LINKED definition actually `return`s — is a run-time TRAP under every
/// call mechanism, never a silent `Stopped` with execution wandering into
/// whatever the linker placed after the call. This is the whole point of
/// the synthesized `trap #0` codegen now emits for a tail-position call:
/// without it, the callee's `ret` lands on the caller's own next
/// instruction (or, with no instruction of its own to fall into, whatever
/// code the linker placed physically next) and runs on silently.
///
/// Mutation: reverting `codegen.rs`'s `Term::Call { then: None, .. }`
/// emission from `trap #0` back to nothing — the run ends `Stopped` with
/// exit code 0 instead of trapping, under every mechanism.
#[test]
fn a_lying_noreturn_header_traps_instead_of_falling_through() {
    let dir = scratch("lying_noreturn");
    let header = write_file(&dir, "liar.tmh", LIAR_HEADER);
    let caller = write_file(&dir, "caller.tmc", LIAR_CALLER);

    // The caller compiles only against the LYING header — `--extern` is
    // the one route an integration test has to build a `Declarations`
    // table at all (`Declarations`'s own constructors besides `stdlib()`
    // are crate-private).
    let out = execute(&args(&[
        "compile",
        caller.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("caller.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile caller: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let caller_object =
        ObjectFile::from_bytes(&std::fs::read(dir.join("caller.tmo")).unwrap()).unwrap();

    // The library compiles on its own — it IS the declared unit, and its
    // own body has nothing to do with the lying header.
    let lib_object = compile(
        LIAR_LIB,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile lib: {e}"))
    .object;

    for mech in MECHS {
        let exe = link(
            &[caller_object.clone(), lib_object.clone()],
            &[],
            LinkOptions {
                call_mech: mech,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("the {mech} link failed: {e}"))
        .executable;
        let (outcome, _) = run_image(&exe, &[2]);
        assert!(
            matches!(outcome, Outcome::Trapped(Trap::UnmappedRead { .. })),
            "under {mech} a lying `noreturn` must trap with UnmappedRead, not {outcome:?}"
        );
    }
}

// ── `tail-call-no-continuation`, end to end ─────────────────────────────

/// The honest counterpart to [`LIAR_LIB`]: `liar` really never returns
/// (only `stp`, never `retx`/`ret` — this signature has no state
/// parameters, so `ret` is its only way back at all).
const HONEST_NORETURN_LIB: &str = "\
alphabet ab { '_', 'a' }

export routine liar(tape t: ab writes {}) {
  entry state s { [*] -> stop; }
}
";

/// The SAME caller `LIAR_CALLER`/`LIAR_HEADER` pair `a_lying_noreturn_
/// header_traps_instead_of_falling_through` links, but through the real
/// `tmt link` CLI rather than the library entry point, so the RENDERED
/// warning text is observable. The caller's header trusts `liar`'s
/// `noreturn` and omits `then`, which the compiler backs with a
/// synthesized `trap #0` (`an_exit_bearing_tail_call_prints_a_trap`
/// pins the codegen shape) — this test is the LINKER side of the same
/// story: when the linked `liar` actually returns, the trap sits right
/// after a call with no continuation, into a callee that CAN return,
/// which is exactly `tail-call-no-continuation`'s shape.
///
/// The caller's compiled shape is `call liar [t: 0]` immediately
/// followed by `trap #0`, which HERE also happens to be `main`'s last
/// instruction (this fixture has one state) — the trap arm, not the
/// "call is the last instruction" one (the compiler always emits the
/// safety trap, so a real `.tmc` program never reaches the latter, the
/// shape `crates/core/tests/link_checks.rs` exercises directly). The
/// trap arm does not require the trap to end the function — see
/// `a_lying_noreturn_header_prints_the_tail_call_warning_with_a_later_
/// state` below for a caller where it does not.
///
/// Mutation it catches: drop the trap arm in `tail_call_no_continuation`
/// (`crates/core/src/linker/engine.rs`) and this compiler-emitted shape
/// — which never hits the "last instruction" arm — goes silent end to
/// end, not just on a synthetic fixture.
#[test]
fn a_lying_noreturn_header_prints_the_tail_call_warning() {
    let dir = scratch("tail_call_warning_fires");
    let header = write_file(&dir, "liar.tmh", LIAR_HEADER);
    let caller = write_file(&dir, "caller.tmc", LIAR_CALLER);

    let out = execute(&args(&[
        "compile",
        caller.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("caller.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile caller: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);

    let lib_object = compile(LIAR_LIB, CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile lib: {e}"))
        .object;
    std::fs::write(dir.join("lib.tmo"), lib_object.to_bytes()).unwrap();

    let out = execute(&args(&[
        "link",
        dir.join("caller.tmo").to_str().unwrap(),
        dir.join("lib.tmo").to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("caller.tmx").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("link: {e}"));
    assert_eq!(
        out.code, 0,
        "a warning does not fail the link: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains("[tail-call-no-continuation]"),
        "{}",
        out.stderr
    );
}

/// The near miss: `liar` really never returns (`HONEST_NORETURN_LIB`),
/// so the same caller, against the same header, links SILENT — the
/// header was telling the truth, and the linker has nothing to warn
/// about.
///
/// Mutation it catches: warning unconditionally whenever a call is
/// followed only by a trap (never consulting `callee_can_return` at
/// all) and this build, whose `liar` genuinely cannot return, would
/// warn anyway.
#[test]
fn an_honest_noreturn_header_prints_no_tail_call_warning() {
    let dir = scratch("tail_call_warning_silent");
    let header = write_file(&dir, "liar.tmh", LIAR_HEADER);
    let caller = write_file(&dir, "caller.tmc", LIAR_CALLER);

    let out = execute(&args(&[
        "compile",
        caller.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("caller.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile caller: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);

    let lib_object = compile(HONEST_NORETURN_LIB, CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile lib: {e}"))
        .object;
    std::fs::write(dir.join("lib.tmo"), lib_object.to_bytes()).unwrap();

    let out = execute(&args(&[
        "link",
        dir.join("caller.tmo").to_str().unwrap(),
        dir.join("lib.tmo").to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("caller.tmx").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("link: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !out.stderr.contains("tail-call-no-continuation"),
        "an honest `noreturn` callee must not warn: {}",
        out.stderr
    );
}

/// A SECOND caller, with a state AFTER the one that makes the `then`-less
/// call: `go`'s block, the one that ends in the synthesized safety trap,
/// is no longer the function's last block — `after` follows it. This is
/// what a real multi-state `.tmc` program looks like far more often than
/// the single-state `LIAR_CALLER`: a world's states all share ONE
/// function, in source order, and the calling state is laid out last
/// only when it happens to be the last one written.
///
/// `after` is unreachable here on purpose — the fixture only needs a
/// second block after `go`'s, not a program that does anything with
/// it — and the compiler says so as an ordinary warning, not a fatal.
const MULTI_STATE_LIAR_CALLER: &str = "\
alphabet ab { '_', 'a' }

use liar;

machine {
  tape t: ab;
  entry state go { [*] -> call liar(t = t); }
  state after { [*] -> stop; }
}
";

/// The multi-state counterpart to `a_lying_noreturn_header_prints_the_
/// tail_call_warning`: same lying header, same library that actually
/// returns, but `go`'s call is now followed by `trap #0` followed by
/// `after`'s own block — the trap is NOT `main`'s last instruction. The
/// warning must still fire: the check looks only at the instruction
/// right after the call, never at what comes after the trap.
///
/// Mutation it catches: require the trap to be the function's last
/// instruction (`next_is_trap`, `crates/core/src/linker/engine.rs`) and
/// this fixture — whose trap has `after`'s `stp` right behind it — goes
/// silent, even though the single-state fixture above still fires.
#[test]
fn a_lying_noreturn_header_prints_the_tail_call_warning_with_a_later_state() {
    let dir = scratch("tail_call_warning_fires_multi_state");
    let header = write_file(&dir, "liar.tmh", LIAR_HEADER);
    let caller = write_file(&dir, "caller.tmc", MULTI_STATE_LIAR_CALLER);

    let out = execute(&args(&[
        "compile",
        caller.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("caller.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile caller: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);

    let lib_object = compile(LIAR_LIB, CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile lib: {e}"))
        .object;
    std::fs::write(dir.join("lib.tmo"), lib_object.to_bytes()).unwrap();

    let out = execute(&args(&[
        "link",
        dir.join("caller.tmo").to_str().unwrap(),
        dir.join("lib.tmo").to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("caller.tmx").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("link: {e}"));
    assert_eq!(
        out.code, 0,
        "a warning does not fail the link: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains("[tail-call-no-continuation]"),
        "{}",
        out.stderr
    );
}

/// The near miss for the multi-state shape: `liar` really never returns,
/// so the same multi-state caller, against the same header, links
/// SILENT.
///
/// Mutation it catches: warning unconditionally whenever a call is
/// immediately followed by a trap (never consulting `callee_can_return`
/// at all) and this build, whose `liar` genuinely cannot return, would
/// warn anyway.
#[test]
fn an_honest_noreturn_header_prints_no_tail_call_warning_with_a_later_state() {
    let dir = scratch("tail_call_warning_silent_multi_state");
    let header = write_file(&dir, "liar.tmh", LIAR_HEADER);
    let caller = write_file(&dir, "caller.tmc", MULTI_STATE_LIAR_CALLER);

    let out = execute(&args(&[
        "compile",
        caller.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        dir.join("caller.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile caller: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);

    let lib_object = compile(HONEST_NORETURN_LIB, CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile lib: {e}"))
        .object;
    std::fs::write(dir.join("lib.tmo"), lib_object.to_bytes()).unwrap();

    let out = execute(&args(&[
        "link",
        dir.join("caller.tmo").to_str().unwrap(),
        dir.join("lib.tmo").to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("caller.tmx").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("link: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !out.stderr.contains("tail-call-no-continuation"),
        "an honest `noreturn` callee must not warn: {}",
        out.stderr
    );
}
