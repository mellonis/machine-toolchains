//! `duplicate-map-source`: a `.map` directive whose `rmap=(…)` or `wmap=(…)`
//! clause lists the same source symbol twice (`rmap=(1->2, 1->3)`). The
//! assembler accepts this silently, and the LAST mapping wins — the emitted
//! object is byte-identical to the one the winning pair alone produces
//! (observed assembler behavior for both clauses; the quickfix test pins it).
//! The earlier pair is therefore dead.
//!
//! # Clause-generic
//!
//! The defect is clause-generic: it is the same last-wins shadowing whether
//! the source symbol repeats in the read map (`rmap`, physical → virtual,
//! docs/formats.md (frame descriptors)) or the write map (`wmap`, virtual →
//! physical). Each clause is checked independently — the two are separate
//! symbol namespaces, so a symbol appearing once in `rmap` and once in `wmap`
//! is not a repeat and does not fire; a `.map` that duplicates in BOTH yields
//! one finding per clause.
//!
//! # What it sees, and the fix
//!
//! The finding spans the LATER (winning) pair, and its fix removes the EARLIER
//! (shadowed) pair together with its trailing comma, so the remaining list
//! still parses. `FramePairCst` keeps no per-pair span, so the spans are
//! reconstructed from the source text within the clause's `(..)` group span,
//! splitting on top-level commas. That group may span more than one physical
//! line — a trailing comma continues a `.map` (docs/formats.md (assembly
//! text)) — so the reconstruction walks the region rather than slicing the
//! line the group opened on, and each pair's span names the line it is on.
//!
//! Top-level `.map` directives only: a `.map` inside a `.rept` body is not
//! scanned (a completeness-only limit — never a wrong finding). The lint runs
//! behind the assemble fatal gate, so every pair here is already well-formed.
//!
//! # Why the fix never meets a comment
//!
//! The fix's deletion span lies strictly inside the clause's `(..)` group,
//! and the interior of a well-formed group can never hold a comment: a `;`
//! before the `)` comments the closer out (the directive is malformed), a
//! trailing comma followed by a comment does not continue the list, and an
//! own-line comment between continuation lines breaks the fold
//! (docs/formats.md (assembly text)). Every such shape is an assemble
//! fatal, and the fatal gate runs before this rule on both routes — so the
//! deletion can never swallow a comment, and no comment-withholding guard
//! is needed here. A comment after the `)` is the one comment a
//! duplicate-carrying `.map` can hold, and it sits outside every deletion
//! span. The tests pin each shape; if the continuation rules ever loosen,
//! they fail and this reasoning must be revisited.

use std::collections::HashMap;

use mtc_core::asm::cst::{AsmItemKind, FrameDirectiveCst, FrameMapCst, FramePairCst};
use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

use crate::lint::tma::TmaLintContext;

pub(crate) fn check(ctx: &TmaLintContext, out: &mut Vec<Diagnostic>) {
    for item in &ctx.cst.items {
        if let AsmItemKind::FrameDirective(FrameDirectiveCst::Map(m)) = &item.kind {
            check_map(ctx.source, m, out);
        }
    }
}

/// Flag repeated source symbols in each of the `.map`'s clauses. `rmap` and
/// `wmap` are independent symbol namespaces, so each is scanned on its own.
fn check_map(source: &str, m: &FrameMapCst, out: &mut Vec<Diagnostic>) {
    check_clause(source, m.rmap.as_deref(), m.rmap_span, out);
    check_clause(source, m.wmap.as_deref(), m.wmap_span, out);
}

