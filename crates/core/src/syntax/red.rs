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
    index: u32,
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
            index: 0,
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
        let mut index = 0u32;
        self.0.green.children().iter().map(move |c| {
            let at = offset;
            let idx = index;
            offset += c.text_len();
            index += 1;
            match c {
                GreenElement::Node(n) => SyntaxElement::Node(SyntaxNode(Rc::new(NodeData {
                    green: n.clone(),
                    parent: Some(self.clone()),
                    offset: at,
                    index: idx,
                }))),
                GreenElement::Token(t) => SyntaxElement::Token(SyntaxToken {
                    green: t.clone(),
                    parent: self.clone(),
                    offset: at,
                    index: idx,
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

    /// Parent chain, nearest first; does not include `self`.
    pub fn ancestors(&self) -> impl Iterator<Item = SyntaxNode> + '_ {
        std::iter::successors(self.parent(), SyntaxNode::parent)
    }

    /// The child element at `idx`, constructed alone: the offset comes
    /// from summing the PRECEDING green children's lengths — u32
    /// additions, no Rc-backed red element per skipped child, which is
    /// what `children_with_tokens().nth(idx)` would allocate. The
    /// sibling queries below loop from extraction's per-declaration
    /// trivia walks and fmt's trivia queries, so their per-call
    /// allocation count is what keeps top-level walks linear.
    fn nth_child_element(&self, idx: usize) -> Option<SyntaxElement> {
        let children = self.0.green.children();
        let target = children.get(idx)?;
        let mut offset = self.0.offset;
        for c in &children[..idx] {
            offset += c.text_len();
        }
        Some(match target {
            GreenElement::Node(n) => SyntaxElement::Node(SyntaxNode(Rc::new(NodeData {
                green: n.clone(),
                parent: Some(self.clone()),
                offset,
                index: idx as u32,
            }))),
            GreenElement::Token(t) => SyntaxElement::Token(SyntaxToken {
                green: t.clone(),
                parent: self.clone(),
                offset,
                index: idx as u32,
            }),
        })
    }

    /// The element immediately before this node among its parent's
    /// children, tokens included.
    pub fn prev_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let idx = self.0.index as usize;
        if idx == 0 {
            return None;
        }
        parent.nth_child_element(idx - 1)
    }

    /// The element immediately after this node among its parent's
    /// children, tokens included.
    pub fn next_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        parent.nth_child_element(self.0.index as usize + 1)
    }

    /// First token of the subtree, in document order.
    pub fn first_token(&self) -> Option<SyntaxToken> {
        self.children_with_tokens().find_map(|e| match e {
            SyntaxElement::Token(t) => Some(t),
            SyntaxElement::Node(n) => n.first_token(),
        })
    }

    /// Last token of the subtree, in document order — one REVERSE walk
    /// over the green children per level (offsets tracked by
    /// subtracting lengths from the node's own end), descending only
    /// into the nodes actually visited, so a non-empty tree pays
    /// O(depth) constructions rather than materializing every child at
    /// every level.
    pub fn last_token(&self) -> Option<SyntaxToken> {
        let children = self.0.green.children();
        let mut offset = self.0.offset + self.0.green.text_len();
        for (idx, c) in children.iter().enumerate().rev() {
            offset -= c.text_len();
            match c {
                GreenElement::Token(t) => {
                    return Some(SyntaxToken {
                        green: t.clone(),
                        parent: self.clone(),
                        offset,
                        index: idx as u32,
                    });
                }
                GreenElement::Node(n) => {
                    let node = SyntaxNode(Rc::new(NodeData {
                        green: n.clone(),
                        parent: Some(self.clone()),
                        offset,
                        index: idx as u32,
                    }));
                    if let Some(t) = node.last_token() {
                        return Some(t);
                    }
                }
            }
        }
        None
    }

    /// Every token of the subtree, document order, all depths.
    pub fn descendant_tokens(&self) -> impl Iterator<Item = SyntaxToken> {
        let mut stack: Vec<SyntaxElement> = self
            .children_with_tokens()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        std::iter::from_fn(move || {
            loop {
                match stack.pop()? {
                    SyntaxElement::Token(t) => return Some(t),
                    SyntaxElement::Node(n) => {
                        let mut children: Vec<SyntaxElement> = n.children_with_tokens().collect();
                        children.reverse();
                        stack.extend(children);
                    }
                }
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct SyntaxToken {
    green: Rc<GreenToken>,
    parent: SyntaxNode,
    offset: u32,
    index: u32,
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

    /// The element immediately before this token among its parent's
    /// children, tokens included.
    pub fn prev_sibling_or_token(&self) -> Option<SyntaxElement> {
        let idx = self.index as usize;
        if idx == 0 {
            return None;
        }
        self.parent.nth_child_element(idx - 1)
    }

    /// The element immediately after this token among its parent's
    /// children, tokens included.
    pub fn next_sibling_or_token(&self) -> Option<SyntaxElement> {
        self.parent.nth_child_element(self.index as usize + 1)
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

    #[test]
    fn ancestors_walk_to_the_root() {
        let root = sample();
        let list = root.children().next().expect("LIST child");
        let chain: Vec<SyntaxKind> = list.ancestors().map(|n| n.kind()).collect();
        assert_eq!(chain, vec![ROOT]);
        assert!(root.ancestors().next().is_none());
    }

    #[test]
    fn sibling_queries_walk_both_ways() {
        let root = sample();
        let list = root.children().next().expect("LIST child");
        // sample(): ROOT > [LIST, WS("\n")]
        let next = list.next_sibling_or_token().expect("has next");
        assert_eq!(next.kind(), WS);
        assert!(matches!(
            next,
            SyntaxElement::Token(ref t) if t.text() == "\n"
        ));
        let SyntaxElement::Token(ws) = next else {
            unreachable!()
        };
        let prev = ws.prev_sibling_or_token().expect("has prev");
        assert_eq!(prev.kind(), LIST);
        assert!(list.prev_sibling_or_token().is_none());
    }

    #[test]
    fn token_edges_and_descendant_tokens() {
        let root = sample();
        // sample() text: "f λx\n" — tokens f, " ", "λx" inside LIST; "\n" in ROOT.
        assert_eq!(root.first_token().expect("first").text(), "f");
        assert_eq!(root.last_token().expect("last").text(), "\n");
        let texts: Vec<String> = root
            .descendant_tokens()
            .map(|t| t.text().to_string())
            .collect();
        assert_eq!(texts, vec!["f", " ", "λx", "\n"]);
    }

    /// The addressed child constructor agrees with the iterator on every
    /// index — kind, range (offset arithmetic included, with a multibyte
    /// token in play), and both sibling directions from every position.
    /// This is the equivalence pin for the O(index) sibling queries: they
    /// sum green lengths instead of materializing every earlier child,
    /// and must land on exactly the element the full walk yields.
    #[test]
    fn addressed_children_agree_with_the_iterator_walk() {
        let root = sample();
        for holder in [root.clone()]
            .into_iter()
            .chain(root.children_with_tokens().filter_map(|e| match e {
                SyntaxElement::Node(n) => Some(n),
                SyntaxElement::Token(_) => None,
            }))
        {
            let walked: Vec<SyntaxElement> = holder.children_with_tokens().collect();
            for (i, e) in walked.iter().enumerate() {
                let addressed = holder.nth_child_element(i).expect("in range");
                assert_eq!(addressed.kind(), e.kind());
                assert_eq!(addressed.text_range(), e.text_range());
                let prev = match e {
                    SyntaxElement::Node(n) => n.prev_sibling_or_token(),
                    SyntaxElement::Token(t) => t.prev_sibling_or_token(),
                };
                let next = match e {
                    SyntaxElement::Node(n) => n.next_sibling_or_token(),
                    SyntaxElement::Token(t) => t.next_sibling_or_token(),
                };
                assert_eq!(
                    prev.map(|p| p.text_range()),
                    i.checked_sub(1).map(|j| walked[j].text_range())
                );
                assert_eq!(
                    next.map(|n| n.text_range()),
                    walked.get(i + 1).map(|n| n.text_range())
                );
            }
            assert!(holder.nth_child_element(walked.len()).is_none());
        }
    }
}
