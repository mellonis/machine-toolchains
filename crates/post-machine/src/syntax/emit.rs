//! Green emission for the existing `.pmc` parser: a `TreeBuilder` fed
//! from a [`super::SigLayout`] schedule. The parser stays the single
//! owner of grammar decisions — the sink only mirrors token
//! consumption and node boundaries, so the green tree and the parser's
//! errors can never disagree (docs/core.md (syntax tree)).

use std::rc::Rc;

use mtc_core::syntax::{Checkpoint, GreenNode, TreeBuilder};

use super::kinds::PmcKind;
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
        for (kind, text) in std::mem::take(&mut self.entries[pos].trivia_before) {
            self.builder.token(kind.into(), text);
        }
        self.flushed_upto = pos + 1;
    }

    /// Flush, then emit significant token `pos` verbatim.
    pub fn token(&mut self, pos: usize, kind: PmcKind) {
        self.flush(pos);
        let text = std::mem::take(&mut self.entries[pos].text);
        debug_assert!(!text.is_empty(), "significant token {pos} emitted twice");
        self.builder.token(kind.into(), text);
    }

    pub fn start(&mut self, kind: PmcKind) {
        self.builder.start_node(kind.into());
    }

    pub fn finish(&mut self) {
        self.builder.finish_node();
    }

    pub fn checkpoint(&self) -> Checkpoint {
        self.builder.checkpoint()
    }

    pub fn start_at(&mut self, cp: Checkpoint, kind: PmcKind) {
        self.builder.start_node_at(cp, kind.into());
    }

    /// Emit the trailing trivia (the Eof entry's schedule) and close.
    /// Call with the FILE node still open — this flushes the tail,
    /// finishes FILE, and closes the build.
    pub fn into_tree(mut self, eof_pos: usize) -> Rc<GreenNode> {
        self.flush(eof_pos);
        self.builder.finish_node();
        self.builder.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtc_core::syntax::{SyntaxNode, debug_dump};

    fn kind_name(kind: mtc_core::syntax::SyntaxKind) -> String {
        super::super::kind_name(kind).to_string()
    }

    /// Direct unit exercise of the sink alone (no parser involved):
    /// `IDENT@0..1 "a"` with one space of leading trivia before it,
    /// checkpoint-wrapped retroactively into a NAMESPACE node.
    #[test]
    fn sink_builds_a_minimal_tree_with_checkpoint_wrap() {
        let entries = vec![
            SigLayout {
                text: "a".to_string(),
                trivia_before: vec![(PmcKind::Whitespace, " ".to_string())],
            },
            SigLayout {
                text: String::new(),
                trivia_before: vec![],
            },
        ];
        let mut sink = GreenSink::new(entries);
        sink.start(PmcKind::File);
        let cp = sink.checkpoint();
        sink.flush(0);
        sink.start_at(cp, PmcKind::Namespace);
        sink.token(0, PmcKind::Ident);
        sink.finish(); // Namespace
        let tree = sink.into_tree(1);
        let root = SyntaxNode::new_root(tree);
        assert_eq!(root.text(), " a");
        let dump = debug_dump(&root, &kind_name);
        assert_eq!(
            dump,
            "FILE@0..2\n  NAMESPACE@0..2\n    WHITESPACE@0..1 \" \"\n    IDENT@1..2 \"a\"\n"
        );
    }
}
