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

use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
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

    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");
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
/// Mutation: allow it (drop the `exits == 0` candidate guard); the call
/// disappears from the `-O1` assembly.
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
/// is the diagnostic.
fn many_state_params(n: usize) -> String {
    let params: Vec<String> = (0..n).map(|i| format!("state p{i}")).collect();
    format!(
        "\
alphabet ab {{ '_', '0' }}

routine wide(tape t: ab, {}) {{
  entry state s {{ [*] -> goto p0; }}
}}

machine {{
  tape d: ab;
  entry state m {{ [*] -> stop; }}
}}
",
        params.join(", ")
    )
}

/// Mutation: narrow the count with a bare `as u8`; the 256-parameter
/// routine then compiles and publishes `exits=0` — silent corruption
/// rather than a diagnostic.
#[test]
fn more_than_255_state_parameters_is_a_typed_error() {
    let err = compile(&many_state_params(256), CompileOptions::default())
        .expect_err("256 state parameters is one too many");
    assert_eq!(err.kind.code(), "too-many-state-params", "{err}");
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
