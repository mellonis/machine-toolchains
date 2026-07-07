//! `.pmc` pretty-printer
//! (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`, "Formatting
//! model"). Thin renderer, same discipline as [`crate::compile`] and
//! [`crate::lint`]: [`format`] returns a `Result` and never prints — the
//! future `cli/fmt.rs` is the only place that renders errors or touches
//! the filesystem.
//!
//! **Scope of this module today (fmt build Tasks 4-6): the TRIVIAL
//! printer subset, label/command-column alignment, and comma-group
//! layout.** It prints a source-faithful skeleton — headers, braces,
//! indentation, one statement per line (with labels aligned into a shared
//! command column, see [`command_column`]), canonical item text, and the
//! comma-group Y / greedy-fill layout (see [`render_items`]) — and
//! deliberately leaves the following as seams for later tasks (each seam
//! is marked at its printing site below):
//!
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
//! half-implement Task 8's rules. The three-check objective harness
//! (`tests/fmt_programs.rs`) is scoped to a SIMPLE program set the
//! printer fully supports (labeled or unlabeled statements, no comments,
//! no namespaces/imports, comma groups including multi-line ones) — Task 8
//! widens that set further as its seam closes.

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
    // Whichever branch above ran, the current physical line now has
    // exactly `command_col` characters on it (indent alone, or margin +
    // label prefix + one space, which `label_margin` sizes to land
    // exactly on `command_col` too) — `render_items` relies on this to
    // size its own line-fit checks without inspecting `out`.
    out.push_str(&render_items(&s.items, command_col));
    out.push_str(";\n");
}

/// Line width limit (spec "Line limit" — 80 characters, matching lint's
/// `line-too-long`; char count, not bytes).
const LINE_WIDTH: usize = 80;

/// Comma-group layout (design doc "Comma-group layout", rules 1-3):
/// respect the author's line breaks (`CommaItem::newline_before`), with a
/// greedy-fill width fallback. `command_col` is both the column the
/// caller's line is already sitting at (see [`print_statement`]'s note)
/// and the continuation column for every wrapped line this function
/// introduces.
///
/// Items are first partitioned into groups at each `newline_before`
/// boundary — the first item always starts group 0, and an item with
/// `newline_before` set always starts a NEW group. When no item sets it,
/// this yields exactly one group holding every item, which collapses
/// rules 1/2 (no author break) onto the very same per-group logic as
/// rule 3's preserved lines: each group is emitted as one line if it fits
/// (`command_col` + its comma-joined text + 1 for the trailing `,`
/// boundary or the statement's final `;`, both width 1, <= 80), else
/// [`greedy_fill_group`] repacks just that group. A non-last group's line
/// ends with a trailing `,` (the boundary to the next group); the very
/// last group carries none — [`print_statement`] appends the final `;`
/// itself.
///
/// Does not read [`CommaItem::leading`] (Task 7).
fn render_items(items: &[CommaItem], command_col: usize) -> String {
    let texts: Vec<String> = items.iter().map(|ci| render_item(&ci.item)).collect();
    let mut groups: Vec<Vec<usize>> = vec![vec![0]];
    for (i, ci) in items.iter().enumerate().skip(1) {
        if ci.newline_before {
            groups.push(vec![i]);
        } else {
            groups.last_mut().expect("groups is never empty").push(i);
        }
    }
    let last_group_idx = groups.len() - 1;
    let mut out = String::new();
    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            out.push('\n');
            out.push_str(&" ".repeat(command_col));
        }
        let group_texts: Vec<&str> = group.iter().map(|&i| texts[i].as_str()).collect();
        let joined = group_texts.join(", ");
        // `+ 1` reserved for the trailing `,`/`;` is folded into `< LINE_WIDTH`
        // (clippy::int_plus_one) rather than `+ 1 <= LINE_WIDTH`.
        if command_col + joined.chars().count() < LINE_WIDTH {
            out.push_str(&joined);
        } else {
            greedy_fill_group(&mut out, &group_texts, command_col);
        }
        if gi != last_group_idx {
            out.push(',');
        }
    }
    out
}

