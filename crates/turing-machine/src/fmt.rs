//! `.tmc` pretty-printer — the TM-1 twin of the PM-1 crate's `.pmc`
//! formatter, and a thin renderer in the same sense: [`format`] returns a
//! `Result` and never prints, never touches the filesystem; `cli/fmt.rs` is
//! the only place a diagnostic or a file write happens.
//!
//! # The contract
//!
//! The printer walks the CST ([`crate::parser::parse_cst`]) rather than the
//! flattened AST, which buys the four properties the fmt battery
//! (`tests/fmt_tmc.rs`) proves on every fixture in the repository:
//!
//! - **Canonical** — the output depends on the token stream and on the few
//!   layout choices the CST records (blank-line presence, whether a state was
//!   written on one line), never on the author's spacing.
//! - **Idempotent** — `format(format(s)) == format(s)`. Every layout decision
//!   is either derived from the token content (widths, the line limit) or
//!   from a property the printer's own output preserves.
//! - **Whitespace-only** — no token is added, dropped, or rewritten. A number
//!   reprints from its WRITTEN spelling (leading zeros survive), a glyph
//!   reprints with only the two escapes the lexer accepts, and the bare-name
//!   `goto` sugar stays bare (`Transition::Goto::explicit` is read, never
//!   normalized either way).
//! - **Trivia-preserving** — every comment reprints somewhere: own-line
//!   comments at their block's indent, same-line trailing comments riding
//!   their line, brace-line comments riding the `{`/`}` they were written
//!   on. Doc (`?`) and attention (`!`) runs — `[deprecated]` included —
//!   stay directly above the declaration they document, in source order. A
//!   comment written INSIDE a comma-separated list — an `alphabet` body, a
//!   `routine`/`graph` signature parameter list, a `call`/`graft`/`bind`
//!   binding list, a `with map` pair list, a `use` path list, or a rule's
//!   pattern/`write`/`move` vector — prints where its author wrote it, keyed
//!   to the entry it precedes: a same-line comment rides the preceding
//!   entry's line, an own-line comment keeps its own line, and a comment
//!   after the last entry prints before the closer. A `//` comment forces
//!   such a list onto multiple lines (nothing can follow it on its physical
//!   line), and so does any OWN-LINE comment, block or line (inlining an
//!   own-line comment onto an entry's line would silently flip that flag on
//!   the next parse); a SAME-LINE `/* … */` comment is the one case that can
//!   stay inline instead, EXCEPT in the bracketed `routine`/`graph`
//!   signature, `call`/`graft`/`bind`, and `with map` lists, which have no
//!   inline-with-comments form at all. A pattern/`write`/`move` vector's
//!   SAME-LINE block comment stays inline like an `alphabet` body's or a
//!   `use` path list's does; its `//` comment, and any OWN-LINE comment
//!   (block or line), differ from every other list, because these three
//!   vectors double as the state-block grid's columns (below) — either kind
//!   does not just force its own vector onto several lines, it takes the
//!   WHOLE enclosing rule off the grid, so the rule renders across several
//!   lines without widening the columns its neighbours share
//!   (docs/tmt/fmt.md (interior comments)).
//!
//! # Indentation
//!
//! Two spaces per level, never tabs. (PM-1's `.pmc` printer uses four; a
//! `.tmc` rule commonly sits five levels deep — namespace, namespace,
//! routine, state, rule — where four-space steps would push the transition
//! table off the right margin.)
//!
//! # The state-block grid
//!
//! Within a grid GROUP, a state's rules are laid out as a table: the pattern
//! is padded to the group's widest pattern, so every `->` lands in one
//! column; then the optional action segments — `debugger`, `write [...]`,
//! `move [...]` — each occupy a column sized to the group's widest instance.
//! A group is either one multi-line state's whole rule list (own-line
//! comments and blank lines inside it do NOT split the grid — a state is one
//! table), or a run of adjacent single-line states (see below).
//!
//! A rule pads a column it does not use only when it has content in a LATER
//! column; trailing columns collapse. That is what keeps a bare-transition
//! row tight against the arrow, which is how these tables are written by
//! hand:
//!
//! ```text
//! ['b'] -> write ['a'] move [>] goto scan;
//! ['a'] ->             move [>] goto scan;
//! ['_'] -> stop;
//! ```
//!
//! The transition itself is NOT column-aligned — it is the row's tail, and
//! padding it would leave a ragged gap in every table whose rules mix
//! `write`-only and `write`+`move` actions.
//!
//! A rule whose pattern, `write`, or `move` vector carries a `//` comment, or
//! any OWN-LINE comment (block or line), cannot be a grid row — nothing may
//! follow `//` on its physical line, and an own-line comment must keep its
//! own line, so either way the vector (and the rule around it) renders
//! across several lines instead. Only a SAME-LINE `/* … */` comment stays
//! inline and leaves the rule on the grid. An off-grid rule is excluded from
//! the group's width computation in both directions: it does not consume the
//! group's shared columns, and it does not widen them for its neighbours
//! (docs/tmt/fmt.md (interior comments)).
//!
//! # Single-line states
//!
//! `state done { [*] -> stop; }` stays on one line when the author wrote it
//! that way (all its rules on the header's own line) and it carries no
//! interior comment. A maximal run of adjacent single-line states — no blank
//! line, no doc run, nothing else in between — is one unit: their headers pad
//! to a common width so the `{` column lines up, and their rules share one
//! grid. If any member of the run would cross the line limit, the whole run
//! expands to block form; expansion is stable, since an expanded state is no
//! longer written on one line.
//!
//! # Argument lists and the width threshold
//!
//! The threshold is the **80-column line limit** — the same width
//! `line-too-long` (docs/core.md (assembly lint)) enforces on the two
//! assembly dialects; `.tmc` has no line-length lint of its own, so
//! fmt's active wrapping below is what keeps most lines under it (see
//! "Blank lines and comments", below, for the one mechanism that isn't
//! wrapping — comment alignment — and why it carries no diagnostic
//! cost here). A parenthesized list — a `call`'s bindings, a `graft`/`bind`'s
//! bindings, a `routine`/`graph` signature, an `alphabet` body — renders on
//! one line while the resulting line fits; past that it breaks one entry per
//! line, indented two columns past the construct's FIRST token, with the
//! closing `)`/`}` returning to that token's column:
//!
//! ```text
//! [*] -> call std::binaryNumbersBare::invertNumber(
//!          num = num with map { '^' => '_', '$' => '_' }
//!        ) then return;
//! ```
//!
//! A single binding argument is never broken further — a `with map { … }`
//! stays inline, so one very long binding may still exceed the limit. That is
//! deliberate: the alternative (breaking a map across lines) buys little and
//! costs the map its at-a-glance readability.
//!
//! # Blank lines and comments
//!
//! Blank-line policy is the `.pmc` one: the author's choice is preserved, any
//! run of blank lines collapses to one, and a blank is never forced. The CST
//! records presence as a bool, so the collapse is free; a list's first item
//! never takes a leading blank, which is also what suppresses a blank
//! immediately after `{`.
//!
//! An own-line comment prints at its block's indent, with each of its lines'
//! trailing whitespace stripped (a block comment's interior indentation is
//! content and is left verbatim). A trailing comment sits one space after the
//! code by default; in a run of two or more adjacent single-line entries that
//! all carry one, the comments align one column past the run's widest line —
//! every member aligns, even one whose aligned comment then crosses 80
//! columns. No lint rule flags that: `.tmc` has no line-length rule of its
//! own, and `line-too-long` (docs/core.md (assembly lint)) covers only the
//! two assembly dialects, `.pma` and `.tma`, never `.tmc` — so alignment
//! here carries no diagnostic cost. Unlike `.pmc`'s rule, this does not
//! consult the author's source columns: a run either aligns or it does not,
//! which is both simpler and one less way for a second pass to disagree
//! with the first.

