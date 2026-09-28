//! The `never writes { … }` contract clause on a signature tape parameter
//! (docs/tmt/language.md (contract clauses)): the symbols a body never
//! writes, subtracted from its `writes` set (or from the whole alphabet when
//! there is no `writes` clause). Two tokens — the reserved word `never`, then
//! `writes` — in the canonical slot between `writes` and `enters`. The clause
//! was spelled `preserves` in `.tmc` 0.1; that spelling is no longer a
//! keyword, and a program still writing it gets a parse error that names
//! the new spelling.
//!
//! Each test states the mutation it catches. Nothing here reaches the wire:
//! a header or an object publishes only the effective `writes` set, so the
//! clause's spelling cannot move either (`interface_emission.rs` pins the
//! compiled stdlib object, `stdlib_header.rs` pins `std.tmh`).

use mtc_core::formats::object::{ObjectFile, RoutineInterface, SymbolDef};
use mtc_turing_machine::CompileErrorKind;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::fmt::format;
use mtc_turing_machine::lint::{LintOptions, lint};

/// A routine over `bits` whose one tape parameter carries `clauses` after
/// its alphabet, exported so its interface is published, plus a machine so
/// the source is a whole program. The body writes only `'0'`.
fn wrap(clauses: &str) -> String {
    format!(
        "alphabet bits {{ '_', '0', '1' }}\n\
         export routine mark(tape t: bits{clauses}) {{\n  \
         entry state s {{ [*] -> write ['0'] return; }}\n\
         }}\n\
         machine {{\n  \
         tape m: bits;\n  \
         entry state go {{ [*] -> call mark(t = m) then fin; }}\n  \
         state fin      {{ [*] -> stop; }}\n\
         }}\n"
    )
}

fn routine_interface<'a>(object: &'a ObjectFile, name: &str) -> &'a RoutineInterface {
    let symbol = object
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol named `{name}` in {:?}", object.symbols));
    let blob = match symbol.def {
        SymbolDef::Defined { blob } | SymbolDef::Local { blob } => blob,
        SymbolDef::External => panic!("`{name}` is external"),
    };
    &object
        .interface
        .as_ref()
        .expect("object carries an interface section")
        .routines[blob as usize]
}

/// The effective `writes` set `mark` publishes for its one tape.
fn effective(src: &str) -> Vec<String> {
    let object = compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("expected a clean compile: {e}\n{src}"))
        .object;
    routine_interface(&object, "mark").writes[0].clone()
}

fn err(src: &str) -> mtc_turing_machine::CompileError {
    compile(src, CompileOptions::default()).expect_err("expected a compile error")
}

/// Mutation: the parser not recognising the two-token form (it would stop
/// at `never` and expect `,` or `)`), or fmt printing the clause as one
/// token — the source is already canonical, so any respelling breaks the
/// byte-identity.
#[test]
fn writes_then_never_writes_compiles_and_formats_back_unchanged() {
    let src = wrap(" writes { '0' } never writes { '1' }");
    assert_eq!(effective(&src), vec!["0".to_string()]);
    assert_eq!(format(&src).expect("formats"), src, "fmt is byte-identical");
}

/// Mutation: `never writes` parsed but dropped (never reaching the resolved
/// tape) — with no `writes` clause the effective set would then be the
/// whole alphabet, blank included.
#[test]
fn a_never_writes_only_clause_subtracts_from_the_whole_alphabet() {
    let src = wrap(" never writes { '_' }");
    assert_eq!(effective(&src), vec!["0".to_string(), "1".to_string()]);
}

/// Mutation: the order check not knowing the new spelling — it would name
/// the old word, or accept the reversed pair.
#[test]
fn never_writes_before_writes_is_a_clause_order_error() {
    let e = err(&wrap(" never writes { '1' } writes { '0' }"));
    assert_eq!(e.kind.code(), "contract-clause-order");
    assert!(
        matches!(&e.kind, CompileErrorKind::ContractClauseOrder { what, before }
            if *what == "writes" && *before == "never writes"),
        "{:?}",
        e.kind
    );
    assert_eq!(
        e.kind.to_string(),
        "`writes` must come before `never writes` (canonical order: `writes` < `never writes` < `enters` < `leaves`)"
    );
}

