//! Green emission for the `.tmc` parser — core's language-agnostic
//! [`GreenSink`] instantiated over this crate's kind space
//! (docs/core.md (syntax trees)). The parser stays the single owner of
//! grammar decisions — the sink only mirrors token consumption and
//! node boundaries, so the green tree and the parser's errors can
//! never disagree. The sink's own mechanics are unit-tested in core
//! against a fake kind space; the tests kept here drive the sink
//! through this crate's REAL lexer and layout adapter — the
//! integration the fake-kind tests cannot cover.

use super::kinds::TmcKind;

/// The `.tmc` green sink — core's, with this crate's kind space.
pub type GreenSink = mtc_core::syntax::GreenSink<TmcKind>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};
    use crate::syntax::kinds::TmcKind;
    use crate::syntax::layout::layout;
    use mtc_core::syntax::SyntaxNode;

    /// A sink driven by hand reproduces the source exactly. This is the
    /// lossless law one level up from `layout`: the builder must place
    /// every piece the schedule carries, in order, and lose none of it.
    ///
    /// The fixture's leading `//` comment is deliberate: `layout` folds
    /// comment tokens into the PRECEDING significant entry's trivia, so
    /// `entries` is shorter than the raw (comment-inclusive) token
    /// stream. `sig` is filtered down to the significant tokens for
    /// exactly that reason — walking the raw stream position-for-
    /// position against `entries` would drift by the comment count and
    /// eventually hand `token` an already-empty (Eof) slot.
    #[test]
    fn a_hand_driven_sink_reproduces_the_source() {
        let src = "// lead\nalphabet ab { '_' }\n";
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let sig: Vec<&crate::lexer::Token> = tokens
            .iter()
            .filter(|t| !matches!(t.kind, crate::lexer::TokenKind::Comment(_)))
            .collect();
        let mut sink = GreenSink::new(layout(src, &tokens));
        sink.start(TmcKind::Root);
        sink.start(TmcKind::Alphabet);
        for (i, t) in sig.iter().enumerate() {
            if matches!(t.kind, crate::lexer::TokenKind::Eof) {
                break;
            }
            sink.token(i, crate::syntax::kinds::token_kind(&t.kind));
        }
        sink.finish();
        let green = sink.finish_tree(sig.len() - 1);
        let root = SyntaxNode::new_root(green);
        assert_eq!(root.text(), src, "the sink lost or reordered source");
    }

    /// A checkpoint started retroactively wraps tokens already emitted —
    /// the mechanism a doc run bound to a later declaration needs.
    ///
    /// The kind check alone would pass even if `start_at` opened DocRun
    /// as an EMPTY node sitting after the doc line rather than wrapping
    /// it — the first node child would still be a `DocRun`, just an
    /// empty one. The `text()` check closes that gap: it only reads
    /// "? doc" back if the DocLine token actually landed inside DocRun.
    ///
    /// The `tokens[i]`/`tokens.len()` indexing below is only safe
    /// because this fixture's tokens are all significant (no `//`
    /// comment) — `tokens` and `layout`'s `entries` stay 1:1. Do not
    /// copy this pattern onto a source with a comment; see the sibling
    /// test above for what goes wrong and how it's avoided.
    #[test]
    fn a_retroactive_checkpoint_wraps_already_emitted_tokens() {
        let src = "? doc\nalphabet ab { '_' }\n";
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let mut sink = GreenSink::new(layout(src, &tokens));
        sink.start(TmcKind::Root);
        let cp = sink.checkpoint();
        sink.token(0, TmcKind::DocLine);
        sink.start_at(cp, TmcKind::DocRun);
        sink.finish();
        for (i, t) in tokens.iter().enumerate().take(tokens.len() - 1).skip(1) {
            sink.token(i, crate::syntax::kinds::token_kind(&t.kind));
        }
        let green = sink.finish_tree(tokens.len() - 1);
        let root = SyntaxNode::new_root(green);
        assert_eq!(root.text(), src);
        let doc_run = root.children().next().expect("a DocRun child node");
        assert_eq!(
            doc_run.kind(),
            TmcKind::DocRun.into(),
            "the checkpoint did not wrap the doc line"
        );
        assert_eq!(
            doc_run.text(),
            "? doc",
            "the DocRun node did not actually contain the doc-line token"
        );
    }
}
