//! Lossless concrete syntax tree (CST) node types for `.tmc` — the front-end
//! mirror of the `.pmc` CST in the sibling PM-1 crate.
//!
//! `crate::parser::Parser::file` and its per-production helpers still build
//! a [`Cst`] tree of these types as they walk (the same grammar walk
//! [`crate::parser::parse_green_from_tokens`] runs, with a green sink
//! attached alongside it) — but nothing in production reads the built tree
//! any more: the compiler front end, the `.tmc` language service, and `fmt`
//! all read the green tree instead, via
//! [`crate::parser::parse_green`]/[`crate::parser::parse_green_from_tokens`]
//! and [`crate::syntax::extract_program`] (docs/core.md (syntax trees)).
//!
//! # The lossless contract
//!
//! The AST flattens for the compiler's convenience (namespaces stamped as a
//! `ns` path, machine bodies split into tapes + behavior, doc runs reduced to
//! a [`crate::parser::Doc`]); the CST keeps the source shape a printer needs:
//!
//! - **Item order and block boundaries are kept as written**, including
//!   namespace reopening — two `namespace n { … }` blocks are two sibling
//!   [`TopKind::Namespace`] nodes, never merged.
//! - **World-body items interleave in source order.** A [`MachineCst`]'s
//!   `items` is one `Vec<WorldItem>` with tape declarations, states, grafts,
//!   binds, and own-line comments interleaved exactly as written; the AST
//!   splits them into separate lists instead.
//! - **Rule internals are reused, not redefined.** [`RuleCst`] embeds the
//!   parser's [`crate::parser::Rule`] verbatim, with no rebuilding.
//! - **Comments are trivia at their real source position** (module-level
//!   own-line comments as [`TopKind::Comment`] items, same-line trailing
//!   comments riding the node they follow, brace-line comments on
//!   `open_trailing`/`close_trailing`) — position is the attachment; there is
//!   no attachment pass, save the one real one for `?`/`!` doc runs (see
//!   [`AlphabetCst::doc_run`]).
//! - **Blank-line presence is a bool** (`blank_before`): the printer collapses
//!   any run of blank lines to at most one, so a count is never needed.
//! - **Interior list comments are index-keyed** (`interior`): a comment
//!   inside a comma-separated list is stored against the index of the entry
//!   it precedes, with the entry count meaning "before the closer". The
//!   entry types stay trivia-free. Several lists sit inside an AST type
//!   embedded verbatim rather than directly on a CST node: a rule's pattern,
//!   `write`, and `move` vectors and a `call` transition's binding list
//!   (all inside [`RuleCst`]'s embedded [`Rule`]), and any `with map` pair
//!   list (inside a [`BindingArg`], which [`RuleCst`], [`GraftCst`], and
//!   [`BindCst`] all embed unchanged). Those get a second-level side-car
//!   ([`RuleCst::pattern_cells`], [`RuleCst::write_cells`],
//!   [`RuleCst::move_cells`], [`RuleCst::call_args`]/[`RuleCst::map_pairs`],
//!   [`GraftCst::map_pairs`], [`BindCst::map_pairs`]) instead.
//!
//! Container nodes deliberately do NOT carry the AST's computed fields (no
//! `ns` tag, no reduced `doc`, no tapes/behavior split) — those are derived
//! from the tree's block/interleaving structure by whatever builds the AST
//! (`crate::syntax::extract`, over the green tree — the CST's own shape
//! mirrors the same nesting); duplicating them here would only risk the two
//! disagreeing.

use mtc_core::diagnostics::Span;

use crate::lexer::Comment;
use crate::parser::{AlphabetElem, BindingArg, DocRunItem, QualName, Rule, Signature};

/// A whole `.tmc` file: top-level items in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cst {
    pub items: Vec<TopItem>,
}

/// One file/namespace-level item, plus whether a blank line precedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopItem {
    pub blank_before: bool,
    pub kind: TopKind,
}

/// A file/namespace-level item as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopKind {
    /// An own-line comment at file or namespace level.
    Comment(Comment),
    Import(UseCst),
    Alphabet(AlphabetCst),
    Namespace(NamespaceCst),
    /// A `routine` or a `graph` — one shape, discriminated by
    /// [`ReuseCst::carrier`].
    Reuse(ReuseCst),
    Machine(MachineCst),
}

/// One path within a `use` list, as written — mirrors
/// [`crate::parser::Import`] minus its lower-copy-computed `ns` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsePath {
    /// `IDENT (:: IDENT)*`, e.g. `use mylib::plusOne;` → `["mylib", "plusOne"]`.
    pub path: Vec<String>,
    /// `as NAME` rebinding; `None` if absent.
    pub alias: Option<String>,
    /// Line of this path's first token.
    pub line: u32,
    /// Path start → last segment end; an `as` alias is NOT included.
    pub span: Span,
}

