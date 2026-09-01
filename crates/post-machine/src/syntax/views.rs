//! Typed views over the `.pmc` green tree (docs/core.md (syntax tree)):
//! zero-copy wrappers over one `super::kinds::PmcKind` node kind each,
//! declared with the core `ast_node!` macro. Accessors walk direct
//! children/tokens with the core `child`/`children`/`token` helpers
//! plus the red-tree navigation primitives — a view never re-derives a
//! grammar decision the parser already made, it only names the shape
//! the green tree already carries.

use mtc_core::ast_node;
use mtc_core::syntax::{AstNode, SyntaxElement, SyntaxNode, SyntaxToken, child, children, token};

use super::kinds::PmcKind;

ast_node!(pub struct FileView: PmcKind::File.into());
ast_node!(pub struct UseDeclView: PmcKind::UseDecl.into());
ast_node!(pub struct UsePathView: PmcKind::UsePath.into());
ast_node!(pub struct NamespaceView: PmcKind::Namespace.into());
ast_node!(pub struct FunctionView: PmcKind::Function.into());
ast_node!(pub struct DocRunView: PmcKind::DocRun.into());
ast_node!(pub struct StatementView: PmcKind::Statement.into());
ast_node!(pub struct LabelView: PmcKind::Label.into());
ast_node!(pub struct ItemView: PmcKind::Item.into());

/// One top-level (or namespace-level) item, in document order — the
/// three kinds `FileView::items`/`NamespaceView::items` yield.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopView {
    Use(UseDeclView),
    Namespace(NamespaceView),
    Function(FunctionView),
}

/// Cast one child node to whichever of the three top-level kinds it
/// is; `None` for anything else (nothing else lives at this level, but
/// the helper stays total rather than assuming it).
fn cast_top(node: SyntaxNode) -> Option<TopView> {
    UseDeclView::cast(node.clone())
        .map(TopView::Use)
        .or_else(|| NamespaceView::cast(node.clone()).map(TopView::Namespace))
        .or_else(|| FunctionView::cast(node).map(TopView::Function))
}

/// Direct top-level-shaped children of `node`, in document order —
/// shared by `FileView::items` and `NamespaceView::items`. A child that
/// casts to none of the three kinds is a tree the parser cannot
/// produce: FILE/NAMESPACE children are only USE_DECL / NAMESPACE /
/// FUNCTION, and a bound DOC_RUN is retro-wrapped into its own FUNCTION
/// rather than sitting at this level. Asserted rather than silently
/// filtered, because every consumer of this iterator treats a short
/// list as "the file had fewer items".
fn top_items(node: &SyntaxNode) -> impl Iterator<Item = TopView> + '_ {
    node.children()
        // A resilient parse's recovery region (docs/core.md (syntax
        // trees), error recovery): nothing reads inside one — the
        // items walk skips it whole, so the green-tier features see
        // the declarations around a broken region.
        .filter(|child| child.kind() != PmcKind::Error.into())
        .filter_map(|child| {
            let kind = child.kind();
            let top = cast_top(child);
            debug_assert!(
                top.is_some(),
                "unexpected node kind at top level: {:?}",
                kind
            );
            top
        })
}

impl FileView {
    pub fn items(&self) -> impl Iterator<Item = TopView> + '_ {
        top_items(self.syntax())
    }
}

impl NamespaceView {
    /// The second significant token — the IDENT after the `namespace`
    /// keyword IDENT.
    pub fn name_token(&self) -> SyntaxToken {
        let idents: Vec<SyntaxToken> = self
            .syntax()
            .children_with_tokens()
            .filter_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == PmcKind::Ident.into() => Some(t),
                _ => None,
            })
            .collect();
        debug_assert_eq!(
            idents.len(),
            2,
            "NAMESPACE header is exactly `namespace <name>`: 2 IDENTs, got {}",
            idents.len()
        );
        idents
            .into_iter()
            .nth(1)
            .expect("NAMESPACE always carries a name IDENT after the keyword IDENT")
    }

    pub fn name(&self) -> String {
        self.name_token().text().to_string()
    }

    pub fn items(&self) -> impl Iterator<Item = TopView> + '_ {
        top_items(self.syntax())
    }
}

