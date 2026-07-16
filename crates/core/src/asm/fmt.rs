//! Canonical-grid printer for assembly text (docs/formats.md (assembly
//! text)): label col 0, mnemonic col 8, operand col 16, trailing
//! comment col 32. Zero token changes — whitespace/newlines only.
//!
//! Pure CST walk (mirrors the `.pmc` printer's discipline, `crates/
//! post-machine/src/fmt/mod.rs`, but is far simpler: assembly text is
//! already line-oriented, so there is no comma-group wrapping, no
//! indentation nesting, no line-width budget — every field lands on a
//! fixed column or, failing that, one space past wherever the previous
//! field ended). Columns below are 0-based (tab-stop convention,
//! matching `disassembler.rs`'s `grid_line` and docs/formats.md); the
//! CST's `Span`/`Pos::col` fields are 1-based, so a 0-based target of
//! 32 is the 1-based column 33 a `TrailingComment.col` would report.

use super::cst::{
    AsmItem, AsmItemKind, LabelCst, LineCst, OperandToken, ReptCst, RoutineDirectiveCst,
    SectionCst, TableDirectiveCst, TableDirectiveKind, TrailingComment, parse_asm_cst_with,
};
use super::syntax::AsmCaps;
use super::{AsmError, AsmErrorKind};

const TOP_COL: usize = 0;
const MNEMONIC_COL: usize = 8;
const OPERAND_COL: usize = 16;
const COMMENT_COL: usize = 32;

/// A label field (name + `:`) of this many chars or fewer leaves a
/// mandatory `>= 1` space before [`MNEMONIC_COL`] and stays on the same
/// physical line as whatever follows; an 8-char field would touch the
/// mnemonic column, so it — and any longer field — moves to its own
/// line instead.
const MAX_INLINE_LABEL_FIELD: usize = 7;

/// `.pma` source → canonical grid text, classic dialect (no opt-in
/// surface). Thin wrapper over [`format_asm_with`] at
/// [`AsmCaps::default`] — byte-identical to the pre-caps printer, since
/// sections, table directives, and `.rept` blocks never shape under the
/// default caps.
pub fn format_asm(source: &str) -> Result<String, AsmError> {
    format_asm_with(source, AsmCaps::default())
}

/// `.pma` source → canonical grid text under `caps` (the dialect's
/// opt-in surface). Err = the structural gate: the file contains a Raw
/// (non-assembly) line — a disassembly-listing row, a stray `<name>`,
/// `A: 5`, and the like; nothing else refuses (an unknown mnemonic still
/// formats — this layer has no semantic gate, only the CST's structural
/// one). Thin renderer: never prints.
///
/// The opt-in nodes normalize to the same column grid as ordinary lines,
/// with ONE exception: a `.rept` block's BODY prints VERBATIM from source
/// (macros as written) — see [`print_rept`] — because a body item's CST
/// shaping is intentionally imperfect for substitution templates
/// (`Linc{v}: nop` shapes labelless), and grid-printing it would corrupt
/// its text.
pub fn format_asm_with(source: &str, caps: AsmCaps) -> Result<String, AsmError> {
    let cst = parse_asm_cst_with(source, caps);
    if let Some(raw) = cst.items.iter().find_map(|item| match &item.kind {
        AsmItemKind::Raw(r) => Some(r),
        _ => None,
    }) {
        return Err(AsmError {
            span: raw.span,
            kind: AsmErrorKind::RawLine,
        });
    }

    let mut out = String::new();
    let mut seen_func = false;
    for (i, item) in cst.items.iter().enumerate() {
        // Blank-line runs already collapsed to one bool by the CST
        // (`blank_before`); item 0 is guaranteed `false` by construction
        // (no leading file blanks), so this also gives "no leading
        // blanks" for free — the `i > 0` guard is defensive, matching
        // the `.pmc` printer's convention.
        if i > 0 && item.blank_before {
            out.push('\n');
        }
        match &item.kind {
            AsmItemKind::Comment(c) => {
                let col = own_line_comment_col(&cst.items, i, seen_func);
                let mut line = " ".repeat(col);
                line.push_str(&c.text);
                out.push_str(line.trim_end());
                out.push('\n');
            }
            AsmItemKind::Func(f) => {
                seen_func = true;
                let mut line = String::from(".func ");
                line.push_str(&f.name);
                if f.local {
                    line.push_str(" local");
                }
                let mut col = line.chars().count();
                if let Some(tc) = &f.trailing {
                    pad_to(&mut line, &mut col, COMMENT_COL);
                    line.push_str(&tc.text);
                }
                out.push_str(line.trim_end());
                out.push('\n');
            }
            AsmItemKind::Line(l) => print_line(&mut out, l),
            AsmItemKind::Raw(_) => unreachable!("the structural gate above already refused"),
            AsmItemKind::Section(s) => print_section(&mut out, s),
            AsmItemKind::TableDirective(d) => print_table_directive(&mut out, d),
            AsmItemKind::Rept(r) => print_rept(&mut out, r, source),
            AsmItemKind::RoutineDirective(r) => print_routine(&mut out, r),
        }
    }
    Ok(out)
}

