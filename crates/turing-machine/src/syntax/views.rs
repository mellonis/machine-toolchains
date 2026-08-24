//! Typed views over the `.tmc` green tree (docs/core.md (syntax trees)):
//! zero-copy wrappers over one `super::kinds::TmcKind` node kind each,
//! declared with the core `ast_node!` macro. Each view accepts exactly
//! the node kind it names and refuses every other — the cast contract
//! every accessor built on top of this layer rests on. This file is
//! the type layer only; accessors are added kind by kind on top of it.

use mtc_core::ast_node;
use mtc_core::syntax::{AstNode, SyntaxElement, SyntaxNode, SyntaxToken, child, children};

use super::kinds::TmcKind;

/// Trivia kinds excluded from a token-run accessor — whitespace and
/// both comment flavors. Shared by every accessor that returns a flat
/// significant-token run (`ReuseView::signature`, `RuleView::pattern_tokens`).
fn is_trivia(t: &SyntaxToken) -> bool {
    t.kind() == TmcKind::Whitespace.into()
        || t.kind() == TmcKind::LineComment.into()
        || t.kind() == TmcKind::BlockComment.into()
}

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
ast_node!(pub struct SigParamView: TmcKind::SigParam.into());
ast_node!(pub struct ContractClauseView: TmcKind::ContractClause.into());
ast_node!(pub struct WriteVecView: TmcKind::WriteVec.into());
ast_node!(pub struct MoveVecView: TmcKind::MoveVec.into());
ast_node!(pub struct TransitionView: TmcKind::Transition.into());
ast_node!(pub struct BindingArgView: TmcKind::BindingArg.into());
ast_node!(pub struct SymMapView: TmcKind::SymMap.into());

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
///
/// The `take_while` below is correct only because `{` is
/// grammar-mandatory right after the name: `parse_alphabet` calls
/// `self.expect(&TokenKind::LBrace, "`{` to open the alphabet
/// body")` immediately after `self.name(...)`, with no production
/// that lets a header omit it (verified: `alphabet ab` with no
/// following `{` is rejected — `expected `{` to open the alphabet
/// body, found ...`). If that terminator were ever optional, this
/// scan would run past the header into the body and every accessor
/// built on it would read the wrong token.
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
///
/// The `take_while` below is correct only because `(` is
/// grammar-mandatory right after the name: `parse_reuse` calls
/// `self.signature()`, which itself calls
/// `self.expect(&TokenKind::LParen, "`(` to open the signature")`
/// as its first step, with no production that lets a header omit it
/// (verified: both `routine r { ... }` and `graph g { ... }` — the
/// signature's parens dropped — are rejected with `expected `(` to
/// open the signature, found ...`). If that terminator were ever
/// optional, this scan would run past the header into the body and
/// every accessor built on it would read the wrong token.
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
    /// this is the token run `Parser::signature` itself consumes — that
    /// production opens with `expect(LParen)` and closes with
    /// `expect(RParen)`, taking nothing outside them — so a consumer
    /// wanting VALUES retokenizes this run and hands it back to that
    /// production rather than splitting it. Splitting is what would
    /// duplicate grammar the parser owns.
    ///
    /// Flattens each element rather than keeping only direct-child
    /// tokens: a parameter is a `SIG_PARAM` node, so a direct-token
    /// filter drops every parameter and keeps only the punctuation
    /// REUSE itself owns. Measured on `routine r(tape t: ab)` with the
    /// flattening removed, this returned `["(", ")"]`. The parens are
    /// REUSE's own tokens, which is why the run still starts and ends
    /// with them.
    pub fn signature(&self) -> Vec<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .skip_while(|e| e.kind() != TmcKind::LParen.into())
            .take_while(|e| e.kind() != TmcKind::World.into())
            .flat_map(|e| match e {
                SyntaxElement::Token(t) => vec![t],
                SyntaxElement::Node(n) => n.descendant_tokens().collect(),
            })
            .filter(|t| !is_trivia(t))
            .collect()
    }

    /// This reuse's signature parameters, in declaration order — which
    /// is the order that IS the vector position
    /// (docs/tmt/language.md (tapes and heads)).
    pub fn params(&self) -> impl Iterator<Item = SigParamView> + '_ {
        children(self.syntax())
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
///
/// The `take_while` below is correct only because `:` is
/// grammar-mandatory right after the name: `parse_tape` calls
/// `self.expect(&TokenKind::Colon, "`:` after the tape name")`
/// immediately after `self.name(...)`, with no production that lets a
/// header omit it (verified: `tape main ab;` with no `:` is rejected —
/// `expected `:` after the tape name, found ...`). If that
/// terminator were ever optional, this scan would run past the header
/// into the body and every accessor built on it would read the wrong
/// token.
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

