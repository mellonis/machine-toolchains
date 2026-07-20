//! `unused-routine`: a non-exported `routine` that no `call`/`bind` names,
//! anywhere in the module. The compiler already raises this during IR
//! lowering; this rule RE-EXPOSES it on the lint channel under allow control,
//! detecting it source-level over `Resolved` (the lint never runs the later
//! pipeline stages). Exported routines are library API and are never flagged.
//!
//! A routine counts as referenced by any direct `call` target OR any `bind`
//! target — a bind's target routine is "used" even when the bind itself is
//! never called. That is a deliberate over-approximation of use: it can only
//! MISS an unused routine, never invent one (the unused bind fires
//! `unused-binding` in its own right), which keeps the rule false-positive-free.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::{ResolvedCallTarget, WorldKind};
use crate::lint::LintContext;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    let mut referenced: HashSet<&str> = HashSet::new();
    for world in &ctx.resolved.worlds {
        for call in &world.calls {
            if let ResolvedCallTarget::Routine { name, .. } = &call.target {
                referenced.insert(name.as_str());
            }
        }
        for bind in &world.binds {
            referenced.insert(bind.target.as_str());
        }
    }

    for world in &ctx.resolved.worlds {
        if world.kind == WorldKind::Routine
            && world.local
            && !referenced.contains(world.name.as_str())
        {
            out.push(Diagnostic {
                code: "unused-routine",
                span: world.name_span,
                message: format!("routine `{}` is never called", world.name),
                fix: None,
            });
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
            .filter(|d| d.code == "unused-routine")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn a_local_uncalled_routine_fires_but_an_exported_one_does_not() {
        let src = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
export routine api(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("helper"), "{f:?}");
    }

    #[test]
    fn a_called_routine_is_quiet() {
        let src = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  entry state go { [*] -> call helper(t = t) then done; }
  state done { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_routine_reached_only_through_a_bind_is_quiet() {
        let src = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  bind helper(t = t) as h;
  entry state go { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }
}
