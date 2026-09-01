//! `.tmc` recursive-descent parser (spec's language chapter): tokens →
//! the green syntax tree via [`parse_green`]/[`parse_green_from_tokens`]
//! — the path the compiler front end and the language service both run.
//! [`parse`] is the convenience wrapper: source in, [`Program`] out,
//! going through that same green tree and
//! [`crate::syntax::extract_program`] (docs/core.md (syntax trees)).
//! `Parser::file` builds the green tree and nothing else. The
//! productions keep the values a caller still reads — the AST fragments
//! the `reparse_*` shims hand back to extraction, and the pattern and
//! write vector `check_char_arithmetic` inspects — and build no second
//! node tree of their own alongside it.
//!
//! The 27 reserved keywords live in one place, [`crate::lexer::RESERVED`]; the
//! parser is the sole enforcer — it rejects a keyword wherever a name is
//! expected. `deprecated` is contextual (an attribute word) and is not in that
//! set.

use std::rc::Rc;

use mtc_core::diagnostics::Span;
use mtc_core::syntax::{Checkpoint, GreenNode, SyntaxNode};

use crate::compiler::{CompileError, CompileErrorKind};
use crate::lexer::{Comment, LexMode, RESERVED, Token, TokenKind, lex_with};
use crate::syntax::{self, GreenSink, TmcKind};

/// The `.tmc` language acceptance-contract version (the spec's language
/// chapter). Pre-1.0 the version is `0.N` and N bumps on ANY grammar change;
/// at a declared 1.0 the axes activate (major = breaking, minor = additive).
/// There is no patch digit — spec-text corrections are errata and
/// implementation-conformance fixes never move it. This is the language's
/// first cut, so `0.1` (mirrors PM-1's `PMC_LANG_VERSION` discipline).
/// (An unreleased version amends in place; the bump discipline binds from the first release.)
pub const TMC_LANG_VERSION: &str = "0.1";

// ---------------------------------------------------------------------------
// AST — the flat program the front end (resolution, IR, codegen) consumes.
// ---------------------------------------------------------------------------

/// A whole `.tmc` program (or library — `machine` is `None` for a library).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub imports: Vec<Import>,
    pub alphabets: Vec<Alphabet>,
    pub routines: Vec<Routine>,
    pub graphs: Vec<Graph>,
    /// The single `machine` block; `None` in a library file. Parsing rejects a
    /// second `machine` block in one file (multiplicity `> 1`); the
    /// zero-in-a-program case is a later semantic check.
    pub machine: Option<Machine>,
}

/// One `use` list item: `use a, mylib::b as c;` yields two of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// `IDENT (:: IDENT)*` — `use mylib::plusOne;` → `["mylib", "plusOne"]`.
    pub path: Vec<String>,
    /// `as NAME` rebinds the bare name; the declared symbol is unchanged.
    pub alias: Option<String>,
    pub line: u32,
    /// The declaring namespace path; empty = file level.
    pub ns: Vec<String>,
    /// Path start → last segment end; an `as` alias is NOT included.
    pub span: Span,
}

impl Import {
    /// The bare name this import binds in its scope.
    pub fn binding(&self) -> &str {
        self.alias.as_deref().unwrap_or_else(|| {
            self.path
                .last()
                .expect("parser: import paths are non-empty")
        })
    }

    /// The full `::`-joined external symbol this import declares.
    pub fn full_path(&self) -> String {
        self.path.join("::")
    }
}

/// A single glyph or numeric symbol literal, with its source span. Numbers
/// keep the digits as WRITTEN (leading zeros preserved) for lossless reprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymLit {
    Glyph {
        value: String,
        span: Span,
    },
    Number {
        value: u32,
        written: String,
        span: Span,
    },
}

impl SymLit {
    pub fn span(&self) -> Span {
        match self {
            SymLit::Glyph { span, .. } | SymLit::Number { span, .. } => *span,
        }
    }

    /// True for a glyph literal, false for a numeric one — the kind a range's
    /// two endpoints must agree on, and the kind a pattern binding takes.
    pub fn is_glyph(&self) -> bool {
        matches!(self, SymLit::Glyph { .. })
    }
}

/// An `export? alphabet NAME { … }` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alphabet {
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    pub col: u32,
    pub exported: bool,
    pub ns: Vec<String>,
    pub elems: Vec<AlphabetElem>,
    pub doc: Option<Doc>,
}

/// One alphabet element: a single symbol, or an inclusive `lo..hi` range whose
/// endpoints are the same kind (`glyph..glyph` or `number..number`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlphabetElem {
    Single(SymLit),
    Range { lo: SymLit, hi: SymLit, span: Span },
}

/// A `routine`/`graph` signature: parameters in declaration order (= vector
/// positions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub params: Vec<SigParam>,
    /// `(` start → `)` end.
    pub span: Span,
}

/// One signature parameter, `tape NAME: ALPHABET` or `state NAME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigParam {
    pub kind: SigParamKind,
    pub name: String,
    pub name_span: Span,
    pub span: Span,
}

/// One `writes { … }` or `preserves { … }` contract clause on a signature
/// tape parameter: the brace-set's elements (the same element grammar as an
/// alphabet body — singles and ranges), the keyword's own span, and the
/// whole clause's span (keyword start → closing `}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractClause {
    pub elems: Vec<AlphabetElem>,
    pub kw_span: Span,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigParamKind {
    Tape {
        alphabet: String,
        alphabet_span: Span,
        volatile: bool,
        /// A declared `writes { … }` clause, signature-only (never present
        /// on a machine tape declaration).
        writes: Option<ContractClause>,
        /// A declared `preserves { … }` clause, signature-only.
        preserves: Option<ContractClause>,
    },
    State,
}

/// An `export? routine NAME(sig) { … }` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routine {
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    pub col: u32,
    pub exported: bool,
    pub ns: Vec<String>,
    pub sig: Signature,
    pub states: Vec<State>,
    pub grafts: Vec<Graft>,
    pub binds: Vec<Bind>,
    pub doc: Option<Doc>,
}

/// An `export? graph NAME(sig) { … }` declaration — the same shape as
/// [`Routine`], kept distinct because the front end treats the two reuse forms
/// differently (routine → `call`, graph → `graft`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graph {
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    pub col: u32,
    pub exported: bool,
    pub ns: Vec<String>,
    pub sig: Signature,
    pub states: Vec<State>,
    pub grafts: Vec<Graft>,
    pub binds: Vec<Bind>,
    pub doc: Option<Doc>,
}

/// The single `machine { … }` block — world data (tape declarations) and world
/// behavior (states/grafts/binds). It carries no name, namespace, or export
/// (a `machine` is never namespaced and never a reuse target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub line: u32,
    pub col: u32,
    pub span: Span,
    pub tapes: Vec<TapeDecl>,
    pub states: Vec<State>,
    pub grafts: Vec<Graft>,
    pub binds: Vec<Bind>,
    pub doc: Option<Doc>,
}

/// A `tape NAME: ALPHABET;` declaration (machine bodies only). Declaration
/// order is the tape's vector position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeDecl {
    pub name: String,
    pub name_span: Span,
    pub alphabet: String,
    pub alphabet_span: Span,
    /// `volatile tape …` — the band is a device (docs/tmt/language.md
    /// (volatile tapes)).
    pub volatile: bool,
    pub line: u32,
    pub span: Span,
}

/// A `[entry] state NAME { rules }` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub entry: bool,
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    pub col: u32,
    /// Rules in source order — order is table-row order is priority.
    pub rules: Vec<Rule>,
    pub span: Span,
    pub doc: Option<Doc>,
}

/// A `[entry] graft TARGET(args) [as NAME];` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graft {
    pub entry: bool,
    pub target: QualName,
    pub args: Vec<BindingArg>,
    /// The instance name; `None` only for an `entry graft` that omits it.
    pub as_name: Option<Ident>,
    pub line: u32,
    pub span: Span,
    pub doc: Option<Doc>,
}

/// A `bind TARGET(args) as NAME;` declaration — a named bound-call target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind {
    pub target: QualName,
    pub args: Vec<BindingArg>,
    pub as_name: Ident,
    pub line: u32,
    pub span: Span,
    pub doc: Option<Doc>,
}

/// A bare identifier with its span (state/instance/alias names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// A qualified name `IDENT (:: IDENT)*` — a `call`/`graft`/`bind` target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualName {
    pub segments: Vec<String>,
    /// First segment start → last segment end.
    pub span: Span,
}

impl QualName {
    /// The full `::`-joined name.
    pub fn joined(&self) -> String {
        self.segments.join("::")
    }
}

/// One `pattern -> action ;` rule (the classical triple; one rule = one step).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub pattern: Pattern,
    /// A leading `debugger` in the action.
    pub debugger: bool,
    pub write: Option<WriteVec>,
    pub mov: Option<MoveVec>,
    pub transition: Transition,
    pub line: u32,
    /// Pattern `[` start → `;` end.
    pub span: Span,
}

/// A bracketed match pattern `[cell, …]` — arity = the world's tape count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub cells: Vec<PatternCell>,
    /// `[` start → `]` end.
    pub span: Span,
}

/// One pattern cell, optionally binding its match with `as v`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternCell {
    pub kind: PatternCellKind,
    /// `as v`; forbidden on a wildcard (`* as v`).
    pub binding: Option<Binding>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternCellKind {
    Wildcard,
    Single(SymLit),
    Range { lo: SymLit, hi: SymLit },
}

/// A pattern-cell binding `as NAME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub name: String,
    pub span: Span,
}

/// A bracketed write vector `[cell, …]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteVec {
    pub cells: Vec<WriteCell>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCell {
    pub kind: WriteCellKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteCellKind {
    /// `-` — keep the cell's current symbol.
    Keep,
    /// A literal glyph or number.
    Lit(SymLit),
    /// A substitution `{expr}`, where `expr` is the assembler's arithmetic
    /// grammar (`+ - * %`, parens, i64) over the rule's pattern bindings
    /// (docs/tmt/language.md (substitution)). A bare name is passthrough; any
    /// other shape is a fold. The binding-kind legality (arithmetic is
    /// numeric-only) is checked at parse time; the fold itself happens during
    /// range expansion.
    Subst { expr: FoldExprNode },
}

/// A binary operator in a write-cell fold expression. `*`/`%` bind tighter
/// than `+`/`-`; all are left-associative
/// (docs/tmt/language.md (substitution)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldOp {
    Add,
    Sub,
    Mul,
    Rem,
}

/// One node of a write-cell fold expression tree, with its source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldExprNode {
    pub kind: FoldExprKind,
    pub span: Span,
}

/// The shape of a [`FoldExprNode`]: a pattern-binding reference, an integer
/// literal, or a binary application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoldExprKind {
    /// An in-scope pattern binding name.
    Var(String),
    Int(i64),
    Bin {
        op: FoldOp,
        lhs: Box<FoldExprNode>,
        rhs: Box<FoldExprNode>,
    },
}

