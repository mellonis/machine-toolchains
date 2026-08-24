//! Typed views over the `.tmc` green tree (docs/core.md (syntax tree)):
//! zero-copy wrappers over one `super::kinds::TmcKind` node kind each,
//! declared with the core `ast_node!` macro. Each view accepts exactly
//! the node kind it names and refuses every other — the cast contract
//! every accessor built on top of this layer rests on. This file is
//! the type layer only; accessors are added kind by kind on top of it.

use mtc_core::ast_node;
use mtc_core::syntax::{AstNode, SyntaxElement, SyntaxNode, SyntaxToken, child, children};

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

    /// The wrapped view's own node, regardless of which variant this
    /// is — lets a caller iterating `items()` cast further (e.g.
    /// `AlphabetView::cast(item.node().clone())`) without matching the
    /// variant first.
    pub fn node(&self) -> &SyntaxNode {
        match self {
            TopView::Use(v) => v.syntax(),
            TopView::Alphabet(v) => v.syntax(),
            TopView::Reuse(v) => v.syntax(),
            TopView::Machine(v) => v.syntax(),
            TopView::Namespace(v) => v.syntax(),
        }
    }

    /// This item's `TmcKind`, one constant per variant. Exists so a
    /// caller iterating `items()` can compare/collect kinds uniformly
    /// (`root.items().map(|i| i.kind())`) instead of matching the
    /// variant per item — NOT redundant with matching the variant: it
    /// is the one place that comparison is a single expression rather
    /// than a `match` at every call site.
    pub fn kind(&self) -> TmcKind {
        match self {
            TopView::Use(_) => TmcKind::Use,
            TopView::Alphabet(_) => TmcKind::Alphabet,
            TopView::Reuse(_) => TmcKind::Reuse,
            TopView::Machine(_) => TmcKind::Machine,
            TopView::Namespace(_) => TmcKind::Namespace,
        }
    }
}

/// Direct top-level-shaped children of `node`, in document order —
/// shared by `RootView::items` and `NamespaceView::items`. A child
/// that casts to none of the five `TopView` kinds is a tree the parser
/// cannot produce: ROOT/NAMESPACE children are only USE / ALPHABET /
/// REUSE / MACHINE / NAMESPACE, and a bound DOC_RUN is retro-wrapped
/// into its own declaration rather than sitting at this level
/// (docs/core.md (syntax trees)). Asserted rather than silently
/// filtered, because every consumer of this iterator treats a short
/// list as "the file/namespace had fewer items".
fn top_items(node: &SyntaxNode) -> impl Iterator<Item = TopView> + '_ {
    node.children().filter_map(|child| {
        let kind = child.kind();
        let top = TopView::cast(child);
        debug_assert!(
            top.is_some(),
            "unexpected node kind at top level: {:?}",
            kind
        );
        top
    })
}

impl RootView {
    pub fn items(&self) -> impl Iterator<Item = TopView> + '_ {
        top_items(self.syntax())
    }
}

impl UseView {
    pub fn paths(&self) -> impl Iterator<Item = UsePathView> + '_ {
        children(self.syntax())
    }
}

/// `UsePathView`'s own IDENT/`::` tokens, in order — trivia and any
/// other kind excluded.
fn use_path_tokens(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t)
                if t.kind() == TmcKind::Ident.into() || t.kind() == TmcKind::ColonColon.into() =>
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
/// literal `as` marker, the second is the alias (`parse_use` in
/// `crates/turing-machine/src/parser.rs`). Shared by `segments` and
/// `alias_token` so the walk is written once.
fn use_path_parts(node: &SyntaxNode) -> (Vec<SyntaxToken>, Option<SyntaxToken>) {
    let toks = use_path_tokens(node);
    let mut segments = Vec::new();
    let mut i = 0;
    if i < toks.len() {
        segments.push(toks[i].clone());
        i += 1;
    }
    while i + 1 < toks.len() && toks[i].kind() == TmcKind::ColonColon.into() {
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

/// `ALPHABET`'s own header IDENTs, in document order — direct child
/// IDENT tokens up to (not including) the opening `{`: an optional
/// `export`, then the `alphabet` keyword, then the name
/// (`parse_alphabet`/the `"export"` arm of `top_items` in
/// `crates/turing-machine/src/parser.rs`). Shared by `name_token` and
/// `exported` so the walk is written once.
fn alphabet_header_idents(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .take_while(|e| e.kind() != TmcKind::LBrace.into())
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
            _ => None,
        })
        .collect()
}

impl AlphabetView {
    /// The alphabet's name: the LAST header IDENT before `{`. `export`
    /// (if written) and the `alphabet` keyword both precede it, so the
    /// name is always the final header IDENT regardless of whether
    /// `export` is present — the same "last one is the name" rule the
    /// PM sibling's `FunctionView::header`
    /// (`crates/post-machine/src/syntax/views.rs`) uses for its own
    /// optional modifier prefix.
    pub fn name_token(&self) -> SyntaxToken {
        alphabet_header_idents(self.syntax())
            .into_iter()
            .next_back()
            .expect("ALPHABET always carries a name IDENT before its `{`")
    }

