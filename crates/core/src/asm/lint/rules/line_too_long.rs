//! `line-too-long` (docs/lint.md): a source line longer than 80
//! characters (char count, not bytes). Report-only — mirrors the
//! `.pmc` rule of the same name: rewrapping is a formatter's job, not
//! lint's.

use crate::asm::lint::AsmLintContext;
use crate::diagnostics::{Diagnostic, Span};

const LIMIT: u32 = 80;

pub(crate) fn check(ctx: &AsmLintContext, out: &mut Vec<Diagnostic>) {
    for (i, text) in ctx.source.lines().enumerate() {
        let line = i as u32 + 1;
        let n = text.chars().count() as u32;
        if n > LIMIT {
            out.push(Diagnostic {
                code: "line-too-long",
                span: Span::new(line, LIMIT + 1, line, n + 1),
                message: format!("line is {n} characters long (limit {LIMIT})"),
                fix: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::cst::parse_asm_cst;
    use crate::asm::lower::lower;
    use crate::asm::syntax::fixture::test_syntax;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let syntax = test_syntax();
        let cst = parse_asm_cst(src);
        let functions = lower(&cst, &syntax, src).unwrap();
        let ctx = AsmLintContext {
            source: src,
            cst: &cst,
            functions: &functions,
            syntax: &syntax,
        };
        let mut out = Vec::new();
        check(&ctx, &mut out);
        out
    }

    #[test]
    fn fires_past_80_chars_with_excess_span() {
        // An own-line comment of exactly 90 characters ahead of a valid
        // program (a comment needs no enclosing function, `lower.rs`
        // skips `AsmItemKind::Comment` entirely).
        let long = format!(";{}", "x".repeat(89)); // 1 + 89 = 90 chars
        assert_eq!(long.chars().count(), 90);
        let src = format!("{long}\n.func f\n        stop\n");
        let d = findings(&src);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "line-too-long");
        assert_eq!(d[0].message, "line is 90 characters long (limit 80)");
        assert_eq!((d[0].span.start.line, d[0].span.start.col), (1, 81));
        assert_eq!(d[0].span.end.col, 91); // end-exclusive: col 81..=90
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn exactly_80_chars_is_clean() {
        let edge = format!(";{}", "x".repeat(79)); // 1 + 79 = 80 chars
        assert_eq!(edge.chars().count(), 80);
        let src = format!("{edge}\n.func f\n        stop\n");
        assert!(findings(&src).is_empty());
    }

    #[test]
    fn counts_chars_not_bytes() {
        // 80 Cyrillic chars (2 bytes each) must stay clean.
        let edge = format!(";{}", "ж".repeat(79));
        assert_eq!(edge.chars().count(), 80);
        let src = format!("{edge}\n.func f\n        stop\n");
        assert!(findings(&src).is_empty());
    }

    #[test]
    fn multibyte_over_limit_span_end_is_char_counted() {
        // 81 Cyrillic chars — over the limit; the span end column must
        // reflect the char count (81+1=82), not a byte count (~161).
        let long = format!(";{}", "ж".repeat(80));
        assert_eq!(long.chars().count(), 81);
        let src = format!("{long}\n.func f\n        stop\n");
        let d = findings(&src);
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].span.start.line, d[0].span.start.col), (1, 81));
        assert_eq!(d[0].span.end.col, 82);
    }
}
