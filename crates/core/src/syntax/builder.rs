//! `TreeBuilder`: the emission surface a recursive-descent parser
//! drives. `start_node`/`finish_node` bracket children in document
//! order; `checkpoint` + `start_node_at` retro-wrap an already-emitted
//! prefix once the parser knows the enclosing kind. Balance errors are
//! panics: an unbalanced build is a parser bug, never an input error.

use std::rc::Rc;

use super::green::{GreenElement, GreenNode, GreenToken, SyntaxKind};

/// A mark into the builder's pending children; see
/// [`TreeBuilder::start_node_at`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint(usize);

#[derive(Debug, Default)]
pub struct TreeBuilder {
    /// Open frames: (kind, index into `children` where the frame began).
    parents: Vec<(SyntaxKind, usize)>,
    /// Pending children of the innermost open frame(s), flat until a
    /// `finish_node` folds a suffix into a node.
    children: Vec<GreenElement>,
}

impl TreeBuilder {
    pub fn new() -> TreeBuilder {
        TreeBuilder::default()
    }

    pub fn token(&mut self, kind: SyntaxKind, text: &str) {
        self.children
            .push(GreenElement::Token(GreenToken::new(kind, text)));
    }

    pub fn start_node(&mut self, kind: SyntaxKind) {
        self.parents.push((kind, self.children.len()));
    }

    pub fn finish_node(&mut self) {
        let (kind, first) = self
            .parents
            .pop()
            .expect("finish_node without a matching start_node");
        let node = GreenNode::new(kind, self.children.split_off(first));
        self.children.push(GreenElement::Node(node));
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint(self.children.len())
    }

    /// Open a node RETROACTIVELY at `cp`: everything emitted since the
    /// checkpoint becomes the new node's leading children. The
    /// checkpoint must not predate the innermost open frame.
    pub fn start_node_at(&mut self, cp: Checkpoint, kind: SyntaxKind) {
        assert!(
            cp.0 <= self.children.len(),
            "checkpoint points past the emitted children"
        );
        if let Some(&(_, first)) = self.parents.last() {
            assert!(
                cp.0 >= first,
                "checkpoint predates the innermost open frame"
            );
        }
        self.parents.push((kind, cp.0));
    }

    /// Close the build. Exactly one root node must remain.
    pub fn finish(mut self) -> Rc<GreenNode> {
        assert!(self.parents.is_empty(), "unfinished node at finish()");
        assert!(
            self.children.len() == 1,
            "finish() requires exactly one root element"
        );
        match self.children.pop().expect("length checked above") {
            GreenElement::Node(n) => n,
            GreenElement::Token(_) => panic!("the root must be a node, not a token"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{GreenElement, SyntaxKind};

    const ROOT: SyntaxKind = SyntaxKind(0);
    const WS: SyntaxKind = SyntaxKind(1);
    const IDENT: SyntaxKind = SyntaxKind(2);
    const COMMA: SyntaxKind = SyntaxKind(3);
    const LIST: SyntaxKind = SyntaxKind(4);

    #[test]
    fn builds_nested_tree_in_document_order() {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        b.start_node(LIST);
        b.token(IDENT, "a");
        b.token(COMMA, ",");
        b.token(WS, " ");
        b.token(IDENT, "b");
        b.finish_node();
        b.token(WS, "\n");
        b.finish_node();
        let root = b.finish();
        assert_eq!(root.text(), "a, b\n");
        assert_eq!(root.children().len(), 2);
        assert_eq!(root.children()[0].kind(), LIST);
    }

    #[test]
    fn checkpoint_wraps_an_already_emitted_prefix() {
        // The parser emits `a` before knowing a comma follows; the
        // checkpoint retro-wraps `a` into the LIST node — the standard
        // fix for "this is only a list once I see the comma".
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        let cp = b.checkpoint();
        b.token(IDENT, "a");
        b.start_node_at(cp, LIST);
        b.token(COMMA, ",");
        b.token(IDENT, "b");
        b.finish_node();
        b.finish_node();
        let root = b.finish();
        assert_eq!(root.text(), "a,b");
        let GreenElement::Node(list) = &root.children()[0] else {
            panic!("expected the LIST node first");
        };
        assert_eq!(list.kind(), LIST);
        assert_eq!(list.text(), "a,b");
    }

    #[test]
    #[should_panic(expected = "unfinished node")]
    fn finish_with_open_frame_panics() {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        let _ = b.finish();
    }

    #[test]
    #[should_panic(expected = "checkpoint predates")]
    fn checkpoint_predating_an_open_frame_panics() {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        let cp = b.checkpoint();
        b.token(IDENT, "a");
        b.start_node(LIST); // opens AFTER the checkpoint's position
        b.start_node_at(cp, LIST);
    }
}