/// `STATE`'s own header IDENTs, in document order — direct child IDENT
/// tokens up to (not including) the opening `{`: an optional `entry`,
/// then the `state` keyword, then the name (`parse_state`/`world_body`
/// in `crates/turing-machine/src/parser.rs`). Shared by `name_token` and
/// `is_entry` so the walk is written once — the same shape and the same
/// `{` terminator `alphabet_header_idents` reads for `ALPHABET`.
///
/// The `take_while` below is correct only because `{` is
/// grammar-mandatory right after the name: `state s;` — the body
/// dropped — is rejected as `StateRedirect` ("a state has a `{ … }`
/// body — the `state name;` redirect form is not supported"), and any
/// other token there (verified: `state s foo {` rejects `foo`) is
/// rejected by
/// `self.expect(&TokenKind::LBrace, "`{` to open the state body")`.
/// If that terminator were ever optional, this scan would run past
/// the header into the body and every accessor built on it would read
/// the wrong token.
fn state_header_idents(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .take_while(|e| e.kind() != TmcKind::LBrace.into())
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
            _ => None,
        })
        .collect()
}

impl StateView {
    /// The state's name: the LAST header IDENT before `{`, mirroring
    /// `AlphabetView::name_token`.
    pub fn name_token(&self) -> SyntaxToken {
        state_header_idents(self.syntax())
            .into_iter()
            .next_back()
            .expect("STATE always carries a name IDENT before its `{`")
    }

    /// Whether `entry` was written — the first header IDENT's text, the
    /// same prefix-modifier rule `AlphabetView::exported` and
    /// `TapeView::volatile` already use. `entry` is one of the 27
    /// fully-reserved words the parser refuses wherever a name is
    /// expected (`crate::lexer::RESERVED`), so no state name can
    /// collide with it. A world marks exactly one entry, on either its
    /// one entry state or its one entry graft (docs/tmt/language.md
    /// (entry)).
    pub fn is_entry(&self) -> bool {
        state_header_idents(self.syntax())
            .first()
            .is_some_and(|t| t.text() == "entry")
    }

    /// This state's own rules, in document order — comments between
    /// rules are trivia, not RULE nodes, so they never appear here.
    pub fn rules(&self) -> impl Iterator<Item = RuleView> + '_ {
        children(self.syntax())
    }

    /// The doc run this declaration retro-wraps, when one was written.
    pub fn doc_run(&self) -> Option<DocRunView> {
        child(self.syntax())
    }
}

/// A `SIG_PARAM`'s own direct IDENT tokens, in order. Exactly
/// `volatile? tape NAME ALPHABET` or `state NAME` — the `writes`/
/// `preserves` keywords are NOT among them, because each clause is a
/// `CONTRACT_CLAUSE` child node and this walk is direct-children-only.
///
/// That exclusion is a CONSEQUENCE of bracketing the clauses, not the
/// reason for it, and the accessors below do not depend on it. Measured
/// with the `CONTRACT_CLAUSE` bracket removed, on
/// `volatile tape a: x writes { '0' } preserves { '1' }`: this run
/// becomes six IDENTs (`volatile`, `tape`, `a`, `x`, `writes`,
/// `preserves`), and every positional accessor still answers correctly
/// — `volatile()` true, `kind()` `Tape`, `name_token()` `a`,
/// `alphabet_token()` `Some("x")` — because a clause keyword can only
/// ever sit AFTER the alphabet. The single thing that breaks is
/// `contract_clauses()`, which goes empty.
///
/// So the clause bracket's reason is the one `super::kinds`'s module
/// doc gives — a clause's EXTENT is decided by its own reserved word,
/// so with no node nothing addresses the clause at all — never a risk
/// to the parameter's name.
fn sig_param_idents(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
            _ => None,
        })
        .collect()
}

/// Which of the two signature-parameter forms a `SIG_PARAM` spells.
///
/// Deliberately the same name as `crate::parser::SigParamKind`, which
/// this mirrors: that one is the AST's, carrying the tape form's
/// payload (alphabet, `volatile`, both clauses); this one is the view
/// layer's, a bare tag over a node whose payload the other accessors
/// answer for. A consumer importing both aliases one at the `use` site
/// — the collision is a compile error, never a silent mix-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SigParamKind {
    Tape,
    State,
}

