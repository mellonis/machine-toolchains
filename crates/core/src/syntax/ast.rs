//! The typed-view contract — docs/core.md (syntax tree). A view is a
//! zero-copy wrapper over a `SyntaxNode` of a known kind. Concrete
//! views live in the language crates; core owns only the casting
//! contract, the declaration macro, and the child/token lookup helpers
//! views are written with.

use super::green::SyntaxKind;
use super::red::{SyntaxElement, SyntaxNode, SyntaxToken};

pub trait AstNode: Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(node: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
}

/// Declare a typed view struct over one syntax kind:
///
/// ```ignore
/// ast_node!(pub struct FunctionNode: kinds::FUNCTION);
/// ```
#[macro_export]
macro_rules! ast_node {
    ($(#[$attr:meta])* pub struct $name:ident: $kind:expr) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name {
            syntax: $crate::syntax::SyntaxNode,
        }

        impl $crate::syntax::AstNode for $name {
            fn can_cast(kind: $crate::syntax::SyntaxKind) -> bool {
                kind == $kind
            }

            fn cast(node: $crate::syntax::SyntaxNode) -> Option<Self> {
                if <Self as $crate::syntax::AstNode>::can_cast(node.kind()) {
                    Some($name { syntax: node })
                } else {
                    None
                }
            }

            fn syntax(&self) -> &$crate::syntax::SyntaxNode {
                &self.syntax
            }
        }
    };
}

/// First child node castable to `N`.
pub fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
    parent.children().find_map(N::cast)
}

/// All child nodes castable to `N`, in document order.
pub fn children<'a, N: AstNode + 'a>(parent: &'a SyntaxNode) -> impl Iterator<Item = N> + 'a {
    parent.children().filter_map(N::cast)
}

/// First child TOKEN of the given kind (direct children only — a
/// view's own token, never a grandchild's).
pub fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    parent.children_with_tokens().find_map(|e| match e {
        SyntaxElement::Token(t) if t.kind() == kind => Some(t),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::ast_node;
    use crate::syntax::{self, AstNode, SyntaxKind, SyntaxNode, TreeBuilder};

    const ROOT: SyntaxKind = SyntaxKind(0);
    const IDENT: SyntaxKind = SyntaxKind(1);
    const COMMA: SyntaxKind = SyntaxKind(2);
    const LIST: SyntaxKind = SyntaxKind(3);
    const ENTRY: SyntaxKind = SyntaxKind(4);

    ast_node!(pub struct ListView: LIST);
    ast_node!(pub struct EntryView: ENTRY);

    /// `(x,y)`-shaped tree: ROOT > LIST > [ENTRY("x"), COMMA, ENTRY("y")].
    fn sample() -> SyntaxNode {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        b.start_node(LIST);
        b.start_node(ENTRY);
        b.token(IDENT, "x");
        b.finish_node();
        b.token(COMMA, ",");
        b.start_node(ENTRY);
        b.token(IDENT, "y");
        b.finish_node();
        b.finish_node();
        b.finish_node();
        SyntaxNode::new_root(b.finish())
    }

    #[test]
    fn cast_checks_the_kind() {
        let root = sample();
        assert!(ListView::cast(root.clone()).is_none());
        let list_node = root.children().next().expect("LIST child");
        let list = ListView::cast(list_node).expect("casts");
        assert_eq!(list.syntax().kind(), LIST);
    }

    #[test]
    fn child_and_children_find_by_view_type() {
        let root = sample();
        let list: ListView = syntax::child(&root).expect("LIST under ROOT");
        let entries: Vec<EntryView> = syntax::children(list.syntax()).collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].syntax().text(), "x");
        assert_eq!(entries[1].syntax().text(), "y");
    }

    #[test]
    fn token_finds_by_kind() {
        let root = sample();
        let list: ListView = syntax::child(&root).expect("LIST under ROOT");
        let comma = syntax::token(list.syntax(), COMMA).expect("comma token");
        assert_eq!(comma.text(), ",");
        assert!(syntax::token(list.syntax(), IDENT).is_none()); // idents sit inside ENTRY nodes
    }
}
