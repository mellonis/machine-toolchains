//! `unused-binding`: a `bind … as N` whose name `N` no `call` in the same
//! world ever targets. New on the lint channel (the deferred hygiene family).
//! A bind is world-local — only a `call N(…)` inside its own world can reach
//! it — so the reference scan is scoped to the declaring world's calls.
//!
//! The fix deletes the whole `bind … ;` statement — leading doc/attention run
//! included (an orphaned `?`/`!` run is a parse error). An uncalled bind names
//! no call target, so removing it is a source-only change (`MaybeIncorrect`);
//! the target routine, if now unreferenced, becomes its own `unused-routine`
//! finding rather than an error.

use std::collections::HashSet;

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::compiler::ResolvedCallTarget;
use crate::lint::LintContext;
use crate::lint::rules::spans::decl_span;
use crate::syntax::BindView;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        let called: HashSet<&str> = world
            .calls
            .iter()
            .filter_map(|c| match &c.target {
                ResolvedCallTarget::Bind { name } => Some(name.as_str()),
                ResolvedCallTarget::Routine { .. } => None,
            })
            .collect();
        for bind in &world.binds {
            if !called.contains(bind.name.as_str()) {
                let fix = decl_span::<BindView>(ctx, bind.target_span).map(|span| Fix {
                    description: format!("delete the unused bind `{}`", bind.name),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![Edit {
                        span,
                        replacement: String::new(),
                    }],
                });
                out.push(Diagnostic {
                    code: "unused-binding",
                    span: bind.span,
                    message: format!("bind `{}` is never called", bind.name),
                    fix,
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
            .filter(|d| d.code == "unused-binding")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn a_bind_no_call_targets_fires() {
        let src = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  bind helper(t = t) as h;
  entry state go { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("`h`"), "{f:?}");
    }

    #[test]
    fn a_called_bind_is_quiet() {
        let src = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  bind helper(t = t) as h;
  entry state go { [*] -> call h() then done; }
  state done { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }
}