use mtc_core::diagnostics::Span;

use crate::compiler::CompileError;
use crate::cst::{
    AlphabetCst, BindCst, Cst, DocRunItem, DocRunKind, GraftCst, MachineCst, NamespaceCst,
    ReuseCarrier, ReuseCst, RuleCst, RuleItem, RuleKind, StateCst, TapeCst, TopItem, TopKind,
    UseCst, UsePath, WorldItem, WorldKind,
};
use crate::lexer::{Comment, CommentKind, LexMode, Token, TokenKind, lex_with};
use crate::parser::{
    AlphabetElem, BindingArg, BindingValue, Continuation, MapArrow, MoveCell, MoveDir, MoveVec,
    Pattern, PatternCell, PatternCellKind, SigParamKind, Signature, SymLit, SymMap, TermKind,
    Transition, WriteCell, WriteCellKind, WriteVec, parse_cst,
};

/// Spaces per block level (module doc, "Indentation").
const INDENT_UNIT: usize = 2;

/// The line limit every width decision is measured against (module doc,
/// "Argument lists and the width threshold").
const LINE_WIDTH: usize = 80;

/// `.tmc` source → canonical text. Lexes with comments retained, builds the
/// lossless CST, and prints it. A lex or parse error is returned, never
/// printed.
pub fn format(source: &str) -> Result<String, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let cst = parse_cst(&tokens)?;
    Ok(print_cst(&cst, &tokens))
}

fn print_cst(cst: &Cst, tokens: &[Token]) -> String {
    let out = flush(&render_top_items(&cst.items, 0, tokens));
    // An empty file still reprints as exactly one newline; a non-empty one
    // already ends in the last item's newline.
    if out.is_empty() {
        "\n".to_string()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// The emit layer: one rendered item, and a list of them.
// ---------------------------------------------------------------------------

/// One printed item: its text (no trailing newline, possibly several lines),
/// the same-line comment that rides its last line, and whether a blank line
/// precedes the whole thing.
struct Rendered {
    blank_before: bool,
    code: String,
    trailing: Option<Comment>,
}

impl Rendered {
    fn new(blank_before: bool, code: String) -> Self {
        Rendered {
            blank_before,
            code,
            trailing: None,
        }
    }

    fn with_trailing(mut self, trailing: Option<&Comment>) -> Self {
        self.trailing = trailing.cloned();
        self
    }
}

/// Writes a rendered list out, placing blank lines and trailing comments.
fn flush(items: &[Rendered]) -> String {
    let spacing = trailing_spacing(items);
    let mut out = String::new();
    for (i, r) in items.iter().enumerate() {
        if i > 0 && r.blank_before {
            out.push('\n');
        }
        out.push_str(&r.code);
        if let Some(c) = &r.trailing {
            out.push_str(&" ".repeat(spacing[i]));
            out.push_str(&normalize_comment_text(&c.text));
        }
        out.push('\n');
    }
    out
}

/// Spaces between an item's code and its trailing comment (module doc,
/// "Blank lines and comments"): one by default; in a run of two or more
/// adjacent single-line entries that all carry a trailing comment, enough to
/// align them one column past the run's widest code line — every member of
/// the run aligns, even one whose aligned comment then crosses the line
/// limit. No lint rule catches that: `line-too-long` (docs/core.md
/// (assembly lint)) is arch-agnostic ASSEMBLY lint, so it fires on `.pma`
/// and `.tma` but never on `.tmc` — an over-80 `.tmc` line goes unreported
/// by any rule, so alignment here carries no diagnostic cost.
fn trailing_spacing(items: &[Rendered]) -> Vec<usize> {
    let mut spacing = vec![1usize; items.len()];
    let eligible = |r: &Rendered| r.trailing.is_some() && !r.code.contains('\n');
    let mut i = 0;
    while i < items.len() {
        if !eligible(&items[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i + 1;
        while end < items.len() && eligible(&items[end]) && !items[end].blank_before {
            end += 1;
        }
        if end - start >= 2 {
            let align_col = (start..end)
                .map(|k| items[k].code.chars().count())
                .max()
                .expect("the run holds at least two entries")
                + 1;
            for k in start..end {
                let width = items[k].code.chars().count();
                spacing[k] = align_col - width;
            }
        }
        i = end;
    }
    spacing
}

/// Strips every line's trailing whitespace from a comment's raw text (a line
/// comment's trailing spaces, or a CRLF source line's `\r`, ride the token
/// verbatim otherwise). Interior LEADING whitespace of a block comment is
/// content and is untouched.
fn normalize_comment_text(text: &str) -> String {
    text.split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn comment_line(comment: &Comment, indent: usize) -> String {
    format!(
        "{}{}",
        " ".repeat(indent),
        normalize_comment_text(&comment.text)
    )
}

/// A `{`'s same-line comments, ready to append to the header line. More than
/// one is only possible for a run of block comments (a line comment eats the
/// rest of its physical line).
fn open_trailing_text(comments: &[Comment]) -> String {
    if comments.is_empty() {
        return String::new();
    }
    let texts: Vec<String> = comments
        .iter()
        .map(|c| normalize_comment_text(&c.text))
        .collect();
    format!(" {}", texts.join(" "))
}

/// One list's interior comments, bucketed per slot. `slots` has one entry
/// per position `0..=entry_count`; the last bucket is the tail slot, printed
/// before the closer (docs/tmt/fmt.md (interior comments)).
struct Interior<'a> {
    slots: Vec<Vec<&'a Comment>>,
    /// A LINE comment anywhere in the list forces it multi-line — nothing
    /// can follow `//` on its physical line.
    forces_break: bool,
}

impl Interior<'_> {
    fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_empty())
    }
}

/// Buckets `interior` by slot. An index past `entry_count` is a bug in the
/// parser's bookkeeping; in release it clamps to the tail slot, because a
/// misplaced comment is recoverable and a dropped one is data loss.
fn bucket(interior: &[(usize, Comment)], entry_count: usize) -> Interior<'_> {
    let mut slots: Vec<Vec<&Comment>> = vec![Vec::new(); entry_count + 1];
    let mut forces_break = false;
    for (index, comment) in interior {
        debug_assert!(
            *index <= entry_count,
            "interior comment index {index} exceeds entry count {entry_count}"
        );
        // A LINE comment forces a break because nothing can follow `//` on
        // its physical line; an own-line comment forces one for a different
        // reason — inlining it onto an entry's line would silently flip its
        // own `own_line` flag from true to false on the next parse.
        if matches!(comment.kind, CommentKind::Line) || comment.own_line {
            forces_break = true;
        }
        slots[(*index).min(entry_count)].push(comment);
    }
    Interior {
        slots,
        forces_break,
    }
}

/// Own-line comments for one slot, each on its own line at `indent`.
fn interior_lines(comments: &[&Comment], indent: usize) -> String {
    let mut out = String::new();
    for c in comments.iter().filter(|c| c.own_line) {
        out.push_str(&comment_line(c, indent));
        out.push('\n');
    }
    out
}

/// The same-line (trailing) comments for one slot, ready to append after a
/// separator. Empty when the slot has only own-line comments.
fn interior_trailing(comments: &[&Comment]) -> String {
    let texts: Vec<String> = comments
        .iter()
        .filter(|c| !c.own_line)
        .map(|c| normalize_comment_text(&c.text))
        .collect();
    if texts.is_empty() {
        String::new()
    } else {
        format!(" {}", texts.join(" "))
    }
}

