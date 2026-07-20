//! `deprecated-call`: a `call`/`graft`/`bind` whose resolved target carries a
//! `! [deprecated]` doc line. `Resolved.docs` is keyed by the same mangled name
//! the target resolves to, so this is a direct map lookup. Report-only — there
//! is no mechanical fix for "stop using this". Only locally-defined targets are
//! checked: an external (imported) target's doc is not in this module's map, so
//! its deprecation cannot be seen here.

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::compiler::ResolvedCallTarget;
use crate::lint::LintContext;

/// The finding for a deprecated `name`, if `resolved.docs` marks it deprecated.
fn finding(ctx: &LintContext, verb: &str, name: &str, span: Span) -> Option<Diagnostic> {
    let message = ctx.resolved.docs.get(name)?.deprecated.as_ref()?;
    let suffix = if message.is_empty() {
        String::new()
    } else {
        format!(": {message}")
    };
    Some(Diagnostic {
        code: "deprecated-call",
        span,
        message: format!("{verb} deprecated `{name}`{suffix}"),
        fix: None,
    })
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        for call in &world.calls {
            if let ResolvedCallTarget::Routine { name, .. } = &call.target
                && let Some(d) = finding(ctx, "call to", name, call.span)
            {
                out.push(d);
            }
        }
        for graft in &world.grafts {
            if let Some(d) = finding(ctx, "graft of", &graft.target, graft.target_span) {
                out.push(d);
            }
        }
        for bind in &world.binds {
            if let Some(d) = finding(ctx, "bind to", &bind.target, bind.target_span) {
                out.push(d);
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
    fn a_call_to_a_deprecated_routine_is_flagged_with_message() {
        let src = "\
alphabet ab { '_', 'a' }
? the old routine.
! [deprecated] use api instead.
routine old(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  entry state go { [*] -> call old(t = t) then done; }
  state done { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f, vec!["call to deprecated `old`: use api instead."]);
    }

    #[test]
    fn a_graft_of_a_deprecated_graph_is_flagged_without_a_message() {
        let src = "\
alphabet marks { '_', 'x' }
! [deprecated]
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  entry graft findX(t = work, found = win, missing = lose) as seek;
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";
        assert_eq!(findings(src), vec!["graft of deprecated `findX`"]);
    }

    #[test]
    fn a_call_to_a_non_deprecated_target_is_quiet() {
        let src = "\
alphabet ab { '_', 'a' }
? a fine routine.
routine fine(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  entry state go { [*] -> call fine(t = t) then done; }
  state done { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let src = "\
alphabet ab { '_', 'a' }
! [deprecated]
routine old(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  entry state go { [*] -> call old(t = t) then done; }
  state done { [*] -> stop; }
}
";
        let report = lint(
            src,
            LintOptions {
                allow: vec!["deprecated-call".to_string()],
                warn: Vec::new(),
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
}
