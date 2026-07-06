//! `line-too-long` (docs/lint.md): a line longer than 80 characters
//! (char count). Report-only — rewrapping is the fmt phase's job.

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::lint::LintContext;

const LIMIT: u32 = 80;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
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
    use crate::lint::{LintOptions, lint};

    #[test]
    fn fires_past_80_chars_with_excess_span() {
        // A comment line of exactly 90 chars inside a valid program.
        let long = format!("// {}", "x".repeat(87));
        let src = format!("{long}\nmain() {{ right; }}\n");
        let report = lint(&src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "line-too-long")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "line is 90 characters long (limit 80)");
        assert_eq!((d[0].span.start.line, d[0].span.start.col), (1, 81));
        assert_eq!(d[0].span.end.col, 91); // end-exclusive: col 81..=90
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn exactly_80_chars_is_clean() {
        let edge = format!("// {}", "x".repeat(77)); // 80 chars
        let src = format!("{edge}\nmain() {{ right; }}\n");
        let report = lint(&src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "line-too-long"));
    }

    #[test]
    fn counts_chars_not_bytes() {
        // 80 Cyrillic chars (160 bytes) — must be clean.
        let edge = format!("// {}", "ж".repeat(77));
        let src = format!("{edge}\nmain() {{ right; }}\n");
        let report = lint(&src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "line-too-long"));
    }

    #[test]
    fn multibyte_over_limit_span_end_is_char_counted() {
        // "// " (3 chars) + 78 Cyrillic chars = 81 chars (159 bytes) — over 80.
        let long = format!("// {}", "ж".repeat(78));
        let src = format!("{long}\nmain() {{ right; }}\n");
        let report = lint(&src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "line-too-long")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].span.start.line, d[0].span.start.col), (1, 81));
        assert_eq!(d[0].span.end.col, 82); // char-counted; a byte count would be ~160
    }
}
