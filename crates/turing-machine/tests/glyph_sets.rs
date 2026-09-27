//! Named glyph sets: a top-level
//! `set NAME { … }` declaration built from literals, ranges and other
//! sets — namespaced, `export`able and `use`-importable exactly as an
//! alphabet is — expanding in place in an alphabet body and in any
//! contract clause, and matching its members in a pattern cell. A set is
//! never a tape type, and a cycle among sets is a compile error rather
//! than a hang.
//!
//! **The central claim** (`a_set_is_a_spelling_of_its_members`): a set name
//! is a SPELLING, not a semantics — a program naming a set compiles to the
//! exact bytes the same program carries with the members written inline.

use std::path::{Path, PathBuf};

use mtc_turing_machine::cli::{CliOutput, execute};
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::fmt::format as fmt_format;
use mtc_turing_machine::ir::IrCell;
use mtc_turing_machine::lint::{LintOptions, lint};
use mtc_turing_machine::optimizer::OptLevel;

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

/// The cycle error names the whole chain, in reference order, back to the
/// set it started from — for a mutual pair and for a self-reference alike.
/// Mutation: reporting only the set whose reference closes the cycle — the
/// two-set message then names `a` alone.
#[test]
fn a_cycle_names_its_whole_chain() {
    let message = |src: &str| {
        compile(src, CompileOptions::default())
            .expect_err("expected a compile error")
            .kind
            .to_string()
    };
    assert_eq!(
        message("set a { b }\nset b { a }\n"),
        "glyph sets form a cycle: `a` -> `b` -> `a` — a set built from sets must bottom \
         out in literals and ranges"
    );
    assert_eq!(
        message("set a { 'x', a }\n"),
        "glyph sets form a cycle: `a` -> `a` — a set built from sets must bottom out in \
         literals and ranges"
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

/// A member reached twice — here `d`'s two glyphs, through both `b` and
/// `c` — keeps the position of its FIRST occurrence, and that order is the
/// alphabet's own symbol order: `a` expands to `x, y, p, q`, so the band
/// is `'_', 'x', 'y', 'p', 'q'`. Mutation: absorbing a repeat at its LAST
/// occurrence instead — the band becomes `'_', 'p', 'q', 'x', 'y'`, and
/// every symbol index after the blank moves.
#[test]
fn a_repeated_member_keeps_its_first_position() {
    let src = "\
set d { 'x', 'y' }
set b { d, 'p' }
set c { 'q', d }
set a { b, c }
alphabet ab { '_', a }
routine r(tape t: ab) {
  entry state s { [*] -> return; }
}
";
    let out = compile(src, CompileOptions::default()).unwrap_or_else(|e| panic!("{e}"));
    let line = out
        .tma
        .lines()
        .find(|l| l.starts_with(".param t,"))
        .unwrap_or_else(|| panic!("a `.param t` line in:\n{}", out.tma));
    // The band's glyph list only — the line's suffixes (the inferred
    // `opaque` bit here) are other facts. No glyph here holds a `)`.
    let band = &line[..=line.find(')').expect("the glyph list closes")];
    assert_eq!(band, ".param t, ('_', 'x', 'y', 'p', 'q')");
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

// ---------------------------------------------------------------------------
// A set in a pattern cell.
// ---------------------------------------------------------------------------

/// Compile at `-O0`, where the generated assembly is the expansion's own
/// rows (no pass merges or reorders them), and return it.
fn tma_o0(src: &str) -> String {
    let options = CompileOptions {
        opt_level: OptLevel::O0,
        ..CompileOptions::default()
    };
    compile(src, options)
        .unwrap_or_else(|e| panic!("expected success: {e}\n{src}"))
        .tma
}

/// A one-tape machine over `alphabet` whose entry state carries `rules`
/// and then a moving catch-all, with `decls` above it.
fn one_tape_program(decls: &str, alphabet: &str, rules: &[String]) -> String {
    let body: String = rules.iter().map(|r| format!("    {r}\n")).collect();
    format!(
        "{decls}alphabet tape_ab {{ {alphabet} }}\nmachine {{\n  tape main: tape_ab;\n  entry state go {{\n{body}    [*] -> move [>] goto go;\n  }}\n}}\n"
    )
}

/// **Row equality.** `[odd] -> …` expands to one row per member, in the
/// set's own member order, exactly the rows the members written out one
/// rule each produce. The expected rows are DERIVED here from the set's
/// written members, never read back from a run; the generated assembly at
/// `-O0` is compared because it carries both the match rows and, through
/// the dispatch targets, the order the expansion emitted them in.
/// Mutation: expanding a set cell to ONE row (its first member, say) —
/// the named form then lacks two rows and the assembly differs.
#[test]
fn a_set_cell_is_one_row_per_member() {
    let members = ["7", "3", "1"];
    let decl = format!(
        "set odd {{ {} }}\n",
        members
            .iter()
            .map(|m| format!("'{m}'"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let alphabet = "'_', '1', '3', '7', 'x'";
    let named = one_tape_program(&decl, alphabet, &["[odd] -> write ['x'] stop;".to_string()]);
    let literal_rules: Vec<String> = members
        .iter()
        .map(|m| format!("['{m}'] -> write ['x'] stop;"))
        .collect();
    let literal = one_tape_program(&decl, alphabet, &literal_rules);
    assert_eq!(tma_o0(&named), tma_o0(&literal));
}

/// **The `as` form.** `[some as d] -> write [{d+1}]` binds each member in
/// turn and folds per row, exactly as a numeric range does: the rows equal
/// the members written out with their folded writes, derived here.
/// Mutation: dropping the binding on the set arm — `{d+1}` then names no
/// binding and the named form is refused.
#[test]
fn a_bound_set_cell_folds_per_row() {
    let members: [u32; 4] = [5, 0, 1, 2];
    let decl = format!(
        "set some {{ {} }}\n",
        members
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    let alphabet = "'_', 0..6";
    let named = one_tape_program(
        &decl,
        alphabet,
        &["[some as d] -> write [{d+1}] stop;".to_string()],
    );
    let literal_rules: Vec<String> = members
        .iter()
        .map(|m| format!("[{m}] -> write [{}] stop;", m + 1))
        .collect();
    let literal = one_tape_program(&decl, alphabet, &literal_rules);
    assert_eq!(tma_o0(&named), tma_o0(&literal));
}

/// A set-bound name takes arithmetic only when EVERY member is a number —
/// the rule a range's binding follows, where a quoted range is glyph-bound.
/// A quoted member anywhere in the set, first or not, makes a fold the
/// `char-arithmetic` refusal; a bare `{c}` passthrough stays legal on any
/// set. Mutation: deciding foldability on the set's FIRST member only (the
/// way a range reads its low end) — the mixed set below, whose first member
/// is a number, then reaches fold evaluation with a glyph in hand.
#[test]
fn a_set_binding_folds_only_when_every_member_is_a_number() {
    let rules = |rule: &str| vec![rule.to_string()];
    assert_eq!(
        code(&one_tape_program(
            "set letters { 'a', 'b' }\n",
            "'_', 'a', 'b', 'c'",
            &rules("[letters as c] -> write [{c+1}] stop;"),
        )),
        "char-arithmetic"
    );
    assert_eq!(
        code(&one_tape_program(
            "set mixed { 1, 'a' }\n",
            "'_', 'a', 1, 2",
            &rules("[mixed as c] -> write [{c+1}] stop;"),
        )),
        "char-arithmetic"
    );
    compiles(&one_tape_program(
        "set letters { 'a', 'b' }\n",
        "'_', 'a', 'b', 'c'",
        &rules("[letters as c] -> write [{c}] stop;"),
    ));
}

/// A set names what a pattern MATCHES; a write cell still takes one
/// symbol, so a set name there stays the grammar's own refusal.
/// Mutation: admitting an identifier in `write_cell` as a set reference —
/// the source then gets past the parser.
#[test]
fn a_set_in_a_write_cell_is_refused() {
    assert_eq!(
        code(&one_tape_program(
            "set odd { '1', '3' }\n",
            "'_', '1', '3'",
            &["[*] -> write [odd] stop;".to_string()],
        )),
        "unexpected-token"
    );
}

/// **The agreement fixture.** One set, expanded once through an alphabet
/// body and once through a pattern cell, lands in the same glyph order.
/// The set is written out of order and through a range; the pattern cell
/// sits on a tape whose alphabet lists the same glyphs in yet another
/// order. The pattern-cell order is read off the lowered IR at `-O0` — the
/// expansion's rows in the order it produced them, before codegen bands
/// and sorts a state's match rows by symbol index — and the alphabet-body
/// order off the other tape's own band.
/// Mutation: a set arm that walks the TAPE's alphabet and keeps the
/// members (tape order `5, 3, 2, 1, 9`), rather than walking the set's
/// members (set order `9, 1, 2, 3, 5`).
#[test]
fn a_set_expands_in_one_order_in_an_alphabet_and_in_a_pattern() {
    let src = "\
set s { '9', '1'..'3', '5' }
alphabet band { '_', s }
alphabet other { '_', '5', '3', '2', '1', '9' }
machine {
  tape a: band;
  tape b: other;
  entry state go {
    [*, s] -> stop;
    [*, *] -> stop;
  }
}
";
    let options = CompileOptions {
        opt_level: OptLevel::O0,
        ..CompileOptions::default()
    };
    let out = compile(src, options).unwrap_or_else(|e| panic!("{e}"));
    let world = out
        .ir
        .worlds
        .iter()
        .find(|w| w.name == "main")
        .expect("the machine world");
    let via_alphabet: Vec<String> = world.tapes[0].glyphs[1..].to_vec();
    let state = world
        .states
        .iter()
        .find(|s| s.name == "go")
        .expect("state go");
    let via_pattern: Vec<String> = state
        .rules
        .iter()
        .filter_map(|r| match r.pattern[1] {
            IrCell::Index { index } => Some(world.tapes[1].glyphs[index as usize].clone()),
            IrCell::Wildcard => None,
        })
        .collect();
    assert_eq!(via_alphabet, ["9", "1", "2", "3", "5"]);
    assert_eq!(via_pattern, via_alphabet);
}

/// A pattern cell's set name resolves the way every other set reference
/// does: nothing by that name is `undefined-set`, a name of another kind
/// is `wrong-target-kind`. Mutation: resolving the cell's name as a glyph
/// label (it would then match nothing and compile as a dead rule).
#[test]
fn an_unknown_or_wrong_kind_set_in_a_pattern_cell_is_refused() {
    let rules = |rule: &str| vec![rule.to_string()];
    assert_eq!(
        code(&one_tape_program(
            "",
            "'_', 'a'",
            &rules("[nosuch] -> stop;")
        )),
        "undefined-set"
    );
    assert_eq!(
        code(&one_tape_program(
            "",
            "'_', 'a'",
            &rules("[tape_ab] -> stop;")
        )),
        "wrong-target-kind"
    );
}

/// An import whose only use is a pattern cell's set name is USED — and
/// load-bearing: the same program without it does not resolve the name.
/// Mutation: an import-usage walk that visits element lists but not
/// pattern cells — the first compile then warns `unused-import`.
#[test]
fn an_import_used_only_by_a_pattern_cell_is_used() {
    let with_use = "\
namespace inner {
  export set digits { '0'..'9' }
}
use inner::digits;
alphabet dec { '_', '0'..'9' }
machine {
  tape main: dec;
  entry state go { [digits] -> stop; [*] -> move [>] goto go; }
}
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

/// Every coverage reader sees a set cell's MEMBERS — not the whole
/// alphabet a wildcard would stand for, not nothing. A state matching only
/// a set that misses the blank may trap; one matching a set that covers
/// the alphabet may not. Mutation: a set cell read as a wildcard (the first
/// finding disappears), as unresolvable (the reader declines, and the first
/// finding disappears too), or as matching nothing (the second state is
/// reported).
#[test]
fn coverage_reads_a_set_cells_members() {
    let traps = |set: &str| {
        let src = format!(
            "set s {{ {set} }}\nalphabet ab {{ '_', 'a', 'b' }}\nmachine {{\n  tape t: ab;\n  entry state go {{ [s] -> stop; }}\n}}\n"
        );
        let options = LintOptions {
            warn: vec!["state-may-trap".to_string()],
            ..LintOptions::default()
        };
        lint(&src, options)
            .unwrap_or_else(|e| panic!("{e}"))
            .diagnostics
            .iter()
            .filter(|d| d.code == "state-may-trap")
            .count()
    };
    assert_eq!(traps("'a', 'b'"), 1);
    assert_eq!(traps("'_', 'a', 'b'"), 0);
}

/// The lint's binding product counts a set cell's members present on the
/// tape: two cells of a seventeen-member set are 289 rows. Mutation:
/// counting a set cell as one row, the way a wildcard counts.
#[test]
fn the_binding_product_counts_a_set_cells_members() {
    let src = "\
set big { 'a'..'q' }
alphabet ab { '_', 'a'..'q' }
machine {
  tape t: ab;
  tape u: ab;
  entry state go { [big, big] -> stop; [*, *] -> stop; }
}
";
    let found = lint(src, LintOptions::default())
        .unwrap_or_else(|e| panic!("{e}"))
        .diagnostics
        .iter()
        .filter(|d| d.code == "binding-product-threshold")
        .count();
    assert_eq!(found, 1);
}

/// `tmt fmt` prints a set cell's name back as written — qualified or
/// bare, bound or not. Mutation: printing the cell by its members, or
/// dropping the `as` binding.
#[test]
fn fmt_prints_a_set_cell_by_its_name() {
    let src = "\
namespace n {
  export set s { 'a' }
}
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go {
    [n::s   as  v] -> write [{v}] stop;
    [ *] -> stop;
  }
}
";
    let out = fmt_format(src).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(out.contains("[n::s as v] -> write [{v}] stop;"), "{out}");
}

const GRAPH_LIB_TMC: &str = "\
namespace lib {
  export alphabet dec { '_', '0'..'9' }
  set evens { '0', '2', '4', '6', '8' }
  export graph skip(tape t: dec, state done) {
    entry state s {
      [evens] -> move [>] goto s;
      [*] -> done;
    }
  }
}
";

/// A printed graph body naming a set prints the name, and so the header
/// prints the set too — as a plain `set`, since this one is not exported
/// on its own — so the header reads back byte for byte and a consumer can
/// graft the graph through it. Mutation: a header that keeps printing only
/// exported sets — reading it back then fails on `evens` (`undefined-set`).
#[test]
fn a_graph_body_naming_a_set_reaches_the_header_and_grafts() {
    let dir = scratch("glyph_sets_graph_header");
    let lib_path = write(&dir, "lib.tmc", GRAPH_LIB_TMC);
    let header = run_interface(&lib_path).stdout;
    assert!(
        header.contains("  set evens { '0', '2', '4', '6', '8' }")
            && header.contains("[evens] -> move [>] goto s;"),
        "{header}"
    );
    let header_path = write(&dir, "lib.tmh", &header);
    assert_eq!(run_interface(&header_path).stdout, header);

    let consumer = "\
use lib::dec;
machine {
  tape main: dec;
  entry graft lib::skip(t = main, done = fin) as i;
  state fin { [*] -> stop; }
}
";
    assert_consumer_compiles(&dir, consumer, &header_path);
}

/// Compile `consumer` against the header at `header_path`.
fn assert_consumer_compiles(dir: &Path, consumer: &str, header_path: &Path) {
    let consumer_path = write(dir, "consumer.tmc", consumer);
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
}

/// A set whose members are numbers prints them bare in a header, so a
/// printed graph body folding over a binding on it reads back as the same
/// fold. Mutation: printing a set's members the way an alphabet's are —
/// single digits quoted — so the header's own set is glyph-bound on
/// read-back and the fold is refused (`char-arithmetic`).
#[test]
fn a_number_sets_members_stay_numbers_through_a_header() {
    let lib = "\
namespace lib {
  export alphabet num { '_', 0..3 }
  export set low { 0..2 }
  export graph inc(tape t: num, state done) {
    entry state s {
      [low as d] -> write [{d+1}] goto done;
      [*] -> goto done;
    }
  }
}
";
    let dir = scratch("glyph_sets_number_header");
    let lib_path = write(&dir, "lib.tmc", lib);
    let header = run_interface(&lib_path).stdout;
    assert!(header.contains("export set low { 0, 1, 2 }"), "{header}");
    let header_path = write(&dir, "lib.tmh", &header);
    assert_eq!(run_interface(&header_path).stdout, header);
    let consumer = "\
use lib::num;
machine {
  tape main: num;
  entry graft lib::inc(t = main, done = fin) as i;
  state fin { [*] -> stop; }
}
";
    assert_consumer_compiles(&dir, consumer, &header_path);
}

/// A printed graph body's bare set name reached through a `use` keeps
/// that `use` line in the header. Mutation: an import-usage scan over a
/// printed graph body that skips pattern cells — the `use` line drops and
/// the header no longer resolves `evens` on read-back.
#[test]
fn a_graph_bodys_imported_set_keeps_its_use_line() {
    let lib = "\
namespace sets {
  export set evens { '0', '2' }
}
namespace lib {
  use sets::evens;
  export alphabet dec { '_', '0'..'3' }
  export graph skip(tape t: dec, state done) {
    entry state s {
      [evens] -> move [>] goto s;
      [*] -> goto done;
    }
  }
}
";
    let dir = scratch("glyph_sets_graph_use");
    let lib_path = write(&dir, "lib.tmc", lib);
    let header = run_interface(&lib_path).stdout;
    assert!(header.contains("use sets::evens;"), "{header}");
    let header_path = write(&dir, "lib.tmh", &header);
    assert_eq!(run_interface(&header_path).stdout, header);
}

/// A routine over `ab` whose one tape declares `clause`, with `decls`
/// above it and `rules` in its entry state.
fn clause_routine(decls: &str, clause: &str, rules: &str) -> String {
    format!(
        "{decls}alphabet ab {{ '_', 'a', 'b' }}\nroutine r(tape t: ab {clause}) {{\n  entry state s {{ {rules} }}\n}}\n"
    )
}

/// The static `leaves` check reads a set cell's members: a ONE-member set
/// pins the glyph a returning row leaves on the tape, exactly as a
/// one-glyph pattern does, so a member outside the declared clause is
/// refused; a two-member set does not pin it, so the check leaves the row
/// to the runtime check, as it does a range. Mutation: `cell_labels`
/// answering nothing for a set cell — the check then declines and the
/// first source compiles.
#[test]
fn the_leaves_check_reads_a_set_cells_members() {
    assert_eq!(
        code(&clause_routine(
            "set one { 'b' }\n",
            "leaves { 'a' }",
            "[one] -> return; [*] -> write ['a'] return;",
        )),
        "leaves-outside-contract"
    );
    compiles(&clause_routine(
        "set two { 'a', 'b' }\n",
        "leaves { 'a' }",
        "[two] -> return; [*] -> write ['a'] return;",
    ));
}

/// The static `enters` check reads a set cell's members: an entry state
/// matching a set that covers every declared `enters` glyph accepts the
/// clause; one whose set misses a declared glyph is refused. Mutation:
/// `cell_labels` answering nothing for a set cell — the check then
/// declines and the second source compiles.
#[test]
fn the_enters_check_reads_a_set_cells_members() {
    compiles(&clause_routine(
        "set both { 'a', 'b' }\n",
        "enters { 'a', 'b' }",
        "[both] -> return;",
    ));
    assert_eq!(
        code(&clause_routine(
            "set one { 'a' }\n",
            "enters { 'a', 'b' }",
            "[one] -> return;",
        )),
        "enters-not-accepted"
    );
}

/// A library graph grafted through its header matches the members of the
/// set its DECLARING unit names, even when the consumer declares a set of
/// the same name with other members: the library's `pick` is `'a'`, the
/// consumer's `pick` is `'b'`, and the spliced match row is `'a'`'s
/// (index 1 on `ab`), never `'b'`'s (index 2). Mutation: filling a
/// grafted graph's set cells from the consumer's own sets — the row then
/// matches index 2.
#[test]
fn a_grafted_graph_matches_its_declaring_units_set() {
    let lib = "\
namespace lib {
  export alphabet ab { '_', 'a', 'b', 'x' }
  set pick { 'a' }
  export graph mark(tape t: ab, state done) {
    entry state s {
      [pick] -> write ['x'] goto done;
      [*] -> goto done;
    }
  }
}
";
    let consumer = "\
use lib::ab;
set pick { 'b' }
alphabet unused { '_', pick }
machine {
  tape main: ab;
  entry graft lib::mark(t = main, done = fin) as i;
  state fin { [*] -> stop; }
}
";
    let dir = scratch("glyph_sets_graft_scope");
    let lib_path = write(&dir, "lib.tmc", lib);
    let header_path = write(&dir, "lib.tmh", &run_interface(&lib_path).stdout);
    let consumer_path = write(&dir, "consumer.tmc", consumer);
    let tma_path = dir.join("consumer.tma");
    let compiled = execute(&args(&[
        "compile",
        "-O0",
        "-S",
        consumer_path.to_str().unwrap(),
        "--extern",
        header_path.to_str().unwrap(),
        "-o",
        tma_path.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile consumer: {e}"));
    assert_eq!(compiled.code, 0, "{}", compiled.stderr);
    let tma = std::fs::read_to_string(&tma_path).unwrap();
    let rows: Vec<&str> = tma
        .lines()
        .filter(|l| l.contains(".row") && !l.contains("[*]"))
        .map(str::trim)
        .collect();
    assert_eq!(rows, ["T0:     .row    [1]"], "{tma}");
}