/// A bracketed move vector `[dir, …]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveVec {
    pub cells: Vec<MoveCell>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveCell {
    pub dir: MoveDir,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDir {
    /// `<`
    Left,
    /// `>`
    Right,
    /// `.`
    Stay,
}

/// A rule's control transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// `goto NAME` (`explicit`) or the bare-name sugar `NAME` (`!explicit`).
    Goto {
        name: String,
        explicit: bool,
        span: Span,
    },
    /// `call TARGET(binding) then CONTINUATION`.
    Call {
        target: QualName,
        args: Vec<BindingArg>,
        then: Continuation,
        span: Span,
    },
    Return {
        span: Span,
    },
    Stop {
        span: Span,
    },
    Halt {
        span: Span,
    },
    /// The transition was omitted — stay in the current state. Legal only when
    /// the rule carries at least one of `write` / `move` / `debugger`; it
    /// resolves to a self-`goto` at expansion time (the rule loops to its own
    /// state, or, in a grafted graph, to its own spliced instance)
    /// (docs/tmt/language.md (rules)).
    Stay {
        span: Span,
    },
}

/// A `call … then` continuation: a state, or a terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continuation {
    State { name: String, span: Span },
    Return { span: Span },
    Stop { span: Span },
    Halt { span: Span },
}

/// One binding argument `name = target [with map { … }]` or
/// `name = terminator`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingArg {
    /// The parameter name being bound (the LHS of `=`).
    pub name: String,
    pub name_span: Span,
    pub value: BindingValue,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingValue {
    /// A bare name — a tape target or a state continuation; resolution decides.
    /// A `with map { … }` (when present) makes it definitively a tape target.
    Named {
        target: String,
        target_span: Span,
        map: Option<SymMap>,
    },
    /// `return` / `stop` / `halt` — a continuation terminator.
    Terminator { kind: TermKind, span: Span },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermKind {
    Return,
    Stop,
    Halt,
}

/// A `with map { pairs }` per-tape symbol map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymMap {
    pub pairs: Vec<MapPair>,
    /// `map` keyword start → `}` end.
    pub span: Span,
}

/// One map pair `src -> dst` (bidirectional) or `src => dst` (read-only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapPair {
    pub src: SymLit,
    pub dst: SymLit,
    pub arrow: MapArrow,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapArrow {
    /// `->` — read and write-back.
    Bidirectional,
    /// `=>` — read-only (collapse allowed, no write-back).
    ReadOnly,
}

/// A declaration's reduced doc/attention run — the front-end shape a future
/// hover/lint consumer reads (raw sigils and spans dropped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// `?` lines, joined into paragraphs (blank `?` splits paragraphs).
    pub paragraphs: Vec<String>,
    /// Bare-prose `!` lines (no `[attr]` prefix), verbatim, in source order.
    pub attention: Vec<String>,
    /// The `[deprecated]` message (possibly empty), or `None`.
    pub deprecated: Option<String>,
}

// ---------------------------------------------------------------------------
// parse / parse_green / parse_green_from_tokens
// ---------------------------------------------------------------------------

/// Source → AST, through the one parse path this crate has: a
/// `WithComments` lex, the green syntax tree, then extraction
/// (docs/core.md (syntax trees)).
///
/// The convenience wrapper for callers that want only the `Program` —
/// it keeps nothing else, and it is the only parse function here that
/// yields one. A caller needing the token stream alongside it uses
/// `compiler::analyze`; a caller needing the green tree uses
/// `compiler::analyze_staged`, which is the one that retains it.
pub fn parse(source: &str) -> Result<Program, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    Ok(crate::syntax::extract_program(
        &SyntaxNode::new_root(green),
        source,
    ))
}

/// A `WithComments` token stream minus its comment trivia. `Comment` is the
/// only trivia kind `.tmc` has at the lexer level — doc and attention lines
/// are semantic tokens both lex modes emit — so filtering it is the whole
/// job.
///
/// It lives at the parser level because the compiler front end is its main
/// caller: [`crate::compiler::analyze`] lexes `WithComments` for the green
/// parse and hands the filtered stream on to the lint layer, which walks
/// token neighbourhoods by adjacency. The language service's own
/// position-classification walks use it for the same reason — a comment must
/// never shift a context decision.
///
/// Element for element, spans included, the result equals a
/// `LexMode::WithoutComments` lex of the same source: the lexer's mode gate
/// decides only whether a `Comment` token is pushed, never how any other
/// token is consumed. That law is pinned over the shipped corpus by
/// `tests/tmc_green_analyze.rs::corpus_significant_tokens_equal_a_comment_free_lex`
/// (which re-derives the filter inline) and against this function itself by
/// `tests::significant_tokens_is_the_comment_free_lex`.
pub(crate) fn significant_tokens(tokens: &[Token]) -> Vec<Token> {
    tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
        .cloned()
        .collect()
}

/// source → green syntax tree (docs/core.md (syntax trees)). Lexes
/// `WithComments` and hands both halves to [`parse_green_from_tokens`].
pub fn parse_green(source: &str) -> Result<Rc<GreenNode>, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    parse_green_from_tokens(source, &tokens)
}

#[cfg(test)]
thread_local! {
    /// Test-only call counter for [`parse_green_from_tokens`]
    /// (docs/core.md (syntax trees)), read by `lsp::tests` to MEASURE —
    /// rather than assume — that a tree-backed language-service request
    /// costs zero additional parses once `DocState.green` is populated.
    /// Thread-local, not a shared global: `cargo test`'s default harness
    /// runs each test on its own thread, and this crate has hundreds of
    /// tests calling this function, so a process-wide counter would be
    /// corrupted by unrelated concurrent tests — a thread-local one
    /// isolates each test's own calls from every other thread's.
    pub(crate) static PARSE_GREEN_FROM_TOKENS_CALLS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// Already-lexed tokens → green syntax tree, for callers that keep the
/// token stream alongside the tree. `tokens` MUST be a
/// `LexMode::WithComments` lex of `source`: [`crate::syntax::layout`]
/// reconstructs verbatim token text and trivia from the two together, so
/// a comment-free stream would lose every comment's own text and break
/// the `text() == source` law. An empty `tokens` slice panics — every
/// real lex result is EOF-terminated.
///
/// Runs `Parser::file`'s one grammar walk with a green sink attached:
/// the sink mirrors token consumption and node boundaries alongside the
/// parser logic, and the tree it builds is the walk's only product
/// (docs/core.md (syntax trees)). The parser itself walks the
/// significant tokens alone — [`significant_tokens`] strips the comment
/// trivia the layout schedule already carries.
pub fn parse_green_from_tokens(
    source: &str,
    tokens: &[Token],
) -> Result<Rc<GreenNode>, CompileError> {
    #[cfg(test)]
    PARSE_GREEN_FROM_TOKENS_CALLS.with(|c| c.set(c.get() + 1));
    let entries = syntax::layout(source, tokens);
    let sig = significant_tokens(tokens);
    let eof_pos = sig.len() - 1;
    let mut sink = GreenSink::new(entries);
    sink.start(TmcKind::Root);
    let sink = Parser {
        tokens: &sig,
        pos: 0,
        machine_seen: false,
        sink: Some(sink),
        recovered: None,
    }
    .file()?;
    Ok(sink
        .expect("parse_green_from_tokens always seeds a sink before calling file()")
        .finish_tree(eof_pos))
}

/// A resilient parse's product: the tree is ALWAYS built — broken
/// regions wrapped in [`TmcKind::Error`] nodes — and lossless
/// (`text() == source`); `errors` carries each recovery's error in
/// source order, empty on a clean document, where the tree is
/// byte-identical to [`parse_green_from_tokens`]'s.
pub struct ResilientParse {
    pub green: Rc<GreenNode>,
    pub errors: Vec<CompileError>,
}

/// [`parse_green_from_tokens`]'s error-recovering twin (docs/core.md
/// (syntax trees), error recovery) — the language service's entry, so
/// the editor keeps a CURRENT tree mid-edit. The batch pipeline never
/// calls this: its fatal contract (first error, no tree) is unchanged.
/// The first recorded error is always the same error, kind and span,
/// that the fatal entry reports on the same input.
pub fn parse_green_resilient(source: &str, tokens: &[Token]) -> ResilientParse {
    let entries = syntax::layout(source, tokens);
    let sig = significant_tokens(tokens);
    let eof_pos = sig.len() - 1;
    let mut sink = GreenSink::new(entries);
    sink.start(TmcKind::Root);
    let parser = Parser {
        tokens: &sig,
        pos: 0,
        machine_seen: false,
        sink: Some(sink),
        recovered: Some(Vec::new()),
    };
    let (sink, errors) = parser
        .file_resilient()
        .expect("resilient mode recovers every item error");
    ResilientParse {
        green: sink
            .expect("parse_green_resilient always seeds a sink")
            .finish_tree(eof_pos),
        errors,
    }
}

/// Which keyword introduced the `routine`/`graph` declaration
/// `Parser::parse_reuse` is walking. Both keywords take that one
/// production and open one green node kind ([`TmcKind::Reuse`]), so the
/// carrier survives only to pick the error wording when the name after
/// the keyword is missing or is a reserved word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReuseCarrier {
    Routine,
    Graph,
}

/// One line of a doc/attention run, plus whether a blank line precedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRunItem {
    pub blank_before: bool,
    pub kind: DocRunKind,
}

/// A doc/attention run's line shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocRunKind {
    /// A `?` line. `text` is the lexer's payload verbatim.
    Doc { text: String, span: Span },
    /// A `!` line. `attr` is `Some` when the payload opens with a valid
    /// `[ident]` attribute (v1: only `[deprecated]`). `text` is the FULL raw
    /// payload verbatim, attribute prefix included.
    Attention {
        attr: Option<AttrCst>,
        text: String,
        span: Span,
    },
    /// An ordinary comment inside the run.
    Comment(Comment),
}

/// An attention line's leading `[ident]` attribute; `span` covers the
/// identifier alone, not the brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrCst {
    pub name: String,
    pub span: Span,
}

