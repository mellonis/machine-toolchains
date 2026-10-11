//! `inert-never-writes`: with both `writes` and `never writes` written on a
//! tape parameter, a `never writes` element naming glyphs `writes` does not
//! name changes nothing — the effective allowed set is `writes` minus
//! `never writes`, and a glyph outside `writes` was never in it.
//!
//! The mirror image of `contract-clause-overlap` (which reports the `writes`
//! element a `never writes` cancels): it reads the same resolved sets and
//! the same AST clauses, and reports per SOURCE ELEMENT of `never writes`,
//! naming only the inert glyphs. A `never writes` without a `writes` clause
//! subtracts from the whole alphabet and is never inert; `never writes {}`
//! has no element. No fix ships. Source only: a header publishes each
//! tape's effective set as one `writes` clause and never prints
//! `never writes` (docs/tmt/lint.md (linting a header)).

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::{Scopes, SetScope};
use crate::lint::LintContext;
use crate::lint::rules::contract_clause_overlap::{
    ascending_glyphs, elem_indices, elem_span, never_writes_clause_for, quoted_list, world_sig,
};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    let Ok(scopes) = Scopes::build(ctx.program) else {
        return;
    };
    let ext_modules = ctx.externals.modules();
    let sets = SetScope {
        scopes: &scopes,
        sets: &ctx.resolved.sets,
        externals: &ext_modules,
    };
    for world in &ctx.resolved.worlds {
        let Some((params, ns)) = world_sig(ctx.program, &world.name) else {
            continue;
        };
        for tape in &world.tapes {
            let (Some(writes), Some(_)) = (tape.writes, tape.never_writes) else {
                continue;
            };
            let Some(clause) = never_writes_clause_for(params, &tape.name) else {
                continue;
            };
            let Some(glyphs) = crate::lint::alphabet_glyphs(ctx.resolved, &tape.alphabet) else {
                continue;
            };
            for elem in &clause.elems {
                let Some(indices) = elem_indices(elem, glyphs, &sets, ns) else {
                    continue;
                };
                let inert: Vec<u32> = indices
                    .iter()
                    .copied()
                    .filter(|ix| !writes.contains(*ix))
                    .collect();
                if inert.is_empty() {
                    continue;
                }
                let named = quoted_list(&ascending_glyphs(glyphs, &inert));
                out.push(Diagnostic {
                    code: "inert-never-writes",
                    span: elem_span(elem),
                    message: format!(
                        "{named} in `never writes` is not in `writes`, so excluding it changes nothing"
                    ),
                    fix: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::Diagnostic;

    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<Diagnostic> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "inert-never-writes")
            .collect()
    }

    const INW: &str = "\
alphabet digit { '_', '0', '1', '2' }
routine r(tape u: digit writes { '0', '1' } never writes { '2' }) {
  entry state e { ['1'] -> write ['0'] return; [*] -> return; }
}
machine {
  tape t: digit;
  entry state go { [*] -> call r(u = t) then stop; }
}
";

    const INW_NEAR: &str = "\
alphabet digit { '_', '0', '1', '2' }
routine both(tape u: digit writes { '0', '1', '2' } never writes { '2' }) {
  entry state e { ['1'] -> write ['0'] return; [*] -> return; }
}
routine only(tape u: digit never writes { '2' }) {
  entry state e { ['1'] -> write ['0'] return; [*] -> return; }
}
machine {
  tape t: digit;
  entry state go { [*] -> call both(u = t) then b; }
  state b { [*] -> call only(u = t) then stop; }
}
";

    /// Mutation: a `check` that never pushes.
    #[test]
    fn a_never_writes_glyph_writes_does_not_name_is_reported() {
        let ds = findings(INW);
        assert_eq!(ds.len(), 1, "{ds:?}");
        assert_eq!((ds[0].span.start.line, ds[0].span.start.col), (2, 60));
        assert_eq!(
            ds[0].message,
            "'2' in `never writes` is not in `writes`, so excluding it changes nothing"
        );
        assert!(ds[0].fix.is_none());
    }

    /// Mutation: fire without requiring `writes` (the `only` routine would
    /// report), or on glyphs `writes` does name (the `both` routine would
    /// report) — the overlap rule's own case stays that rule's.
    #[test]
    fn an_effective_or_writes_less_never_writes_is_silent() {
        let ds = findings(INW_NEAR);
        assert!(ds.is_empty(), "{ds:?}");
    }
}