/// Flag each pair in one clause whose source symbol an earlier pair in the
/// same clause already mapped. `pairs`/`group` are `Some` iff the clause is
/// present.
fn check_clause(
    source: &str,
    pairs: Option<&[FramePairCst]>,
    group: Option<Span>,
    out: &mut Vec<Diagnostic>,
) {
    let (Some(pairs), Some(group)) = (pairs, group) else {
        return;
    };
    let spans = pair_spans(source, group);
    // A malformed group the assemble gate would already have rejected — bail
    // rather than misalign pairs against reconstructed spans.
    if spans.len() != pairs.len() {
        return;
    }
    let mut last_seen: HashMap<u32, usize> = HashMap::new();
    for (k, pair) in pairs.iter().enumerate() {
        if let Some(&prev) = last_seen.get(&pair.from) {
            // Remove the earlier pair and its trailing comma: from the earlier
            // pair's start up to the next pair's start. A later duplicate at
            // `k > prev` guarantees `prev + 1` exists.
            let remove = Span {
                start: spans[prev].start,
                end: spans[prev + 1].start,
            };
            out.push(Diagnostic {
                code: "duplicate-map-source",
                span: spans[k],
                message: format!(
                    "source symbol {} mapped twice; the last mapping wins",
                    pair.from
                ),
                fix: Some(Fix {
                    description: format!(
                        "remove the shadowed mapping of source symbol {}",
                        pair.from
                    ),
                    applicability: Applicability::MachineApplicable,
                    edits: vec![Edit {
                        span: remove,
                        replacement: String::new(),
                    }],
                }),
            });
        }
        last_seen.insert(pair.from, k);
    }
}

/// One character of the group's interior, carrying the physical position it
/// was written at. Positions travel with the character rather than being
/// recomputed from a column offset, which is what lets the group span more
/// than one line.
struct Cell {
    line: u32,
    col: u32,
    ch: char,
}

/// The per-pair source spans of a `.map` clause's `(..)` group, in list order.
/// Pairs split on top-level commas (a pair never nests), each span trimmed of
/// surrounding whitespace. Fewer than the pair count only on a malformed group
/// (which the assemble gate rejects first).
///
/// The group may span SEVERAL physical lines: a trailing comma continues a
/// `.map` onto the next line (docs/formats.md (assembly text)), which puts the
/// opening and closing parens on different lines. So the interior is collected
/// as positioned characters and split over that, never sliced out of the line
/// the group started on.
fn pair_spans(source: &str, group: Span) -> Vec<Span> {
    let cells = group_cells(source, group);
    let mut spans = Vec::new();
    let mut seg_lo = 0usize;
    for i in 0..cells.len() {
        if cells[i].ch == ',' {
            push_trimmed(&mut spans, &cells, seg_lo, i);
            seg_lo = i + 1;
        }
    }
    push_trimmed(&mut spans, &cells, seg_lo, cells.len());
    spans
}

/// The characters strictly between the group's `(` and `)`, in source order.
/// `group.start` is the `(`; `group.end` is one past the `)`. Bounds come from
/// each line's real length, so nothing can index past a line.
fn group_cells(source: &str, group: Span) -> Vec<Cell> {
    if (group.end.line, group.end.col) <= (group.start.line, group.start.col) {
        return Vec::new();
    }
    let mut cells = Vec::new();
    for (offset, text) in source
        .lines()
        .skip(group.start.line as usize - 1)
        .take((group.end.line - group.start.line + 1) as usize)
        .enumerate()
    {
        let line = group.start.line + offset as u32;
        // Half-open column window `[lo, hi)` of this line's interior: past
        // the `(` on the first line, up to the `)` on the last.
        let lo = if line == group.start.line {
            group.start.col + 1
        } else {
            1
        };
        let hi = if line == group.end.line {
            group.end.col - 1
        } else {
            u32::MAX
        };
        for (j, ch) in text.chars().enumerate() {
            let col = j as u32 + 1;
            if col >= lo && col < hi {
                cells.push(Cell { line, col, ch });
            }
        }
    }
    cells
}

