//! Lossless assembly CST (docs/formats.md (assembly text)). Total:
//! every text parses — lines that are not assembly-shaped become Raw
//! nodes. Trivia-complete: comments with columns, blank-line presence,
//! raw text, and — for the three list directives a trailing comma may
//! continue onto the next line — the physical lines the directive was
//! written across, verbatim. Validity checking lives in lower.rs, not
//! here.
//!
//! An item is normally one physical line. The one exception is that
//! continuation, and it never renumbers anything: line numbers stay
//! PHYSICAL throughout, because diagnostics, the debug line map and both
//! editor services all read them.

use super::lexer::{AsmToken, AsmTokenKind, lex_line};
use super::syntax::AsmCaps;
use crate::diagnostics::{Pos, Span};

// ---------------------------------------------------------------------
// Directive words — the single spelling of every directive the
// assembler framework recognizes (docs/formats.md (assembly text)).
// The recognizers consult these consts — the `.rept` block opener and
// `shape_line`'s branches here, the `.func`/`.byte`/malformed-directive
// paths in lower.rs — never a re-spelled literal, so the public
// inventory below is assembled from the same words recognition reads.
//
// Residual risk, stated honestly: recognition is structurally scattered
// (a block opener, several shaping branches, lowering paths), so a
// future directive COULD bypass this block with an inline literal and
// stay invisible to [`recognized_directives`]. The convention is
// load-bearing; the `recognized_directives_match_the_real_recognizer`
// test pins every listed word's caps tier to the real assembler, and
// the editor-grammar drift guards set-compare the inventory against
// each dialect's grammar.

pub(crate) const FUNC_WORD: &str = ".func";
pub(crate) const BYTE_WORD: &str = ".byte";
pub(crate) const SECTION_WORD: &str = ".section";
pub(crate) const ROUTINE_WORD: &str = ".routine";
pub(crate) const REPT_WORD: &str = ".rept";
pub(crate) const ENDR_WORD: &str = ".endr";
pub(crate) const ROW_WORD: &str = ".row";
pub(crate) const TARGETS_WORD: &str = ".targets";
pub(crate) const TARGET_WORD: &str = ".target";
pub(crate) const FRAME_WORD: &str = ".frame";
pub(crate) const MAP_WORD: &str = ".map";
pub(crate) const EXITS_WORD: &str = ".exits";

/// The frame-descriptor directive family, shaped under
/// [`AsmCaps::tables`] and reported precisely by lower when malformed.
pub(crate) const FRAME_DIRECTIVE_WORDS: [&str; 3] = [FRAME_WORD, MAP_WORD, EXITS_WORD];

/// The directives whose operand list may continue onto the next physical
/// line when the line ends in a comma (docs/formats.md (assembly text)).
/// These three are the grammar's only unbounded lists: a dispatch table's
/// targets, a frame's exits, and a frame's symbol maps all grow with the
/// program. Every other comma-separated region is bounded — `.row`,
/// `alpha=(…)` and `.frame tapes=(…)` by the tape count, `.target` and
/// `.section` by taking a single operand — so none of them can reach a
/// width where a line break earns its keep, and a trailing comma stays
/// the error it has always been there.
///
/// All three ride [`AsmCaps::tables`], so a dialect without that
/// capability never reaches the fold at all.
const CONTINUABLE_WORDS: [&str; 3] = [TARGETS_WORD, EXITS_WORD, MAP_WORD];

/// Every directive word the assembler framework recognizes under
/// `caps`, sorted (docs/formats.md (assembly text)). `.func` and
/// `.byte` are the caps-independent classic surface; the
/// section/table/signature/frame family rides [`AsmCaps::tables`];
/// `.rept`/`.endr` ride [`AsmCaps::rept`]; the vectors cap adds operand
/// tokens, never directives.
///
/// This is the drift-guard authority: the editor-grammar suites
/// set-compare the directive words each dialect's grammar paints
/// against this list under that dialect's caps, so a directive added
/// to the assembler with no grammar entry fails a test — and a
/// directive invented in a grammar fails the same test the other way.
pub fn recognized_directives(caps: AsmCaps) -> Vec<&'static str> {
    let mut words = vec![FUNC_WORD, BYTE_WORD];
    if caps.tables {
        words.extend([
            SECTION_WORD,
            ROUTINE_WORD,
            ROW_WORD,
            TARGETS_WORD,
            TARGET_WORD,
        ]);
        words.extend(FRAME_DIRECTIVE_WORDS);
    }
    if caps.rept {
        words.extend([REPT_WORD, ENDR_WORD]);
    }
    words.sort_unstable();
    words
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmCst {
    pub items: Vec<AsmItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmItem {
    pub blank_before: bool,
    pub kind: AsmItemKind,
    /// The physical lines this item was written across, present only when
    /// a trailing comma continued a `.targets`/`.exits`/`.map` list onto
    /// the next line (docs/formats.md (assembly text)) — two entries or
    /// more, in source order. `None` is the ordinary one-line item, which
    /// is every item a dialect without [`AsmCaps::tables`] can produce.
    ///
    /// This is what keeps the join lossless: `kind` holds the shaped
    /// directive as though it had been written on one line, and these
    /// entries hold the region exactly as the author typed it, each
    /// segment's own indentation and the trailing comment included.
    pub continuation: Option<Vec<ContinuedLine>>,
}

/// One physical line of an item written across several (see
/// [`AsmItem::continuation`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuedLine {
    /// 1-based PHYSICAL line number. Joining lines never renumbers
    /// anything: every span inside the shaped item, and every entry here,
    /// still names the line the text actually sits on, so diagnostics, the
    /// debug line map and the editor services all keep pointing at the
    /// right place.
    pub line_no: u32,
    /// The line exactly as written, its newline excluded — so joining an
    /// item's entries with newlines reproduces the source region byte for
    /// byte.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsmItemKind {
    /// Own-line comment: `; text`.
    Comment(AsmComment),
    /// `.func name [local]` — only when structurally exact; otherwise
    /// the line lands in Line (word ".func") and lower.rs reports the
    /// precise legacy error.
    Func(FuncCst),
    /// labels + optional instruction (label-only lines have instr: None).
    Line(LineCst),
    /// Not assembly-shaped (first token isn't a Word, or a Junk token
    /// is present). Lossless: the verbatim line text.
    Raw(RawCst),
    /// `.section NAME` region marker — shaped only under
    /// [`AsmCaps::tables`], and only when structurally exact (the
    /// `.section` word plus one Word name); anything else starting
    /// `.section` stays a Line, mirroring the `.func` degradation.
    Section(SectionCst),
    /// `.row [..]` / `.targets L1, ..` / `.target L` — shaped only under
    /// [`AsmCaps::tables`]. A `.row` whose region is not a single
    /// bracketed vector degrades to a Line (mirror `.func`).
    TableDirective(TableDirectiveCst),
    /// `.rept v, lo, hi` … `.endr` block — shaped only under
    /// [`AsmCaps::rept`]. The body is kept AS WRITTEN (unexpanded).
    /// Degradations (all mirror the malformed-`.func` path — the header
    /// or delimiter stays a Line and lower reports the error): a nested
    /// `.rept` inside a body does not open a block; an unterminated
    /// `.rept` degrades its header to a Line; a stray `.endr` shapes as a
    /// Line.
    Rept(ReptCst),
    /// `.routine <name>, tapes=<int>, alpha=(<int>, …)` — a
    /// generic-routine signature declaration (docs/formats.md (MO)),
    /// shaped under [`AsmCaps::tables`] and only when structurally
    /// exact; anything else starting `.routine` stays a Line (mirror
    /// `.func`). Token gating in practice needs BOTH the tables and
    /// rept caps: `=` lexes under tables, `(`/`)` under rept — with
    /// either cap off some field character stays Junk and the line
    /// shapes Raw.
    RoutineDirective(RoutineDirectiveCst),
    /// `.frame`/`.map`/`.exits` — the frame-descriptor directive family
    /// (docs/formats.md (frame descriptors)), shaped under
    /// [`AsmCaps::tables`] (+ `rept` for the `(..)` groups, + the arrow
    /// tokens the tables cap also gates) and only when structurally exact;
    /// anything else starting one of these words stays a Line (mirror
    /// `.func`). `.frame` carries the descriptor label; `.map`/`.exits`
    /// continue the open frame group.
    FrameDirective(FrameDirectiveCst),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmComment {
    pub text: String, // incl. `;`
    pub col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrailingComment {
    pub text: String, // incl. `;`
    pub col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncCst {
    pub name: String,
    pub name_span: Span,
    pub local: bool,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelCst {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineCst {
    pub labels: Vec<LabelCst>,
    pub instr: Option<InstrCst>,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrCst {
    pub word: String, // mnemonic / `.byte` / junk word
    pub word_span: Span,
    pub operands: Vec<OperandToken>,
}

/// One comma-separated operand: the raw source slice between
/// delimiters, trimmed; span covers the trimmed slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperandToken {
    pub text: String,
    pub span: Span,
}

/// A trailing comment stays inside `text` and `span` — unlike Line and
/// Func, which split it out into `trailing` — so the node remains one
/// verbatim record of the unshapeable line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCst {
    pub text: String,
    pub span: Span,
}

/// `.section NAME`. `span` covers the directive (the `.section` word
/// through the name), excluding any trailing comment — mirroring
/// [`FuncCst`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionCst {
    pub name: String,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
}

/// Which table directive a [`TableDirectiveCst`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableDirectiveKind {
    /// `.row [..]` — a vector row.
    Row,
    /// `.targets L1, L2, ..` — a list of target labels.
    Targets,
    /// `.target L` — a single target label.
    Target,
}

/// `.row`/`.targets`/`.target`, optionally labeled (`Tfetch: .row [..]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDirectiveCst {
    pub labels: Vec<LabelCst>,
    pub kind: TableDirectiveKind,
    /// `Row`: the whole bracketed vector as ONE verbatim `[..]` token
    /// (the interior commas do not split it); `Targets`/`Target`: the
    /// label-name operands, comma-split. The CST keeps the raw source
    /// slices — parsing the contents happens at lower, a later task.
    pub operands: Vec<OperandToken>,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
}

/// `.rept v, lo, hi` … `.endr`. `span` covers the `.rept` header line
/// (excluding a trailing comment); `endr_span` covers the closing
/// `.endr` word (excluding its trailing comment), and `endr_trailing`
/// retains that comment — together they make the block self-describing,
/// so a printer bounds the body by physical line (header line + 1 through
/// `.endr` line − 1) without re-scanning for the terminator. `body` holds
/// the block's lines shaped AS WRITTEN — substitution markers `{…}`
/// survive verbatim inside each item's operand text; expansion happens at
/// lower.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReptCst {
    pub var: String,
    pub lo: i64,
    pub hi: i64,
    pub body: Vec<AsmItem>,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
    pub endr_span: Span,
    pub endr_trailing: Option<TrailingComment>,
}

/// `.routine <name>, tapes=<int>, alpha=(<int>, …)`. `span` covers the
/// directive minus any trailing comment (mirror [`FuncCst`]); the field
/// spans point at the tapes value and the whole `(..)` alpha group —
/// the exact text lowering's signature diagnostics indicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineDirectiveCst {
    pub name: String,
    pub name_span: Span,
    pub tapes: u32,
    pub tapes_span: Span,
    pub alpha: Vec<u32>,
    pub alpha_span: Span,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
}

