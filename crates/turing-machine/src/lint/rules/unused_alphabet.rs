//! `unused-alphabet`: an `alphabet` declaration no tape draws on — neither a
//! machine tape declaration nor a routine/graph signature tape parameter
//! names it. Both surface as resolved tapes, so a single scan over every
//! world's tape table decides use.
//!
//! Like `unused-graph` / `unused-routine`, an EXPORTED alphabet is never
//! reported: another unit may import it through this unit's header or
//! sibling source (`docs/tmt/language.md (alphabets, maps and graphs across
//! units)`), a use this rule cannot see, and the deletion fix would remove
//! what a consumer needs. Detected source-level over `Resolved`.
//!
//! The fix deletes the whole declaration, including any leading doc/attention
//! run — an orphaned `?`/`!` run is a parse error, so the doc goes with the
//! alphabet it documents.

use std::collections::HashSet;

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::lint::rules::spans::decl_span;
use crate::syntax::AlphabetView;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    // Every alphabet some world's tape draws on (machine tape declarations
    // and routine/graph signature tape parameters alike become resolved
    // tapes keyed by the alphabet's mangled name).
    let mut used: HashSet<&str> = HashSet::new();
    for world in &ctx.resolved.worlds {
        for tape in &world.tapes {
            used.insert(tape.alphabet.as_str());
        }
    }

    for (name, alphabet) in &ctx.resolved.alphabets {
        if !alphabet.exported && !used.contains(name.as_str()) {
            let fix = decl_span::<AlphabetView>(ctx, alphabet.name_span).map(|span| Fix {
                description: format!("delete the unused alphabet `{name}`"),
                applicability: Applicability::MaybeIncorrect,
                edits: vec![Edit {
                    span,
                    replacement: String::new(),
                }],
            });
            out.push(Diagnostic {
                code: "unused-alphabet",
                span: alphabet.name_span,
                message: format!("alphabet `{name}` is never used by any tape"),
                fix,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    const EXP: &str = "\
alphabet digit { '_', '0', '1' }
export alphabet deadA { '_', 'x' }
alphabet deadB { '_', 'y' }
export set deadS { '0' }
set deadT { '1' }
export map deadM: digit -> digit { '0' -> '1', '1' -> '0' }
map deadN: digit -> digit { '0' -> '1', '1' -> '0' }
machine {
  tape t: digit;
  entry state go { ['0'] -> write ['1'] stop; [*] -> stop; }
}
";

    fn findings(src: &str) -> Vec<String> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "unused-alphabet")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn an_alphabet_no_tape_draws_on_fires() {
        let src = "\
alphabet bit { '_', '1' }
alphabet marks { '_', 'x' }
machine {
  tape t: bit;
  entry state s { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("marks"), "{f:?}");
    }

    #[test]
    fn an_alphabet_used_only_by_a_signature_parameter_is_quiet() {
        // `marks` is drawn on by no machine tape — only by the graph's tape
        // parameter. A signature tape parameter counts as a use, so nothing
        // fires (`bit` is drawn on by the machine tape).
        let src = "\
alphabet bit { '_', '1' }
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state done) {
  entry state w { ['x'] -> done; [*] -> move [>] goto w; }
}
machine {
  tape work: bit;
  entry state s { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    /// An exported alphabet is never reported (another unit may import it);
    /// the same declaration without `export` still is, and keeps its fix.
    /// Mutation: dropping the `exported` skip in `check` — `deadA` returns.
    #[test]
    fn an_exported_alphabet_is_not_reported() {
        let report = lint(EXP, LintOptions::default()).unwrap();
        let found: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "unused-alphabet")
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].message.contains("deadB"), "{found:?}");
        assert_eq!((found[0].span.start.line, found[0].span.start.col), (3, 10));
        assert!(found[0].fix.is_some());
    }
}
