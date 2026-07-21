//! `unused-exit`: a `graph` declares a `state` exit parameter its own body
//! never targets — no rule `goto`, no bare-name goto, no `call … then`, and
//! no binding argument hands it on. A reference reached only through a nested
//! construct (an exit passed as an argument to an inner graft/bind/call)
//! still counts as a use — the body-reference scan is the same one the
//! `unused-graft-instance` rule uses.
//!
//! Scoped to graphs: a graph's `state` parameters are its exits (the
//! continuations a graft wires up), and a declared-but-unreached exit is dead
//! surface every caller is still obliged to bind. New on the lint channel
//! (the deferred hygiene family), detected source-level over `Resolved`.
//!
//! No fix ships: the exit is part of the graph's signature, so removing it is
//! an API change at every graft site that must currently bind it — not a
//! safe local textual edit.

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::compiler::{WorldKind, full_name};
use crate::lint::LintContext;
use crate::lint::rules::unused_graft_instance::body_referenced_names;
use crate::parser::{Program, SigParamKind};

/// The declaration span of a graph world's `state` exit parameter, read off
/// the AST (the resolved module keeps state-parameter NAMES but not their
/// spans). Falls back to `None` if the correlation misses — the caller then
/// points at the graph name.
fn exit_param_span(program: &Program, world_name: &str, param: &str) -> Option<Span> {
    let graph = program
        .graphs
        .iter()
        .find(|g| full_name(&g.ns, &g.name) == world_name)?;
    graph
        .sig
        .params
        .iter()
        .find(|p| matches!(p.kind, SigParamKind::State) && p.name == param)
        .map(|p| p.name_span)
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        if world.kind != WorldKind::Graph {
            continue;
        }
        let referenced = body_referenced_names(world);
        for param in &world.state_params {
            if !referenced.contains(param.as_str()) {
                out.push(Diagnostic {
                    code: "unused-exit",
                    span: exit_param_span(ctx.program, &world.name, param)
                        .unwrap_or(world.name_span),
                    message: format!(
                        "graph `{}` declares exit `{param}`, which its body never targets",
                        world.name
                    ),
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
            .filter(|d| d.code == "unused-exit")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn an_exit_the_body_never_targets_fires() {
        // `miss` is a declared exit no rule ever routes to.
        let src = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state hit, state miss) {
  entry state w { ['x'] -> hit; [*] -> move [>] goto w; }
}
machine {
  tape work: marks;
  entry graft findX(t = work, hit = win, miss = lose) as seek;
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("miss"), "{f:?}");
    }

    #[test]
    fn an_exit_reached_only_through_a_nested_graft_is_quiet() {
        // `out` is never a direct goto in `outer`'s body — it is only handed
        // to the inner graft as `inner`'s `done` continuation. That nested
        // reference still counts as a use.
        let src = "\
alphabet marks { '_', 'x' }
graph inner(tape t: marks, state done) {
  entry state w { ['x'] -> done; [*] -> move [>] goto w; }
}
graph outer(tape t: marks, state out) {
  entry graft inner(t = t, done = out) as step;
}
machine {
  tape work: marks;
  entry graft outer(t = work, out = fin) as run;
  state fin { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }
}