// ---------------------------------------------------------------------------
// Doc/attention runs.
// ---------------------------------------------------------------------------

/// A declaration's `?`/`!` run, printed at the declaration's own indent, one
/// canonical space after the sigil. Returns the lines (each newline-
/// terminated) or the empty string; `blank_before_decl` is the wrapping
/// item's repurposed `blank_before` — the gap between the run and the
/// declaration it documents.
fn doc_run_text(run: &[DocRunItem], indent: usize, blank_before_decl: bool) -> String {
    if run.is_empty() {
        return String::new();
    }
    let pad = " ".repeat(indent);
    let mut out = String::new();
    for (i, item) in run.iter().enumerate() {
        if i > 0 && item.blank_before {
            out.push('\n');
        }
        match &item.kind {
            DocRunKind::Doc { text, .. } => out.push_str(&doc_line(&pad, '?', text)),
            DocRunKind::Attention { text, .. } => out.push_str(&doc_line(&pad, '!', text)),
            DocRunKind::Comment(c) => {
                out.push_str(&comment_line(c, indent));
                out.push('\n');
            }
        }
    }
    if blank_before_decl {
        out.push('\n');
    }
    out
}

fn doc_line(pad: &str, sigil: char, text: &str) -> String {
    if text.is_empty() {
        format!("{pad}{sigil}\n")
    } else {
        format!("{pad}{sigil} {text}\n")
    }
}

/// Whether an item leads with a blank line. A documented declaration
/// repurposes its own `blank_before` for the run→declaration gap, so the
/// blank-before-the-whole-unit decision moves to the run's first line.
fn leads_with_blank(blank_before: bool, doc_run: &[DocRunItem]) -> bool {
    match doc_run.first() {
        Some(first) => first.blank_before,
        None => blank_before,
    }
}

// ---------------------------------------------------------------------------
// Token-level text.
// ---------------------------------------------------------------------------

/// A glyph literal, re-escaped exactly as far as the lexer requires: only `'`
/// and `\` ever take a backslash, so the reprint re-lexes to the same value.
fn glyph_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('\'');
    out
}

/// A symbol literal. A number prints its WRITTEN digits (leading zeros
/// included) — the printer never re-derives a token from a parsed value.
fn sym_text(sym: &SymLit) -> String {
    match sym {
        SymLit::Glyph { value, .. } => glyph_text(value),
        SymLit::Number { written, .. } => written.clone(),
    }
}

fn alphabet_elem_text(elem: &AlphabetElem) -> String {
    match elem {
        AlphabetElem::Single(sym) => sym_text(sym),
        AlphabetElem::Range { lo, hi, .. } => format!("{}..{}", sym_text(lo), sym_text(hi)),
    }
}

/// A pattern vector on the grid (or single-line-inline) path: `interior`
/// carries only BLOCK comments here — a LINE comment sends the rule down
/// [`render_rule_off_grid`] before this is ever called.
fn pattern_text(pattern: &Pattern, interior: &Interior<'_>) -> String {
    let cells: Vec<String> = pattern.cells.iter().map(pattern_cell_text).collect();
    format!("[{}]", join_cells_with_interior(&cells, interior))
}

/// Joins rendered cells with `, `, splicing each slot's BLOCK comments
/// inline. Slot `i`'s comments precede cell `i`; the tail slot's precede
/// the closing bracket. Only reached when no LINE comment is present —
/// the caller has already sent that case down the multi-line path.
fn join_cells_with_interior(cells: &[String], interior: &Interior<'_>) -> String {
    let mut out = String::new();
    for (i, cell) in cells.iter().enumerate() {
        for c in interior.slots[i].iter() {
            out.push_str(&normalize_comment_text(&c.text));
            out.push(' ');
        }
        out.push_str(cell);
        if i + 1 < cells.len() {
            out.push_str(", ");
        }
    }
    for c in interior.slots[cells.len()].iter() {
        out.push(' ');
        out.push_str(&normalize_comment_text(&c.text));
    }
    out
}

fn pattern_cell_text(cell: &PatternCell) -> String {
    let mut out = match &cell.kind {
        PatternCellKind::Wildcard => "*".to_string(),
        PatternCellKind::Single(sym) => sym_text(sym),
        PatternCellKind::Range { lo, hi } => format!("{}..{}", sym_text(lo), sym_text(hi)),
    };
    if let Some(binding) = &cell.binding {
        out.push_str(" as ");
        out.push_str(&binding.name);
    }
    out
}

fn write_cell_text(cell: &WriteCell, tokens: &[Token]) -> String {
    match &cell.kind {
        WriteCellKind::Keep => "-".to_string(),
        WriteCellKind::Lit(sym) => sym_text(sym),
        WriteCellKind::Subst { expr } => format!("{{{}}}", subst_body_text(&expr.span, tokens)),
    }
}

/// A write vector on the grid (or single-line-inline) path — see
/// [`pattern_text`]'s note on what `interior` carries here.
fn write_vec_text(vec: &WriteVec, tokens: &[Token], interior: &Interior<'_>) -> String {
    let cells: Vec<String> = vec
        .cells
        .iter()
        .map(|cell| write_cell_text(cell, tokens))
        .collect();
    format!("write [{}]", join_cells_with_interior(&cells, interior))
}

/// Reprint a substitution `{…}` from its SOURCE TOKENS, tight (no interior
/// whitespace), rather than re-deriving from the parsed tree. This keeps the
/// formatter whitespace-only (docs/tmt/fmt.md (whitespace-only)): the source's
/// own parenthesization survives (`{(v*2)+1}` is not rewritten to `{v*2+1}`),
/// and a number reprints from its written spelling. `span` is the expression's
/// span (braces excluded); the significant tokens lying within it are exactly
/// the expression, comments having been split off before the grammar walk, so
/// concatenating their spellings yields the tight form.
fn subst_body_text(span: &Span, tokens: &[Token]) -> String {
    tokens
        .iter()
        .filter(|t| {
            !matches!(t.kind, TokenKind::Comment(_))
                && t.span().start >= span.start
                && t.span().end <= span.end
        })
        .map(|t| fold_token_text(&t.kind))
        .collect()
}

/// The source spelling of one write-cell fold-expression token. Only the fold
/// grammar's tokens (`+ - * %`, parens, names, integers) fall within a
/// substitution's span, so nothing else is reachable here.
fn fold_token_text(kind: &TokenKind) -> &str {
    match kind {
        TokenKind::Ident(s) => s,
        TokenKind::Number(_, spelling) => spelling,
        TokenKind::Plus => "+",
        TokenKind::Dash => "-",
        TokenKind::Star => "*",
        TokenKind::Percent => "%",
        TokenKind::LParen => "(",
        TokenKind::RParen => ")",
        _ => "",
    }
}

fn move_cell_text(cell: &MoveCell) -> String {
    match cell.dir {
        MoveDir::Left => "<",
        MoveDir::Right => ">",
        MoveDir::Stay => ".",
    }
    .to_string()
}

/// A move vector on the grid (or single-line-inline) path — see
/// [`pattern_text`]'s note on what `interior` carries here.
fn move_vec_text(vec: &MoveVec, interior: &Interior<'_>) -> String {
    let cells: Vec<String> = vec.cells.iter().map(move_cell_text).collect();
    format!("move [{}]", join_cells_with_interior(&cells, interior))
}