/// Reduce a doc/attention run into a [`Doc`] — `None` for an empty run.
/// `?` lines join into paragraphs (an empty `?` line splits, leading/trailing
/// blanks produce no empty paragraph); a `[deprecated]` attention line becomes
/// `deprecated`; bare-prose `!` lines become `attention`; comments and empty
/// lines contribute nothing. Mirrors PM-1's `reduce_doc_run`. `pub(crate)`
/// rather than private: `crate::syntax::extract::extract_doc` is the
/// production caller, folding the green tree's own retokenized run the
/// same way; its own tests also reduce a green-side reparsed run
/// directly, to check equality where a comment-interleaved run makes raw
/// item equality impossible (a one-pass reparse drops interleaved
/// comments as trivia).
pub(crate) fn reduce_doc_run(doc_run: &[DocRunItem]) -> Option<Doc> {
    if doc_run.is_empty() {
        return None;
    }
    let mut paragraphs = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut attention = Vec::new();
    let mut deprecated = None;
    for item in doc_run {
        match &item.kind {
            DocRunKind::Doc { text, .. } => {
                if text.is_empty() {
                    if !current.is_empty() {
                        paragraphs.push(current.join(" "));
                        current.clear();
                    }
                } else {
                    current.push(text.as_str());
                }
            }
            DocRunKind::Attention { attr, text, .. } => match attr {
                Some(a) if a.name == "deprecated" => {
                    let close = text.find(']').expect(
                        "parser: a `deprecated`-tagged attention line always has a closing `]`",
                    );
                    deprecated = Some(text[close + 1..].trim().to_string());
                }
                _ if text.is_empty() => {}
                _ => attention.push(text.clone()),
            },
            DocRunKind::Comment(_) => {}
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    Some(Doc {
        paragraphs,
        attention,
        deprecated,
    })
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

fn join(a: Span, b: Span) -> Span {
    Span {
        start: a.start,
        end: b.end,
    }
}

fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(n) => format!("`{n}`"),
        TokenKind::Number(v, _) => format!("`{v}`"),
        TokenKind::Glyph(g) => format!("glyph `{g}`"),
        TokenKind::DotDot => "`..`".into(),
        TokenKind::Arrow => "`->`".into(),
        TokenKind::FatArrow => "`=>`".into(),
        TokenKind::ColonColon => "`::`".into(),
        TokenKind::Dot => "`.`".into(),
        TokenKind::Dash => "`-`".into(),
        TokenKind::Plus => "`+`".into(),
        TokenKind::Eq => "`=`".into(),
        TokenKind::Star => "`*`".into(),
        TokenKind::Percent => "`%`".into(),
        TokenKind::Lt => "`<`".into(),
        TokenKind::Gt => "`>`".into(),
        TokenKind::LBracket => "`[`".into(),
        TokenKind::RBracket => "`]`".into(),
        TokenKind::LBrace => "`{`".into(),
        TokenKind::RBrace => "`}`".into(),
        TokenKind::LParen => "`(`".into(),
        TokenKind::RParen => "`)`".into(),
        TokenKind::Comma => "`,`".into(),
        TokenKind::Semi => "`;`".into(),
        TokenKind::Colon => "`:`".into(),
        TokenKind::At => "`@`".into(),
        TokenKind::Bang => "`!`".into(),
        TokenKind::Eof => "end of file".into(),
        TokenKind::Comment(_) => "a comment".into(),
        TokenKind::DocLine(_) => "a doc line".into(),
        TokenKind::AttentionLine(_) => "an attention line".into(),
    }
}

struct Parser<'a> {
    /// Significant (comment-free) tokens only.
    tokens: &'a [Token],
    pos: usize,
    /// A `machine` block has already been seen (multiplicity guard).
    machine_seen: bool,
    /// Green-tree emission, when this walk is [`parse_green`]'s: `bump()`
    /// mirrors every consumed token into it, and the `g_*` helpers below
    /// bracket node boundaries. `None` only for [`bare_parser`]'s partial
    /// re-parse of an already-retokenized node's own tokens (the
    /// `reparse_*` shims), which never re-emits a green tree, only a
    /// value — those helpers are then no-ops, so the underlying grammar
    /// walk is unaffected by whether a sink is attached.
    sink: Option<GreenSink>,
    /// `Some` = resilient mode (`parse_green_resilient`): item-level
    /// errors are recorded here and recovered from at the loop seams;
    /// `None` = the fatal contract (every other entry).
    recovered: Option<Vec<CompileError>>,
}

/// What one `Parser::top_item` iteration decided: the level is done
/// (Eof at file level, or the terminator was consumed), or one item
/// was parsed and the loop continues.
enum Step {
    Done,
    Continue,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn bump(&mut self) {
        if !matches!(self.tokens[self.pos].kind, TokenKind::Eof) {
            if let Some(sink) = &mut self.sink {
                sink.token(self.pos, syntax::token_kind(&self.tokens[self.pos].kind));
            }
            self.pos += 1;
        }
    }

    /// Open a green node, flushing the upcoming token's trivia into the
    /// PARENT first — the trivia-placement rule: a node starts at its
    /// first significant token, so whitespace/comments before it belong
    /// to whatever is still open. No-op when `sink` is `None`.
    fn g_flush_start(&mut self, kind: TmcKind) {
        if let Some(sink) = &mut self.sink {
            sink.flush(self.pos);
            sink.start(kind);
        }
    }

    /// Close the innermost open green node. No-op when `sink` is `None`.
    fn g_finish(&mut self) {
        if let Some(sink) = &mut self.sink {
            sink.finish();
        }
    }

    /// Flush the upcoming token's trivia into the parent, then mark a
    /// green checkpoint for a later [`Self::g_start_at`] — for
    /// productions that only learn their node kind (or discover an
    /// optional prefix, e.g. `export`) after parsing has started.
    /// `None` when `sink` is `None`.
    fn g_checkpoint(&mut self) -> Option<Checkpoint> {
        self.sink.as_mut().map(|sink| {
            sink.flush(self.pos);
            sink.checkpoint()
        })
    }

    /// Open a green node retroactively at a checkpoint taken by
    /// [`Self::g_checkpoint`]. No-op when either is `None`.
    fn g_start_at(&mut self, cp: Option<Checkpoint>, kind: TmcKind) {
        if let (Some(sink), Some(cp)) = (&mut self.sink, cp) {
            sink.start_at(cp, kind);
        }
    }

    fn err_at(t: &Token, kind: CompileErrorKind) -> CompileError {
        CompileError {
            span: t.span(),
            kind,
        }
    }

    fn expected(t: &Token, what: &'static str) -> CompileError {
        Self::err_at(
            t,
            CompileErrorKind::Expected {
                what,
                found: describe(&t.kind),
            },
        )
    }

    fn expect(&mut self, kind: &TokenKind, what: &'static str) -> Result<Token, CompileError> {
        if &self.peek().kind == kind {
            let t = self.peek().clone();
            self.bump();
            Ok(t)
        } else {
            Err(Self::expected(self.peek(), what))
        }
    }

    /// Read a non-reserved identifier where a name is expected, returning its
    /// text and span. A reserved keyword here is a `ReservedName` error; any
    /// other token is `Expected { what }`.
    fn name(&mut self, what: &'static str) -> Result<(String, Span), CompileError> {
        let t = self.peek().clone();
        let TokenKind::Ident(n) = &t.kind else {
            return Err(Self::expected(&t, what));
        };
        if RESERVED.contains(&n.as_str()) {
            return Err(Self::err_at(
                &t,
                CompileErrorKind::ReservedName {
                    name: n.clone(),
                    what,
                },
            ));
        }
        self.bump();
        Ok((n.clone(), t.span()))
    }

