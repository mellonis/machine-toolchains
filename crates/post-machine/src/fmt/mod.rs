//! `.pmc` pretty-printer
//! (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`, "Formatting
//! model"). Thin renderer, same discipline as [`crate::compile`] and
//! [`crate::lint`]: [`format`] returns a `Result` and never prints — the
//! future `cli/fmt.rs` is the only place that renders errors or touches
//! the filesystem.
//!
//! **Scope of this module today (fmt build Tasks 4-8b, the pure-printer
//! finish): the TRIVIAL printer subset, label/command-column alignment,
//! comma-group layout, comment placement/alignment, namespaces,
//! blank-line policy, imports/export printing, and the full
//! intra-statement spacing table / spaced-form normalization / textual
//! hygiene / edge cases.** It prints a source-faithful skeleton —
//! headers, braces, indentation (including namespace nesting, see
//! [`print_namespace`]), one statement per line (with labels aligned into
//! a shared command column, see [`command_column`]), canonical item text,
//! the comma-group Y / greedy-fill layout (see [`render_items`]), every
//! comment (own-line, trailing, and mid-comma-group) reprinted per the
//! design doc's "Comments = trivia-tokens native in the CST" + "Trailing
//! comments", the general blank-line policy (preserve / collapse runs /
//! never force, see [`top_wants_blank_before`] / [`body_wants_blank_before`]),
//! grouped `use` lists (see [`print_use`]), the verbatim `export` keyword
//! (see [`FunctionCst::has_export`]), and — Task 8b's own contribution —
//! the spacing-table/spaced-form/hygiene/edge-case tests in this module's
//! own `tests` submodule. That last part needed no renderer change:
//! `parse_cst` only ever hands the printer the parsed VALUE (a label's
//! `u32`, a path's `Vec<String>` segments), never the author's original
//! spacing or line endings, so every item shape this printer covers was
//! already canonical, and the full reprint (spaces + `\n` only, from the
//! CST) already discards trailing whitespace / CRLF / tabs by
//! construction — Task 8b's tests PIN that rather than fixing a gap.
//! Task 9 points the objective-guard harness (`tests/fmt_programs.rs`) at
//! the full corpus.
//!
//! ## Comment placement (Task 7)
//!
//! Every own-line comment ([`crate::cst::TopKind::Comment`] /
//! [`crate::cst::BodyKind::Comment`]) prints IDENTICALLY regardless of
//! whether the design doc would label it leading, standalone, or dangling
//! (see [`print_comment`]) — those three names describe the comment's
//! RELATIONSHIP to its neighbors (purely for the blank-line decision
//! above), not a different rendering. A same-line trailing comment
//! (`StatementCst::trailing`) rides the statement's own line; a run of
//! trailing comments the author aligned in source is kept aligned,
//! recomputed against the reformatted code (see [`compute_trailing_spacing`]).
//! A mid-comma-group comment ([`crate::cst::CommaItem::leading`]) either
//! stays inline (a BLOCK comment) or forces the group onto multiple lines
//! (a LINE comment can't be followed by code on its own line — see
//! [`render_items`]).

use crate::compiler::CompileError;
use crate::cst::{
    BodyItem, BodyKind, CommaItem, Cst, FunctionCst, NamespaceCst, StatementCst, TopItem, TopKind,
    UseCst, UsePath,
};
use crate::lexer::{Comment, CommentKind, LexMode, lex_with};
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
    print_top_items(&mut out, &cst.items, 0);
    // Edge case (spec "Edge cases"): an empty file still reprints with
    // exactly one final newline. Non-empty output already ends in `\n`
    // from the last printed function/comment, so nothing further is
    // needed there (asserted by the Task-4 tests below).
    if out.is_empty() {
        out.push('\n');
    }
    out
}

/// One level's `TopItem` list, at `indent` — the file level (`indent ==
/// 0`) and a [`NamespaceCst`]'s own `items` (one level deeper) share this
/// loop, so [`print_namespace`] gets recursion "for free".
fn print_top_items(out: &mut String, items: &[TopItem], indent: usize) {
    for (i, item) in items.iter().enumerate() {
        if top_wants_blank_before(items, i) {
            out.push('\n');
        }
        print_top_item(out, item, indent);
    }
}

/// Whether item `i` (`i > 0`) should be preceded by a blank line (spec
/// "Blank lines": preserve the author's choice, collapse any run to one,
/// force nothing). `blank_before` is already a bool (Task-3 CST design),
/// so a source run of 2+ blanks already collapsed to one by construction.
/// The `i > 0` guard IS the brace-edge suppression for "immediately after
/// `{`" (index 0 is always the first item of a body/namespace/file, and
/// never gets a blank); "immediately before `}`" never arises in the
/// first place — no `TopItem`/`BodyItem` exists AFTER the last one to
/// carry a trailing blank's `blank_before`, so nothing further is needed
/// for that edge.
fn top_wants_blank_before(items: &[TopItem], i: usize) -> bool {
    i > 0 && items[i].blank_before
}