/// Rule 2's greedy-fill, applied to one group's items (the whole
/// statement when there was no author break at all; one preserved line
/// when rule 3's grouping still leaves a line over 80). Packs items onto
/// the current line while they fit, breaking after the last comma that
/// fit (the comma trails the closed line); the new line starts at
/// `command_col`.
///
/// Every item — including the group's last — is followed on its own
/// physical line by exactly one trailing punctuation character: an
/// interior item's separating `,`, or (for the group's last item) the
/// caller's boundary `,`/final `;`. Both are width 1, so every placement
/// check below reserves exactly 1 for "whatever comes right after this
/// item on this line" uniformly. An item placed first on a (new) line is
/// never re-checked against the limit — with no preceding comma to break
/// on, a single over-wide command stays overlong (`line-too-long` lint's
/// job, not fmt's).
fn greedy_fill_group(out: &mut String, texts: &[&str], command_col: usize) {
    let mut items = texts.iter();
    let first = items.next().expect("a comma group is never empty");
    out.push_str(first);
    let mut col = command_col + first.chars().count();
    for text in items {
        let w = text.chars().count();
        // Same `+ 1 <= LINE_WIDTH` -> `< LINE_WIDTH` fold as above.
        if col + 2 + w < LINE_WIDTH {
            out.push_str(", ");
            out.push_str(text);
            col += 2 + w;
        } else {
            out.push(',');
            out.push('\n');
            out.push_str(&" ".repeat(command_col));
            out.push_str(text);
            col = command_col + w;
        }
    }
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

    // -- Task 6: comma-group layout (Y + greedy-fill) ----------------

    #[test]
    fn e_rule_1_no_newline_fits_stays_a_single_line() {
        // Unchanged from Task 5 — no `newline_before` anywhere and the
        // one-line form fits comfortably under 80.
        assert_eq!(
            format("f() { left, right; }").unwrap(),
            "f() {\n    left, right;\n}\n"
        );
        assert_eq!(
            format("main() { left, right, mark; }").unwrap(),
            "main() {\n    left, right, mark;\n}\n"
        );
    }

    #[test]
    fn f_rule_3_preserves_the_authors_line_break() {
        // Brief's byte test: `1:` -> C=4; author put a newline before
        // `mark`, so `left, right` stays on the label's line (trailing
        // comma) and `mark` continues at the command column.
        assert_eq!(
            format("main() {\n1: left, right,\nmark;\n}").unwrap(),
            "main() {\n 1: left, right,\n    mark;\n}\n"
        );
    }

    #[test]
    fn g_rule_2_greedy_fill_breaks_after_the_last_fitting_comma() {
        // No author newline; the one-line join overflows 80. Four
        // identical 20-char calls (`@` + a 17-char name + `()`), command
        // column 4 (unlabeled `main`): one-line width = 4 + (4*20 + 3*2)
        // + 1 (`;`) = 4 + 86 + 1 = 91 > 80, so rule 2 applies.
        //
        // Hand trace (col starts at the command column, 4):
        //   call0: col = 4 + 20 = 24 (first on line, placed unconditionally)
        //   call1: 24 + 2 + 20 + 1 (reserve) = 47 <= 80 -> fits -> col = 46
        //   call2: 46 + 2 + 20 + 1 = 69 <= 80 -> fits -> col = 68
        //   call3: 68 + 2 + 20 + 1 = 91 > 80 -> breaks to the command column
        // Line 1 ends up 4 + "call, call, call," (65) = 69 chars; line 2 is
        // 4 + "call;" (21) = 25 chars — both <= 80.
        const CALL: &str = "@abcdefghijklmnopq()";
        let src =
            format!("main() {{ {CALL}, {CALL}, {CALL}, {CALL}; }} abcdefghijklmnopq() {{ halt; }}");
        let expected = format!(
            "main() {{\n    {CALL}, {CALL}, {CALL},\n    {CALL};\n}}\nabcdefghijklmnopq() {{\n    halt;\n}}\n"
        );
        let out = format(&src).unwrap();
        assert_eq!(out, expected);
        // The whole point of greedy-fill: no emitted line may exceed 80
        // (fmt IS the `line-too-long` fix — spec "Line limit").
        assert!(out.lines().all(|l| l.chars().count() <= 80));

        // Idempotent: reformatting the already-wrapped output must be a
        // no-op (the harness pins this generically; this test pins the
        // exact bytes for this specific overflow shape too).
        assert_eq!(format(&expected).unwrap(), expected);
    }

    #[test]
    fn i_greedy_fill_boundary_at_exactly_80_chars() {
        // Pins the `+ 1` reserve exactly at the 80-char edge, not just
        // gross overflow: `item0` is `@aaaaaaa()` (10 chars, name = 7
        // `a`s), command column 4 (unlabeled `main`), so col after item0
        // = 4 + 10 = 14.
        let name0 = "a".repeat(7);

        // `name1_fits` = 60 `a`s -> item1 width 63. Joined = 10+2+63=75;
        // whole line = 4 + 75 + 1 (`;`) = 80 -- exactly the limit, still
        // one line (rule 1, not rule 2).
        let name1_fits = "a".repeat(60);
        let src_fits = format!("main() {{ @{name0}(), @{name1_fits}(); }}");
        let expected_fits = format!("main() {{\n    @{name0}(), @{name1_fits}();\n}}\n");
        let out_fits = format(&src_fits).unwrap();
        assert_eq!(out_fits, expected_fits);
        assert!(out_fits.lines().all(|l| l.chars().count() <= 80));

        // One `a` longer (61 `a`s -> item1 width 64): the one-line join is
        // now 81 chars -- one over the limit -- so rule 2 wraps, breaking
        // right after item0 (item1 alone can't share the line: even by
        // itself, `col(14) + 2 + 64 = 80`, which the `< LINE_WIDTH` check
        // rejects since it must also leave room for the trailing `;`).
        let name1_overflows = "a".repeat(61);
        let src_overflow = format!("main() {{ @{name0}(), @{name1_overflows}(); }}");
        let expected_overflow =
            format!("main() {{\n    @{name0}(),\n    @{name1_overflows}();\n}}\n");
        let out_overflow = format(&src_overflow).unwrap();
        assert_eq!(out_overflow, expected_overflow);
        assert!(out_overflow.lines().all(|l| l.chars().count() <= 80));
    }

    #[test]
    fn h_rule_3_line_that_still_overflows_gets_greedy_filled_too() {
        // A preserved (author-split) group whose FIRST line alone already
        // overflows 80 falls back to rule 2's greedy-fill for THAT line
        // only; the second preserved line (`mark`) is untouched. Reusing
        // `g`'s four-call group (same arithmetic: breaks after the 3rd
        // call), followed by an author newline before `mark`.
        const CALL: &str = "@abcdefghijklmnopq()";
        let src = format!(
            "main() {{ {CALL}, {CALL}, {CALL}, {CALL},\nmark; }} abcdefghijklmnopq() {{ halt; }}"
        );
        let expected = format!(
            "main() {{\n    {CALL}, {CALL}, {CALL},\n    {CALL},\n    mark;\n}}\nabcdefghijklmnopq() {{\n    halt;\n}}\n"
        );
        let out = format(&src).unwrap();
        assert_eq!(out, expected);
        assert!(out.lines().all(|l| l.chars().count() <= 80));

        // Re-parsing the wrapped output re-derives a DIFFERENT grouping
        // (3 preserved lines instead of 2 — the greedy-fill break now
        // itself reads back as an author newline), but the rendered bytes
        // must still be stable.
        assert_eq!(format(&expected).unwrap(), expected);
    }

    #[test]
    fn idempotent_on_multi_line_groups() {
        const CALL: &str = "@abcdefghijklmnopq()";
        for src in [
            "main() {\n1: left, right,\nmark;\n}".to_string(),
            format!("main() {{ {CALL}, {CALL}, {CALL}, {CALL}; }} abcdefghijklmnopq() {{ halt; }}"),
            format!(
                "main() {{ {CALL}, {CALL}, {CALL}, {CALL},\nmark; }} abcdefghijklmnopq() {{ halt; }}"
            ),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }
}
