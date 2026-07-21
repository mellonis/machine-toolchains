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
//!
//! The fix deletes the whole declaration, including any leading doc/attention
//! run — an orphaned `?`/`!` run is a parse error, so the doc goes with the
//! alphabet it documents.

use std::collections::HashSet;

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

use crate::lexer::{Token, TokenKind};
use crate::lint::LintContext;

/// The full source span of an alphabet declaration — its leading doc/attention
/// run (if any), the `export`/`alphabet` header, and the `{ … }` body through
/// the closing brace — anchored on the NAME identifier's span. Read off the
/// comment-free token stream: neither the resolved module nor the AST keeps a
/// declaration's closing brace or the extent of its attached doc run, and the
/// doc run must go with the alphabet (an orphaned `?`/`!` run is a parse
/// error). `None` if the token neighbourhood is not the expected shape.
fn decl_span(tokens: &[Token], name_span: Span) -> Option<Span> {
    // The NAME identifier token.
    let name_ix = tokens
        .iter()
        .position(|t| t.span().start == name_span.start)?;
    // Back up over the `alphabet` keyword, then an optional `export`, then any
    // contiguous doc/attention run bound to this declaration.
    let is_kw = |t: &Token, kw: &str| matches!(&t.kind, TokenKind::Ident(k) if k == kw);
    let mut start_ix = name_ix.checked_sub(1)?;
    if !is_kw(&tokens[start_ix], "alphabet") {
        return None;
    }
    if let Some(prev) = start_ix.checked_sub(1)
        && is_kw(&tokens[prev], "export")
    {
        start_ix = prev;
    }
    while let Some(prev) = start_ix.checked_sub(1)
        && matches!(
            tokens[prev].kind,
            TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
        )
    {
        start_ix = prev;
    }
    // The body's closing brace — an alphabet body has no nested braces, and a
    // glyph like `'{'` lexes as a symbol, not a delimiter, so the first RBrace
    // after the name is the declaration's end.
    let close = tokens[name_ix..]
        .iter()
        .find(|t| matches!(t.kind, TokenKind::RBrace))?;
    Some(Span {
        start: tokens[start_ix].span().start,
        end: close.span().end,
    })
}

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
            let fix = decl_span(ctx.tokens, alphabet.name_span).map(|span| Fix {
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
