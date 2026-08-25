//! The `.tmc` printer over the green syntax tree — the replacement for
//! the C1-CST printer in this module's parent, built one surface at a
//! time and held byte-identical to it at every step.
//!
//! # Two inputs, two owners
//!
//! Every printed VALUE comes from `crate::syntax::extract`'s own
//! helpers, which are oracle-tested against the C1 lowering: an
//! alphabet's elements, a `use` path's segments, a doc run's items. Every
//! derived layout FACT — a blank line, a trailing comment, the comments
//! riding a `{` — comes from [`super::trivia`], which re-derives them
//! from the tree. Neither half re-implements the other's decisions
//! (docs/tmt/fmt.md (comments), docs/tmt/fmt.md (blank lines)).
//!
//! # The duplication is deliberate
//!
//! The layout machinery and the value→text functions below are copied
//! VERBATIM from the C1 printer rather than shared with it. That printer
//! is this one's differential oracle until the whole surface is ported,
//! and a helper shared between the two would make the oracle blind to
//! any bug introduced inside it. The copies go away with the C1 printer
//! itself.
//!
//! # What `blank_before` costs here
//!
//! C1 needed a two-way branch (`leads_with_blank`) because a documented
//! declaration repurposed its own `blank_before` for the run→declaration
//! gap. On the green tree the bound doc run is INSIDE the declaration's
//! node, so both questions are "the gap before this node's first token"
//! and the branch disappears: the unit's own flag answers the outer one,
//! and `trivia::blank_before_decl` answers the smaller inner one.

// The green printer takes the language one surface at a time; until the
// last surface is wired it has no production caller, and the copied
// helpers a later surface needs are reachable only from this module's
// own tests.
#![allow(dead_code)]

use mtc_core::syntax::{AstNode, SyntaxNode, TextLineIndex};

use super::trivia::{self, Unit, UnitKind};
use crate::compiler::CompileError;
use crate::cst::{DocRunItem, DocRunKind};
use mtc_core::diagnostics::Span;

use crate::lexer::{Comment, CommentKind, LexMode, Token, TokenKind, lex_with};
use crate::parser::{
    AlphabetElem, BindingArg, BindingValue, Continuation, ContractClause, Import, MapArrow,
    MoveCell, MoveDir, MoveVec, Pattern, PatternCell, PatternCellKind, Rule, SigParam,
    SigParamKind, SymLit, SymMap, TermKind, Transition, WriteCell, WriteCellKind, WriteVec,
    parse_green_from_tokens, reparse_sig_param,
};
use crate::syntax::extract::{
    extract_alphabet, extract_bind, extract_doc_items, extract_graft, extract_import, extract_rule,
    sig_tokens,
};
use crate::syntax::{
    AlphabetView, BindView, DocRunView, GraftView, MachineView, NamespaceView, ReuseKind,
    ReuseView, RootView, RuleView, StateView, TapeView, TopView, UseView, WorldView,
};

/// Spaces per block level (`super`'s module doc, "Indentation").
const INDENT_UNIT: usize = 2;

/// The line limit every width decision is measured against (`super`'s
/// module doc, "Argument lists and the width threshold").
const LINE_WIDTH: usize = 80;

/// `.tmc` source → canonical text, printed from the green syntax tree.
/// Lexes with comments retained, builds the tree, and walks it. A lex or
/// parse error is returned, never printed.
pub(crate) fn format_green(source: &str) -> Result<String, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    let root = RootView::cast(SyntaxNode::new_root(green)).expect("the green root is a ROOT node");
    let index = TextLineIndex::new(source);
    let out = flush(&render_top_items(root.syntax(), 0, source, &index));
    // An empty file still reprints as exactly one newline; a non-empty one
    // already ends in the last item's newline.
    Ok(if out.is_empty() {
        "\n".to_string()
    } else {
        out
    })
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

/// Spaces between an item's code and its trailing comment (`super`'s module
/// doc, "Blank lines and comments"): one by default; in a run of two or more
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
/// trivia bookkeeping; in release it clamps to the tail slot, because a
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

/// A declaration's bound run, as items. Empty when none was written —
/// which is also when [`doc_run_text`] prints nothing and
/// `trivia::blank_before_decl`'s answer is never asked for.
fn doc_items(run: Option<DocRunView>, source: &str, index: &TextLineIndex) -> Vec<DocRunItem> {
    run.map(|dr| extract_doc_items(&dr, source, index))
        .unwrap_or_default()
}

/// A declaration's `?`/`!` run, printed at the declaration's own indent, one
/// canonical space after the sigil. Returns the lines (each newline-
/// terminated) or the empty string; `blank_before_decl` is the gap between
/// the run and the declaration it documents.
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

/// One write cell. `tokens` is the enclosing WRITE_VEC's own significant
/// token run — see [`subst_body_text`] for why a substitution reprints
/// from tokens rather than from its parsed value.
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
///
/// A substitution has no node of its own — its `{`, its expression and its
/// `}` are plain tokens of the enclosing WRITE_VEC — so the span filter is
/// still what selects it. `tokens` is that vector's own significant run
/// (`syntax::extract`'s `sig_tokens`) rather than the whole file's: the
/// spans are absolute either way, so the filter selects the same tokens,
/// and the run ends in a synthetic `Eof` that lies past every cell. One
/// consequence of that narrowing, stated because a reader will otherwise
/// look for the case it covers: `sig_tokens` drops trivia, so the
/// comment test below is DEAD here — it is kept only because this
/// function is a verbatim copy of the C1 printer's, where the whole
/// file's tokens (comments included) are what arrives.
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

fn continuation_text(cont: &Continuation) -> String {
    match cont {
        Continuation::State { name, .. } => name.clone(),
        Continuation::Return { .. } => "return".to_string(),
        Continuation::Stop { .. } => "stop".to_string(),
        Continuation::Halt { .. } => "halt".to_string(),
    }
}

/// One `writes { … }` or `preserves { … }` clause, re-encoded losslessly:
/// a single leading space ahead of the keyword, then the same brace-body
/// spacing an `alphabet` renders inline (`{ elem, elem }`,
/// [`render_alphabet`]). An empty clause is meaningful — it declares that
/// the parameter writes (or preserves) nothing, distinct from no clause at
/// all — and prints `{}` with no inner space; this deliberately does NOT
/// mirror `render_alphabet`'s empty-body spacing because a bare `alphabet`
/// body can never be empty (the compiler rejects it), so that path renders
/// no real input and sets no convention. A clause carries no interior
/// comments (the parser never attaches any to one), so there is no
/// interior/wrapping case to consider here — unlike `render_alphabet` or
/// [`paren_list`], a clause is always one unbroken run of tokens on the line
/// its parameter entry occupies.
fn contract_clause_text(keyword: &str, clause: &ContractClause) -> String {
    let entries: Vec<String> = clause.elems.iter().map(alphabet_elem_text).collect();
    if entries.is_empty() {
        format!(" {keyword} {{}}")
    } else {
        format!(" {keyword} {{ {} }}", entries.join(", "))
    }
}

