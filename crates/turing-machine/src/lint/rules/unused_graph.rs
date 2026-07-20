//! `unused-graph`: a non-exported `graph` that no `graft` names, anywhere in
//! the module. The compiler emits no warning for this today — the rule is new
//! here (the deferred hygiene family landing on the lint channel). Detected
//! source-level over `Resolved`; exported graphs are library API and are never
//! flagged.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::WorldKind;
use crate::lint::LintContext;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    let mut grafted: HashSet<&str> = HashSet::new();
    for world in &ctx.resolved.worlds {
        for graft in &world.grafts {
            grafted.insert(graft.target.as_str());
        }
    }

    for world in &ctx.resolved.worlds {
        if world.kind == WorldKind::Graph && world.local && !grafted.contains(world.name.as_str()) {
            out.push(Diagnostic {
                code: "unused-graph",
                span: world.name_span,
                message: format!("graph `{}` is never grafted", world.name),
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
            .filter(|d| d.code == "unused-graph")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn a_local_ungrafted_graph_fires_but_an_exported_one_does_not() {
        let src = "\
alphabet marks { '_', 'x' }
graph dead(tape t: marks, state hit, state miss) {
  entry state w { ['x'] -> hit; [*] -> miss; }
}
export graph api(tape t: marks, state hit, state miss) {
  entry state w { ['x'] -> hit; [*] -> miss; }
}
machine {
  tape work: marks;
  entry state go { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("dead"), "{f:?}");
    }

    #[test]
    fn a_grafted_graph_is_quiet() {
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
}