/// Same rule as [`top_wants_blank_before`], scoped to a function body.
fn body_wants_blank_before(items: &[BodyItem], i: usize) -> bool {
    i > 0 && items[i].blank_before
}

fn print_top_item(out: &mut String, item: &TopItem, indent: usize) {
    match &item.kind {
        TopKind::Function(f) => print_function(out, f, indent),
        TopKind::Comment(c) => print_comment(out, c, indent),
        TopKind::Namespace(ns) => print_namespace(out, ns, indent),
        TopKind::Import(use_cst) => print_use(out, use_cst, indent),
    }
}

/// `namespace NAME { … }` (spec "Headers and braces": one space before
/// `{`, the closing `}` alone at the header's own indent) — its `items`
/// print one level deeper via the same [`print_top_items`] loop the file
/// level uses, so nesting (a namespace inside a namespace) recurses for
/// free, and a nested function's body indent (namespace +1, function +1)
/// already falls out of [`print_function`]'s own `indent + INDENT_UNIT`.
fn print_namespace(out: &mut String, ns: &NamespaceCst, indent: usize) {
    let pad = " ".repeat(indent);
    out.push_str(&pad);
    out.push_str("namespace ");
    out.push_str(&ns.name);
    out.push_str(" {\n");
    print_top_items(out, &ns.items, indent + INDENT_UNIT);
    out.push_str(&pad);
    out.push_str("}\n");
}

/// One `use` list (spec "Imports"): paths in source order, never
/// reordered/merged/split (module doc's `UseCst` grouping — Task-3's
/// former per-path `ImportCst` gap, fixed at the CST level). Trailing
/// comment placement is the same one-space default as everywhere else in
/// this task (the design doc's context-sensitive alignment rule targets
/// function-body statement runs — [`compute_trailing_spacing`] — and
/// names no equivalent rule for imports).
fn print_use(out: &mut String, u: &UseCst, indent: usize) {
    out.push_str(&" ".repeat(indent));
    out.push_str("use ");
    let rendered: Vec<String> = u.paths.iter().map(render_use_path).collect();
    out.push_str(&rendered.join(", "));
    out.push(';');
    if let Some(tc) = &u.trailing {
        out.push(' ');
        out.push_str(&normalize_comment_text(&tc.comment.text));
    }
    out.push('\n');
}

/// One `use`-list path (spec "Intra-statement token spacing" → Path row +
/// "Imports"): `::` tight, ` as ALIAS` one space each side if present.
fn render_use_path(p: &UsePath) -> String {
    let mut s = p.path.join("::");
    if let Some(alias) = &p.alias {
        s.push_str(" as ");
        s.push_str(alias);
    }
    s
}

