//! Green layer of the syntax framework — docs/core.md (syntax tree).
//! Immutable, structure-shared value trees. A green tree owns its exact
//! source text and knows nothing about absolute positions — the red
//! layer (`red.rs`) adds offsets on top. Whitespace and comments are
//! ordinary tokens; there is no side-channel trivia storage, so the
//! lossless contract is one law: the root's `text()` equals the source,
//! byte for byte.
//!
//! Language-agnostic: kinds are opaque `u16`s owned by the language
//! crates, mirroring how `vm::Arch` owns its opcodes.

use std::rc::Rc;

/// An opaque syntax-kind tag. Each language crate owns its kind space
/// (tokens and nodes share one space); core only ever compares kinds
/// for equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SyntaxKind(pub u16);

/// A leaf: one token and its exact source text.
#[derive(Debug, PartialEq, Eq)]
pub struct GreenToken {
    kind: SyntaxKind,
    text: String,
}

impl GreenToken {
    pub fn new(kind: SyntaxKind, text: impl Into<String>) -> Rc<GreenToken> {
        Rc::new(GreenToken {
            kind,
            text: text.into(),
        })
    }

    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Length in BYTES (offsets across the framework are byte offsets;
    /// line/col conversion happens in `TextLineIndex`).
    pub fn text_len(&self) -> u32 {
        self.text.len() as u32
    }
}

/// One child slot of a green node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreenElement {
    Node(Rc<GreenNode>),
    Token(Rc<GreenToken>),
}

impl GreenElement {
    pub fn kind(&self) -> SyntaxKind {
        match self {
            GreenElement::Node(n) => n.kind(),
            GreenElement::Token(t) => t.kind(),
        }
    }

    pub fn text_len(&self) -> u32 {
        match self {
            GreenElement::Node(n) => n.text_len(),
            GreenElement::Token(t) => t.text_len(),
        }
    }
}

/// An interior node: a kind plus children, with the subtree's total
/// text length cached so red-layer offset math is O(children), never
/// O(subtree).
#[derive(Debug, PartialEq, Eq)]
pub struct GreenNode {
    kind: SyntaxKind,
    text_len: u32,
    children: Vec<GreenElement>,
}

impl GreenNode {
    pub fn new(kind: SyntaxKind, children: Vec<GreenElement>) -> Rc<GreenNode> {
        let text_len = children.iter().map(GreenElement::text_len).sum();
        Rc::new(GreenNode {
            kind,
            text_len,
            children,
        })
    }

    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    pub fn text_len(&self) -> u32 {
        self.text_len
    }

    pub fn children(&self) -> &[GreenElement] {
        &self.children
    }

    /// The subtree's full text.
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(self.text_len as usize);
        self.write_text(&mut out);
        out
    }

    fn write_text(&self, out: &mut String) {
        for c in &self.children {
            match c {
                GreenElement::Token(t) => out.push_str(t.text()),
                GreenElement::Node(n) => n.write_text(out),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    const ROOT: SyntaxKind = SyntaxKind(0);
    const WS: SyntaxKind = SyntaxKind(1);
    const IDENT: SyntaxKind = SyntaxKind(2);
    const LIST: SyntaxKind = SyntaxKind(3);

    #[test]
    fn green_tree_reproduces_text_and_caches_len() {
        let inner = GreenNode::new(
            LIST,
            vec![
                GreenElement::Token(GreenToken::new(IDENT, "ab")),
                GreenElement::Token(GreenToken::new(WS, " ")),
                GreenElement::Token(GreenToken::new(IDENT, "cd")),
            ],
        );
        assert_eq!(inner.text_len(), 5);
        let root = GreenNode::new(
            ROOT,
            vec![
                GreenElement::Node(inner),
                GreenElement::Token(GreenToken::new(WS, "\n")),
            ],
        );
        assert_eq!(root.kind(), ROOT);
        assert_eq!(root.text_len(), 6);
        assert_eq!(root.text(), "ab cd\n");
        assert_eq!(root.children().len(), 2);
    }

    #[test]
    fn text_len_counts_bytes_not_chars() {
        // Offsets across the framework are byte offsets; `TextLineIndex`
        // owns the char-column conversion.
        let t = GreenToken::new(IDENT, "λ");
        assert_eq!(t.text_len(), 2);
    }

    #[test]
    fn structure_sharing_is_by_rc() {
        let shared = GreenToken::new(IDENT, "x");
        let a = GreenNode::new(LIST, vec![GreenElement::Token(shared.clone())]);
        let b = GreenNode::new(LIST, vec![GreenElement::Token(shared.clone())]);
        assert_eq!(a, b);
        assert_eq!(Rc::strong_count(&shared), 3);
    }
}
