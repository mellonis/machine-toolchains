//! `duplicate-map-source`: a `.map` directive whose `rmap=(…)` clause lists the
//! same source symbol twice (`rmap=(1->2, 1->3)`). The assembler accepts this
//! silently, and the LAST mapping wins — the emitted object is byte-identical
//! to the one the winning pair alone produces (observed assembler behavior; the
//! quickfix test pins it). The earlier pair is therefore dead.
//!
//! # What it sees, and the fix
//!
//! Scoped to the `rmap` clause (physical → virtual, docs/formats.md (frame
//! descriptors)); `wmap` is out of scope — its last-wins behavior is not
//! asserted here. The finding spans the LATER (winning) pair, and its fix
//! removes the EARLIER (shadowed) pair together with its trailing comma, so the
//! remaining list still parses. `FramePairCst` keeps no per-pair span, so the
//! spans are reconstructed from the source text within the clause's `(..)`
//! group span (the clause is a single line; pairs split on top-level commas).
//!
//! Top-level `.map` directives only: a `.map` inside a `.rept` body is not
//! scanned (a completeness-only limit — never a wrong finding). The lint runs
//! behind the assemble fatal gate, so every pair here is already well-formed.

use std::collections::HashMap;

use mtc_core::asm::cst::{AsmItemKind, FrameDirectiveCst, FrameMapCst};
use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

use crate::lint::tma::TmaLintContext;

pub(crate) fn check(ctx: &TmaLintContext, out: &mut Vec<Diagnostic>) {
    for item in &ctx.cst.items {
        if let AsmItemKind::FrameDirective(FrameDirectiveCst::Map(m)) = &item.kind {
            check_map(ctx.source, m, out);
        }
    }
}

/// Flag each `rmap` pair whose source symbol an earlier pair already mapped.
fn check_map(source: &str, m: &FrameMapCst, out: &mut Vec<Diagnostic>) {
    let (Some(pairs), Some(group)) = (&m.rmap, m.rmap_span) else {
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

/// The per-pair source spans of a `.map` clause's `(..)` group, in list order.
/// The group is on one line; pairs split on top-level commas (a pair never
/// nests), each span trimmed of surrounding whitespace. Fewer than the pair
/// count only on a malformed group (which the assemble gate rejects first).
fn pair_spans(source: &str, group: Span) -> Vec<Span> {
    let line_no = group.start.line;
    let Some(line) = source.lines().nth(line_no as usize - 1) else {
        return Vec::new();
    };
    let chars: Vec<char> = line.chars().collect();
    // The group covers `(..)`: `(` at 0-based `start.col - 1`, `)` at
    // `end.col - 2` (end.col is one past the `)`).
    let lparen0 = group.start.col as usize - 1;
    let rparen0 = group.end.col as usize - 2;
    if rparen0 <= lparen0 || rparen0 > chars.len() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut seg_lo = lparen0 + 1;
    for i in (lparen0 + 1)..rparen0 {
        if chars[i] == ',' {
            push_trimmed(&mut spans, &chars, line_no, seg_lo, i);
            seg_lo = i + 1;
        }
    }
    push_trimmed(&mut spans, &chars, line_no, seg_lo, rparen0);
    spans
}

/// Push the trimmed span of the char range `[lo, hi)` (0-based indices into
/// `chars`) as a single-line span, dropping it if the trim leaves nothing. A
/// 0-based char index `j` maps to 1-based column `j + 1`.
fn push_trimmed(spans: &mut Vec<Span>, chars: &[char], line: u32, lo: usize, hi: usize) {
    let mut a = lo;
    let mut b = hi;
    while a < b && chars[a].is_whitespace() {
        a += 1;
    }
    while b > a && chars[b - 1].is_whitespace() {
        b -= 1;
    }
    if a < b {
        spans.push(Span::new(line, (a + 1) as u32, line, (b + 1) as u32));
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::{Diagnostic, Edit, Pos};

    use crate::asm::assemble;
    use crate::lint::tma::lint_tma;

    /// A frame descriptor whose `.map 0` rmap clause is `RMAP`, wired into a
    /// `call.m` so the file assembles. Alphabets are 4-wide so symbols 0..3 are
    /// all valid map values.
    fn program(rmap: &str) -> String {
        format!(
            "\
.routine main, tapes=4, alpha=(2, 2, 4, 2)
.routine helper, tapes=2, alpha=(4, 4)
.section tables
Fh: .frame  tapes=(2, 0)
    .map    0, rmap=({rmap})
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

    #[test]
    fn a_repeated_source_symbol_fires() {
        let src = program("1->2, 1->3");
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
    fn distinct_source_symbols_are_silent() {
        assert!(diagnostics(&program("1->2, 2->3")).is_empty());
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let report = lint_tma(
            &program("1->2, 1->3"),
            &["duplicate-map-source".to_string()],
        )
        .unwrap();
        assert!(report.iter().all(|d| d.code != "duplicate-map-source"));
    }

    #[test]
    fn the_fix_removes_the_shadowed_clause() {
        // apply -> re-lint clean -> byte-identical to hand-removing the clause,
        // and both assemble to the same object as the original (last wins).
        let original = program("1->2, 1->3");
        let d = diagnostics(&original)
            .into_iter()
            .next()
            .expect("a finding");
        let fix = d.fix.expect("a fix");
        let fixed = apply(&original, &fix.edits[0]);

        // The fix produces exactly what hand-removing `1->2, ` gives.
        let hand_removed = program("1->3");
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