fn signature_params(params: &[SigParam]) -> Vec<String> {
    params
        .iter()
        .map(|param| match &param.kind {
            SigParamKind::Tape {
                alphabet,
                volatile,
                writes,
                preserves,
                ..
            } => {
                let prefix = if *volatile { "volatile " } else { "" };
                let mut out = format!("{prefix}tape {}: {alphabet}", param.name);
                if let Some(clause) = writes {
                    out.push_str(&contract_clause_text("writes", clause));
                }
                if let Some(clause) = preserves {
                    out.push_str(&contract_clause_text("preserves", clause));
                }
                out
            }
            SigParamKind::State => format!("state {}", param.name),
        })
        .collect()
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
/// (`super`'s module doc, "Argument lists and the width threshold";
/// docs/tmt/fmt.md (interior comments)).
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
    // itself (`super`'s module doc, "Blank lines and comments").
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
        // see the indexing rule (`super`'s module doc, "Blank lines and
        // comments").
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

/// `head(entries)tail` on one line while it fits from column `col`
/// (`super`'s module doc, "Argument lists and the width threshold"), else
/// one entry per line. `head` starts AT `col` and never carries the leading
/// indent itself — a caller opening a line emits that indent before calling.
/// `interior` is the list's interior comments, bucketed by [`bucket`]; a
/// caller with no such list passes `&bucket(&[], entries.len())`. An entry
/// that already spans several physical lines (a binding argument whose own
/// nested `with map` broke on an interior comment) forces the list to break
/// too — the alternative would splice that entry's own newlines into what
/// the width check believes is one line, with no indent for the
/// continuation.
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
    // itself (`super`'s module doc, "Blank lines and comments").
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
        // see the indexing rule (`super`'s module doc, "Blank lines and
        // comments").
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

/// One `use` path's text. Takes the [`Import`] extraction builds rather
/// than re-reading the view's own tokens, so the segment order and the
/// alias rule stay owned by the one walk the C1 lowering is pinned
/// against.
fn use_path_text(path: &Import) -> String {
    let mut out = path.path.join("::");
    if let Some(alias) = &path.alias {
        out.push_str(" as ");
        out.push_str(alias);
    }
    out
}

// ---------------------------------------------------------------------------
// The rule grid.
// ---------------------------------------------------------------------------

/// The column widths one grid group shares (`super`'s module doc, "The
/// state-block grid"). A width of zero means the group has no rule using
/// that segment, so the column does not exist at all.
struct Grid {
    pattern: usize,
    debugger: usize,
    write: usize,
    mov: usize,
}

/// One rule, ready to lay out: its value, its own WRITE_VEC token run
/// (what a `{expr}` substitution reprints from), and whether it is off
/// the grid.
///
/// The value comes from `syntax::extract`'s own `extract_rule`, which is
/// oracle-tested against the C1 lowering; the off-grid flag is trivia,
/// re-derived by [`super::trivia::rule_vector_comments`].
struct PreparedRule {
    rule: Rule,
    write_tokens: Vec<Token>,
    off_grid: bool,
}

/// A RULE node → the pieces the grid and the row renderer need.
fn prepare_rule(view: &RuleView, index: &TextLineIndex) -> PreparedRule {
    PreparedRule {
        rule: extract_rule(view, index),
        // Only a `write` vector can hold a substitution, and only the
        // vector's own tokens are ever consulted — so an absent one
        // needs no run at all.
        write_tokens: view
            .write_vec()
            .map(|w| sig_tokens(w.syntax(), index))
            .unwrap_or_default(),
        off_grid: breaks_the_grid(view.syntax()),
    }
}

/// True when any glyph vector carries a LINE comment or an OWN-LINE
/// comment (mirrors [`bucket`]'s `forces_break`). A LINE comment cannot
/// share its physical line; an own-line comment, block or line, would
/// silently flip its own `own_line` flag on the next parse if inlined
/// onto a cell's line. Either way the rule cannot be a grid row, so it
/// renders multi-line and is excluded from the grid's width computation
/// (docs/tmt/fmt.md (interior comments)).
///
/// "In a glyph vector" is the OLD PARSER's reading of it, not the
/// bracket's: a comment written in a vector's header — `write /* c */ [`
/// — is claimed by that vector's interior list too. `rule_vector_comments`
/// answers the union of all three lists, header halves included.
fn breaks_the_grid(rule: &SyntaxNode) -> bool {
    trivia::rule_vector_comments(rule)
        .iter()
        .any(|c| matches!(c.kind, CommentKind::Line) || c.own_line)
}

/// `rules` off the grid (`super`'s module doc, "The state-block grid")
/// are excluded from every column: they consume none of the group's
/// width and, since they render themselves, do not need one.
fn grid_for(rules: &[&PreparedRule]) -> Grid {
    let width = |s: &str| s.chars().count();
    let on_grid: Vec<&PreparedRule> = rules.iter().copied().filter(|p| !p.off_grid).collect();
    // Interior comments are a later surface — see `render_use`. Every
    // width below is measured with the same empty interior the row
    // renderer prints with, so the two cannot disagree.
    Grid {
        pattern: on_grid
            .iter()
            .map(|p| {
                let interior = bucket(&[], p.rule.pattern.cells.len());
                width(&pattern_text(&p.rule.pattern, &interior))
            })
            .max()
            .unwrap_or(0),
        debugger: if on_grid.iter().any(|p| p.rule.debugger) {
            "debugger".len()
        } else {
            0
        },
        write: on_grid
            .iter()
            .filter_map(|p| {
                p.rule.write.as_ref().map(|w| {
                    let interior = bucket(&[], w.cells.len());
                    width(&write_vec_text(w, &p.write_tokens, &interior))
                })
            })
            .max()
            .unwrap_or(0),
        mov: on_grid
            .iter()
            .filter_map(|p| {
                p.rule.mov.as_ref().map(|m| {
                    let interior = bucket(&[], m.cells.len());
                    width(&move_vec_text(m, &interior))
                })
            })
            .max()
            .unwrap_or(0),
    }
}

/// One rule as a grid row: `indent`, the padded pattern, the arrow, the
/// action columns, the transition, `;`. Delegates to
/// [`render_rule_off_grid`] first when the rule is off the grid — such a
/// rule ignores `grid` entirely, since it was excluded from the
/// computation that produced it.
fn render_rule(prepared: &PreparedRule, grid: &Grid, indent: usize) -> String {
    if prepared.off_grid {
        return render_rule_off_grid(prepared, indent);
    }
    let rule = &prepared.rule;
    let mut line = " ".repeat(indent);
    let pattern_interior = bucket(&[], rule.pattern.cells.len());
    let pattern = pattern_text(&rule.pattern, &pattern_interior);
    let pattern_width = pattern.chars().count();
    line.push_str(&pattern);
    line.push_str(&" ".repeat(grid.pattern.saturating_sub(pattern_width)));
    line.push_str(" -> ");

    let write_text = match &rule.write {
        Some(w) => write_vec_text(w, &prepared.write_tokens, &bucket(&[], w.cells.len())),
        None => String::new(),
    };
    let move_text = match &rule.mov {
        Some(m) => move_vec_text(m, &bucket(&[], m.cells.len())),
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
    let transition = transition_text(&rule.transition, col);
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

/// A rule off the grid (`super`'s module doc, "The state-block grid"): a
/// LINE comment in one of its glyph vectors forces it there, and the
/// whole rule renders across several lines instead of padding to the
/// group's shared columns — every vector it carries breaks, not only the
/// one with the comment, so the rule reads as one consistent shape
/// rather than a mix of broken and padded segments.
fn render_rule_off_grid(prepared: &PreparedRule, indent: usize) -> String {
    let rule = &prepared.rule;
    let mut line = " ".repeat(indent);

    let pattern_cells: Vec<String> = rule.pattern.cells.iter().map(pattern_cell_text).collect();
    let pattern_interior = bucket(&[], pattern_cells.len());
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
        let cells: Vec<String> = w
            .cells
            .iter()
            .map(|c| write_cell_text(c, &prepared.write_tokens))
            .collect();
        let interior = bucket(&[], cells.len());
        line.push_str(&glyph_vec_multiline("write [", &cells, &interior, indent));
        line.push(' ');
    }
    if let Some(m) = &rule.mov {
        let cells: Vec<String> = m.cells.iter().map(move_cell_text).collect();
        let interior = bucket(&[], cells.len());
        line.push_str(&glyph_vec_multiline("move [", &cells, &interior, indent));
        line.push(' ');
    }

    let col = col_after(&line);
    let transition = transition_text(&rule.transition, col);
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
/// [`breaks_the_grid`] sends a rule down. `head` is the vector's leading
/// text up to and including its opening `[` (`"["` for a pattern,
/// `"write ["` / `"move ["` for the action vectors). Mirrors
/// [`paren_list`]'s multi-line branch and the same indexing rule as every
/// other list: slot `i`'s own-line comments print above cell `i`; slot
/// `i + 1`'s same-line comments print at the end of cell `i`'s line
/// (docs/tmt/fmt.md (interior comments)).
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
    // itself (`super`'s module doc, "Blank lines and comments").
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
        // see the indexing rule above (`super`'s module doc, "Blank lines
        // and comments").
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
/// breaks against. A `call`'s own interior comments (its binding list's
/// and any nested `with map`'s) are a later surface — see `render_use`.
fn transition_text(transition: &Transition, col: usize) -> String {
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
            let entries = binding_entries(args, entry_col, &[]);
            let head = format!("call {}", target.joined());
            // The `;` the caller appends is reserved by rendering it into the
            // tail used for the fit measurement.
            let tail = format!(" then {};", continuation_text(then));
            let rendered = paren_list(col, &head, &entries, &tail, &bucket(&[], entries.len()));
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

/// One container's items — the file (ROOT) or a `namespace` body. The
/// unit stream, the blank lines and the trailing comments all come from
/// [`super::trivia`]; this walk only dispatches on kind.
fn render_top_items(
    container: &SyntaxNode,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Vec<Rendered> {
    trivia::units(container, index)
        .into_iter()
        .map(|unit| render_top_item(&unit, indent, source, index))
        .collect()
}

fn render_top_item(unit: &Unit, indent: usize, source: &str, index: &TextLineIndex) -> Rendered {
    match &unit.kind {
        UnitKind::Comment(c) => Rendered::new(unit.blank_before, comment_line(c, indent)),
        UnitKind::Node(node) => match TopView::cast(node.clone())
            .expect("a container's item node is one of the top-level kinds")
        {
            TopView::Use(v) => render_use(&v, unit, indent, index),
            TopView::Alphabet(v) => render_alphabet(&v, unit, indent, source, index),
            TopView::Namespace(v) => render_namespace(&v, unit, indent, source, index),
            TopView::Reuse(v) => render_reuse(&v, unit, indent, source, index),
            TopView::Machine(v) => render_machine(&v, unit, indent, source, index),
        },
    }
}

fn render_use(view: &UseView, unit: &Unit, indent: usize, index: &TextLineIndex) -> Rendered {
    let paths: Vec<String> = view
        .paths()
        .map(|p| use_path_text(&extract_import(&p, &[], index)))
        .collect();
    // Interior comments are a later surface; a comment-free list buckets
    // to an empty interior, which is the branch every source without one
    // takes anyway.
    let interior = bucket(&[], paths.len());
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
        // column right past `use ` (`super`'s module doc, "Argument lists
        // and the width threshold").
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
        // is non-empty (`super`'s module doc, "Blank lines and comments").
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
            // line — see the indexing rule (`super`'s module doc, "Blank
            // lines and comments").
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
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

fn render_alphabet(
    view: &AlphabetView,
    unit: &Unit,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Rendered {
    let a = extract_alphabet(view, &[], source, index);
    let open_trailing = trivia::open_trailing(view.syntax(), index);
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(
        &doc_items(view.doc_run(), source, index),
        indent,
        trivia::blank_before_decl(view.syntax()),
    );
    let head = format!(
        "{pad}{}alphabet {}",
        if a.exported { "export " } else { "" },
        a.name
    );
    let entries: Vec<String> = a.elems.iter().map(alphabet_elem_text).collect();
    // Interior comments are a later surface — see `render_use`.
    let interior = bucket(&[], a.elems.len());
    let one_line = format!("{head} {{ {} }}", entries.join(", "));
    // A comment on the `{`, any LINE comment inside the body, or any
    // own-line comment inside the body forces the body onto its own lines
    // whatever the width says (`bucket`'s `forces_break`).
    if open_trailing.is_empty() && interior.is_empty() && one_line.chars().count() <= LINE_WIDTH {
        code.push_str(&one_line);
    } else if open_trailing.is_empty()
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
        code.push_str(&open_trailing_text(&open_trailing));
        // Slot 0's same-line comments precede every entry, so there is no
        // preceding entry's line for them to trail — they ride the opening
        // `{` itself (`super`'s module doc, "Blank lines and comments").
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
            // — see the indexing rule above.
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
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

fn render_namespace(
    view: &NamespaceView,
    unit: &Unit,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(
        &doc_items(view.doc_run(), source, index),
        indent,
        trivia::blank_before_decl(view.syntax()),
    );
    code.push_str(&format!("{pad}namespace {} {{", view.name()));
    code.push_str(&open_trailing_text(&trivia::open_trailing(
        view.syntax(),
        index,
    )));
    code.push('\n');
    code.push_str(&flush(&render_top_items(
        view.syntax(),
        indent + INDENT_UNIT,
        source,
        index,
    )));
    code.push_str(&pad);
    code.push('}');
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

fn render_reuse(
    view: &ReuseView,
    unit: &Unit,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(
        &doc_items(view.doc_run(), source, index),
        indent,
        trivia::blank_before_decl(view.syntax()),
    );
    let carrier = match view.kind() {
        ReuseKind::Routine => "routine",
        ReuseKind::Graph => "graph",
    };
    let head = format!(
        "{}{carrier} {}",
        if view.exported() { "export " } else { "" },
        view.name_token().text()
    );
    let params: Vec<SigParam> = view
        .params()
        .map(|p| reparse_sig_param(&sig_tokens(p.syntax(), index)))
        .collect();
    let entries = signature_params(&params);
    let world = view
        .world()
        .expect("a parsed REUSE always carries its WORLD body");
    code.push_str(&pad);
    // The signature's interior comments are a later surface — see
    // `render_use`.
    code.push_str(&paren_list(
        indent,
        &head,
        &entries,
        " {",
        &bucket(&[], entries.len()),
    ));
    code.push_str(&render_world_after_brace(&world, indent, source, index));
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

fn render_machine(
    view: &MachineView,
    unit: &Unit,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(
        &doc_items(view.doc_run(), source, index),
        indent,
        trivia::blank_before_decl(view.syntax()),
    );
    let world = view
        .world()
        .expect("a parsed MACHINE always carries its WORLD body");
    code.push_str(&format!("{pad}machine {{"));
    code.push_str(&render_world_after_brace(&world, indent, source, index));
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

/// A world body from just past its `{` — the brace's own comment run, the
/// items at one more indent level, and the closing `}` back at `indent`.
/// The `{` itself is the CALLER's: `render_machine` writes it into the
/// header, `render_reuse` gets it from `paren_list`'s tail.
fn render_world_after_brace(
    world: &WorldView,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> String {
    // The braces belong to WORLD, not to the declaration that carries it,
    // so the open run is asked of WORLD (`super::trivia`'s module doc).
    let mut out = open_trailing_text(&trivia::open_trailing(world.syntax(), index));
    out.push('\n');
    out.push_str(&flush(&render_world_items(
        world.syntax(),
        indent + INDENT_UNIT,
        source,
        index,
    )));
    out.push_str(&" ".repeat(indent));
    out.push('}');
    out
}

// ---------------------------------------------------------------------------
// World bodies.
// ---------------------------------------------------------------------------

/// A world body's items — tape declarations, grafts, binds, states, and the
/// own-line comments between them, in source order. The stream comes from
/// [`super::trivia::units`] over the WORLD node rather than from
/// `WorldView`'s per-kind accessors, which are filters and lose source
/// order.
///
/// Runs of adjacent single-line states are found FIRST, so the run's
/// shared header width and shared rule grid are known before any of its
/// members is rendered.
fn render_world_items(
    world: &SyntaxNode,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Vec<Rendered> {
    let units = trivia::units(world, index);
    let tape_names = tape_name_widths(&units);
    let inline = inline_state_runs(&units, indent, index);
    units
        .iter()
        .enumerate()
        .map(|(i, unit)| {
            render_world_item(
                unit,
                tape_names[i],
                inline[i].as_ref(),
                indent,
                source,
                index,
            )
        })
        .collect()
}

fn render_world_item(
    unit: &Unit,
    name_width: usize,
    inline: Option<&InlineShape>,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Rendered {
    let node = match &unit.kind {
        UnitKind::Comment(c) => return Rendered::new(unit.blank_before, comment_line(c, indent)),
        UnitKind::Node(node) => node,
    };
    if let Some(v) = TapeView::cast(node.clone()) {
        render_tape(&v, unit, name_width, indent)
    } else if let Some(v) = GraftView::cast(node.clone()) {
        render_graft(&v, unit, indent, source, index)
    } else if let Some(v) = BindView::cast(node.clone()) {
        render_bind(&v, unit, indent, source, index)
    } else if let Some(v) = StateView::cast(node.clone()) {
        match inline {
            Some(shape) => render_inline_state(&v, unit, shape, indent, index),
            None => render_block_state(&v, unit, indent, source, index),
        }
    } else {
        unimplemented!("a world holds only tapes, grafts, binds and states")
    }
}

/// Per world item, the name width a tape declaration pads to. A run of
/// adjacent `tape` declarations (no blank line, nothing else between them) is
/// a little table of its own: the alphabets line up in one column.
fn tape_name_widths(units: &[Unit]) -> Vec<usize> {
    let mut out = vec![0usize; units.len()];
    let name = |unit: &Unit| match &unit.kind {
        UnitKind::Node(n) => {
            TapeView::cast(n.clone()).map(|t| t.name_token().text().chars().count())
        }
        UnitKind::Comment(_) => None,
    };
    let mut i = 0;
    while i < units.len() {
        let Some(first) = name(&units[i]) else {
            i += 1;
            continue;
        };
        let start = i;
        let mut end = i + 1;
        let mut width = first;
        while end < units.len() && !units[end].blank_before {
            let Some(next) = name(&units[end]) else { break };
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

fn render_tape(view: &TapeView, unit: &Unit, name_width: usize, indent: usize) -> Rendered {
    // `name_width` is name-length-only (see `tape_name_widths`): the
    // `volatile ` prefix does not enter the run's column alignment, so a
    // mixed volatile/plain run aligns names but not the modifier.
    let name = view.name_token();
    let name = name.text();
    let code = format!(
        "{}{}tape {}:{} {};",
        " ".repeat(indent),
        if view.volatile() { "volatile " } else { "" },
        name,
        " ".repeat(name_width.saturating_sub(name.chars().count())),
        view.alphabet_token().text()
    );
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

fn render_graft(
    view: &GraftView,
    unit: &Unit,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Rendered {
    let g = extract_graft(view, source, index);
    let mut code = doc_run_text(
        &doc_items(view.doc_run(), source, index),
        indent,
        trivia::blank_before_decl(view.syntax()),
    );
    let head = format!(
        "{}graft {}",
        if g.entry { "entry " } else { "" },
        g.target.joined()
    );
    let tail = match &g.as_name {
        Some(name) => format!(" as {};", name.name),
        None => ";".to_string(),
    };
    // Interior comments — the binding list's own and any nested `with map`'s
    // — are a later surface; see `render_use`.
    let entries = binding_entries(&g.args, indent + INDENT_UNIT, &[]);
    code.push_str(&" ".repeat(indent));
    code.push_str(&paren_list(
        indent,
        &head,
        &entries,
        &tail,
        &bucket(&[], entries.len()),
    ));
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

fn render_bind(
    view: &BindView,
    unit: &Unit,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Rendered {
    let b = extract_bind(view, source, index);
    let mut code = doc_run_text(
        &doc_items(view.doc_run(), source, index),
        indent,
        trivia::blank_before_decl(view.syntax()),
    );
    let head = format!("bind {}", b.target.joined());
    let tail = format!(" as {};", b.as_name.name);
    // Interior comments are a later surface — see `render_graft`.
    let entries = binding_entries(&b.args, indent + INDENT_UNIT, &[]);
    code.push_str(&" ".repeat(indent));
    code.push_str(&paren_list(
        indent,
        &head,
        &entries,
        &tail,
        &bucket(&[], entries.len()),
    ));
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

fn state_header_text(view: &StateView) -> String {
    format!(
        "{}state {}",
        if view.is_entry() { "entry " } else { "" },
        view.name_token().text()
    )
}

/// The shared layout of the single-line-state run a state belongs to.
struct InlineShape {
    header: usize,
    grid: Grid,
}

/// One state's rules, prepared in document order.
///
/// Read off `StateView::rules` rather than off the state's own unit
/// stream, because the run scan asks for it about states it has not yet
/// decided on — and the two agree wherever the answer is used: a state
/// that reaches the inline path carries no comment between its rules, so
/// its unit stream holds exactly these nodes in exactly this order.
fn prepared_rules(view: &StateView, index: &TextLineIndex) -> Vec<PreparedRule> {
    view.rules().map(|r| prepare_rule(&r, index)).collect()
}

/// Whether a rule's `call` binding list — or a `with map` nested inside
/// one — carries an interior comment. Such a comment breaks its own list
/// across physical lines, which a single-line state cannot absorb
/// (docs/tmt/fmt.md (interior comments)).
///
/// **Stubbed to `false`** — "no rule carries one" — because this printer
/// does not render a rule's interior comments at all yet: `render_rule`
/// prints every list with an empty interior, so a source carrying such a
/// comment already diverges on the dropped comment itself, and no
/// differential source can exercise the TRUE answer.
///
/// The `false` answer is load-bearing and pinned: a constant `true`
/// disqualifies every state from every run, which the run tests below
/// catch at once (measured). What no source here can catch is the real
/// predicate answering `false` for a rule that does carry such a
/// comment — so its body and the inline path's own interior rendering
/// have to land in the same change.
fn rule_has_interior_comment(_rule: &RuleView) -> bool {
    false
}

/// Whether a state can print on one line at all: every rule written on
/// the header's own line, no comment between rules, no comment riding a
/// rule's `;`, no interior comment in a rule's binding list or map, and
/// no rule off the grid — an off-grid rule renders across several lines,
/// which a single-line state cannot absorb.
///
/// Two readings this deliberately does NOT take:
///
/// - **"The header's line" is the NAME token's line**, not the node's
///   first token's. The old printer records a state's line off its name
///   span, so `entry` written alone on the line above still leaves the
///   state a candidate.
/// - **"A rule carries no trailing comment" is asked of the rule's UNIT,
///   not of its node.** A comment written inside a rule is relocated
///   onto its `;` ([`super::trivia`]'s `unclaimed_inside`), and it is
///   the relocated result the old printer tests — so
///   `[*] -> stop /* c */;` blocks its state. Asking whether the RULE
///   node holds a comment is a different question, and answers `false`
///   for the same rule read from a vector's interior list.
fn inline_candidate(view: &StateView, index: &TextLineIndex) -> bool {
    if !trivia::open_trailing(view.syntax(), index).is_empty() {
        return false;
    }
    let state_line = index.line_col(view.name_token().text_range().start).0;
    trivia::units(view.syntax(), index)
        .iter()
        .all(|unit| match &unit.kind {
            UnitKind::Comment(_) => false,
            UnitKind::Node(node) => {
                let rule = RuleView::cast(node.clone()).expect("a state's item node is a RULE");
                let prepared = prepare_rule(&rule, index);
                prepared.rule.line == state_line
                    && unit.trailing.is_none()
                    && !rule_has_interior_comment(&rule)
                    && !prepared.off_grid
            }
        })
}

/// Per world item, the inline shape to print a state with (`None` =
/// block form). A run is maximal over adjacent inline-capable,
/// UNDOCUMENTED states with no blank line between them; if any member
/// would cross [`LINE_WIDTH`], the WHOLE run falls back to block form —
/// a per-state width check would keep the short members inline and split
/// the table.
///
/// Membership is computed over the same unit stream the block path
/// walks, so a comment written between two states ends the run exactly
/// as the old printer's own comment item did.
fn inline_state_runs(
    units: &[Unit],
    indent: usize,
    index: &TextLineIndex,
) -> Vec<Option<InlineShape>> {
    let mut out: Vec<Option<InlineShape>> = units.iter().map(|_| None).collect();
    let member = |unit: &Unit| -> Option<StateView> {
        match &unit.kind {
            // A documented state is never a member: its run has to print
            // on lines of its own above the header.
            UnitKind::Node(node) => StateView::cast(node.clone())
                .filter(|v| v.doc_run().is_none() && inline_candidate(v, index)),
            UnitKind::Comment(_) => None,
        }
    };
    let mut i = 0;
    while i < units.len() {
        if member(&units[i]).is_none() {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i + 1;
        while end < units.len() && member(&units[end]).is_some() && !units[end].blank_before {
            end += 1;
        }
        let states: Vec<StateView> = (start..end)
            .map(|k| member(&units[k]).expect("run members are inline-capable states"))
            .collect();
        let header = states
            .iter()
            .map(|s| state_header_text(s).chars().count())
            .max()
            .expect("a run holds at least one state");
        let bodies: Vec<Vec<PreparedRule>> =
            states.iter().map(|s| prepared_rules(s, index)).collect();
        let rules: Vec<&PreparedRule> = bodies.iter().flatten().collect();
        let grid = grid_for(&rules);
        let fits = states.iter().zip(&bodies).all(|(s, body)| {
            inline_state_line(s, body, header, &grid, indent)
                .chars()
                .count()
                <= LINE_WIDTH
        });
        if fits {
            // The run's SHARED grid is what every member prints with —
            // that is what makes a block of one-line states read as one
            // table.
            for slot in out.iter_mut().take(end).skip(start) {
                *slot = Some(InlineShape {
                    header,
                    grid: grid_for(&rules),
                });
            }
        }
        i = end;
    }
    out
}

/// One state on a single line: the header padded to the run's shared
/// width, then every rule rendered as a grid row at indent zero, then
/// the closing `}`. A zero-row state prints `state z { }` — the `{` and
/// the ` }` with no rule between them.
fn inline_state_line(
    view: &StateView,
    rules: &[PreparedRule],
    header_width: usize,
    grid: &Grid,
    indent: usize,
) -> String {
    let header = state_header_text(view);
    let mut line = format!(
        "{}{header}{} {{",
        " ".repeat(indent),
        " ".repeat(header_width.saturating_sub(header.chars().count()))
    );
    for prepared in rules {
        line.push(' ');
        line.push_str(&render_rule(prepared, grid, 0));
    }
    line.push_str(" }");
    line
}

/// A state printed inline. The unit's own `blank_before` is what prints:
/// a member carries no doc run, so the two questions
/// [`render_block_state`] splits between `blank_before` and
/// `blank_before_decl` collapse to the outer one.
fn render_inline_state(
    view: &StateView,
    unit: &Unit,
    shape: &InlineShape,
    indent: usize,
    index: &TextLineIndex,
) -> Rendered {
    let rules = prepared_rules(view, index);
    let code = inline_state_line(view, &rules, shape.header, &shape.grid, indent);
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

/// A state in block form: the header, the brace's own comment run, the
/// rule table at one more indent level, and the closing `}`.
///
/// The whole state is ONE grid group — an own-line comment or a blank
/// line between two rules does not split the table (`super`'s module
/// doc, "The state-block grid"), so the group is computed over every
/// RULE unit at once and the walk below only interleaves the comment
/// units back in at their own positions.
///
/// A zero-row state is valid — it traps on entry (docs/tmt/language.md
/// (rules)) — and renders as an empty body, never as an error. A BARE
/// one never arrives here: [`inline_candidate`] is vacuously true over
/// an empty rule list, so only a documented (or otherwise disqualified)
/// zero-row state takes this path.
fn render_block_state(
    view: &StateView,
    unit: &Unit,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(
        &doc_items(view.doc_run(), source, index),
        indent,
        trivia::blank_before_decl(view.syntax()),
    );
    code.push_str(&format!("{pad}{} {{", state_header_text(view)));
    code.push_str(&open_trailing_text(&trivia::open_trailing(
        view.syntax(),
        index,
    )));
    code.push('\n');
    let units = trivia::units(view.syntax(), index);
    let prepared: Vec<Option<PreparedRule>> = units
        .iter()
        .map(|u| match &u.kind {
            UnitKind::Comment(_) => None,
            UnitKind::Node(n) => Some(prepare_rule(
                &RuleView::cast(n.clone()).expect("a state's item node is a RULE"),
                index,
            )),
        })
        .collect();
    // Every rule of the state, not only the on-grid ones: `grid_for`
    // does that filtering itself.
    let rules: Vec<&PreparedRule> = prepared.iter().flatten().collect();
    let grid = grid_for(&rules);
    let body: Vec<Rendered> = units
        .iter()
        .zip(&prepared)
        .map(|(u, p)| match (p, &u.kind) {
            (Some(p), _) => {
                Rendered::new(u.blank_before, render_rule(p, &grid, indent + INDENT_UNIT))
                    .with_trailing(u.trailing.as_ref())
            }
            (None, UnitKind::Comment(c)) => {
                Rendered::new(u.blank_before, comment_line(c, indent + INDENT_UNIT))
            }
            (None, UnitKind::Node(_)) => unreachable!("every node unit was prepared above"),
        })
        .collect();
    code.push_str(&flush(&body));
    code.push_str(&pad);
    code.push('}');
    Rendered::new(unit.blank_before, code).with_trailing(unit.trailing.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The differential oracle: the green printer and the C1 printer must
    /// agree byte for byte. A green walk that renders something BETTER still
    /// fails here, and that is the point — the C1 printer is the only
    /// reference that proves this rewrite faithful.
    #[track_caller]
    fn agrees(src: &str) {
        let green = format_green(src).expect("the green printer formats");
        let c1 = crate::fmt::format(src).expect("the C1 printer formats");
        assert_eq!(green, c1, "printers diverged for:\n{src}");
    }

    #[test]
    fn the_file_skeleton_agrees() {
        agrees("");
        agrees("use a::b;\n");
        agrees("use a::b, c::d as e;\n");
        agrees("// standalone\n\nuse a::b; // trailing\n");
        agrees("alphabet ab { '_', 'a' }\n");
        agrees("export alphabet ab { '_'..'z' }\n");
        agrees("? doc\n![deprecated] gone\nalphabet ab { '_' }\n");
        agrees("? doc\n\nalphabet ab { '_' }\n");
        agrees("namespace n {\n  namespace m {\n    alphabet ab { '_' }\n  }\n}\n");
        agrees("namespace n { // open\n  alphabet ab { '_' }\n} // close\n");
        agrees("use a::b;\n\n\nuse c::d;\n");
    }

    /// The unit's own `blank_before` is what the green walk prints, in
    /// place of C1's `leads_with_blank` branch — so a documented
    /// declaration that actually CARRIES a leading blank is the fixture
    /// that proves the substitution, and none of the skeleton sources
    /// above has one. The second source adds the near-edge asymmetry
    /// Task 2 pinned: a MULTI-LINE comment riding a `;` leaves the gap's
    /// near edge on the `;`, so the run below it reads as blank-separated
    /// even though only one newline separates them.
    #[test]
    fn a_documented_declarations_leading_blank_agrees() {
        agrees("alphabet a { '_' }\n\n? doc\nalphabet b { '_' }\n");
        agrees("use a; /* one\ntwo */\n? doc\nalphabet b { '0' }\n");
        agrees("alphabet a { '_' } /* one\ntwo */\n? doc\nalphabet b { '0' }\n");
    }

    /// A comment written inside a doc run — between two `?` lines, or
    /// between the last one and the declaration it documents — is one of
    /// the RUN's items (`crate::syntax::extract`'s `doc_run_tokens`), and
    /// the item after it measures its gap against the COMMENT. Both
    /// sources here are already canonical, so the whole assertion is that
    /// neither printer changes them: before the fix the green walk lost
    /// the comment outright and invented a blank line before `? more`.
    ///
    /// The last two sources cover the other half — for NAMESPACE (and
    /// STATE) the post-run comment is a direct child token ahead of the
    /// `{`, exactly where the container walk finds a comment the body
    /// drain would have claimed, so it takes a second reader to keep the
    /// two apart. The final one carries both at once: `/* a */` belongs
    /// to the run and `/* b */` — written after the keyword — to the
    /// body.
    #[test]
    fn a_comment_inside_a_doc_run_agrees() {
        agrees("? doc\n/* c */\nalphabet b { '0' }\n");
        agrees("? doc\n// c\n? more\nalphabet b { '0' }\n");
        agrees("? doc\n/* multi\nline */\n? more\nalphabet b { '0' }\n");
        agrees("? doc\n/* c1 */ /* c2 */\nalphabet b { '0' }\n");
        agrees("? doc\n\n/* c */\nalphabet b { '0' }\n");
        agrees("? doc\n/* c */\n\nalphabet b { '0' }\n");
        agrees("? doc\n/* a */\nnamespace n {\n  alphabet b { '0' }\n}\n");
        agrees("? doc\n/* a */\nnamespace /* b */ n {\n  alphabet b { '0' }\n}\n");
    }

    /// Every doc-run ITEM shape, over the surfaces this printer covers.
    /// The shipped fixture that exercises them — `docs_and_attention.tmc`
    /// — carries worlds, so it cannot go through `agrees` yet, and these
    /// two sources stand in for it: an empty `?` line, a blank line
    /// between two `?` lines (a `blank_before` on a DOC item rather than
    /// on a comment one), a bare empty `!`, a bare-prose `!`, and a
    /// tagged `![deprecated]`, which is the one item the parser wraps in
    /// its own ATTR node.
    ///
    /// The second source is the segmentation edge: a comment splits the
    /// run so that the ATTR is alone in the SECOND segment, where
    /// `parse_attr` and a freshly-reset `seen_deprecated` run over it
    /// rather than over the whole run. Every other fixture here leaves
    /// the ATTR in the first segment, where a whole-run reparse and a
    /// segmented one cannot differ.
    #[test]
    fn every_doc_run_item_shape_agrees() {
        agrees(
            "? one\n?\n? two\n\n? three\n!\n! bare prose\n![deprecated] gone\n\
             alphabet ab { '_' }\n",
        );
        agrees("? doc\n/* c */\n![deprecated] gone\nalphabet ab { '_' }\n");
    }

    /// The container walk starts its pre-brace scan at the declaring
    /// keyword, and these are the two sources that show why it cannot
    /// start at the node. `/* a */` belongs to the doc run; a scan from
    /// the node claims it a SECOND time, so both sources fail first on
    /// the plainest symptom there is — the comment is printed TWICE,
    /// once above `namespace` and once as the body's first item.
    ///
    /// Past the duplicate, the same scan drags the walk's near edge up
    /// onto `/* a */`'s own line, and that inverts exactly ONE gap per
    /// source — which is why one source cannot carry the claim:
    ///
    /// - **With a body comment** (the first source): `/* b */` gains a
    ///   blank line nobody wrote. The gap before `alphabet` is `true`
    ///   either way, so this source says nothing about it.
    /// - **Without one** (the second): there is no body comment to
    ///   absorb the moved edge, so the first real declaration gains the
    ///   invented blank instead — C1 prints `alphabet` tight against the
    ///   `{`.
    ///
    /// Both are written with three newlines after `/* a */`, because a
    /// shorter gap cannot separate the two candidate near edges at all.
    #[test]
    fn a_doc_runs_comment_does_not_move_the_body_walks_near_edge() {
        agrees("? doc\n/* a */\n\n\nnamespace\n/* b */\nn {\n  alphabet b { '0' }\n}\n");
        agrees("? doc\n/* a */\n\n\nnamespace n {\n  alphabet b { '0' }\n}\n");
    }

    /// The four copied layout rules no other source in this module can
    /// reach — verified by neutralizing each one and watching the whole
    /// suite stay green. Every function in this file is a verbatim copy
    /// of the C1 printer's, so these were coverage gaps rather than
    /// latent bugs; a copy nothing exercises is still a copy nothing
    /// would catch drifting.
    ///
    /// One source per rule, and each one is the minimum that reaches it:
    ///
    /// - **`trailing_spacing`'s alignment run** — two ADJACENT
    ///   single-line items that BOTH carry a trailing comment, which is
    ///   the whole precondition (`end - start >= 2`). `a` and `bb`
    ///   differ in width so the padding is actually computed rather than
    ///   uniform.
    /// - **`render_alphabet`'s width decision** — the one-line form of
    ///   the first is exactly 80 columns and stays inline; the second is
    ///   its one-character-longer twin at 81 and breaks. The pair pins
    ///   the boundary from both sides, which a single under-the-limit
    ///   source cannot do. One limit, measured rather than assumed: the
    ///   80-column source only bites when BOTH width tests move together.
    ///   Loosening the first alone is unobservable, because a
    ///   comment-free list falls through to the second branch, which
    ///   renders a bare `head { a, b }` byte-identically to the one-line
    ///   form it just declined. That is inherited from the C1 printer's
    ///   own shape, not a weakness of the fixture.
    /// - **`glyph_text`'s escape branch** — the only two characters the
    ///   lexer requires a backslash for, `'` and `\`.
    /// - **`normalize_comment_text`** — trailing spaces INSIDE a line
    ///   comment's text, which the printer strips and nothing else in
    ///   this module writes.
    #[test]
    fn the_copied_layout_rules_agree() {
        agrees("use a; // one\nuse bb; // two\n");
        agrees(
            "alphabet aaaaaaaaaaaaaaaaaaaaaaa { 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i' }\n",
        );
        agrees(
            "alphabet aaaaaaaaaaaaaaaaaaaaaaaa { 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i' }\n",
        );
        agrees("alphabet a { '\\'', '\\\\' }\n");
        agrees("use a; // one   \nuse bb; // two\n");
    }

    /// `machine`, `routine` and `graph` headers and bodies, the tape
    /// declarations' shared name column, and grafts and binds — the
    /// brief's own eight sources, every one of which parses (a world
    /// with no states is accepted).
    #[test]
    fn worlds_tapes_grafts_and_binds_agree() {
        agrees("alphabet ab { '_' }\nmachine {\n  tape main: ab;\n}\n");
        agrees(
            "alphabet ab { '_' }\nmachine {\n  tape m: ab;\n  tape longer: ab;\n  \
             volatile tape x: ab;\n}\n",
        );
        agrees(
            "alphabet ab { '_' }\nmachine { // open\n  tape main: ab; // trailing\n} // close\n",
        );
        // A blank line ENDS a tape run's shared name column, so the two
        // here size independently. Without that boundary both would pad
        // to the wider name.
        agrees("alphabet ab { '_' }\nmachine {\n  tape m: ab;\n\n  tape longerName: ab;\n}\n");
        agrees("alphabet ab { '_' }\nnamespace n {\n  routine r(tape t: ab) {\n  }\n}\n");
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  export graph g(tape t: ab, state done) \
             {\n  }\n}\n",
        );
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  export routine r(\n    \
             tape t: ab writes { '_' }\n  ) {\n  }\n}\n",
        );
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\n\
             machine {\n  tape main: ab;\n  \
             entry graft n::g(t = main, done = fin) as inst; // trailing\n  \
             bind n::g(t = main, done = fin) as other;\n}\n",
        );
        agrees(
            "alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\n\
             machine {\n  tape main: ab;\n  \
             bind n::g(t = main with map { '_' -> '_', 'a' -> 'a' }, d = fin) as one;\n}\n",
        );
    }

    /// **Hazard 1** — a comment between a `machine`/`routine`/`graph`
    /// header and its `{`. WORLD opens AT the brace, so the comment is
    /// the DECLARATION's child, one level up, and the head scan that
    /// solves this for NAMESPACE and STATE cannot see it; C1 body-drains
    /// it and prints it as the body's FIRST item.
    ///
    /// Three sources, because one cannot carry the claim:
    ///
    /// - `machine /* x */ {` — the plain relocation. Without the fix the
    ///   comment is dropped outright.
    /// - `machine // why\n{` — the comment's line sits ABOVE the brace's,
    ///   so the near edge moves BACKWARDS and the first real item gains a
    ///   blank line nobody wrote (`constraints.md`'s mandated quirk 1).
    ///   A fix that finds the comment but appends it to the item list
    ///   without moving the edge passes the first source and fails this
    ///   one.
    /// - `routine r(…) /* c */ {` — the REUSE twin, where the comment
    ///   sits after the signature's `)` rather than after a bare keyword.
    ///
    /// The scan runs BACKWARDS from WORLD deliberately: a comment written
    /// earlier in a REUSE header — `routine /* c */ r(…)` — belongs to
    /// the SIGNATURE's interior list, not the body, so it is excluded
    /// here and is a later surface's to render. It is in no source below.
    #[test]
    fn a_comment_between_a_world_header_and_its_brace_agrees() {
        agrees("alphabet ab { '_' }\nmachine /* x */ {\n  tape main: ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine // why\n{\n  tape main: ab;\n}\n");
        agrees("alphabet ab { '_' }\nnamespace n {\n  routine r(tape t: ab) /* c */ {\n  }\n}\n");
        // The pre-brace comment is a UNIT of the body, so it also shifts
        // every later unit's index — which is what `tape_name_widths`
        // keys off. A misaligned unit list moves the name column, and
        // nothing else here would catch it.
        agrees("alphabet ab { '_' }\nmachine /* x */ {\n  tape m: ab;\n  tape longer: ab;\n}\n");
        // Two of them, so the backwards scan's order is asserted: it
        // collects from the brace towards the keyword and reverses, and a
        // missing reverse prints them swapped.
        agrees("alphabet ab { '_' }\nmachine /* a */ /* b */ {\n  tape main: ab;\n}\n");
    }

    /// **A pre-brace comment SUPPRESSES the whole open run.** C1's
    /// `capture_open_trailing` pops from a global cursor and keeps only
    /// comments whose next significant token is past the `{`; a pre-brace
    /// comment is still pending, sits at the head of that cursor, and
    /// fails that test — so the loop breaks on its first iteration and
    /// takes NOTHING, however many comments ride the brace's own line.
    /// Both then print as body items in source order, and the near edge
    /// stays on the brace.
    ///
    /// Three symptoms, which is why every source below writes a comment
    /// on BOTH sides of the `{`: the two comments come out in the wrong
    /// ORDER, the brace-line one is wrongly RETAINED on the header line,
    /// and — when the pre-brace comment sits on an earlier line than the
    /// brace — the near edge moves backwards past a run that already
    /// advanced it, inventing a blank line C1 does not emit.
    ///
    /// The rule belongs to `open_run`, not to any one surface: the
    /// NAMESPACE twin is the same shape, and so are `routine`/`graph` and
    /// (once it renders) `state`. The `alphabet` twin cannot go through
    /// `agrees` — its pair lands in the element list's interior, a later
    /// surface — and is asserted directly in `super::trivia`'s own tests
    /// instead.
    #[test]
    fn a_pre_brace_comment_suppresses_the_open_run_and_agrees() {
        agrees("alphabet ab { '_' }\nmachine /* x */ { // open\n  tape main: ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine // why\n{ // open\n  tape main: ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine /* x */ { /* a */ /* b */\n  tape main: ab;\n}\n");
        agrees("alphabet ab { '_' }\nnamespace n /* x */ { // open\n  alphabet b { '0' }\n}\n");
        agrees("alphabet ab { '_' }\nnamespace n /* x */ { // open\n\n  alphabet b { '0' }\n}\n");
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  routine r(tape t: ab) /* c */ { // open\n               }\n}\n",
        );
        // The STATE twin, on printed BYTES. Task 4 fixed the rule that
        // governs it but had no state surface, so it could pin the shape
        // only on `trivia`'s unit stream — these are the first sources
        // that compare output. The second and third put the pre-brace
        // comment on an EARLIER line than the `{`; only the THIRD, whose
        // `{` carries no comment of its own, actually shows the near
        // edge moving backwards — measured, C1 prints
        //
        //     entry state s {        entry state s {
        //       // why                 // why
        //       // open       vs
        //       [*] -> stop;           [*] -> stop;
        //     }                      }
        //
        // so the `// open` of the second source occupies the gap and
        // suppresses the blank line the third one gains.
        agrees(
            "alphabet ab { '_' }\nmachine {\n  tape main: ab;\n  \
             entry state s /* x */ { // open\n    [*] -> stop;\n  }\n}\n",
        );
        agrees(
            "alphabet ab { '_' }\nmachine {\n  tape main: ab;\n  \
             entry // why\n  state s { // open\n    [*] -> stop;\n  }\n}\n",
        );
        agrees(
            "alphabet ab { '_' }\nmachine {\n  tape main: ab;\n  \
             entry // why\n  state s {\n    [*] -> stop;\n  }\n}\n",
        );
    }

    /// **Hazard 2** — a comment written INSIDE a `;`-terminated
    /// declaration. C1's `take_trailing` claims whatever is still PENDING
    /// at the `;`, and a comment inside the statement is pending there,
    /// so it is RELOCATED to after the `;`. It lives inside the node, so
    /// the container's sibling walk never sees it.
    ///
    /// The sources pin all four outcomes of that one rule, each of which
    /// a narrower fix gets wrong:
    ///
    /// - inside, same line as the `;`, not own-line → the node's trailing.
    /// - inside, NOT on the `;`'s line → an item BELOW the declaration
    ///   (`take_trailing` compares lines).
    /// - inside and own-line, even on the `;`'s line → an item
    ///   (`take_trailing` refuses an own-line comment).
    /// - two inside → the first is the trailing, the rest are items.
    ///
    /// The `/* a */: ab; /* b */` source is the one that pins "an inside
    /// comment is ahead of the container's own stream": C1 inspects only
    /// the FIRST pending comment, so `/* b */` is an item and not a
    /// second trailing.
    ///
    /// For a graft or a bind only the region AFTER the binding list's
    /// `)` is pending — everything at or before it was swept into that
    /// list's interior — so the sources put the comment around the
    /// `as NAME`, never inside the parentheses.
    #[test]
    fn a_comment_inside_a_semicolon_declaration_agrees() {
        let graph = "alphabet ab { '_' }\nnamespace n {\n  graph g(tape t: ab, state d) \
                     {\n  }\n}\n";
        agrees("alphabet ab { '_' }\nmachine {\n  tape main /* c */: ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine {\n  tape /* c */ main: ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine {\n  tape main /* c */\n    : ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine {\n  tape main\n    /* c */: ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine {\n  tape main /* c1 */ /* c2 */: ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine {\n  tape main /* a */: ab; /* b */\n}\n");
        agrees(&format!(
            "{graph}machine {{\n  tape main: ab;\n  \
             entry graft n::g(t = main, d = fin) /* c */ as inst;\n}}\n"
        ));
        agrees(&format!(
            "{graph}machine {{\n  tape main: ab;\n  \
             bind n::g(t = main, d = fin) as one /* c */;\n}}\n"
        ));
        agrees(&format!(
            "{graph}machine {{\n  tape main: ab;\n  \
             entry graft n::g(t = main, d = fin) /* c1 */ as inst /* c2 */;\n}}\n"
        ));
    }

    /// The world surfaces' own value and layout branches, one source per
    /// branch — the same audit `the_copied_layout_rules_agree` runs for
    /// the file skeleton.
    ///
    /// - **`paren_list`'s break** — a signature and a binding list each
    ///   long enough to cross the 80-column limit. The brief's own
    ///   sources all collapse to one line, so the multi-line body is a
    ///   copied branch nothing else reaches.
    /// - **`contract_clause_text`** — both keywords, a range element, and
    ///   the EMPTY clause, which prints `{}` with no inner space and is
    ///   the one shape `render_alphabet`'s body spacing does not mirror.
    /// - **`SigParamKind`'s two arms and the `volatile` prefix.**
    /// - **`binding_value_text`'s three shapes** — a bare name, a
    ///   terminator (all three of `return`/`stop`/`halt`), and a
    ///   `with map`, whose `MapArrow` has both spellings.
    /// - **`sym_text`'s number arm** — written digits, leading zeros
    ///   included, in an alphabet range and in a map pair.
    /// - **`render_graft`'s tail** — an entry graft may omit `as NAME`;
    ///   every other graft and every bind carries one.
    /// - **An empty signature**, which is `paren_list`'s
    ///   `entries.is_empty()` short-circuit.
    ///
    /// `paren_list`'s `has_multiline_entry` guard stays unreachable: only
    /// a nested `with map` broken by an interior comment produces an
    /// entry with a newline in it, and interior comments are a later
    /// surface. Deliberately not faked with a source that cannot reach
    /// it.
    #[test]
    fn the_world_value_and_layout_rules_agree() {
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  \
             export routine longRoutineName(tape firstTape: ab, tape secondTape: ab, \
             state doneState) {\n  }\n}\n",
        );
        agrees(
            "alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\n\
             machine {\n  tape main: ab;\n  \
             entry graft n::g(t = main with map { '_' -> '_', 'a' -> 'a' }, d = fin) \
             as instanceName;\n}\n",
        );
        agrees(
            "alphabet ab { '_', 'a' }\nnamespace n {\n  \
             routine r(volatile tape t: ab writes {} preserves { '_'..'a' }, state s) {\n  }\n}\n",
        );
        agrees("alphabet ab { '_' }\nnamespace n {\n  graph g() {\n  }\n}\n");
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\n\
             machine {\n  tape main: ab;\n  entry graft n::g(t = main, d = stop) as i;\n  \
             bind n::g(t = main, d = halt) as j;\n  bind n::g(t = main, d = return) as k;\n}\n",
        );
        agrees(
            "alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\n\
             machine {\n  tape main: ab;\n  \
             bind n::g(t = main with map { '_' => '_' }, d = fin) as one;\n}\n",
        );
        agrees(
            "alphabet nb { 00..05 }\nnamespace n {\n  graph g(tape t: nb, state d) {\n  }\n}\n\
             machine {\n  tape main: nb;\n  \
             bind n::g(t = main with map { 00 -> 01 }, d = fin) as one;\n}\n",
        );
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\n\
             machine {\n  tape main: ab;\n  entry graft n::g(t = main, d = fin);\n}\n",
        );
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  routine r(tape t: ab) { // open\n  } \
                // close\n}\n",
        );
    }

    /// A world's doc-run surfaces. `blank_before_decl` on GRAFT, BIND,
    /// REUSE and MACHINE is what the green walk substitutes for C1's
    /// `leads_with_blank`, and every source above is undocumented — so
    /// each declaration here carries a run, one with a real leading blank
    /// and one with a run→declaration gap, which are the two halves the
    /// substitution splits.
    #[test]
    fn a_documented_world_declaration_agrees() {
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\n\
             machine {\n  tape main: ab;\n\n  ? doc\n\n  \
             entry graft n::g(t = main, d = fin) as inst;\n  ? bdoc\n  \
             bind n::g(t = main, d = fin) as other;\n}\n",
        );
        agrees(
            "alphabet ab { '_' }\n\n? routine doc\n![deprecated] gone\nnamespace n {\n  \
             ? inner\n\n  routine r(tape t: ab) {\n  }\n}\n\n? machine doc\nmachine {\n  \
             tape main: ab;\n}\n",
        );
        // MACHINE and BIND with a real run→declaration gap. Every source
        // above writes its run tight against the keyword, where
        // `blank_before_decl` answering `false` unconditionally is
        // unobservable.
        agrees(
            "alphabet ab { '_' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\n\
             ? machine doc\n\nmachine {\n  tape main: ab;\n  ? bind doc\n\n  \
             bind n::g(t = main, d = fin) as one;\n}\n",
        );
    }

    /// States in block form, and the rule table inside one.
    ///
    /// Every state below is deliberately NOT a single-line candidate —
    /// each writes its rules on their own lines — because the
    /// single-line run is a later surface and C1 would render such a
    /// state inline. That includes the filler states a fixture only
    /// carries to parse: `state fin { [*] -> stop; }`, the natural
    /// shape, IS a candidate and must be written open.
    ///
    /// The sources, in order: the grid's column padding and the
    /// collapse of a skipped column; a state whose body carries a brace
    /// comment, an own-line comment, a rule trailing comment and a
    /// blank line — none of which splits the table; the `debugger`
    /// column, a `{expr}` substitution and `halt`; a documented state;
    /// and a ZERO-ROW state, which is valid, traps on entry, and must
    /// render as an empty body. The zero-row state is given a doc run
    /// precisely to keep it out of the single-line path, where C1 would
    /// print `state z { }`.
    #[test]
    fn block_states_and_the_grid_agree() {
        let head = "alphabet ab { '_', 'a', 'b' }\nmachine {\n  tape main: ab;\n";
        agrees(&format!(
            "{head}  entry state s {{\n    ['b'] -> write ['a'] move [>] goto s;\n    \
             ['a'] ->             move [>] goto s;\n    ['_'] -> stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state s {{ // open\n    // own-line\n    ['a'] -> stop; // trailing\n\n\
             \x20   ['_'] -> stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> debugger goto s;\n    \
             ['a'] -> write [{{0 + 1}}] move [.] stop;\n    ['_'] -> halt;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  ? documented\n  entry state s {{\n    [*] -> stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> goto s;\n  }}\n  ? empty\n  state z {{\n  }}\n}}\n"
        ));
        // A state's own CLOSE trailing. It is the same primitive
        // MACHINE and REUSE carry, but a different call site, and every
        // other source in this module reaches that primitive through
        // one of those two — so without these the one wire in
        // `render_block_state` that carries it has nothing on it. The
        // second source adds the near-edge half: a `}`'s trailing
        // comment ADVANCES the near edge (a `;`'s does not), so `state
        // z` reads as adjacent even though the comment spans two lines.
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> stop;\n  }} // close\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> goto z;\n  }} /* one\ntwo */\n  \
             state z {{\n    [*] -> stop;\n  }}\n}}\n"
        ));
    }

    /// A state's own doc-run halves, which the sources above cannot
    /// separate: `? documented` there is written tight against the
    /// keyword with no blank line above it, so `unit.blank_before`
    /// answering `false` unconditionally AND `blank_before_decl`
    /// answering `false` unconditionally would both go unnoticed. One
    /// source per half — a real leading blank, and a real
    /// run→declaration gap.
    #[test]
    fn a_documented_state_agrees() {
        let head = "alphabet ab { '_' }\nmachine {\n  tape main: ab;\n";
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> goto z;\n  }}\n\n  ? doc\n  \
             state z {{\n    [*] -> stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  ? doc\n\n  entry state s {{\n    [*] -> stop;\n  }}\n}}\n"
        ));
    }

    /// Runs of adjacent single-line states.
    ///
    /// The first two sources are the RUN itself. The first is the plain
    /// shape and pins nothing about sharing: both headers are 13 columns
    /// and both patterns 5, so a per-state header width and a per-state
    /// grid render it byte-identically. The second is the one that
    /// separates them — the names differ in length AND the patterns do,
    /// so the shorter header pads to the run's width and the narrower
    /// pattern pads to the run's column. The wide pattern is written in
    /// the SECOND member deliberately: with it in the first, a grid
    /// computed over that member alone is the same grid, and the source
    /// stops separating them.
    ///
    /// Then, one source per thing that ENDS or DISQUALIFIES a run:
    ///
    /// - a blank line between two members;
    /// - a doc run on the second (a documented state prints its run on
    ///   lines of its own, so it can never be a member);
    /// - an own-line comment BETWEEN two states — the unit that ends a
    ///   run the way C1's own comment item did;
    /// - an own-line comment INSIDE a state's body, which is
    ///   `inline_candidate`'s comment arm rather than the run scan's;
    /// - a comment on the `{`. Written so the rule STAYS on the header's
    ///   line: with the rule on a line of its own the line test already
    ///   excludes the state and the `open_trailing` clause decides
    ///   nothing;
    /// - a rule written on a line of its own, which is that line test;
    /// - a member that would cross the line limit, which drops the WHOLE
    ///   run to block form. Measured: the long member's inline line is
    ///   94 columns and the short one's 61, so a per-state check would
    ///   keep the second inline and split the table.
    ///
    /// `entry` alone on the line above its `state` keyword is the
    /// boundary of the line test itself: C1 records a state's line off
    /// its NAME span, so that state is still a candidate. Under the
    /// obvious alternative — the node's first token — it would not be,
    /// and every other source here would stay green.
    ///
    /// The last two sources are the inline path's own trailing comment:
    /// a `}`'s comment rides the state's line, and two ADJACENT inline
    /// states carrying one reach `trailing_spacing`'s alignment run,
    /// which a single-line state could not reach before this surface
    /// existed (a block state's code holds a newline, and the run
    /// requires every member to be one line).
    #[test]
    fn single_line_state_runs_agree() {
        let head = "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n";
        agrees(&format!(
            "{head}  entry state a {{ ['a'] -> goto b; }}\n  \
             state b {{ ['_'] -> stop; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state aa {{ [*] -> goto b; }}\n  \
             state b {{ ['_'..'a'] -> stop; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state a {{ ['a'] -> goto b; }}\n\n  \
             state b {{ ['_'] -> stop; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state a {{ ['a'] -> goto b; }}\n  ? doc\n  \
             state b {{ ['_'] -> stop; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state a {{ ['a'] -> goto b; }}\n  // c\n  \
             state b {{ ['_'] -> stop; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state a {{ ['a'] -> goto b;\n    // c\n  }}\n  \
             state b {{ ['_'] -> stop; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state a {{ /* c */ ['a'] -> stop; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state a {{ // c\n    ['a'] -> stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
             {{ ['a'] -> goto bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb; }}\n  \
             state bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb {{ ['_'] -> stop; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry\n  state a {{ ['a'] -> goto b; }}\n  \
             state b {{ ['_'] -> stop; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state a {{ ['a'] -> stop; }} // c\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state a {{ ['a'] -> goto b; }} // one\n  \
             state bb {{ ['_'] -> stop; }} // two\n}}\n"
        ));
    }

    /// The two shapes Task 5 measured and could not cover, each silent
    /// if missed.
    ///
    /// - **A BARE zero-row state is always a single-line candidate** —
    ///   `inline_candidate` is vacuously true over an empty rule list,
    ///   so C1 prints `state z { }`. Task 5's own zero-row source had to
    ///   carry a doc run to force block form, which is exactly what
    ///   keeps it out of this path.
    /// - **A rule whose pending comment was RELOCATED gains a trailing
    ///   comment, and C1 prints its state in block form.** The comment
    ///   sits inside the rule as written, so a predicate asking "does
    ///   the RULE node carry a comment?" answers a different question
    ///   and inlines the state — dropping the comment outright, since
    ///   the inline path prints no rule trailing at all. The second
    ///   source pairs the rule with a sibling that IS inline-capable, so
    ///   a wrong answer shows up as a mixed run rather than only as a
    ///   lost comment.
    #[test]
    fn a_bare_zero_row_state_inlines_and_a_relocated_trailing_blocks() {
        let head = "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n";
        agrees(&format!(
            "{head}  entry state s {{ [*] -> goto z; }}\n  state z {{ }}\n}}\n"
        ));
        agrees(&format!("{head}  entry state z {{ }}\n}}\n"));
        agrees(&format!(
            "{head}  entry state a {{ [*] -> stop /* c */; }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state a {{ ['a'] -> stop /* c */; }}\n  \
             state b {{ ['_'] -> stop; }}\n}}\n"
        ));
    }

    /// [`inline_candidate`]'s off-grid clause, asserted DIRECTLY — for
    /// the same reason [`breaks_the_grid`] is: the only thing that sends
    /// a rule off the grid is a LINE or own-line comment inside a glyph
    /// vector, and this printer does not render a vector's interior
    /// comments yet, so such a source diverges whichever way the clause
    /// answers and a differential case would prove nothing about it.
    ///
    /// The clause is not implied by the line test: a rule may START on
    /// the header's own line and still run off the grid, because its
    /// broken vector spans the lines BELOW. Measured against C1, which
    /// prints the first source's state in block form and the second's —
    /// a same-line block comment, the one interior comment a vector
    /// keeps inline — on one line.
    #[test]
    fn an_off_grid_rule_keeps_its_state_out_of_a_run() {
        let head = "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n";
        for (body, expect) in [
            (
                "[*] -> write [\n      /* c */\n      'a'\n    ] stop;",
                false,
            ),
            ("[*] -> write [/* c */ 'a'] stop;", true),
        ] {
            let src = format!("{head}  entry state a {{ {body} }}\n}}\n");
            let tokens = lex_with(&src, LexMode::WithComments).expect("lexes");
            let green = parse_green_from_tokens(&src, &tokens).expect("parses");
            let root = SyntaxNode::new_root(green);
            let index = TextLineIndex::new(&src);
            let mut stack: Vec<SyntaxNode> = root.children().collect();
            let mut state = None;
            while let Some(n) = stack.pop() {
                if let Some(v) = StateView::cast(n.clone()) {
                    state = Some(v);
                    break;
                }
                stack.extend(n.children());
            }
            let state = state.expect("a STATE");
            assert_eq!(inline_candidate(&state, &index), expect, "for:\n{src}");
        }
    }

    /// **Hazard 2, on a RULE.** A rule is `;`-terminated like a `tape`
    /// or a `bind`, and it runs several interior drains of its own — the
    /// pattern's, each glyph vector's, and, for a `call`, its binding
    /// list's. Everything written past the LAST of them is still pending
    /// when C1 reaches the `;`, so `take_trailing` relocates it:
    ///
    /// ```text
    /// [*] -> stop /* c */;   prints   [*] -> stop; /* c */
    /// ```
    ///
    /// Nothing in the shipped corpus or the adversarial set writes that
    /// shape, so before these sources existed the green walk dropped
    /// such a comment with the whole suite staying green.
    ///
    /// The sources walk the boundary outwards from the shortest rule to
    /// the longest, because each shape moves the last drain:
    ///
    /// - no action at all → the boundary is the pattern's `]`, so a
    ///   comment after it, after the `->`, or after a bare `debugger`
    ///   is pending.
    /// - a `write` but no `move` → the boundary is the write vector's
    ///   `]`.
    /// - a `call` → the boundary is the binding list's `)`, which lies
    ///   INSIDE the TRANSITION node, and the pending comment can sit
    ///   after that `)`, after the `then`, or after the continuation.
    ///   This is the furthest a pending comment travels.
    ///
    /// The last two sources cover `take_trailing`'s two refusals, the
    /// same pair `a_comment_inside_a_semicolon_declaration_agrees` pins
    /// for a `tape`: a comment not on the `;`'s line, and an own-line
    /// one — both fall through to the state's own drain and print as
    /// items BELOW the rule.
    #[test]
    fn a_comment_inside_a_rule_agrees() {
        let head = "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n";
        let call_head = "alphabet ab { '_', 'a' }\nnamespace n {\n  routine r(tape t: ab) {\n    \
                         entry state q {\n      [*] -> stop;\n    }\n  }\n}\nmachine {\n  \
                         tape main: ab;\n";
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> stop /* c */;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state s {{\n    [*] /* c */ -> stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> /* c */ stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> debugger /* c */ stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> write ['a'] /* c */ stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{call_head}  entry state s {{\n    \
             [*] -> call n::r(t = main) /* c */ then stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{call_head}  entry state s {{\n    \
             [*] -> call n::r(t = main) then /* c */ stop;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{call_head}  entry state s {{\n    \
             [*] -> call n::r(t = main) then stop /* c */;\n  }}\n}}\n"
        ));
        // Two of them: the first is the trailing, the rest drain as
        // items — C1 inspects only the FIRST pending comment.
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> stop /* a */ /* b */;\n  }}\n}}\n"
        ));
        // Declined on the line test, and declined on `own_line`.
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> stop /* c */\n      ;\n  }}\n}}\n"
        ));
        agrees(&format!(
            "{head}  entry state s {{\n    [*] -> stop\n      /* c */;\n  }}\n}}\n"
        ));
    }

    /// The rule surface's own value and layout branches, one source per
    /// branch — the audit `the_copied_layout_rules_agree` runs for the
    /// file skeleton and `the_world_value_and_layout_rules_agree` for a
    /// world.
    ///
    /// - **`pattern_cell_text`'s three kinds and its `as` binding** — a
    ///   wildcard, a literal, a range, and a bound cell.
    /// - **`write_cell_text`'s three kinds** — `-` (keep), a literal,
    ///   and a `{expr}` substitution. The substitution is written
    ///   PARENTHESIZED and spaced (`{(v)*2 + 1}`): it reprints tight but
    ///   keeps its own parens, which is what separates reprinting from
    ///   the source tokens from re-deriving the expression from its
    ///   parsed value. A second, bare `{v}` cell in the same vector pins
    ///   that the span filter selects one cell rather than the whole
    ///   vector.
    /// - **`move_cell_text`'s three directions.**
    /// - **`sym_text`'s number arm**, in a pattern range and in a write
    ///   cell.
    /// - **`transition_text`'s six arms** — explicit `goto`, the bare
    ///   name sugar, `return`, `stop`, `halt`, and a `call`; plus
    ///   `Stay`, the OMITTED transition, whose `;` abuts the last
    ///   action.
    /// - **`grid_for`'s zero-width collapse** — the third rule uses
    ///   neither vector, so it pads to the pattern column and stops.
    /// - **`paren_list` inside a transition** — the second source's
    ///   `call` is long enough to break, which puts the closing `)` back
    ///   at the transition's own column rather than at the rule's.
    ///
    /// `render_rule_off_grid` (and with it `glyph_vec_multiline` and
    /// `col_after`) stays UNREACHABLE from this harness, deliberately
    /// unfaked: the only thing that sends a rule off the grid is a LINE
    /// or own-line comment inside one of its glyph vectors, and this
    /// printer does not render a vector's interior comments yet — so any
    /// source that reaches the branch also diverges by the dropped
    /// comment. `breaks_the_grid` itself is asserted directly in
    /// `super::trivia`'s own tests instead.
    #[test]
    fn the_rule_value_and_layout_rules_agree() {
        agrees(
            "alphabet nb { 00..05 }\nnamespace n {\n  routine r(tape t: nb, tape u: nb) {\n    \
             entry state q {\n      [00 as v, *] -> write [{(v)*2 + 1}, {v}] move [<, .] return;\n\
             \x20     [01..03, *] -> write [-, 00] move [>, >];\n      [*, *] -> q;\n    }\n  }\n\
             }\nmachine {\n  tape main: nb;\n  tape aux: nb;\n  entry state s {\n    \
             [*, *] -> debugger call n::r(t = main, u = aux) then fin;\n  }\n  state fin {\n    \
             [*, *] -> halt;\n  }\n}\n",
        );
        agrees(
            "alphabet nb { 00..05 }\nnamespace nnnnnnnnnnnnnn {\n  \
             routine rrrrrrrrrrrrrrrrrrrr(tape ttttttttttt: nb, tape uuuuuuuuuuu: nb) {\n    \
             entry state q {\n      [*, *] -> stop;\n    }\n  }\n}\nmachine {\n  \
             tape main: nb;\n  tape aux: nb;\n  entry state s {\n    \
             [*, *] -> call nnnnnnnnnnnnnn::rrrrrrrrrrrrrrrrrrrr(ttttttttttt = main, \
             uuuuuuuuuuu = aux) then stop;\n  }\n}\n",
        );
    }

    /// [`breaks_the_grid`], asserted directly. It cannot be reached
    /// through `agrees`: the only thing that sends a rule off the grid
    /// is a LINE or own-line comment inside one of its glyph vectors,
    /// and such a comment is one this printer does not render yet — so
    /// the source diverges whichever way the predicate answers, and a
    /// differential case would prove nothing about it.
    ///
    /// The four sources are the predicate's whole surface, one boundary
    /// each: a same-line BLOCK comment stays on the grid (the one
    /// interior comment a glyph vector can hold inline); a LINE comment
    /// does not, because nothing can follow `//` on its line; an
    /// own-line BLOCK comment does not either, because inlining it would
    /// flip its own `own_line` flag on the next parse; and a comment in
    /// the vector's HEADER — `write /* c */ [` — counts as the vector's
    /// too, which is the old parser's reading and not the bracket's.
    #[test]
    fn a_line_or_own_line_comment_in_a_glyph_vector_takes_the_rule_off_the_grid() {
        let head = "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    ";
        for (rule, expect) in [
            ("[*] -> write [/* c */ 'a'] stop;", false),
            ("[*] -> write [\n      // c\n      'a'\n    ] stop;", true),
            (
                "[*] -> write [\n      /* c */\n      'a'\n    ] stop;",
                true,
            ),
            ("[*] -> write /* c */ ['a'] stop;", false),
            ("[*] -> write\n      // c\n      ['a'] stop;", true),
        ] {
            let src = format!("{head}{rule}\n  }}\n}}\n");
            let tokens = lex_with(&src, LexMode::WithComments).expect("lexes");
            let green = parse_green_from_tokens(&src, &tokens).expect("parses");
            let root = SyntaxNode::new_root(green);
            let mut stack: Vec<SyntaxNode> = root.children().collect();
            let mut rule_node = None;
            while let Some(n) = stack.pop() {
                if n.kind() == crate::syntax::TmcKind::Rule.into() {
                    rule_node = Some(n);
                    break;
                }
                stack.extend(n.children());
            }
            let rule_node = rule_node.expect("a RULE");
            assert_eq!(breaks_the_grid(&rule_node), expect, "for:\n{rule}");
        }
    }

    /// The whole adversarial set, over the surfaces this printer covers.
    /// The fixtures the file skeleton cannot yet render (anything with a
    /// world) are excluded by name rather than by a `catch_unwind`, so a
    /// fixture that starts failing for a REAL reason is not silently
    /// swallowed.
    #[test]
    fn the_skeleton_only_adversarial_sources_agree() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fmt_adversarial");
        for name in [
            "divergence_semicolon_block_comment",
            "doc_run_interior_comment",
        ] {
            let path = format!("{dir}/{name}.tmc");
            let src = std::fs::read_to_string(&path).expect("a readable fixture");
            agrees(&src);
        }
    }
}
