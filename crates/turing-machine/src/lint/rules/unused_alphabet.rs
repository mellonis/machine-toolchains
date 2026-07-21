//! `unused-alphabet`: an `alphabet` declaration no tape draws on — neither a
//! machine tape declaration nor a routine/graph signature tape parameter
//! names it. Both surface as resolved tapes, so a single scan over every
//! world's tape table decides use.
//!
//! Unlike `unused-graph` / `unused-routine`, an EXPORTED alphabet is flagged
//! too: a tape may draw only on a locally-defined alphabet, so an alphabet
//! has no cross-module consumers to protect — an exported-but-undrawn-on
//! alphabet is as dead as a private one. New on the lint channel (the
//! deferred hygiene family), detected source-level over `Resolved`.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;

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
        if !used.contains(name.as_str()) {
            out.push(Diagnostic {
                code: "unused-alphabet",
                span: alphabet.name_span,
                message: format!("alphabet `{name}` is never used by any tape"),
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
}