/// One frame-descriptor directive, in one of its three shapes
/// (docs/formats.md (frame descriptors)). Lossless: every field is parsed
/// from canonically-spelled tokens, so the printer reconstructs the line
/// without altering any token's text (mirrors [`RoutineDirectiveCst`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameDirectiveCst {
    /// `Fname: .frame tapes=(<int>, …)` — opens a descriptor. `arity` is
    /// the tapes-list length; virtual tape `k` projects to physical tape
    /// `tapes[k]`.
    Header(FrameHeaderCst),
    /// `.map <k>[, rmap=(<pair>, …)][, wmap=(<pair>, …)]` — per-virtual-tape
    /// symbol maps. `rmap` pairs are PHYSICAL->VIRTUAL (read direction),
    /// `wmap` pairs VIRTUAL->PHYSICAL (write direction). `->` and `=>` both
    /// add a pair; `=>` additionally marks it one-way (rmap only) — a
    /// distinction the composition engine uses, not the wire descriptor.
    Map(FrameMapCst),
    /// `.exits <label>, …` — the multi-exit return targets, code labels in
    /// the owning function.
    Exits(FrameExitsCst),
}

/// `Fname: .frame tapes=(<int>, …)`. `span` covers the directive minus any
/// trailing comment; `tapes_span` covers the whole `(..)` group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeaderCst {
    pub label: LabelCst,
    pub tapes: Vec<u32>,
    pub tapes_span: Span,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
}

/// One `<from> -> <to>` (or `=>`) map pair; `one_way` distinguishes the
/// `=>` spelling. Values are canonically-spelled u32s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramePairCst {
    pub from: u32,
    pub to: u32,
    pub one_way: bool,
}

/// `.map <k>[, rmap=(…)][, wmap=(…)]`. Each map group is `Some` iff its
/// `rmap=`/`wmap=` clause is present; the group span covers its `(..)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameMapCst {
    pub k: u32,
    pub k_span: Span,
    pub rmap: Option<Vec<FramePairCst>>,
    pub rmap_span: Option<Span>,
    pub wmap: Option<Vec<FramePairCst>>,
    pub wmap_span: Option<Span>,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
}

/// `.exits <label>, …`. Operands keep their raw label-name slices (comma
/// split), like a `.targets` directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameExitsCst {
    pub targets: Vec<OperandToken>,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
}

impl FrameDirectiveCst {
    /// The directive line's span (excluding any trailing comment).
    pub fn span(&self) -> Span {
        match self {
            FrameDirectiveCst::Header(h) => h.span,
            FrameDirectiveCst::Map(m) => m.span,
            FrameDirectiveCst::Exits(e) => e.span,
        }
    }
}

/// Total: never fails. Uses the classic assembly grammar
/// (`AsmCaps::default()`); dialects with opt-in surface call
/// [`parse_asm_cst_with`].
pub fn parse_asm_cst(source: &str) -> AsmCst {
    parse_asm_cst_with(source, AsmCaps::default())
}

/// Total: never fails. `caps` selects the per-dialect opt-in lexer
/// surface (vector operands, `{…}` substitution) and the matching
/// shaping (sections, table directives, `.rept` blocks). With
/// `AsmCaps::default()` this is byte-identical to [`parse_asm_cst`]:
/// the block-opening below is gated on `caps.rept`, `shape_line`'s
/// section/table branches on `caps.tables`, and the list-continuation
/// fold on `caps.tables` too, so every added path is dead under default
/// caps and the item sequence is unchanged — one item per non-blank
/// physical line.
pub fn parse_asm_cst_with(source: &str, caps: AsmCaps) -> AsmCst {
    let records = line_records(source, caps);
    let mut items: Vec<AsmItem> = Vec::new();
    let mut i = 0;
    while i < records.len() {
        let rec = &records[i];
        // A well-formed `.rept v, lo, hi` opens a block (`caps.rept`).
        // The first bare `.endr` after it closes the block. There is no
        // nesting: a `.rept` inside the body shapes through `shape_line`
        // (which never opens a block), so it degrades to a Line and does
        // not consume an `.endr`.
        if caps.rept
            && let Some(header) = rept_header(rec)
        {
            let rest = &records[i + 1..];
            if let Some(off) = rest.iter().position(|r| is_endr(&r.tokens)) {
                let body = shape_body(&rest[..off], caps);
                let (endr_span, endr_trailing) = endr_parts(&rest[off]);
                items.push(AsmItem {
                    blank_before: rec.blank_before,
                    kind: AsmItemKind::Rept(ReptCst {
                        var: header.var,
                        lo: header.lo,
                        hi: header.hi,
                        body,
                        span: header.span,
                        trailing: header.trailing,
                        endr_span,
                        endr_trailing,
                    }),
                    // A block is many physical lines but not a continued
                    // list — it carries its own line bounds in `span` and
                    // `endr_span`, and its body items each stay one line.
                    continuation: None,
                });
                i += 1 + off + 1; // header + body + the closing `.endr`
                continue;
            }
            // Unterminated `.rept`: no matching `.endr`. Degrade the
            // header to a plain Line (mirror malformed-`.func`); the body
            // lines then shape as ordinary top-level items on the
            // following iterations, via the fall-through below.
        }
        let candidate = continued_len(&records[i..], caps);
        let (kind, used) = shape_records(&records[i..i + candidate], caps);
        items.push(AsmItem {
            blank_before: rec.blank_before,
            kind,
            continuation: (used > 1).then(|| {
                records[i..i + used]
                    .iter()
                    .map(|r| ContinuedLine {
                        line_no: r.line_no,
                        text: r.line.to_string(),
                    })
                    .collect()
            }),
        });
        i += used;
    }
    AsmCst { items }
}

/// Shapes one item out of `records` — more than one record only when
/// [`continued_len`] saw a list continued by a trailing comma. The join
/// is kept ONLY when it produces the continued directive it promised;
/// if the joined text degrades to anything else (a malformed list, a
/// plain Line, a Raw), the item re-shapes from the first record alone and
/// the rest stay separate items. So a trailing comma that does not end up
/// continuing a well-formed list keeps exactly the error it has today.
/// Returns the shaped kind and how many records it consumed.
fn shape_records(records: &[LineRecord<'_>], caps: AsmCaps) -> (AsmItemKind, usize) {
    if records.len() > 1 {
        let lines: Vec<&str> = records.iter().map(|r| r.line).collect();
        // Each segment's tokens already carry their own physical line, so
        // concatenating the per-line token vectors yields a joined stream
        // whose spans stay physical with no renumbering. Only the final
        // segment can hold a Comment (a comma followed by a comment does
        // not continue), so the joined stream still satisfies the
        // one-trailing-comment shape `split_trailing` relies on.
        let tokens: Vec<AsmToken> = records
            .iter()
            .flat_map(|r| r.tokens.iter().cloned())
            .collect();
        let src = ItemText::new(records[0].line_no, &lines);
        let kind = shape_line(&src, &tokens, caps);
        if is_continued_list(&kind) {
            return (kind, records.len());
        }
    }
    let rec = &records[0];
    let lines = [rec.line];
    let src = ItemText::new(rec.line_no, &lines);
    (shape_line(&src, &rec.tokens, caps), 1)
}

/// Did a joined region shape into one of the three directives whose list
/// may be continued? Anything else means the join was not what the
/// trailing comma promised, and the caller falls back to one line per
/// item.
fn is_continued_list(kind: &AsmItemKind) -> bool {
    match kind {
        AsmItemKind::TableDirective(d) => matches!(d.kind, TableDirectiveKind::Targets),
        AsmItemKind::FrameDirective(d) => {
            matches!(d, FrameDirectiveCst::Map(_) | FrameDirectiveCst::Exits(_))
        }
        _ => false,
    }
}

/// How many records the item starting at `records[0]` may occupy: 1 for
/// every ordinary line, more when a `.targets`/`.exits`/`.map` line ends
/// in a comma and the lines below continue its list (docs/formats.md
/// (assembly text)).
///
/// The comma must be the line's LAST token. A comma followed by a comment
/// does not continue — that keeps the joined token stream's single
/// trailing comment at the end where every consumer expects it, and keeps
/// a comment from being silently relocated into the middle of a list.
///
/// Three things end the run: input running out, a blank line (recorded as
/// the next record's `blank_before` — folding across it would swallow the
/// blank), and a line that opens a statement of its own. That last guard
/// is what stops a stray dangling comma from eating the `.section`,
/// `.func` or labeled line beneath it.
fn continued_len(records: &[LineRecord<'_>], caps: AsmCaps) -> usize {
    if !caps.tables
        || !statement_word(&records[0].tokens).is_some_and(|w| CONTINUABLE_WORDS.contains(&w))
    {
        return 1;
    }
    let mut n = 1;
    while n < records.len()
        && matches!(
            records[n - 1].tokens.last().map(|t| &t.kind),
            Some(AsmTokenKind::Comma)
        )
        && !records[n].blank_before
        && !opens_a_statement(&records[n].tokens, caps)
    {
        n += 1;
    }
    n
}

/// The directive or mnemonic word a line's statement starts with, past
/// any leading `Word Colon` labels; `None` when the line is not shaped
/// `label* word …`.
fn statement_word(tokens: &[AsmToken]) -> Option<&str> {
    let mut at = 0;
    while at + 1 < tokens.len()
        && matches!(tokens[at].kind, AsmTokenKind::Word(_))
        && matches!(tokens[at + 1].kind, AsmTokenKind::Colon)
    {
        at += 2;
    }
    word_text(tokens.get(at)?)
}

/// Does this line open a statement of its own? A leading label or a
/// recognized directive word always does, and such a line can therefore
/// never be read as the continuation of the list above it. The check is
/// deliberately structural — the CST is arch-agnostic and has no mnemonic
/// table, so a bare instruction word is not something it can recognize.
fn opens_a_statement(tokens: &[AsmToken], caps: AsmCaps) -> bool {
    let labeled = tokens.len() > 1
        && matches!(tokens[0].kind, AsmTokenKind::Word(_))
        && matches!(tokens[1].kind, AsmTokenKind::Colon);
    labeled || word_text(&tokens[0]).is_some_and(|w| recognized_directives(caps).contains(&w))
}

/// The physical source text one item was read from: one line normally,
/// several when a trailing comma continued its list. Every lookup names
/// the PHYSICAL line a token sits on, so operand text is sliced out of
/// the line it was really written on and the spans built alongside stay
/// physical.
struct ItemText<'a> {
    first_line_no: u32,
    lines: &'a [&'a str],
}

impl<'a> ItemText<'a> {
    fn new(first_line_no: u32, lines: &'a [&'a str]) -> ItemText<'a> {
        ItemText {
            first_line_no,
            lines,
        }
    }

    /// The physical line `line_no`, or `""` if it is outside the item.
    fn line(&self, line_no: u32) -> &'a str {
        line_no
            .checked_sub(self.first_line_no)
            .and_then(|i| self.lines.get(i as usize))
            .copied()
            .unwrap_or_default()
    }

    /// The char range `[start_col, end_col)` of one physical line.
    /// Columns are char-counted (crate::diagnostics), so slice by chars.
    fn slice(&self, line_no: u32, start_col: u32, end_col: u32) -> String {
        self.line(line_no)
            .chars()
            .skip(start_col as usize - 1)
            .take((end_col - start_col) as usize)
            .collect()
    }

    /// The whole region as written, physical line breaks included.
    fn joined(&self) -> String {
        self.lines.join("\n")
    }
}