impl SigParamView {
    /// Whether `volatile` was written — the first direct IDENT's text,
    /// the same prefix-modifier rule `AlphabetView::exported` and
    /// `TapeView::volatile` already use. `volatile` is one of the 27
    /// fully-reserved words the parser refuses wherever a name is
    /// expected, so no parameter name can collide with it.
    pub fn volatile(&self) -> bool {
        sig_param_idents(self.syntax())
            .first()
            .is_some_and(|t| t.text() == "volatile")
    }

    /// `tape` or `state` — the first non-`volatile` direct IDENT.
    pub fn kind(&self) -> SigParamKind {
        let idents = sig_param_idents(self.syntax());
        let kw = idents
            .get(usize::from(self.volatile()))
            .expect("SIG_PARAM always carries a `tape`/`state` keyword IDENT");
        match kw.text() {
            "tape" => SigParamKind::Tape,
            "state" => SigParamKind::State,
            other => panic!(
                "unexpected SIG_PARAM keyword IDENT {other:?} — the parser accepts only \
                 `tape`/`state` here"
            ),
        }
    }

    /// The parameter's own name: the IDENT after the `tape`/`state`
    /// keyword.
    pub fn name_token(&self) -> SyntaxToken {
        let idents = sig_param_idents(self.syntax());
        idents
            .into_iter()
            .nth(usize::from(self.volatile()) + 1)
            .expect("SIG_PARAM always carries a name IDENT after its keyword")
    }

    /// The alphabet a tape parameter is declared over — the IDENT after
    /// `:`. `None` for a `state` parameter, which declares no alphabet.
    pub fn alphabet_token(&self) -> Option<SyntaxToken> {
        let idents = sig_param_idents(self.syntax());
        idents.into_iter().nth(usize::from(self.volatile()) + 2)
    }

    /// This parameter's own contract clauses, in the order written.
    /// The parser fixes that order (`writes` before `preserves`) and
    /// rejects a duplicate of either, so a caller reads each clause's
    /// own keyword rather than its position.
    pub fn contract_clauses(&self) -> impl Iterator<Item = ContractClauseView> + '_ {
        children(self.syntax())
    }
}

impl ContractClauseView {
    /// `writes` or `preserves` — the clause's own first token. The
    /// keyword is what says which field this clause fills; its position
    /// among the parameter's clauses does not.
    pub fn keyword_token(&self) -> SyntaxToken {
        self.syntax()
            .first_token()
            .expect("CONTRACT_CLAUSE always opens on its own keyword IDENT")
    }
}

impl BindingArgView {
    /// The parameter name being bound — the argument's own first token,
    /// the LHS of `=`.
    pub fn name_token(&self) -> SyntaxToken {
        self.syntax()
            .first_token()
            .expect("BINDING_ARG always opens on its own name IDENT")
    }

    /// The `map { … }` this argument's value carries, when `with map`
    /// was written.
    pub fn sym_map(&self) -> Option<SymMapView> {
        child(self.syntax())
    }
}

impl RuleView {
    /// This rule's own pattern: `[` through the matching `]`, trivia
    /// excluded — the run up to (not including) `->`. Unlike
    /// `write_vec`/`move_vec`/`transition`, the pattern is not itself a
    /// node: it is positionally first and mandatory in every rule
    /// (`Parser::rule` calls `self.pattern()` before anything else), so
    /// the derivation that gave `WRITE_VEC`/`MOVE_VEC`/`TRANSITION` their
    /// own node kind — an optional, keyword-decided extent — gives this
    /// one none (`super::kinds`'s module doc, rule 1). Returned unparsed
    /// for the same reason `ReuseView::signature` is: turning it into
    /// values is extraction's job, not the view layer's.
    pub fn pattern_tokens(&self) -> Vec<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .take_while(|e| e.kind() != TmcKind::Arrow.into())
            .filter_map(|e| match e {
                SyntaxElement::Token(t) => Some(t),
                _ => None,
            })
            .filter(|t| !is_trivia(t))
            .collect()
    }

    /// The `write […]` vector this rule carries, when one was written.
    pub fn write_vec(&self) -> Option<WriteVecView> {
        child(self.syntax())
    }

    /// The `move […]` vector this rule carries, when one was written.
    pub fn move_vec(&self) -> Option<MoveVecView> {
        child(self.syntax())
    }

    /// The transition this rule writes, when one was written.
    ///
    /// `None` here IS `Transition::Stay` — "stay in the current
    /// state" — never an error and never a missing feature. A rule may
    /// omit its transition only when it already carries an action
    /// (`write`, `move`, or a leading `debugger`); with none of those, an
    /// omitted transition is instead a parse error, so by the time a
    /// RULE node exists at all, `None` here can only mean `Stay`
    /// (docs/tmt/language.md (transitions)). The green tree carries this fact
    /// as the ABSENCE of a TRANSITION node under the rule — no token
    /// scan could express it, since an omitted transition leaves nothing
    /// behind but the `;` that would have followed a written one either
    /// way. A later reader must not "fix" this `None` into a panic.
    pub fn transition(&self) -> Option<TransitionView> {
        child(self.syntax())
    }
}

