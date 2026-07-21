//! `unused-graft-instance`: a named, non-entry `graft … as N` whose instance
//! name `N` nothing in the world jumps to — a dead splice. New on the lint
//! channel (the deferred hygiene family). An entry graft is the world's entry
//! and is always live.
//!
//! Soundness: the reference scan OVER-approximates. A graft instance is
//! reachable through a `goto`, a `call … then N`, or a binding argument that
//! passes `N` as a state continuation (`found = N`) — and at this stage a
//! bare binding target (`x = N`, no `with map`) is not yet classified as a
//! tape target vs a state continuation. Rather than re-run that classification,
//! every bare binding target across the world is treated as a potential
//! reference. That can only let a genuinely-dead instance slip through, never
//! flag a live one.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::ResolvedWorld;
use crate::lint::LintContext;
use crate::parser::{BindingArg, BindingValue, Continuation, Transition};

/// Extend `names` with every bare binding-argument target in `args` (a bare
/// `x = N` target — over-approx of the state continuations among them; see the
/// module note).
fn collect_arg_targets<'a>(args: &'a [BindingArg], names: &mut HashSet<&'a str>) {
    for arg in args {
        if let BindingValue::Named { target, .. } = &arg.value {
            names.insert(target.as_str());
        }
    }
}

/// Every name the world's BODY jumps to: `goto` targets, `call … then` state
/// continuations, and every bare binding-argument target (over-approx — see
/// the module note). The world's own entry is deliberately NOT counted — a
/// declaration is not a use of itself. Shared with the `unused-graft-name`
/// and `unused-exit` rules so all three judge a body reference identically.
pub(crate) fn body_referenced_names(world: &ResolvedWorld) -> HashSet<&str> {
    let mut names: HashSet<&str> = HashSet::new();
    for graft in &world.grafts {
        collect_arg_targets(&graft.args, &mut names);
    }
    for bind in &world.binds {
        collect_arg_targets(&bind.args, &mut names);
    }
    for state in &world.states {
        for rule in &state.rules {
            match &rule.transition {
                Transition::Goto { name, .. } => {
                    names.insert(name.as_str());
                }
                Transition::Call { args, then, .. } => {
                    collect_arg_targets(args, &mut names);
                    if let Continuation::State { name, .. } = then {
                        names.insert(name.as_str());
                    }
                }
                Transition::Return { .. } | Transition::Stop { .. } | Transition::Halt { .. } => {}
                // An omitted transition self-loops to the current (own) state,
                // never to a graft instance — it references no instance name.
                Transition::Stay { .. } => {}
            }
        }
    }
    names
}

/// Body references plus the world's own entry, for good measure. `unused-graft-
/// instance` treats a non-entry graft named by the entry as reachable.
fn referenced_names(world: &ResolvedWorld) -> HashSet<&str> {
    let mut names = body_referenced_names(world);
    if let Some(entry) = &world.entry {
        names.insert(entry.as_str());
    }
    names
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        let referenced = referenced_names(world);
        for graft in &world.grafts {
            if graft.entry {
                continue;
            }
            let Some(name) = &graft.as_name else {
                continue;
            };
            if !referenced.contains(name.as_str()) {
                out.push(Diagnostic {
                    code: "unused-graft-instance",
                    span: graft.span,
                    message: format!("graft instance `{name}` is never used"),
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
            .filter(|d| d.code == "unused-graft-instance")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn a_named_graft_nothing_jumps_to_fires() {
        // `seek` is spliced but no rule ever gotos it; the entry runs straight
        // to a stop.
        let src = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  graft findX(t = work, found = win, missing = lose) as seek;
  entry state go { [*] -> stop; }
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("seek"), "{f:?}");
    }

    #[test]
    fn an_entry_graft_is_never_flagged() {
        let src = "\
alphabet marks { '_', 'x' }
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
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_graft_a_goto_reaches_is_quiet() {
        let src = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  graft findX(t = work, found = win, missing = lose) as seek;
  entry state go { [*] -> goto seek; }
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }
}