/// One `use` declaration list — `use a, b;` is ONE node holding two
/// [`UsePath`] entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseCst {
    pub paths: Vec<UsePath>,
    /// Comments written INSIDE the list, in source order, each keyed by the
    /// index of the entry it precedes. An index equal to the entry count
    /// means "after the last entry, before the closer".
    ///
    /// Sparse and index-keyed rather than a per-entry wrapper, so the entry
    /// types are untouched and no AST-facing type carries trivia
    /// (docs/tmt/fmt.md (interior comments)).
    pub interior: Vec<(usize, Comment)>,
    /// Line of the `use` keyword.
    pub line: u32,
    /// First path's start → last path's end.
    pub span: Span,
    /// A comment on the same source line, after the `;`.
    pub trailing: Option<Comment>,
}

/// One `alphabet NAME { … }` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlphabetCst {
    pub name: String,
    pub name_span: Span,
    /// Line of the name token.
    pub line: u32,
    /// Column of the `export`/`alphabet` keyword (the header's first token).
    pub col: u32,
    /// The literal `export` keyword was written.
    pub exported: bool,
    /// Elements in source order.
    pub elems: Vec<AlphabetElem>,
    /// Comments written INSIDE the list, in source order, each keyed by the
    /// index of the entry it precedes. An index equal to the entry count
    /// means "after the last entry, before the closer".
    ///
    /// Sparse and index-keyed rather than a per-entry wrapper, so the entry
    /// types are untouched and no AST-facing type carries trivia
    /// (docs/tmt/fmt.md (interior comments)).
    pub interior: Vec<(usize, Comment)>,
    /// Header first token → closing `}` end.
    pub span: Span,
    /// The `?`/`!` run bound to this declaration, in source order; empty when
    /// undocumented. Unlike every other trivia field, this IS an attachment
    /// pass — the parser binds a run to the NEXT doc-accepting declaration at
    /// its scope (a run with anything else next is a `DanglingDocRun` error).
    /// [`crate::parser::reduce_doc_run`] reduces it to [`crate::parser::Doc`].
    pub doc_run: Vec<DocRunItem>,
    /// Comment(s) on the same physical line as the opening `{`.
    pub open_trailing: Vec<Comment>,
    /// A comment on the same physical line as the closing `}`.
    pub close_trailing: Option<Comment>,
}

/// `routine` vs `graph` — a `ReuseCst`'s carrier kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReuseCarrier {
    Routine,
    Graph,
}

/// One `routine`/`graph NAME(sig) { … }` declaration — the two share a shape
/// (signature + world body); [`ReuseCst::carrier`] tells them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseCst {
    pub carrier: ReuseCarrier,
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    /// Column of the header's first token (`export`/`routine`/`graph`).
    pub col: u32,
    pub exported: bool,
    pub sig: Signature,
    /// Interior comments of the SIGNATURE's parameter list, keyed as
    /// [`AlphabetCst::interior`] is. Named apart from a plain `interior`
    /// because this node's list is `sig.params`, not a field of its own.
    pub sig_interior: Vec<(usize, Comment)>,
    /// World-body items in source order (states, grafts, binds, comments).
    pub items: Vec<WorldItem>,
    /// Header first token → closing `}` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub open_trailing: Vec<Comment>,
    pub close_trailing: Option<Comment>,
}

/// The single `machine { … }` block (a program has one; a library has none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineCst {
    /// Line of the `machine` keyword.
    pub line: u32,
    /// Column of the `machine` keyword.
    pub col: u32,
    /// World-body items in source order (tape decls, states, grafts, binds,
    /// comments).
    pub items: Vec<WorldItem>,
    /// `machine` keyword start → closing `}` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub open_trailing: Vec<Comment>,
    pub close_trailing: Option<Comment>,
}

/// One `namespace NAME { … }` block exactly as written — a reopened namespace
/// is a SEPARATE sibling node, never merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceCst {
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    /// `namespace` keyword start → closing `}` end.
    pub span: Span,
    /// Body items in source order; may itself nest [`TopKind::Namespace`].
    pub items: Vec<TopItem>,
    pub doc_run: Vec<DocRunItem>,
    pub open_trailing: Vec<Comment>,
    pub close_trailing: Option<Comment>,
}

/// One world-body item, plus whether a blank line precedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldItem {
    pub blank_before: bool,
    pub kind: WorldKind,
}

/// A world-body item as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldKind {
    /// An own-line comment inside a world body.
    Comment(Comment),
    /// `[volatile] tape NAME: ALPHABET;` — grammatical only in a `machine`
    /// block.
    Tape(TapeCst),
    State(StateCst),
    Graft(GraftCst),
    Bind(BindCst),
}

/// A `tape NAME: ALPHABET;` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeCst {
    pub name: String,
    pub name_span: Span,
    pub alphabet: String,
    pub alphabet_span: Span,
    /// `volatile tape …` — the band is a device (docs/tmt/language.md
    /// (volatile tapes)).
    pub volatile: bool,
    pub line: u32,
    /// First token (`volatile` or `tape`) start → `;` end.
    pub span: Span,
    pub trailing: Option<Comment>,
}