/// Mutation: `never writes` after `enters` accepted, or named by the old
/// word.
#[test]
fn never_writes_after_enters_is_a_clause_order_error() {
    let e = err(&wrap(" enters { '0' } never writes { '1' }"));
    assert!(
        matches!(&e.kind, CompileErrorKind::ContractClauseOrder { what, before }
            if *what == "never writes" && *before == "enters"),
        "{:?}",
        e.kind
    );
}

/// Mutation: the duplicate check keyed on the old word.
#[test]
fn a_second_never_writes_clause_is_a_duplicate() {
    let e = err(&wrap(" never writes { '1' } never writes { '_' }"));
    assert_eq!(e.kind.code(), "duplicate-contract-clause");
    assert_eq!(e.kind.to_string(), "duplicate `never writes` clause");
}

/// Mutation: `never` accepted bare, as a one-token clause keyword. The
/// message must name `writes` as what `never` needs — a code-only check
/// could not tell this error from the generic `,` or `)` one.
#[test]
fn bare_never_is_an_error_naming_writes() {
    for clause in [" never { '1' }", " never enters { '1' }"] {
        let e = err(&wrap(clause));
        assert_eq!(e.kind.code(), "unexpected-token", "{clause}");
        let msg = e.kind.to_string();
        assert!(
            msg.starts_with("expected `writes` after `never`"),
            "{clause}: {msg}"
        );
    }
}

/// Mutation: `preserves` still accepted as the clause keyword, or refused
/// with a bare "expected `,` or `)`" that gives no hint of the rename.
#[test]
fn the_old_preserves_spelling_is_an_error_naming_the_rename() {
    let src = wrap(" preserves { '1' }");
    let e = err(&src);
    assert_eq!(e.kind.code(), "unexpected-token");
    let msg = e.kind.to_string();
    assert!(msg.contains("`never writes`"), "{msg}");
    assert!(msg.contains("`preserves`"), "{msg}");
    // Reported at the `preserves` token itself.
    let line = src.lines().nth(e.span.start.line as usize - 1).unwrap();
    let col = e.span.start.col as usize - 1;
    assert!(
        line.chars()
            .skip(col)
            .collect::<String>()
            .starts_with("preserves"),
        "{line} @ {col}"
    );
}

/// Mutation: `never` left out of the reserved words — it would then name a
/// state or a tape.
#[test]
fn never_is_reserved_as_a_name() {
    for src in [
        "alphabet bits { '_' }\nmachine { tape t: bits; entry state never { [*] -> stop; } }\n",
        "alphabet bits { '_' }\nmachine { tape never: bits; entry state s { [*] -> stop; } }\n",
    ] {
        assert_eq!(err(src).kind.code(), "reserved-name", "{src}");
    }
}

/// Near miss: `preserves` is no longer reserved, so it names things again.
/// Mutation: `preserves` kept in the reserved words beside `never`.
#[test]
fn preserves_is_an_ordinary_name_again() {
    let src =
        "alphabet bits { '_' }\nmachine { tape preserves: bits; entry state s { [*] -> stop; } }\n";
    compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("`preserves` must name a tape: {e}"));
}

/// Mutation: the overlap lint, or its message, keyed on the old word; its
/// fix must still remove the `writes`-side reference.
#[test]
fn the_overlap_lint_names_never_writes_and_its_fix_still_applies() {
    let src = wrap(" writes { '0', '1' } never writes { '1' }");
    let report = lint(&src, LintOptions::default()).expect("lints");
    let found: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "contract-clause-overlap")
        .collect();
    assert_eq!(found.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(
        found[0].message,
        "'1' is in both `writes` and `never writes`; `never writes` wins, so the `writes` entry is inert"
    );
    let fix = found[0].fix.as_ref().expect("a single glyph gets the fix");
    assert_eq!(fix.edits.len(), 1);
    let edit = &fix.edits[0];
    // One-line edit on the routine's line: slice it out by column.
    assert_eq!(edit.span.start.line, edit.span.end.line);
    let line = src.lines().nth(edit.span.start.line as usize - 1).unwrap();
    let chars: Vec<char> = line.chars().collect();
    let removed: String = chars[edit.span.start.col as usize - 1..edit.span.end.col as usize - 1]
        .iter()
        .collect();
    assert_eq!(removed, ", '1'", "the writes-side element and its comma");
    let fixed_line: String = chars[..edit.span.start.col as usize - 1]
        .iter()
        .chain(chars[edit.span.end.col as usize - 1..].iter())
        .collect();
    assert!(
        fixed_line.contains("writes { '0' } never writes { '1' }"),
        "{fixed_line}"
    );
}