/// One binding argument, at the column it will print from — needed only to
/// hand a nested (and possibly broken) `with map` the column its own closing
/// `}` must return to.
fn binding_arg_text(arg: &BindingArg, col: usize, map_interior: &Interior<'_>) -> String {
    let value_col = col + arg.name.chars().count() + 3; // "NAME = "
    format!(
        "{} = {}",
        arg.name,
        binding_value_text(&arg.value, value_col, map_interior)
    )
}

fn binding_value_text(value: &BindingValue, col: usize, map_interior: &Interior<'_>) -> String {
    match value {
        BindingValue::Named { target, map, .. } => match map {
            Some(map) => {
                let map_col = col + target.chars().count() + 1; // "TARGET "
                format!("{target} {}", sym_map_text(map, map_col, map_interior))
            }
            None => target.clone(),
        },
        BindingValue::Terminator { kind, .. } => term_text(*kind).to_string(),
    }
}

/// A `with map { … }`, starting at column `col` (where the `w` of `with`
/// lands) — the column its closing `}` returns to once broken. `interior` is
/// this map's OWN interior comments, one level down from the binding list's
/// (module doc, "Argument lists and the width threshold"; docs/tmt/fmt.md
/// (interior comments)).
fn sym_map_text(map: &SymMap, col: usize, interior: &Interior<'_>) -> String {
    let pairs: Vec<String> = map
        .pairs
        .iter()
        .map(|pair| {
            let arrow = match pair.arrow {
                MapArrow::Bidirectional => "->",
                MapArrow::ReadOnly => "=>",
            };
            format!("{} {arrow} {}", sym_text(&pair.src), sym_text(&pair.dst))
        })
        .collect();
    if interior.is_empty() {
        return format!("with map {{ {} }}", pairs.join(", "));
    }
    let entry_pad = " ".repeat(col + INDENT_UNIT);
    // Slot 0's same-line comments precede every pair, so there is no
    // preceding pair's line for them to trail — they ride the opening `{`
    // itself (module doc, "Blank lines and comments").
    let mut out = String::from("with map {");
    out.push_str(&interior_trailing(&interior.slots[0]));
    out.push('\n');
    for (i, pair) in pairs.iter().enumerate() {
        out.push_str(&interior_lines(&interior.slots[i], col + INDENT_UNIT));
        out.push_str(&entry_pad);
        out.push_str(pair);
        if i + 1 < pairs.len() {
            out.push(',');
        }
        // The NEXT slot's same-line comments belong to THIS pair's line —
        // see the indexing rule (module doc, "Blank lines and comments").
        out.push_str(&interior_trailing(&interior.slots[i + 1]));
        out.push('\n');
    }
    out.push_str(&interior_lines(
        &interior.slots[pairs.len()],
        col + INDENT_UNIT,
    ));
    out.push_str(&" ".repeat(col));
    out.push('}');
    out
}

/// The pair count of a binding argument's map, or 0 when it carries none —
/// what [`bucket`] needs to size a map's own slot list.
fn map_pair_count(value: &BindingValue) -> usize {
    match value {
        BindingValue::Named { map: Some(m), .. } => m.pairs.len(),
        _ => 0,
    }
}

/// One binding argument's `with map` interior comments, filtered out of a
/// binding list's flat `(arg index, pair index, comment)` side-car and
/// re-keyed to the plain `(pair index, comment)` shape [`bucket`] expects
/// (docs/tmt/fmt.md (interior comments)).
fn map_interior_for(
    map_pairs: &[(usize, usize, Comment)],
    arg_index: usize,
) -> Vec<(usize, Comment)> {
    map_pairs
        .iter()
        .filter(|(ai, _, _)| *ai == arg_index)
        .map(|(_, pair_index, comment)| (*pair_index, comment.clone()))
        .collect()
}

/// A binding list's entries, each rendered from column `entry_col` (the
/// column a broken list's own entries — and so a broken map nested inside
/// one — line up at), pulling each argument's own `with map` interior
/// comments out of the list's flat side-car.
fn binding_entries(
    args: &[BindingArg],
    entry_col: usize,
    map_pairs: &[(usize, usize, Comment)],
) -> Vec<String> {
    args.iter()
        .enumerate()
        .map(|(i, arg)| {
            let filtered = map_interior_for(map_pairs, i);
            let map_interior = bucket(&filtered, map_pair_count(&arg.value));
            binding_arg_text(arg, entry_col, &map_interior)
        })
        .collect()
}

fn term_text(kind: TermKind) -> &'static str {
    match kind {
        TermKind::Return => "return",
        TermKind::Stop => "stop",
        TermKind::Halt => "halt",
    }
}

fn continuation_text(cont: &Continuation) -> String {
    match cont {
        Continuation::State { name, .. } => name.clone(),
        Continuation::Return { .. } => "return".to_string(),
        Continuation::Stop { .. } => "stop".to_string(),
        Continuation::Halt { .. } => "halt".to_string(),
    }
}

fn signature_params(sig: &Signature) -> Vec<String> {
    sig.params
        .iter()
        .map(|param| match &param.kind {
            SigParamKind::Tape { alphabet, .. } => {
                format!("tape {}: {alphabet}", param.name)
            }
            SigParamKind::State => format!("state {}", param.name),
        })
        .collect()
}

/// `head(entries)tail` on one line while it fits from column `col`, else one
/// entry per line (module doc, "Argument lists and the width threshold").
/// `head` starts AT `col` and never carries the leading indent itself — a
/// caller opening a line emits that indent before calling. `interior` is the
/// list's interior comments, bucketed by [`bucket`]; a caller with no such
/// list passes `&bucket(&[], entries.len())`. An entry that already spans
/// several physical lines (a binding argument whose own nested `with map`
/// broke on an interior comment) forces the list to break too — the
/// alternative would splice that entry's own newlines into what the width
/// check believes is one line, with no indent for the continuation.
fn paren_list(
    col: usize,
    head: &str,
    entries: &[String],
    tail: &str,
    interior: &Interior<'_>,
) -> String {
    let one_line = format!("{head}({}){tail}", entries.join(", "));
    let has_multiline_entry = entries.iter().any(|e| e.contains('\n'));
    if (entries.is_empty() || col + one_line.chars().count() <= LINE_WIDTH)
        && interior.is_empty()
        && !has_multiline_entry
    {
        return one_line;
    }
    let entry_pad = " ".repeat(col + INDENT_UNIT);
    // Slot 0's same-line comments precede every entry, so there is no
    // preceding entry's line for them to trail — they ride the opening `(`
    // itself (module doc, "Blank lines and comments").
    let mut out = format!("{head}(");
    out.push_str(&interior_trailing(&interior.slots[0]));
    out.push('\n');
    for (i, entry) in entries.iter().enumerate() {
        out.push_str(&interior_lines(&interior.slots[i], col + INDENT_UNIT));
        out.push_str(&entry_pad);
        out.push_str(entry);
        if i + 1 < entries.len() {
            out.push(',');
        }
        // The NEXT slot's same-line comments belong to THIS entry's line —
        // see the indexing rule (module doc, "Blank lines and comments").
        out.push_str(&interior_trailing(&interior.slots[i + 1]));
        out.push('\n');
    }
    out.push_str(&interior_lines(
        &interior.slots[entries.len()],
        col + INDENT_UNIT,
    ));
    out.push_str(&" ".repeat(col));
    out.push(')');
    out.push_str(tail);
    out
}

// ---------------------------------------------------------------------------
// The rule grid.
// ---------------------------------------------------------------------------

/// The column widths one grid group shares (module doc, "The state-block
/// grid"). A width of zero means the group has no rule using that segment, so
/// the column does not exist at all.
struct Grid {
    pattern: usize,
    debugger: usize,
    write: usize,
    mov: usize,
}

