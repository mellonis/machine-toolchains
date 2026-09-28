//! `unused-set`: a `set` declaration (docs/tmt/language.md (glyph sets))
//! no alphabet body, contract clause or pattern cell references.
//! Export-independent, mirroring `unused-alphabet` and `unused-map`: the
//! rule sees this unit only, so an exported set that another unit imports
//! through this unit's header and uses is still reported, and a library
//! silences it with the allow list rather than applying the deletion fix
//! (docs/tmt/lint.md (unused-set)). Detected source-level over `Resolved`.
//!
//! Usage is decided by `Resolved.set_refs`, the one recording built for
//! exactly this purpose: `SetScope::members` inserts a referenced set's
//! mangled name into it at each of the three sites — an alphabet body (via
//! `resolve_alphabet_glyphs`), a contract clause, and a pattern cell (via
//! `resolve_pattern_sets`) — so this rule is a single membership test over
//! `Resolved.sets`, the `unused-alphabet` shape (one uniform source), not
//! `unused-map`'s three hand-rolled walks: that recording exists precisely
//! so this rule does not hand-roll a fourth.
//!
//! `Resolved.sets` holds only this unit's OWN declarations — a set reached
//! only through a declarations module has no local declaration to delete
//! and is never a candidate here.
//!
//! The fix deletes the whole declaration, including any leading doc/
//! attention run — an orphaned `?`/`!` run is a parse error, so the doc
//! goes with the set it documents, mirroring every sibling `unused-*` rule
//! in this crate.

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::lint::rules::spans::decl_span;
use crate::syntax::SetDeclView;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for (name, set) in &ctx.resolved.sets {
        if !ctx.resolved.set_refs.contains(name.as_str()) {
            let fix = decl_span::<SetDeclView>(ctx, set.name_span).map(|span| Fix {
                description: format!("delete the unused set `{name}`"),
                applicability: Applicability::MaybeIncorrect,
                edits: vec![Edit {
                    span,
                    replacement: String::new(),
                }],
            });
            out.push(Diagnostic {
                code: "unused-set",
                span: set.name_span,
                message: format!("set `{name}` is never used by any alphabet, contract or pattern"),
                fix,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<mtc_core::diagnostics::Diagnostic> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "unused-set")
            .collect()
    }

    /// A set referenced from an alphabet body ONLY (never a contract clause
    /// or a pattern cell) is a use — silent. Mutation: not recording a
    /// reference at the alphabet-body expansion site
    /// (`resolve_alphabet_glyphs`'s call into `SetScope::members`), which
    /// would make this set look unreferenced and fire.
    #[test]
    fn a_set_referenced_only_from_an_alphabet_body_is_quiet() {
        let src = "\
set s { '_', 'x' }
alphabet ab { s }
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    /// A set referenced from a contract clause ONLY (the alphabet body
    /// spells its own glyphs out, never naming the set) is a use — silent.
    /// Mutation: not recording at the clause site (the `enters`/`leaves`
    /// element-list road's own call into `SetScope::members`).
    #[test]
    fn a_set_referenced_only_from_a_contract_clause_is_quiet() {
        let src = "\
set s { '_', 'x' }
alphabet ab { '_', 'x' }
routine r(tape t: ab enters { s }) {
  entry state e { [*] -> return; }
}
machine {
  tape t: ab;
  entry state go { [*] -> call r(t = t) then stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    /// A set referenced from a pattern cell ONLY (`[s] -> …`) is a use —
    /// silent. Mutation: not recording at the pattern-cell site
    /// (`resolve_pattern_sets`'s own call into `SetScope::members`).
    #[test]
    fn a_set_referenced_only_from_a_pattern_cell_is_quiet() {
        let src = "\
set s { '_', 'x' }
alphabet ab { '_', 'x' }
machine {
  tape t: ab;
  entry state go { [s] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    /// A set referenced from nowhere fires exactly once, at its
    /// declaration. Mutation: a `set_refs` lookup that never misses (e.g.
    /// always treating a set as used, or checking the wrong table).
    #[test]
    fn a_set_referenced_from_nowhere_fires_once() {
        let src = "\
set dead { '_', 'x' }
alphabet ab { '_', 'x' }
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].message,
            "set `dead` is never used by any alphabet, contract or pattern"
        );
    }

    /// The same fixture's finding carries a fix. A separate row from
    /// "fires once": a rule that fires with no fix would still pass that
    /// assertion. Mutation: the fix never attaching — e.g.
    /// `decl_span::<SetDeclView>` returning `None` because the view or the
    /// anchor (`set.name_span`) is wrong.
    #[test]
    fn the_finding_carries_a_fix() {
        let src = "\
set dead { '_', 'x' }
alphabet ab { '_', 'x' }
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].fix.is_some(), "decl_span found the declaration");
    }
}
