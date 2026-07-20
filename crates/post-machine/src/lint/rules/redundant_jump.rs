//! `redundant-jump-to-next` (docs/pmt/lint.md): a `goto N;` statement or a
//! `(N)` successor whose target labels the lexically next statement —
//! fall-through is identical (codegen's layout even elides such jumps).

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::parser::{Item, Successor};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        for window in f.body.windows(2) {
            let (stmt, next) = (&window[0], &window[1]);
            let next_has = |n: u32| next.labels.iter().any(|l| l.value == n);
            // `goto` is never grouped (parser rule), so it is the only item.
            let last = stmt.items.last().expect("parser: statements have items");
            match last {
                Item::Goto { label, .. } if next_has(*label) => {
                    let fix = stmt.labels.is_empty().then(|| Fix {
                        description: format!("remove the redundant 'goto {label};'"),
                        applicability: Applicability::MaybeIncorrect,
                        edits: vec![Edit {
                            span: stmt.span,
                            replacement: String::new(),
                        }],
                    });
                    out.push(Diagnostic {
                        code: "redundant-jump-to-next",
                        span: stmt.span,
                        message: format!(
                            "goto {label} targets the next statement — fall-through is identical"
                        ),
                        fix,
                    });
                }
                Item::Builtin {
                    succ, succ_span, ..
                }
                | Item::Call {
                    succ, succ_span, ..
                } => {
                    if let (Successor::Label(n), Some(sspan)) = (succ, succ_span)
                        && next_has(*n)
                    {
                        out.push(Diagnostic {
                            code: "redundant-jump-to-next",
                            span: *sspan,
                            message: format!(
                                "successor ({n}) targets the next statement — drop it"
                            ),
                            fix: Some(Fix {
                                description: format!("remove the redundant successor ({n})"),
                                applicability: Applicability::MaybeIncorrect,
                                edits: vec![Edit {
                                    span: *sspan,
                                    replacement: String::new(),
                                }],
                            }),
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<(String, bool)> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "redundant-jump-to-next")
            .map(|d| (d.message, d.fix.is_some()))
            .collect()
    }

    #[test]
    fn goto_to_lexically_next_statement_fires_with_fix() {
        let src = "main() {\n    goto 5;\n5:  right;\n}\n";
        let f = findings(src);
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].0,
            "goto 5 targets the next statement — fall-through is identical"
        );
        assert!(f[0].1, "unlabeled goto statement gets the delete-fix");
    }

    #[test]
    fn labeled_goto_statement_is_report_only() {
        // Deleting `3: goto 5;` would orphan the reference to 3.
        let src = "main() {\n    check(3, 5);\n3:  goto 5;\n5:  right;\n}\n";
        let f = findings(src);
        assert_eq!(f.len(), 1);
        assert!(!f[0].1, "labeled statement must not carry a fix");
    }

    #[test]
    fn successor_to_next_statement_fires_and_deletes_only_the_successor() {
        let src = "main() {\n    right(5);\n5:  left;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "redundant-jump-to-next")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "successor (5) targets the next statement — drop it"
        );
        let fix = d[0].fix.as_ref().unwrap();
        // The edit deletes the whole `(5)` successor group (leaves `right;`).
        assert_eq!(fix.edits[0].replacement, "");
        assert_eq!(
            (fix.edits[0].span.start.line, fix.edits[0].span.start.col),
            (2, 10)
        );
    }

    #[test]
    fn jump_past_the_next_statement_is_clean() {
        let src = "main() {\n    goto 6;\n5:  right;\n6:  left, check(5, !);\n}\n";
        assert!(findings(src).is_empty());
    }
}
