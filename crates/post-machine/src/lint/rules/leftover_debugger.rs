//! `leftover-debugger` (docs/pmt/lint.md): a `debugger` statement in source.
//! Builds strip breakpoints with `--strip-debugger`, and an un-stripped
//! `brk` is an optimizer observability barrier — shipping one also
//! pessimizes `-O1` output. Delete-fix only for a lone, unlabeled
//! `debugger;` statement (anything else risks orphaning labels or
//! mangling a comma group).

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::parser::Item;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        for stmt in &f.body {
            for item in &stmt.items {
                if !matches!(item, Item::Debugger { .. }) {
                    continue;
                }
                let deletable = stmt.labels.is_empty() && stmt.items.len() == 1;
                let fix = deletable.then(|| Fix {
                    description: "remove the 'debugger;' statement".to_string(),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![Edit {
                        span: stmt.span,
                        replacement: String::new(),
                    }],
                });
                out.push(Diagnostic {
                    code: "leftover-debugger",
                    span: stmt.span,
                    message: "leftover 'debugger' statement".to_string(),
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
    fn lone_unlabeled_debugger_fires_with_delete_fix() {
        let src = "main() {\n    debugger;\n    right;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leftover-debugger")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "leftover 'debugger' statement");
        assert!(d[0].fix.is_some());
    }

    #[test]
    fn labeled_or_grouped_debugger_is_report_only() {
        // Labeled: deleting would orphan the `goto 5` reference.
        let labeled = "main() {\n    goto 5;\n5:  debugger;\n    right;\n}\n";
        let report = lint(labeled, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leftover-debugger")
            .collect();
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none());

        // Grouped: the statement carries more than the debugger.
        let grouped = "main() {\n    debugger, right;\n}\n";
        let report = lint(grouped, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leftover-debugger")
            .collect();
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none());
    }
}
