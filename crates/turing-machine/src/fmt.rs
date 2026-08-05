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
//! - **Trivia-preserving, with one narrow exception** — every comment
//!   reprints somewhere: own-line comments at their block's indent, same-line
//!   trailing comments riding their line, brace-line comments riding the
//!   `{`/`}` they were written on. Doc (`?`) and attention (`!`) runs —
//!   `[deprecated]` included — stay directly above the declaration they
//!   document, in source order. A comment written INSIDE a comma-separated
//!   list — an `alphabet` body, a `routine`/`graph` signature parameter list,
//!   a `call`/`graft`/`bind` binding list, a `with map` pair list, or a `use`
//!   path list — prints where its author wrote it, keyed to the entry it
//!   precedes: a same-line comment rides the preceding entry's line, an
//!   own-line comment keeps its own line, and a comment after the last entry
//!   prints before the closer. A `//` comment forces such a list onto
//!   multiple lines (nothing can follow it on its physical line); a
//!   `/* … */` comment does not. The exception: a comment inside a pattern,
//!   write, or move vector still reprints as an own-line comment after the
//!   enclosing rule rather than in place — those vectors are positional and
//!   walked per row by the compiler, so giving them per-entry trivia is
//!   tracked separately (docs/tmt/fmt.md (interior comments)).
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
//! The threshold is the **80-column line limit** (the same one `line-too-long`
//! lints). A parenthesized list — a `call`'s bindings, a `graft`/`bind`'s
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
//! and any member that would then cross 80 columns falls back to a single
//! space on its own. Unlike `.pmc`'s rule, this does not consult the author's
//! source columns: a run either aligns or it does not, which is both simpler
//! and one less way for a second pass to disagree with the first.

use mtc_core::diagnostics::Span;

use crate::compiler::CompileError;
use crate::cst::{
    AlphabetCst, BindCst, Cst, DocRunItem, DocRunKind, GraftCst, MachineCst, NamespaceCst,
    ReuseCarrier, ReuseCst, RuleCst, RuleItem, RuleKind, StateCst, TapeCst, TopItem, TopKind,
    UseCst, UsePath, WorldItem, WorldKind,
};
use crate::lexer::{Comment, CommentKind, LexMode, Token, TokenKind, lex_with};
use crate::parser::{
    AlphabetElem, BindingArg, BindingValue, Continuation, MapArrow, MoveDir, MoveVec, Pattern,
    PatternCell, PatternCellKind, Rule, SigParamKind, Signature, SymLit, SymMap, TermKind,
    Transition, WriteCellKind, WriteVec, parse_cst,
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
/// align them one column past the run's widest code line — except for a
/// member that would then cross the line limit, which keeps its single space.
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
                let comment = normalize_comment_text(
                    &items[k]
                        .trailing
                        .as_ref()
                        .expect("eligible entries carry a trailing comment")
                        .text,
                )
                .chars()
                .count();
                spacing[k] = if align_col + comment <= LINE_WIDTH {
                    align_col - width
                } else {
                    1
                };
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
        if matches!(comment.kind, CommentKind::Line) {
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

fn pattern_text(pattern: &Pattern) -> String {
    let cells: Vec<String> = pattern.cells.iter().map(pattern_cell_text).collect();
    format!("[{}]", cells.join(", "))
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

fn write_vec_text(vec: &WriteVec, tokens: &[Token]) -> String {
    let cells: Vec<String> = vec
        .cells
        .iter()
        .map(|cell| match &cell.kind {
            WriteCellKind::Keep => "-".to_string(),
            WriteCellKind::Lit(sym) => sym_text(sym),
            WriteCellKind::Subst { expr } => format!("{{{}}}", subst_body_text(&expr.span, tokens)),
        })
        .collect();
    format!("write [{}]", cells.join(", "))
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

fn move_vec_text(vec: &MoveVec) -> String {
    let cells: Vec<&str> = vec
        .cells
        .iter()
        .map(|cell| match cell.dir {
            MoveDir::Left => "<",
            MoveDir::Right => ">",
            MoveDir::Stay => ".",
        })
        .collect();
    format!("move [{}]", cells.join(", "))
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

fn grid_for(rules: &[&Rule], tokens: &[Token]) -> Grid {
    let width = |s: &str| s.chars().count();
    Grid {
        pattern: rules
            .iter()
            .map(|r| width(&pattern_text(&r.pattern)))
            .max()
            .unwrap_or(0),
        debugger: if rules.iter().any(|r| r.debugger) {
            "debugger".len()
        } else {
            0
        },
        write: rules
            .iter()
            .filter_map(|r| r.write.as_ref().map(|w| width(&write_vec_text(w, tokens))))
            .max()
            .unwrap_or(0),
        mov: rules
            .iter()
            .filter_map(|r| r.mov.as_ref().map(|m| width(&move_vec_text(m))))
            .max()
            .unwrap_or(0),
    }
}

/// One rule as a grid row: `indent`, the padded pattern, the arrow, the
/// action columns, the transition, `;`.
fn render_rule(rc: &RuleCst, grid: &Grid, indent: usize, tokens: &[Token]) -> String {
    let rule = &rc.rule;
    let mut line = " ".repeat(indent);
    let pattern = pattern_text(&rule.pattern);
    let pattern_width = pattern.chars().count();
    line.push_str(&pattern);
    line.push_str(&" ".repeat(grid.pattern.saturating_sub(pattern_width)));
    line.push_str(" -> ");

    let segments: [(bool, String, usize); 3] = [
        (rule.debugger, "debugger".to_string(), grid.debugger),
        (
            rule.write.is_some(),
            rule.write
                .as_ref()
                .map(|w| write_vec_text(w, tokens))
                .unwrap_or_default(),
            grid.write,
        ),
        (
            rule.mov.is_some(),
            rule.mov.as_ref().map(move_vec_text).unwrap_or_default(),
            grid.mov,
        ),
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
        let mut out = interior_lines(&interior.slots[0], indent);
        out.push_str(&pad);
        out.push_str("use");
        // Slot 0's same-line comments have no preceding entry to trail, so
        // they ride the `use` keyword's own line instead — printing them
        // before `use` would reorder the token stream. A LINE comment there
        // eats the rest of the physical line, so the first path moves to a
        // fresh continuation line whenever this slot is non-empty (module
        // doc, "Blank lines and comments").
        let slot0_trailing = interior_trailing(&interior.slots[0]);
        if slot0_trailing.is_empty() {
            out.push(' ');
        } else {
            out.push_str(&slot0_trailing);
            out.push('\n');
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
    // A comment on the `{`, or any LINE comment inside the body, forces the
    // body onto its own lines whatever the width says.
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
/// single-line state can't absorb.
fn inline_candidate(state: &StateCst) -> bool {
    state.open_trailing.is_empty()
        && state.rules.iter().all(|item| match &item.kind {
            RuleKind::Comment(_) => false,
            RuleKind::Rule(r) => {
                r.rule.line == state.line
                    && r.trailing.is_none()
                    && r.call_args.is_empty()
                    && r.map_pairs.is_empty()
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
        let rules: Vec<&Rule> = states
            .iter()
            .flat_map(|s| s.rules.iter())
            .filter_map(|item| match &item.kind {
                RuleKind::Rule(r) => Some(&r.rule),
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
    let rules: Vec<&Rule> = state
        .rules
        .iter()
        .filter_map(|item| match &item.kind {
            RuleKind::Rule(r) => Some(&r.rule),
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