/// Normalizes one comment's raw trivia text for printing (spec "Textual
/// hygiene", §C): every line's TRAILING whitespace stripped, joined back
/// with LF only. [`Comment::text`] is raw lexer trivia, captured
/// character-for-character from source (module doc's "Comments =
/// trivia-tokens native in the CST") — a line comment's trailing spaces,
/// or a `\r` immediately before the closing `\n` of a CRLF source line,
/// survive into the token verbatim unless stripped here; nothing else in
/// the pipeline ever touches comment text. Only each line's END is
/// touched — a block comment's interior LEADING whitespace (content
/// fidelity, "Re-indentation": "interior lines are preserved verbatim")
/// is untouched by `trim_end`.
fn normalize_comment_text(text: &str) -> String {
    text.split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// One own-line comment (leading / standalone / dangling — content
/// printing is IDENTICAL for all three; only the surrounding blank-line
/// decision differs, made by the caller from `blank_before`; module doc's
/// "Comment placement"). Re-indentation (design doc, content fidelity):
/// a LINE comment is single-line already, so prefixing `indent` IS the
/// whole re-indent; a BLOCK comment's `text` already carries any interior
/// lines VERBATIM (raw source whitespace, never reflowed) — prefixing
/// `indent` once, before the first line, is the entire re-indent for it
/// too. [`normalize_comment_text`] applies the §C hygiene rules on top.
fn print_comment(out: &mut String, comment: &Comment, indent: usize) {
    out.push_str(&" ".repeat(indent));
    out.push_str(&normalize_comment_text(&comment.text));
    out.push('\n');
}

/// Header + body + closing brace (spec "Headers and braces"). Used for
/// both top-level and nested functions — a nested [`FunctionCst`] has the
/// same shape, just one indent level deeper and (per the grammar) never
/// `has_export`.
///
/// **Export keyword, verbatim** (fmt design doc §D — resolves what was
/// Task-4's "Known CST information-loss gap"): `f.has_export` records
/// whether the author literally wrote `export`, independent of
/// `f.exported` (which additionally folds in top-level `main`'s
/// auto-export — `parser.rs`'s `f.exported = exported || (ns.is_empty()
/// && f.name == "main")`). Printing `has_export` directly means
/// `export main() { … }` keeps its (legal but redundant) `export`, and
/// bare `main() { … }` stays bare — both compile identically either way.
fn print_function(out: &mut String, f: &FunctionCst, indent: usize) {
    let pad = " ".repeat(indent);
    out.push_str(&pad);
    if f.has_export {
        out.push_str("export ");
    }
    out.push_str(&f.name);
    out.push_str("() {\n");
    let body_indent = indent + INDENT_UNIT;
    // Spec "Label / command alignment": the command column is scoped to
    // THIS function's own body (a nested function computes its own, one
    // level deeper — recursion below handles that for free).
    let command_col = command_column(max_inline_label_prefix_width(&f.body), body_indent);
    // Every statement's code (label + items, no `;`) is rendered ONCE up
    // front — the trailing-comment alignment pre-pass (§C) needs every
    // run member's rendered width before any of them is printed; the
    // print loop below reuses the same strings instead of re-rendering.
    // Non-statement items get an unused empty placeholder.
    let codes: Vec<String> = f
        .body
        .iter()
        .map(|bi| match &bi.kind {
            BodyKind::Statement(s) => render_statement_code(s, command_col),
            BodyKind::Nested(_) | BodyKind::Comment(_) => String::new(),
        })
        .collect();
    let trailing_spacing = compute_trailing_spacing(&f.body, &codes);
    for (i, body_item) in f.body.iter().enumerate() {
        if body_wants_blank_before(&f.body, i) {
            out.push('\n');
        }
        print_body_item(out, body_item, body_indent, &codes[i], trailing_spacing[i]);
    }
    out.push_str(&pad);
    out.push_str("}\n");
}

fn print_body_item(
    out: &mut String,
    item: &BodyItem,
    indent: usize,
    code: &str,
    trailing_spacing: usize,
) {
    match &item.kind {
        BodyKind::Statement(s) => print_statement(out, s, code, trailing_spacing),
        BodyKind::Nested(f) => print_function(out, f, indent),
        BodyKind::Comment(c) => print_comment(out, c, indent),
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
/// Returns the statement's code up to but NOT including the final `;` —
/// no trailing comment, no newline. Split out from [`print_statement`] so
/// [`compute_trailing_spacing`] (§C) can render every statement once,
/// up front, to measure line widths before any trailing comment is
/// placed (`print_function` renders each body item's code exactly once
/// and reuses it for both purposes).
fn render_statement_code(s: &StatementCst, command_col: usize) -> String {
    let mut out = String::new();
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
    out
}

/// One statement's final line(s): the precomputed `code`
/// ([`render_statement_code`]), the `;`, then a same-line trailing
/// comment if any — spaced per `trailing_spacing`
/// ([`compute_trailing_spacing`], §C) — then the newline.
fn print_statement(out: &mut String, s: &StatementCst, code: &str, trailing_spacing: usize) {
    out.push_str(code);
    out.push(';');
    if let Some(tc) = &s.trailing {
        out.push_str(&" ".repeat(trailing_spacing));
        out.push_str(&normalize_comment_text(&tc.comment.text));
    }
    out.push('\n');
}

/// Char width of `code`'s LAST physical line, `+ 1` for the `;` that
/// follows it (a statement's trailing comment always rides the line
/// carrying the final `;`, even when a multi-line own-line label or
/// comma-group spreads the rest of the statement across earlier lines —
/// design doc "Trailing comments": alignment is against "the longest
/// reformatted code LINE in the run", not the whole statement).
fn code_line_width_incl_semi(code: &str) -> usize {
    let last_line = code.rsplit('\n').next().unwrap_or(code);
    last_line.chars().count() + 1
}

/// Trailing-comment context-sensitive alignment (design doc "Trailing
/// comments", brief §C). Returns, per `body` index, the number of spaces
/// to place between the `;` and a trailing `//`/`/* */` — meaningful only
/// where that [`BodyItem`] is a [`BodyKind::Statement`] with
/// `trailing.is_some()`; other entries are unused filler.
///
/// A **run** is a maximal sequence of consecutive [`BodyKind::Statement`]
/// items that each carry a trailing comment, unbroken by a blank line
/// (`blank_before`) or by a non-statement / no-trailing-comment item in
/// between. A lone trailing comment, or a run whose source `//` columns
/// are not all equal, gets one space each. A run of >= 2 sharing a common
/// SOURCE column is kept aligned at `(widest reformatted code line in the
/// run) + 1 space`; a line that would cross 80 at that column falls back
/// to one space instead (design doc, "If placing an aligned `//` would
/// push its line past 80").
///
/// **Idempotence note**: the aligned-vs-source-column check reads ONLY
/// the run members that do NOT hit the 80-column fallback. A line that
/// falls back renders at its OWN width-derived column, which need not
/// equal the run's aligned column — including it in the alignment check
/// would make a second pass (which re-derives source columns from THIS
/// pass's OUTPUT) see a different, non-matching set of columns and flip
/// the run from aligned to ragged. Excluding it keeps the aligned/ragged
/// verdict — and thus the whole run's layout — stable across passes.
fn compute_trailing_spacing(body: &[BodyItem], codes: &[String]) -> Vec<usize> {
    let mut spacing = vec![1usize; body.len()];
    let has_trailing =
        |bi: &BodyItem| matches!(&bi.kind, BodyKind::Statement(s) if s.trailing.is_some());
    let mut i = 0;
    while i < body.len() {
        if !has_trailing(&body[i]) {
            i += 1;
            continue;
        }
        let run_start = i;
        let mut j = i + 1;
        while j < body.len() && has_trailing(&body[j]) && !body[j].blank_before {
            j += 1;
        }
        let run_end = j;
        let run_len = run_end - run_start;

        let code_w: Vec<usize> = (run_start..run_end)
            .map(|k| code_line_width_incl_semi(&codes[k]))
            .collect();
        let comment_w: Vec<usize> = (run_start..run_end)
            .map(|k| {
                let BodyKind::Statement(s) = &body[k].kind else {
                    unreachable!("has_trailing guarantees a Statement");
                };
                // Measured on the NORMALIZED text (§C): a raw trailing
                // `\r`/space in the token must not inflate the column
                // math for a width nothing will actually print.
                normalize_comment_text(
                    &s.trailing
                        .as_ref()
                        .expect("has_trailing guarantees Some")
                        .comment
                        .text,
                )
                .chars()
                .count()
            })
            .collect();

        if run_len >= 2 {
            let max_code_w = *code_w.iter().max().expect("run_len >= 2");
            let align_col = max_code_w + 1;
            let overflow: Vec<bool> = (0..run_len)
                .map(|off| align_col + comment_w[off] > LINE_WIDTH)
                .collect();
            let source_cols: Vec<u32> = (run_start..run_end)
                .map(|k| {
                    let BodyKind::Statement(s) = &body[k].kind else {
                        unreachable!("has_trailing guarantees a Statement");
                    };
                    s.trailing
                        .as_ref()
                        .expect("has_trailing guarantees Some")
                        .col
                })
                .collect();
            let non_overflow_cols: Vec<u32> = source_cols
                .iter()
                .zip(&overflow)
                .filter(|&(_, ovf)| !ovf)
                .map(|(&c, _)| c)
                .collect();
            let aligned = non_overflow_cols.windows(2).all(|w| w[0] == w[1]);
            for off in 0..run_len {
                spacing[run_start + off] = if aligned && !overflow[off] {
                    align_col - code_w[off]
                } else {
                    1
                };
            }
        }
        // run_len == 1 (lone): leave the default 1.
        i = run_end;
    }
    spacing
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
/// Also renders [`CommaItem::leading`] (§D, mid-comma-group comments —
/// `a, /* x */ b` / `a, // x` then `b` on the next line): see
/// [`layout_leading`] for how a leading comment run resolves to an inline
/// prefix or a forced break, folded into the SAME group-boundary
/// machinery a `newline_before` break uses (a forced break behaves like
/// an author newline for grouping purposes).
fn render_items(items: &[CommaItem], command_col: usize) -> String {
    let layouts: Vec<LeadingLayout> = items.iter().map(|ci| layout_leading(&ci.leading)).collect();
    let texts: Vec<String> = items
        .iter()
        .zip(&layouts)
        .map(|(ci, layout)| format!("{}{}", layout.inline_prefix, render_item(&ci.item)))
        .collect();
    let mut groups: Vec<Vec<usize>> = vec![vec![0]];
    for (i, ci) in items.iter().enumerate().skip(1) {
        if ci.newline_before || layouts[i].forced_break {
            groups.push(vec![i]);
        } else {
            groups.last_mut().expect("groups is never empty").push(i);
        }
    }
    let last_group_idx = groups.len() - 1;
    let mut out = String::new();
    // Rare (`parser.rs`'s own leading-trivia doc: "a comment between the
    // label and the first command"): a forcing LINE comment on item 0
    // has no preceding `,` to attach to — emit it directly (the caller
    // already left `out`'s position at `command_col`, per
    // `render_statement_code`'s invariant).
    if layouts[0].forced_break {
        emit_forced_break(&mut out, &layouts[0], command_col);
    }
    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            let first_idx = group[0];
            if layouts[first_idx].forced_break {
                // The preceding group's trailing `,` (below) is already
                // in `out`; one space, then the comment(s), then break.
                out.push(' ');
                emit_forced_break(&mut out, &layouts[first_idx], command_col);
            } else {
                out.push('\n');
                out.push_str(&" ".repeat(command_col));
            }
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

/// How one [`CommaItem::leading`] list resolves (§D). A BLOCK comment
/// with no LINE comment among it/its siblings stays inline, prepended to
/// the item's own text (`inline_prefix`) — the join/greedy-fill logic
/// downstream never has to know a comment was there. A LINE comment
/// forces a break: nothing can follow `//` on its physical line, so
/// everything up to and including the first LINE comment becomes
/// `break_inline` (emitted right after the preceding separator, before
/// the forced newline); anything AFTER that first LINE comment
/// (pathological, but still MUST be reprinted per the brief — fidelity
/// over layout) becomes `pre_item_lines`, each on its own re-indented
/// line ahead of the item.
struct LeadingLayout {
    inline_prefix: String,
    forced_break: bool,
    break_inline: String,
    pre_item_lines: Vec<String>,
}

fn layout_leading(leading: &[Comment]) -> LeadingLayout {
    match leading
        .iter()
        .position(|c| matches!(c.kind, CommentKind::Line))
    {
        Some(break_pos) => {
            let mut break_inline = String::new();
            for c in &leading[..break_pos] {
                break_inline.push_str(&normalize_comment_text(&c.text));
                break_inline.push(' ');
            }
            break_inline.push_str(&normalize_comment_text(&leading[break_pos].text));
            let pre_item_lines = leading[break_pos + 1..]
                .iter()
                .map(|c| normalize_comment_text(&c.text))
                .collect();
            LeadingLayout {
                inline_prefix: String::new(),
                forced_break: true,
                break_inline,
                pre_item_lines,
            }
        }
        None => {
            let mut inline_prefix = String::new();
            for c in leading {
                inline_prefix.push_str(&normalize_comment_text(&c.text));
                inline_prefix.push(' ');
            }
            LeadingLayout {
                inline_prefix,
                forced_break: false,
                break_inline: String::new(),
                pre_item_lines: Vec::new(),
            }
        }
    }
}

/// Emits a [`LeadingLayout`]'s forced break: `break_inline` on the
/// current line, a newline, each `pre_item_lines` entry on its own line
/// at `command_col`, then `command_col` spaces — leaving the cursor
/// ready for the item that follows. The caller has already placed
/// whatever separator (`,` + space, or nothing for item 0) belongs
/// before it.
fn emit_forced_break(out: &mut String, layout: &LeadingLayout, command_col: usize) {
    out.push_str(&layout.break_inline);
    out.push('\n');
    for line in &layout.pre_item_lines {
        out.push_str(&" ".repeat(command_col));
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&" ".repeat(command_col));
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

/// Canonical item text (spec "Intra-statement token spacing", full
/// table). Since `parse_cst` only ever hands back the parsed VALUE
/// (never the author's original spacing), this renderer produces the
/// canonical (tight) form for every item shape it covers — spaced-form
/// inputs like `1 : right` or `std :: goToEnd` normalize for free (see
/// `tests` submodule, "Task 8b").
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

    // -- Task 7: comments -------------------------------------------

    #[test]
    fn j_leading_comments_stay_above_the_node_at_its_indent() {
        // A run of blank_before-false own-line comments, immediately
        // above `f`, no blank anywhere — a byte-identical round trip
        // pins both content and (no) reflow.
        let src = "// leading comment stays above f at indent 0\n// a note\nf() {\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn k_trailing_lone_comment_gets_one_space() {
        let src = "f() {\n    right; // go\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn l_trailing_run_aligned_in_source_is_maintained() {
        // `mark;` (code width 9 incl `;`) and `check(1, 2);` (width 16)
        // — the run's alignment column is 16 + 1 = 17, an 8-space pad
        // for `mark` and a 1-space pad for `check`, landing both `//` at
        // the same absolute source column (18). Byte-identical round
        // trip pins alignment maintenance AND idempotence together.
        let src = format!(
            "f() {{\n    mark;{}// a\n    check(1, 2); // b\n}}\n",
            " ".repeat(8)
        );
        assert_eq!(format(&src).unwrap(), src);
    }

    #[test]
    fn m_trailing_run_ragged_in_source_stays_one_space_each() {
        // Both lines have one space in source, but at DIFFERENT absolute
        // columns (`mark;` is shorter) — not author-aligned, so ragged:
        // stays one space each, unchanged.
        let src = "f() {\n    mark; // a\n    check(1, 2); // b\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn n_dangling_comment_before_closing_brace() {
        let src = "f() {\n    right;\n    // dangling\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn o_standalone_comment_keeps_its_blank_separation() {
        let src = "f() {\n    right;\n\n    // standalone\n\n    left;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn p_block_comment_interior_line_reindents_first_line_only() {
        // The comment sits flush-left in source (a leading comment for
        // `right;`, inside the body); its first line moves to the body
        // indent, but the interior line's OWN 3-space indent is untouched
        // (design doc, content fidelity: block comments never reflow).
        let src = "f() {\n/* line one\n   line two */\n    right;\n}\n";
        let expected = "f() {\n    /* line one\n   line two */\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), expected);
    }

    #[test]
    fn q_mid_comma_group_block_comment_stays_inline() {
        assert_eq!(
            format("f() { 1: left, /* mid */ right; }").unwrap(),
            "f() {\n 1: left, /* mid */ right;\n}\n"
        );
    }

    #[test]
    fn r_mid_comma_group_line_comment_forces_a_break() {
        assert_eq!(
            format("f() { left, // note\nright; }").unwrap(),
            "f() {\n    left, // note\n    right;\n}\n"
        );
    }

    /// Every Task-7 source above must be idempotent — the comment-
    /// fidelity harness (`tests/fmt_programs.rs`) pins the same set at
    /// the corpus level; this pins it locally too, one failure per shape.
    #[test]
    fn idempotent_on_commented_shapes() {
        let aligned_run = format!(
            "f() {{\n    mark;{}// a\n    check(1, 2); // b\n}}\n",
            " ".repeat(8)
        );
        for src in [
            "// leading comment stays above f at indent 0\n// a note\nf() {\n    right;\n}\n"
                .to_string(),
            "f() {\n    right; // go\n}\n".to_string(),
            aligned_run,
            "f() {\n    mark; // a\n    check(1, 2); // b\n}\n".to_string(),
            "f() {\n    right;\n    // dangling\n}\n".to_string(),
            "f() {\n    right;\n\n    // standalone\n\n    left;\n}\n".to_string(),
            "f() {\n    /* line one\n   line two */\n    right;\n}\n".to_string(),
            "f() {\n 1: left, /* mid */ right;\n}\n".to_string(),
            "f() {\n    left, // note\n    right;\n}\n".to_string(),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Task 8a: namespaces, blank lines, imports, export verbatim --

    #[test]
    fn s_namespace_prints_at_plus_one_indent() {
        // Brief §A's byte example verbatim.
        let src = "namespace ns {\n    f() {\n        right;\n    }\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn t_namespace_body_feeds_the_deeper_base_indent_into_command_column() {
        // A namespaced function's body indent is 8 (namespace +4, function
        // +4) — the Task-5 `command_column` already treats this as
        // `base_body_indent`; this end-to-end test proves the recursive
        // wiring, not just the pure function (already pinned by
        // `command_column_namespaced_base_indent`). P=2 (`1:`):
        // command_column(2, 8) = max(8, 4) = 8, so the label right-aligns
        // with a 5-space margin (8 - 1 - 2) and `left;` sits at indent 8.
        let src = "namespace ns {\n    f() {\n     1: right;\n        left;\n    }\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn namespace_nesting_recurses_at_increasing_indent() {
        // Brief §A: "namespaces nest" — a namespace inside a namespace,
        // proving `print_top_items`'s recursion (not just a function
        // nested one level deep, covered above).
        let src = "namespace a {\n    namespace b {\n        f() {\n            right;\n        }\n    }\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn u_blank_line_preserved_between_declarations() {
        // Brief §B's byte example verbatim.
        let src = "f() {\n    right;\n}\n\ng() {\n    left;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn v_blank_line_run_collapses_to_one() {
        let src = "f() {\n    right;\n}\n\n\n\ng() {\n    left;\n}\n";
        let expected = "f() {\n    right;\n}\n\ng() {\n    left;\n}\n";
        assert_eq!(format(src).unwrap(), expected);
    }

    #[test]
    fn w_blank_line_suppressed_at_brace_edges() {
        // A blank right after `{` is suppressed (index 0 never gets a
        // blank); a blank right before `}` never reaches the CST at all
        // (no BodyItem follows the last statement to carry it) — both
        // edges land on the same one-liner.
        let src = "f() {\n\n    right;\n\n}\n";
        let expected = "f() {\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), expected);
    }

    #[test]
    fn x_use_list_grouping_and_spacing() {
        // Brief §C's byte example verbatim — one `use` node per statement,
        // never split/merged; `,` tight + one space, `::` tight, ` as `
        // one space each side.
        let src = "use std::goToEnd;\nuse a, b::c as d;\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn y_export_keyword_printed_verbatim_when_written() {
        assert_eq!(
            format("export main() { right; }").unwrap(),
            "export main() {\n    right;\n}\n"
        );
    }

    #[test]
    fn z_bare_main_stays_bare() {
        assert_eq!(
            format("main() { right; }").unwrap(),
            "main() {\n    right;\n}\n"
        );
    }

    #[test]
    fn idempotent_on_task_8a_shapes() {
        for src in [
            "namespace ns {\n    f() {\n        right;\n    }\n}\n".to_string(),
            "namespace ns {\n    f() {\n     1: right;\n        left;\n    }\n}\n".to_string(),
            "namespace a {\n    namespace b {\n        f() {\n            right;\n        }\n    }\n}\n"
                .to_string(),
            "f() {\n    right;\n}\n\ng() {\n    left;\n}\n".to_string(),
            "f() {\n    right;\n}\n\n\n\ng() {\n    left;\n}\n".to_string(),
            "f() {\n\n    right;\n\n}\n".to_string(),
            "use std::goToEnd;\nuse a, b::c as d;\n".to_string(),
            "export main() { right; }".to_string(),
            "main() { right; }".to_string(),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Task 8b: spacing table, spaced-form normalization, hygiene,
    //    edge cases ---------------------------------------------------
    //
    // `parse_cst` only ever hands the printer the parsed VALUE (never the
    // author's original spacing/line-endings), so most of §A/§B fall out
    // of Tasks 4-8a's renderer for free; these tests PIN that rather than
    // changing behaviour. Each is named for its spec-table row / brief
    // bullet, not reusing the single-letter scheme Tasks 5-8a used (that
    // alphabet is specific to their own briefs).

    // §A: full intra-statement spacing table (one test per row).

    #[test]
    fn spacing_table_call() {
        // `@` tight to name (grammar-level, can't even be spaced), name
        // tight to `(`, contents tight: `@f()`, `@f(5)`, `@f(!)`.
        assert_eq!(
            format("f() { @f(); @f(5); @f(!); }").unwrap(),
            "f() {\n    @f();\n    @f(5);\n    @f(!);\n}\n"
        );
    }

    #[test]
    fn spacing_table_builtin_and_successor() {
        // No space before `(`, contents tight; bare form has no parens at
        // all (`FallThrough`, grammar 0.2 forbids empty builtin `()`).
        assert_eq!(
            format("f() { left; left(5); mark(!); }").unwrap(),
            "f() {\n    left;\n    left(5);\n    mark(!);\n}\n"
        );
    }

    #[test]
    fn spacing_table_check() {
        // Tight `(`/`)`, exactly one space after the arm comma, both arm
        // shapes (label, `!`).
        assert_eq!(
            format("f() { check(1, 3); check(!, 1); }").unwrap(),
            "f() {\n    check(1, 3);\n    check(!, 1);\n}\n"
        );
    }

    #[test]
    fn spacing_table_goto() {
        assert_eq!(
            format("f() { goto 5; }").unwrap(),
            "f() {\n    goto 5;\n}\n"
        );
    }

    #[test]
    fn spacing_table_label_single_and_stacked() {
        // Single label `1:` then one space before the command (regression
        // pin, already covered structurally by Task 5's
        // `a_single_inline_label_command_column_4`, isolated here as the
        // pure spacing-table row); stacked `1: 2:` — one space between
        // the two, one space after the final colon (Task 5's
        // `d_stacked_labels_round_up_command_column` pins the same
        // bytes under its command-column-rounding framing).
        assert_eq!(
            format("f() { 1: right; }").unwrap(),
            "f() {\n 1: right;\n}\n"
        );
        assert_eq!(
            format("f() { 1: 2: right; }").unwrap(),
            "f() {\n  1: 2: right;\n}\n"
        );
    }

    #[test]
    fn spacing_table_path() {
        // `::` tight, including a 3-segment path (already-tight source —
        // confirms the canonical form is a pass-through, not just a
        // 2-segment special case).
        assert_eq!(
            format("f() { @std::api::run(); }").unwrap(),
            "f() {\n    @std::api::run();\n}\n"
        );
    }

    #[test]
    fn spacing_table_comma_and_semicolon() {
        // `,` tight to the preceding token, one space after; `;` tight to
        // the preceding token, newline after.
        assert_eq!(
            format("f() { left, right, mark; }").unwrap(),
            "f() {\n    left, right, mark;\n}\n"
        );
    }

    #[test]
    fn spacing_table_as_alias() {
        // `as` (imports): one space each side.
        assert_eq!(
            format("use their::name as alias;").unwrap(),
            "use their::name as alias;\n"
        );
    }

    #[test]
    fn spacing_table_bang() {
        // `!` tight in both positions it can appear: a call/builtin
        // successor and a `check` arm.
        assert_eq!(
            format("f() { @f(!); check(!, 1); }").unwrap(),
            "f() {\n    @f(!);\n    check(!, 1);\n}\n"
        );
    }

    // §B: spaced-form normalization — the grammar accepts extra
    // whitespace around `:` and `::`; the printer always emits the
    // parsed VALUE, so these normalize to tight without any renderer
    // change (pinned, not fixed).

    #[test]
    fn spaced_label_normalizes_to_tight() {
        assert_eq!(
            format("main() { 1 : right; }").unwrap(),
            "main() {\n 1: right;\n}\n"
        );
    }

    #[test]
    fn spaced_path_normalizes_in_import_and_call() {
        assert_eq!(
            format("use std :: goToEnd;").unwrap(),
            "use std::goToEnd;\n"
        );
        assert_eq!(
            format("f() { @std :: goToEnd(); }").unwrap(),
            "f() {\n    @std::goToEnd();\n}\n"
        );
    }

    // §C: textual hygiene.

    #[test]
    fn hygiene_no_trailing_whitespace_even_when_source_has_it() {
        // Trailing whitespace on every source line — the full reprint is
        // CST-driven, not a textual copy, so none of it survives.
        let src = "f() {   \n    right;   \n}   \n";
        let out = format(src).unwrap();
        assert_eq!(out, "f() {\n    right;\n}\n");
        assert!(
            out.lines().all(|l| l == l.trim_end()),
            "trailing whitespace in {out:?}"
        );
    }

    #[test]
    fn hygiene_exactly_one_final_newline_regardless_of_trailing_blanks() {
        // A run of blank lines at the very end of the file has no
        // following item to carry `blank_before` (module doc's
        // "Blank-line presence" note) — it disappears, leaving exactly
        // the one final `\n` the last item already prints.
        assert_eq!(
            format("f() { right; }\n\n\n").unwrap(),
            "f() {\n    right;\n}\n"
        );
    }

    #[test]
    fn hygiene_crlf_and_tabs_reprint_as_lf_and_spaces() {
        // CRLF line endings and a tab-indented body — the full reprint
        // discards ALL input whitespace (indentation is fmt's own, in
        // spaces), so the only surviving shape is the parsed structure.
        let src = "f() {\r\n\tright;\r\n}\r\n";
        assert_eq!(format(src).unwrap(), "f() {\n    right;\n}\n");
    }

    // The three cases above only exercise CODE lines — a comment's own
    // text is raw lexer trivia (module doc's "Comments = trivia-tokens
    // native in the CST"), captured character-for-character from source,
    // so it carries its own trailing whitespace / CRLF independently of
    // anything the renderer decides about layout. `normalize_comment_text`
    // is the fix; these three pin it directly.

    #[test]
    fn hygiene_trailing_whitespace_stripped_from_a_trailing_comment() {
        let src = "f() {\n    right; // note   \n}\n";
        assert_eq!(format(src).unwrap(), "f() {\n    right; // note\n}\n");
    }

    #[test]
    fn hygiene_crlf_stripped_from_a_trailing_comment() {
        // CRLF puts a `\r` right before the `\n` a line comment's capture
        // loop stops at — the token's raw text ends in `\r` unless
        // normalized.
        let src = "f() {\r\n    right; // note\r\n}\r\n";
        let out = format(src).unwrap();
        assert_eq!(out, "f() {\n    right; // note\n}\n");
        assert!(!out.contains('\r'), "CR leaked into {out:?}");
    }

    #[test]
    fn hygiene_crlf_stripped_from_a_block_comment_interior() {
        // A block comment's interior line keeps its LEADING whitespace
        // verbatim (content fidelity) but must still lose a CRLF's
        // trailing `\r` — the two rules coexist because `trim_end` only
        // touches the end of each line.
        let src = "/* a\r\n b */\nf() { right; }";
        let out = format(src).unwrap();
        assert_eq!(out, "/* a\n b */\nf() {\n    right;\n}\n");
        assert!(!out.contains('\r'), "CR leaked into {out:?}");
    }

    // §D: edge cases (spec "Edge cases").

    #[test]
    fn edge_whitespace_only_file_is_one_final_newline() {
        // Complements `empty_file_is_one_final_newline` (literal `""`):
        // a file that is whitespace but has no tokens at all.
        assert_eq!(format("   \n\t\n  \n").unwrap(), "\n");
    }

    #[test]
    fn edge_comments_only_file_reprints_verbatim() {
        // No declarations at all — every item is `TopKind::Comment`;
        // reprints the comments with one final newline.
        let src = "// a\n// b\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn edge_empty_function_body_pin() {
        // Regression pin alongside `empty_function_body_has_no_blank_line`
        // (Task 4): header line + closing brace on its own line, no
        // blank line between.
        assert_eq!(format("f() { }").unwrap(), "f() {\n}\n");
    }

    #[test]
    fn idempotent_on_task_8b_shapes() {
        for src in [
            "f() { @f(); @f(5); @f(!); }".to_string(),
            "f() { left; left(5); mark(!); }".to_string(),
            "f() { check(1, 3); check(!, 1); }".to_string(),
            "f() { goto 5; }".to_string(),
            "f() { 1: 2: right; }".to_string(),
            "f() { @std::api::run(); }".to_string(),
            "use their::name as alias;".to_string(),
            "f() { @f(!); check(!, 1); }".to_string(),
            "main() { 1 : right; }".to_string(),
            "use std :: goToEnd;\nf() { @std :: goToEnd(); }".to_string(),
            "f() {   \n    right;   \n}   \n".to_string(),
            "f() { right; }\n\n\n".to_string(),
            "f() {\r\n\tright;\r\n}\r\n".to_string(),
            "// a\n// b\n".to_string(),
            "f() { }".to_string(),
            "f() {\n    right; // note   \n}\n".to_string(),
            "f() {\r\n    right; // note\r\n}\r\n".to_string(),
            "/* a\r\n b */\nf() { right; }".to_string(),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }
}
