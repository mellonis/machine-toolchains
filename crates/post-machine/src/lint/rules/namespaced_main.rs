//! `namespaced-main` (docs/lint.md): a function named `main` inside a
//! namespace. Only the un-namespaced top-level `main` is the program
//! entry, and a namespaced `main` is not auto-exported either — it
//! silently becomes an ordinary local function. Almost always a
//! misunderstanding.

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        // Fires only on a directly-namespaced `main`. The `!name.contains('.')`
        // guard is redundant TODAY — nested defs always carry empty `ns`, and a
        // `.` only enters a name via nesting, so `ns`-nonempty and a dotted name
        // are mutually exclusive (condition 1 alone already excludes nested
        // `main`s). It is kept as belt-and-suspenders against future changes to
        // that mangling invariant.
        let is_namespaced_main =
            !f.ns.is_empty() && !f.name.contains('.') && f.name.rsplit("::").next() == Some("main");
        if is_namespaced_main {
            out.push(Diagnostic {
                code: "namespaced-main",
                span: f.name_span,
                message: format!(
                    "'{}' is not the program entry (only top-level 'main' is)",
                    f.name
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
    fn main_inside_a_namespace_fires() {
        let src = "namespace app {\nmain() { right; }\n}\nmain() { @app::main(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "namespaced-main")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "'app::main' is not the program entry (only top-level 'main' is)"
        );
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn top_level_main_and_nested_main_are_clean() {
        // Top-level main IS the entry; a NESTED function named main
        // (dot-mangled `outer.main`) is not the namespaced footgun.
        let src = "main() {\n    @helper();\nhelper() {\n    right;\n}\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "namespaced-main")
        );
    }

    #[test]
    fn namespaced_but_nested_main_is_clean() {
        // A `main` NESTED inside a namespaced function flattens to
        // `app::outer.main` but carries EMPTY `ns` (nested defs are never
        // namespaced) — so it is not the namespaced-entry footgun and must
        // stay clean. Guards the ns-vs-dot mutual-exclusion invariant.
        let src =
            "main() { @app::outer(); }\nnamespace app {\nouter() {\nmain() { right; }\n}\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "namespaced-main")
        );
    }
}
