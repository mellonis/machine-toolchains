//! `.tmc` pretty-printer — the TM-1 twin of the PM-1 crate's `.pmc`
//! formatter, and a thin renderer in the same sense: [`format`] returns a
//! `Result` and never prints, never touches the filesystem; `cli/fmt.rs` is
//! the only place a diagnostic or a file write happens.
//!
//! # The contract
//!
//! The printer walks the lossless CST ([`crate::parser::parse_cst`]) rather
//! than the flattened AST, which buys the four properties the fmt battery
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
//! - **Trivia-preserving** — every comment is reprinted: own-line comments at
//!   their block's indent, same-line trailing comments riding their line,
//!   brace-line comments riding the `{`/`}` they were written on. Doc (`?`)
//!   and attention (`!`) runs — `[deprecated]` included — stay directly above
//!   the declaration they document, in source order.
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

use crate::compiler::CompileError;
use crate::cst::{
    AlphabetCst, BindCst, Cst, DocRunItem, DocRunKind, GraftCst, MachineCst, NamespaceCst,
    ReuseCarrier, ReuseCst, RuleItem, RuleKind, StateCst, TapeCst, TopItem, TopKind, UseCst,
    UsePath, WorldItem, WorldKind,
};
use crate::lexer::{Comment, LexMode, lex_with};
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
    Ok(print_cst(&cst))
}

