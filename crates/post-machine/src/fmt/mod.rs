//! `.pmc` pretty-printer
//! (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`, "Formatting
//! model"). Thin renderer, same discipline as [`crate::compile`] and
//! [`crate::lint`]: [`format`] returns a `Result` and never prints — the
//! future `cli/fmt.rs` is the only place that renders errors or touches
//! the filesystem.
//!
//! **Scope of this module today (fmt build Tasks 4-5): the TRIVIAL
//! printer subset, plus label/command-column alignment.** It prints a
//! source-faithful skeleton — headers, braces, indentation, one
//! statement per line (with labels aligned into a shared command column,
//! see [`command_column`]), canonical item text, a single-line comma
//! join — and deliberately leaves the following as seams for later tasks
//! (each seam is marked at its printing site below):
//!
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
//! half-implement Tasks 6-8's rules. The three-check objective harness
//! (`tests/fmt_programs.rs`) is scoped to a SIMPLE program set the
//! printer fully supports (labeled or unlabeled statements, no comments,
//! no namespaces/imports, single-line comma groups) — each later task
//! widens that set as its seam closes.

use crate::compiler::CompileError;
use crate::cst::{BodyItem, BodyKind, CommaItem, Cst, FunctionCst, StatementCst, TopItem, TopKind};
use crate::lexer::{LexMode, lex_with};
use crate::parser::{Builtin, CheckArm, Item, Label, Successor, parse_cst};

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
    // Spec "Label / command alignment": the command column is scoped to
    // THIS function's own body (a nested function computes its own, one
    // level deeper — recursion below handles that for free).
    let command_col = command_column(max_inline_label_prefix_width(&f.body), body_indent);
    for body_item in &f.body {
        print_body_item(out, body_item, body_indent, command_col);
    }
    out.push_str(&pad);
    out.push_str("}\n");
}

fn print_body_item(out: &mut String, item: &BodyItem, indent: usize, command_col: usize) {
    // Seam (Task 8): `item.blank_before` is not read yet.
    match &item.kind {
        BodyKind::Statement(s) => print_statement(out, s, command_col),
        BodyKind::Nested(f) => print_function(out, f, indent),
        // Seam (Task 7): own-line body comments.
        BodyKind::Comment(_) => {}
    }
}

/// Label prefix width: the smallest multiple of [`INDENT_UNIT`] that is
/// `>= max(base_body_indent, P + 2)`, where `P` is the widest INLINE
/// labeled statement's label-prefix width in the body (own-line labels
/// and unlabeled statements don't count toward `P` — spec "Label /
/// command alignment"). Pure and independently unit-tested below.
fn command_column(p: usize, base_body_indent: usize) -> usize {
    let min = base_body_indent.max(p + 2);
    min.div_ceil(INDENT_UNIT) * INDENT_UNIT
}