/// Own-line comment indent (docs/formats.md (assembly text)): column 8
/// inside a function's body, column 0 at top level — before the first
/// `.func`, or in the gap between two functions. "The gap" is read as a
/// forward-looking property, not a state reset: a run of own-line
/// comments that leads into the NEXT `.func` (with no code line in
/// between) reads as belonging to that upcoming function header, not to
/// the body just left, so the whole run prints at column 0 — matching
/// how such a comment block is typically meant (a header note for what
/// follows) rather than a dangling footnote on what came before.
fn own_line_comment_col(items: &[AsmItem], i: usize, seen_func: bool) -> usize {
    if !seen_func {
        return TOP_COL;
    }
    let mut j = i;
    while j < items.len() && matches!(items[j].kind, AsmItemKind::Comment(_)) {
        j += 1;
    }
    match items.get(j) {
        Some(item) if matches!(item.kind, AsmItemKind::Func(_)) => TOP_COL,
        _ => MNEMONIC_COL,
    }
}

/// One `label* [word operands] [; comment]` line. Non-last labels
/// always get their own line (position rule); the last label shares
/// its rule with a solo label — own line at 8+ chars, otherwise inline
/// with whatever follows on the same physical line. When the line ends
/// up with nothing to print after an own-line label (a long, bare,
/// label-only line), the empty continuation is dropped rather than
/// leaving a blank line behind.
///
/// A long label with NO instruction (a label-only line) is the one
/// case where "own line" must NOT split the physical line further: if
/// it carries a trailing comment, that comment stays on the label's
/// own line (padded to [`COMMENT_COL`], or one space past the field
/// when the field itself runs past that stop) rather than moving to a
/// bare continuation line. A bare continuation reparses as an OWN-LINE
/// comment (no label on that physical line), which the printer would
/// then re-indent to [`MNEMONIC_COL`] on a second pass — an
/// idempotence violation (`format(format(x)) != format(x)`). Keeping
/// the comment on the label's line reparses to the identical
/// label-with-trailing-comment shape, so pass 1 is already a fixed
/// point. This only applies when `instr` is `None`: when an
/// instruction follows, it owns the continuation line and the label
/// line has nothing else to carry.
fn print_line(out: &mut String, line: &LineCst) {
    let instr = line
        .instr
        .as_ref()
        .map(|i| (i.word.as_str(), i.operands.as_slice()));
    print_fields(out, &line.labels, instr, &line.trailing);
}

/// The shared `label* [word operands] [; comment]` grid printer, driving
/// both [`print_line`] and [`print_table_directive`] — a table directive
/// (`.row`/`.targets`/`.target`) is the same shape with a mandatory
/// directive word standing in for the mnemonic. `instr` is `None` only
/// for a label-only Line; a table directive always passes `Some`, so the
/// long-label-only-line-with-trailing-comment idempotency guard (the
/// `instr.is_none()` branch below) can never fire for one.
fn print_fields(
    out: &mut String,
    labels: &[LabelCst],
    instr: Option<(&str, &[OperandToken])>,
    trailing: &Option<TrailingComment>,
) {
    let n = labels.len();
    for label in &labels[..n.saturating_sub(1)] {
        out.push_str(&label.name);
        out.push_str(":\n");
    }

    let mut cur = String::new();
    if let Some(last) = labels.last() {
        let field = format!("{}:", last.name);
        let fits_inline = field.chars().count() <= MAX_INLINE_LABEL_FIELD;
        if fits_inline || instr.is_none() {
            cur.push_str(&field);
        } else {
            out.push_str(&field);
            out.push('\n');
        }
    }
    let mut col = cur.chars().count();

    if let Some((word, operands)) = instr {
        pad_to(&mut cur, &mut col, MNEMONIC_COL);
        cur.push_str(word);
        col += word.chars().count();

        let operand_text = join_operands(operands);
        if !operand_text.is_empty() {
            pad_to(&mut cur, &mut col, OPERAND_COL);
            cur.push_str(&operand_text);
            col += operand_text.chars().count();
        }
    }

    if let Some(tc) = trailing {
        pad_to(&mut cur, &mut col, COMMENT_COL);
        cur.push_str(&tc.text);
    }

    let trimmed = cur.trim_end();
    if !trimmed.is_empty() {
        out.push_str(trimmed);
        out.push('\n');
    }
}

