//! `unused-graft-name`: an ENTRY graft's `as NAME` that nothing references.
//! An entry graft is reachable by being the world's entry, and its splice
//! runs whether or not it carries a name — the name matters only when some
//! `goto` / `call … then` / binding argument routes back to the instance. If
//! none does, the name is dead surface, and an entry graft may legally omit
//! it.
//!
//! This is the reachable-but-unreferenced gap the sibling `unused-graft-
//! instance` rule structurally skips: that rule flags only NON-entry grafts
//! (an unreferenced non-entry instance is unreachable — nothing splices to
//! it), so an entry graft never reaches it. The two rules partition the
//! grafts by entry-ness and never double-report. Both judge "referenced" by
//! the same body-reference scan.
//!
//! New on the lint channel (the deferred hygiene family), detected
//! source-level over `Resolved`. The fix removes exactly the ` as NAME`
//! clause, leaving a valid unnamed entry graft.

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

use crate::lexer::{Token, TokenKind};
use crate::lint::LintContext;
use crate::lint::rules::unused_graft_instance::body_referenced_names;

/// The span of the ` as NAME` clause within a graft — from the end of the
/// binding's closing `)` through the end of the instance name. Read off the
/// comment-free token stream: the `as` keyword's position survives in no other
/// artifact (the AST and CST keep only the NAME identifier's span). Deleting
/// exactly this span turns `graft T(args) as N;` into `graft T(args);`, which
/// an entry graft may legally be. `None` if the token shape is unexpected.
fn as_clause_span(tokens: &[Token], graft_span: Span) -> Option<Span> {
    let within = |s: Span| graft_span.start <= s.start && s.end <= graft_span.end;
    let as_ix = tokens
        .iter()
        .position(|t| matches!(&t.kind, TokenKind::Ident(k) if k == "as") && within(t.span()))?;
    let close = tokens.get(as_ix.checked_sub(1)?)?;
    let name = tokens.get(as_ix + 1)?;
    if !matches!(close.kind, TokenKind::RParen) || !matches!(name.kind, TokenKind::Ident(_)) {
        return None;
    }
    Some(Span {
        start: close.span().end,
        end: name.span().end,
    })
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        let referenced = body_referenced_names(world);
        for graft in &world.grafts {
            // Non-entry instances are `unused-graft-instance`'s domain.
            if !graft.entry {
                continue;
            }
            let Some(name) = &graft.as_name else {
                continue;
            };
            if !referenced.contains(name.as_str()) {
                let fix = as_clause_span(ctx.tokens, graft.span).map(|span| Fix {
                    description: format!("remove the unused instance name `{name}`"),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![Edit {
                        span,
                        replacement: String::new(),
                    }],
                });
                out.push(Diagnostic {
                    code: "unused-graft-name",
                    span: graft.span,
                    message: format!("entry graft instance name `{name}` is never used"),
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
            .filter(|d| d.code == "unused-graft-name")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn an_entry_graft_whose_name_nothing_references_fires() {
        // `seek` names the entry graft, but no rule routes back to it — the
        // name is redundant.
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
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("seek"), "{f:?}");
    }

    #[test]
    fn an_entry_graft_whose_name_is_referenced_is_quiet() {
        // `win` routes back to the entry graft `seek`, so the name earns its
        // keep.
        let src = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  entry graft findX(t = work, found = win, missing = lose) as seek;
  state win  { [*] -> goto seek; }
  state lose { [*] -> halt; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }
}