/// `P`: the max label-prefix width among `body`'s own INLINE labeled
/// statements. Only looks at this function's OWN [`BodyItem`]s — a
/// nested function's statements belong to ITS body/command-column, not
/// this one (module doc's per-body scoping).
fn max_inline_label_prefix_width(body: &[BodyItem]) -> usize {
    body.iter()
        .filter_map(|item| match &item.kind {
            BodyKind::Statement(s) if !s.labels.is_empty() && !s.label_break => {
                Some(label_prefix_width(&s.labels))
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// A statement's label prefix as printed: each label `N:`, joined by one
/// space (spec "Intra-statement token spacing" → Label row), e.g. `1:` or
/// the stacked `1: 2:`. Empty for an unlabeled statement.
fn label_prefix_text(labels: &[Label]) -> String {
    labels
        .iter()
        .map(|l| format!("{}:", l.value))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Char width of [`label_prefix_text`] — brief step 1: "Width = char
/// count."
fn label_prefix_width(labels: &[Label]) -> usize {
    label_prefix_text(labels).chars().count()
}

/// Left margin for a `prefix_width`-wide label prefix at `command_col`,
/// or `None` if that would leave less than the mandatory 1-space margin
/// (brief step 6's "too long" case; `command_col - 1 - prefix_width`
/// computed without underflowing on a wide own-line label).
fn label_margin(command_col: usize, prefix_width: usize) -> Option<usize> {
    command_col
        .checked_sub(prefix_width + 1)
        .filter(|&margin| margin >= 1)
}

/// One statement (spec "Statements" + "Label / command alignment" +
/// "Own-line labels"):
///
/// - **Unlabeled**: `command_col` spaces of indent, then the comma-joined
///   items.
/// - **Inline labeled** (`label_break == false`): the label prefix
///   right-aligned so its final `:` sits at `command_col - 2`, one space,
///   then the command at `command_col`. [`max_inline_label_prefix_width`]
///   guarantees a `>= 1` margin for every inline labeled statement in the
///   body, so [`label_margin`] always returns `Some` here.
/// - **Own-line labeled** (`label_break == true`, excluded from `P`): the
///   prefix on its own line — right-aligned like an inline label if it
///   fits, else hung at a strict 1-space margin — then the command on the
///   following line at `command_col`. fmt never auto-breaks a label:
///   `label_break` is read, never inferred or overridden.
///
/// Seam (Task 6): comma groups always join on one line at `command_col`.
/// Seam (Task 7): `s.trailing` (a same-line trailing comment) is not read.
fn print_statement(out: &mut String, s: &StatementCst, command_col: usize) {
    if s.labels.is_empty() {
        out.push_str(&" ".repeat(command_col));
    } else {
        let prefix = label_prefix_text(&s.labels);
        let width = prefix.chars().count();
        if s.label_break {
            match label_margin(command_col, width) {
                Some(margin) => out.push_str(&" ".repeat(margin)),
                None => out.push(' '),
            }
            out.push_str(&prefix);
            out.push('\n');
            out.push_str(&" ".repeat(command_col));
        } else {
            let margin = label_margin(command_col, width).expect(
                "max_inline_label_prefix_width guarantees a >=1 margin for every inline label",
            );
            out.push_str(&" ".repeat(margin));
            out.push_str(&prefix);
            out.push(' ');
        }
    }
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

    // -- Task 5: label/command alignment -----------------------------

    #[test]
    fn command_column_worked_values() {
        // P=0 (no labels): base indent alone.
        assert_eq!(command_column(0, 4), 4);
        // P=2 (`1:`): max(4, 4) = 4.
        assert_eq!(command_column(2, 4), 4);
        // P=6 (`11111:`): max(4, 8) = 8.
        assert_eq!(command_column(6, 4), 8);
        // P=3 (`12:`): max(4, 5) = 5, rounded up to 8.
        assert_eq!(command_column(3, 4), 8);
        // P=5, stacked labels (`1: 2:`): max(4, 7) = 7, rounded up to 8.
        assert_eq!(command_column(5, 4), 8);
    }

    #[test]
    fn command_column_namespaced_base_indent() {
        // base_body_indent 8 (one level deeper, e.g. a namespaced/nested
        // body per Task 8). No label wide enough to push past it.
        assert_eq!(command_column(0, 8), 8);
        assert_eq!(command_column(2, 8), 8);
        // P=10 pushes past the deeper base: max(8, 12) = 12, already a
        // multiple of 4.
        assert_eq!(command_column(10, 8), 12);
    }

    #[test]
    fn a_single_inline_label_command_column_4() {
        assert_eq!(
            format("main() { 1: right; check(1, 2); }").unwrap(),
            "main() {\n 1: right;\n    check(1, 2);\n}\n"
        );
    }

    #[test]
    fn b_widest_inline_label_pads_narrower_ones_left() {
        // `stop` in the brief's illustration isn't a real `.pmc` command;
        // substituted with `halt` (identical 4-char width, so the
        // alignment columns this test pins are unaffected).
        assert_eq!(
            format("main() { 11111: right; left; 12: halt; }").unwrap(),
            "main() {\n 11111: right;\n        left;\n    12: halt;\n}\n"
        );
    }

    #[test]
    fn c_own_line_labels_fit_and_overflow() {
        // `12:` is own-line but fits (right-aligns like an inline label);
        // `999999999:` is own-line and too long (hangs at 1 space). Both
        // commands land on the same command column (8) set by `11111:`.
        let src = "main() {\n11111: right;\n12:\nleft;\n999999999:\nhalt;\n}\n";
        assert_eq!(
            format(src).unwrap(),
            "main() {\n 11111: right;\n    12:\n        left;\n 999999999:\n        halt;\n}\n"
        );
    }

    #[test]
    fn d_stacked_labels_round_up_command_column() {
        // Prefix `1: 2:` has width 5 -> C = max(4, 7) = 7, rounded up to
        // 8 (the only multiple of 4 satisfying the round-up rule) -> a
        // 2-space left margin (8 - 1 - 5 = 2). NOTE: the task brief's
        // illustrative code block shows a single leading space (i.e. an
        // un-rounded C=7); that contradicts the brief's own stated
        // algorithm (explicitly "rounded to 8" in the same block) and the
        // mandatory P=3 unit test above (which only round-up produces).
        // Implemented per the stated round-up rule; see task-5-report.md.
        assert_eq!(
            format("main() { 1: 2: right; }").unwrap(),
            "main() {\n  1: 2: right;\n}\n"
        );
    }
}