    /// Whether `export` was written. This lexer has no dedicated
    /// keyword token kind at all — `export` arrives as an ordinary
    /// `IDENT`, the same as every other word in the language
    /// (`crates/turing-machine/src/lexer.rs`). What refuses `export`
    /// wherever a name is expected is the PARSER: it is one of the 27
    /// fully-reserved words in `crate::lexer::RESERVED`, so
    /// `Parser::name()` rejects it — and rejects `alphabet` the same
    /// way — as an alphabet's own name (`ReservedName`; `alphabet
    /// export { ... }` does not parse). The header is therefore always
    /// exactly `export? alphabet <name>` with no name/keyword collision
    /// possible, so matching the FIRST header IDENT's text against
    /// `"export"` is a real presence check, not a guess.
    pub fn exported(&self) -> bool {
        alphabet_header_idents(self.syntax())
            .first()
            .is_some_and(|t| t.text() == "export")
    }

    /// This alphabet's own glyph literals, in document order — direct
    /// `GLYPH` children only. A nested structure under `ALPHABET` is
    /// not possible in the current grammar, but a descendant walk
    /// would silently start picking up glyphs from elsewhere if that
    /// ever changed. An alphabet element can also be written as a
    /// NUMBER literal (`alphabet a { 0..5 }`; `sym_lit` in
    /// `crates/turing-machine/src/parser.rs` accepts a `GLYPH` or a
    /// `NUMBER`) — those are `NUMBER` tokens, not `GLYPH`, so a purely
    /// numeric alphabet yields an empty `Vec` here.
    pub fn glyph_tokens(&self) -> Vec<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == TmcKind::Glyph.into() => Some(t),
                _ => None,
            })
            .collect()
    }

    /// The doc run this declaration retro-wraps, when one was written —
    /// the alphabet's own first child node (docs/core.md (syntax
    /// trees)).
    pub fn doc_run(&self) -> Option<DocRunView> {
        child(self.syntax())
    }
}

/// `REUSE`'s own header IDENTs, in document order — direct child IDENT
/// tokens up to (not including) the opening `(` of the signature: an
/// optional `export`, then the `routine`/`graph` keyword, then the
/// name (`parse_reuse` in `crates/turing-machine/src/parser.rs`, called
/// from both the plain and the `export`-prefixed arms of `top_items`).
/// Shared by `name_token`, `exported`, and `kind` so the walk is
/// written once — the same shape `alphabet_header_idents` reads for
/// `ALPHABET`, with `(` in place of `{` as the header's terminator.
fn reuse_header_idents(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .take_while(|e| e.kind() != TmcKind::LParen.into())
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
            _ => None,
        })
        .collect()
}

/// Which reusable-graph carrier a `REUSE` node spells — `routine` or
/// `graph`, matched on the header keyword's own text (`crate::lexer`'s
/// lexer emits no keyword token kind at all: every word, reserved or
/// not, arrives as an ordinary `IDENT`, and `routine`/`graph` are two
/// of the 27 fully-reserved words in `crate::lexer::RESERVED` that the
/// PARSER refuses wherever a name is expected — not a contextual word;
/// `deprecated` is this language's only contextual one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReuseKind {
    Routine,
    Graph,
}

impl ReuseView {
    /// `routine` or `graph`: the header IDENT immediately before the
    /// name, regardless of whether `export` precedes it — the same
    /// "second-to-last is the keyword, last is the name" position
    /// `AlphabetView::name_token`'s doc explains for `ALPHABET`'s own
    /// optional `export` prefix.
    pub fn kind(&self) -> ReuseKind {
        let idents = reuse_header_idents(self.syntax());
        debug_assert!(
            idents.len() >= 2,
            "REUSE header is `export? routine|graph <name>`: at least 2 IDENTs, got {}",
            idents.len()
        );
        match idents[idents.len() - 2].text() {
            "routine" => ReuseKind::Routine,
            "graph" => ReuseKind::Graph,
            other => panic!(
                "unexpected REUSE keyword IDENT {other:?} — the parser accepts only \
                 `routine`/`graph` here"
            ),
        }
    }

    /// The reuse's name: the LAST header IDENT before `(`, mirroring
    /// `AlphabetView::name_token`.
    pub fn name_token(&self) -> SyntaxToken {
        reuse_header_idents(self.syntax())
            .into_iter()
            .next_back()
            .expect("REUSE always carries a name IDENT before its signature `(`")
    }

    /// Whether `export` was written — the first header IDENT's text,
    /// mirroring `AlphabetView::exported`.
    pub fn exported(&self) -> bool {
        reuse_header_idents(self.syntax())
            .first()
            .is_some_and(|t| t.text() == "export")
    }

