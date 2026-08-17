//! Red layer — docs/core.md (syntax tree). Position-carrying cursors
//! over a green tree. A `SyntaxNode` is a cheap-to-clone `Rc` handle
//! knowing its absolute byte range, parent, and children; red nodes are
//! created on demand while walking, never stored in the green tree.
//! Equality is positional identity: the same green node at the same
//! offset.

use std::rc::Rc;

use super::green::{GreenElement, GreenNode, GreenToken, SyntaxKind};

/// A half-open byte range `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    pub start: u32,
    pub end: u32,
}

impl TextRange {
    pub fn new(start: u32, end: u32) -> TextRange {
        assert!(start <= end);
        TextRange { start, end }
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn contains(&self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }
}

#[derive(Debug, Clone)]
pub struct SyntaxNode(Rc<NodeData>);

#[derive(Debug)]
struct NodeData {
    green: Rc<GreenNode>,
    parent: Option<SyntaxNode>,
    offset: u32,
}

/// Positional identity: the same green node at the same offset.
/// Meaningful within one tree — but note that two nodes from
/// *different* trees that happen to share a green `Rc` (structure
/// sharing) at equal offsets compare equal too.
impl PartialEq for SyntaxNode {
    fn eq(&self, other: &SyntaxNode) -> bool {
        self.0.offset == other.0.offset && Rc::ptr_eq(&self.0.green, &other.0.green)
    }
}

impl Eq for SyntaxNode {}

impl SyntaxNode {
    pub fn new_root(green: Rc<GreenNode>) -> SyntaxNode {
        SyntaxNode(Rc::new(NodeData {
            green,
            parent: None,
            offset: 0,
        }))
    }

    pub fn kind(&self) -> SyntaxKind {
        self.0.green.kind()
    }

    pub fn text_range(&self) -> TextRange {
        TextRange::new(self.0.offset, self.0.offset + self.0.green.text_len())
    }

    pub fn parent(&self) -> Option<SyntaxNode> {
        self.0.parent.clone()
    }

    pub fn green(&self) -> &Rc<GreenNode> {
        &self.0.green
    }

    pub fn text(&self) -> String {
        self.0.green.text()
    }

    /// All children — nodes and tokens — in document order.
    pub fn children_with_tokens(&self) -> impl Iterator<Item = SyntaxElement> + '_ {
        let mut offset = self.0.offset;
        self.0.green.children().iter().map(move |c| {
            let at = offset;
            offset += c.text_len();
            match c {
                GreenElement::Node(n) => SyntaxElement::Node(SyntaxNode(Rc::new(NodeData {
                    green: n.clone(),
                    parent: Some(self.clone()),
                    offset: at,
                }))),
                GreenElement::Token(t) => SyntaxElement::Token(SyntaxToken {
                    green: t.clone(),
                    parent: self.clone(),
                    offset: at,
                }),
            }
        })
    }

