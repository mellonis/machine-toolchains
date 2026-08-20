//! `non-camel-case` (docs/pmt/lint.md): user-owned definition names —
//! functions, namespaces, import bindings — must be lowerCamelCase
//! (`^[a-z][a-zA-Z0-9]*$`, checked by hand: no regex dependency). The
//! project's de-facto house style; the stdlib is uniformly camelCase.
//! Report-only: a rename is a multi-site edit and, for exports, changes
//! the mangled symbol name (link-time ABI). The message carries a
//! mechanically derived suggestion where one exists; a name whose
//! derivation doesn't land inside the convention's alphabet has none,
//! and says so instead.

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

/// The convention's alphabet, named literally in both messages below
/// wherever no rename is offered.
const ASCII_ALPHABET: &str = "ASCII [a-z][a-zA-Z0-9]*";

/// The mechanical rename for `name`, or `None` when [`to_camel`]'s
/// derivation doesn't land inside the convention's alphabet
/// (docs/pmt/lint.md (non-camel-case)) — offering it anyway would send
/// the author in a circle: `Шаг` derives `шаг`, which is no more
/// camelCase than what they wrote.
fn suggestion(name: &str) -> Option<String> {
    let camel = to_camel(name);
    is_lower_camel(&camel).then_some(camel)
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
            let message = match suggestion(last) {
                Some(camel) => {
                    format!("function '{last}' is not camelCase — rename to '{camel}'")
                }
                None => {
                    format!(
                        "function '{last}' is not camelCase — camelCase names are {ASCII_ALPHABET}"
                    )
                }
            };
            out.push(Diagnostic {
                code: "non-camel-case",
                span: f.name_span,
                message,
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
                let message = match suggestion(&segment) {
                    Some(camel) => {
                        format!("namespace '{segment}' is not camelCase — rename to '{camel}'")
                    }
                    None => format!(
                        "namespace '{segment}' is not camelCase — camelCase names are {ASCII_ALPHABET}"
                    ),
                };
                out.push(Diagnostic {
                    code: "non-camel-case",
                    span: f.name_span,
                    message,
                    fix: None,
                });
            }
        }
    }
    // Import bindings: the binding is the user's to rename via `as`.
    for imp in &ctx.ast.imports {
        let binding = imp.binding();
        if !is_lower_camel(binding) {
            let message = match suggestion(binding) {
                Some(camel) => format!(
                    "import binding '{binding}' is not camelCase — alias it: 'use {} as {camel}'",
                    imp.full_path()
                ),
                None => format!(
                    "import binding '{binding}' is not camelCase — \
                     alias it to an {ASCII_ALPHABET} name"
                ),
            };
            out.push(Diagnostic {
                code: "non-camel-case",
                span: imp.span,
                message,
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

    /// The identity case: `шагВперёд` has no `_` and already starts
    /// lowercase, so [`to_camel`] hands back the same string. The
    /// message must not advise a rename to the name the author already
    /// wrote. (The case below covers a derivation that DOES change the
    /// name and still fails to conform.)
    #[test]
    fn non_ascii_function_fires_without_a_tautological_suggestion() {
        let m = messages("export шагВперёд() { right; }\nmain() { @шагВперёд(); }\n");
        assert_eq!(
            m,
            vec![
                "function 'шагВперёд' is not camelCase — camelCase names are ASCII [a-z][a-zA-Z0-9]*"
            ]
        );
    }

    /// The case the old `camel != name` predicate got wrong: `Шаг`'s
    /// derivation IS a different string (`шаг`, lowercased) — so the old
    /// check offered it — but `шаг` is no more camelCase than `Шаг` was,
    /// so suggesting it would send the author in a circle.
    #[test]
    fn non_ascii_function_whose_derivation_changes_but_still_fails_has_no_suggestion() {
        let m = messages("export Шаг() { right; }\nmain() { @Шаг(); }\n");
        assert_eq!(
            m,
            vec!["function 'Шаг' is not camelCase — camelCase names are ASCII [a-z][a-zA-Z0-9]*"]
        );
    }

    #[test]
    fn non_ascii_namespace_fires_without_a_tautological_suggestion() {
        let m = messages(
            "namespace ыHelpers { export inner() { right; } }\nmain() { @ыHelpers::inner(); }\n",
        );
        assert_eq!(
            m,
            vec![
                "namespace 'ыHelpers' is not camelCase — camelCase names are ASCII [a-z][a-zA-Z0-9]*"
            ]
        );
    }

    /// The import site keeps its actionable advice — aliasing IS the fix
    /// for a binding — but stops naming an alias identical to the
    /// binding it would replace.
    #[test]
    fn non_ascii_import_binding_advises_an_alias_without_naming_one() {
        let m = messages("use their::шаг;\nmain() { @шаг(); }\n");
        assert_eq!(
            m,
            vec![
                "import binding 'шаг' is not camelCase — alias it to an ASCII [a-z][a-zA-Z0-9]* name"
            ]
        );
    }
}
