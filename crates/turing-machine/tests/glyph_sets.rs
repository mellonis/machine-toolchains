//! Named glyph sets: a top-level
//! `set NAME { … }` declaration built from literals, ranges and other
//! sets — namespaced, `export`able and `use`-importable exactly as an
//! alphabet is — expanding in place in an alphabet body and in any
//! contract clause. A set is never a tape type, and a cycle among sets is
//! a compile error rather than a hang.
//!
//! **The central claim** (`a_set_is_a_spelling_of_its_members`): a set name
//! is a SPELLING, not a semantics — a program naming a set compiles to the
//! exact bytes the same program carries with the members written inline.

use std::path::{Path, PathBuf};

use mtc_turing_machine::cli::{CliOutput, execute};
use mtc_turing_machine::compiler::{CompileOptions, compile};

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter so concurrent test processes
/// sharing one `CARGO_TARGET_TMPDIR` never collide.
fn scratch(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// The stable code of the fatal a source compiles to. Codes, not variants:
/// a code is the published contract (docs/tmt/cli.md (compile errors)).
fn code(src: &str) -> &'static str {
    compile(src, CompileOptions::default())
        .expect_err("expected a compile error")
        .kind
        .code()
}

fn compiles(src: &str) {
    compile(src, CompileOptions::default()).unwrap_or_else(|e| panic!("expected success: {e}"));
}

// ---------------------------------------------------------------------------
// The central claim: a set name is a spelling, not a semantics.
// ---------------------------------------------------------------------------

/// One program in two spellings. `digits` reaches BOTH grammar sites an
/// element list has — an alphabet body and an `enters` clause — and is
/// itself built from another set, so a set-in-a-set expands through the
/// same road.
fn digits_program(named: bool) -> String {
    let (decls, body, clause) = if named {
        (
            "set low { '0'..'4' }\nset digits { low, '5'..'9' }\n",
            "'_', digits, '+'",
            "digits",
        )
    } else {
        ("", "'_', '0'..'4', '5'..'9', '+'", "'0'..'4', '5'..'9'")
    };
    format!(
        "\
{decls}alphabet dec {{ {body} }}

routine skip(tape t: dec enters {{ {clause} }}) {{
  entry state s {{
    ['0'..'9'] -> move [>] goto s;
    ['_'] -> return;
  }}
}}

machine {{
  tape main: dec;
  entry state go {{
    ['0'..'9'] -> call skip(t = main) then mark;
    [*] -> stop;
  }}
  state mark {{ [*] -> write ['+'] stop; }}
}}
"
    )
}

/// **The central claim.** Mutation: an expansion that skips a `SetRef` in
/// the alphabet body (the band loses ten glyphs, every index moves), or in
/// the clause (the `enters=` list shrinks), or emits the members out of
/// declaration order — each changes the generated `.tma` and the object,
/// so both byte compares below turn red.
#[test]
fn a_set_is_a_spelling_of_its_members() {
    let inline = compile(&digits_program(false), CompileOptions::default())
        .unwrap_or_else(|e| panic!("inline form: {e}"));
    let named = compile(&digits_program(true), CompileOptions::default())
        .unwrap_or_else(|e| panic!("named form: {e}"));
    assert_eq!(
        named.tma, inline.tma,
        "a set is a spelling — the generated assembly must match byte for byte"
    );
    assert_eq!(
        named.object.to_bytes(),
        inline.object.to_bytes(),
        "the assembled object must match byte for byte"
    );
    assert!(
        named.report.diagnostics.is_empty(),
        "the named form compiles warning-free: {:?}",
        named.report.diagnostics
    );
}

/// A set in an `enters` clause reaches the object already expanded: the
/// `.param` line carries the member glyphs, never the set's name.
/// Mutation: the clause road handing a set name through as though it were
/// a literal glyph — the line below would name `digits`, or the clause
/// would be refused as naming a glyph outside the alphabet.
#[test]
fn a_set_in_a_clause_reaches_the_param_line_as_its_members() {
    let out =
        compile(&digits_program(true), CompileOptions::default()).unwrap_or_else(|e| panic!("{e}"));
    let line = out
        .tma
        .lines()
        .find(|l| l.starts_with(".param t,"))
        .unwrap_or_else(|| panic!("a `.param t` line in:\n{}", out.tma));
    assert_eq!(
        line,
        ".param t, ('_', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '+'), \
         enters=('0', '1', '2', '3', '4', '5', '6', '7', '8', '9')"
    );
}