/// One non-blank physical line, retained with its tokens for shaping.
struct LineRecord<'a> {
    line: &'a str,
    tokens: Vec<AsmToken>,
    line_no: u32,
    blank_before: bool,
}

/// Splits `source` into one record per non-blank physical line, folding
/// runs of blank lines into the next record's `blank_before` (leading
/// file blanks set nothing — there is no record to precede). This is the
/// same fold the pre-block loop did inline; pulling it out lets the
/// shaper look ahead across lines for a `.rept` block's `.endr`.
fn line_records(source: &str, caps: AsmCaps) -> Vec<LineRecord<'_>> {
    let mut records: Vec<LineRecord<'_>> = Vec::new();
    let mut pending_blank = false;
    for (idx, line) in source.lines().enumerate() {
        let line_no = idx as u32 + 1;
        let tokens = lex_line(line, line_no, caps);
        if tokens.is_empty() {
            pending_blank = !records.is_empty();
            continue;
        }
        records.push(LineRecord {
            line,
            tokens,
            line_no,
            blank_before: pending_blank,
        });
        pending_blank = false;
    }
    records
}

/// Shapes a run of records into body items (used for a `.rept` body).
/// Each record goes through `shape_line`, which never opens a block, so
/// a nested `.rept` degrades to a Line here and the block's own `.endr`
/// — already consumed by the caller as the terminator — never reaches
/// this slice.
///
/// A body item is ALWAYS exactly one physical line: list continuation is
/// a top-level fold only. Expansion recovers each body line verbatim by
/// its line number before substituting into it, so a body item spanning
/// two lines would expand to half of itself; inside a block a trailing
/// comma therefore keeps the error it has today.
fn shape_body(records: &[LineRecord<'_>], caps: AsmCaps) -> Vec<AsmItem> {
    records
        .iter()
        .map(|rec| {
            let lines = [rec.line];
            let src = ItemText::new(rec.line_no, &lines);
            AsmItem {
                blank_before: rec.blank_before,
                kind: shape_line(&src, &rec.tokens, caps),
                continuation: None,
            }
        })
        .collect()
}

/// A parsed `.rept v, lo, hi` header (structurally exact only).
struct ReptHeader {
    var: String,
    lo: i64,
    hi: i64,
    span: Span,
    trailing: Option<TrailingComment>,
}

/// Recognizes a well-formed `.rept v, lo, hi` header line: leading word
/// `.rept`, no labels, exactly three comma operands — a non-empty `var`
/// name and two `i64` bounds. Anything else is `None` (the caller
/// degrades it to a Line, mirroring malformed-`.func`). Name validity is
/// left to lower, a later task; here we only need enough structure to
/// know a block opens.
fn rept_header(rec: &LineRecord<'_>) -> Option<ReptHeader> {
    let (body, trailing) = split_trailing(&rec.tokens);
    let [first, rest @ ..] = body else {
        return None;
    };
    if word_text(first) != Some(REPT_WORD) {
        return None;
    }
    let lines = [rec.line];
    let src = ItemText::new(rec.line_no, &lines);
    let operands = operand_region(
        &src,
        rest,
        Pos {
            line: first.line,
            col: first.col + first.len,
        },
    );
    let [var, lo, hi] = operands.as_slice() else {
        return None;
    };
    if var.text.is_empty() {
        return None;
    }
    let lo = lo.text.parse::<i64>().ok()?;
    let hi = hi.text.parse::<i64>().ok()?;
    let last = body.last().expect("body starts with the `.rept` word");
    Some(ReptHeader {
        var: var.text.clone(),
        lo,
        hi,
        span: Span::new(rec.line_no, first.col, rec.line_no, last.col + last.len),
        trailing,
    })
}

/// A bare `.endr` directive: leading word `.endr`, nothing after it but
/// an optional trailing comment. A malformed `.endr` (junk operands) is
/// not a closer — it degrades to a Line and the block reads as
/// unterminated, again mirroring `.func`.
fn is_endr(tokens: &[AsmToken]) -> bool {
    let (body, _) = split_trailing(tokens);
    matches!(body, [only] if word_text(only) == Some(ENDR_WORD))
}

/// The `.endr` record's word span (excluding its trailing comment) and
/// that trailing comment, if any. Precondition: `rec` passed [`is_endr`],
/// so its non-comment body is exactly the `.endr` word.
fn endr_parts(rec: &LineRecord<'_>) -> (Span, Option<TrailingComment>) {
    let (body, trailing) = split_trailing(&rec.tokens);
    let word = &body[0];
    let span = Span::new(rec.line_no, word.col, rec.line_no, word.col + word.len);
    (span, trailing)
}

/// Splits a trailing comment token off the end of a line's tokens; the
/// returned body keeps every non-comment token. Shared by `shape_line`,
/// the `.rept` header parse, and `.endr` detection.
fn split_trailing(tokens: &[AsmToken]) -> (&[AsmToken], Option<TrailingComment>) {
    match tokens {
        [body @ .., last] if matches!(last.kind, AsmTokenKind::Comment(_)) => {
            let AsmTokenKind::Comment(text) = &last.kind else {
                unreachable!("guard matched Comment");
            };
            (
                body,
                Some(TrailingComment {
                    text: text.clone(),
                    col: last.col,
                }),
            )
        }
        _ => (tokens, None),
    }
}

/// Shapes one non-blank line (docs/formats.md (assembly text) grammar:
/// `label* [word operands] [; comment]`). Anything that does not fit
/// falls back to Raw — never an error. `caps` gates the opt-in
/// directive shaping (sections, table directives); block-spanning
/// `.rept` is handled by the caller, so this only ever sees the `.rept`
/// header (and any `.endr`) as an ordinary line, which is exactly the
/// degradation nested/unterminated blocks rely on.
///
/// `src` is normally one physical line; it holds several only for a list
/// continued by a trailing comma, and then every span below is still
/// built from the token's OWN physical line, never from a renumbered
/// join.
fn shape_line(src: &ItemText<'_>, tokens: &[AsmToken], caps: AsmCaps) -> AsmItemKind {
    // Own-line comment. The lexer emits at most one Comment token,
    // always last, so a lone Comment is the whole line.
    if let [only] = tokens
        && let AsmTokenKind::Comment(text) = &only.kind
    {
        return AsmItemKind::Comment(AsmComment {
            text: text.clone(),
            col: only.col,
        });
    }

    // Not assembly-shaped: any Junk, or a first token that is not a
    // Word (listing lines such as `  0004:  21 …` or `<goToEnd>`).
    let has_junk = tokens
        .iter()
        .any(|t| matches!(t.kind, AsmTokenKind::Junk(_)));
    if has_junk || !matches!(tokens[0].kind, AsmTokenKind::Word(_)) {
        return raw_line(src, tokens);
    }

    // Split off the trailing comment; `body` keeps at least tokens[0].
    let (body, trailing) = split_trailing(tokens);
    let last = body.last().expect("first token is a Word, never Comment");
    // The item's span: the line's trimmed extent minus the comment. For a
    // continued list the extent ends on the last physical line the code
    // reaches, so start and end name different lines.
    let span = Span::new(body[0].line, body[0].col, last.line, last.col + last.len);

    // `.func` special case: structurally exact directives only.
    // Anything else starting `.func` stays a Line so lower.rs can
    // replicate the legacy errors verbatim.
    if word_text(&body[0]) == Some(FUNC_WORD) {
        let exact = match body {
            [_, name] => word_text(name).map(|n| (n, name, false)),
            [_, name, kw] if word_text(kw) == Some("local") => {
                word_text(name).map(|n| (n, name, true))
            }
            _ => None,
        };
        if let Some((name, name_token, local)) = exact {
            return AsmItemKind::Func(FuncCst {
                name: name.to_string(),
                name_span: name_token.span(),
                local,
                span,
                trailing,
            });
        }
    }

    // `.section NAME` (caps.tables): a no-label region marker, mirroring
    // `.func` — structurally exact (`.section` + one Word name) only,
    // else it stays a Line.
    if caps.tables
        && word_text(&body[0]) == Some(SECTION_WORD)
        && let [_, name] = body
        && let Some(name) = word_text(name)
    {
        return AsmItemKind::Section(SectionCst {
            name: name.to_string(),
            span,
            trailing,
        });
    }

    // `.routine <name>, tapes=<int>, alpha=(<int>, …)` (caps.tables):
    // a no-label signature declaration, structurally exact only —
    // anything else starting `.routine` stays a Line for lower to
    // report precisely. In practice the directive needs the rept cap
    // too: `=` lexes under caps.tables but `(`/`)` lex under caps.rept,
    // so with rept off the parens stay Junk and the line shapes Raw
    // before this branch is ever reached.
    if caps.tables
        && word_text(&body[0]) == Some(ROUTINE_WORD)
        && let Some(directive) = routine_directive(body, span, &trailing)
    {
        return AsmItemKind::RoutineDirective(directive);
    }

    // Labels: leading repeated `Word Colon` pairs, regardless of the
    // word's grammar (`foo.bar:` / `std::x:` are label candidates —
    // lower.rs rejects bad names with a precise span).
    let mut labels = Vec::new();
    let mut at = 0;
    while at + 1 < body.len()
        && matches!(body[at].kind, AsmTokenKind::Word(_))
        && matches!(body[at + 1].kind, AsmTokenKind::Colon)
    {
        let AsmTokenKind::Word(name) = &body[at].kind else {
            unreachable!("loop condition matched Word");
        };
        labels.push(LabelCst {
            name: name.clone(),
            span: body[at].span(),
        });
        at += 2;
    }

    if at == body.len() {
        return AsmItemKind::Line(LineCst {
            labels,
            instr: None,
            span,
            trailing,
        });
    }
    let word_token = &body[at];
    let Some(word) = word_text(word_token) else {
        // `label* <non-word>` — the instruction-word slot holds a
        // token no rule accepts; the line is not assembly-shaped.
        return raw_line(src, tokens);
    };
    let after_word = Pos {
        line: word_token.line,
        col: word_token.col + word_token.len,
    };

    // Table directives (caps.tables): `.row`/`.targets`/`.target`,
    // optionally labeled. `.row` captures its `[..]` vector as ONE
    // verbatim operand (interior commas do not split); the others
    // comma-split their label-name operands. A `.row` whose region is
    // not a single bracketed vector degrades to a Line (mirror `.func`).
    if caps.tables
        && let Some(dir_kind) = table_directive_kind(word)
    {
        let region = &body[at + 1..];
        let operands = match dir_kind {
            TableDirectiveKind::Row => vector_operand(src, region).map(|op| vec![op]),
            TableDirectiveKind::Targets | TableDirectiveKind::Target => {
                Some(operand_region(src, region, after_word))
            }
        };
        if let Some(operands) = operands {
            return AsmItemKind::TableDirective(TableDirectiveCst {
                labels,
                kind: dir_kind,
                operands,
                span,
                trailing,
            });
        }
        // `.row` without a bracketed vector — fall through to Line.
    }

    // Frame-descriptor directives (caps.tables): `.frame`/`.map`/`.exits`.
    // `.frame` carries the descriptor label (exactly one); `.map`/`.exits`
    // are unlabeled continuations of the open group. Structurally exact
    // only — anything else degrades to a Line for lower to report
    // precisely (mirror `.func`/`.routine`).
    if caps.tables
        && FRAME_DIRECTIVE_WORDS.contains(&word)
        && let Some(directive) = frame_directive(
            word,
            &labels,
            src,
            &body[at + 1..],
            span,
            &trailing,
            after_word,
        )
    {
        return AsmItemKind::FrameDirective(directive);
    }

    // A bracketed operand region on an ordinary instruction line
    // (caps.vectors) is captured as ONE verbatim `[..]` token, exactly
    // like a `.row`'s vector — the interior commas must not split it.
    // Two shapes carry a bracket: a lone vector (`wr [1, 2]`, region is
    // the bracket) and a call-target-then-binding (`call f [2, 0]`, a
    // name then the bracket). Both capture everything from the first `[`
    // to the last `]` as one verbatim operand; any operands before the
    // `[` comma-split as usual (the name half of a binding call). Under
    // default caps `LBracket` tokens never exist, so this is dead and the
    // comma-split below is byte-identical to before.
    let region = &body[at + 1..];
    let operands = match caps
        .vectors
        .then(|| {
            region
                .iter()
                .position(|t| matches!(t.kind, AsmTokenKind::LBracket))
        })
        .flatten()
    {
        Some(open) => match vector_operand(src, &region[open..]) {
            Some(bracket) => {
                let mut ops = operand_region(src, &region[..open], after_word);
                ops.push(bracket);
                ops
            }
            // A malformed bracket region (no closing `]`) degrades to the
            // plain comma-split; lower reports the mismatch precisely.
            None => operand_region(src, region, after_word),
        },
        None => operand_region(src, region, after_word),
    };
    AsmItemKind::Line(LineCst {
        labels,
        instr: Some(InstrCst {
            word: word.to_string(),
            word_span: word_token.span(),
            operands,
        }),
        span,
        trailing,
    })
}

/// Recognizes the exact `.routine` token shape after the leading word:
///
/// `Word(name) , Word(tapes) = Number , Word(alpha) = ( Number [, Number]* )`
///
/// Numbers must be canonically spelled u32s (no sign, no leading
/// zeros): the printer reconstructs the directive from the PARSED
/// values, and reprinting must not alter any token's text. `None` =
/// not structurally exact; the caller degrades the line to a plain
/// Line (mirror `.func`).
fn routine_directive(
    body: &[AsmToken],
    span: Span,
    trailing: &Option<TrailingComment>,
) -> Option<RoutineDirectiveCst> {
    let is = |t: &AsmToken, k: &AsmTokenKind| &t.kind == k;
    let [
        name_tok,
        c1,
        tapes_kw,
        eq1,
        tapes_tok,
        c2,
        alpha_kw,
        eq2,
        lparen,
        rest @ ..,
    ] = &body[1..]
    else {
        return None;
    };
    let name = word_text(name_tok)?;
    if !is(c1, &AsmTokenKind::Comma)
        || word_text(tapes_kw) != Some("tapes")
        || !is(eq1, &AsmTokenKind::Eq)
        || !is(c2, &AsmTokenKind::Comma)
        || word_text(alpha_kw) != Some("alpha")
        || !is(eq2, &AsmTokenKind::Eq)
        || !is(lparen, &AsmTokenKind::LParen)
    {
        return None;
    }
    let (tapes, tapes_span) = canonical_u32(tapes_tok)?;
    let [inner @ .., rparen] = rest else {
        return None;
    };
    if !is(rparen, &AsmTokenKind::RParen) || inner.len().is_multiple_of(2) {
        return None; // `()`, a trailing comma, or a doubled one
    }
    let mut alpha = Vec::with_capacity(inner.len() / 2 + 1);
    for (i, tok) in inner.iter().enumerate() {
        if i % 2 == 0 {
            alpha.push(canonical_u32(tok)?.0);
        } else if !is(tok, &AsmTokenKind::Comma) {
            return None;
        }
    }
    Some(RoutineDirectiveCst {
        name: name.to_string(),
        name_span: name_tok.span(),
        tapes,
        tapes_span,
        alpha,
        alpha_span: Span::new(
            lparen.line,
            lparen.col,
            rparen.line,
            rparen.col + rparen.len,
        ),
        span,
        trailing: trailing.clone(),
    })
}

/// A `Number` token's value as a canonically spelled u32 — the spelling
/// must equal the value's own rendering (rejects `-1`, `007`, overflow).
pub(super) fn canonical_u32(token: &AsmToken) -> Option<(u32, Span)> {
    let AsmTokenKind::Number(text) = &token.kind else {
        return None;
    };
    let value = text.parse::<u32>().ok()?;
    (*text == value.to_string()).then(|| (value, token.span()))
}

/// Shapes one frame-descriptor directive (`.frame`/`.map`/`.exits`) from
/// the tokens after the directive word. Structurally exact only — `None`
/// degrades the line to a plain Line (mirror `.func`/`.routine`), which
/// lower reports precisely. `labels` are the leading labels already
/// parsed: `.frame` requires exactly one (the descriptor name);
/// `.map`/`.exits` require none.
fn frame_directive(
    word: &str,
    labels: &[LabelCst],
    src: &ItemText<'_>,
    region: &[AsmToken],
    span: Span,
    trailing: &Option<TrailingComment>,
    after_word: Pos,
) -> Option<FrameDirectiveCst> {
    match word {
        FRAME_WORD => {
            let [label] = labels else {
                return None;
            };
            let (tapes, tapes_span) = parse_frame_tapes(region)?;
            Some(FrameDirectiveCst::Header(FrameHeaderCst {
                label: label.clone(),
                tapes,
                tapes_span,
                span,
                trailing: trailing.clone(),
            }))
        }
        MAP_WORD => {
            if !labels.is_empty() {
                return None;
            }
            parse_frame_map(region, span, trailing).map(FrameDirectiveCst::Map)
        }
        EXITS_WORD => {
            if !labels.is_empty() {
                return None;
            }
            let targets = operand_region(src, region, after_word);
            if targets.is_empty() || targets.iter().any(|t| t.text.is_empty()) {
                return None;
            }
            Some(FrameDirectiveCst::Exits(FrameExitsCst {
                targets,
                span,
                trailing: trailing.clone(),
            }))
        }
        _ => None,
    }
}

/// `tapes=(<int>, …)` after the `.frame` word: `Word("tapes") = ( Number
/// [, Number]* )`, canonically spelled. Returns the phys list and the
/// `(..)` group span.
fn parse_frame_tapes(region: &[AsmToken]) -> Option<(Vec<u32>, Span)> {
    let is = |t: &AsmToken, k: &AsmTokenKind| &t.kind == k;
    let [tapes_kw, eq, lparen, rest @ ..] = region else {
        return None;
    };
    if word_text(tapes_kw) != Some("tapes")
        || !is(eq, &AsmTokenKind::Eq)
        || !is(lparen, &AsmTokenKind::LParen)
    {
        return None;
    }
    let [inner @ .., rparen] = rest else {
        return None;
    };
    if !is(rparen, &AsmTokenKind::RParen) || inner.is_empty() || inner.len().is_multiple_of(2) {
        return None; // `()`, a trailing comma, or a doubled one
    }
    let mut tapes = Vec::with_capacity(inner.len() / 2 + 1);
    for (i, tok) in inner.iter().enumerate() {
        if i % 2 == 0 {
            tapes.push(canonical_u32(tok)?.0);
        } else if !is(tok, &AsmTokenKind::Comma) {
            return None;
        }
    }
    let span = Span::new(
        lparen.line,
        lparen.col,
        rparen.line,
        rparen.col + rparen.len,
    );
    Some((tapes, span))
}

/// `.map <k>[, rmap=(<pair>, …)][, wmap=(<pair>, …)]` after the `.map`
/// word. The two clauses are optional and canonical order — `rmap` before
/// `wmap`; anything else is `None`.
fn parse_frame_map(
    region: &[AsmToken],
    span: Span,
    trailing: &Option<TrailingComment>,
) -> Option<FrameMapCst> {
    let [k_tok, rest @ ..] = region else {
        return None;
    };
    let (k, k_span) = canonical_u32(k_tok)?;
    let mut rest = rest;
    let (rmap, rmap_span) = match parse_named_pairs(rest, "rmap") {
        Some((pairs, group_span, after)) => {
            rest = after;
            (Some(pairs), Some(group_span))
        }
        None => (None, None),
    };
    let (wmap, wmap_span) = match parse_named_pairs(rest, "wmap") {
        Some((pairs, group_span, after)) => {
            rest = after;
            (Some(pairs), Some(group_span))
        }
        None => (None, None),
    };
    if !rest.is_empty() {
        return None;
    }
    Some(FrameMapCst {
        k,
        k_span,
        rmap,
        rmap_span,
        wmap,
        wmap_span,
        span,
        trailing: trailing.clone(),
    })
}

/// `, <name>=( <pair>, … )` — a named map clause. Consumes the leading
/// comma; returns the parsed pairs, the `(..)` group span, and the tokens
/// after the closing paren. `None` = the clause is not present/exact.
fn parse_named_pairs<'a>(
    rest: &'a [AsmToken],
    name: &str,
) -> Option<(Vec<FramePairCst>, Span, &'a [AsmToken])> {
    let is = |t: &AsmToken, k: &AsmTokenKind| &t.kind == k;
    let [comma, kw, eq, lparen, tail @ ..] = rest else {
        return None;
    };
    if !is(comma, &AsmTokenKind::Comma)
        || word_text(kw) != Some(name)
        || !is(eq, &AsmTokenKind::Eq)
        || !is(lparen, &AsmTokenKind::LParen)
    {
        return None;
    }
    // No nesting inside a pair list, so the first `)` closes the group.
    let close = tail
        .iter()
        .position(|t| matches!(t.kind, AsmTokenKind::RParen))?;
    let pairs = parse_pairs(&tail[..close])?;
    let rparen = &tail[close];
    let group_span = Span::new(
        lparen.line,
        lparen.col,
        rparen.line,
        rparen.col + rparen.len,
    );
    Some((pairs, group_span, &tail[close + 1..]))
}