impl RuleCst {
    /// True when any glyph vector carries a LINE comment or an OWN-LINE
    /// comment (mirrors `bucket`'s `forces_break`). A LINE comment cannot
    /// share its physical line; an own-line comment, block or line, would
    /// silently flip its own `own_line` flag on the next parse if inlined
    /// onto a cell's line. Either way the rule cannot be a grid row, so it
    /// renders multi-line and is excluded from the grid's width computation
    /// (docs/tmt/fmt.md (interior comments)).
    fn breaks_the_grid(&self) -> bool {
        [&self.pattern_cells, &self.write_cells, &self.move_cells]
            .iter()
            .any(|v| {
                v.iter()
                    .any(|(_, c)| matches!(c.kind, CommentKind::Line) || c.own_line)
            })
    }
}

/// `rules` off the grid (module doc, "The state-block grid") are excluded
/// from every column: they consume none of the group's width and, since
/// they render themselves, do not need one.
fn grid_for(rules: &[&RuleCst], tokens: &[Token]) -> Grid {
    let width = |s: &str| s.chars().count();
    let on_grid: Vec<&RuleCst> = rules
        .iter()
        .copied()
        .filter(|rc| !rc.breaks_the_grid())
        .collect();
    Grid {
        pattern: on_grid
            .iter()
            .map(|rc| {
                let interior = bucket(&rc.pattern_cells, rc.rule.pattern.cells.len());
                width(&pattern_text(&rc.rule.pattern, &interior))
            })
            .max()
            .unwrap_or(0),
        debugger: if on_grid.iter().any(|rc| rc.rule.debugger) {
            "debugger".len()
        } else {
            0
        },
        write: on_grid
            .iter()
            .filter_map(|rc| {
                rc.rule.write.as_ref().map(|w| {
                    let interior = bucket(&rc.write_cells, w.cells.len());
                    width(&write_vec_text(w, tokens, &interior))
                })
            })
            .max()
            .unwrap_or(0),
        mov: on_grid
            .iter()
            .filter_map(|rc| {
                rc.rule.mov.as_ref().map(|m| {
                    let interior = bucket(&rc.move_cells, m.cells.len());
                    width(&move_vec_text(m, &interior))
                })
            })
            .max()
            .unwrap_or(0),
    }
}

/// One rule as a grid row: `indent`, the padded pattern, the arrow, the
/// action columns, the transition, `;`. Delegates to
/// [`render_rule_off_grid`] first when [`RuleCst::breaks_the_grid`] is true
/// — such a rule ignores `grid` entirely, since it was excluded from the
/// computation that produced it.
fn render_rule(rc: &RuleCst, grid: &Grid, indent: usize, tokens: &[Token]) -> String {
    if rc.breaks_the_grid() {
        return render_rule_off_grid(rc, indent, tokens);
    }
    let rule = &rc.rule;
    let mut line = " ".repeat(indent);
    let pattern_interior = bucket(&rc.pattern_cells, rule.pattern.cells.len());
    let pattern = pattern_text(&rule.pattern, &pattern_interior);
    let pattern_width = pattern.chars().count();
    line.push_str(&pattern);
    line.push_str(&" ".repeat(grid.pattern.saturating_sub(pattern_width)));
    line.push_str(" -> ");

    let write_text = match &rule.write {
        Some(w) => write_vec_text(w, tokens, &bucket(&rc.write_cells, w.cells.len())),
        None => String::new(),
    };
    let move_text = match &rule.mov {
        Some(m) => move_vec_text(m, &bucket(&rc.move_cells, m.cells.len())),
        None => String::new(),
    };
    let segments: [(bool, String, usize); 3] = [
        (rule.debugger, "debugger".to_string(), grid.debugger),
        (rule.write.is_some(), write_text, grid.write),
        (rule.mov.is_some(), move_text, grid.mov),
    ];
    // Trailing columns collapse: padding exists only to line up what comes
    // AFTER it, so a rule pads a column it skips (and its own last column is
    // never padded) exactly while a later segment still has to be reached.
    let last_used = segments.iter().rposition(|(present, _, _)| *present);
    for (i, (present, text, column)) in segments.iter().enumerate() {
        match last_used {
            Some(last) if i < last => {
                if *column == 0 {
                    continue;
                }
                if *present {
                    line.push_str(text);
                    line.push_str(&" ".repeat(column - text.chars().count()));
                } else {
                    line.push_str(&" ".repeat(*column));
                }
                line.push(' ');
            }
            Some(last) if i == last => {
                line.push_str(text);
                line.push(' ');
            }
            _ => {}
        }
    }

    let col = line.chars().count();
    let transition = transition_text(&rule.transition, col, &rc.call_args, &rc.map_pairs);
    if transition.is_empty() {
        // Omitted transition: no token to print. Trim the trailing space the
        // action segments left so the `;` abuts the last action.
        while line.ends_with(' ') {
            line.pop();
        }
    } else {
        line.push_str(&transition);
    }
    line.push(';');
    line
}

/// A rule off the grid (module doc, "The state-block grid"): a LINE comment
/// in one of its glyph vectors forces it there, and the whole rule renders
/// across several lines instead of padding to the group's shared columns —
/// every vector it carries breaks, not only the one with the comment, so
/// the rule reads as one consistent shape rather than a mix of broken and
/// padded segments.
fn render_rule_off_grid(rc: &RuleCst, indent: usize, tokens: &[Token]) -> String {
    let rule = &rc.rule;
    let mut line = " ".repeat(indent);

    let pattern_cells: Vec<String> = rule.pattern.cells.iter().map(pattern_cell_text).collect();
    let pattern_interior = bucket(&rc.pattern_cells, pattern_cells.len());
    line.push_str(&glyph_vec_multiline(
        "[",
        &pattern_cells,
        &pattern_interior,
        indent,
    ));
    line.push_str(" -> ");

    if rule.debugger {
        line.push_str("debugger ");
    }
    if let Some(w) = &rule.write {
        let cells: Vec<String> = w.cells.iter().map(|c| write_cell_text(c, tokens)).collect();
        let interior = bucket(&rc.write_cells, cells.len());
        line.push_str(&glyph_vec_multiline("write [", &cells, &interior, indent));
        line.push(' ');
    }
    if let Some(m) = &rule.mov {
        let cells: Vec<String> = m.cells.iter().map(move_cell_text).collect();
        let interior = bucket(&rc.move_cells, cells.len());
        line.push_str(&glyph_vec_multiline("move [", &cells, &interior, indent));
        line.push(' ');
    }

    let col = col_after(&line);
    let transition = transition_text(&rule.transition, col, &rc.call_args, &rc.map_pairs);
    if transition.is_empty() {
        while line.ends_with(' ') {
            line.pop();
        }
    } else {
        line.push_str(&transition);
    }
    line.push(';');
    line
}

