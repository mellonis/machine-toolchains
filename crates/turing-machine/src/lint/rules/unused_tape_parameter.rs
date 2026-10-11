//! `unused-tape-parameter`: a `routine` or `graph` tape parameter its body
//! never reads, writes or moves, and never passes on to a `call`, `graft` or
//! `bind` — the definition `unused-tape` uses for a machine tape
//! (`unused_tape::position_is_inert` and `bound_tape_names`). A body that
//! reuses another unit with no argument list (a transparent reuse) binds its
//! tapes by position and counts as touching every one, so such a world is
//! skipped whole (docs/tmt/lint.md (unused-tape-parameter)).
//!
//! It fires whether or not the world is exported: a parameter nobody can
//! use is dead for every caller alike, unlike an unused declaration another
//! unit might import.
//!
//! No fix ships: removing a parameter changes the signature every caller
//! binds, which is not an edit this unit can make on its own.

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::WorldKind;
use crate::lint::LintContext;
use crate::lint::rules::unused_tape::{
    bound_tape_names, makes_transparent_reuse, position_is_inert,
};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        if world.kind == WorldKind::Machine {
            continue;
        }
        if makes_transparent_reuse(ctx, world) {
            continue;
        }
        let bound = bound_tape_names(world);
        for (pos, tape) in world.tapes.iter().enumerate() {
            if bound.contains(tape.name.as_str()) || !position_is_inert(world, pos) {
                continue;
            }
            out.push(Diagnostic {
                code: "unused-tape-parameter",
                span: tape.name_span,
                message: format!(
                    "tape parameter `{}` of `{}` is never read, written, moved, or passed on",
                    tape.name, world.name
                ),
                fix: None,
            });
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
            .filter(|d| d.code == "unused-tape-parameter")
            .collect()
    }

    const UTP: &str = "\
alphabet ab { '_', 'a' }
routine r(tape t: ab, tape u: ab) {
  entry state s { ['a', *] -> write ['_', -] return; [*, *] -> return; }
}
machine {
  tape x: ab;
  tape y: ab;
  entry state go { [*, *] -> call r(t = x, u = y) then stop; }
}
";

    const UTP_NEAR: &str = "\
alphabet ab { '_', 'a' }
routine leaf(tape v: ab) {
  entry state s { ['a'] -> write ['_'] return; [*] -> return; }
}
routine byname(tape t: ab, tape u: ab) {
  entry state s { ['a', *] -> call leaf(v = u) then done; [*, *] -> return; }
  state done { [*, *] -> return; }
}
routine bymove(tape t: ab, tape u: ab) {
  entry state s { ['a', *] -> move [., >] return; [*, *] -> return; }
}
routine transparent(tape t: ab, tape u: ab) {
  entry state s { [*, *] -> call std::binaryNumbersBare::plusOne() then return; }
}
machine {
  tape x: ab;
  tape y: ab;
  entry state go { [*, *] -> call byname(t = x, u = y) then b; }
  state b { [*, *] -> call bymove(t = x, u = y) then c; }
  state c { [*, *] -> call transparent(t = x, u = y) then stop; }
}
";

    /// Mutation: a `check` that never pushes.
    #[test]
    fn an_untouched_tape_parameter_is_reported() {
        let ds = findings(UTP);
        assert_eq!(ds.len(), 1, "{ds:?}");
        assert_eq!((ds[0].span.start.line, ds[0].span.start.col), (2, 28));
        assert_eq!(
            ds[0].message,
            "tape parameter `u` of `r` is never read, written, moved, or passed on"
        );
        assert!(ds[0].fix.is_none());
    }

    /// Mutation: forget one of the three uses — drop the `bound_tape_names`
    /// test (`byname`), the `position_is_inert` test (`bymove`) or the
    /// `makes_transparent_reuse` skip (`transparent`); each routine here is
    /// exactly one of them, so each mutation reddens this test.
    #[test]
    fn a_parameter_passed_on_moved_or_reached_transparently_is_used() {
        let ds = findings(UTP_NEAR);
        assert!(ds.is_empty(), "{ds:?}");
    }

    /// A contract clause alone is not a use: it constrains writes the body
    /// never makes. Mutation: count a parameter carrying a `writes` clause as
    /// used (skip any tape whose `writes` is `Some`).
    #[test]
    fn a_contract_clause_alone_is_not_a_use() {
        let src = UTP.replace("tape u: ab)", "tape u: ab writes { '_' })");
        let ds = findings(&src);
        assert_eq!(ds.len(), 1, "{ds:?}");
        assert_eq!(
            ds[0].message,
            "tape parameter `u` of `r` is never read, written, moved, or passed on"
        );
    }

    /// Mutation: skip exported worlds.
    #[test]
    fn an_exported_routine_is_reported_too() {
        let src = UTP.replace("routine r(", "export routine r(");
        let ds = findings(&src);
        assert_eq!(ds.len(), 1, "{ds:?}");
        assert_eq!((ds[0].span.start.line, ds[0].span.start.col), (2, 35));
    }
}