/// `GRAFT`'s own header IDENTs, in document order — direct child IDENT
/// tokens up to (not including) the opening `(` of the binding list: an
/// optional `entry`, then the `graft` keyword, then the target's own
/// first name segment (`parse_graft`/`world_body` in
/// `crates/turing-machine/src/parser.rs`; a qualified target's further
/// `:: IDENT` segments, if any, follow but are not part of what
/// `target_token` answers). Shared by `target_token` and `is_entry` so
/// the walk is written once — the same shape `alphabet_header_idents`
/// reads for `ALPHABET`, with `(` in place of `{` as the header's
/// terminator.
///
/// The `take_while` below is correct only because `(` is
/// grammar-mandatory right after the target: `binding_args` opens with
/// `self.expect(&TokenKind::LParen, "`(` to open the binding")`
/// immediately after `self.qual_name(...)` returns, with no production
/// that lets a header omit it (verified: `graft g;` — the argument list
/// dropped — is rejected with `expected `(` to open the binding, found
/// `;``). If that terminator were ever optional, this scan would run
/// past the header into the argument list and every accessor built on
/// it would read the wrong token.
fn graft_header_idents(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .take_while(|e| e.kind() != TmcKind::LParen.into())
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
            _ => None,
        })
        .collect()
}

/// The `as NAME` tail after `terminator`, when GRAFT or BIND's own
/// binding-list close paren is behind it — the grammar allows exactly
/// two shapes there: nothing, or `as` followed by the name. Both
/// `parse_graft` and `parse_bind` bump `as` and immediately call
/// `self.name(...)` for the name in the same breath — the pair is
/// atomic, so an `as` with no name after it cannot reach the tree.
/// Shared so the walk is written once; `as`'s own text is never read —
/// the SECOND ident found (if any) is the answer.
fn as_name_after(node: &SyntaxNode, terminator: TmcKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .skip_while(|e| e.kind() != terminator.into())
        .skip(1)
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
            _ => None,
        })
        .nth(1)
}

impl GraftView {
    /// Whether `entry` was written — the first header IDENT's text, the
    /// same prefix-modifier rule `StateView::is_entry` uses. `entry` is
    /// one of the 27 fully-reserved words the parser refuses wherever a
    /// name is expected (`crate::lexer::RESERVED`); it attaches only to
    /// `state`/`graft`, never `bind` (docs/tmt/language.md (entry)).
    pub fn is_entry(&self) -> bool {
        graft_header_idents(self.syntax())
            .first()
            .is_some_and(|t| t.text() == "entry")
    }

    /// The graft's own target: the first IDENT after the `graft`
    /// keyword. A qualified target's further `:: IDENT` segments are not
    /// part of this answer.
    pub fn target_token(&self) -> SyntaxToken {
        let idents = graft_header_idents(self.syntax());
        idents
            .into_iter()
            .nth(usize::from(self.is_entry()) + 1)
            .expect("GRAFT always carries a target IDENT after its `graft` keyword")
    }

    /// The `as name` instance name, when written. Mandatory on every
    /// non-entry graft — the parser rejects one without it
    /// (`GraftNeedsName`: "a non-entry `graft` needs an `as name` — only
    /// an `entry graft` may omit it") — and optional on an entry graft.
    pub fn as_name(&self) -> Option<SyntaxToken> {
        as_name_after(self.syntax(), TmcKind::RParen)
    }

    /// This graft's own binding arguments, in document order.
    pub fn bindings(&self) -> impl Iterator<Item = BindingArgView> + '_ {
        children(self.syntax())
    }
}