impl UseDeclView {
    pub fn paths(&self) -> impl Iterator<Item = UsePathView> + '_ {
        children(self.syntax())
    }
}

/// `UsePathView`'s own IDENT/`::` tokens, in order — trivia excluded.
fn use_path_tokens(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t)
                if t.kind() == PmcKind::Ident.into() || t.kind() == PmcKind::ColonColon.into() =>
            {
                Some(t)
            }
            _ => None,
        })
        .collect()
}

/// Splits a `USE_PATH`'s tokens into the `::`-joined path segments and
/// (if present) the alias — the `as`-marker rule: `IDENT (:: IDENT)*`
/// is the path; two more IDENTs, if any, follow — the first is the
/// literal `as` marker, the second is the alias. Shared by `segments`
/// and `alias_token` so the walk is written once.
fn use_path_parts(node: &SyntaxNode) -> (Vec<SyntaxToken>, Option<SyntaxToken>) {
    let toks = use_path_tokens(node);
    let mut segments = Vec::new();
    let mut i = 0;
    if i < toks.len() {
        segments.push(toks[i].clone());
        i += 1;
    }
    while i + 1 < toks.len() && toks[i].kind() == PmcKind::ColonColon.into() {
        segments.push(toks[i + 1].clone());
        i += 2;
    }
    let alias = if i + 1 < toks.len() {
        Some(toks[i + 1].clone())
    } else {
        None
    };
    (segments, alias)
}

impl UsePathView {
    /// The path IDENTs, `as`-alias excluded: `IDENT (:: IDENT)*`.
    pub fn segments(&self) -> Vec<SyntaxToken> {
        use_path_parts(self.syntax()).0
    }

    /// The IDENT after the `as` marker, when present.
    pub fn alias_token(&self) -> Option<SyntaxToken> {
        use_path_parts(self.syntax()).1
    }
}

/// `FunctionView::header`'s result — the contextual-keyword decode over
/// a FUNCTION node's own header tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnHeader {
    pub name: SyntaxToken,
    pub has_volatile: bool,
    pub has_export: bool,
}

impl FunctionView {
    /// The contextual-keyword rule: among the FUNCTION node's
    /// direct-child IDENT tokens before its first `L_PAREN` token, the
    /// LAST one is the name; among the earlier ones, text `"volatile"`
    /// sets `has_volatile` and text `"export"` sets `has_export`
    /// (mirrors the parser's own modifier order). A lone IDENT is
    /// always the name — `export() {}` names a function `export`, it
    /// is not the modifier.
    pub fn header(&self) -> FnHeader {
        let idents: Vec<SyntaxToken> = self
            .syntax()
            .children_with_tokens()
            .take_while(|e| e.kind() != PmcKind::LParen.into())
            .filter_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == PmcKind::Ident.into() => Some(t),
                _ => None,
            })
            .collect();
        let name = idents
            .last()
            .cloned()
            .expect("FUNCTION always carries a name IDENT before its first `(`");
        let mut has_volatile = false;
        let mut has_export = false;
        for t in &idents[..idents.len() - 1] {
            match t.text() {
                "volatile" => has_volatile = true,
                "export" => has_export = true,
                other => debug_assert!(
                    false,
                    "unexpected modifier IDENT {other:?} before the function name — \
                     the parser accepts only `export` and `volatile`"
                ),
            }
        }
        FnHeader {
            name,
            has_volatile,
            has_export,
        }
    }

    pub fn doc_run(&self) -> Option<DocRunView> {
        child(self.syntax())
    }

    pub fn statements(&self) -> impl Iterator<Item = StatementView> + '_ {
        children(self.syntax())
    }

    /// Direct children only — a nested definition's own nested
    /// functions don't surface here.
    pub fn nested(&self) -> impl Iterator<Item = FunctionView> + '_ {
        children(self.syntax())
    }
}

