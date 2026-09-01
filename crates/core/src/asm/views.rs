//! Typed views over the assembly green tree (docs/core.md (syntax
//! trees)) and the pairing that locates every CST item in it.
//!
//! The views are zero-copy wrappers over one [`AsmKind`] node kind
//! each, declared with the core `ast_node!` macro: `RootView::items`
//! and `ReptView::body` walk the item nodes the emitter brackets, kind
//! by kind. They name the shape the tree already carries and never
//! re-derive a grammar decision — the CST stays the semantic view
//! every consumer reads.
//!
//! [`locate_items`] is the bridge the language services read positions
//! from: the flattened item walk (a `.rept` header followed by its body
//! items, spliced in place) paired element-by-element with the tree — a
//! node per non-comment item, an OWN-LINE comment token per comment
//! item. That is the position source that replaced the services' own
//! reconstruction (a zip against non-blank lines, a cursor walk over
//! blocks): the tree is lossless, so a comment has a byte range like
//! any other item. The pairing is exact by construction — the tree and
//! the CST come out of the one shaping walk — and a mismatch is a
//! parser bug, which panics the way the framework's balance errors do;
//! the laws that pin it away live in the crate's `asm_green` tests.

use crate::ast_node;
use crate::syntax::{AstNode, SyntaxElement, SyntaxNode, SyntaxToken, TextRange};

use super::cst::{AsmCst, AsmItem, AsmItemKind};
use super::kinds::AsmKind;

ast_node!(pub struct RootView: AsmKind::Root.into());
ast_node!(pub struct LineView: AsmKind::Line.into());
ast_node!(pub struct FuncView: AsmKind::Func.into());
ast_node!(pub struct RawView: AsmKind::Raw.into());
ast_node!(pub struct SectionView: AsmKind::Section.into());
ast_node!(pub struct TableDirectiveView: AsmKind::TableDirective.into());
ast_node!(pub struct ReptView: AsmKind::Rept.into());
ast_node!(pub struct RoutineDirectiveView: AsmKind::RoutineDirective.into());
ast_node!(pub struct VolatileView: AsmKind::Volatile.into());
ast_node!(pub struct FrameDirectiveView: AsmKind::FrameDirective.into());

/// One item node, in whichever of the nine shapes the emitter gave it —
/// the same set as [`AsmItemKind`] minus `Comment`, which is trivia in
/// the tree, not a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemView {
    Line(LineView),
    Func(FuncView),
    Raw(RawView),
    Section(SectionView),
    TableDirective(TableDirectiveView),
    Rept(ReptView),
    RoutineDirective(RoutineDirectiveView),
    Volatile(VolatileView),
    FrameDirective(FrameDirectiveView),
}

impl ItemView {
    /// Cast one node to whichever item shape it is; `None` for anything
    /// else (only `Root` — nothing else is a node in this kind space,
    /// but the cast stays total rather than assuming it).
    pub fn cast(node: SyntaxNode) -> Option<ItemView> {
        let kind = node.kind();
        Some(if kind == AsmKind::Line.into() {
            ItemView::Line(LineView::cast(node)?)
        } else if kind == AsmKind::Func.into() {
            ItemView::Func(FuncView::cast(node)?)
        } else if kind == AsmKind::Raw.into() {
            ItemView::Raw(RawView::cast(node)?)
        } else if kind == AsmKind::Section.into() {
            ItemView::Section(SectionView::cast(node)?)
        } else if kind == AsmKind::TableDirective.into() {
            ItemView::TableDirective(TableDirectiveView::cast(node)?)
        } else if kind == AsmKind::Rept.into() {
            ItemView::Rept(ReptView::cast(node)?)
        } else if kind == AsmKind::RoutineDirective.into() {
            ItemView::RoutineDirective(RoutineDirectiveView::cast(node)?)
        } else if kind == AsmKind::Volatile.into() {
            ItemView::Volatile(VolatileView::cast(node)?)
        } else if kind == AsmKind::FrameDirective.into() {
            ItemView::FrameDirective(FrameDirectiveView::cast(node)?)
        } else {
            return None;
        })
    }