fn print_cst(cst: &Cst) -> String {
    let out = flush(&render_top_items(&cst.items, 0));
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

fn write_vec_text(vec: &WriteVec) -> String {
    let cells: Vec<String> = vec
        .cells
        .iter()
        .map(|cell| match &cell.kind {
            WriteCellKind::Keep => "-".to_string(),
            WriteCellKind::Lit(sym) => sym_text(sym),
            WriteCellKind::Subst { name, delta, .. } => match delta.cmp(&0) {
                std::cmp::Ordering::Equal => format!("{{{name}}}"),
                std::cmp::Ordering::Greater => format!("{{{name}+{delta}}}"),
                std::cmp::Ordering::Less => format!("{{{name}-{}}}", delta.unsigned_abs()),
            },
        })
        .collect();
    format!("write [{}]", cells.join(", "))
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

fn binding_arg_text(arg: &BindingArg) -> String {
    format!("{} = {}", arg.name, binding_value_text(&arg.value))
}

fn binding_value_text(value: &BindingValue) -> String {
    match value {
        BindingValue::Named { target, map, .. } => match map {
            Some(map) => format!("{target} {}", sym_map_text(map)),
            None => target.clone(),
        },
        BindingValue::Terminator { kind, .. } => term_text(*kind).to_string(),
    }
}

fn sym_map_text(map: &SymMap) -> String {
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
    format!("with map {{ {} }}", pairs.join(", "))
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
/// caller opening a line emits that indent before calling.
fn paren_list(col: usize, head: &str, entries: &[String], tail: &str) -> String {
    let one_line = format!("{head}({}){tail}", entries.join(", "));
    if entries.is_empty() || col + one_line.chars().count() <= LINE_WIDTH {
        return one_line;
    }
    let entry_pad = " ".repeat(col + INDENT_UNIT);
    let mut out = format!("{head}(\n");
    for (i, entry) in entries.iter().enumerate() {
        out.push_str(&entry_pad);
        out.push_str(entry);
        if i + 1 < entries.len() {
            out.push(',');
        }
        out.push('\n');
    }
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

fn grid_for(rules: &[&Rule]) -> Grid {
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
            .filter_map(|r| r.write.as_ref().map(|w| width(&write_vec_text(w))))
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
fn render_rule(rule: &Rule, grid: &Grid, indent: usize) -> String {
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
            rule.write.as_ref().map(write_vec_text).unwrap_or_default(),
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
    line.push_str(&transition_text(&rule.transition, col));
    line.push(';');
    line
}

/// A transition, starting at column `col` — the column an argument list
/// breaks against.
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
            let entries: Vec<String> = args.iter().map(binding_arg_text).collect();
            let head = format!("call {}", target.joined());
            // The `;` the caller appends is reserved by rendering it into the
            // tail used for the fit measurement.
            let tail = format!(" then {};", continuation_text(then));
            let rendered = paren_list(col, &head, &entries, &tail);
            rendered
                .strip_suffix(';')
                .expect("the tail ends in the reserved `;`")
                .to_string()
        }
        Transition::Return { .. } => "return".to_string(),
        Transition::Stop { .. } => "stop".to_string(),
        Transition::Halt { .. } => "halt".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Top-level items.
// ---------------------------------------------------------------------------

fn render_top_items(items: &[TopItem], indent: usize) -> Vec<Rendered> {
    items
        .iter()
        .map(|item| render_top_item(item, indent))
        .collect()
}

fn render_top_item(item: &TopItem, indent: usize) -> Rendered {
    match &item.kind {
        TopKind::Comment(c) => Rendered::new(item.blank_before, comment_line(c, indent)),
        TopKind::Import(u) => render_use(u, item.blank_before, indent),
        TopKind::Alphabet(a) => render_alphabet(a, item.blank_before, indent),
        TopKind::Namespace(ns) => render_namespace(ns, item.blank_before, indent),
        TopKind::Reuse(r) => render_reuse(r, item.blank_before, indent),
        TopKind::Machine(m) => render_machine(m, item.blank_before, indent),
    }
}

fn render_use(u: &UseCst, blank_before: bool, indent: usize) -> Rendered {
    let paths: Vec<String> = u.paths.iter().map(use_path_text).collect();
    let code = format!("{}use {};", " ".repeat(indent), paths.join(", "));
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
    let one_line = format!("{head} {{ {} }}", entries.join(", "));
    // A comment on the `{` forces the body onto its own lines, whatever the
    // width says.
    if a.open_trailing.is_empty() && one_line.chars().count() <= LINE_WIDTH {
        code.push_str(&one_line);
    } else {
        code.push_str(&head);
        code.push_str(" {");
        code.push_str(&open_trailing_text(&a.open_trailing));
        code.push('\n');
        let entry_pad = " ".repeat(indent + INDENT_UNIT);
        for (i, entry) in entries.iter().enumerate() {
            code.push_str(&entry_pad);
            code.push_str(entry);
            if i + 1 < entries.len() {
                code.push(',');
            }
            code.push('\n');
        }
        code.push_str(&pad);
        code.push('}');
    }
    Rendered::new(leads_with_blank(blank_before, &a.doc_run), code)
        .with_trailing(a.close_trailing.as_ref())
}

fn render_namespace(ns: &NamespaceCst, blank_before: bool, indent: usize) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(&ns.doc_run, indent, blank_before);
    code.push_str(&format!("{pad}namespace {} {{", ns.name));
    code.push_str(&open_trailing_text(&ns.open_trailing));
    code.push('\n');
    code.push_str(&flush(&render_top_items(&ns.items, indent + INDENT_UNIT)));
    code.push_str(&pad);
    code.push('}');
    Rendered::new(leads_with_blank(blank_before, &ns.doc_run), code)
        .with_trailing(ns.close_trailing.as_ref())
}

fn render_reuse(r: &ReuseCst, blank_before: bool, indent: usize) -> Rendered {
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
    code.push_str(&paren_list(indent, &head, &signature_params(&r.sig), " {"));
    code.push_str(&open_trailing_text(&r.open_trailing));
    code.push('\n');
    code.push_str(&flush(&render_world_items(&r.items, indent + INDENT_UNIT)));
    code.push_str(&pad);
    code.push('}');
    Rendered::new(leads_with_blank(blank_before, &r.doc_run), code)
        .with_trailing(r.close_trailing.as_ref())
}

fn render_machine(m: &MachineCst, blank_before: bool, indent: usize) -> Rendered {
    let pad = " ".repeat(indent);
    let mut code = doc_run_text(&m.doc_run, indent, blank_before);
    code.push_str(&format!("{pad}machine {{"));
    code.push_str(&open_trailing_text(&m.open_trailing));
    code.push('\n');
    code.push_str(&flush(&render_world_items(&m.items, indent + INDENT_UNIT)));
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
fn render_world_items(items: &[WorldItem], indent: usize) -> Vec<Rendered> {
    let inline = inline_state_runs(items, indent);
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
                Some(shape) => render_inline_state(s, shape, item.blank_before, indent),
                None => render_block_state(s, item.blank_before, indent),
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
/// header's own line, no interior comment, no comment on the `{`.
fn inline_candidate(state: &StateCst) -> bool {
    state.open_trailing.is_empty()
        && state.rules.iter().all(|item| match &item.kind {
            RuleKind::Comment(_) => false,
            RuleKind::Rule(r) => r.rule.line == state.line && r.trailing.is_none(),
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
fn inline_state_runs(items: &[WorldItem], indent: usize) -> Vec<Option<InlineShape>> {
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
        let grid = grid_for(&rules);
        let fits = states
            .iter()
            .all(|s| inline_state_line(s, header, &grid, indent).chars().count() <= LINE_WIDTH);
        if fits {
            // The run's SHARED grid is what every member prints with — that
            // is what makes a block of one-line states read as one table.
            for offset in 0..states.len() {
                out[start + offset] = Some(InlineShape {
                    header,
                    grid: grid_for(&rules),
                });
            }
        }
        i = end;
    }
    out
}

fn inline_state_line(state: &StateCst, header_width: usize, grid: &Grid, indent: usize) -> String {
    let header = state_header_text(state);
    let mut line = format!(
        "{}{header}{} {{",
        " ".repeat(indent),
        " ".repeat(header_width.saturating_sub(header.chars().count()))
    );
    for item in &state.rules {
        if let RuleKind::Rule(r) = &item.kind {
            line.push(' ');
            line.push_str(&render_rule(&r.rule, grid, 0));
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
) -> Rendered {
    let code = inline_state_line(state, shape.header, &shape.grid, indent);
    Rendered::new(blank_before, code).with_trailing(state.close_trailing.as_ref())
}

fn render_block_state(state: &StateCst, blank_before: bool, indent: usize) -> Rendered {
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
    let grid = grid_for(&rules);
    let body: Vec<Rendered> = state
        .rules
        .iter()
        .map(|item| render_rule_item(item, &grid, indent + INDENT_UNIT))
        .collect();
    code.push_str(&flush(&body));
    code.push_str(&pad);
    code.push('}');
    Rendered::new(leads_with_blank(blank_before, &state.doc_run), code)
        .with_trailing(state.close_trailing.as_ref())
}

fn render_rule_item(item: &RuleItem, grid: &Grid, indent: usize) -> Rendered {
    match &item.kind {
        RuleKind::Comment(c) => Rendered::new(item.blank_before, comment_line(c, indent)),
        RuleKind::Rule(r) => Rendered::new(item.blank_before, render_rule(&r.rule, grid, indent))
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
    let entries: Vec<String> = g.args.iter().map(binding_arg_text).collect();
    code.push_str(&" ".repeat(indent));
    code.push_str(&paren_list(indent, &head, &entries, &tail));
    Rendered::new(leads_with_blank(blank_before, &g.doc_run), code)
        .with_trailing(g.trailing.as_ref())
}

fn render_bind(b: &BindCst, blank_before: bool, indent: usize) -> Rendered {
    let mut code = doc_run_text(&b.doc_run, indent, blank_before);
    let head = format!("bind {}", b.target.joined());
    let tail = format!(" as {};", b.as_name.0);
    let entries: Vec<String> = b.args.iter().map(binding_arg_text).collect();
    code.push_str(&" ".repeat(indent));
    code.push_str(&paren_list(indent, &head, &entries, &tail));
    Rendered::new(leads_with_blank(blank_before, &b.doc_run), code)
        .with_trailing(b.trailing.as_ref())
}

#[cfg(test)]
mod tests;