impl StatementView {
    pub fn labels(&self) -> impl Iterator<Item = LabelView> + '_ {
        children(self.syntax())
    }

    pub fn items(&self) -> impl Iterator<Item = ItemView> + '_ {
        children(self.syntax())
    }
}

impl LabelView {
    pub fn number_token(&self) -> SyntaxToken {
        token(self.syntax(), PmcKind::Number.into()).expect("LABEL always carries a NUMBER token")
    }

    pub fn colon_token(&self) -> SyntaxToken {
        token(self.syntax(), PmcKind::Colon.into()).expect("LABEL always carries a COLON token")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_green;
    use mtc_core::syntax::{AstNode, SyntaxNode};

    fn file(src: &str) -> FileView {
        FileView::cast(SyntaxNode::new_root(parse_green(src).expect("parses")))
            .expect("root is FILE")
    }

    #[test]
    fn use_paths_segments_and_alias() {
        let f = file("use std::goToEnd, std::goToBegin as gb;\n");
        let TopView::Use(u) = f.items().next().expect("one item") else {
            panic!("expected use decl");
        };
        let paths: Vec<UsePathView> = u.paths().collect();
        assert_eq!(paths.len(), 2);
        let seg: Vec<String> = paths[0]
            .segments()
            .iter()
            .map(|t| t.text().to_string())
            .collect();
        assert_eq!(seg, vec!["std", "goToEnd"]);
        assert!(paths[0].alias_token().is_none());
        assert_eq!(paths[1].alias_token().expect("alias").text(), "gb");
    }

    /// The `as`-marker rule disambiguates even the degenerate case where
    /// every segment happens to spell the marker's own text: a bare
    /// path `as`, the literal `as` marker, and an alias `as` are told
    /// apart by POSITION, never by comparing text against `"as"`.
    #[test]
    fn use_path_as_marker_is_positional_not_textual() {
        let f = file("use as as as;\n");
        let TopView::Use(u) = f.items().next().expect("one item") else {
            panic!("expected use decl");
        };
        let paths: Vec<UsePathView> = u.paths().collect();
        assert_eq!(paths.len(), 1);
        let seg: Vec<String> = paths[0]
            .segments()
            .iter()
            .map(|t| t.text().to_string())
            .collect();
        assert_eq!(seg, vec!["as"]);
        assert_eq!(paths[0].alias_token().expect("alias").text(), "as");
    }

    #[test]
    fn function_header_contextual_keywords() {
        let f = file("export main() { right; }\nexport() { left; }\n");
        let headers: Vec<FnHeader> = f
            .items()
            .map(|i| match i {
                TopView::Function(func) => func.header(),
                _ => panic!("expected functions"),
            })
            .collect();
        assert_eq!(headers[0].name.text(), "main");
        assert!(headers[0].has_export);
        // A lone IDENT before `(` is always the NAME — `export` here is
        // a function named export, not a modifier.
        assert_eq!(headers[1].name.text(), "export");
        assert!(!headers[1].has_export);
    }

    #[test]
    fn statements_labels_and_nesting() {
        let f = file("main() {\n1: 2: right, left;\ng() { right; }\n}\n");
        let TopView::Function(func) = f.items().next().expect("fn") else {
            panic!("expected function");
        };
        let stmts: Vec<StatementView> = func.statements().collect();
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0].labels().count(), 2);
        assert_eq!(stmts[0].items().count(), 2);
        assert_eq!(func.nested().count(), 1);
        assert!(func.doc_run().is_none());
    }

    /// `volatile` (only legal on the un-namespaced top-level `main`) and
    /// a bound doc run together: the one shape where a DOC_RUN *node*
    /// sits among the FUNCTION node's direct children before its header
    /// IDENTs, exercising `header`'s `take_while` past a node child
    /// rather than only tokens.
    #[test]
    fn function_header_volatile_and_doc_run() {
        let f = file("? drives the head to the end\nvolatile main() { right; }\n");
        let TopView::Function(func) = f.items().next().expect("fn") else {
            panic!("expected function");
        };
        let h = func.header();
        assert_eq!(h.name.text(), "main");
        assert!(h.has_volatile);
        assert!(!h.has_export);
        assert!(func.doc_run().is_some());
    }

    #[test]
    fn namespace_name_and_items() {
        let f = file("namespace ns {\nuse std::goToEnd;\ninner() { right; }\n}\n");
        let TopView::Namespace(ns) = f.items().next().expect("one item") else {
            panic!("expected namespace");
        };
        assert_eq!(ns.name(), "ns");
        assert_eq!(ns.name_token().text(), "ns");
        let items: Vec<TopView> = ns.items().collect();
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], TopView::Use(_)));
        assert!(matches!(items[1], TopView::Function(_)));
    }

    #[test]
    fn label_tokens() {
        let f = file("main() {\n1: right;\n}\n");
        let TopView::Function(func) = f.items().next().expect("fn") else {
            panic!("expected function");
        };
        let stmt = func.statements().next().expect("one statement");
        let label = stmt.labels().next().expect("one label");
        assert_eq!(label.number_token().text(), "1");
        assert_eq!(label.colon_token().text(), ":");
    }

    // Gated like the three tests below it: in a release build (where
    // `debug_assert!` compiles out) this import would otherwise be
    // reported unused, since nothing outside those `#[cfg(debug_assertions)]`
    // bodies names `TreeBuilder`.
    #[cfg(debug_assertions)]
    use mtc_core::syntax::TreeBuilder;

    /// A FILE whose child node is not one of the three top-level kinds.
    /// `top_items` filter-maps such a child away; the assertion makes
    /// the drop loud in debug builds instead of yielding a short list.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "unexpected node kind at top level")]
    fn top_items_refuses_an_unexpected_child_kind() {
        let mut b = TreeBuilder::new();
        b.start_node(PmcKind::File.into());
        b.start_node(PmcKind::Statement.into());
        b.token(PmcKind::Ident.into(), "right");
        b.token(PmcKind::Semi.into(), ";");
        b.finish_node();
        b.finish_node();
        let root = SyntaxNode::new_root(b.finish());
        let file = FileView::cast(root).expect("root is FILE");
        let _ = file.items().count();
    }

    /// A NAMESPACE header is exactly `namespace <name>`: two IDENTs
    /// before the block. `.nth(1)` would happily take the second of
    /// three and hand back a wrong name.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "NAMESPACE header is exactly")]
    fn name_token_refuses_a_three_ident_header() {
        let mut b = TreeBuilder::new();
        b.start_node(PmcKind::Namespace.into());
        b.token(PmcKind::Ident.into(), "namespace");
        b.token(PmcKind::Ident.into(), "a");
        b.token(PmcKind::Ident.into(), "b");
        b.token(PmcKind::LBrace.into(), "{");
        b.token(PmcKind::RBrace.into(), "}");
        b.finish_node();
        let root = SyntaxNode::new_root(b.finish());
        let ns = NamespaceView::cast(root).expect("root is NAMESPACE");
        let _ = ns.name();
    }

    /// The only two modifier IDENTs the parser accepts before a
    /// function name are `export` and `volatile`. A third would be
    /// silently ignored by the catch-all arm.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "unexpected modifier IDENT")]
    fn header_refuses_an_unknown_modifier_ident() {
        let mut b = TreeBuilder::new();
        b.start_node(PmcKind::Function.into());
        b.token(PmcKind::Ident.into(), "inline");
        b.token(PmcKind::Ident.into(), "f");
        b.token(PmcKind::LParen.into(), "(");
        b.token(PmcKind::RParen.into(), ")");
        b.token(PmcKind::LBrace.into(), "{");
        b.token(PmcKind::RBrace.into(), "}");
        b.finish_node();
        let root = SyntaxNode::new_root(b.finish());
        let f = FunctionView::cast(root).expect("root is FUNCTION");
        let _ = f.header();
    }
}
