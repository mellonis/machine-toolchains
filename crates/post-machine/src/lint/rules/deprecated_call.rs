//! `deprecated-call` (docs/lint.md): a call whose resolved target
//! carries a `! [deprecated]` doc line. `flatten` already mangles a
//! call's `Item::Call::name` onto the same fully-qualified form it keys
//! `ctx.docs` by, so this rule is a
//! direct map lookup — no separate resolution walk. Report-only: there
//! is no mechanical fix for "stop calling this function". Recursion is
//! not exempt — a deprecated function calling itself is flagged like any
//! other caller.

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;
use crate::parser::Item;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        for stmt in &f.body {
            for item in &stmt.items {
                let Item::Call {
                    name, name_span, ..
                } = item
                else {
                    continue;
                };
                let Some(doc) = ctx.docs.get(name) else {
                    continue;
                };
                let Some(message) = &doc.deprecated else {
                    continue;
                };
                let suffix = if message.is_empty() {
                    String::new()
                } else {
                    format!(": {message}")
                };
                out.push(Diagnostic {
                    code: "deprecated-call",
                    span: *name_span,
                    message: format!("call to deprecated function '{name}'{suffix}"),
                    fix: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<String> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "deprecated-call")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn direct_call_to_deprecated_function_is_flagged_with_message() {
        let src = "\
?old function.
! [deprecated] use newFn instead.
old() { right; }
main() { @old(); }
";
        let f = findings(src);
        assert_eq!(
            f,
            vec!["call to deprecated function 'old': use newFn instead."]
        );
    }

    #[test]
    fn namespaced_call_to_deprecated_function_is_flagged() {
        let src = "\
namespace ns {
?old function.
! [deprecated] use newFn instead.
export old() { right; }
}
main() { @ns::old(); }
";
        let f = findings(src);
        assert_eq!(
            f,
            vec!["call to deprecated function 'ns::old': use newFn instead."]
        );
    }

    #[test]
    fn messageless_deprecated_attribute_renders_without_the_colon_suffix() {
        let src = "\
?old function.
! [deprecated]
old() { right; }
main() { @old(); }
";
        let f = findings(src);
        assert_eq!(f, vec!["call to deprecated function 'old'"]);
    }

    #[test]
    fn call_to_documented_but_not_deprecated_function_is_not_flagged() {
        let src = "\
?fine function.
fine() { right; }
main() { @fine(); }
";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn self_call_inside_the_deprecated_function_is_flagged() {
        let src = "\
?old function.
! [deprecated] spins forever.
old() { @old(); }
main() { right; }
";
        let f = findings(src);
        assert_eq!(f, vec!["call to deprecated function 'old': spins forever."]);
    }

    #[test]
    fn allow_deprecated_call_suppresses_the_finding() {
        let src = "\
?old function.
! [deprecated] use newFn instead.
old() { right; }
main() { @old(); }
";
        let report = lint(
            src,
            LintOptions {
                allow: vec!["deprecated-call".to_string()],
            },
        )
        .unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "deprecated-call")
        );
    }

    #[test]
    fn e2e_lint_dispatches_deprecated_call() {
        // House pattern (redundant_jump's e2e test): drive the finding
        // through the full `lint()` entry, not just this rule's `check`.
        let src = "\
?old function.
! [deprecated] use newFn instead.
old() { right; }
main() { @old(); }
";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "deprecated-call")
        );
    }
}
