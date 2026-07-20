//! `shadowed-import` (docs/pmt/lint.md): a function definition whose name
//! outranks an import binding of the same bare name in the SAME scope —
//! legal (definitions always win), but a bare `@name()` call silently
//! hits the local function while the `use` line suggests the external.
//! Cross-scope shadowing (inner over outer) is legal layering: not flagged.

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for (scope, bindings) in &ctx.scopes.bindings {
        let Some(defs) = ctx.scopes.defs.get(scope) else {
            continue;
        };
        for (bare, (_idx, full_path)) in bindings {
            let Some(full_def) = defs.get(bare) else {
                continue;
            };
            // Anchor at the shadowing definition's name token.
            let Some(f) = ctx.ast.functions.iter().find(|f| &f.name == full_def) else {
                continue;
            };
            out.push(Diagnostic {
                code: "shadowed-import",
                span: f.name_span,
                message: format!(
                    "function '{bare}' shadows the import of '{full_path}' — bare calls resolve to the local definition"
                ),
                fix: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    #[test]
    fn definition_outranking_same_scope_import_fires() {
        let src = "use std::goToEnd;\ngoToEnd() { right; }\nmain() { @goToEnd(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "shadowed-import")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "function 'goToEnd' shadows the import of 'std::goToEnd' — bare calls resolve to the local definition"
        );
        assert!(d[0].fix.is_none());
        // Anchored at the shadowing definition, line 2.
        assert_eq!(d[0].span.start.line, 2);
    }

    #[test]
    fn cross_scope_shadowing_is_legal_layering() {
        // File-level import, namespace-level definition: inner shadows
        // outer by design — not flagged (same-scope only).
        let src = "use std::goToEnd;\nnamespace inner {\ngoToEnd() { right; }\n}\nmain() { @goToEnd(); @inner::goToEnd(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "shadowed-import")
        );
    }
}
