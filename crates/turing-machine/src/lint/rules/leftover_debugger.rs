//! `leftover-debugger`: a `debugger` marker left on a rule. It lowers to a
//! `brk` (docs/core.md (debug break)); an un-stripped `brk` is an optimizer
//! observability barrier, so shipping one also pessimizes `-O1` output. A `brk`
//! is a no-op in a plain run (it only pauses a debug session), so removing the
//! marker is behaviour-preserving in ordinary execution.
//!
//! The fix removes just the `debugger` keyword (the marker sits after the
//! pattern's `->`, ahead of any write/move/transition). It is offered ONLY when
//! the rule carries another action or an explicit transition, so what remains
//! is a valid rule: a rule whose SOLE action is `debugger` (`… -> debugger;`)
//! would become `… -> ;`, the "expected a transition" parse error, so no fix is
//! offered there. `MaybeIncorrect` — the removal deletes a deliberate marker.

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

use crate::lexer::{Token, TokenKind};
use crate::lint::LintContext;
use crate::parser::Transition;

/// The span to delete to remove a rule's `debugger` marker: from the marker
/// keyword through the start of the following token, so the trailing space goes
/// too. The marker is the token right after the rule's first `->`. `None` if
/// the token shape is unexpected (then no fix is offered).
fn marker_span(tokens: &[Token], rule_span: Span) -> Option<Span> {
    let within = |s: Span| rule_span.start <= s.start && s.end <= rule_span.end;
    let arrow_ix = tokens
        .iter()
        .position(|t| matches!(t.kind, TokenKind::Arrow) && within(t.span()))?;
    let marker = tokens.get(arrow_ix + 1)?;
    if !matches!(&marker.kind, TokenKind::Ident(k) if k == "debugger") {
        return None;
    }
    // Offered only when a following action/transition token remains, so the
    // token after the marker always exists here.
    let next = tokens.get(arrow_ix + 2)?;
    Some(Span {
        start: marker.span().start,
        end: next.span().start,
    })
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        for state in &world.states {
            for rule in &state.rules {
                if !rule.debugger {
                    continue;
                }
                // Safe to remove only when the rule keeps a valid action after
                // the marker goes: a write/move, or an explicit transition.
                let has_other = rule.write.is_some()
                    || rule.mov.is_some()
                    || !matches!(rule.transition, Transition::Stay { .. });
                let fix = has_other
                    .then(|| marker_span(ctx.tokens, rule.span))
                    .flatten()
                    .map(|span| Fix {
                        description: "remove the leftover `debugger` marker".to_string(),
                        applicability: Applicability::MaybeIncorrect,
                        edits: vec![Edit {
                            span,
                            replacement: String::new(),
                        }],
                    });
                out.push(Diagnostic {
                    code: "leftover-debugger",
                    span: rule.span,
                    message: "leftover 'debugger' marker".to_string(),
                    fix,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    const SRC: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> debugger goto s;
    ['_'] -> stop;
  }
}
";

    #[test]
    fn a_debugger_marker_fires_with_a_removal_fix() {
        // The marked rule keeps an explicit `goto s`, so removing the marker
        // leaves a valid rule and a fix is offered.
        let report = lint(SRC, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leftover-debugger")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "leftover 'debugger' marker");
        let fix = d[0].fix.as_ref().expect("a removal fix");
        assert_eq!(fix.edits.len(), 1);
        assert!(fix.edits[0].replacement.is_empty());
    }

    #[test]
    fn a_sole_debugger_rule_fires_without_a_fix() {
        // `debugger` is the rule's ONLY action (transition omitted). Removing
        // it would leave `-> ;`, a parse error, so no fix is offered.
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> debugger;
    ['_'] -> stop;
  }
}
";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leftover-debugger")
            .collect();
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none(), "sole-action debugger offers no fix");
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let report = lint(
            SRC,
            LintOptions {
                allow: vec!["leftover-debugger".to_string()],
                warn: Vec::new(),
            },
        )
        .unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "leftover-debugger")
        );
    }

    #[test]
    fn a_clean_program_is_quiet() {
        let clean = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { [*] -> stop; }
}
";
        let report = lint(clean, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "leftover-debugger")
        );
    }
}