/// `.routine name, tapes=N, alpha=(c1, c2, …)` — a column-0 directive
/// like `.func`, reconstructed from the parsed fields. The CST's
/// structurally-exact gate admits only canonically spelled values, so
/// the reconstruction changes no token's text; interior spacing
/// normalizes to the `, ` convention (whitespace-only, per this
/// printer's contract).
fn print_routine(out: &mut String, r: &RoutineDirectiveCst) {
    let alpha = r
        .alpha
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut line = format!(".routine {}, tapes={}, alpha=({})", r.name, r.tapes, alpha);
    let mut col = line.chars().count();
    if let Some(tc) = &r.trailing {
        pad_to(&mut line, &mut col, COMMENT_COL);
        line.push_str(&tc.text);
    }
    out.push_str(line.trim_end());
    out.push('\n');
}

/// `.section NAME` — a column-0 region marker, printed like `.func`
/// (single space before the name, trailing comment padded to
/// [`COMMENT_COL`]).
fn print_section(out: &mut String, s: &SectionCst) {
    let mut line = format!(".section {}", s.name);
    let mut col = line.chars().count();
    if let Some(tc) = &s.trailing {
        pad_to(&mut line, &mut col, COMMENT_COL);
        line.push_str(&tc.text);
    }
    out.push_str(line.trim_end());
    out.push('\n');
}

/// `.row [..]` / `.targets L1, ..` / `.target L` — the same
/// label/word/operands grid as an instruction line, with the directive
/// keyword standing in for the mnemonic. Operands print verbatim from
/// their CST tokens (a `.row` keeps its whole bracketed vector as one
/// token; `.targets` comma-joins its names), so interior spelling
/// survives.
fn print_table_directive(out: &mut String, d: &TableDirectiveCst) {
    let word = match d.kind {
        TableDirectiveKind::Row => ".row",
        TableDirectiveKind::Targets => ".targets",
        TableDirectiveKind::Target => ".target",
    };
    print_fields(
        out,
        &d.labels,
        Some((word, d.operands.as_slice())),
        &d.trailing,
    );
}

/// `.rept v, lo, hi` … `.endr`. The header and terminator normalize to
/// the grid (column-0 directives, like `.func`); the BODY prints VERBATIM
/// — every physical source line strictly between the header line and the
/// `.endr` line, exactly as written (macros as written), with only
/// trailing whitespace trimmed to honor the whole-file no-trailing-space
/// invariant. Body items are NOT re-shaped through the grid: a
/// substitution template such as `Linc{v}: nop` shapes labelless (the
/// `{` breaks the word), and grid-printing it would corrupt its spacing
/// and text. Recovering the body by physical-line range (`endr_span`
/// bounds it) also preserves body comments and blank lines, which carry
/// no line number of their own on a Comment item.
fn print_rept(out: &mut String, r: &ReptCst, source: &str) {
    // Header: reconstructed from the parsed bounds and normalized.
    let mut header = format!(".rept {}, {}, {}", r.var, r.lo, r.hi);
    let mut col = header.chars().count();
    if let Some(tc) = &r.trailing {
        pad_to(&mut header, &mut col, COMMENT_COL);
        header.push_str(&tc.text);
    }
    out.push_str(header.trim_end());
    out.push('\n');

    // Body: source lines (1-based) in (header_line, endr_line), verbatim.
    let lines: Vec<&str> = source.lines().collect();
    let body_start = r.span.start.line as usize + 1;
    let body_end = r.endr_span.start.line as usize;
    for n in body_start..body_end {
        if let Some(text) = lines.get(n - 1) {
            out.push_str(text.trim_end());
            out.push('\n');
        }
    }

    // Terminator: `.endr` (+ its retained trailing comment).
    let mut endr = String::from(".endr");
    let mut col = endr.chars().count();
    if let Some(tc) = &r.endr_trailing {
        pad_to(&mut endr, &mut col, COMMENT_COL);
        endr.push_str(&tc.text);
    }
    out.push_str(endr.trim_end());
    out.push('\n');
}

