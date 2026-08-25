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
use crate::lexer::{Comment, CommentKind, LexMode, lex_with};
use crate::parser::{
    AlphabetElem, BindingArg, BindingValue, ContractClause, Import, MapArrow, SigParam,
    SigParamKind, SymLit, SymMap, TermKind, parse_green_from_tokens, reparse_sig_param,
};
use crate::syntax::extract::{
    extract_alphabet, extract_bind, extract_doc_items, extract_graft, extract_import, sig_tokens,
};
use crate::syntax::{
    AlphabetView, BindView, DocRunView, GraftView, MachineView, NamespaceView, ReuseKind,
    ReuseView, RootView, TapeView, TopView, UseView, WorldView,
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
fn render_world_items(
    world: &SyntaxNode,
    indent: usize,
    source: &str,
    index: &TextLineIndex,
) -> Vec<Rendered> {
    let units = trivia::units(world, index);
    let tape_names = tape_name_widths(&units);
    units
        .iter()
        .enumerate()
        .map(|(i, unit)| render_world_item(unit, tape_names[i], indent, source, index))
        .collect()
}

fn render_world_item(
    unit: &Unit,
    name_width: usize,
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
    } else {
        unimplemented!(
            "the green printer covers a world's tapes, grafts and binds; states arrive \
             with their own surface"
        )
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