/// One glyph vector's cells, one per line at `indent + INDENT_UNIT`, closer
/// on its own line back at `indent` — the off-grid form
/// [`RuleCst::breaks_the_grid`] sends a rule down. `head` is the vector's
/// leading text up to and including its opening `[` (`"["` for a pattern,
/// `"write ["` / `"move ["` for the action vectors). Mirrors `paren_list`'s
/// multi-line branch and the same indexing rule as every other list: slot
/// `i`'s own-line comments print above cell `i`; slot `i + 1`'s same-line
/// comments print at the end of cell `i`'s line (docs/tmt/fmt.md (interior
/// comments)).
fn glyph_vec_multiline(
    head: &str,
    cells: &[String],
    interior: &Interior<'_>,
    indent: usize,
) -> String {
    let cell_pad = " ".repeat(indent + INDENT_UNIT);
    let mut out = String::from(head);
    // Slot 0's same-line comments precede every cell, so there is no
    // preceding cell's line for them to trail — they ride the opening `[`
    // itself (module doc, "Blank lines and comments").
    out.push_str(&interior_trailing(&interior.slots[0]));
    out.push('\n');
    for (i, cell) in cells.iter().enumerate() {
        out.push_str(&interior_lines(&interior.slots[i], indent + INDENT_UNIT));
        out.push_str(&cell_pad);
        out.push_str(cell);
        if i + 1 < cells.len() {
            out.push(',');
        }
        // The NEXT slot's same-line comments belong to THIS cell's line —
        // see the indexing rule above (module doc, "Blank lines and
        // comments").
        out.push_str(&interior_trailing(&interior.slots[i + 1]));
        out.push('\n');
    }
    out.push_str(&interior_lines(
        &interior.slots[cells.len()],
        indent + INDENT_UNIT,
    ));
    out.push_str(&" ".repeat(indent));
    out.push(']');
    out
}

/// The column position right after the LAST physical line of `s` — what a
/// nested list beginning immediately after `s` breaks against, when `s`
/// itself may already contain embedded newlines (an off-grid rule's broken
/// glyph vectors, e.g.). Degrades to `s.chars().count()` when `s` has none.
fn col_after(s: &str) -> usize {
    match s.rsplit_once('\n') {
        Some((_, last)) => last.chars().count(),
        None => s.chars().count(),
    }
}

/// A transition, starting at column `col` — the column an argument list
/// breaks against. `call_args`/`map_pairs` are the enclosing [`RuleCst`]'s
/// side-cars (empty for every non-`call` transition).
fn transition_text(
    transition: &Transition,
    col: usize,
    call_args: &[(usize, Comment)],
    map_pairs: &[(usize, usize, Comment)],
) -> String {
    match transition {
        Transition::Goto { name, explicit, .. } => {
            if *explicit {
                format!("goto {name}")
            } else {
                name.clone()
            }
        }
        Transition::Call {
            target, args, then, ..
        } => {
            let entry_col = col + INDENT_UNIT;
            let entries = binding_entries(args, entry_col, map_pairs);
            let head = format!("call {}", target.joined());
            // The `;` the caller appends is reserved by rendering it into the
            // tail used for the fit measurement.
            let tail = format!(" then {};", continuation_text(then));
            let rendered = paren_list(
                col,
                &head,
                &entries,
                &tail,
                &bucket(call_args, entries.len()),
            );
            rendered
                .strip_suffix(';')
                .expect("the tail ends in the reserved `;`")
                .to_string()
        }
        Transition::Return { .. } => "return".to_string(),
        Transition::Stop { .. } => "stop".to_string(),
        Transition::Halt { .. } => "halt".to_string(),
        // An omitted transition prints nothing — the `;` abuts the last action
        // (the caller trims the trailing action space).
        Transition::Stay { .. } => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Top-level items.
// ---------------------------------------------------------------------------

fn render_top_items(items: &[TopItem], indent: usize, tokens: &[Token]) -> Vec<Rendered> {
    items
        .iter()
        .map(|item| render_top_item(item, indent, tokens))
        .collect()
}

fn render_top_item(item: &TopItem, indent: usize, tokens: &[Token]) -> Rendered {
    match &item.kind {
        TopKind::Comment(c) => Rendered::new(item.blank_before, comment_line(c, indent)),
        TopKind::Import(u) => render_use(u, item.blank_before, indent),
        TopKind::Alphabet(a) => render_alphabet(a, item.blank_before, indent),
        TopKind::Namespace(ns) => render_namespace(ns, item.blank_before, indent, tokens),
        TopKind::Reuse(r) => render_reuse(r, item.blank_before, indent, tokens),
        TopKind::Machine(m) => render_machine(m, item.blank_before, indent, tokens),
    }
}

fn render_use(u: &UseCst, blank_before: bool, indent: usize) -> Rendered {
    let paths: Vec<String> = u.paths.iter().map(use_path_text).collect();
    let interior = bucket(&u.interior, paths.len());
    let pad = " ".repeat(indent);
    let code = if interior.is_empty() {
        format!("{pad}use {};", paths.join(", "))
    } else if !interior.forces_break {
        // Block-only interior comments stay inline, each before its entry —
        // same treatment as `render_alphabet`'s inline branch.
        let mut line = format!("{pad}use ");
        for (i, entry) in paths.iter().enumerate() {
            for c in interior.slots[i].iter() {
                line.push_str(&normalize_comment_text(&c.text));
                line.push(' ');
            }
            line.push_str(entry);
            if i + 1 < paths.len() {
                line.push_str(", ");
            }
        }
        for c in interior.slots[paths.len()].iter() {
            line.push(' ');
            line.push_str(&normalize_comment_text(&c.text));
        }
        line.push(';');
        line
    } else {
        // A LINE comment forces the path list onto multiple lines, each
        // continuation aligned 4 columns past the statement indent — the
        // column right past `use ` (module doc, "Argument lists and the
        // width threshold").
        let cont_pad = " ".repeat(indent + 4);
        let mut out = String::new();
        out.push_str(&pad);
        out.push_str("use");
        // Slot 0's comments have no preceding entry to trail, so they ride
        // AFTER the `use` keyword instead of before it — printing them
        // before `use` would reorder the token stream (mirrors the sibling
        // crate's `print_use`). A same-line comment rides the `use` line
        // itself; an own-line comment prints on its own continuation line
        // below. A LINE comment eats the rest of its physical line, so the
        // first path moves to a fresh continuation line whenever this slot
        // is non-empty (module doc, "Blank lines and comments").
        let slot0_trailing = interior_trailing(&interior.slots[0]);
        let slot0_lines = interior_lines(&interior.slots[0], indent + 4);
        if !slot0_trailing.is_empty() {
            out.push_str(&slot0_trailing);
            out.push('\n');
        } else if slot0_lines.is_empty() {
            out.push(' ');
        }
        if !slot0_lines.is_empty() {
            if slot0_trailing.is_empty() {
                out.push('\n');
            }
            out.push_str(&slot0_lines);
        }
        if !slot0_trailing.is_empty() || !slot0_lines.is_empty() {
            out.push_str(&cont_pad);
        }
        for (i, entry) in paths.iter().enumerate() {
            if i > 0 {
                out.push('\n');
                out.push_str(&interior_lines(&interior.slots[i], indent + 4));
                out.push_str(&cont_pad);
            }
            out.push_str(entry);
            if i + 1 < paths.len() {
                out.push(',');
            }
            // The NEXT slot's same-line comments belong to THIS entry's
            // line — see the indexing rule (module doc, "Blank lines and
            // comments").
            out.push_str(&interior_trailing(&interior.slots[i + 1]));
        }
        let tail_lines = interior_lines(&interior.slots[paths.len()], indent + 4);
        // A same-line LINE comment on the tail slot was already printed on
        // the last entry's own line above (`interior_trailing`, drawn from
        // this same slot); `tail_lines` only sees own-line comments, so it
        // stays empty in that case even though `;` cannot follow `//` on
        // that physical line — check the raw slot too before deciding the
        // closer rides the last entry's line unchanged.
        let tail_same_line_forces_break = interior.slots[paths.len()]
            .iter()
            .any(|c| !c.own_line && matches!(c.kind, CommentKind::Line));
        if tail_lines.is_empty() && !tail_same_line_forces_break {
            out.push(';');
        } else {
            out.push('\n');
            out.push_str(&tail_lines);
            out.push_str(&pad);
            out.push(';');
        }
        out
    };
    Rendered::new(blank_before, code).with_trailing(u.trailing.as_ref())
}

fn use_path_text(path: &UsePath) -> String {
    let mut out = path.path.join("::");
    if let Some(alias) = &path.alias {
        out.push_str(" as ");
        out.push_str(alias);
    }
    out
}

fn render_alphabet(a: &AlphabetCst, blank_before: bool, indent: usize) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(&a.doc_run, indent, blank_before);
    let head = format!(
        "{pad}{}alphabet {}",
        if a.exported { "export " } else { "" },
        a.name
    );
    let entries: Vec<String> = a.elems.iter().map(alphabet_elem_text).collect();
    let interior = bucket(&a.interior, a.elems.len());
    let one_line = format!("{head} {{ {} }}", entries.join(", "));
    // A comment on the `{`, any LINE comment inside the body, or any
    // own-line comment inside the body forces the body onto its own lines
    // whatever the width says (`bucket`'s `forces_break`).
    if a.open_trailing.is_empty() && interior.is_empty() && one_line.chars().count() <= LINE_WIDTH {
        code.push_str(&one_line);
    } else if a.open_trailing.is_empty()
        && !interior.forces_break
        && one_line.chars().count() <= LINE_WIDTH
    {
        // Block-only interior comments stay inline, each before its entry.
        let mut line = format!("{head} {{ ");
        for (i, entry) in entries.iter().enumerate() {
            for c in interior.slots[i].iter() {
                line.push_str(&normalize_comment_text(&c.text));
                line.push(' ');
            }
            line.push_str(entry);
            if i + 1 < entries.len() {
                line.push_str(", ");
            }
        }
        for c in interior.slots[entries.len()].iter() {
            line.push(' ');
            line.push_str(&normalize_comment_text(&c.text));
        }
        line.push_str(" }");
        code.push_str(&line);
    } else {
        code.push_str(&head);
        code.push_str(" {");
        code.push_str(&open_trailing_text(&a.open_trailing));
        // Slot 0's same-line comments precede every entry, so there is no
        // preceding entry's line for them to trail — they ride the opening
        // `{` itself (module doc, "Blank lines and comments").
        code.push_str(&interior_trailing(&interior.slots[0]));
        code.push('\n');
        let entry_pad = " ".repeat(indent + INDENT_UNIT);
        for (i, entry) in entries.iter().enumerate() {
            code.push_str(&interior_lines(&interior.slots[i], indent + INDENT_UNIT));
            code.push_str(&entry_pad);
            code.push_str(entry);
            if i + 1 < entries.len() {
                code.push(',');
            }
            // The NEXT slot's same-line comments belong to THIS entry's line
            // — see the indexing rule above (module doc, "Blank lines and
            // comments").
            code.push_str(&interior_trailing(&interior.slots[i + 1]));
            code.push('\n');
        }
        code.push_str(&interior_lines(
            &interior.slots[entries.len()],
            indent + INDENT_UNIT,
        ));
        code.push_str(&pad);
        code.push('}');
    }
    Rendered::new(leads_with_blank(blank_before, &a.doc_run), code)
        .with_trailing(a.close_trailing.as_ref())
}

fn render_namespace(
    ns: &NamespaceCst,
    blank_before: bool,
    indent: usize,
    tokens: &[Token],
) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(&ns.doc_run, indent, blank_before);
    code.push_str(&format!("{pad}namespace {} {{", ns.name));
    code.push_str(&open_trailing_text(&ns.open_trailing));
    code.push('\n');
    code.push_str(&flush(&render_top_items(
        &ns.items,
        indent + INDENT_UNIT,
        tokens,
    )));
    code.push_str(&pad);
    code.push('}');
    Rendered::new(leads_with_blank(blank_before, &ns.doc_run), code)
        .with_trailing(ns.close_trailing.as_ref())
}

