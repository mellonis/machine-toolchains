//! `.pmc` pretty-printer
//! (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`, "Formatting
//! model"). Thin renderer, same discipline as [`crate::compile`] and
//! [`crate::lint`]: [`format`] returns a `Result` and never prints — the
//! future `cli/fmt.rs` is the only place that renders errors or touches
//! the filesystem.
//!
//! **Scope of this module today (fmt build Task 4): the TRIVIAL printer
//! subset only.** It prints a source-faithful skeleton — headers, braces,
//! indentation, one statement per line, canonical item text, a
//! single-line comma join — and deliberately leaves the following as
//! seams for later tasks (each seam is marked at its printing site
//! below):
//!
//! - **Labels / command-column alignment** (Task 5) — this task assumes
//!   every statement is unlabeled; [`crate::cst::StatementCst::labels`]
//!   and `label_break` are not read yet.
//! - **Comma-group Y / greedy-fill layout** (Task 6) — [`render_items`]
//!   always joins on one line; the author's line breaks and the 80-column
//!   fallback are not implemented yet.
//! - **Comments** (Task 7) — [`crate::cst::TopKind::Comment`],
//!   [`crate::cst::BodyKind::Comment`], and every node's `trailing` field
//!   are read nowhere in this module; comment nodes are silently skipped
//!   (never emitted, never panicked on).
//! - **Blank lines, imports, namespaces, full intra-statement spacing
//!   normalization, and the edge cases** (Task 8) —
//!   [`crate::cst::TopKind::Import`] and [`crate::cst::TopKind::Namespace`]
//!   are silently skipped; `blank_before` is not read anywhere.
//!
//! "Silently skipped" is a deliberate design choice for this task, per the
//! brief: an unhandled node must never panic, but this task also must not
//! half-implement Tasks 5-8's rules. The three-check objective harness
//! (`tests/fmt_programs.rs`) is scoped to a SIMPLE program set the
//! trivial printer fully supports (unlabeled statements, no comments, no
//! namespaces/imports, single-line comma groups) — each later task widens
//! that set as its seam closes.

use crate::compiler::CompileError;
use crate::cst::{BodyItem, BodyKind, CommaItem, Cst, FunctionCst, StatementCst, TopItem, TopKind};
use crate::lexer::{LexMode, lex_with};
use crate::parser::{Builtin, CheckArm, Item, Successor, parse_cst};

/// Spaces per block level (spec "Indentation" — 4 spaces, never tabs).
const INDENT_UNIT: usize = 4;

/// `.pmc` source → canonical text. Lexes `WithComments` (so later tasks'
/// comment handling has trivia to read), builds the lossless CST via
/// [`parse_cst`], and pretty-prints it. A lex/parse error is returned as
/// `Err`, never printed (thin renderer).
pub fn format(source: &str) -> Result<String, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let cst = parse_cst(&tokens)?;
    Ok(print_cst(&cst))
}

fn print_cst(cst: &Cst) -> String {
    let mut out = String::new();
    for item in &cst.items {
        print_top_item(&mut out, item);
    }
    // Edge case (spec "Edge cases"): an empty file still reprints with
    // exactly one final newline. Non-empty output already ends in `\n`
    // from the last printed function, so nothing further is needed there
    // (asserted by the Task-4 tests below).
    if out.is_empty() {
        out.push('\n');
    }
    out
}

fn print_top_item(out: &mut String, item: &TopItem) {
    // Seam (Task 8): `item.blank_before` is not read — no blank lines are
    // ever emitted by this task's printer.
    match &item.kind {
        TopKind::Function(f) => print_function(out, f, 0),
        // Seam (Task 8): namespace blocks. Seam (Task 7): own-line
        // top-level comments. Neither is emitted yet — skip, don't panic.
        TopKind::Namespace(_) | TopKind::Comment(_) => {}
        // Seam (Task 8): `use` imports.
        TopKind::Import(_) => {}
    }
}

/// Header + body + closing brace (spec "Headers and braces"). Used for
/// both top-level and nested functions — a nested [`FunctionCst`] has the
/// same shape, just one indent level deeper and never `exported`.
///
/// **Known CST information-loss gap** (parallel to Task 3's deferred
/// `use`-list-grouping gap): un-namespaced top-level `main` always has
/// `exported: true` (`parser.rs`'s `f.exported = exported ||
/// (ns.is_empty() && f.name == "main")`), whether or not the author
/// literally wrote `export` — the CST has no field distinguishing
/// `main() { … }` from the legal-but-redundant `export main() { … }`
/// (`docs/language.md`). `indent == 0` uniquely identifies this
/// unnamespaced-top-level case (a nested body is always indented; a
/// namespace's contents indent from its own body, never 0 — spec
/// "Indentation"), so this printer omits the redundant `export` there
/// rather than always emitting it — the far more common bare-`main`
/// spelling must not gain a token fmt never removes elsewhere. This
/// narrows the "zero token changes" decision by exactly the one spelling
/// the CST cannot preserve; compiled behavior is identical either way
/// (`main` is always the entry regardless).
fn print_function(out: &mut String, f: &FunctionCst, indent: usize) {
    let pad = " ".repeat(indent);
    out.push_str(&pad);
    if f.exported && !(indent == 0 && f.name == "main") {
        out.push_str("export ");
    }
    out.push_str(&f.name);
    out.push_str("() {\n");
    let body_indent = indent + INDENT_UNIT;
    for body_item in &f.body {
        print_body_item(out, body_item, body_indent);
    }
    out.push_str(&pad);
    out.push_str("}\n");
}