    /// The signature's own tokens, `(` through the matching `)`,
    /// trivia excluded — no dedicated node wraps a `.tmc` signature
    /// (the module doc's REUSE/WORLD accounting explains why: WORLD
    /// wraps only the body, and a signature's tokens sit directly
    /// under REUSE between the name and WORLD). Returned unparsed:
    /// turning this token run into typed parameters is extraction's
    /// job (a later layer built on top of these views), not this
    /// one's — a view answers what token run the tree holds, not what
    /// it means.
    pub fn signature(&self) -> Vec<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .skip_while(|e| e.kind() != TmcKind::LParen.into())
            .take_while(|e| e.kind() != TmcKind::World.into())
            .filter_map(|e| match e {
                SyntaxElement::Token(t)
                    if t.kind() != TmcKind::Whitespace.into()
                        && t.kind() != TmcKind::LineComment.into()
                        && t.kind() != TmcKind::BlockComment.into() =>
                {
                    Some(t)
                }
                _ => None,
            })
            .collect()
    }

    /// This reuse's own body — `None` only for a tree that cannot come
    /// from the parser, since `parse_reuse` always opens a WORLD node
    /// around the `{ … }` body. Returning `Option` rather than
    /// panicking anyway: a view's job is to answer what the tree
    /// holds, and reporting absence is an answer, not a defect.
    pub fn world(&self) -> Option<WorldView> {
        child(self.syntax())
    }

    /// The doc run this declaration retro-wraps, when one was written.
    pub fn doc_run(&self) -> Option<DocRunView> {
        child(self.syntax())
    }
}

impl MachineView {
    /// This machine's own body. See `ReuseView::world` for why this is
    /// `Option` rather than a panicking accessor.
    pub fn world(&self) -> Option<WorldView> {
        child(self.syntax())
    }

    /// The doc run this declaration retro-wraps, when one was written.
    pub fn doc_run(&self) -> Option<DocRunView> {
        child(self.syntax())
    }
}

/// `TAPE`'s own header IDENTs, in document order — direct child IDENT
/// tokens up to (not including) the `:` before the alphabet name: an
/// optional `volatile`, then the `tape` keyword, then the name
/// (`parse_tape` in `crates/turing-machine/src/parser.rs`). Shared by
/// `name_token` and `volatile` so the walk is written once — the same
/// shape `alphabet_header_idents` reads for `ALPHABET`, with `:` in
/// place of `{` as the header's terminator.
fn tape_header_idents(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .take_while(|e| e.kind() != TmcKind::Colon.into())
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
            _ => None,
        })
        .collect()
}

impl TapeView {
    /// The tape's name: the LAST header IDENT before `:`, mirroring
    /// `AlphabetView::name_token`.
    pub fn name_token(&self) -> SyntaxToken {
        tape_header_idents(self.syntax())
            .into_iter()
            .next_back()
            .expect("TAPE always carries a name IDENT before its `:`")
    }

    /// Whether `volatile` was written — the first header IDENT's text,
    /// mirroring `AlphabetView::exported`. `volatile` is a modifier on
    /// the tape declaration itself, never a separate node.
    pub fn volatile(&self) -> bool {
        tape_header_idents(self.syntax())
            .first()
            .is_some_and(|t| t.text() == "volatile")
    }

    /// The alphabet name this tape is declared over: the first IDENT
    /// after `:`.
    pub fn alphabet_token(&self) -> SyntaxToken {
        self.syntax()
            .children_with_tokens()
            .skip_while(|e| e.kind() != TmcKind::Colon.into())
            .filter_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
                _ => None,
            })
            .next()
            .expect("TAPE always carries an alphabet IDENT after its `:`")
    }
}

impl WorldView {
    /// This world's own tape declarations, in document order. Direct
    /// children only, like every accessor at this layer.
    pub fn tapes(&self) -> impl Iterator<Item = TapeView> + '_ {
        children(self.syntax())
    }

    /// This world's own states, in document order.
    pub fn states(&self) -> impl Iterator<Item = StateView> + '_ {
        children(self.syntax())
    }

    /// This world's own grafts, in document order.
    pub fn grafts(&self) -> impl Iterator<Item = GraftView> + '_ {
        children(self.syntax())
    }

    /// This world's own binds, in document order.
    pub fn binds(&self) -> impl Iterator<Item = BindView> + '_ {
        children(self.syntax())
    }
}

impl NamespaceView {
    /// The second header IDENT — the name after the `namespace`
    /// keyword IDENT. Unlike `ALPHABET`, a `NAMESPACE` header never
    /// carries an `export` prefix (`top_items` in
    /// `crates/turing-machine/src/parser.rs` accepts `export` only
    /// before `alphabet`/`routine`/`graph`), so the header is always
    /// exactly two IDENTs and the name is always the second.
    pub fn name_token(&self) -> SyntaxToken {
        let idents: Vec<SyntaxToken> = self
            .syntax()
            .children_with_tokens()
            .filter_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
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

    /// This namespace's own items, not the whole file's.
    pub fn items(&self) -> impl Iterator<Item = TopView> + '_ {
        top_items(self.syntax())
    }
}
