//! `unused-fold-variable`: a pattern binding (`as NAME`) that nothing in its
//! own rule's write vector reads. A read inside an arithmetic substitution
//! (`{x + 1}`) counts, however deeply the name is nested in the expression
//! (docs/tmt/language.md (substitution)).
//!
//! No fix ships: a binding may be kept for readability, and a positional
//! one costs nothing, so the rule reports without guessing.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;
use crate::parser::{FoldExprKind, FoldExprNode, WriteCellKind};

/// Every variable name `node` reads, through nested binary applications.
fn collect_reads<'a>(node: &'a FoldExprNode, reads: &mut HashSet<&'a str>) {
    match &node.kind {
        FoldExprKind::Var(name) => {
            reads.insert(name.as_str());
        }
        FoldExprKind::Int(_) => {}
        FoldExprKind::Bin { lhs, rhs, .. } => {
            collect_reads(lhs, reads);
            collect_reads(rhs, reads);
        }
    }
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        for state in &world.states {
            for rule in &state.rules {
                let mut reads: HashSet<&str> = HashSet::new();
                if let Some(write) = &rule.write {
                    for cell in &write.cells {
                        if let WriteCellKind::Subst { expr } = &cell.kind {
                            collect_reads(expr, &mut reads);
                        }
                    }
                }
                for cell in &rule.pattern.cells {
                    let Some(binding) = &cell.binding else {
                        continue;
                    };
                    if reads.contains(binding.name.as_str()) {
                        continue;
                    }
                    out.push(Diagnostic {
                        code: "unused-fold-variable",
                        span: binding.span,
                        message: format!(
                            "pattern binding `{}` is never read by this rule's write vector",
                            binding.name
                        ),
                        fix: None,
                    });
                }
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
            .filter(|d| d.code == "unused-fold-variable")
            .collect()
    }

    const UFV: &str = "\
alphabet digit { '_', '0', '1', '2' }
machine {
  tape t: digit;
  entry state go { ['1' as x] -> write ['0'] move [>] goto go; [*] -> stop; }
}
";

    const UFV_NEAR: &str = "\
alphabet digit { '_', '0', '1', '2' }
machine {
  tape t: digit;
  tape u: digit;
  entry state go {
    [0..1 as x, 0..2 as y] -> write [{x + 1}, {y}] move [>, >] goto go;
    [*, *] -> stop;
  }
}
";

    /// Mutation: a `check` that never pushes.
    #[test]
    fn an_unread_binding_is_reported() {
        let ds = findings(UFV);
        assert_eq!(ds.len(), 1, "{ds:?}");
        assert_eq!((ds[0].span.start.line, ds[0].span.start.col), (4, 28));
        assert_eq!(
            ds[0].message,
            "pattern binding `x` is never read by this rule's write vector"
        );
        assert!(ds[0].fix.is_none());
    }

    /// Mutation: collect only top-level `Var` reads, so `x` inside `x + 1`
    /// is missed and reported.
    #[test]
    fn a_binding_read_inside_a_fold_is_used() {
        let ds = findings(UFV_NEAR);
        assert!(ds.is_empty(), "{ds:?}");
    }
}