fn render_reuse(r: &ReuseCst, blank_before: bool, indent: usize, tokens: &[Token]) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(&r.doc_run, indent, blank_before);
    let carrier = match r.carrier {
        ReuseCarrier::Routine => "routine",
        ReuseCarrier::Graph => "graph",
    };
    let head = format!(
        "{}{carrier} {}",
        if r.exported { "export " } else { "" },
        r.name
    );
    code.push_str(&pad);
    code.push_str(&paren_list(
        indent,
        &head,
        &signature_params(&r.sig),
        " {",
        &bucket(&r.sig_interior, r.sig.params.len()),
    ));
    code.push_str(&open_trailing_text(&r.open_trailing));
    code.push('\n');
    code.push_str(&flush(&render_world_items(
        &r.items,
        indent + INDENT_UNIT,
        tokens,
    )));
    code.push_str(&pad);
    code.push('}');
    Rendered::new(leads_with_blank(blank_before, &r.doc_run), code)
        .with_trailing(r.close_trailing.as_ref())
}

fn render_machine(m: &MachineCst, blank_before: bool, indent: usize, tokens: &[Token]) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(&m.doc_run, indent, blank_before);
    code.push_str(&format!("{pad}machine {{"));
    code.push_str(&open_trailing_text(&m.open_trailing));
    code.push('\n');
    code.push_str(&flush(&render_world_items(
        &m.items,
        indent + INDENT_UNIT,
        tokens,
    )));
    code.push_str(&pad);
    code.push('}');
    Rendered::new(leads_with_blank(blank_before, &m.doc_run), code)
        .with_trailing(m.close_trailing.as_ref())
}

// ---------------------------------------------------------------------------
// World bodies.
// ---------------------------------------------------------------------------

/// A world body (a `machine`, `routine`, or `graph` block). Runs of adjacent
/// single-line states are found first, so the run's shared header width and
/// shared rule grid are known before any of its members is rendered.
fn render_world_items(items: &[WorldItem], indent: usize, tokens: &[Token]) -> Vec<Rendered> {
    let inline = inline_state_runs(items, indent, tokens);
    let tape_names = tape_name_widths(items);
    items
        .iter()
        .enumerate()
        .map(|(i, item)| match &item.kind {
            WorldKind::Comment(c) => Rendered::new(item.blank_before, comment_line(c, indent)),
            WorldKind::Tape(t) => render_tape(t, tape_names[i], item.blank_before, indent),
            WorldKind::Graft(g) => render_graft(g, item.blank_before, indent),
            WorldKind::Bind(b) => render_bind(b, item.blank_before, indent),
            WorldKind::State(s) => match &inline[i] {
                Some(shape) => render_inline_state(s, shape, item.blank_before, indent, tokens),
                None => render_block_state(s, item.blank_before, indent, tokens),
            },
        })
        .collect()
}

/// The shared layout of the single-line-state run a state belongs to.
struct InlineShape {
    header: usize,
    grid: Grid,
}

/// Whether a state can print on one line at all: every rule written on the
/// header's own line, no interior comment, no comment on the `{`. A rule
/// whose `call_args`/`map_pairs` carry a comment is excluded too — that
/// comment forces its own binding list onto several physical lines, which a
/// single-line state can't absorb; a rule off the grid
/// ([`RuleCst::breaks_the_grid`]) is excluded for the same reason.
fn inline_candidate(state: &StateCst) -> bool {
    state.open_trailing.is_empty()
        && state.rules.iter().all(|item| match &item.kind {
            RuleKind::Comment(_) => false,
            RuleKind::Rule(r) => {
                r.rule.line == state.line
                    && r.trailing.is_none()
                    && r.call_args.is_empty()
                    && r.map_pairs.is_empty()
                    && !r.breaks_the_grid()
            }
        })
}

