//! `unused-label` (docs/lint.md): a label nothing in its function
//! references — no goto, no check arm, no command successor. Function-
//! scoped, the same scope as label resolution. The delete-fix is gated:
//! an unused label may be evidence of a jump the author forgot to write.

use std::collections::HashSet;

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::parser::{CheckArm, Item, Successor};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        let mut referenced: HashSet<u32> = HashSet::new();
        for stmt in &f.body {
            for item in &stmt.items {
                match item {
                    Item::Goto { label, .. } => {
                        referenced.insert(*label);
                    }
                    Item::Check { marked, blank, .. } => {
                        for arm in [marked, blank] {
                            if let CheckArm::Label(n) = arm {
                                referenced.insert(*n);
                            }
                        }
                    }
                    Item::Builtin { succ, .. } | Item::Call { succ, .. } => {
                        if let Successor::Label(n) = succ {
                            referenced.insert(*n);
                        }
                    }
                    Item::Halt { .. } | Item::Debugger { .. } => {}
                }
            }
        }
        for stmt in &f.body {
            for label in &stmt.labels {
                if !referenced.contains(&label.value) {
                    out.push(Diagnostic {
                        code: "unused-label",
                        span: label.span,
                        message: format!(
                            "label {} is never referenced (function '{}')",
                            label.value, f.name
                        ),
                        fix: Some(Fix {
                            description: format!("remove the label prefix '{}:'", label.value),
                            applicability: Applicability::MaybeIncorrect,
                            edits: vec![Edit {
                                span: label.span,
                                replacement: String::new(),
                            }],
                        }),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::Applicability;

    use crate::lint::{LintOptions, lint};

    #[test]
    fn unreferenced_label_fires_with_qualified_function_name() {
        let src = "namespace api {\nhelper() {\n5: right;\n}\n}\nmain() { @api::helper(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "unused-label")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "label 5 is never referenced (function 'api::helper')"
        );
        let fix = d[0].fix.as_ref().unwrap();
        assert!(matches!(fix.applicability, Applicability::MaybeIncorrect));
        assert_eq!(fix.edits[0].replacement, "");
        // The label span covers `5:` — number start to colon end.
        assert_eq!((d[0].span.start.line, d[0].span.start.col), (3, 1));
        assert_eq!(d[0].span.end.col, 3);
    }

    #[test]
    fn referenced_labels_are_clean() {
        // goto, check arm, and successor references all count.
        let src = "main() {\n1: right(2);\n2: check(1, 3);\n3: goto 1;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "unused-label"));
    }

    #[test]
    fn self_loop_label_on_single_statement_body_is_used() {
        // The single-statement sibling rule is subsumed: a referenced
        // label on the only statement is a self-loop, not a finding.
        let src = "main() {\n1: check(1, !);\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "unused-label"));
    }
}
