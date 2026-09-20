//! `unreachable-continuation`: a `then` written on a call whose callee is
//! KNOWN to be `noreturn` — the continuation can never run, since the
//! callee never hands control back. "Known" means the callee's
//! DECLARATION carries the `noreturn` clause and this unit can see it: an
//! IN-UNIT callee's own `ResolvedWorld::declared_noreturn`, or an
//! out-of-unit one's entry in `ctx.externals` (docs/tmt/language.md
//! (declarations)). A callee this unit cannot see at all — an external
//! call with no declarations for it — is left alone even when it happens
//! to BE `noreturn` in reality: the linker never checks a `then` either
//! way, so nothing here can tell "unreachable" from "merely unproven".
//!
//! Deliberately DECLARATION-only, never the full body inference
//! `ir::body_can_return` runs: this rule works over `Resolved` (lint never
//! runs `expand`/`lower` — the module doc's own reasoning: those stages
//! can fatal on input `analyze` accepted). A routine that is genuinely
//! `noreturn` but never says so in its own signature is a false negative
//! here — under-reporting, which a lint may do; a `then` this rule leaves
//! alone is never wrongly flagged.
//!
//! The fix drops the whole ` then …` clause (`spans::then_clause_span`),
//! subject to the shared comment guard (`crate::lint::run_rules`).

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

use crate::compiler::ResolvedCallTarget;
use crate::lint::LintContext;
use crate::lint::rules::spans::then_clause_span;
use crate::parser::Continuation;

fn continuation_span(cont: &Continuation) -> Span {
    match cont {
        Continuation::State { span, .. }
        | Continuation::Return { span }
        | Continuation::Stop { span }
        | Continuation::Halt { span } => *span,
    }
}

/// Whether `target` is KNOWN, to this unit, to be `noreturn` — `None` when
/// it cannot be seen at all (an external call this unit has no
/// declarations for).
fn known_noreturn(ctx: &LintContext, target: &str, external: bool) -> Option<bool> {
    if external {
        ctx.externals
            .routine(target)
            .map(|rw| rw.declared_noreturn.is_some())
    } else {
        ctx.resolved
            .worlds
            .iter()
            .find(|w| w.name == target)
            .map(|w| w.declared_noreturn.is_some())
    }
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        for call in &world.calls {
            // An omitted `then` is exactly what this rule wants — nothing
            // to flag, and `ir::lower_rule` is the one place its own
            // legality (a KNOWN `noreturn` callee) is checked.
            let Some(then) = &call.then else {
                continue;
            };
            let (target, external) = match &call.target {
                ResolvedCallTarget::Routine { name, external, .. } => (name.as_str(), *external),
                ResolvedCallTarget::Bind { name } => {
                    let Some(bind) = world.binds.iter().find(|b| &b.name == name) else {
                        continue;
                    };
                    (bind.target.as_str(), bind.external)
                }
            };
            if known_noreturn(ctx, target, external) != Some(true) {
                continue;
            }
            let fix = then_clause_span(ctx, call.span).map(|span| Fix {
                description: "drop the unreachable `then` clause".to_string(),
                applicability: Applicability::MachineApplicable,
                edits: vec![Edit {
                    span,
                    replacement: String::new(),
                }],
            });
            out.push(Diagnostic {
                code: "unreachable-continuation",
                span: continuation_span(then),
                message: format!("this `then` is unreachable — `{target}` is `noreturn`"),
                fix,
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
            .filter(|d| d.code == "unreachable-continuation")
            .map(|d| d.message)
            .collect()
    }

    const NORETURN_ROUTINE: &str = "\
routine forever(tape t: ab) noreturn {
  entry state s { [*] -> goto s; }
}
";

    /// The `then` fires — `r`'s declarations are in this unit, and it is
    /// declared `noreturn`. Mutation: dropping the `known_noreturn` check
    /// entirely (always true) would also pass this alone; paired with the
    /// third fixture below, only a real check survives both.
    #[test]
    fn a_then_on_a_known_noreturn_callee_is_a_finding() {
        let src = format!(
            "alphabet ab {{ '_', 'a' }}\n{NORETURN_ROUTINE}\
machine {{
  tape t: ab;
  entry state go {{ [*] -> call forever(t = t) then done; }}
  state done {{ [*] -> stop; }}
}}
"
        );
        assert_eq!(
            findings(&src),
            vec!["this `then` is unreachable — `forever` is `noreturn`"]
        );
    }

    /// The same site with `then` omitted: clean — an omitted `then` is
    /// never this rule's finding, it is `ir::lower_rule`'s own legality
    /// check (which this lint layer never runs).
    #[test]
    fn an_omitted_then_on_a_known_noreturn_callee_is_clean() {
        let src = format!(
            "alphabet ab {{ '_', 'a' }}\n{NORETURN_ROUTINE}\
machine {{
  tape t: ab;
  entry state go {{ [*] -> call forever(t = t); }}
}}
"
        );
        assert!(findings(&src).is_empty(), "{:?}", findings(&src));
    }

    /// The same site, but naming a callee whose declarations are NOT in
    /// this unit's table (a plain external call — nothing here declares
    /// `outside`, and the embedded stdlib does not export anything by that
    /// name) — clean, and `then` stays mandatory (a separate, compile-time
    /// concern). Mutation: keying the finding on the callee's name/shape
    /// without checking that its declarations were actually GIVEN to this
    /// unit — only this fixture goes red under that mutation, since the
    /// first two never exercise the "undeclared" branch at all.
    #[test]
    fn a_then_on_an_undeclared_callee_is_clean_and_still_mandatory() {
        let src = "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { [*] -> call outside(t = t) then done; }
  state done { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    /// A comment inside the fix's own edit span withholds it — the shared
    /// guard `run_rules` applies to every rule, this one included. The
    /// finding still reports; only the fix is missing.
    #[test]
    fn the_comment_guard_withholds_the_quickfix() {
        let src = format!(
            "alphabet ab {{ '_', 'a' }}\n{NORETURN_ROUTINE}\
machine {{
  tape t: ab;
  entry state go {{ [*] -> call forever(t = t) then /* c */ done; }}
  state done {{ [*] -> stop; }}
}}
"
        );
        let report = lint(&src, LintOptions::default()).unwrap();
        let d = report
            .diagnostics
            .iter()
            .find(|d| d.code == "unreachable-continuation")
            .expect("the finding still reports");
        assert!(d.fix.is_none(), "a comment in the span withholds the fix");
    }

    #[test]
    fn the_fix_drops_the_whole_then_clause() {
        let src = format!(
            "alphabet ab {{ '_', 'a' }}\n{NORETURN_ROUTINE}\
machine {{
  tape t: ab;
  entry state go {{ [*] -> call forever(t = t) then done; }}
  state done {{ [*] -> stop; }}
}}
"
        );
        let report = lint(&src, LintOptions::default()).unwrap();
        let d = report
            .diagnostics
            .iter()
            .find(|d| d.code == "unreachable-continuation")
            .expect("a finding");
        let fix = d.fix.as_ref().expect("a fix");
        assert_eq!(fix.edits.len(), 1);
        assert!(fix.edits[0].replacement.is_empty());
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let src = format!(
            "alphabet ab {{ '_', 'a' }}\n{NORETURN_ROUTINE}\
machine {{
  tape t: ab;
  entry state go {{ [*] -> call forever(t = t) then done; }}
  state done {{ [*] -> stop; }}
}}
"
        );
        let report = lint(
            &src,
            LintOptions {
                allow: vec!["unreachable-continuation".to_string()],
                warn: Vec::new(),
            },
        )
        .unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "unreachable-continuation")
        );
    }
}