/// A `[entry] state NAME { rules }` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCst {
    pub entry: bool,
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    /// Column of the header's first token (`entry`/`state`).
    pub col: u32,
    /// Rules and own-line comments interleaved in source order.
    pub rules: Vec<RuleItem>,
    /// Header first token → closing `}` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub open_trailing: Vec<Comment>,
    pub close_trailing: Option<Comment>,
}

/// One state-body item, plus whether a blank line precedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleItem {
    pub blank_before: bool,
    pub kind: RuleKind,
}

/// A state-body item as written: an own-line comment or a rule. The rule is
/// boxed — a [`RuleCst`] dwarfs a [`Comment`], so an unboxed variant would
/// bloat every `RuleItem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleKind {
    Comment(Comment),
    Rule(Box<RuleCst>),
}

/// One `pattern -> action ;` rule, embedding the parser's [`Rule`] verbatim
/// plus a same-line trailing comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleCst {
    pub rule: Rule,
    pub trailing: Option<Comment>,
    /// Interior comments of a `call` transition's own binding list, keyed as
    /// [`AlphabetCst::interior`] is. A SIDE-CAR rather than a field on the
    /// embedded [`Rule`]: that type is handed to the AST verbatim, and the
    /// AST is contractually comment-independent. Empty for any rule whose
    /// transition is not a call (docs/tmt/fmt.md (interior comments)).
    pub call_args: Vec<(usize, Comment)>,
    /// Interior comments of every `with map` pair list nested inside the
    /// `call`'s binding list, keyed by `(binding-arg index, pair index)` —
    /// a map nests one level inside a binding argument, so its own comments
    /// need a second index alongside the one [`Self::call_args`] uses. Empty
    /// for any binding argument whose value carries no map, or whose map
    /// carries no interior comment.
    pub map_pairs: Vec<(usize, usize, Comment)>,
    /// Interior comments of the rule's pattern vector, keyed by the index
    /// of the cell each precedes, with the cell count meaning "before the
    /// closing `]`". A SIDE-CAR for the same reason [`Self::call_args`] is:
    /// the vector types are handed to the AST verbatim
    /// (docs/tmt/fmt.md (interior comments)).
    pub pattern_cells: Vec<(usize, Comment)>,
    /// Interior comments of the rule's `write` vector, keyed as
    /// [`Self::pattern_cells`] is. Empty when the rule has no write vector.
    pub write_cells: Vec<(usize, Comment)>,
    /// Interior comments of the rule's `move` vector, keyed as
    /// [`Self::pattern_cells`] is. Empty when the rule has no move vector.
    pub move_cells: Vec<(usize, Comment)>,
}

/// A `[entry] graft TARGET(args) [as NAME];` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraftCst {
    pub entry: bool,
    pub target: QualName,
    pub args: Vec<BindingArg>,
    /// Comments written INSIDE the list, in source order, each keyed by the
    /// index of the entry it precedes. An index equal to the entry count
    /// means "after the last entry, before the closer".
    ///
    /// Sparse and index-keyed rather than a per-entry wrapper, so the entry
    /// types are untouched and no AST-facing type carries trivia
    /// (docs/tmt/fmt.md (interior comments)).
    pub interior: Vec<(usize, Comment)>,
    /// Interior comments of every `with map` pair list nested inside this
    /// binding list, keyed by `(binding-arg index, pair index)` — mirrors
    /// [`RuleCst::map_pairs`]; a SIDE-CAR rather than a field on
    /// [`BindingArg`] for the same reason. Empty for any binding argument
    /// whose value carries no map, or whose map carries no interior
    /// comment.
    pub map_pairs: Vec<(usize, usize, Comment)>,
    /// `as NAME` instance name (name, span); required unless `entry`.
    pub as_name: Option<(String, Span)>,
    pub line: u32,
    /// Header first token → `;` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub trailing: Option<Comment>,
}

/// A `bind TARGET(args) as NAME;` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindCst {
    pub target: QualName,
    pub args: Vec<BindingArg>,
    /// Comments written INSIDE the list, in source order, each keyed by the
    /// index of the entry it precedes. An index equal to the entry count
    /// means "after the last entry, before the closer".
    ///
    /// Sparse and index-keyed rather than a per-entry wrapper, so the entry
    /// types are untouched and no AST-facing type carries trivia
    /// (docs/tmt/fmt.md (interior comments)).
    pub interior: Vec<(usize, Comment)>,
    /// Interior comments of every `with map` pair list nested inside this
    /// binding list, keyed by `(binding-arg index, pair index)` — mirrors
    /// [`RuleCst::map_pairs`]; a SIDE-CAR rather than a field on
    /// [`BindingArg`] for the same reason. Empty for any binding argument
    /// whose value carries no map, or whose map carries no interior
    /// comment.
    pub map_pairs: Vec<(usize, usize, Comment)>,
    /// `as NAME` — always present for a bind.
    pub as_name: (String, Span),
    pub line: u32,
    /// `bind` keyword start → `;` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub trailing: Option<Comment>,
}