    /// True iff the current token is the keyword `w`.
    fn at_kw(&self, w: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(k) if k == w)
    }

    // ---- doc runs ---------------------------------------------------------

    /// Consume a doc/attention run at the current position (the caller
    /// has confirmed the leading token is a `?`/`!` line), enforcing its
    /// ordering and attribute rules, and answer the run's first-line
    /// span — which is what a `DanglingDocRun` error is reported at.
    ///
    /// The items themselves go nowhere on this walk: extraction rebuilds
    /// them from the green tree, retokenizing a DOC_RUN node's own
    /// extent and re-running the same production through
    /// [`reparse_doc_items`]. `0` is passed for `prev_end_line` because
    /// the only thing it seeds — the first item's `blank_before` — is
    /// discarded here along with the items; the reparse supplies the
    /// real value.
    fn doc_run(&mut self) -> Result<Span, CompileError> {
        Ok(self.doc_run_items(0)?.1)
    }

    /// [`Self::doc_run`]'s item-collecting form. `prev_end_line` is the
    /// end line of whatever precedes the run, seeding the first item's
    /// `blank_before`; every later item measures its gap against the
    /// item before it.
    ///
    /// The items are `DocLine`/`AttentionLine` shapes only. [`DocRunKind`]'s
    /// `Comment` variant is not dead — a comment written inside a run is
    /// green-tree trivia, and `crate::syntax::extract::extract_doc_items`
    /// constructs the item for it there, interleaving it with what this
    /// production returns.
    fn doc_run_items(
        &mut self,
        mut prev_end_line: u32,
    ) -> Result<(Vec<DocRunItem>, Span), CompileError> {
        let first_span = self.peek().span();
        let mut items: Vec<DocRunItem> = Vec::new();
        let mut seen_attention = false;
        let mut seen_deprecated = false;
        loop {
            let t = self.peek().clone();
            match &t.kind {
                TokenKind::DocLine(text) => {
                    if seen_attention {
                        return Err(Self::err_at(&t, CompileErrorKind::DocLineOrder));
                    }
                    let text = text.clone();
                    self.bump();
                    let blank_before = t.line > prev_end_line + 1;
                    prev_end_line = t.line;
                    items.push(DocRunItem {
                        blank_before,
                        kind: DocRunKind::Doc {
                            text,
                            span: t.span(),
                        },
                    });
                }
                TokenKind::AttentionLine(text) => {
                    let text = text.clone();
                    // The lexer folds a `! [ident] …` line into ONE
                    // `AttentionLine` payload (docs/core.md (syntax
                    // trees)) — `[ident]` is never its own token, so ATTR
                    // can only ever wrap this single token, never a
                    // sub-span of it. Whether it does is known only
                    // after `parse_attr` reads the payload below, i.e.
                    // after the token is already bumped — hence the
                    // checkpoint, taken before, to wrap retroactively.
                    let attr_cp = self.g_checkpoint();
                    self.bump();
                    seen_attention = true;
                    let attr = Self::parse_attr(&text, &t);
                    if attr.is_some() {
                        self.g_start_at(attr_cp, TmcKind::Attr);
                        self.g_finish(); // Attr — wraps the one AttentionLine token
                    }
                    if let Some(a) = &attr {
                        if a.name == "deprecated" {
                            if seen_deprecated {
                                return Err(CompileError {
                                    span: a.span,
                                    kind: CompileErrorKind::DuplicateAttribute,
                                });
                            }
                            seen_deprecated = true;
                        } else {
                            return Err(CompileError {
                                span: a.span,
                                kind: CompileErrorKind::UnknownAttribute(a.name.clone()),
                            });
                        }
                    }
                    let blank_before = t.line > prev_end_line + 1;
                    prev_end_line = t.line;
                    items.push(DocRunItem {
                        blank_before,
                        kind: DocRunKind::Attention {
                            attr,
                            text,
                            span: t.span(),
                        },
                    });
                }
                _ => break,
            }
        }
        Ok((items, first_span))
    }

    /// Parse a leading `[ident]` attribute off an attention line's payload —
    /// the exact shape `[` ident `]` at the very start (anything else = no
    /// attribute, `None`). The span covers the identifier alone; column math is
    /// char-counted throughout (`token.len` vs the stored payload's char count
    /// differ by the 0-or-1 leading space the lexer stripped).
    fn parse_attr(text: &str, token: &Token) -> Option<AttrCst> {
        let chars: Vec<char> = text.chars().collect();
        if chars.first() != Some(&'[') {
            return None;
        }
        let close = chars.iter().position(|&c| c == ']')?;
        let ident_chars = &chars[1..close];
        let (first, rest) = ident_chars.split_first()?;
        if !(first.is_alphabetic() || *first == '_') {
            return None;
        }
        if !rest.iter().all(|c| c.is_alphanumeric() || *c == '_') {
            return None;
        }
        let name: String = ident_chars.iter().collect();
        let stripped = token.len - 1 - text.chars().count() as u32;
        let bracket_col = token.col + 1 + stripped;
        let start_col = bracket_col + 1;
        let end_col = start_col + name.chars().count() as u32;
        Some(AttrCst {
            name,
            span: Span::new(token.line, start_col, token.line, end_col),
        })
    }

    // ---- top level --------------------------------------------------------

    /// The whole file is the `ns == []` namespace level. Hands back the
    /// (possibly `None`) green sink alongside the items: `self` is
    /// consumed by value, so this is the only place it can escape.
    fn file(mut self) -> Result<Option<GreenSink>, CompileError> {
        self.top_items(&[], None)?;
        Ok(self.sink)
    }

    /// [`Self::file`] in resilient mode: also hands back the recovered
    /// errors. `Err` is unreachable — every item error is recovered at
    /// the loop seams — but the signature keeps the walk's own `?`s.
    #[allow(clippy::type_complexity)]
    fn file_resilient(mut self) -> Result<(Option<GreenSink>, Vec<CompileError>), CompileError> {
        self.top_items(&[], None)?;
        Ok((
            self.sink,
            self.recovered
                .expect("file_resilient runs in resilient mode"),
        ))
    }

    /// True iff the current token starts a declaration that accepts a doc run.
    fn next_is_top_doc_accepting(&self) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(w)
            if matches!(w.as_str(),
                "export" | "alphabet" | "routine" | "graph" | "machine" | "namespace"))
    }

    /// One namespace level's item loop — the recovery seam
    /// (docs/core.md (syntax trees), error recovery). Consumes through
    /// the block's closing `}` (or to EOF at file level). In fatal mode
    /// (`recovered: None`) an item error propagates unchanged; in
    /// resilient mode the error is recorded, the sink unwinds to this
    /// loop's depth, everything since the iteration's checkpoint is
    /// retro-wrapped into an ERROR node together with the tokens
    /// skipped to the next sync point, and the loop continues — one
    /// broken item never takes its neighbours' parse with it. Mirrors
    /// the sibling `.pmc` parser's wrapper of the same shape.
    fn top_items(
        &mut self,
        ns: &[String],
        terminator: Option<&TokenKind>,
    ) -> Result<(), CompileError> {
        loop {
            let cp_for_recovery = self.g_checkpoint();
            let depth = self.sink.as_ref().map(GreenSink::open_depth);
            match self.top_item(ns, terminator, cp_for_recovery) {
                Ok(Step::Done) => return Ok(()),
                Ok(Step::Continue) => {}
                Err(e) => {
                    if self.recovered.is_none() {
                        return Err(e);
                    }
                    self.recovered.as_mut().expect("checked above").push(e);
                    if let (Some(sink), Some(d)) = (&mut self.sink, depth) {
                        sink.finish_to(d);
                    }
                    self.g_start_at(cp_for_recovery, TmcKind::Error);
                    self.skip_to_sync(terminator);
                    self.g_finish(); // Error
                    if matches!(self.peek().kind, TokenKind::Eof) {
                        // Nothing left to resync on — an unclosed block
                        // ends at Eof with its error recorded.
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Skip-and-emit resynchronization: consumes tokens into the open
    /// ERROR node until a token that can start the next item (a
    /// doc/attention line, or one of the top-level keywords), the
    /// level's terminator (left for the loop to consume), or Eof. A `;`
    /// is consumed INTO the region and ends it. Always makes progress:
    /// the sync check is skipped on the first token, since a failed
    /// parse may have consumed nothing and its own first token may
    /// itself be a sync token.
    fn skip_to_sync(&mut self, terminator: Option<&TokenKind>) {
        let mut first = true;
        loop {
            let t = self.peek().clone();
            if matches!(t.kind, TokenKind::Eof) {
                return;
            }
            if let Some(term) = terminator
                && &t.kind == term
            {
                return;
            }
            let sync = match &t.kind {
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_) => true,
                TokenKind::Ident(w) => matches!(
                    w.as_str(),
                    "use" | "alphabet" | "routine" | "graph" | "machine" | "namespace" | "export"
                ),
                _ => false,
            };
            if sync && !first {
                return;
            }
            let semi = matches!(t.kind, TokenKind::Semi);
            self.bump();
            first = false;
            if semi {
                return;
            }
        }
    }

    /// One iteration of [`Self::top_items`]'s loop — the former loop
    /// body, factored out so the recovery wrapper owns the seam.
    fn top_item(
        &mut self,
        ns: &[String],
        terminator: Option<&TokenKind>,
        cp_for_recovery: Option<Checkpoint>,
    ) -> Result<Step, CompileError> {
        {
            // Green checkpoint for whichever node this item turns out to
            // be: taken at the top of the iteration, BEFORE the doc run
            // (if any) — mirrors PM's `fn_cp` placement
            // (crates/post-machine/src/parser.rs) so `g_start_at` below
            // retro-wraps the run, an `export` prefix when present, and
            // the header onward, all from one checkpoint. A declaration
            // retro-wraps its bound doc run by design — see this
            // crate's `syntax` module doc for the decision and its
            // reasoning (docs/core.md (syntax trees)). Unused only when
            // this token starts no valid top-level declaration at all;
            // harmless, a fresh checkpoint is taken every loop
            // iteration.
            let cp = cp_for_recovery;
            if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                self.g_flush_start(TmcKind::DocRun);
                let first_span = self.doc_run()?;
                self.g_finish(); // DocRun
                if !self.next_is_top_doc_accepting() {
                    return Err(CompileError {
                        span: first_span,
                        kind: CompileErrorKind::DanglingDocRun,
                    });
                }
            }
            let t = self.peek().clone();
            match (&t.kind, terminator) {
                (TokenKind::Eof, None) => return Ok(Step::Done),
                (TokenKind::Eof, Some(_)) => {
                    return Err(Self::expected(&t, "`}` to close the namespace block"));
                }
                (k, Some(term)) if k == term => {
                    self.bump();
                    return Ok(Step::Done);
                }
                _ => {}
            }
            match &t.kind {
                TokenKind::Ident(w) => match w.as_str() {
                    "use" => {
                        self.g_start_at(cp, TmcKind::Use);
                        self.parse_use()?;
                        self.g_finish(); // Use — closes right after the `;`
                    }
                    "alphabet" => {
                        self.g_start_at(cp, TmcKind::Alphabet);
                        self.parse_alphabet()?;
                        self.g_finish(); // Alphabet
                    }
                    "routine" => {
                        self.g_start_at(cp, TmcKind::Reuse);
                        self.parse_reuse(ReuseCarrier::Routine)?;
                        self.g_finish(); // Reuse — closes right after the `}`
                    }
                    "graph" => {
                        self.g_start_at(cp, TmcKind::Reuse);
                        self.parse_reuse(ReuseCarrier::Graph)?;
                        self.g_finish(); // Reuse — closes right after the `}`
                    }
                    "namespace" => {
                        self.g_start_at(cp, TmcKind::Namespace);
                        self.parse_namespace(ns)?;
                        self.g_finish(); // Namespace — closes right after the `}`
                    }
                    "machine" => {
                        if !ns.is_empty() {
                            return Err(Self::err_at(
                                &t,
                                CompileErrorKind::Expected {
                                    what: "a declaration (a `machine` block cannot be nested in a namespace)",
                                    found: describe(&t.kind),
                                },
                            ));
                        }
                        if self.machine_seen {
                            return Err(Self::err_at(&t, CompileErrorKind::MultipleMachines));
                        }
                        self.machine_seen = true;
                        self.g_start_at(cp, TmcKind::Machine);
                        self.parse_machine()?;
                        self.g_finish(); // Machine — closes right after the `}`
                    }
                    "export" => {
                        self.bump();
                        let t2 = self.peek().clone();
                        match &t2.kind {
                            TokenKind::Ident(w2) if w2 == "alphabet" => {
                                self.g_start_at(cp, TmcKind::Alphabet);
                                self.parse_alphabet()?;
                                self.g_finish(); // Alphabet — `export` included
                            }
                            TokenKind::Ident(w2) if w2 == "routine" => {
                                self.g_start_at(cp, TmcKind::Reuse);
                                self.parse_reuse(ReuseCarrier::Routine)?;
                                self.g_finish(); // Reuse — `export` included
                            }
                            TokenKind::Ident(w2) if w2 == "graph" => {
                                self.g_start_at(cp, TmcKind::Reuse);
                                self.parse_reuse(ReuseCarrier::Graph)?;
                                self.g_finish(); // Reuse — `export` included
                            }
                            _ => {
                                return Err(Self::expected(
                                    &t2,
                                    "`alphabet`, `routine`, or `graph` after `export`",
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(Self::expected(&t, "a top-level declaration"));
                    }
                },
                _ => return Err(Self::expected(&t, "a top-level declaration")),
            }
        }
        Ok(Step::Continue)
    }

    fn parse_use(&mut self) -> Result<(), CompileError> {
        self.bump(); // `use`
        loop {
            self.g_flush_start(TmcKind::UsePath);
            self.name("an imported name")?;
            while matches!(self.peek().kind, TokenKind::ColonColon) {
                self.bump();
                self.name("a path segment")?;
            }
            if self.at_kw("as") {
                self.bump();
                self.name("an alias")?;
            }
            self.g_finish(); // UsePath — the alias, if any, is its last token
            let sep = self.peek().clone();
            match sep.kind {
                TokenKind::Comma => self.bump(),
                TokenKind::Semi => {
                    self.bump();
                    break;
                }
                _ => return Err(Self::expected(&sep, "`,` or `;`")),
            }
        }
        Ok(())
    }

    fn parse_alphabet(&mut self) -> Result<(), CompileError> {
        self.bump(); // `alphabet`
        self.name("an alphabet name")?;
        self.expect(&TokenKind::LBrace, "`{` to open the alphabet body")?;
        self.alphabet_elems()?;
        self.expect(&TokenKind::RBrace, "`}` to close the alphabet body")?;
        Ok(())
    }

    /// An alphabet body's element loop, from just past the opening `{`
    /// up to (not consuming) the closing `}`. Split out of
    /// [`Self::parse_alphabet`] so [`reparse_alphabet_elems`] runs this
    /// exact loop rather than a second copy of it: the comma/`}`
    /// separator rule has one owner. An empty body (`{ }`) skips the
    /// loop and yields no elements, which is why the `RBrace` pre-check
    /// sits inside here rather than at the call site — the shim hands
    /// over a whole declaration's tokens and cannot make that decision
    /// itself.
    fn alphabet_elems(&mut self) -> Result<Vec<AlphabetElem>, CompileError> {
        let mut elems: Vec<AlphabetElem> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBrace) {
            loop {
                elems.push(self.alphabet_elem()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RBrace => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `}`")),
                }
            }
        }
        Ok(elems)
    }

    fn alphabet_elem(&mut self) -> Result<AlphabetElem, CompileError> {
        let (lo, hi) = self.sym_or_range()?;
        Ok(match hi {
            None => AlphabetElem::Single(lo),
            Some(hi) => {
                let span = join(lo.span(), hi.span());
                AlphabetElem::Range { lo, hi, span }
            }
        })
    }

    fn parse_namespace(&mut self, ns: &[String]) -> Result<(), CompileError> {
        self.bump(); // `namespace`
        let (name, _) = self.name("a namespace name")?;
        self.expect(&TokenKind::LBrace, "`{` to open the namespace body")?;
        let mut child = ns.to_vec();
        child.push(name);
        self.top_items(&child, Some(&TokenKind::RBrace))
    }

    fn parse_reuse(&mut self, carrier: ReuseCarrier) -> Result<(), CompileError> {
        self.bump(); // `routine` / `graph`
        let what = match carrier {
            ReuseCarrier::Routine => "a routine name",
            ReuseCarrier::Graph => "a graph name",
        };
        self.name(what)?;
        self.signature()?;
        // WORLD wraps the `{ … }` body — the shape `machine`/`routine`/
        // `graph` share (docs/tmt/language.md (worlds)); see the
        // `syntax` module doc for why it gets its own node kind rather
        // than folding into REUSE/MACHINE directly. Opened before the
        // `{` so the brace itself is WORLD's first token.
        self.g_flush_start(TmcKind::World);
        self.expect(&TokenKind::LBrace, "`{` to open the body")?;
        self.world_body(false)?;
        self.g_finish(); // World — closes right after the closing `}`
        Ok(())
    }

    fn parse_machine(&mut self) -> Result<(), CompileError> {
        self.bump(); // `machine`
        // WORLD wraps the `{ … }` body — see `parse_reuse`'s matching
        // comment; the same shared shape, no signature to skip over here.
        self.g_flush_start(TmcKind::World);
        self.expect(&TokenKind::LBrace, "`{` to open the machine body")?;
        self.world_body(true)?;
        self.g_finish(); // World — closes right after the closing `}`
        Ok(())
    }

    fn signature(&mut self) -> Result<(), CompileError> {
        self.expect(&TokenKind::LParen, "`(` to open the signature")?;
        if !matches!(self.peek().kind, TokenKind::RParen) {
            loop {
                // SIG_PARAM opens at the parameter's first significant
                // token, so a comment written between two parameters
                // flushes into the enclosing REUSE rather than into
                // either parameter (docs/core.md (syntax trees)).
                self.g_flush_start(TmcKind::SigParam);
                self.sig_param()?;
                self.g_finish(); // SigParam
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RParen => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `)`")),
                }
            }
        }
        self.expect(&TokenKind::RParen, "`)` to close the signature")?;
        Ok(())
    }

    fn sig_param(&mut self) -> Result<SigParam, CompileError> {
        let t = self.peek().clone();
        let volatile = if self.at_kw("volatile") {
            self.bump();
            if !self.at_kw("tape") {
                return Err(Self::expected(
                    self.peek(),
                    "`tape` after `volatile` (only tape parameters can be volatile)",
                ));
            }
            true
        } else {
            false
        };
        if self.at_kw("tape") {
            self.bump();
            let (name, name_span) = self.name("a tape parameter name")?;
            self.expect(&TokenKind::Colon, "`:` after the tape parameter name")?;
            let (alphabet, alphabet_span) = self.name("an alphabet name")?;
            // `writes { … }`, then `preserves { … }`, both optional — the
            // fixed order is a grammar rule, not an fmt convention, because
            // fmt is token-preserving and cannot reorder an author's
            // clauses.
            let mut writes: Option<ContractClause> = None;
            let mut preserves: Option<ContractClause> = None;
            loop {
                if self.at_kw("writes") {
                    if preserves.is_some() {
                        return Err(Self::err_at(
                            self.peek(),
                            CompileErrorKind::ContractClauseOrder,
                        ));
                    }
                    if writes.is_some() {
                        return Err(Self::err_at(
                            self.peek(),
                            CompileErrorKind::DuplicateContractClause { what: "writes" },
                        ));
                    }
                    writes = Some(self.contract_clause()?);
                } else if self.at_kw("preserves") {
                    if preserves.is_some() {
                        return Err(Self::err_at(
                            self.peek(),
                            CompileErrorKind::DuplicateContractClause { what: "preserves" },
                        ));
                    }
                    preserves = Some(self.contract_clause()?);
                } else {
                    break;
                }
            }
            let last_span = preserves
                .as_ref()
                .or(writes.as_ref())
                .map_or(alphabet_span, |c| c.span);
            Ok(SigParam {
                kind: SigParamKind::Tape {
                    alphabet,
                    alphabet_span,
                    volatile,
                    writes,
                    preserves,
                },
                name,
                name_span,
                span: join(t.span(), last_span),
            })
        } else if self.at_kw("state") {
            self.bump();
            let (name, name_span) = self.name("a state parameter name")?;
            Ok(SigParam {
                kind: SigParamKind::State,
                name,
                name_span,
                span: join(t.span(), name_span),
            })
        } else {
            Err(Self::expected(
                &t,
                "a `tape` or `state` signature parameter",
            ))
        }
    }

    /// A `writes { … }` or `preserves { … }` clause body, the current token
    /// already the keyword: mirrors [`Self::parse_alphabet`]'s body loop
    /// (comma-separated [`Self::alphabet_elem`], empty allowed). Interior
    /// comments are deliberately not accepted here — unlike an alphabet
    /// body, a clause is a short one-line construct, and a comment splitting
    /// one open is not worth the complexity budget this early; a comment
    /// written inside one still parses, but reprints outside the clause on
    /// the next fmt pass (the same relocation an author sees writing a
    /// comment inside a signature or binding argument list).
    fn contract_clause(&mut self) -> Result<ContractClause, CompileError> {
        let kw_span = self.peek().span();
        // Bracketed here rather than at the two call sites above, so the
        // node opens at the clause keyword for both — its extent is then
        // exactly `kw_span` → the closing `}`, i.e. `ContractClause::span`.
        self.g_flush_start(TmcKind::ContractClause);
        self.bump(); // `writes` / `preserves`
        self.expect(&TokenKind::LBrace, "`{` to open the clause body")?;
        let mut elems: Vec<AlphabetElem> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBrace) {
            loop {
                elems.push(self.alphabet_elem()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RBrace => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `}`")),
                }
            }
        }
        let close = self.expect(&TokenKind::RBrace, "`}` to close the clause body")?;
        self.g_finish(); // ContractClause
        Ok(ContractClause {
            elems,
            kw_span,
            span: join(kw_span, close.span()),
        })
    }

    // ---- world bodies -----------------------------------------------------

    fn next_is_world_doc_accepting(&self) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(w)
            if matches!(w.as_str(), "entry" | "state" | "graft" | "bind"))
    }

    /// A world body (machine / routine / graph), after its opening `{`.
    /// `in_machine` allows tape declarations (routines/graphs take tapes from
    /// the signature — a tape decl there is a `TapeNotInMachine` error).
    fn world_body(&mut self, in_machine: bool) -> Result<(), CompileError> {
        loop {
            // Green checkpoint for whichever node this item turns out to
            // be: same placement rule as `top_items`'s `cp` — top of the
            // iteration, before the doc run (if any) — so
            // `g_start_at` below retro-wraps the run and an `entry`
            // prefix, when present, alongside the header. TAPE never
            // accepts a doc run (`next_is_world_doc_accepting` excludes
            // it), so this same checkpoint sits at TAPE's own header
            // token whenever it fires there — no separate checkpoint
            // needed.
            let cp = self.g_checkpoint();
            if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                self.g_flush_start(TmcKind::DocRun);
                let first_span = self.doc_run()?;
                self.g_finish(); // DocRun
                if !self.next_is_world_doc_accepting() {
                    return Err(CompileError {
                        span: first_span,
                        kind: CompileErrorKind::DanglingDocRun,
                    });
                }
            }
            let t = self.peek().clone();
            if matches!(t.kind, TokenKind::RBrace) {
                self.bump();
                return Ok(());
            }
            if matches!(t.kind, TokenKind::Eof) {
                return Err(Self::expected(&t, "`}` to close the body"));
            }
            if self.at_kw("entry") {
                self.bump();
                if self.at_kw("state") {
                    self.g_start_at(cp, TmcKind::State);
                    self.parse_state()?;
                    self.g_finish(); // State — `entry` included
                } else if self.at_kw("graft") {
                    self.g_start_at(cp, TmcKind::Graft);
                    self.parse_graft(true)?;
                    self.g_finish(); // Graft — `entry` included
                } else {
                    return Err(Self::expected(
                        self.peek(),
                        "`state` or `graft` after `entry`",
                    ));
                }
            } else if self.at_kw("state") {
                self.g_start_at(cp, TmcKind::State);
                self.parse_state()?;
                self.g_finish(); // State
            } else if self.at_kw("graft") {
                self.g_start_at(cp, TmcKind::Graft);
                self.parse_graft(false)?;
                self.g_finish(); // Graft
            } else if self.at_kw("bind") {
                self.g_start_at(cp, TmcKind::Bind);
                self.parse_bind()?;
                self.g_finish(); // Bind
            } else if self.at_kw("volatile") {
                self.bump(); // `volatile`
                if !self.at_kw("tape") {
                    return Err(Self::expected(self.peek(), "`tape` after `volatile`"));
                }
                if in_machine {
                    self.g_start_at(cp, TmcKind::Tape); // `volatile` included
                    self.parse_tape()?;
                    self.g_finish(); // Tape
                } else {
                    return Err(Self::err_at(&t, CompileErrorKind::TapeNotInMachine));
                }
            } else if self.at_kw("tape") {
                if in_machine {
                    self.g_start_at(cp, TmcKind::Tape);
                    self.parse_tape()?;
                    self.g_finish(); // Tape
                } else {
                    return Err(Self::err_at(&t, CompileErrorKind::TapeNotInMachine));
                }
            } else {
                return Err(Self::expected(
                    &t,
                    "a tape declaration, `state`, `graft`, or `bind`",
                ));
            }
        }
    }

    fn parse_tape(&mut self) -> Result<(), CompileError> {
        self.bump(); // `tape`
        self.name("a tape name")?;
        self.expect(&TokenKind::Colon, "`:` after the tape name")?;
        self.name("an alphabet name")?;
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(())
    }

    fn parse_state(&mut self) -> Result<(), CompileError> {
        self.bump(); // `state`
        self.name("a state name")?;
        // `state name;` redirect form is not supported.
        if matches!(self.peek().kind, TokenKind::Semi) {
            return Err(Self::err_at(self.peek(), CompileErrorKind::StateRedirect));
        }
        self.expect(&TokenKind::LBrace, "`{` to open the state body")?;
        self.state_rules()
    }

    /// A state body's rule loop, from just past the opening `{` through
    /// the closing `}`.
    fn state_rules(&mut self) -> Result<(), CompileError> {
        loop {
            let t = self.peek().clone();
            if matches!(t.kind, TokenKind::RBrace) {
                self.bump();
                return Ok(());
            }
            if matches!(t.kind, TokenKind::Eof) {
                return Err(Self::expected(&t, "`}` to close the state body"));
            }
            // A bare (bracket-less) pattern is the deliberately-absent
            // single-tape sugar — name it clearly rather than "expected `[`".
            if matches!(
                t.kind,
                TokenKind::Glyph(_) | TokenKind::Number(_, _) | TokenKind::Star
            ) {
                return Err(Self::err_at(&t, CompileErrorKind::NakedPattern));
            }
            if !matches!(t.kind, TokenKind::LBracket) {
                return Err(Self::expected(&t, "a rule (`[…] -> …;`) or `}`"));
            }
            self.g_flush_start(TmcKind::Rule);
            self.rule()?;
            self.g_finish(); // Rule
        }
    }

    // ---- rules ------------------------------------------------------------

    /// Parses one rule. The pattern and the write vector are kept as
    /// values because [`Self::check_char_arithmetic`] reads them once
    /// the rule's bindings are known; everything else this walk parses
    /// is dropped, and extraction rebuilds it from the green tree.
    fn rule(&mut self) -> Result<(), CompileError> {
        let pattern = self.pattern()?;
        self.expect(&TokenKind::Arrow, "`->` after the pattern")?;
        let debugger = if self.at_kw("debugger") {
            self.bump();
            true
        } else {
            false
        };
        let write = if self.at_kw("write") {
            self.bump();
            Some(self.write_vec()?)
        } else {
            None
        };
        let mov = if self.at_kw("move") {
            self.bump();
            Some(self.move_vec()?)
        } else {
            None
        };
        // The transition may be omitted (`stay in the current state`) only
        // when the rule already carries an action — write, move, or a leading
        // `debugger`. With no action, `-> ;` stays the "expected a transition"
        // error (docs/tmt/language.md (rules)).
        //
        // Only a WRITTEN transition is bracketed: `Stay` is the green
        // tree's ABSENCE of a TRANSITION node under the rule, never a
        // zero-width one. That asymmetry is the point — it is the one
        // fact about a rule that no token run can carry.
        let has_action = debugger || write.is_some() || mov.is_some();
        if !(has_action && matches!(self.peek().kind, TokenKind::Semi)) {
            self.g_flush_start(TmcKind::Transition);
            self.transition()?;
            self.g_finish(); // Transition
        }
        self.expect(&TokenKind::Semi, "`;` to end the rule")?;
        // Char arithmetic is deliberately absent: a `{c±k}` on a glyph-bound
        // pattern name is rejected here, where the rule's bindings are known.
        self.check_char_arithmetic(&pattern, &write)
    }

    fn check_char_arithmetic(
        &self,
        pattern: &Pattern,
        write: &Option<WriteVec>,
    ) -> Result<(), CompileError> {
        let Some(w) = write else {
            return Ok(());
        };
        let mut glyph_bound: Vec<&str> = Vec::new();
        for cell in &pattern.cells {
            if let Some(b) = &cell.binding {
                let is_glyph = match &cell.kind {
                    PatternCellKind::Single(s) => s.is_glyph(),
                    PatternCellKind::Range { lo, .. } => lo.is_glyph(),
                    PatternCellKind::Wildcard => false,
                };
                if is_glyph {
                    glyph_bound.push(b.name.as_str());
                }
            }
        }
        for cell in &w.cells {
            if let WriteCellKind::Subst { expr } = &cell.kind
                // A bare name keeps passthrough semantics — legal for a glyph
                // binding. Any other shape is a fold, which is numeric-only.
                && !matches!(expr.kind, FoldExprKind::Var(_))
                && let Some(span) = Self::glyph_var_span(expr, &glyph_bound)
            {
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::CharArithmetic,
                });
            }
        }
        Ok(())
    }

    /// The span of the first (leftmost) fold-expression reference to a
    /// glyph-bound name, or `None` if the expression references none.
    fn glyph_var_span(expr: &FoldExprNode, glyph_bound: &[&str]) -> Option<Span> {
        match &expr.kind {
            FoldExprKind::Var(name) => glyph_bound.contains(&name.as_str()).then_some(expr.span),
            FoldExprKind::Int(_) => None,
            FoldExprKind::Bin { lhs, rhs, .. } => Self::glyph_var_span(lhs, glyph_bound)
                .or_else(|| Self::glyph_var_span(rhs, glyph_bound)),
        }
    }

    /// Parses a bracketed pattern. The [`Pattern`] is what
    /// [`Self::check_char_arithmetic`] reads before the rule ends, and
    /// what [`reparse_pattern`] hands back to extraction.
    fn pattern(&mut self) -> Result<Pattern, CompileError> {
        let lb = self.expect(&TokenKind::LBracket, "`[` to open the pattern")?;
        let mut cells: Vec<PatternCell> = Vec::new();
        loop {
            cells.push(self.pattern_cell()?);
            match self.peek().kind {
                TokenKind::Comma => self.bump(),
                TokenKind::RBracket => break,
                _ => return Err(Self::expected(self.peek(), "`,` or `]`")),
            }
        }
        let rb = self.expect(&TokenKind::RBracket, "`]` to close the pattern")?;
        Ok(Pattern {
            cells,
            span: join(lb.span(), rb.span()),
        })
    }

    fn pattern_cell(&mut self) -> Result<PatternCell, CompileError> {
        let t = self.peek().clone();
        let (kind, kind_span) = match &t.kind {
            TokenKind::Star => {
                self.bump();
                (PatternCellKind::Wildcard, t.span())
            }
            TokenKind::Glyph(_) | TokenKind::Number(_, _) => {
                let (lo, hi) = self.sym_or_range()?;
                match hi {
                    None => {
                        let sp = lo.span();
                        (PatternCellKind::Single(lo), sp)
                    }
                    Some(hi) => {
                        let sp = join(lo.span(), hi.span());
                        (PatternCellKind::Range { lo, hi }, sp)
                    }
                }
            }
            _ => {
                return Err(Self::expected(
                    &t,
                    "a pattern element (glyph, number, range, or `*`)",
                ));
            }
        };
        let (binding, end) = if self.at_kw("as") {
            self.bump();
            let (n, sp) = self.name("a binding name")?;
            (Some(Binding { name: n, span: sp }), sp)
        } else {
            (None, kind_span)
        };
        // `* as v` is forbidden.
        if matches!(kind, PatternCellKind::Wildcard) && binding.is_some() {
            return Err(Self::err_at(&t, CompileErrorKind::WildcardBinding));
        }
        Ok(PatternCell {
            kind,
            binding,
            span: join(kind_span, end),
        })
    }

    /// A single symbol, or the low end plus a same-kind high end after `..`.
    fn sym_or_range(&mut self) -> Result<(SymLit, Option<SymLit>), CompileError> {
        let lo = self.sym_lit()?;
        if matches!(self.peek().kind, TokenKind::DotDot) {
            self.bump();
            let hi = self.sym_lit()?;
            if lo.is_glyph() != hi.is_glyph() {
                return Err(CompileError {
                    span: join(lo.span(), hi.span()),
                    kind: CompileErrorKind::RangeKindMismatch,
                });
            }
            Ok((lo, Some(hi)))
        } else {
            Ok((lo, None))
        }
    }

    fn sym_lit(&mut self) -> Result<SymLit, CompileError> {
        let t = self.peek().clone();
        match &t.kind {
            TokenKind::Glyph(v) => {
                self.bump();
                Ok(SymLit::Glyph {
                    value: v.clone(),
                    span: t.span(),
                })
            }
            TokenKind::Number(n, written) => {
                self.bump();
                Ok(SymLit::Number {
                    value: *n,
                    written: written.clone(),
                    span: t.span(),
                })
            }
            _ => Err(Self::expected(&t, "a glyph or number")),
        }
    }

    /// Parses a bracketed write vector. The [`WriteVec`] is what
    /// [`Self::check_char_arithmetic`] reads before the rule ends, and
    /// what [`reparse_write_vec`] hands back to extraction.
    fn write_vec(&mut self) -> Result<WriteVec, CompileError> {
        // The node opens at `[`, not at the `write` keyword the caller
        // already consumed, so its extent is exactly `WriteVec::span`.
        // The keyword stays a token of the enclosing RULE — nothing has
        // to read it, since the node's own KIND says which vector this
        // is (docs/core.md (syntax trees)).
        self.g_flush_start(TmcKind::WriteVec);
        let lb = self.expect(&TokenKind::LBracket, "`[` to open the write vector")?;
        let mut cells: Vec<WriteCell> = Vec::new();
        loop {
            cells.push(self.write_cell()?);
            match self.peek().kind {
                TokenKind::Comma => self.bump(),
                TokenKind::RBracket => break,
                _ => return Err(Self::expected(self.peek(), "`,` or `]`")),
            }
        }
        let rb = self.expect(&TokenKind::RBracket, "`]` to close the write vector")?;
        self.g_finish(); // WriteVec
        Ok(WriteVec {
            cells,
            span: join(lb.span(), rb.span()),
        })
    }

    fn write_cell(&mut self) -> Result<WriteCell, CompileError> {
        let t = self.peek().clone();
        match &t.kind {
            TokenKind::Dash => {
                self.bump();
                Ok(WriteCell {
                    kind: WriteCellKind::Keep,
                    span: t.span(),
                })
            }
            TokenKind::Glyph(_) | TokenKind::Number(_, _) => {
                let s = self.sym_lit()?;
                let span = s.span();
                Ok(WriteCell {
                    kind: WriteCellKind::Lit(s),
                    span,
                })
            }
            TokenKind::LBrace => {
                self.bump();
                let expr = self.fold_expr()?;
                let rb = self.expect(&TokenKind::RBrace, "`}` to close the substitution")?;
                Ok(WriteCell {
                    kind: WriteCellKind::Subst { expr },
                    span: join(t.span(), rb.span()),
                })
            }
            _ => Err(Self::expected(
                &t,
                "a write element (glyph, number, `{binding}`, or `-`)",
            )),
        }
    }

    /// Parse a write-cell fold expression, the assembler's arithmetic grammar
    /// (docs/tmt/language.md (substitution)):
    ///
    /// ```text
    /// expr := mul (('+' | '-') mul)*
    /// mul  := atom (('*' | '%') atom)*
    /// atom := var | integer | '(' expr ')'
    /// ```
    ///
    /// `+`/`-` are left-associative; `*`/`%` bind tighter.
    fn fold_expr(&mut self) -> Result<FoldExprNode, CompileError> {
        let mut lhs = self.fold_mul()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Plus => FoldOp::Add,
                TokenKind::Dash => FoldOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.fold_mul()?;
            let span = join(lhs.span, rhs.span);
            lhs = FoldExprNode {
                kind: FoldExprKind::Bin {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn fold_mul(&mut self) -> Result<FoldExprNode, CompileError> {
        let mut lhs = self.fold_atom()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Star => FoldOp::Mul,
                TokenKind::Percent => FoldOp::Rem,
                _ => break,
            };
            self.bump();
            let rhs = self.fold_atom()?;
            let span = join(lhs.span, rhs.span);
            lhs = FoldExprNode {
                kind: FoldExprKind::Bin {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn fold_atom(&mut self) -> Result<FoldExprNode, CompileError> {
        let t = self.peek().clone();
        match &t.kind {
            TokenKind::LParen => {
                self.bump();
                let inner = self.fold_expr()?;
                let rp = self.expect(&TokenKind::RParen, "`)` to close the sub-expression")?;
                Ok(FoldExprNode {
                    kind: inner.kind,
                    span: join(t.span(), rp.span()),
                })
            }
            TokenKind::Number(n, _) => {
                self.bump();
                Ok(FoldExprNode {
                    kind: FoldExprKind::Int(*n as i64),
                    span: t.span(),
                })
            }
            TokenKind::Ident(_) => {
                let (name, span) = self.name("a substitution binding name")?;
                Ok(FoldExprNode {
                    kind: FoldExprKind::Var(name),
                    span,
                })
            }
            _ => Err(Self::expected(
                &t,
                "a fold expression element (binding name, number, or `(`)",
            )),
        }
    }

    /// Parses a bracketed move vector. The [`MoveVec`] is what
    /// [`reparse_move_vec`] hands back to extraction; the main walk
    /// drops it.
    fn move_vec(&mut self) -> Result<MoveVec, CompileError> {
        // Opens at `[` — see `write_vec`'s identical note.
        self.g_flush_start(TmcKind::MoveVec);
        let lb = self.expect(&TokenKind::LBracket, "`[` to open the move vector")?;
        let mut cells: Vec<MoveCell> = Vec::new();
        loop {
            cells.push(self.move_cell()?);
            match self.peek().kind {
                TokenKind::Comma => self.bump(),
                TokenKind::RBracket => break,
                _ => return Err(Self::expected(self.peek(), "`,` or `]`")),
            }
        }
        let rb = self.expect(&TokenKind::RBracket, "`]` to close the move vector")?;
        self.g_finish(); // MoveVec
        Ok(MoveVec {
            cells,
            span: join(lb.span(), rb.span()),
        })
    }

    fn move_cell(&mut self) -> Result<MoveCell, CompileError> {
        let t = self.peek().clone();
        let dir = match t.kind {
            TokenKind::Lt => MoveDir::Left,
            TokenKind::Gt => MoveDir::Right,
            TokenKind::Dot => MoveDir::Stay,
            _ => {
                return Err(Self::expected(&t, "a move element (`<`, `>`, or `.`)"));
            }
        };
        self.bump();
        Ok(MoveCell {
            dir,
            span: t.span(),
        })
    }

    /// Parses one written transition — `goto`, `call … then …`,
    /// `return`, `stop`, `halt`, or a bare state name (goto sugar).
    ///
    /// The [`Transition`] outlives this walk only for
    /// [`reparse_transition`]'s sake: extraction retokenizes a
    /// TRANSITION node and re-runs this exact production, so a `call`'s
    /// `args` have to be built faithfully here rather than left empty.
    /// [`Self::rule`], the only other caller, drops the value.
    ///
    /// `Transition::Stay` is not among the variants this can answer. An
    /// omitted transition is the ABSENCE of a TRANSITION node, which
    /// [`Self::rule`] decides before it would call in here at all
    /// (docs/tmt/language.md (transitions)).
    fn transition(&mut self) -> Result<Transition, CompileError> {
        let t = self.peek().clone();
        match &t.kind {
            TokenKind::Ident(w) if w == "goto" => {
                self.bump();
                let (name, name_span) = self.name("a goto target")?;
                Ok(Transition::Goto {
                    name,
                    explicit: true,
                    span: join(t.span(), name_span),
                })
            }
            TokenKind::Ident(w) if w == "call" => {
                self.bump();
                let target = self.qual_name("a call target")?;
                let args = self.binding_args()?;
                self.expect_kw("then", "`then` after the call target")?;
                let then = self.continuation()?;
                let end = match &then {
                    Continuation::State { span, .. }
                    | Continuation::Return { span }
                    | Continuation::Stop { span }
                    | Continuation::Halt { span } => *span,
                };
                Ok(Transition::Call {
                    target,
                    args,
                    then,
                    span: join(t.span(), end),
                })
            }
            TokenKind::Ident(w) if w == "return" => {
                self.bump();
                Ok(Transition::Return { span: t.span() })
            }
            TokenKind::Ident(w) if w == "stop" => {
                self.bump();
                Ok(Transition::Stop { span: t.span() })
            }
            TokenKind::Ident(w) if w == "halt" => {
                self.bump();
                Ok(Transition::Halt { span: t.span() })
            }
            TokenKind::Ident(w) if !RESERVED.contains(&w.as_str()) => {
                // Bare-name transition = goto sugar.
                self.bump();
                Ok(Transition::Goto {
                    name: w.clone(),
                    explicit: false,
                    span: t.span(),
                })
            }
            _ => Err(Self::expected(
                &t,
                "a transition: `goto`, a state name, `call … then …`, `return`, `stop`, or `halt`",
            )),
        }
    }

    fn continuation(&mut self) -> Result<Continuation, CompileError> {
        let t = self.peek().clone();
        match &t.kind {
            TokenKind::Ident(w) if w == "return" => {
                self.bump();
                Ok(Continuation::Return { span: t.span() })
            }
            TokenKind::Ident(w) if w == "stop" => {
                self.bump();
                Ok(Continuation::Stop { span: t.span() })
            }
            TokenKind::Ident(w) if w == "halt" => {
                self.bump();
                Ok(Continuation::Halt { span: t.span() })
            }
            TokenKind::Ident(w) if !RESERVED.contains(&w.as_str()) => {
                self.bump();
                Ok(Continuation::State {
                    name: w.clone(),
                    span: t.span(),
                })
            }
            _ => Err(Self::expected(
                &t,
                "a continuation: a state name, `return`, `stop`, or `halt`",
            )),
        }
    }

    fn expect_kw(&mut self, w: &'static str, what: &'static str) -> Result<(), CompileError> {
        if self.at_kw(w) {
            self.bump();
            Ok(())
        } else {
            Err(Self::expected(self.peek(), what))
        }
    }

    fn qual_name(&mut self, what: &'static str) -> Result<QualName, CompileError> {
        let (first, first_span) = self.name(what)?;
        let mut segments = vec![first];
        let mut end = first_span;
        while matches!(self.peek().kind, TokenKind::ColonColon) {
            self.bump();
            let (seg, seg_span) = self.name("a path segment")?;
            segments.push(seg);
            end = seg_span;
        }
        Ok(QualName {
            segments,
            span: join(first_span, end),
        })
    }

    /// A parenthesised binding list. The arguments themselves survive
    /// the walk: a `call`'s list becomes [`Transition::Call`]'s `args`,
    /// which [`reparse_transition`] must reproduce faithfully. A
    /// `graft`/`bind`'s list is dropped by its caller.
    fn binding_args(&mut self) -> Result<Vec<BindingArg>, CompileError> {
        self.expect(&TokenKind::LParen, "`(` to open the binding")?;
        let mut args: Vec<BindingArg> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RParen) {
            loop {
                // BINDING_ARG opens at the argument's own name IDENT, so
                // its extent is exactly `BindingArg::span` and a comment
                // between two arguments flushes into the enclosing
                // GRAFT/BIND/TRANSITION instead.
                self.g_flush_start(TmcKind::BindingArg);
                args.push(self.binding_arg()?);
                self.g_finish(); // BindingArg
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RParen => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `)`")),
                }
            }
        }
        self.expect(&TokenKind::RParen, "`)` to close the binding")?;
        Ok(args)
    }

    /// Parses one `name = value` binding argument — the unit
    /// [`reparse_binding_arg`] re-runs, `with map { … }` included.
    fn binding_arg(&mut self) -> Result<BindingArg, CompileError> {
        let (name, name_span) = self.name("a binding argument name")?;
        self.expect(&TokenKind::Eq, "`=` in the binding argument")?;
        let t = self.peek().clone();
        let (value, end) = match &t.kind {
            TokenKind::Ident(w) if w == "return" => {
                self.bump();
                (
                    BindingValue::Terminator {
                        kind: TermKind::Return,
                        span: t.span(),
                    },
                    t.span(),
                )
            }
            TokenKind::Ident(w) if w == "stop" => {
                self.bump();
                (
                    BindingValue::Terminator {
                        kind: TermKind::Stop,
                        span: t.span(),
                    },
                    t.span(),
                )
            }
            TokenKind::Ident(w) if w == "halt" => {
                self.bump();
                (
                    BindingValue::Terminator {
                        kind: TermKind::Halt,
                        span: t.span(),
                    },
                    t.span(),
                )
            }
            TokenKind::Ident(w) if !RESERVED.contains(&w.as_str()) => {
                let target = w.clone();
                let target_span = t.span();
                self.bump();
                let (map, end) = if self.at_kw("with") {
                    self.bump();
                    let m = self.sym_map()?;
                    let sp = m.span;
                    (Some(m), sp)
                } else {
                    (None, target_span)
                };
                (
                    BindingValue::Named {
                        target,
                        target_span,
                        map,
                    },
                    end,
                )
            }
            _ => {
                return Err(Self::expected(
                    &t,
                    "a binding target: a tape/state name, `return`, `stop`, or `halt`",
                ));
            }
        };
        Ok(BindingArg {
            name,
            name_span,
            value,
            span: join(name_span, end),
        })
    }

    /// `map { pairs }` after a consumed `with` — the production
    /// [`reparse_sym_map`] re-runs over a SYM_MAP node's own tokens.
    fn sym_map(&mut self) -> Result<SymMap, CompileError> {
        // Opens at `map`, not at the `with` the caller already consumed:
        // `SymMap::span` runs `map` → `}`, and keeping the node's extent
        // equal to that span is what lets extraction copy it rather than
        // recompute it.
        self.g_flush_start(TmcKind::SymMap);
        let map_tok = self.expect_kw_tok("map", "`map` after `with`")?;
        self.expect(&TokenKind::LBrace, "`{` to open the map")?;
        let mut pairs: Vec<MapPair> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBrace) {
            loop {
                pairs.push(self.map_pair()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RBrace => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `}`")),
                }
            }
        }
        let rb = self.expect(&TokenKind::RBrace, "`}` to close the map")?;
        self.g_finish(); // SymMap
        Ok(SymMap {
            pairs,
            span: join(map_tok.span(), rb.span()),
        })
    }

    fn expect_kw_tok(
        &mut self,
        w: &'static str,
        what: &'static str,
    ) -> Result<Token, CompileError> {
        if self.at_kw(w) {
            let t = self.peek().clone();
            self.bump();
            Ok(t)
        } else {
            Err(Self::expected(self.peek(), what))
        }
    }

    fn map_pair(&mut self) -> Result<MapPair, CompileError> {
        let src = self.sym_lit()?;
        let arrow = match self.peek().kind {
            TokenKind::Arrow => MapArrow::Bidirectional,
            TokenKind::FatArrow => MapArrow::ReadOnly,
            _ => return Err(Self::expected(self.peek(), "`->` or `=>` in the map")),
        };
        self.bump();
        let dst = self.sym_lit()?;
        Ok(MapPair {
            span: join(src.span(), dst.span()),
            src,
            dst,
            arrow,
        })
    }

    /// `entry` is not decoration here: it is what decides whether the
    /// missing `as` clause below is legal — a non-entry graft must be
    /// named (docs/tmt/language.md (grafts)).
    fn parse_graft(&mut self, entry: bool) -> Result<(), CompileError> {
        let graft_tok = self.peek().clone();
        self.bump(); // `graft`
        self.qual_name("a graft target")?;
        self.binding_args()?;
        let named = if self.at_kw("as") {
            self.bump();
            self.name("a graft instance name")?;
            true
        } else {
            false
        };
        // A non-entry graft must be named.
        if !entry && !named {
            return Err(Self::err_at(&graft_tok, CompileErrorKind::GraftNeedsName));
        }
        self.expect(&TokenKind::Semi, "`;` to end the graft")?;
        Ok(())
    }

    fn parse_bind(&mut self) -> Result<(), CompileError> {
        self.bump(); // `bind`
        self.qual_name("a bind target")?;
        self.binding_args()?;
        self.expect_kw("as", "`as` (a bind needs an instance name)")?;
        self.name("a bind instance name")?;
        self.expect(&TokenKind::Semi, "`;` to end the bind")?;
        Ok(())
    }
}

/// Builds a bare `Parser` over already-retokenized tokens
/// (`crate::syntax::extract::sig_tokens`'s own output) — `sink: None`,
/// since a reparse shim never re-emits a green tree, only a value.
/// Shared by every `reparse_*` shim below so the field list lives once.
/// No `#[allow(dead_code)]` of its own, and none needed: all but one of
/// the shims below are live callees of
/// `crate::syntax::extract::extract_program`, so this is reachable from
/// real code.
fn bare_parser(tokens: &[Token]) -> Parser<'_> {
    Parser {
        tokens,
        pos: 0,
        machine_seen: false,
        sink: None,
        recovered: None,
    }
}