fn print_body_item(out: &mut String, item: &BodyItem, indent: usize) {
    // Seam (Task 8): `item.blank_before` is not read yet.
    match &item.kind {
        BodyKind::Statement(s) => print_statement(out, s, indent),
        BodyKind::Nested(f) => print_function(out, f, indent),
        // Seam (Task 7): own-line body comments.
        BodyKind::Comment(_) => {}
    }
}

/// One statement, one line: comma-joined items + `;` (spec "Statements").
///
/// Seam (Task 5): this assumes an UNLABELED statement — `s.labels` and
/// `s.label_break` are not read. A labeled body is out of this task's
/// SIMPLE test set.
///
/// Seam (Task 7): `s.trailing` (a same-line trailing comment) is not read.
fn print_statement(out: &mut String, s: &StatementCst, indent: usize) {
    out.push_str(&" ".repeat(indent));
    out.push_str(&render_items(&s.items));
    out.push_str(";\n");
}

/// Seam (Task 6): always a single-line comma join — the Y/greedy-fill
/// layout (respect author line breaks, 80-column overflow) is not
/// implemented yet. Also does not read [`CommaItem::leading`] (Task 7).
fn render_items(items: &[CommaItem]) -> String {
    items
        .iter()
        .map(|ci| render_item(&ci.item))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Canonical item text (spec "Intra-statement token spacing"). This task
/// gets the canonical (tight) forms right; the full spacing table and
/// spaced-form normalization (`1 : right` -> `1: right`) is Task 8's — but
/// since `parse_cst` only ever hands back the parsed VALUE (never the
/// author's original spacing), this renderer already produces the
/// canonical form for every item shape it covers.
pub(crate) fn render_item(item: &Item) -> String {
    match item {
        Item::Builtin { which, succ, .. } => {
            format!(
                "{}{}",
                builtin_name(*which),
                render_builtin_successor(*succ)
            )
        }
        Item::Debugger { .. } => "debugger".to_string(),
        Item::Call { name, succ, .. } => format!("@{name}({})", render_successor(*succ)),
        Item::Check { marked, blank, .. } => {
            format!(
                "check({}, {})",
                render_check_arm(*marked),
                render_check_arm(*blank)
            )
        }
        Item::Halt { .. } => "halt".to_string(),
        Item::Goto { label, .. } => format!("goto {label}"),
    }
}

fn builtin_name(which: Builtin) -> &'static str {
    match which {
        Builtin::Left => "left",
        Builtin::Right => "right",
        Builtin::Mark => "mark",
        Builtin::Unmark => "unmark",
    }
}

/// A builtin's successor parens are OMITTED for `FallThrough` (`left`, not
/// `left()` — empty builtin parens are a grammar-0.2 syntax error and can
/// never occur); a call's parens are always present (mandatory, per the
/// grammar), so [`Item::Call`] renders through [`render_successor`]
/// directly instead.
fn render_builtin_successor(succ: Successor) -> String {
    match succ {
        Successor::FallThrough => String::new(),
        _ => format!("({})", render_successor(succ)),
    }
}

fn render_successor(succ: Successor) -> String {
    match succ {
        Successor::FallThrough => String::new(),
        Successor::Label(n) => n.to_string(),
        Successor::Return => "!".to_string(),
    }
}

fn render_check_arm(arm: CheckArm) -> String {
    match arm {
        CheckArm::Label(n) => n.to_string(),
        CheckArm::Return => "!".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CompileErrorKind;

    #[test]
    fn empty_file_is_one_final_newline() {
        assert_eq!(format("").unwrap(), "\n");
    }

    #[test]
    fn single_unlabeled_statement() {
        assert_eq!(
            format("main() { right; }").unwrap(),
            "main() {\n    right;\n}\n"
        );
    }

    #[test]
    fn exported_function_header() {
        assert_eq!(
            format("export f() { left; }").unwrap(),
            "export f() {\n    left;\n}\n"
        );
    }

    #[test]
    fn comma_group_joins_on_one_line() {
        assert_eq!(
            format("f() { left, right; }").unwrap(),
            "f() {\n    left, right;\n}\n"
        );
    }

    #[test]
    fn multiple_top_level_functions_and_a_call() {
        assert_eq!(
            format("f() { right; @g(); } g() { left; }").unwrap(),
            "f() {\n    right;\n    @g();\n}\ng() {\n    left;\n}\n"
        );
    }

    #[test]
    fn renders_every_trivial_item_shape() {
        assert_eq!(
            format("f() { right(5); mark(!); @g(3); @h(!); check(1, !); goto 2; halt; debugger; }")
                .unwrap(),
            "f() {\n    right(5);\n    mark(!);\n    @g(3);\n    @h(!);\n    check(1, !);\n    goto 2;\n    halt;\n    debugger;\n}\n"
        );
    }

    #[test]
    fn empty_function_body_has_no_blank_line() {
        assert_eq!(format("f() { }").unwrap(), "f() {\n}\n");
    }

    #[test]
    fn parse_error_returns_err() {
        let e = format("f() { 1: right; 1: left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateLabel(1)));
    }

    #[test]
    fn idempotent_on_supported_shapes() {
        for src in [
            "main() { right; }",
            "f() { right; @g(); } g() { left; }",
            "f() { left, right, mark; }",
        ] {
            let once = format(src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }
}