    /// Child NODES only, in document order.
    pub fn children(&self) -> impl Iterator<Item = SyntaxNode> + '_ {
        self.children_with_tokens().filter_map(|e| match e {
            SyntaxElement::Node(n) => Some(n),
            SyntaxElement::Token(_) => None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SyntaxToken {
    green: Rc<GreenToken>,
    parent: SyntaxNode,
    offset: u32,
}

impl PartialEq for SyntaxToken {
    fn eq(&self, other: &SyntaxToken) -> bool {
        self.offset == other.offset && Rc::ptr_eq(&self.green, &other.green)
    }
}

impl Eq for SyntaxToken {}

impl SyntaxToken {
    pub fn kind(&self) -> SyntaxKind {
        self.green.kind()
    }

    pub fn text(&self) -> &str {
        self.green.text()
    }

    pub fn text_range(&self) -> TextRange {
        TextRange::new(self.offset, self.offset + self.green.text_len())
    }

    pub fn parent(&self) -> SyntaxNode {
        self.parent.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxElement {
    Node(SyntaxNode),
    Token(SyntaxToken),
}

impl SyntaxElement {
    pub fn kind(&self) -> SyntaxKind {
        match self {
            SyntaxElement::Node(n) => n.kind(),
            SyntaxElement::Token(t) => t.kind(),
        }
    }

    pub fn text_range(&self) -> TextRange {
        match self {
            SyntaxElement::Node(n) => n.text_range(),
            SyntaxElement::Token(t) => t.text_range(),
        }
    }
}

/// Render an indented tree dump for debugging and golden tests. Core
/// knows no kind names — the caller supplies them.
pub fn debug_dump(node: &SyntaxNode, kind_name: &dyn Fn(SyntaxKind) -> String) -> String {
    let mut out = String::new();
    dump_node(node, kind_name, 0, &mut out);
    out
}

fn dump_node(
    node: &SyntaxNode,
    kind_name: &dyn Fn(SyntaxKind) -> String,
    depth: usize,
    out: &mut String,
) {
    let range = node.text_range();
    out.push_str(&"  ".repeat(depth));
    out.push_str(&format!(
        "{}@{}..{}\n",
        kind_name(node.kind()),
        range.start,
        range.end
    ));
    for e in node.children_with_tokens() {
        match e {
            SyntaxElement::Node(n) => dump_node(&n, kind_name, depth + 1, out),
            SyntaxElement::Token(t) => {
                let range = t.text_range();
                out.push_str(&"  ".repeat(depth + 1));
                out.push_str(&format!(
                    "{}@{}..{} {:?}\n",
                    kind_name(t.kind()),
                    range.start,
                    range.end,
                    t.text()
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{SyntaxKind, TreeBuilder};

    const ROOT: SyntaxKind = SyntaxKind(0);
    const WS: SyntaxKind = SyntaxKind(1);
    const IDENT: SyntaxKind = SyntaxKind(2);
    const LIST: SyntaxKind = SyntaxKind(3);

    /// `f λx` with the two idents wrapped in a LIST: exercises offsets
    /// across a multi-byte char, node/token interleaving, and parents.
    fn sample() -> SyntaxNode {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        b.start_node(LIST);
        b.token(IDENT, "f");
        b.token(WS, " ");
        b.token(IDENT, "λx");
        b.finish_node();
        b.token(WS, "\n");
        b.finish_node();
        SyntaxNode::new_root(b.finish())
    }

    #[test]
    fn ranges_are_absolute_byte_ranges() {
        let root = sample();
        assert_eq!(root.text_range(), TextRange::new(0, 6)); // "f λx\n" = 1+1+3+1
        let list = root.children().next().expect("LIST child");
        assert_eq!(list.kind(), LIST);
        assert_eq!(list.text_range(), TextRange::new(0, 5));
        assert_eq!(list.text(), "f λx");

        let tokens: Vec<SyntaxToken> = list
            .children_with_tokens()
            .filter_map(|e| match e {
                SyntaxElement::Token(t) => Some(t),
                SyntaxElement::Node(_) => None,
            })
            .collect();
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[2].text(), "λx");
        assert_eq!(tokens[2].text_range(), TextRange::new(2, 5));
    }

    #[test]
    fn parent_links_go_back_up() {
        let root = sample();
        let list = root.children().next().expect("LIST child");
        assert_eq!(list.parent().expect("has parent"), root);
        assert!(root.parent().is_none());
        let first_tok = list.children_with_tokens().next().expect("has children");
        assert_eq!(first_tok.text_range(), TextRange::new(0, 1));
    }

    #[test]
    fn equality_is_positional_identity() {
        let root = sample();
        let a = root.children().next().expect("LIST");
        let b = root.children().next().expect("LIST again");
        assert_eq!(a, b); // same green node, same offset
        assert_ne!(a, root);
    }

    #[test]
    fn children_ranges_tile_the_parent() {
        let root = sample();
        let mut cursor = root.text_range().start;
        for e in root.children_with_tokens() {
            assert_eq!(e.text_range().start, cursor);
            cursor = e.text_range().end;
        }
        assert_eq!(cursor, root.text_range().end);
    }

    fn fake_kind_name(kind: SyntaxKind) -> String {
        match kind {
            ROOT => "ROOT",
            WS => "WS",
            IDENT => "IDENT",
            LIST => "LIST",
            _ => unreachable!("sample() uses only the fake kinds above"),
        }
        .to_owned()
    }

    #[test]
    fn debug_dump_renders_an_indented_tree() {
        let root = sample();
        let dump = debug_dump(&root, &fake_kind_name);
        let expected = [
            "ROOT@0..6",
            "  LIST@0..5",
            "    IDENT@0..1 \"f\"",
            "    WS@1..2 \" \"",
            "    IDENT@2..5 \"λx\"",
            "  WS@5..6 \"\\n\"",
            "",
        ]
        .join("\n");
        assert_eq!(dump, expected);
    }
}
