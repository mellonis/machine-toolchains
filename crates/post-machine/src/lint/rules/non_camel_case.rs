//! `non-camel-case` (docs/pmt/lint.md): user-owned definition names —
//! functions, namespaces, import bindings — must be lowerCamelCase
//! (`^[a-z][a-zA-Z0-9]*$`, checked by hand: no regex dependency). The
//! project's de-facto house style; the stdlib is uniformly camelCase.
//! Report-only: a rename is a multi-site edit and, for exports, changes
//! the mangled symbol name (link-time ABI). The message carries a
//! mechanically derived suggestion instead.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;

/// `^[a-z][a-zA-Z0-9]*$` by hand.
fn is_lower_camel(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// Mechanical camelCase derivation: drop `_`, capitalize the char after
/// each dropped `_`, lowercase the first char.
pub(super) fn to_camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper_next = false;
    for c in name.chars() {
        if c == '_' {
            upper_next = true;
            continue;
        }
        if out.is_empty() {
            out.extend(c.to_lowercase());
        } else if upper_next {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        upper_next = false;
    }
    out
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    // Functions: judge the user-authored final segment of the flattened
    // name (`std::api.helper` → `helper`; plain `api` → `api`).
    for f in &ctx.ast.functions {
        let last = f
            .name
            .rsplit("::")
            .next()
            .and_then(|s| s.rsplit('.').next())
            .expect("rsplit always yields at least one item");
        if !is_lower_camel(last) {
            out.push(Diagnostic {
                code: "non-camel-case",
                span: f.name_span,
                message: format!(
                    "function '{last}' is not camelCase — rename to '{}'",
                    to_camel(last)
                ),
                fix: None,
            });
        }
    }
    // Namespace segments, once per unique path prefix. The flattened AST
    // retains no namespace-name spans, so the finding anchors at the
    // first function defined under that namespace.
    let mut seen_ns: HashSet<Vec<String>> = HashSet::new();
    for f in &ctx.ast.functions {
        for depth in 1..=f.ns.len() {
            let prefix = f.ns[..depth].to_vec();
            let segment = prefix.last().expect("depth >= 1").clone();
            if !seen_ns.insert(prefix) {
                continue;
            }
            if !is_lower_camel(&segment) {
                out.push(Diagnostic {
                    code: "non-camel-case",
                    span: f.name_span,
                    message: format!(
                        "namespace '{segment}' is not camelCase — rename to '{}'",
                        to_camel(&segment)
                    ),
                    fix: None,
                });
            }
        }
    }
    // Import bindings: the binding is the user's to rename via `as`.
    for imp in &ctx.ast.imports {
        let binding = imp.binding();
        if !is_lower_camel(binding) {
            out.push(Diagnostic {
                code: "non-camel-case",
                span: imp.span,
                message: format!(
                    "import binding '{binding}' is not camelCase — alias it: 'use {} as {}'",
                    imp.full_path(),
                    to_camel(binding)
                ),
                fix: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn messages(src: &str) -> Vec<String> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "non-camel-case")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn snake_case_function_fires_with_suggestion() {
        let m = messages("export sum_bits() { right; }\nmain() { @sum_bits(); }\n");
        assert_eq!(
            m,
            vec!["function 'sum_bits' is not camelCase — rename to 'sumBits'"]
        );
    }

    #[test]
    fn violating_import_binding_suggests_an_alias() {
        let m = messages("use their::do_thing;\nmain() { @do_thing(); }\n");
        assert_eq!(
            m,
            vec![
                "import binding 'do_thing' is not camelCase — alias it: 'use their::do_thing as doThing'"
            ]
        );
    }

    #[test]
    fn violating_namespace_segment_fires_once() {
        let src = "namespace my_ns {\nexport a() { right; }\nexport b() { right; }\n}\nmain() { @my_ns::a(); @my_ns::b(); }\n";
        let m = messages(src);
        assert_eq!(
            m,
            vec!["namespace 'my_ns' is not camelCase — rename to 'myNs'"]
        );
    }

    #[test]
    fn camel_case_names_are_clean() {
        let m = messages("main() { @goToEnd(); }\ngoToEnd() { right; }\n");
        assert!(m.is_empty());
    }

    #[test]
    fn to_camel_derivations() {
        use super::to_camel;
        assert_eq!(to_camel("sum_bits"), "sumBits");
        assert_eq!(to_camel("Foo"), "foo");
        assert_eq!(to_camel("do_thing_2"), "doThing2");
    }
}