/// Push the trimmed span of the cell range `[lo, hi)`, dropping it if the trim
/// leaves nothing. The line break between two segments contributes no cell at
/// all, so a wrapped list trims exactly like a flat one.
fn push_trimmed(spans: &mut Vec<Span>, cells: &[Cell], lo: usize, hi: usize) {
    let mut a = lo;
    let mut b = hi;
    while a < b && cells[a].ch.is_whitespace() {
        a += 1;
    }
    while b > a && cells[b - 1].ch.is_whitespace() {
        b -= 1;
    }
    if a < b {
        let (first, last) = (&cells[a], &cells[b - 1]);
        spans.push(Span::new(first.line, first.col, last.line, last.col + 1));
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::{Diagnostic, Edit, Pos};

    use crate::asm::assemble;
    use crate::lint::tma::lint_tma;

    /// A frame descriptor whose `.map 0` clause tail is `map_tail` (e.g.
    /// `rmap=(1->2, 1->3)` or `rmap=(…), wmap=(…)`), wired into a `call.m` so
    /// the file assembles. Alphabets are 4-wide so symbols 0..3 are all valid
    /// map values.
    fn program(map_tail: &str) -> String {
        format!(
            "\
.routine main, tapes=4, alpha=(2, 2, 4, 2)
.routine helper, tapes=2, alpha=(4, 4)
.section tables
Fh: .frame  tapes=(2, 0)
    .map    0, {map_tail}
    .exits  done, alt
.section code
.func main
        call.m  helper, Fh
done:   stp
alt:    hlt
.func helper
        wr      [1, -]
        retx    #1
"
        )
    }

    fn diagnostics(src: &str) -> Vec<Diagnostic> {
        lint_tma(src, &[])
            .unwrap()
            .into_iter()
            .filter(|d| d.code == "duplicate-map-source")
            .collect()
    }

    /// Apply one edit to `src` (char-counted (line, col) → byte offset).
    fn apply(src: &str, edit: &Edit) -> String {
        let byte_of = |pos: Pos| {
            let (mut line, mut col) = (1u32, 1u32);
            for (i, c) in src.char_indices() {
                if line == pos.line && col == pos.col {
                    return i;
                }
                if c == '\n' {
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
            }
            src.len()
        };
        let (start, end) = (byte_of(edit.span.start), byte_of(edit.span.end));
        format!("{}{}{}", &src[..start], edit.replacement, &src[end..])
    }

    /// A `.map` whose clause is continued by a trailing comma
    /// (docs/formats.md (assembly text)) puts the group's parens on
    /// different lines. The rule must keep firing, and must span the
    /// physical line each pair is really on.
    #[test]
    fn a_wrapped_group_still_finds_its_duplicate() {
        // The duplicate is the pair that WRAPPED onto line 6.
        let src = program("rmap=(1->2,\n            1->3)");
        let f = diagnostics(&src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!((f[0].span.start.line, f[0].span.start.col), (6, 13));
        assert_eq!((f[0].span.end.line, f[0].span.end.col), (6, 17));

        // The sharp case: the duplicate sits entirely on the FIRST line,
        // and only a later pair wrapped. Slicing the group out of one line
        // used to silence this one too.
        let src = program("rmap=(1->2, 1->3,\n            2->3)");
        let f = diagnostics(&src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!((f[0].span.start.line, f[0].span.start.col), (5, 28));
        assert_eq!((f[0].span.end.line, f[0].span.end.col), (5, 32));
    }

    /// The quickfix stays machine-applicable across the line break. Until
    /// the group could wrap, the `spans.len() != pairs.len()` bail happened
    /// to swallow this case; now the counts agree, so the edit has to be
    /// right rather than merely absent.
    #[test]
    fn the_fix_removes_a_shadowed_mapping_across_the_line_break() {
        assert_fix_no_op("rmap=(1->2,\n            1->3)", "rmap=(1->3)");
    }

    /// A trailing comma followed by a comment does not continue the list
    /// (docs/formats.md (assembly text)) — the directive is malformed and
    /// the assemble fatal gate rejects it before this rule can run. If
    /// this ever starts assembling, a comment can reach a deletion span
    /// and the fix must learn to withhold itself (module head).
    #[test]
    fn a_comma_followed_by_a_comment_is_an_assemble_fatal() {
        let src = program("rmap=(1->2, ; note\n            1->3)");
        assert!(lint_tma(&src, &[]).is_err());
    }

    /// An own-line comment between continuation lines breaks the fold —
    /// same fatal gate, same consequence: the rule never sees the group.
    #[test]
    fn an_own_line_comment_inside_the_group_is_an_assemble_fatal() {
        let src = program("rmap=(1->2,\n    ; note\n            1->3)");
        assert!(lint_tma(&src, &[]).is_err());
    }

    /// A comment after the `)` is the one comment a duplicate-carrying
    /// `.map` can hold, and it sits outside every deletion span: the fix
    /// leaves it byte-for-byte in place.
    #[test]
    fn a_trailing_comment_after_the_group_survives_the_fix() {
        assert_fix_no_op("rmap=(1->2, 1->3) ; note", "rmap=(1->3) ; note");
    }

    #[test]
    fn a_repeated_rmap_source_symbol_fires() {
        let src = program("rmap=(1->2, 1->3)");
        let f = diagnostics(&src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].message,
            "source symbol 1 mapped twice; the last mapping wins"
        );
        // Spans the LATER clause `1->3` (col 28), not the earlier `1->2`
        // (col 22) — `    .map    0, rmap=(1->2, 1->3)`.
        assert_eq!((f[0].span.start.line, f[0].span.start.col), (5, 28));
        // And the fix removes the earlier clause, starting at `1->2` (col 22).
        let edits = &f[0].fix.as_ref().unwrap().edits;
        assert_eq!(edits[0].span.start.col, 22);
    }

    #[test]
    fn a_repeated_wmap_source_symbol_fires() {
        // `wmap` is `w` where `rmap` had `r`, so the column layout is identical.
        let src = program("wmap=(1->2, 1->3)");
        let f = diagnostics(&src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].message,
            "source symbol 1 mapped twice; the last mapping wins"
        );
        assert_eq!((f[0].span.start.line, f[0].span.start.col), (5, 28));
        assert_eq!(f[0].fix.as_ref().unwrap().edits[0].span.start.col, 22);
    }

    #[test]
    fn distinct_source_symbols_are_silent() {
        assert!(diagnostics(&program("rmap=(1->2, 2->3)")).is_empty());
        assert!(diagnostics(&program("wmap=(1->2, 2->3)")).is_empty());
    }

    #[test]
    fn rmap_and_wmap_are_separate_namespaces() {
        // Source 1 appears once in each clause — not a repeat in either, so no
        // cross-clause finding.
        assert!(diagnostics(&program("rmap=(1->2), wmap=(1->3)")).is_empty());
    }

    #[test]
    fn a_duplicate_in_both_clauses_yields_one_finding_per_clause() {
        // `rmap` repeats source 1, `wmap` repeats source 2 — two findings, one
        // per clause, source-ordered (the `rmap` clause is earlier).
        let f = diagnostics(&program("rmap=(1->2, 1->3), wmap=(2->1, 2->0)"));
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(
            f[0].message,
            "source symbol 1 mapped twice; the last mapping wins"
        );
        assert_eq!(
            f[1].message,
            "source symbol 2 mapped twice; the last mapping wins"
        );
        // The two findings span distinct clauses (rmap's later pair precedes
        // wmap's later pair on the line).
        assert!(f[0].span.start.col < f[1].span.start.col, "{f:?}");
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let report = lint_tma(
            &program("rmap=(1->2, 1->3)"),
            &["duplicate-map-source".to_string()],
        )
        .unwrap();
        assert!(report.iter().all(|d| d.code != "duplicate-map-source"));
    }

    #[test]
    fn the_fix_removes_the_shadowed_rmap_clause() {
        assert_fix_no_op("rmap=(1->2, 1->3)", "rmap=(1->3)");
    }

    #[test]
    fn the_fix_removes_the_shadowed_wmap_clause() {
        assert_fix_no_op("wmap=(1->2, 1->3)", "wmap=(1->3)");
    }

    /// apply -> re-lint clean -> byte-identical to hand-removing the clause,
    /// and both assemble to the same object as the original (last wins).
    fn assert_fix_no_op(dup_tail: &str, winner_tail: &str) {
        let original = program(dup_tail);
        let d = diagnostics(&original)
            .into_iter()
            .next()
            .expect("a finding");
        let fix = d.fix.expect("a fix");
        let fixed = apply(&original, &fix.edits[0]);

        // The fix produces exactly what hand-removing the earlier pair gives.
        let hand_removed = program(winner_tail);
        assert_eq!(fixed, hand_removed, "fixed:\n{fixed}");

        // Re-lint: the duplicate is gone.
        assert!(diagnostics(&fixed).is_empty(), "{:?}", diagnostics(&fixed));

        // Last-wins byte proof: the shadowed clause was truly dead.
        let obj_original = assemble(&original, false).unwrap().to_bytes();
        let obj_fixed = assemble(&fixed, false).unwrap().to_bytes();
        assert_eq!(
            obj_original, obj_fixed,
            "removing the shadowed clause is an object no-op"
        );
    }
}
