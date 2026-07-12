//! `leading-zeros` (docs/lint.md): a numeric token written with leading
//! zeros. The lexer parses digit runs straight to `u32`, so `007` and `7`
//! denote the same label while looking unrelated. Token-level — fires on
//! definitions, goto targets, check arms, and call successors alike, and
//! never inside comments (comments produce no tokens).

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lexer::TokenKind;
use crate::lint::{LintContext, span_text};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for tok in ctx.tokens {
        let TokenKind::Number(value, _) = &tok.kind else {
            continue;
        };
        let text = span_text(ctx.source, tok.span());
        if text.len() > 1 && text.starts_with('0') {
            let canonical = value.to_string();
            out.push(Diagnostic {
                code: "leading-zeros",
                span: tok.span(),
                message: format!("'{text}' has leading zeros — write '{canonical}'"),
                fix: Some(Fix {
                    description: format!("rewrite '{text}' as '{canonical}'"),
                    applicability: Applicability::MachineApplicable,
                    edits: vec![Edit {
                        span: tok.span(),
                        replacement: canonical,
                    }],
                }),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::Applicability;

    use crate::lint::{LintOptions, lint};

    #[test]
    fn fires_on_label_definition_and_goto_target() {
        let src = "main() {\n007: right;\n    goto 007;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leading-zeros")
            .collect();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].message, "'007' has leading zeros — write '7'");
        let fix = d[0].fix.as_ref().unwrap();
        assert!(matches!(
            fix.applicability,
            Applicability::MachineApplicable
        ));
        assert_eq!(fix.edits[0].replacement, "7");
        // Span covers exactly the three digits of the definition.
        assert_eq!((d[0].span.start.line, d[0].span.start.col), (2, 1));
        assert_eq!(d[0].span.end.col, 4);
    }

    #[test]
    fn plain_numbers_and_comments_are_clean() {
        let src = "main() {\n7: right; // 007 in a comment is fine\n    goto 7;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "leading-zeros"));
    }

    #[test]
    fn leading_zeros_boundary_cases() {
        // bare `0` is already canonical — NOT flagged.
        let r = lint("main() { 0: right; }", LintOptions::default()).unwrap();
        assert!(r.diagnostics.iter().all(|d| d.code != "leading-zeros"));
        // `00` fires, canonical `0`.
        let r = lint("main() { 00: right; }", LintOptions::default()).unwrap();
        let lz: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.code == "leading-zeros")
            .collect();
        assert_eq!(lz.len(), 1);
        assert_eq!(lz[0].fix.as_ref().unwrap().edits[0].replacement, "0");
        // `10` starts with a non-zero digit — NOT flagged.
        let r = lint("main() { 10: right; }", LintOptions::default()).unwrap();
        assert!(r.diagnostics.iter().all(|d| d.code != "leading-zeros"));
    }
}