/// `BIND`'s own header IDENTs, in document order — direct child IDENT
/// tokens up to (not including) the opening `(` of the binding list:
/// the `bind` keyword, then the target's own first name segment
/// (`parse_bind` in `crates/turing-machine/src/parser.rs`). Unlike
/// `GRAFT`'s, this header never carries an `entry` prefix: `world_body`
/// reaches its `bind` branch only from an `else if` reached after the
/// leading `entry` branch was not taken, and `entry`'s own branch
/// accepts only `state`/`graft` after it — `entry bind` is a parse
/// error (docs/tmt/language.md (entry)).
///
/// The `take_while` below is correct only because `(` is
/// grammar-mandatory right after the target — see `graft_header_idents`'s
/// identical note; `binding_args` is the same production both call.
fn bind_header_idents(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .take_while(|e| e.kind() != TmcKind::LParen.into())
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind() == TmcKind::Ident.into() => Some(t),
            _ => None,
        })
        .collect()
}

impl BindView {
    /// The bind's own target: the first IDENT after the `bind` keyword —
    /// mirrors `GraftView::target_token`, minus the `entry` shift, since
    /// `BIND` never carries one.
    pub fn target_token(&self) -> SyntaxToken {
        bind_header_idents(self.syntax())
            .into_iter()
            .nth(1)
            .expect("BIND always carries a target IDENT after its `bind` keyword")
    }

    /// The `as name` instance name — mandatory on every `BIND`:
    /// `parse_bind` calls
    /// `self.expect_kw("as", "`as` (a bind needs an instance name)")`
    /// unconditionally after the binding list, with no production that
    /// lets it be omitted.
    pub fn as_name(&self) -> SyntaxToken {
        as_name_after(self.syntax(), TmcKind::RParen)
            .expect("BIND always carries an `as name` after its binding list")
    }

    /// This bind's own binding arguments, in document order.
    pub fn bindings(&self) -> impl Iterator<Item = BindingArgView> + '_ {
        children(self.syntax())
    }
}

impl DocRunView {
    /// This run's own `?` lines, in document order — direct `DOC_LINE`
    /// tokens only.
    pub fn doc_lines(&self) -> Vec<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == TmcKind::DocLine.into() => Some(t),
                _ => None,
            })
            .collect()
    }

    /// This run's own BARE-PROSE `!` lines, in document order — an
    /// `AttentionLine` token that carries no leading `[ident]`, so
    /// `Parser::doc_run` never wraps it in an ATTR and it stays a direct
    /// token here. A TAGGED line (`! [deprecated] …`) is NOT among
    /// these: `doc_run` wraps that one token in its own ATTR the moment
    /// `Self::parse_attr` recognizes the payload, so it answers under
    /// `attrs()` instead, never here (docs/tmt/language.md (doc lines
    /// and attention lines)).
    pub fn attention_lines(&self) -> Vec<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == TmcKind::AttentionLine.into() => Some(t),
                _ => None,
            })
            .collect()
    }

    /// This run's own tagged attention lines, each wrapped in its own
    /// ATTR node, in document order.
    pub fn attrs(&self) -> impl Iterator<Item = AttrView> + '_ {
        children(self.syntax())
    }
}

impl AttrView {
    /// The single `AttentionLine` token this node wraps, payload and
    /// all. `ATTR` can wrap nothing else and no finer-grained accessor
    /// (a `name_token`) can exist: the lexer folds a whole `! [ident] …`
    /// line into ONE `AttentionLine` token (docs/core.md (syntax
    /// trees)), so `[deprecated]` is never its own token.
    ///
    /// Getting the attribute NAME back out through `Parser::parse_attr`
    /// is more than a visibility flip. That function takes the lexer's
    /// own `Token` — `line`/`col`/`len` plus a DECODED payload with the
    /// leading `!` and one optional space already stripped
    /// (`crates/turing-machine/src/lexer.rs`'s `?`/`!` line-lexing arm)
    /// — never this token's raw `.text()`. A caller needs
    /// `TextLineIndex` to rebuild `line`/`col` from this token's
    /// `TextRange` and must re-derive the decoded payload from the raw
    /// text before calling in. Doing THAT re-derivation here instead —
    /// rather than handing real glue to `parse_attr` — is the string
    /// surgery this whole layer exists to avoid.
    pub fn line_token(&self) -> SyntaxToken {
        self.syntax()
            .first_token()
            .expect("ATTR always wraps exactly one AttentionLine token")
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
