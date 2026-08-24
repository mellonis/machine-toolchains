//! Typed views over the `.tmc` green tree (docs/core.md (syntax tree)):
//! zero-copy wrappers over one `super::kinds::TmcKind` node kind each,
//! declared with the core `ast_node!` macro. Each view accepts exactly
//! the node kind it names and refuses every other — the cast contract
//! every accessor built on top of this layer rests on. This file is
//! the type layer only; accessors are added kind by kind on top of it.

use mtc_core::ast_node;
use mtc_core::syntax::{AstNode, SyntaxNode};

use super::kinds::TmcKind;

ast_node!(pub struct RootView: TmcKind::Root.into());
ast_node!(pub struct UseView: TmcKind::Use.into());
ast_node!(pub struct UsePathView: TmcKind::UsePath.into());
ast_node!(pub struct AlphabetView: TmcKind::Alphabet.into());
ast_node!(pub struct ReuseView: TmcKind::Reuse.into());
ast_node!(pub struct MachineView: TmcKind::Machine.into());
ast_node!(pub struct NamespaceView: TmcKind::Namespace.into());
ast_node!(pub struct WorldView: TmcKind::World.into());
ast_node!(pub struct TapeView: TmcKind::Tape.into());
ast_node!(pub struct StateView: TmcKind::State.into());
ast_node!(pub struct RuleView: TmcKind::Rule.into());
ast_node!(pub struct GraftView: TmcKind::Graft.into());
ast_node!(pub struct BindView: TmcKind::Bind.into());
ast_node!(pub struct DocRunView: TmcKind::DocRun.into());
ast_node!(pub struct AttrView: TmcKind::Attr.into());

/// One item that can appear at file level or inside a `NAMESPACE`
/// body — the five kinds the grammar allows there: `use`, `alphabet`,
/// `machine`, a nested `namespace` itself, and `reuse` (both `routine`
/// and `graph` — the two reusable-graph carriers — parse to the same
/// `REUSE` node kind, distinguished by their own header token, not by
/// kind).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopView {
    Use(UseView),
    Alphabet(AlphabetView),
    Reuse(ReuseView),
    Machine(MachineView),
    Namespace(NamespaceView),
}

impl TopView {
    /// Cast one node to whichever top-level kind it is; `None` for
    /// anything else.
    pub fn cast(node: SyntaxNode) -> Option<Self> {
        UseView::cast(node.clone())
            .map(TopView::Use)
            .or_else(|| AlphabetView::cast(node.clone()).map(TopView::Alphabet))
            .or_else(|| ReuseView::cast(node.clone()).map(TopView::Reuse))
            .or_else(|| MachineView::cast(node.clone()).map(TopView::Machine))
            .or_else(|| NamespaceView::cast(node).map(TopView::Namespace))
    }
}