/// A comma-separated list of `<from> (-> | =>) <to>` pairs (canonically
/// spelled values). An empty token slice is the empty list (`rmap=()` =
/// identity). `None` on any structural violation.
pub(super) fn parse_pairs(inner: &[AsmToken]) -> Option<Vec<FramePairCst>> {
    let mut pairs = Vec::new();
    let mut i = 0;
    while i < inner.len() {
        let from = canonical_u32(inner.get(i)?)?.0;
        let one_way = match inner.get(i + 1)?.kind {
            AsmTokenKind::Arrow => false,
            AsmTokenKind::FatArrow => true,
            _ => return None,
        };
        let to = canonical_u32(inner.get(i + 2)?)?.0;
        pairs.push(FramePairCst { from, to, one_way });
        i += 3;
        if i < inner.len() {
            if !matches!(inner[i].kind, AsmTokenKind::Comma) {
                return None;
            }
            i += 1;
            // A trailing comma with no pair after it is malformed.
            if i == inner.len() {
                return None;
            }
        }
    }
    Some(pairs)
}

/// Shapes a declarative binding-call operand's interior (the text
/// between the operand's `[` and `]`) into per-entry `(physIdx, pairs)`
/// tuples — list position is the callee virtual tape (docs/formats.md
/// (bound calls)). An entry is a canonical `<physIdx>`, optionally
/// followed by a `{ <pair>, … }` symbol map reusing the `.map` pair
/// grammar (`->` bidirectional, `=>` one-way). Total shaping: `None` on
/// any structural violation (bad number, unbalanced braces, empty entry,
/// trailing junk); an empty/whitespace interior shapes as `Some(vec![])`
/// so the caller distinguishes `[]` (rejected there) from malformed.
///
/// The interior is re-lexed at bracket depth 0 (no `vectors` cap) so the
/// `->`/`=>` arrows lex as arrows rather than the in-bracket move markers
/// — the binding lives in `[..]` at source level, but its pairs read like
/// a `.map` clause's `(..)` pairs.
pub(super) fn parse_binding(inner: &str, line_no: u32) -> Option<Vec<(u32, Vec<FramePairCst>)>> {
    let inner = inner.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    let caps = AsmCaps {
        tables: true,
        rept: true,
        vectors: false,
    };
    let tokens: Vec<AsmToken> = lex_line(inner, line_no, caps)
        .into_iter()
        .filter(|t| !matches!(t.kind, AsmTokenKind::Comment(_)))
        .collect();
    // Split into entries at brace-depth-0 commas; commas inside a `{..}`
    // map belong to that entry's pair list.
    let mut segments: Vec<&[AsmToken]> = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        match t.kind {
            AsmTokenKind::LBrace => depth += 1,
            AsmTokenKind::RBrace => depth -= 1,
            AsmTokenKind::Comma if depth == 0 => {
                segments.push(&tokens[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    segments.push(&tokens[start..]);
    let mut entries = Vec::with_capacity(segments.len());
    for seg in segments {
        entries.push(parse_binding_entry(seg)?);
    }
    Some(entries)
}

/// One binding entry: a canonical physical-tape index, then an optional
/// `{ <pairs> }` group. `None` on any structural violation.
fn parse_binding_entry(seg: &[AsmToken]) -> Option<(u32, Vec<FramePairCst>)> {
    let (first, rest) = seg.split_first()?;
    let phys = canonical_u32(first)?.0;
    if rest.is_empty() {
        return Some((phys, Vec::new()));
    }
    let [lbrace, mid @ .., rbrace] = rest else {
        return None;
    };
    if !matches!(lbrace.kind, AsmTokenKind::LBrace) || !matches!(rbrace.kind, AsmTokenKind::RBrace)
    {
        return None;
    }
    Some((phys, parse_pairs(mid)?))
}

/// The [`TableDirectiveKind`] a leading directive word names, or `None`
/// for any other word.
fn table_directive_kind(word: &str) -> Option<TableDirectiveKind> {
    match word {
        ROW_WORD => Some(TableDirectiveKind::Row),
        TARGETS_WORD => Some(TableDirectiveKind::Targets),
        TARGET_WORD => Some(TableDirectiveKind::Target),
        _ => None,
    }
}

/// Captures a `.row`'s `[..]` region as one lossless [`OperandToken`]:
/// the verbatim source slice from the opening `[` to the closing `]`.
/// Requires `region` to begin with `LBracket` and end with `RBracket`
/// (the bracket tokens exist only under `caps.vectors`); otherwise
/// `None` and the caller degrades the line to a plain Line. The
/// interior — including commas — is not interpreted here; lower parses
/// it in a later task.
fn vector_operand(src: &ItemText<'_>, region: &[AsmToken]) -> Option<OperandToken> {
    let [first, .., last] = region else {
        return None;
    };
    if !matches!(first.kind, AsmTokenKind::LBracket) || !matches!(last.kind, AsmTokenKind::RBracket)
    {
        return None;
    }
    let start = first.col;
    let end = last.col + last.len;
    let text = src.slice(first.line, start, end);
    Some(OperandToken {
        text: text.trim().to_string(),
        span: Span::new(first.line, start, last.line, end),
    })
}

/// The lossless fallback: verbatim line text; span = the line's
/// trimmed extent (all tokens, including a trailing comment). A Raw is
/// never a continued list — [`shape_records`] keeps a join only when it
/// shapes into one of the three list directives — so the verbatim text
/// here is always a single physical line.
fn raw_line(src: &ItemText<'_>, tokens: &[AsmToken]) -> AsmItemKind {
    let first = &tokens[0];
    let last = tokens.last().expect("caller guarantees tokens");
    AsmItemKind::Raw(RawCst {
        text: src.joined(),
        span: Span::new(first.line, first.col, last.line, last.col + last.len),
    })
}

fn word_text(token: &AsmToken) -> Option<&str> {
    match &token.kind {
        AsmTokenKind::Word(text) => Some(text),
        _ => None,
    }
}

/// Splits the operand region at commas. Each group's text is the raw
/// source slice from its first to its last token (interior spacing
/// preserved — `std :: api` survives verbatim for lower.rs to reject
/// exactly as before); an empty group (doubled / leading / trailing
/// comma) yields an empty-text token with a zero-width span just past
/// the preceding delimiter, where the operand would have been.
fn operand_region(src: &ItemText<'_>, region: &[AsmToken], after_word: Pos) -> Vec<OperandToken> {
    if region.is_empty() {
        return Vec::new();
    }
    let mut operands = Vec::new();
    let mut group: Vec<&AsmToken> = Vec::new();
    let mut empty_group_at = after_word;
    for token in region {
        if matches!(token.kind, AsmTokenKind::Comma) {
            operands.push(operand_token(src, &group, empty_group_at));
            group.clear();
            empty_group_at = Pos {
                line: token.line,
                col: token.col + token.len,
            };
        } else {
            group.push(token);
        }
    }
    operands.push(operand_token(src, &group, empty_group_at));
    operands
}

/// One operand group's text and span. A group never straddles a physical
/// line break: the only place a list continues is straight after a comma,
/// and a comma is itself a group boundary — so the group's first and last
/// tokens always sit on the same line, and slicing that one line is
/// exact.
fn operand_token(src: &ItemText<'_>, group: &[&AsmToken], empty_group_at: Pos) -> OperandToken {
    let (Some(first), Some(last)) = (group.first(), group.last()) else {
        return OperandToken {
            text: String::new(),
            span: Span {
                start: empty_group_at,
                end: empty_group_at,
            },
        };
    };
    let start = first.col;
    let end = last.col + last.len;
    let text = src.slice(first.line, start, end);
    OperandToken {
        text: text.trim().to_string(),
        span: Span::new(first.line, start, last.line, end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn as_comment(item: &AsmItem) -> &AsmComment {
        match &item.kind {
            AsmItemKind::Comment(c) => c,
            other => panic!("expected Comment, got {other:?}"),
        }
    }

    fn as_func(item: &AsmItem) -> &FuncCst {
        match &item.kind {
            AsmItemKind::Func(f) => f,
            other => panic!("expected Func, got {other:?}"),
        }
    }

    fn as_line(item: &AsmItem) -> &LineCst {
        match &item.kind {
            AsmItemKind::Line(l) => l,
            other => panic!("expected Line, got {other:?}"),
        }
    }

    fn as_raw(item: &AsmItem) -> &RawCst {
        match &item.kind {
            AsmItemKind::Raw(r) => r,
            other => panic!("expected Raw, got {other:?}"),
        }
    }

    fn label_names(line: &LineCst) -> Vec<&str> {
        line.labels.iter().map(|l| l.name.as_str()).collect()
    }

    fn instr_word(line: &LineCst) -> &str {
        &line.instr.as_ref().expect("expected an instruction").word
    }

    fn operand_texts(line: &LineCst) -> Vec<&str> {
        line.instr
            .as_ref()
            .expect("expected an instruction")
            .operands
            .iter()
            .map(|o| o.text.as_str())
            .collect()
    }

    fn trailing_text(trailing: &Option<TrailingComment>) -> Option<&str> {
        trailing.as_ref().map(|t| t.text.as_str())
    }

    // The `.pma` example from docs/formats.md (assembly text).
    const DOC_EXAMPLE: &str = "\
.func goToEnd                   ; emits ent, defines symbol
L1:     rgt
        jm      L1              ; assembler picks jm.s automatically
        lft
        ret

.func main
        call    goToEnd         ; width decided at link time
        rgt
        wr      1               ; mark
        stp
";

    #[test]
    fn doc_example_parses_into_the_expected_item_sequence() {
        let cst = parse_asm_cst(DOC_EXAMPLE);
        assert_eq!(cst.items.len(), 10);

        // Only the second Func (after the blank line) carries blank_before.
        let blanks: Vec<bool> = cst.items.iter().map(|i| i.blank_before).collect();
        assert_eq!(
            blanks,
            vec![
                false, false, false, false, false, true, false, false, false, false
            ]
        );

        let f = as_func(&cst.items[0]);
        assert_eq!(f.name, "goToEnd");
        assert!(!f.local);
        assert_eq!(
            trailing_text(&f.trailing),
            Some("; emits ent, defines symbol")
        );

        let l1 = as_line(&cst.items[1]);
        assert_eq!(label_names(l1), vec!["L1"]);
        assert_eq!(instr_word(l1), "rgt");
        assert_eq!(operand_texts(l1), Vec::<&str>::new());
        assert_eq!(l1.trailing, None);

        // The representative line for exact span assertions:
        //     `        jm      L1              ; assembler picks ...`
        let jm = as_line(&cst.items[2]);
        assert_eq!(jm.labels, vec![]);
        let instr = jm.instr.as_ref().unwrap();
        assert_eq!(instr.word, "jm");
        assert_eq!(instr.word_span, Span::new(3, 9, 3, 11));
        assert_eq!(
            instr.operands,
            vec![OperandToken {
                text: "L1".to_string(),
                span: Span::new(3, 17, 3, 19),
            }]
        );
        assert_eq!(jm.span, Span::new(3, 9, 3, 19)); // excludes the comment
        assert_eq!(
            jm.trailing,
            Some(TrailingComment {
                text: "; assembler picks jm.s automatically".to_string(),
                col: 33,
            })
        );

        assert_eq!(instr_word(as_line(&cst.items[3])), "lft");
        assert_eq!(instr_word(as_line(&cst.items[4])), "ret");

        let main = as_func(&cst.items[5]);
        assert_eq!(main.name, "main");
        assert!(!main.local);
        assert_eq!(main.trailing, None);

        let call = as_line(&cst.items[6]);
        assert_eq!(instr_word(call), "call");
        assert_eq!(operand_texts(call), vec!["goToEnd"]);
        assert_eq!(
            trailing_text(&call.trailing),
            Some("; width decided at link time")
        );

        assert_eq!(instr_word(as_line(&cst.items[7])), "rgt");

        let wr = as_line(&cst.items[8]);
        assert_eq!(instr_word(wr), "wr");
        assert_eq!(operand_texts(wr), vec!["1"]);
        assert_eq!(trailing_text(&wr.trailing), Some("; mark"));

        assert_eq!(instr_word(as_line(&cst.items[9])), "stp");
    }

    #[test]
    fn label_only_and_multi_label_lines() {
        let cst = parse_asm_cst("L1:\nA: B: nop\n");
        assert_eq!(cst.items.len(), 2);

        let only = as_line(&cst.items[0]);
        assert_eq!(
            only.labels,
            vec![LabelCst {
                name: "L1".to_string(),
                span: Span::new(1, 1, 1, 3),
            }]
        );
        assert_eq!(only.instr, None);
        assert_eq!(only.span, Span::new(1, 1, 1, 4)); // includes the colon

        let multi = as_line(&cst.items[1]);
        assert_eq!(
            multi.labels,
            vec![
                LabelCst {
                    name: "A".to_string(),
                    span: Span::new(2, 1, 2, 2),
                },
                LabelCst {
                    name: "B".to_string(),
                    span: Span::new(2, 4, 2, 5),
                },
            ]
        );
        assert_eq!(instr_word(multi), "nop");
        assert_eq!(multi.span, Span::new(2, 1, 2, 10));
    }

    #[test]
    fn dotted_word_before_a_colon_is_a_label_candidate_not_raw() {
        // Shape only — lower.rs rejects the bad label name with a
        // precise span; the CST must not misfile the line as Raw or
        // fold `foo.bar` into the instruction word.
        let cst = parse_asm_cst("foo.bar:  nop");
        assert_eq!(cst.items.len(), 1);
        let line = as_line(&cst.items[0]);
        assert_eq!(
            line.labels,
            vec![LabelCst {
                name: "foo.bar".to_string(),
                span: Span::new(1, 1, 1, 8),
            }]
        );
        assert_eq!(instr_word(line), "nop");
    }

    #[test]
    fn structurally_exact_func_directives_shape_as_func() {
        let cst = parse_asm_cst(".func f");
        let f = as_func(&cst.items[0]);
        assert_eq!(f.name, "f");
        assert!(!f.local);
        assert_eq!(f.name_span, Span::new(1, 7, 1, 8));
        assert_eq!(f.span, Span::new(1, 1, 1, 8));
        assert_eq!(f.trailing, None);

        let cst = parse_asm_cst(".func f local");
        let f = as_func(&cst.items[0]);
        assert_eq!(f.name, "f");
        assert!(f.local);
        assert_eq!(f.span, Span::new(1, 1, 1, 14));

        let cst = parse_asm_cst(".func f local ; note");
        let f = as_func(&cst.items[0]);
        assert!(f.local);
        assert_eq!(f.span, Span::new(1, 1, 1, 14)); // excludes the comment
        assert_eq!(trailing_text(&f.trailing), Some("; note"));
    }

    #[test]
    fn malformed_func_directives_stay_lines_with_word_func() {
        // lower.rs replicates the legacy errors from the operand region.
        let cases: [(&str, Vec<&str>); 3] = [
            (".func f loco", vec!["f loco"]),
            (".func f local extra", vec!["f local extra"]),
            (".func", vec![]),
        ];
        for (source, operands) in cases {
            let cst = parse_asm_cst(source);
            assert_eq!(cst.items.len(), 1, "{source:?}");
            let line = as_line(&cst.items[0]);
            assert_eq!(line.labels, vec![], "{source:?}");
            assert_eq!(instr_word(line), ".func", "{source:?}");
            assert_eq!(operand_texts(line), operands, "{source:?}");
        }
    }

    #[test]
    fn operands_keep_raw_spelling_and_split_at_commas() {
        let cst = parse_asm_cst("wr 007, -1 ; c");
        let line = as_line(&cst.items[0]);
        assert_eq!(instr_word(line), "wr");
        assert_eq!(
            line.instr.as_ref().unwrap().operands,
            vec![
                OperandToken {
                    text: "007".to_string(),
                    span: Span::new(1, 4, 1, 7),
                },
                OperandToken {
                    text: "-1".to_string(),
                    span: Span::new(1, 9, 1, 11),
                },
            ]
        );
        assert_eq!(
            line.trailing,
            Some(TrailingComment {
                text: "; c".to_string(),
                col: 12,
            })
        );
    }

    #[test]
    fn empty_operand_groups_yield_empty_text_tokens() {
        // `wr 1,,2`: cols  w=1 r=2 1=4 ,=5 ,=6 2=7 — the empty middle
        // group gets a zero-width span just past its left delimiter.
        let cst = parse_asm_cst("wr 1,,2");
        let line = as_line(&cst.items[0]);
        assert_eq!(operand_texts(line), vec!["1", "", "2"]);
        assert_eq!(
            line.instr.as_ref().unwrap().operands[1].span,
            Span::new(1, 6, 1, 6)
        );

        let cst = parse_asm_cst("wr 1,");
        let line = as_line(&cst.items[0]);
        assert_eq!(operand_texts(line), vec!["1", ""]);
    }

    #[test]
    fn operand_slices_preserve_interior_anomalies() {
        // `std :: api` must survive verbatim so lowering rejects it
        // exactly as today; `@name` stays one operand text.
        let cst = parse_asm_cst("call std :: api");
        let line = as_line(&cst.items[0]);
        assert_eq!(
            line.instr.as_ref().unwrap().operands,
            vec![OperandToken {
                text: "std :: api".to_string(),
                span: Span::new(1, 6, 1, 16),
            }]
        );

        let cst = parse_asm_cst("call @std::api");
        let line = as_line(&cst.items[0]);
        assert_eq!(operand_texts(line), vec!["@std::api"]);
    }

    #[test]
    fn listing_lines_shape_as_raw_with_verbatim_text() {
        let listing = "  0004:  21 05 00 00 00  call    0x0005 <goToEnd>";
        let cst = parse_asm_cst(listing);
        assert_eq!(cst.items.len(), 1);
        let raw = as_raw(&cst.items[0]);
        assert_eq!(raw.text, listing);
        let end = listing.chars().count() as u32 + 1;
        assert_eq!(raw.span, Span::new(1, 3, 1, end)); // trimmed extent

        let cst = parse_asm_cst("<goToEnd>");
        let raw = as_raw(&cst.items[0]);
        assert_eq!(raw.text, "<goToEnd>");
        assert_eq!(raw.span, Span::new(1, 1, 1, 10));
    }

    #[test]
    fn non_word_after_labels_shapes_as_raw() {
        // `label* [word operands]` — a non-Word where the instruction
        // word belongs means the line is not assembly-shaped.
        let cst = parse_asm_cst("A: 5");
        let raw = as_raw(&cst.items[0]);
        assert_eq!(raw.text, "A: 5");
        assert_eq!(raw.span, Span::new(1, 1, 1, 5));
    }

    #[test]
    fn raw_line_with_trailing_comment_keeps_full_extent() {
        // Unlike Line/Func, Raw does not split a trailing comment out:
        // both text and span cover through the end of the comment.
        let cst = parse_asm_cst("A: 5 ; note");
        assert_eq!(cst.items.len(), 1);
        let raw = as_raw(&cst.items[0]);
        assert_eq!(raw.text, "A: 5 ; note");
        assert_eq!(raw.span, Span::new(1, 1, 1, 12));
    }

    #[test]
    fn label_only_line_can_carry_a_trailing_comment() {
        let cst = parse_asm_cst("A: ; c");
        let line = as_line(&cst.items[0]);
        assert_eq!(label_names(line), vec!["A"]);
        assert_eq!(line.instr, None);
        assert_eq!(line.span, Span::new(1, 1, 1, 3));
        assert_eq!(trailing_text(&line.trailing), Some("; c"));
    }

    #[test]
    fn blank_line_runs_fold_to_one_blank_before() {
        let cst = parse_asm_cst("nop\n\n\nrgt\n");
        assert_eq!(cst.items.len(), 2);
        assert!(!cst.items[0].blank_before);
        assert!(cst.items[1].blank_before);
    }

    #[test]
    fn leading_file_blanks_set_nothing() {
        let cst = parse_asm_cst("\n   \nnop\n");
        assert_eq!(cst.items.len(), 1);
        assert!(!cst.items[0].blank_before);
    }

    #[test]
    fn own_line_comment_keeps_its_column() {
        let cst = parse_asm_cst("        ; note");
        assert_eq!(cst.items.len(), 1);
        let comment = as_comment(&cst.items[0]);
        assert_eq!(comment.text, "; note");
        assert_eq!(comment.col, 9);
    }

    // -- Opt-in caps: sections, table directives, `.rept` blocks --------

    fn caps_all() -> AsmCaps {
        AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        }
    }

    #[test]
    fn shapes_sections_and_table_directives() {
        let src = ".section tables\nTfetch: .row [1, *, *]\nDfetch: .targets A, B\n.section code\n";
        let cst = parse_asm_cst_with(src, caps_all());
        assert!(matches!(&cst.items[0].kind, AsmItemKind::Section(s) if s.name == "tables"));
        assert!(matches!(&cst.items[1].kind, AsmItemKind::TableDirective(d)
            if matches!(d.kind, TableDirectiveKind::Row) && d.labels[0].name == "Tfetch"));
        assert!(matches!(&cst.items[2].kind, AsmItemKind::TableDirective(d)
            if matches!(d.kind, TableDirectiveKind::Targets) && d.operands.len() == 2));
        assert!(matches!(&cst.items[3].kind, AsmItemKind::Section(s) if s.name == "code"));
    }

    #[test]
    fn rept_body_is_kept_verbatim() {
        let src = ".rept v, 0, 2\nLinc{v}: nop\n.endr\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let AsmItemKind::Rept(r) = &cst.items[0].kind else {
            panic!("not a rept")
        };
        assert_eq!((r.var.as_str(), r.lo, r.hi), ("v", 0, 2));
        assert_eq!(r.body.len(), 1); // one line, unexpanded
        // The block is self-describing: the header spans line 1, the
        // `.endr` word spans line 3, so the body occupies exactly line 2.
        assert_eq!(r.span, Span::new(1, 1, 1, 14));
        assert_eq!(r.endr_span, Span::new(3, 1, 3, 6));
        assert_eq!(r.endr_trailing, None);
    }

    #[test]
    fn rept_retains_the_endr_trailing_comment() {
        let src = ".rept v, 0, 0\n        nop\n.endr   ; close\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let AsmItemKind::Rept(r) = &cst.items[0].kind else {
            panic!("not a rept")
        };
        assert_eq!(r.endr_span, Span::new(3, 1, 3, 6)); // the `.endr` word
        assert_eq!(trailing_text(&r.endr_trailing), Some("; close"));
    }

    #[test]
    fn default_caps_shape_unchanged() {
        // The same source under default caps: every unknown-directive line
        // becomes Raw (via Junk) or a Line, exactly as before this task.
        let cst = parse_asm_cst(".section tables\n");
        assert!(!matches!(&cst.items[0].kind, AsmItemKind::Section(_)));
    }

    #[test]
    fn instruction_vector_operand_is_one_verbatim_token() {
        // With caps.vectors, an instruction's bracketed operand region is
        // one lossless `[..]` token — the interior commas do not split it
        // (mirrors the `.row` treatment below).
        let cst = parse_asm_cst_with("vwrite [1, -, 2]\n", caps_all());
        let line = as_line(&cst.items[0]);
        assert_eq!(operand_texts(line), vec!["[1, -, 2]"]);
    }

    #[test]
    fn row_vector_is_one_verbatim_operand_not_comma_split() {
        // The interior commas of `[1, *, *]` must NOT split the operand:
        // the whole bracketed slice is captured as a single lossless token.
        let cst = parse_asm_cst_with(".row [1, *, *]\n", caps_all());
        let AsmItemKind::TableDirective(d) = &cst.items[0].kind else {
            panic!("not a table directive")
        };
        assert_eq!(d.operands.len(), 1);
        assert_eq!(d.operands[0].text, "[1, *, *]");
    }

    #[test]
    fn unterminated_rept_degrades_its_header_to_a_line() {
        // No `.endr`: the `.rept` header degrades to a plain Line (mirror
        // malformed-`.func`); no Rept node is produced.
        let cst = parse_asm_cst_with(".rept v, 0, 2\nnop\n", caps_all());
        assert!(
            !cst.items
                .iter()
                .any(|it| matches!(it.kind, AsmItemKind::Rept(_)))
        );
    }

    #[test]
    fn stray_endr_without_open_rept_is_a_line() {
        // `.endr` outside any `.rept` is not a block-closer here — it
        // shapes as a plain Line (word `.endr`); lower rejects it.
        let cst = parse_asm_cst_with(".endr\n", caps_all());
        assert!(matches!(&cst.items[0].kind, AsmItemKind::Line(l)
            if l.instr.as_ref().unwrap().word == ".endr"));
    }

    #[test]
    fn shapes_routine_directive_with_field_spans() {
        let src = ".routine main, tapes=2, alpha=(3, 5) ; sig\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let AsmItemKind::RoutineDirective(r) = &cst.items[0].kind else {
            panic!("not a routine directive: {:?}", cst.items[0].kind)
        };
        assert_eq!(r.name, "main");
        assert_eq!(r.name_span, Span::new(1, 10, 1, 14));
        assert_eq!(r.tapes, 2);
        assert_eq!(r.tapes_span, Span::new(1, 22, 1, 23));
        assert_eq!(r.alpha, vec![3, 5]);
        assert_eq!(r.alpha_span, Span::new(1, 31, 1, 37));
        assert_eq!(r.span, Span::new(1, 1, 1, 37)); // excludes the comment
        assert_eq!(trailing_text(&r.trailing), Some("; sig"));
    }

    #[test]
    fn malformed_routine_directives_stay_lines() {
        for src in [
            ".routine main",                         // fields missing
            ".routine main, tapes=2",                // alpha missing
            ".routine main, tapes=x, alpha=(1)",     // not a number
            ".routine main, tapes=02, alpha=(1, 1)", // non-canonical spelling
            ".routine main, tapes=2, alpha=(1,)",    // trailing comma
            ".routine main, tapes=2, alpha=()",      // empty list
            ".routine main, tapes=2, alpha=(1) x",   // junk after the list
        ] {
            let cst = parse_asm_cst_with(src, caps_all());
            assert!(
                matches!(&cst.items[0].kind, AsmItemKind::Line(l)
                    if l.instr.as_ref().unwrap().word == ".routine"),
                "{src:?} must degrade to a Line: {:?}",
                cst.items[0].kind
            );
        }
    }

    #[test]
    fn default_caps_never_shape_routine() {
        // Byte-compat: with caps off `=` (and the parens) stay Junk, so
        // the line is Raw — exactly as before the directive existed.
        let cst = parse_asm_cst(".routine main, tapes=2, alpha=(3, 5)\n");
        assert!(matches!(&cst.items[0].kind, AsmItemKind::Raw(_)));
    }

    // -- Frame-descriptor directives (caps.tables + rept + arrows) ------

    fn as_frame(item: &AsmItem) -> &FrameDirectiveCst {
        match &item.kind {
            AsmItemKind::FrameDirective(d) => d,
            other => panic!("expected FrameDirective, got {other:?}"),
        }
    }

    #[test]
    fn shapes_the_frame_directive_family() {
        let src = "\
F0: .frame tapes=(3, 0)
    .map 0, rmap=(2->1, 4=>0), wmap=(1->2)
    .exits Lodd, Leven
";
        let cst = parse_asm_cst_with(src, caps_all());
        assert_eq!(cst.items.len(), 3);

        let FrameDirectiveCst::Header(h) = as_frame(&cst.items[0]) else {
            panic!("not a header: {:?}", cst.items[0].kind)
        };
        assert_eq!(h.label.name, "F0");
        assert_eq!(h.tapes, vec![3, 0]);
        assert_eq!(h.tapes_span, Span::new(1, 18, 1, 24)); // the `(3, 0)` group

        let FrameDirectiveCst::Map(m) = as_frame(&cst.items[1]) else {
            panic!("not a map: {:?}", cst.items[1].kind)
        };
        assert_eq!(m.k, 0);
        assert_eq!(
            m.rmap,
            Some(vec![
                FramePairCst {
                    from: 2,
                    to: 1,
                    one_way: false
                },
                FramePairCst {
                    from: 4,
                    to: 0,
                    one_way: true
                },
            ])
        );
        assert_eq!(
            m.wmap,
            Some(vec![FramePairCst {
                from: 1,
                to: 2,
                one_way: false
            }])
        );

        let FrameDirectiveCst::Exits(e) = as_frame(&cst.items[2]) else {
            panic!("not exits: {:?}", cst.items[2].kind)
        };
        assert_eq!(
            e.targets
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Lodd", "Leven"]
        );
    }

    #[test]
    fn map_clauses_are_each_optional() {
        // rmap-only, wmap-only, and neither all shape.
        let cst = parse_asm_cst_with(".map 1, rmap=(2->3)\n", caps_all());
        let FrameDirectiveCst::Map(m) = as_frame(&cst.items[0]) else {
            panic!()
        };
        assert!(m.rmap.is_some() && m.wmap.is_none());

        let cst = parse_asm_cst_with(".map 1, wmap=(2->3)\n", caps_all());
        let FrameDirectiveCst::Map(m) = as_frame(&cst.items[0]) else {
            panic!()
        };
        assert!(m.rmap.is_none() && m.wmap.is_some());

        let cst = parse_asm_cst_with(".map 2\n", caps_all());
        let FrameDirectiveCst::Map(m) = as_frame(&cst.items[0]) else {
            panic!()
        };
        assert!(m.rmap.is_none() && m.wmap.is_none() && m.k == 2);
    }

    #[test]
    fn wmap_one_way_pair_still_shapes_at_the_cst_level() {
        // A one-way `=>` pair in `wmap` is a *lowering* error (wmap is the
        // write direction; `=>` is read-direction only), NOT a shaping one:
        // the CST stays lossless so fmt round-trips the source verbatim and
        // lowering reports a precise span. The pair carries `one_way: true`.
        let cst = parse_asm_cst_with(".map 0, wmap=(1=>2)\n", caps_all());
        let FrameDirectiveCst::Map(m) = as_frame(&cst.items[0]) else {
            panic!("not a map: {:?}", cst.items[0].kind)
        };
        assert_eq!(
            m.wmap,
            Some(vec![FramePairCst {
                from: 1,
                to: 2,
                one_way: true
            }])
        );
    }

    #[test]
    fn malformed_frame_directives_degrade_to_lines() {
        for src in [
            ".frame tapes=(1)\n",                 // no descriptor label
            "F0: .frame tapes=()\n",              // empty tapes list
            "F0: .frame tapes=(1,)\n",            // trailing comma
            "L0: .map 0, rmap=(1->2)\n",          // `.map` must not carry a label
            ".map 0, wmap=(1->2), rmap=(2->1)\n", // wmap before rmap
            ".map 0, rmap=(1->)\n",               // pair missing its `to`
            ".map 0, rmap=(1-2)\n",               // no arrow token (`-` then `2`)
        ] {
            let cst = parse_asm_cst_with(src, caps_all());
            assert!(
                matches!(
                    &cst.items[0].kind,
                    AsmItemKind::Line(_) | AsmItemKind::Raw(_)
                ),
                "{src:?} must degrade, got {:?}",
                cst.items[0].kind
            );
        }
    }

    #[test]
    fn default_caps_never_shape_frame_directives() {
        // Byte-compat: with the tables cap off the arrows/parens stay Junk,
        // so a `.map` line shapes Raw — exactly as before frames existed.
        let cst = parse_asm_cst(".map 0, rmap=(2->1)\n");
        assert!(matches!(&cst.items[0].kind, AsmItemKind::Raw(_)));
    }

    #[test]
    fn nested_rept_degrades_the_inner_to_a_line_body_item() {
        // The inner `.rept` does NOT open a nested block — it degrades to
        // a Line body item; the first `.endr` closes the outer block.
        let src = ".rept v, 0, 1\n.rept w, 0, 1\n.endr\n.endr\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let AsmItemKind::Rept(outer) = &cst.items[0].kind else {
            panic!("outer not a rept")
        };
        assert!(matches!(&outer.body[0].kind, AsmItemKind::Line(l)
            if l.instr.as_ref().unwrap().word == ".rept"));
    }

    // -- List continuation: a trailing comma joins the next line --------

    fn table_directives(cst: &AsmCst) -> Vec<&TableDirectiveCst> {
        cst.items
            .iter()
            .filter_map(|i| match &i.kind {
                AsmItemKind::TableDirective(d) => Some(d),
                _ => None,
            })
            .collect()
    }

    fn operand_slices(operands: &[OperandToken]) -> Vec<&str> {
        operands.iter().map(|o| o.text.as_str()).collect()
    }

    #[test]
    fn a_trailing_comma_continues_a_targets_list() {
        let src = ".section tables\nD0:     .targets aa,\n        bb\n.section code\n";
        let cst = parse_asm_cst_with(src, caps_all());
        // The two physical lines are ONE logical directive carrying both
        // labels — not two items, and not a Raw line.
        let targets = table_directives(&cst);
        assert_eq!(targets.len(), 1, "one logical directive");
        assert_eq!(operand_slices(&targets[0].operands), vec!["aa", "bb"]);
    }

    #[test]
    fn a_trailing_comma_continues_an_exits_list() {
        let src = ".section tables\nF0:     .frame tapes=(0)\n        .exits aa,\n        bb\n.section code\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let AsmItemKind::FrameDirective(FrameDirectiveCst::Exits(e)) = &cst.items[2].kind else {
            panic!("expected a continued .exits, got {:?}", cst.items[2].kind);
        };
        assert_eq!(operand_slices(&e.targets), vec!["aa", "bb"]);
        assert_eq!(cst.items.len(), 4); // two sections, the frame, the exits
    }

    #[test]
    fn a_trailing_comma_continues_a_map_clause() {
        // The comma inside the `rmap=(..)` group continues the line too —
        // the rule keys on the directive, not on where in it the comma is.
        let src = ".section tables\nF0:     .frame tapes=(0, 1)\n        .map 0, rmap=(1->2,\n                      3->4)\n.section code\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let AsmItemKind::FrameDirective(FrameDirectiveCst::Map(m)) = &cst.items[2].kind else {
            panic!("expected a continued .map, got {:?}", cst.items[2].kind);
        };
        assert_eq!(
            m.rmap,
            Some(vec![
                FramePairCst {
                    from: 1,
                    to: 2,
                    one_way: false
                },
                FramePairCst {
                    from: 3,
                    to: 4,
                    one_way: false
                },
            ])
        );
        // The `(..)` group opens on line 3 and closes on line 4.
        assert_eq!(m.rmap_span, Some(Span::new(3, 22, 4, 28)));
    }

    #[test]
    fn a_continued_list_round_trips_byte_for_byte() {
        let src = ".section tables\nD0:     .targets aa,\n                 bb, cc        ; the rest\n.section code\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let lines = cst.items[1]
            .continuation
            .as_ref()
            .expect("the directive spans two physical lines");
        let source_lines: Vec<&str> = src.lines().collect();
        // Every retained segment IS its physical source line, indentation
        // and trailing comment included — so the join reproduces the
        // region byte for byte, and `line_no` is the physical number.
        for line in lines {
            assert_eq!(line.text, source_lines[line.line_no as usize - 1]);
        }
        assert_eq!(
            lines.iter().map(|l| l.line_no).collect::<Vec<_>>(),
            vec![2, 3]
        );
        let region: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(region.join("\n"), source_lines[1..3].join("\n"));
        // The comment on the last segment is the directive's own trailing
        // comment; the shaped directive reads as one line.
        let d = table_directives(&cst)[0];
        assert_eq!(operand_slices(&d.operands), vec!["aa", "bb", "cc"]);
        assert_eq!(trailing_text(&d.trailing), Some("; the rest"));
    }

    #[test]
    fn a_continued_list_keeps_physical_line_numbers() {
        let src = ".section tables\nD0:     .targets aa,\n        bb,\n        cc\n.section code\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let d = table_directives(&cst)[0];
        // Each operand's span names the line it was really written on, so
        // a diagnostic about `cc` points at line 4, not at line 2.
        let operand_lines: Vec<u32> = d.operands.iter().map(|o| o.span.start.line).collect();
        assert_eq!(operand_lines, vec![2, 3, 4]);
        assert_eq!(d.operands[2].span, Span::new(4, 9, 4, 11));
        // The directive's own extent runs from its first line to its last.
        assert_eq!(d.span, Span::new(2, 1, 4, 11));
        // And the item after it resumes at the right physical line.
        assert!(matches!(&cst.items[2].kind, AsmItemKind::Section(s)
            if s.name == "code" && s.span.start.line == 5));
    }

    #[test]
    fn a_trailing_comma_outside_the_three_lists_stays_an_error() {
        // `.row`, `.target` and an ordinary instruction all keep their
        // dangling comma — the line below stays a separate item.
        for src in [
            ".section tables\nT0:     .row [1],\n        [2]\n",
            ".section tables\nD0:     .target aa,\n        bb\n",
            ".func f\n        wr 1,\n        2\n",
            ".routine main, tapes=1,\n        alpha=(2)\n",
        ] {
            let cst = parse_asm_cst_with(src, caps_all());
            assert!(
                cst.items.iter().all(|i| i.continuation.is_none()),
                "{src:?} must not continue"
            );
            assert_eq!(cst.items.len(), src.lines().count(), "{src:?}");
        }
    }

    #[test]
    fn a_trailing_comma_never_continues_without_the_tables_cap() {
        // The three continuable directives all ride `caps.tables`, so a
        // dialect without it — `.pma`, and the classic grammar — cannot
        // reach the fold at all.
        let src = ".section tables\nD0:     .targets aa,\n        bb\n";
        for caps in [
            AsmCaps::default(),
            AsmCaps {
                tables: false,
                rept: true,
                vectors: true,
            },
        ] {
            let cst = parse_asm_cst_with(src, caps);
            assert_eq!(cst.items.len(), 3);
            assert!(cst.items.iter().all(|i| i.continuation.is_none()));
        }
    }

    #[test]
    fn a_comma_followed_by_a_comment_does_not_continue() {
        // The comma must be the line's LAST token. Otherwise the comment
        // would land in the middle of the joined list, where no consumer
        // expects one — so this keeps the error it has today.
        let src = ".section tables\nD0:     .targets aa,  ; more below\n        bb\n";
        let cst = parse_asm_cst_with(src, caps_all());
        assert_eq!(cst.items.len(), 3);
        assert!(cst.items.iter().all(|i| i.continuation.is_none()));
    }

    #[test]
    fn a_blank_line_ends_a_continuation() {
        let src = ".section tables\nD0:     .targets aa,\n\n        bb\n";
        let cst = parse_asm_cst_with(src, caps_all());
        assert_eq!(cst.items.len(), 3);
        assert!(cst.items.iter().all(|i| i.continuation.is_none()));
        assert!(cst.items[2].blank_before, "the blank line survives");
    }

    #[test]
    fn a_continuation_never_swallows_a_line_that_opens_a_statement() {
        // A stray dangling comma must not eat the directive, labeled line
        // or block below it.
        for (src, items) in [
            (".section tables\nD0:     .targets aa,\n.section code\n", 3),
            (".section tables\nD0:     .targets aa,\nbb:     nop\n", 3),
            (
                ".section tables\nD0:     .targets aa,\n.rept v, 0, 1\n.endr\n",
                3,
            ),
        ] {
            let cst = parse_asm_cst_with(src, caps_all());
            assert_eq!(cst.items.len(), items, "{src:?}");
            assert!(
                cst.items.iter().all(|i| i.continuation.is_none()),
                "{src:?}"
            );
        }
    }

    #[test]
    fn a_join_that_does_not_shape_into_a_list_falls_back_to_one_item_per_line() {
        // `.map` with junk after the continuation degrades — the joined
        // text is not a well-formed `.map`, so the fold is abandoned and
        // both lines shape exactly as they do today.
        let src = ".section tables\n        .map 0,\n        nonsense=(1)\n";
        let cst = parse_asm_cst_with(src, caps_all());
        assert_eq!(cst.items.len(), 3);
        assert!(cst.items.iter().all(|i| i.continuation.is_none()));
    }

    #[test]
    fn a_rept_body_item_is_always_one_physical_line() {
        // Continuation is a top-level fold only: expansion recovers each
        // body line by its number, so a two-line body item would expand to
        // half of itself. Inside a block the trailing comma keeps today's
        // behaviour.
        let src = ".rept v, 0, 1\nD{v}:   .targets aa,\n        bb\n.endr\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let AsmItemKind::Rept(r) = &cst.items[0].kind else {
            panic!("not a rept")
        };
        assert_eq!(r.body.len(), 2);
        assert!(r.body.iter().all(|i| i.continuation.is_none()));
    }

    proptest! {
        #[test]
        fn total_and_every_nonblank_line_becomes_an_item(source in any::<String>()) {
            let cst = parse_asm_cst(&source);
            // The lexer skips only spaces and tabs, so a line yields an
            // item exactly when it carries any other character.
            let nonblank = source
                .lines()
                .filter(|line| line.chars().any(|c| c != ' ' && c != '\t'))
                .count();
            prop_assert_eq!(cst.items.len(), nonblank);
        }
    }

    // -- The directive inventory matches the real recognizer -----------

    /// The fixture syntax with `caps` swapped in — the assembler probes
    /// below need every caps tier on one dialect.
    fn syntax_with(caps: AsmCaps) -> crate::asm::ArchSyntax {
        let mut syntax = crate::asm::syntax::fixture::test_syntax();
        syntax.caps = caps;
        syntax
    }

    /// A probe source whose directive line must reach a real handler
    /// when the word is recognized. Well-formed where a clean assembly
    /// proves recognition; deliberately malformed where the directive's
    /// own precise complaint (never "unknown mnemonic") proves it —
    /// which also keeps the probe valid on tiers missing the OTHER caps
    /// the well-formed spelling would need to lex (`.routine`'s parens
    /// ride the rept cap, `.row`'s brackets the vectors cap).
    fn recognized_probe(word: &str) -> String {
        match word {
            FUNC_WORD => ".func probe\nstop\n".to_string(),
            BYTE_WORD => ".func probe\n.byte 7\nstop\n".to_string(),
            SECTION_WORD => ".section code\n.func probe\nstop\n".to_string(),
            // `.rept`/`.endr` are only directives as a matched pair —
            // the block recognizer consumes both or neither.
            REPT_WORD | ENDR_WORD => ".func probe\n.rept v, 0, 0\nnop\n.endr\nstop\n".to_string(),
            // Bare in code: answered by the malformed-`.routine` /
            // malformed-frame-directive complaints.
            ROUTINE_WORD | FRAME_WORD | MAP_WORD | EXITS_WORD => {
                format!(".func probe\n{word}\nstop\n")
            }
            // Bare in the tables section: answered by the `.row` vector
            // complaint / the table-space label complaints.
            ROW_WORD | TARGETS_WORD | TARGET_WORD => {
                format!(".section tables\nT: {word}\n.section code\n.func probe\nstop\n")
            }
            _ => panic!("no probe for `{word}`"),
        }
    }

    /// Pins [`recognized_directives`] to the recognizer's actual
    /// behavior on every caps tier: each listed word must reach a real
    /// directive handler (its probe never answers "unknown mnemonic"
    /// naming it), and each word the tier does NOT list must fall
    /// through to mnemonic lookup and fail there. A word added to the
    /// inventory without a recognizer — or under the wrong cap — fails
    /// here; so does a recognizer whose cap gate moves.
    #[test]
    fn recognized_directives_match_the_real_recognizer() {
        use crate::asm::{AsmErrorKind, assemble};
        let all_on = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        let everything = recognized_directives(all_on);
        assert_eq!(everything.len(), 12, "the audited directive surface");
        let tiers = [
            AsmCaps::default(),
            AsmCaps {
                tables: true,
                ..AsmCaps::default()
            },
            AsmCaps {
                rept: true,
                ..AsmCaps::default()
            },
            all_on,
        ];
        for caps in tiers {
            let inventory = recognized_directives(caps);
            for word in &everything {
                if inventory.contains(word) {
                    if let Err(e) =
                        assemble(&syntax_with(caps), 0x7F, &recognized_probe(word), false)
                        && let AsmErrorKind::UnknownMnemonic(w) = &e.kind
                    {
                        assert_ne!(
                            w, word,
                            "inventory lists `{word}` under {caps:?}, but the \
                             recognizer rejects it as an unknown mnemonic"
                        );
                    }
                } else {
                    let source = format!(".func probe\n{word}\nstop\n");
                    let err = assemble(&syntax_with(caps), 0x7F, &source, false)
                        .expect_err("an unrecognized directive cannot assemble");
                    assert_eq!(
                        err.kind,
                        AsmErrorKind::UnknownMnemonic((*word).to_string()),
                        "inventory omits `{word}` under {caps:?}, so it must fall \
                         through to mnemonic lookup"
                    );
                }
            }
            // A word in no tier's inventory is unknown under every caps
            // combination — dotted words carry no blanket special-casing.
            let err = assemble(
                &syntax_with(caps),
                0x7F,
                ".func probe\n.bogus\nstop\n",
                false,
            )
            .expect_err("an invented directive cannot assemble");
            assert_eq!(
                err.kind,
                AsmErrorKind::UnknownMnemonic(".bogus".to_string())
            );
        }
        // The vectors cap adds operand tokens, never directives.
        assert_eq!(
            recognized_directives(AsmCaps {
                vectors: true,
                ..AsmCaps::default()
            }),
            recognized_directives(AsmCaps::default())
        );
    }
}