// ---------------------------------------------------------------------------
// Refusals.
// ---------------------------------------------------------------------------

/// Two sets naming each other, and one naming itself, are a compile error.
/// Mutation: memoising the expansion without a visiting-set check — the
/// walk then never terminates (or overflows the stack) instead of
/// reporting the cycle.
#[test]
fn a_cycle_among_sets_is_an_error_not_a_hang() {
    assert_eq!(
        code("set a { b }\nset b { a }\nalphabet ab { '_', 'x' }\n"),
        "set-cycle"
    );
    assert_eq!(
        code("set a { 'x', a }\nalphabet ab { '_', 'x' }\n"),
        "set-cycle"
    );
}

/// A set names glyphs; it is not an alphabet, so it is never a tape type —
/// neither a machine tape's nor a signature parameter's. It lands exactly
/// where a map or a routine named as a tape type lands: the name resolves
/// to no alphabet. Mutation: widening the tape-type resolution to accept a
/// set because both resolve to glyph lists — both sources then compile.
#[test]
fn a_set_is_never_a_tape_type() {
    assert_eq!(
        code(
            "set digits { '0'..'9' }\nmachine {\n  tape t: digits;\n  entry state s { [*] -> stop; }\n}\n"
        ),
        "unresolved-alphabet"
    );
    assert_eq!(
        code(
            "set digits { '0'..'9' }\nroutine r(tape t: digits) {\n  entry state s { [*] -> return; }\n}\n"
        ),
        "unresolved-alphabet"
    );
}

/// A name nothing declares, in a set position, is its own refusal; a name
/// that resolves to something other than a set is the wrong-kind one.
#[test]
fn an_unknown_or_wrong_kind_set_name_is_refused() {
    assert_eq!(code("alphabet ab { '_', nosuch }\n"), "undefined-set");
    assert_eq!(
        code("set s { nosuch }\nalphabet ab { '_', 'x' }\n"),
        "undefined-set"
    );
    assert_eq!(
        code(
            "alphabet bits { '_', '0' }\nroutine r(tape t: bits writes { nosuch }) {\n  entry state s { [*] -> return; }\n}\n"
        ),
        "undefined-set"
    );
    assert_eq!(
        code("alphabet small { '_', 'x' }\nalphabet big { '_', small }\n"),
        "wrong-target-kind"
    );
}

/// A set that expands to nothing leaves a head-position clause stating no
/// moment and an alphabet with no symbol — the same two refusals the
/// literally empty spellings get. Mutation: checking emptiness on the
/// written elements only, before expansion — both sources then compile,
/// and the `enters` one publishes an empty `enters=` list on the wire.
#[test]
fn an_empty_set_leaves_a_clause_or_an_alphabet_empty() {
    assert_eq!(
        code(
            "set none { }\nalphabet bits { '_', '0' }\nroutine r(tape t: bits enters { none }) {\n  entry state s { [*] -> return; }\n}\n"
        ),
        "empty-head-clause"
    );
    assert_eq!(
        code("set none { }\nalphabet ab { none }\n"),
        "empty-alphabet"
    );
}

/// A set is a SET: a repeat inside one is absorbed. Its members still land
/// in an alphabet under the alphabet's own uniqueness rule, so a member
/// written again beside the set is a duplicate glyph.
#[test]
fn a_repeat_inside_a_set_is_absorbed_but_an_alphabet_still_refuses_one() {
    compiles("set s { '0', '0'..'1' }\nalphabet ab { '_', s }\n");
    assert_eq!(
        code("set s { '0', '1' }\nalphabet ab { '_', '0', s }\n"),
        "duplicate-glyph"
    );
}

/// A set accepts a doc run the way every top-level declaration does.
/// Mutation: leaving `set` out of the words a doc run may bind to — the
/// run is then dangling and the source is refused.
#[test]
fn a_set_carries_a_doc_run() {
    compiles("? The ten decimal digits.\nset digits { '0'..'9' }\nalphabet dec { '_', digits }\n");
}

