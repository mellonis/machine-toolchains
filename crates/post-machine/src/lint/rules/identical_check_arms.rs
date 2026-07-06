//! `identical-check-arms` (docs/lint.md): `check(N, N)` — both arms land
//! in the same place, so the branch is unconditional; `goto N` was meant
//! or one arm is a typo. `check(!, !)` is EXEMPT: it is the language's
//! only pure mid-function return (there is no `return` keyword and `(!)`
//! successors need a carrier action).

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::parser::{CheckArm, Item};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        for stmt in &f.body {
            for item in &stmt.items {
                let Item::Check {
                    marked: CheckArm::Label(a),
                    blank: CheckArm::Label(b),
                    span,
                    ..
                } = item
                else {
                    continue; // different arms, or `!` arms (exempt)
                };
                if a != b {
                    continue;
                }
                // Standalone statement → replace with `goto N` (labels stay
                // attached — this is a replacement). Group-final → report
                // only: `goto` is barred from comma groups.
                let fix = (stmt.items.len() == 1).then(|| Fix {
                    description: format!("replace 'check({a}, {a})' with 'goto {a}'"),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![Edit {
                        span: *span,
                        replacement: format!("goto {a}"),
                    }],
                });
                out.push(Diagnostic {
                    code: "identical-check-arms",
                    span: *span,
                    message: format!("both check arms target {a} — replace with 'goto {a}'"),
                    fix,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    #[test]
    fn identical_label_arms_fire_with_goto_replacement() {
        let src = "main() {\n5:  check(5, 5);\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "identical-check-arms")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "both check arms target 5 — replace with 'goto 5'"
        );
        let fix = d[0].fix.as_ref().unwrap();
        // Replacement, not deletion — statement labels stay attached.
        assert_eq!(fix.edits[0].replacement, "goto 5");
    }

    #[test]
    fn group_final_check_is_report_only() {
        // `goto` is barred from comma groups — no legal substitution.
        let src = "main() {\n5:  right, check(5, 5);\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "identical-check-arms")
            .collect();
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn identical_bang_arms_are_exempt() {
        // check(!, !) is the language's only pure mid-function return —
        // there is no `return` keyword; legitimate, nothing to suggest.
        let src = "main() {\n1:  check(!, !);\n    goto 1;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "identical-check-arms")
        );
    }

    #[test]
    fn different_arms_are_clean() {
        let src = "main() {\n1: right;\n2: check(1, !);\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "identical-check-arms")
        );
    }
}