    pub fn syntax(&self) -> &SyntaxNode {
        match self {
            ItemView::Line(v) => v.syntax(),
            ItemView::Func(v) => v.syntax(),
            ItemView::Raw(v) => v.syntax(),
            ItemView::Section(v) => v.syntax(),
            ItemView::TableDirective(v) => v.syntax(),
            ItemView::Rept(v) => v.syntax(),
            ItemView::RoutineDirective(v) => v.syntax(),
            ItemView::Volatile(v) => v.syntax(),
            ItemView::FrameDirective(v) => v.syntax(),
        }
    }
}

/// The item nodes directly under `node`, in document order — the root's
/// items or a block's body; comments and whitespace are trivia tokens
/// at this level and are not items.
fn item_nodes(node: &SyntaxNode) -> impl Iterator<Item = ItemView> + '_ {
    node.children().filter_map(ItemView::cast)
}

impl RootView {
    pub fn items(&self) -> impl Iterator<Item = ItemView> + '_ {
        item_nodes(self.syntax())
    }
}

impl ReptView {
    /// The block's body items — the nodes between its header tokens and
    /// its `.endr`; a comment-only body line is trivia here too.
    pub fn body(&self) -> impl Iterator<Item = ItemView> + '_ {
        item_nodes(self.syntax())
    }
}

/// A CST item with the byte range the tree gives it: a node's whole
/// extent (a `.rept` block's runs from its header through its `.endr`;
/// a continued list's through its last continued line — trailing
/// comments excluded, they are trivia after the node), or an own-line
/// comment's token.
#[derive(Debug, Clone, Copy)]
pub struct LocatedItem<'a> {
    pub item: &'a AsmItem,
    pub range: TextRange,
}

/// Pair every CST item with its range in `root` — the tree
/// `parse_asm_green` built alongside `cst`. Flattened document order:
/// a `.rept` header, then its body items spliced in place, then what
/// follows the block.
pub fn locate_items<'a>(cst: &'a AsmCst, root: &SyntaxNode) -> Vec<LocatedItem<'a>> {
    let mut out = Vec::new();
    pair(root, &cst.items, &mut out);
    out
}

/// Walk one level of the tree against one level of the CST: `items` is
/// the root's item list, or a block's body under its REPT node.
fn pair<'a>(level: &SyntaxNode, items: &'a [AsmItem], out: &mut Vec<LocatedItem<'a>>) {
    let mut elements = level.children_with_tokens();
    for item in items {
        let element = match &item.kind {
            AsmItemKind::Comment(_) => elements.find(|e| match e {
                SyntaxElement::Token(t) => is_own_line_comment(t),
                SyntaxElement::Node(_) => false,
            }),
            _ => elements.find(|e| matches!(e, SyntaxElement::Node(_))),
        }
        .expect("the tree mirrors the CST by construction (docs/core.md (syntax trees))");
        out.push(LocatedItem {
            item,
            range: element.text_range(),
        });
        if let AsmItemKind::Rept(r) = &item.kind {
            let SyntaxElement::Node(node) = element else {
                unreachable!("a non-comment item pairs with a node")
            };
            debug_assert_eq!(node.kind(), AsmKind::Rept.into());
            pair(&node, &r.body, out);
        }
    }
}

/// An own-line comment: a COMMENT token that starts its line — nothing
/// before it at this level, or a whitespace run that reaches the line
/// start (it holds a newline, or it is itself the first element: the
/// indentation of a file's first line has no newline before it). A
/// trailing comment follows its item's last token, its node, or
/// newline-free whitespace.
fn is_own_line_comment(t: &SyntaxToken) -> bool {
    if t.kind() != AsmKind::Comment.into() {
        return false;
    }
    match t.prev_sibling_or_token() {
        None => true,
        Some(SyntaxElement::Token(p)) => {
            p.kind() == AsmKind::Whitespace.into()
                && (p.text().contains('\n') || p.prev_sibling_or_token().is_none())
        }
        Some(SyntaxElement::Node(_)) => false,
    }
}