fn state_header_text(state: &StateCst) -> String {
    format!(
        "{}state {}",
        if state.entry { "entry " } else { "" },
        state.name
    )
}

/// Per world item, the inline shape to print a state with (`None` = block
/// form). A run is maximal over adjacent inline-capable, undocumented states
/// with no blank line between them; if any member would cross the line limit,
/// the whole run falls back to block form.
fn inline_state_runs(
    items: &[WorldItem],
    indent: usize,
    tokens: &[Token],
) -> Vec<Option<InlineShape>> {
    let mut out: Vec<Option<InlineShape>> = items.iter().map(|_| None).collect();
    fn member(item: &WorldItem) -> Option<&StateCst> {
        match &item.kind {
            WorldKind::State(s) => (s.doc_run.is_empty() && inline_candidate(s)).then_some(s),
            _ => None,
        }
    }
    let mut i = 0;
    while i < items.len() {
        if member(&items[i]).is_none() {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i + 1;
        while end < items.len() && member(&items[end]).is_some() && !items[end].blank_before {
            end += 1;
        }
        let states: Vec<&StateCst> = (start..end)
            .map(|k| member(&items[k]).expect("run members are inline-capable states"))
            .collect();
        let header = states
            .iter()
            .map(|s| state_header_text(s).chars().count())
            .max()
            .expect("a run holds at least one state");
        let rules: Vec<&RuleCst> = states
            .iter()
            .flat_map(|s| s.rules.iter())
            .filter_map(|item| match &item.kind {
                RuleKind::Rule(r) => Some(r.as_ref()),
                RuleKind::Comment(_) => None,
            })
            .collect();
        let grid = grid_for(&rules, tokens);
        let fits = states.iter().all(|s| {
            inline_state_line(s, header, &grid, indent, tokens)
                .chars()
                .count()
                <= LINE_WIDTH
        });
        if fits {
            // The run's SHARED grid is what every member prints with — that
            // is what makes a block of one-line states read as one table.
            for offset in 0..states.len() {
                out[start + offset] = Some(InlineShape {
                    header,
                    grid: grid_for(&rules, tokens),
                });
            }
        }
        i = end;
    }
    out
}

fn inline_state_line(
    state: &StateCst,
    header_width: usize,
    grid: &Grid,
    indent: usize,
    tokens: &[Token],
) -> String {
    let header = state_header_text(state);
    let mut line = format!(
        "{}{header}{} {{",
        " ".repeat(indent),
        " ".repeat(header_width.saturating_sub(header.chars().count()))
    );
    for item in &state.rules {
        if let RuleKind::Rule(r) = &item.kind {
            line.push(' ');
            line.push_str(&render_rule(r, grid, 0, tokens));
        }
    }
    line.push_str(" }");
    line
}

fn render_inline_state(
    state: &StateCst,
    shape: &InlineShape,
    blank_before: bool,
    indent: usize,
    tokens: &[Token],
) -> Rendered {
    let code = inline_state_line(state, shape.header, &shape.grid, indent, tokens);
    Rendered::new(blank_before, code).with_trailing(state.close_trailing.as_ref())
}

fn render_block_state(
    state: &StateCst,
    blank_before: bool,
    indent: usize,
    tokens: &[Token],
) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(&state.doc_run, indent, blank_before);
    code.push_str(&format!("{pad}{} {{", state_header_text(state)));
    code.push_str(&open_trailing_text(&state.open_trailing));
    code.push('\n');
    let rules: Vec<&RuleCst> = state
        .rules
        .iter()
        .filter_map(|item| match &item.kind {
            RuleKind::Rule(r) => Some(r.as_ref()),
            RuleKind::Comment(_) => None,
        })
        .collect();
    let grid = grid_for(&rules, tokens);
    let body: Vec<Rendered> = state
        .rules
        .iter()
        .map(|item| render_rule_item(item, &grid, indent + INDENT_UNIT, tokens))
        .collect();
    code.push_str(&flush(&body));
    code.push_str(&pad);
    code.push('}');
    Rendered::new(leads_with_blank(blank_before, &state.doc_run), code)
        .with_trailing(state.close_trailing.as_ref())
}

fn render_rule_item(item: &RuleItem, grid: &Grid, indent: usize, tokens: &[Token]) -> Rendered {
    match &item.kind {
        RuleKind::Comment(c) => Rendered::new(item.blank_before, comment_line(c, indent)),
        RuleKind::Rule(r) => Rendered::new(item.blank_before, render_rule(r, grid, indent, tokens))
            .with_trailing(r.trailing.as_ref()),
    }
}

/// Per world item, the name width a tape declaration pads to. A run of
/// adjacent `tape` declarations (no blank line, nothing else between them) is
/// a little table of its own: the alphabets line up in one column.
fn tape_name_widths(items: &[WorldItem]) -> Vec<usize> {
    let mut out = vec![0usize; items.len()];
    let name = |item: &WorldItem| match &item.kind {
        WorldKind::Tape(t) => Some(t.name.chars().count()),
        _ => None,
    };
    let mut i = 0;
    while i < items.len() {
        let Some(first) = name(&items[i]) else {
            i += 1;
            continue;
        };
        let start = i;
        let mut end = i + 1;
        let mut width = first;
        while end < items.len() && !items[end].blank_before {
            let Some(next) = name(&items[end]) else { break };
            width = width.max(next);
            end += 1;
        }
        for slot in out.iter_mut().take(end).skip(start) {
            *slot = width;
        }
        i = end;
    }
    out
}

fn render_tape(t: &TapeCst, name_width: usize, blank_before: bool, indent: usize) -> Rendered {
    let code = format!(
        "{}tape {}:{} {};",
        " ".repeat(indent),
        t.name,
        " ".repeat(name_width.saturating_sub(t.name.chars().count())),
        t.alphabet
    );
    Rendered::new(blank_before, code).with_trailing(t.trailing.as_ref())
}

fn render_graft(g: &GraftCst, blank_before: bool, indent: usize) -> Rendered {
    let mut code = doc_run_text(&g.doc_run, indent, blank_before);
    let head = format!(
        "{}graft {}",
        if g.entry { "entry " } else { "" },
        g.target.joined()
    );
    let tail = match &g.as_name {
        Some((name, _)) => format!(" as {name};"),
        None => ";".to_string(),
    };
    let entries = binding_entries(&g.args, indent + INDENT_UNIT, &g.map_pairs);
    code.push_str(&" ".repeat(indent));
    code.push_str(&paren_list(
        indent,
        &head,
        &entries,
        &tail,
        &bucket(&g.interior, entries.len()),
    ));
    Rendered::new(leads_with_blank(blank_before, &g.doc_run), code)
        .with_trailing(g.trailing.as_ref())
}

fn render_bind(b: &BindCst, blank_before: bool, indent: usize) -> Rendered {
    let mut code = doc_run_text(&b.doc_run, indent, blank_before);
    let head = format!("bind {}", b.target.joined());
    let tail = format!(" as {};", b.as_name.0);
    let entries = binding_entries(&b.args, indent + INDENT_UNIT, &b.map_pairs);
    code.push_str(&" ".repeat(indent));
    code.push_str(&paren_list(
        indent,
        &head,
        &entries,
        &tail,
        &bucket(&b.interior, entries.len()),
    ));
    Rendered::new(leads_with_blank(blank_before, &b.doc_run), code)
        .with_trailing(b.trailing.as_ref())
}

#[cfg(test)]
mod tests;