/// A set shares the one per-scope name space every declaration shares.
#[test]
fn a_set_and_an_alphabet_cannot_share_a_name() {
    assert_eq!(
        code("set ab { 'x' }\nalphabet ab { '_', 'x' }\n"),
        "duplicate-name"
    );
}

// ---------------------------------------------------------------------------
// Namespaces and imports.
// ---------------------------------------------------------------------------

/// A namespaced set resolves by its qualified name, and by `use`. An
/// import whose only use is a set reference is USED — the discriminator is
/// the second source: the very same program with the import deleted fails
/// to resolve the name, so the import is load-bearing, not decorative.
/// Mutation: an import-usage walk that skips set references — the first
/// compile then reports `unused-import`.
#[test]
fn an_import_used_only_by_a_set_reference_is_used() {
    let with_use = "\
namespace inner {
  export set digits { '0'..'9' }
}
use inner::digits;
alphabet dec { '_', digits }
alphabet qualified { '_', inner::digits }
";
    let out = compile(with_use, CompileOptions::default()).unwrap_or_else(|e| panic!("{e}"));
    let unused: Vec<&str> = out
        .report
        .diagnostics
        .iter()
        .filter(|d| d.code == "unused-import")
        .map(|d| d.message.as_str())
        .collect();
    assert!(unused.is_empty(), "{unused:?}");
    let without_use = with_use.replace("use inner::digits;\n", "");
    assert_eq!(code(&without_use), "undefined-set");
}

const LIB_TMC: &str = "\
namespace mylib {
  set low { '0'..'4' }
  export set digits { low, '5'..'9' }
}
";

fn run_interface(path: &Path) -> CliOutput {
    let out = execute(&args(&["interface", path.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("interface {}: {e}", path.display()));
    assert_eq!(out.code, 0, "interface {}: {}", path.display(), out.stderr);
    out
}

/// An exported set reaches the source-arm header as its expanded members
/// (a set it was built from need not itself be exported), and that header
/// reads back under the strict declarations-only reader byte for byte.
/// Mutation: printing a set the strict reader cannot parse, or printing
/// the written elements (the unexported `low` would then be unresolvable).
#[test]
fn an_exported_set_reaches_the_header_and_reads_back() {
    let dir = scratch("glyph_sets_header");
    let path = write(&dir, "lib.tmc", LIB_TMC);
    let out = run_interface(&path);
    assert!(
        out.stdout
            .contains("export set digits { '0', '1', '2', '3', '4', '5', '6', '7', '8', '9' }"),
        "{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("low"),
        "an unexported set is not printed: {}",
        out.stdout
    );
    let header_path = write(&dir, "lib.tmh", &out.stdout);
    let round = run_interface(&header_path);
    assert_eq!(
        round.stdout, out.stdout,
        "the header round-trips byte for byte"
    );
}

/// A consumer imports the set through the header and expands it; without
/// the header the same name is reached through `use` but has nothing
/// behind it, and the refusal names the remedy.
#[test]
fn an_imported_set_expands_in_the_consumer() {
    let dir = scratch("glyph_sets_import");
    let lib_path = write(&dir, "lib.tmc", LIB_TMC);
    let header_path = write(&dir, "lib.tmh", &run_interface(&lib_path).stdout);
    let consumer = "\
use mylib::digits;
alphabet dec { '_', digits }
machine {
  tape main: dec;
  entry state go { ['9'] -> stop; [*] -> move [>] goto go; }
}
";
    let consumer_path = write(&dir, "consumer.tmc", consumer);
    let out_path = dir.join("consumer.tmo");
    let compiled = execute(&args(&[
        "compile",
        consumer_path.to_str().unwrap(),
        "--extern",
        header_path.to_str().unwrap(),
        "-o",
        out_path.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile consumer: {e}"));
    assert_eq!(compiled.code, 0, "{}", compiled.stderr);

    let err = execute(&args(&[
        "compile",
        consumer_path.to_str().unwrap(),
        "-o",
        out_path.to_str().unwrap(),
    ]))
    .expect_err("without the header the consumer must not compile");
    assert!(err.contains("undefined-set"), "{err}");
    assert!(
        err.contains("mylib::digits") && err.contains("declarations were not given"),
        "{err}"
    );
}
