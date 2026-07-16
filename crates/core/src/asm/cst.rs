//! Lossless assembly CST (docs/formats.md (assembly text)). Total:
//! every text parses — lines that are not assembly-shaped become Raw
//! nodes. Trivia-complete: comments with columns, blank-line presence,
//! raw text. Validity checking lives in lower.rs, not here.

use super::lexer::{AsmToken, AsmTokenKind, lex_line};
use super::syntax::AsmCaps;
use crate::diagnostics::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmCst {
    pub items: Vec<AsmItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmItem {
    pub blank_before: bool,
    pub kind: AsmItemKind,
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
/// (excluding a trailing comment); the closing `.endr` is consumed but
/// not stored — its own trailing comment, if any, is not retained (the
/// struct has no slot for it). `body` holds the block's lines shaped AS
/// WRITTEN — substitution markers `{…}` survive verbatim inside each
/// item's operand text; expansion happens in a later task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReptCst {
    pub var: String,
    pub lo: i64,
    pub hi: i64,
    pub body: Vec<AsmItem>,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
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
/// the block-opening below is gated on `caps.rept`, and `shape_line`'s
/// section/table branches on `caps.tables`, so every added path is dead
/// under default caps and the item sequence is unchanged.
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
                items.push(AsmItem {
                    blank_before: rec.blank_before,
                    kind: AsmItemKind::Rept(ReptCst {
                        var: header.var,
                        lo: header.lo,
                        hi: header.hi,
                        body,
                        span: header.span,
                        trailing: header.trailing,
                    }),
                });
                i += 1 + off + 1; // header + body + the closing `.endr`
                continue;
            }
            // Unterminated `.rept`: no matching `.endr`. Degrade the
            // header to a plain Line (mirror malformed-`.func`); the body
            // lines then shape as ordinary top-level items on the
            // following iterations, via the fall-through below.
        }
        items.push(AsmItem {
            blank_before: rec.blank_before,
            kind: shape_line(rec.line, &rec.tokens, rec.line_no, caps),
        });
        i += 1;
    }
    AsmCst { items }
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
fn shape_body(records: &[LineRecord<'_>], caps: AsmCaps) -> Vec<AsmItem> {
    records
        .iter()
        .map(|rec| AsmItem {
            blank_before: rec.blank_before,
            kind: shape_line(rec.line, &rec.tokens, rec.line_no, caps),
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
    if word_text(first) != Some(".rept") {
        return None;
    }
    let operands = operand_region(rec.line, rest, rec.line_no, first.col + first.len);
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
    matches!(body, [only] if word_text(only) == Some(".endr"))
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
fn shape_line(line: &str, tokens: &[AsmToken], line_no: u32, caps: AsmCaps) -> AsmItemKind {
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
        return raw_line(line, tokens, line_no);
    }

    // Split off the trailing comment; `body` keeps at least tokens[0].
    let (body, trailing) = split_trailing(tokens);
    let last = body.last().expect("first token is a Word, never Comment");
    // The item's span: the line's trimmed extent minus the comment.
    let span = Span::new(line_no, body[0].col, line_no, last.col + last.len);

    // `.func` special case: structurally exact directives only.
    // Anything else starting `.func` stays a Line so lower.rs can
    // replicate the legacy errors verbatim.
    if word_text(&body[0]) == Some(".func") {
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
        && word_text(&body[0]) == Some(".section")
        && let [_, name] = body
        && let Some(name) = word_text(name)
    {
        return AsmItemKind::Section(SectionCst {
            name: name.to_string(),
            span,
            trailing,
        });
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
        return raw_line(line, tokens, line_no);
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
            TableDirectiveKind::Row => vector_operand(line, region, line_no).map(|op| vec![op]),
            TableDirectiveKind::Targets | TableDirectiveKind::Target => Some(operand_region(
                line,
                region,
                line_no,
                word_token.col + word_token.len,
            )),
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

    // A bracketed operand region on an ordinary instruction line
    // (caps.vectors) is captured as ONE verbatim `[..]` token, exactly
    // like a `.row`'s vector — the interior commas must not split it.
    // Under default caps `LBracket` tokens never exist, so this is dead
    // and the comma-split below is byte-identical to before.
    let region = &body[at + 1..];
    let operands = if caps.vectors
        && let Some(op) = vector_operand(line, region, line_no)
    {
        vec![op]
    } else {
        operand_region(line, region, line_no, word_token.col + word_token.len)
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

/// The [`TableDirectiveKind`] a leading directive word names, or `None`
/// for any other word.
fn table_directive_kind(word: &str) -> Option<TableDirectiveKind> {
    match word {
        ".row" => Some(TableDirectiveKind::Row),
        ".targets" => Some(TableDirectiveKind::Targets),
        ".target" => Some(TableDirectiveKind::Target),
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
fn vector_operand(line: &str, region: &[AsmToken], line_no: u32) -> Option<OperandToken> {
    let [first, .., last] = region else {
        return None;
    };
    if !matches!(first.kind, AsmTokenKind::LBracket) || !matches!(last.kind, AsmTokenKind::RBracket)
    {
        return None;
    }
    let start = first.col;
    let end = last.col + last.len;
    // Columns are char-counted (crate::diagnostics), so slice by chars.
    let text: String = line
        .chars()
        .skip(start as usize - 1)
        .take((end - start) as usize)
        .collect();
    Some(OperandToken {
        text: text.trim().to_string(),
        span: Span::new(line_no, start, line_no, end),
    })
}

/// The lossless fallback: verbatim line text; span = the line's
/// trimmed extent (all tokens, including a trailing comment).
fn raw_line(line: &str, tokens: &[AsmToken], line_no: u32) -> AsmItemKind {
    let first = &tokens[0];
    let last = tokens.last().expect("caller guarantees tokens");
    AsmItemKind::Raw(RawCst {
        text: line.to_string(),
        span: Span::new(line_no, first.col, line_no, last.col + last.len),
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
fn operand_region(
    line: &str,
    region: &[AsmToken],
    line_no: u32,
    after_word_col: u32,
) -> Vec<OperandToken> {
    if region.is_empty() {
        return Vec::new();
    }
    let mut operands = Vec::new();
    let mut group: Vec<&AsmToken> = Vec::new();
    let mut empty_group_col = after_word_col;
    for token in region {
        if matches!(token.kind, AsmTokenKind::Comma) {
            operands.push(operand_token(line, &group, line_no, empty_group_col));
            group.clear();
            empty_group_col = token.col + token.len;
        } else {
            group.push(token);
        }
    }
    operands.push(operand_token(line, &group, line_no, empty_group_col));
    operands
}

fn operand_token(
    line: &str,
    group: &[&AsmToken],
    line_no: u32,
    empty_group_col: u32,
) -> OperandToken {
    let (Some(first), Some(last)) = (group.first(), group.last()) else {
        return OperandToken {
            text: String::new(),
            span: Span::new(line_no, empty_group_col, line_no, empty_group_col),
        };
    };
    let start = first.col;
    let end = last.col + last.len;
    // Columns are char-counted (crate::diagnostics), so slice by chars.
    let text: String = line
        .chars()
        .skip(start as usize - 1)
        .take((end - start) as usize)
        .collect();
    OperandToken {
        text: text.trim().to_string(),
        span: Span::new(line_no, start, line_no, end),
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
}
