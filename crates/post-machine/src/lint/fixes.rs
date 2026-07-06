//! Fix application (docs/lint.md, docs/cli.md): one batch pass against
//! original-source coordinates — no re-analysis between edits. A `Fix`'s
//! edits apply atomically; a fix overlapping an already-kept fix is
//! skipped whole and counted. The CLI re-lints the fixed source and
//! reports from the re-run (cascades are reported, not looped).

use mtc_core::diagnostics::{Diagnostic, Pos};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixOutcome {
    pub fixed_source: String,
    pub applied: usize,
    pub skipped: usize,
}

/// Char-counted (line, col) → byte offset; end-of-input if past the end.
fn byte_offset(source: &str, pos: Pos) -> usize {
    let (mut line, mut col) = (1u32, 1u32);
    for (i, c) in source.char_indices() {
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
    source.len()
}

pub fn apply_fixes(source: &str, diagnostics: &[Diagnostic]) -> FixOutcome {
    // Phase 1 — plan: keep each fix whose edits overlap no kept edit.
    let mut kept_edits: Vec<(usize, usize, String)> = Vec::new();
    let mut kept_ranges: Vec<(usize, usize)> = Vec::new();
    let (mut applied, mut skipped) = (0usize, 0usize);
    for d in diagnostics {
        let Some(fix) = &d.fix else { continue };
        let ranges: Vec<(usize, usize)> = fix
            .edits
            .iter()
            .map(|e| {
                (
                    byte_offset(source, e.span.start),
                    byte_offset(source, e.span.end),
                )
            })
            .collect();
        let overlaps = ranges
            .iter()
            .any(|&(s, e)| kept_ranges.iter().any(|&(ks, ke)| s < ke && ks < e));
        if overlaps {
            skipped += 1;
            continue;
        }
        for (&(s, e), edit) in ranges.iter().zip(&fix.edits) {
            kept_edits.push((s, e, edit.replacement.clone()));
        }
        kept_ranges.extend(ranges);
        applied += 1;
    }
    // Phase 2 — apply bottom-up: descending start keeps every pending
    // (lower) offset valid; kept edits are pairwise disjoint by phase 1.
    kept_edits.sort_by_key(|&(s, _, _)| std::cmp::Reverse(s));
    let mut fixed_source = source.to_string();
    for (s, e, rep) in kept_edits {
        fixed_source.replace_range(s..e, &rep);
    }
    FixOutcome {
        fixed_source,
        applied,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

    use super::*;

    fn fix_diag(span: Span, replacement: &str) -> Diagnostic {
        Diagnostic {
            code: "test",
            span,
            message: "test".into(),
            fix: Some(Fix {
                description: "test".into(),
                applicability: Applicability::MachineApplicable,
                edits: vec![Edit {
                    span,
                    replacement: replacement.into(),
                }],
            }),
        }
    }

    #[test]
    fn deletes_and_replaces() {
        let src = "5: right;\n";
        // Delete the `5:` prefix (cols 1..3).
        let out = apply_fixes(src, &[fix_diag(Span::new(1, 1, 1, 3), "")]);
        assert_eq!(out.fixed_source, " right;\n");
        assert_eq!((out.applied, out.skipped), (1, 0));

        // Replace `right` with `left` (cols 4..9).
        let out = apply_fixes(src, &[fix_diag(Span::new(1, 4, 1, 9), "left")]);
        assert_eq!(out.fixed_source, "5: left;\n");
    }

    #[test]
    fn two_disjoint_fixes_apply_bottom_up() {
        let src = "007: right;\ngoto 007;\n";
        let fixes = [
            fix_diag(Span::new(1, 1, 1, 4), "7"),
            fix_diag(Span::new(2, 6, 2, 9), "7"),
        ];
        let out = apply_fixes(src, &fixes);
        assert_eq!(out.fixed_source, "7: right;\ngoto 7;\n");
        assert_eq!((out.applied, out.skipped), (2, 0));
    }

    #[test]
    fn overlapping_fix_is_skipped_whole() {
        let src = "abcdef\n";
        let fixes = [
            fix_diag(Span::new(1, 1, 1, 4), "X"), // abc -> X
            fix_diag(Span::new(1, 3, 1, 6), "Y"), // cde overlaps -> skipped
        ];
        let out = apply_fixes(src, &fixes);
        assert_eq!(out.fixed_source, "Xdef\n");
        assert_eq!((out.applied, out.skipped), (1, 1));
    }

    #[test]
    fn diagnostics_without_fixes_are_ignored() {
        let src = "x\n";
        let d = Diagnostic {
            code: "test",
            span: Span::new(1, 1, 1, 2),
            message: "no fix".into(),
            fix: None,
        };
        let out = apply_fixes(src, &[d]);
        assert_eq!(out.fixed_source, src);
        assert_eq!((out.applied, out.skipped), (0, 0));
    }

    #[test]
    fn char_positions_survive_unicode() {
        // Cyrillic chars are 2 bytes each; spans are char-counted.
        let src = "жж 007;\n";
        let out = apply_fixes(src, &[fix_diag(Span::new(1, 4, 1, 7), "7")]);
        assert_eq!(out.fixed_source, "жж 7;\n");
    }
}