/// Retokenization reuse shim (`crate::syntax::extract::sig_tokens`'s
/// counterpart) for a RULE's own TRANSITION node: re-parse it through
/// the SAME production the original parse used
/// (`Parser::transition`), so a green-tree reparse and the original
/// parse can never disagree on what a transition means. `expect`s on
/// error: extraction only ever runs on a tree that already parsed
/// once, so a failure here is a bug in the retokenization, not a
/// malformed program.
///
/// **Structurally uncallable for one of `Transition`'s six variants,
/// `Stay`.** `Stay` is the ABSENCE of a TRANSITION node — an omitted
/// transition, legal only when the rule already carries an action
/// (docs/tmt/language.md (transitions)) — so `RuleView::transition()`
/// answers `None` for it and there is never a node to retokenize in
/// the first place. A caller (extraction) must synthesize `Stay`
/// itself from that `None`; this shim only ever runs for the other
/// five (`Goto` in both its `explicit` spellings, `Call`, `Return`,
/// `Stop`, `Halt`).
pub(crate) fn reparse_transition(tokens: &[Token]) -> Transition {
    bare_parser(tokens)
        .transition()
        .expect("reparse_transition: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for a GRAFT/BIND's own BINDING_ARG node:
/// re-parses it through `Parser::binding_arg`, the exact unit
/// `crate::syntax::kinds`'s own module doc names as what a caller
/// "retokenizes and hands back" — `BINDING_ARG` carries no dedicated
/// list wrapper (the `(`/`)` are its parent GRAFT/BIND's own tokens), so
/// the reparse unit is one argument at a time, mirroring
/// `Parser::sig_param`'s identical shape.
pub(crate) fn reparse_binding_arg(tokens: &[Token]) -> BindingArg {
    bare_parser(tokens)
        .binding_arg()
        .expect("reparse_binding_arg: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for an ALPHABET node: re-parses its
/// element list through [`Parser::alphabet_elems`], the same production
/// `parse_alphabet` itself runs.
///
/// Takes the WHOLE declaration's token run and walks to just past its
/// first `{`, rather than asking the caller to slice the body out. That
/// run is `DocLine|AttentionLine* export? alphabet NAME { … }` — the
/// node retro-wraps its bound doc run, so those lines are part of it —
/// and NEITHER prefix can carry an `LBrace`: a `?`/`!` line lexes to
/// one whole-line token, never a brace, and the header is exactly
/// `export? alphabet NAME`. So the first `LBrace` in the run is always
/// the body's opener. Doing the slice here rather than at the call site
/// keeps "where an alphabet body begins" next to the production that
/// decides it.
pub(crate) fn reparse_alphabet_elems(tokens: &[Token]) -> Vec<AlphabetElem> {
    let mut p = bare_parser(tokens);
    while !matches!(p.peek().kind, TokenKind::LBrace | TokenKind::Eof) {
        p.bump();
    }
    p.bump(); // `{`
    p.alphabet_elems()
        .expect("reparse_alphabet_elems: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for a RULE's own pattern: re-parses it
/// through [`Parser::pattern`]. Unlike every other shim here the input
/// is not a node's token run — a pattern is deliberately unbracketed
/// (`crate::syntax::kinds`'s own module doc: mandatory and first, so
/// nothing optional has to be decided to find it) — so the caller
/// supplies the `[` … `]` token slice instead
/// (`crate::syntax::views::RuleView::pattern_tokens`).
pub(crate) fn reparse_pattern(tokens: &[Token]) -> Pattern {
    bare_parser(tokens)
        .pattern()
        .expect("reparse_pattern: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for a RULE's own WRITE_VEC node:
/// re-parses it through [`Parser::write_vec`]. The node's tokens
/// already start at `[`, not at the `write` keyword its caller
/// consumed — `crate::syntax::kinds`'s own module doc records why (the
/// node's extent is chosen to match `WriteVec::span` exactly) — which
/// is where that production expects to begin.
pub(crate) fn reparse_write_vec(tokens: &[Token]) -> WriteVec {
    bare_parser(tokens)
        .write_vec()
        .expect("reparse_write_vec: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for a RULE's own MOVE_VEC node:
/// re-parses it through [`Parser::move_vec`] — the same `[`-opening
/// extent rule as [`reparse_write_vec`].
pub(crate) fn reparse_move_vec(tokens: &[Token]) -> MoveVec {
    bare_parser(tokens)
        .move_vec()
        .expect("reparse_move_vec: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for a GRAFT/BIND target: re-parses it
/// through [`Parser::qual_name`], so the `::` walk and the
/// first-segment-start → last-segment-end span join stay in the parser.
///
/// A qualified name has no node of its own (`IDENT (:: IDENT)*` sits
/// directly under GRAFT/BIND), so the caller supplies a token slice.
/// That slice may run PAST the name — `qual_name` continues only while
/// the next token is `::`, so it stops of its own accord at the `(`
/// that always follows a target — which means a caller need only skip
/// the header keywords, never find the name's end.
pub(crate) fn reparse_qual_name(tokens: &[Token]) -> QualName {
    bare_parser(tokens)
        .qual_name("a reuse target")
        .expect("reparse_qual_name: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for a BINDING_ARG's own SYM_MAP node:
/// re-parses it through `Parser::sym_map`. A SYM_MAP node's own tokens
/// already start at `map`, not at the `with` its caller consumed —
/// `crate::syntax::kinds`'s own module doc records why (the node's
/// extent is chosen to match `SymMap::span` exactly) — which is
/// exactly where `Parser::sym_map` expects to begin.
///
/// The one shim here that keeps its `#[allow(dead_code)]`: extraction
/// never calls it, because a map is reached through the argument that
/// owns it and [`reparse_binding_arg`] parses `with map { … }` as part
/// of the argument's own value. It stays because a SYM_MAP node is
/// separately addressable (a language service asking about one map, not
/// the whole argument, is the case it exists for), and its fidelity
/// test in `crate::syntax::extract`'s own test module holds
/// `Parser::sym_map` reachable from a green node independently of that.
#[allow(dead_code)]
pub(crate) fn reparse_sym_map(tokens: &[Token]) -> SymMap {
    bare_parser(tokens)
        .sym_map()
        .expect("reparse_sym_map: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for a REUSE's own SIG_PARAM node:
/// re-parses it through `Parser::sig_param`. Covers both shapes the
/// production itself branches on — `Tape` (with its optional
/// `writes`/`preserves` clauses) and the plain `State` parameter.
pub(crate) fn reparse_sig_param(tokens: &[Token]) -> SigParam {
    bare_parser(tokens)
        .sig_param()
        .expect("reparse_sig_param: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for a bound DOC_RUN node: re-parses its
/// `DocLine`/`AttentionLine` tokens through `Parser::doc_run_items`
/// itself, rather than a hand-rolled reimplementation of its item loop.
/// Sound even though that production also re-enforces the
/// `DocLineOrder` ordering check and the duplicate-/unknown-attribute
/// checks: those checks already passed once, over these exact tokens,
/// during the parse that produced the tree this run was retokenized
/// from, so re-running them is redundant work, never a new failure
/// mode. It is also the only place the items exist at all — the walk
/// that built the tree discards its own.
///
/// The items are `?`/`!` lines only. A comment written inside a run is
/// green-tree trivia, which `crate::syntax::extract::sig_tokens`
/// filters out before this ever runs;
/// `crate::syntax::extract::extract_doc_items` builds that comment's
/// own `DocRunKind::Comment` item straight from the tree and
/// interleaves it with what this shim returns.
///
/// `prev_end_line` seeds the run's FIRST item's `blank_before` gap
/// check — the end line of whatever preceded this run in the real
/// file (`0` when the run opens the file). The caller MUST supply the
/// real value: `tokens` holds only the DOC_RUN node's own descendant
/// tokens (`crate::syntax::extract::sig_tokens`'s whole contract), and
/// whether a blank line preceded the run's own first line is a
/// property of whatever came BEFORE the node — information the
/// node's own tokens can never carry. Every item AFTER the first computes
/// its own gap from the item before it, entirely inside `tokens`, so
/// only the first item's `blank_before` is at stake here; passing the
/// wrong value claims a blank line the source does not have (or misses
/// one it does), on exactly that one field of exactly that one item.
/// Mirrors the
/// sibling crate's own
/// `reparse_item(tokens, in_group)`, whose `in_group` flag is the same
/// shape of caller-supplied context an isolated retokenized node
/// cannot recover on its own.
pub(crate) fn reparse_doc_items(tokens: &[Token], prev_end_line: u32) -> Vec<DocRunItem> {
    bare_parser(tokens)
        .doc_run_items(prev_end_line)
        .expect("reparse_doc_items: extraction only ever runs on an already-parsed tree")
        .0
}

#[cfg(test)]
mod tests;
