//! `confusable-names` (docs/lint.md): two definitions or bindings in the
//! SAME scope whose names differ only under a confusability
//! normalization — lowercase, strip `_`, map `1→l`, `i→l`, `0→o`.
//! Deterministic; one finding per pair, reported at the later
//! definition, naming the earlier one.

use std::collections::HashMap;

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::lint::LintContext;

fn normalize(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|&c| c != '_')
        .map(|c| match c {
            '1' | 'i' => 'l',
            '0' => 'o',
            other => other,
        })
        .collect()
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    // Scope → the names visible in it: definitions and import bindings.
    let mut scopes: HashMap<&[String], Vec<(&str, Span)>> = HashMap::new();
    for (scope, defs) in &ctx.scopes.defs {
        for (bare, full) in defs {
            if let Some(f) = ctx.ast.functions.iter().find(|f| &f.name == full) {
                scopes.entry(scope).or_default().push((bare, f.name_span));
            }
        }
    }
    for (scope, bindings) in &ctx.scopes.bindings {
        for (bare, (idx, _path)) in bindings {
            if let Some(imp) = ctx.ast.imports.get(*idx) {
                scopes.entry(scope).or_default().push((bare, imp.span));
            }
        }
    }
    for names in scopes.values_mut() {
        names.sort_by_key(|(_, span)| span.start); // source order
        let mut by_norm: HashMap<String, (&str, Span)> = HashMap::new();
        for &(raw, span) in names.iter() {
            let norm = normalize(raw);
            match by_norm.get(&norm) {
                Some(&(first_raw, first_span)) if first_raw != raw => {
                    out.push(Diagnostic {
                        code: "confusable-names",
                        span,
                        message: format!(
                            "'{raw}' is confusable with '{first_raw}' (defined at line {})",
                            first_span.start.line
                        ),
                        fix: None,
                    });
                }
                Some(_) => {} // same raw name (e.g. def + its own re-listing)
                None => {
                    by_norm.insert(norm, (raw, span));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    #[test]
    fn confusable_pair_reports_at_the_later_definition() {
        let src =
            "sumBits() { right; }\nsum_bits() { left; }\nmain() { @sumBits(); @sum_bits(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "confusable-names")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "'sum_bits' is confusable with 'sumBits' (defined at line 1)"
        );
        assert_eq!(d[0].span.start.line, 2);
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn digit_letter_confusables_fire() {
        // fool vs foo1: '1' normalizes to 'l'.
        let src = "fool() { right; }\nfoo1() { left; }\nmain() { @fool(); @foo1(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|d| d.code == "confusable-names")
                .count(),
            1
        );
    }

    #[test]
    fn distinct_names_and_cross_scope_pairs_are_clean() {
        let src = "namespace a {\nexport doIt() { right; }\n}\ndoIt() { left; }\nmain() { @doIt(); @a::doIt(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "confusable-names")
        );
    }

    #[test]
    fn confusable_but_cross_scope_is_not_flagged() {
        // `a::doIt` (namespace a) and top-level `do_it` normalize to the
        // same "doit" but are DIFFERENT raw names in DIFFERENT scopes —
        // confusable-names is same-scope-only, so it must NOT fire.
        // (`do_it` still trips non-camel-case; we filter for the code we mean.)
        let src = "namespace a {\nexport doIt() { right; }\n}\ndo_it() { left; }\nmain() { @a::doIt(); @do_it(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "confusable-names")
        );
    }
}
