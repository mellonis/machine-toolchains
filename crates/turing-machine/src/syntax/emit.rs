//! Green emission for the `.tmc` parser: a `TreeBuilder` fed from a
//! [`super::SigLayout`] schedule. The parser stays the single owner
//! of grammar decisions — the sink only mirrors token consumption and
//! node boundaries, so the green tree and the parser's errors can
//! never disagree (docs/core.md (syntax trees)). Ported from the
//! sibling `.pmc` crate's sink of the same name — the mechanism is
//! language-independent.

use std::rc::Rc;

use mtc_core::syntax::{Checkpoint, GreenNode, TreeBuilder};

use super::kinds::TmcKind;
use super::layout::SigLayout;

pub struct GreenSink {
    builder: TreeBuilder,
    entries: Vec<SigLayout>,
    /// First significant-token index whose trivia is not yet emitted.
    flushed_upto: usize,
}

impl GreenSink {
    pub fn new(entries: Vec<SigLayout>) -> GreenSink {
        GreenSink {
            builder: TreeBuilder::new(),
            entries,
            flushed_upto: 0,
        }
    }

    /// Emit `trivia_before[pos]` into the currently open node, once.
    /// Idempotent — a later call at the same or an already-flushed
    /// `pos` is a no-op, so callers needn't track whether some other
    /// helper already flushed this position.
    pub fn flush(&mut self, pos: usize) {
        if self.flushed_upto > pos {
            return;
        }
        debug_assert_eq!(self.flushed_upto, pos, "trivia flushed out of order");
        for (kind, text) in &self.entries[pos].trivia_before {
            self.builder.token((*kind).into(), text);
        }
        self.flushed_upto = pos + 1;
    }

    /// Flush, then emit significant token `pos` verbatim.
    pub fn token(&mut self, pos: usize, kind: TmcKind) {
        self.flush(pos);
        let text = std::mem::take(&mut self.entries[pos].text);
        debug_assert!(!text.is_empty(), "significant token {pos} emitted twice");
        self.builder.token(kind.into(), &text);
    }

    pub fn start(&mut self, kind: TmcKind) {
        self.builder.start_node(kind.into());
    }

    pub fn finish(&mut self) {
        self.builder.finish_node();
    }

    pub fn checkpoint(&self) -> Checkpoint {
        self.builder.checkpoint()
    }

    pub fn start_at(&mut self, cp: Checkpoint, kind: TmcKind) {
        self.builder.start_node_at(cp, kind.into());
    }

    /// Emit the trailing trivia (the Eof entry's schedule) and close.
    /// Call with the root node still open — this flushes the tail,
    /// finishes the root, and closes the build.
    pub fn finish_tree(mut self, pos_after_last: usize) -> Rc<GreenNode> {
        self.flush(pos_after_last);
        self.builder.finish_node();
        self.builder.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};
    use crate::syntax::kinds::TmcKind;
    use crate::syntax::layout::layout;
    use mtc_core::syntax::{SyntaxNode, debug_dump};

    fn kind_name(kind: mtc_core::syntax::SyntaxKind) -> String {
        super::super::kind_name(kind).to_string()
    }

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

    /// `token` refuses to re-emit an already-taken significant position.
    /// The second call's own `flush(0)` is a no-op (idempotent — see
    /// `flush`'s doc comment), so by the time it reaches
    /// `mem::take(&mut entries[0].text)` that slot is already `""` and
    /// the assert fires. This is the guard that (mis-aimed, at the Eof
    /// entry rather than a re-taken one) is what actually caught Step
    /// 1's index-drift bug during development — see
    /// `a_hand_driven_sink_reproduces_the_source`'s doc comment.
    #[test]
    #[cfg(debug_assertions)] // the guard is a debug_assert; release strips it
    #[should_panic(expected = "emitted twice")]
    fn a_token_emitted_twice_is_caught() {
        let entries = vec![SigLayout {
            text: "a".to_string(),
            trivia_before: vec![],
        }];
        let mut sink = GreenSink::new(entries);
        sink.start(TmcKind::Root);
        sink.token(0, TmcKind::Ident);
        sink.token(0, TmcKind::Ident);
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

    /// Direct unit exercise of the sink alone (no lexer/layout schedule
    /// involved beyond a two-entry stub): `IDENT@0..1 "a"` with one
    /// space of leading trivia before it, checkpoint-wrapped
    /// retroactively into a NAMESPACE node. Ported from the sibling
    /// `.pmc` crate's test of the same name — `PmcKind::File` becomes
    /// `TmcKind::Root`, everything else carries over unchanged. Also
    /// covers `flush`'s idempotence directly: `flush(0)` runs first,
    /// then `token(0, ..)` flushes the SAME position again before
    /// taking the text — a no-op on the trivia side, proven by the
    /// dump showing the leading space exactly once.
    #[test]
    fn sink_builds_a_minimal_tree_with_checkpoint_wrap() {
        let entries = vec![
            SigLayout {
                text: "a".to_string(),
                trivia_before: vec![(TmcKind::Whitespace, " ".to_string())],
            },
            SigLayout {
                text: String::new(),
                trivia_before: vec![],
            },
        ];
        let mut sink = GreenSink::new(entries);
        sink.start(TmcKind::Root);
        let cp = sink.checkpoint();
        sink.flush(0);
        sink.start_at(cp, TmcKind::Namespace);
        sink.token(0, TmcKind::Ident);
        sink.finish(); // Namespace
        let tree = sink.finish_tree(1);
        let root = SyntaxNode::new_root(tree);
        assert_eq!(root.text(), " a");
        let dump = debug_dump(&root, &kind_name);
        assert_eq!(
            dump,
            "ROOT@0..2\n  NAMESPACE@0..2\n    WHITESPACE@0..1 \" \"\n    IDENT@1..2 \"a\"\n"
        );
    }
}