/// Operand text verbatim from the CST's `OperandToken`s (never
/// retokenized/rewritten — leading zeros, sign, spelling all survive),
/// comma-joined (docs/formats.md (assembly text)).
fn join_operands(operands: &[OperandToken]) -> String {
    operands
        .iter()
        .map(|o| o.text.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Advances `cur`/`col` to `target`: pads with spaces when there is
/// room, or inserts exactly one separating space when the cursor has
/// already reached or passed it (docs/formats.md (assembly text): "a
/// single space when a field overflows its stop").
fn pad_to(cur: &mut String, col: &mut usize, target: usize) {
    if *col < target {
        cur.push_str(&" ".repeat(target - *col));
        *col = target;
    } else {
        cur.push(' ');
        *col += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::assembler::assemble;
    use crate::asm::disassembler::disassemble_object;
    use crate::asm::lexer::{AsmTokenKind, lex_line};
    use crate::asm::syntax::AsmCaps;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::diagnostics::Span;

    // The `.pma` example from docs/formats.md (assembly text) — the
    // SAME constant `cst.rs`'s own doc-example test pins, reproduced
    // here as a `const` so this module's tests don't reach across a
    // sibling module's private `#[cfg(test)]` items.
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

    // -- Case 1: the docs example reprints byte-identically -----------

    #[test]
    fn case1_doc_example_is_a_fixed_point() {
        assert_eq!(format_asm(DOC_EXAMPLE).unwrap(), DOC_EXAMPLE);
    }

    // -- Case 2: scrambled whitespace formats TO the canonical text ---

    #[test]
    fn case2_scrambled_whitespace_formats_to_canonical() {
        // Same program, whitespace mangled: tight/loose spacing, tabs,
        // no grid alignment at all, and a spaced colon (`L1 :`) that
        // must normalize to `L1:` (a whitespace-only change — the
        // colon is a separate token either way, so the label name is
        // untouched).
        let scrambled = "\
.func goToEnd ; emits ent, defines symbol
L1 :  rgt
 jm L1 ; assembler picks jm.s automatically
lft
\tret

.func main
call goToEnd ; width decided at link time
rgt
   wr 1 ; mark
stp
";
        assert_eq!(format_asm(scrambled).unwrap(), DOC_EXAMPLE);
    }

    // -- Grid-stop overflow: single space, not padding to the next stop.
    // None of the 11 enumerated fixtures exercise this branch of
    // `pad_to` (every mnemonic/operand in them is short) — the brief's
    // "Printing rules (each is a test)" still covers it, so it gets its
    // own tests here.

    #[test]
    fn overflow_mnemonic_gets_one_space_before_operand() {
        // "verylongmnem" (12 chars) starts at col 8, ends at col 20 —
        // past OPERAND_COL (16) — so the operand gets exactly one
        // separating space instead of padding back to col 16.
        let src = ".func f\n        verylongmnem 1\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn overflow_boundary_mnemonic_exactly_at_operand_col() {
        // "abcdefgh" (8 chars) starts at col 8, ends EXACTLY at col 16
        // (OPERAND_COL) — `pad_to`'s `<` is strict, so landing exactly
        // on the stop still takes the overflow (one-space) branch, not
        // the padding branch. Mnemonic-side mirror of the label's
        // 7-vs-8 boundary test above.
        let src = ".func f\n        abcdefgh 1\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn overflow_operand_gets_one_space_before_trailing_comment() {
        // mnemonic "wr" ends at col 10, pads to col 16, then a 26-char
        // operand runs to col 42 — past COMMENT_COL (32) — so the
        // trailing comment gets one space, not padding back to 32.
        let operand = "a".repeat(26);
        let src = format!(".func f\n        wr      {operand} ; c\n");
        assert_eq!(format_asm(&src).unwrap(), src);
    }

    #[test]
    fn overflow_shapes_are_idempotent() {
        for src in [
            ".func f\n        verylongmnem 1\n".to_string(),
            ".func f\n        abcdefgh 1\n".to_string(),
            format!(".func f\n        wr      {} ; c\n", "a".repeat(26)),
        ] {
            let once = format_asm(&src).unwrap();
            let twice = format_asm(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Case 3: a long label goes on its own line ---------------------

    #[test]
    fn case3_long_label_own_line_instruction_follows() {
        let src = ".func f\nverylongname:  nop\n";
        let expected = ".func f\nverylongname:\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn case3_eight_char_field_boundary_is_own_line() {
        // "abcdefg:" is exactly 8 chars (7-letter name + colon) — the
        // brief's own stated boundary: 8+ is own-line, 7 or fewer
        // stays inline.
        let src = ".func f\nabcdefg: nop\n";
        let expected = ".func f\nabcdefg:\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn case3_seven_char_field_stays_inline() {
        // "abcdef:" is 7 chars — the largest field that still fits.
        let src = ".func f\nabcdef: nop\n";
        let expected = ".func f\nabcdef: nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    // -- Case 4: multi-label lines -------------------------------------

    #[test]
    fn case4_non_last_label_own_line_last_stays_inline() {
        let src = ".func f\nA: B: nop\n";
        let expected = ".func f\nA:\nB:      nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn case4_multi_label_with_a_long_last_label() {
        // Every label goes own-line here: A: is non-last (position
        // rule), verylongname: is last but too long (length rule).
        let src = ".func f\nA: verylongname: nop\n";
        let expected = ".func f\nA:\nverylongname:\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn label_only_line_short_label_no_trailing() {
        let src = ".func f\nL1:\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn label_only_line_with_trailing_comment() {
        // "A:" (2 chars) padded straight to COMMENT_COL (32).
        let src = format!(
            ".func f\nA:{}; c\n        nop\n",
            " ".repeat(COMMENT_COL - 2)
        );
        assert_eq!(format_asm(&src).unwrap(), src);
    }

    #[test]
    fn label_only_line_long_label_with_trailing_comment() {
        let src = ".func f\nverylongname:\n";
        let expected = ".func f\nverylongname:\n";
        assert_eq!(format_asm(src).unwrap(), expected);

        // Same, but with a trailing comment: the comment stays on the
        // LABEL's own line (padded from the field's end to col 32),
        // not on a bare continuation line. A bare continuation would
        // reparse as an own-line comment (no label on that physical
        // line) and get re-indented to MNEMONIC_COL on a second pass —
        // an idempotence violation. See `case5_idempotent_over_every_fixture`'s
        // `"verylongname: ; note\n"` fixture for the pinned round-trip.
        let src_c = ".func f\nverylongname: ; note\n";
        let expected_c = format!(
            ".func f\nverylongname:{}; note\n",
            " ".repeat(COMMENT_COL - "verylongname:".chars().count())
        );
        assert_eq!(format_asm(src_c).unwrap(), expected_c);
    }

    // -- Case 5 + 6 fixtures (shared) ----------------------------------

    fn fixtures() -> Vec<&'static str> {
        vec![
            DOC_EXAMPLE,
            ".func f\nverylongname:  nop\n",
            ".func f\nA: B: nop\n",
            ".func f\nA: verylongname: nop\n",
            ".func f\n        wr      007, -1  ; leading zero survives\n",
            ".func f\n        bogus   1, 2\n", // unknown mnemonic
            ".func f local\n        nop\n\n\n\n.func g\n        ret\n", // blank-run collapse
            "; preamble\n.func f\n        nop\n; between f and g\n.func g\n        ret\n; trailing\n",
            ".func f\n        nop\n        ; inside f\n        ret\n",
            ".func f\n        verylongmnem 1\n", // mnemonic overflows into the operand column
            ".func f\n        abcdefgh 1\n",     // mnemonic ends exactly at the operand column
            ".func f\nverylongname: ; note\n",   // long label-only line with a trailing comment
            ".func f\nverylongname: short: nop ; note\n", // multi-label variant: long non-last label + trailing comment on the short last label's instruction line
        ]
    }

    // -- Case 5: idempotence over every fixture ------------------------

    #[test]
    fn case5_idempotent_over_every_fixture() {
        for src in fixtures() {
            let once = format_asm(src).unwrap();
            let twice = format_asm(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Case 6: zero token changes ------------------------------------

    /// Flattens every physical line's tokens (in source order) into one
    /// `Vec<AsmTokenKind>`. `AsmTokenKind` carries no position — only
    /// kind + text (`Word`/`Number`/`Comment` hold their spelling) — so
    /// comparing the flattened sequences both drops columns AND
    /// compares comments by text, in one step. Input and output can
    /// have different LINE counts (a long label splits one source line
    /// into two printed lines) so this compares the flattened token
    /// stream, not line-by-line.
    fn flat_kinds(source: &str) -> Vec<AsmTokenKind> {
        source
            .lines()
            .enumerate()
            .flat_map(|(i, line)| lex_line(line, i as u32 + 1, AsmCaps::default()))
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn case6_zero_token_changes_over_every_fixture() {
        for src in fixtures() {
            let out = format_asm(src).unwrap();
            assert_eq!(
                flat_kinds(src),
                flat_kinds(&out),
                "token stream changed for {src:?}\n---\n{out}"
            );
        }
    }

    #[test]
    fn case6_flatten_sanity_check_on_a_relabeled_line() {
        // Direct check on the case-4 shape: `A: B: nop` -> `A:\nB:      nop\n`
        // must flatten to the identical token-kind sequence.
        let input = "A: B: nop";
        let output = "A:\nB:      nop";
        assert_eq!(flat_kinds(input), flat_kinds(output));
    }

    // -- Case 7: leading-zero / signed operands survive verbatim -------

    #[test]
    fn case7_leading_zero_and_signed_operands_survive() {
        let src = ".func f\n        wr      007, -1\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    // -- Case 8: the structural gate -----------------------------------

    #[test]
    fn case8_listing_shaped_line_is_a_raw_line_error() {
        let src = ".func f\n  0004:  21 05 00 00 00  call    0x0005 <goToEnd>\n";
        let err = format_asm(src).unwrap_err();
        assert_eq!(err.kind, AsmErrorKind::RawLine);
        assert_eq!(err.span, Span::new(2, 3, 2, 50));
    }

    #[test]
    fn case8_nothing_formats_when_any_line_is_raw() {
        // The gate is whole-file: even a perfectly good line elsewhere
        // does not partially format.
        let src = ".func f\n        nop\n<stray>\n";
        assert!(format_asm(src).is_err());
    }

    // -- Case 9: unknown mnemonics still format ------------------------

    #[test]
    fn case9_unknown_mnemonic_still_formats() {
        let src = ".func f\n        bogus   1, 2\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    // -- Case 10: blank-run collapse + final newline -------------------

    #[test]
    fn case10_blank_run_collapses_to_one_and_ends_in_one_newline() {
        let src = ".func f local\n        nop\n\n\n\n.func g\n        ret\n";
        let expected = ".func f local\n        nop\n\n.func g\n        ret\n";
        let out = format_asm(src).unwrap();
        assert_eq!(out, expected);
        assert!(out.ends_with('\n') && !out.ends_with("\n\n"));
    }

    #[test]
    fn case10_no_leading_blank_lines() {
        let src = "\n\n.func f\n        nop\n";
        let out = format_asm(src).unwrap();
        assert!(!out.starts_with('\n'));
        assert_eq!(out, ".func f\n        nop\n");
    }

    #[test]
    fn case10_crlf_normalizes_to_lf() {
        let src = ".func f\r\n        nop\r\n";
        let out = format_asm(src).unwrap();
        assert_eq!(out, ".func f\n        nop\n");
        assert!(!out.contains('\r'));
    }

    #[test]
    fn empty_source_formats_to_empty() {
        assert_eq!(format_asm("").unwrap(), "");
    }

    #[test]
    fn blank_only_file_formats_to_empty() {
        // Real blank/whitespace-only lines, not the empty string: every
        // line tokenizes to nothing, so `parse_asm_cst` never produces an
        // item and the print loop never runs — same end result as `""`,
        // pinned here because it goes through a different code path
        // (repeated `pending_blank` folding in `parse_asm_cst`, not the
        // zero-line case).
        let src = "\n\n   \n\t\n";
        let out = format_asm(src).unwrap();
        assert_eq!(out, "");
        assert_eq!(format_asm(&out).unwrap(), out);
    }

    // -- Trailing whitespace on lines (item 4: pinned, not a renderer
    // change — the printer rebuilds every line from CST tokens using the
    // canonical column constants, so whitespace after the last real
    // token on a physical line was never captured into any token in the
    // first place and cannot survive into the output).

    #[test]
    fn trailing_whitespace_on_every_line_shape_formats_clean_and_idempotent() {
        // Same content as `case3_seven_char_field_stays_inline`'s and
        // `case1_doc_example_is_a_fixed_point`'s fixtures, but every
        // physical line — the `.func` header, a plain instruction line,
        // and an own-line comment — carries trailing spaces or a tab.
        let src = ".func f  \nabcdef: nop\t\n        ; note   \n        stop  \n";
        let expected = ".func f\nabcdef: nop\n        ; note\n        stop\n";
        let once = format_asm(src).unwrap();
        assert_eq!(once, expected);
        assert!(
            once.lines().all(|l| l == l.trim_end()),
            "trailing whitespace survived: {once:?}"
        );
        let twice = format_asm(&once).unwrap();
        assert_eq!(twice, once);
    }

    #[test]
    fn trailing_whitespace_after_a_trailing_comment_is_stripped() {
        // The comment token itself captures everything from `;` to the
        // end of the physical line (lexer.rs), so trailing whitespace
        // AFTER the comment text is part of the comment token's text —
        // `print_line`'s `cur.trim_end()` is what drops it, not the
        // lexer failing to capture it.
        let src = ".func f\n        wr      1 ; c   \n";
        let expected = ".func f\n        wr      1               ; c\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    // -- Comment-only file (item 5): no `.func` anywhere, so every
    // own-line comment is TOP_COL regardless of its original indentation
    // (`own_line_comment_col`'s `!seen_func` branch) — pinned separately
    // from the function-body/preamble comment-placement tests above,
    // none of which cover a file with zero functions.

    #[test]
    fn comment_only_file_prints_every_comment_at_top_level_col_0() {
        let src = "; first\n    ; indented, but still top-level\n; last\n";
        let expected = "; first\n; indented, but still top-level\n; last\n";
        let once = format_asm(src).unwrap();
        assert_eq!(once, expected);
        let twice = format_asm(&once).unwrap();
        assert_eq!(twice, once);
    }

    // -- Own-line comment placement (unpinned by the 11 enumerated
    // cases, but part of the printing-rule list — a judgment call
    // documented on `own_line_comment_col`) --------------------------

    #[test]
    fn preamble_comment_before_the_first_func_is_col_0() {
        let src = "; preamble\n.func f\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn comment_inside_a_function_body_is_col_8() {
        let src = ".func f\n        nop\n        ; note\n        ret\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn comment_leading_into_the_next_func_is_col_0() {
        let src = ".func f\n        nop\n; about g\n.func g\n        ret\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn trailing_comment_after_the_last_function_stays_col_8() {
        // No upcoming `.func` to lead into (end of file) — reads as
        // still belonging to the last function's body.
        let src = ".func f\n        nop\n        ; done\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    // -- Case 11: `grid_line`'s long-label rule (unit test lives in
    // `disassembler.rs`'s own test module, next to `grid_line`; see
    // `grid_line_long_label_own_line` there) — this module only
    // exercises the effect through `format_asm`/`disassemble_object`
    // (below), since fmt has no direct call into `grid_line`.

    // -- Self-canonical: format_asm(dis x) == dis x --------------------

    #[test]
    fn self_canonical_over_disassembled_objects() {
        let syntax = test_syntax();
        let programs = [
            ".func f\nL0001:  nop\n        jmp.s   L0001\n        wr      1\n        call    g\n        stop\n",
            "\
.func f
START:  nop
        jmp     START
        wr      1, 2
        call    g
        call    missing
        stop
.func g
        wr      0
        ret
",
        ];
        for src in programs {
            let obj = assemble(&syntax, 0x7E, src, false).unwrap();
            let dis = disassemble_object(&syntax, &obj);
            assert_eq!(
                format_asm(&dis).as_deref(),
                Ok(dis.as_str()),
                "disassembly is not already canonical:\n{dis}"
            );
        }
    }

    // -- Task 5: sections, table directives, and `.rept` blocks --------
    // All exercise the opt-in surface, so they format under caps-on.

    fn caps_all() -> AsmCaps {
        AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        }
    }

    #[test]
    fn section_and_table_directives_normalize_to_the_grid() {
        // `.section` is a column-0 directive (single space, like `.func`);
        // `.row`/`.targets` are label + word + operands on the same grid
        // as an instruction line. A `.row`'s bracketed vector survives as
        // one verbatim operand; `.targets` comma-joins its names.
        let src = "\
.section    tables
T0:   .row  [1, 2]
      .row [1, *]
D0:.targets  A,B
.section code
";
        let expected = "\
.section tables
T0:     .row    [1, 2]
        .row    [1, *]
D0:     .targets A, B
.section code
";
        assert_eq!(format_asm_with(src, caps_all()).unwrap(), expected);
    }

    #[test]
    fn rept_header_and_endr_normalize_but_the_body_prints_verbatim() {
        // Header reconstructed + normalized (`.rept v,0,1` → grid spacing);
        // body line kept AS WRITTEN — odd interior spacing and its comment
        // survive, only trailing whitespace trimmed; `.endr` keeps its own
        // trailing comment, padded to the comment column.
        let src = ".rept v,0,1\n   Linc{v}:    nop      ; step   \n.endr  ; done\n";
        let expected = format!(
            ".rept v, 0, 1\n   Linc{{v}}:    nop      ; step\n.endr{}; done\n",
            " ".repeat(COMMENT_COL - ".endr".len())
        );
        assert_eq!(format_asm_with(src, caps_all()).unwrap(), expected);
    }

    #[test]
    fn rept_body_preserves_comment_and_blank_lines() {
        // Comment items carry no line number, so the body is recovered by
        // physical-line range (bounded by `endr_span`), not by walking
        // body items — a comment-only line and a blank line both survive.
        let src = ".rept v, 0, 0\n        ; a note\n\n        nop\n.endr\n";
        let out = format_asm_with(src, caps_all()).unwrap();
        assert_eq!(out, src);
        assert_eq!(format_asm_with(&out, caps_all()).unwrap(), out);
    }

    #[test]
    fn idempotent_over_all_mechanisms() {
        // Sections, grid-normalized table directives, a `.rept` block with
        // a verbatim (template) body, and a vector operand all in one file:
        // format(format(x)) == format(x).
        let src = "\
.section tables
Tm:     .row    [5, 6]
        .row    [*, *]
.rept v, 1, 2
Tr{v}:  .targets loop
.endr
.section code
.func main
        wr      [1, -, 2]
        tmatch  Tm
        stp
loop:   nop
";
        let once = format_asm_with(src, caps_all()).unwrap();
        let twice = format_asm_with(&once, caps_all()).unwrap();
        assert_eq!(twice, once, "not idempotent:\n{once}");
    }

    #[test]
    fn routine_directive_normalizes_and_is_idempotent() {
        // Tight interior spacing normalizes to the `, ` convention
        // (whitespace-only — the structurally-exact gate admits only
        // canonically spelled values, so no token text changes); the
        // 36-char directive overflows COMMENT_COL, so the trailing
        // comment gets the single overflow space.
        let src = ".routine main,tapes=2,alpha=(3,5) ; sig\n";
        let once = format_asm_with(src, caps_all()).unwrap();
        assert_eq!(once, ".routine main, tapes=2, alpha=(3, 5) ; sig\n");
        assert_eq!(format_asm_with(&once, caps_all()).unwrap(), once);
    }

    #[test]
    fn routine_directive_already_canonical_is_verbatim() {
        let src = ".routine main, tapes=2, alpha=(3, 5)\n.func main\n        stp\n";
        assert_eq!(format_asm_with(src, caps_all()).unwrap(), src);
    }

    #[test]
    fn default_caps_still_refuse_the_new_directive_lines() {
        // Under default caps `.section`/`.row` never shape as their nodes;
        // a `.row` line degrades to a Line with a Junk bracket → a Raw
        // node → the structural gate. This pins that PM-1 fmt is
        // unaffected by the opt-in surface.
        assert!(format_asm(".section tables\nT0: .row [1, 2]\n").is_err());
    }
}
