//! `.tmc` recursive-descent parser (spec's language chapter): tokens → AST,
//! via a lossless CST. The front-end mirror of the `.pmc` parser in the
//! sibling PM-1 crate, using the same `parse = lower_cst ∘ parse_cst` seam:
//! `parse_cst` builds the [`crate::cst::Cst`] (which `fmt` walks directly —
//! the language service instead reads the green tree's typed views), and
//! `lower_cst` copies it — infallibly — into the flat [`Program`] the rest
//! of the front end consumes. Every fatal is raised by `parse_cst`;
//! `lower_cst` never fails.
//!
//! The 27 reserved keywords live in one place, [`crate::lexer::RESERVED`]; the
//! parser is the sole enforcer — it rejects a keyword wherever a name is
//! expected. `deprecated` is contextual (an attribute word) and is not in that
//! set.

use std::rc::Rc;

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::syntax::{Checkpoint, GreenNode};

use crate::compiler::{CompileError, CompileErrorKind};
use crate::cst::{
    AlphabetCst, AttrCst, BindCst, Cst, DocRunItem, DocRunKind, GraftCst, MachineCst, NamespaceCst,
    ReuseCarrier, ReuseCst, RuleCst, RuleItem, RuleKind, StateCst, TapeCst, TopItem, TopKind,
    UseCst, UsePath, WorldItem, WorldKind,
};
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
// parse / parse_cst / lower_cst
// ---------------------------------------------------------------------------

/// tokens → AST, via the lossless CST.
pub fn parse(tokens: &[Token]) -> Result<Program, CompileError> {
    parse_cst(tokens).map(|cst| lower_cst(&cst))
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

/// Split a token stream into its significant tokens and its comment
/// trivia — the shape both parse paths hand to `Parser`. `sig_index`
/// records how many significant tokens precede each comment, which is
/// the `pos` the significant-token walk sits at while that comment is
/// pending. A comment-free stream yields an empty `comments` and clones
/// straight through.
fn split_comments(tokens: &[Token]) -> (Vec<Token>, Vec<CommentAt>) {
    let mut sig: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut comments: Vec<CommentAt> = Vec::new();
    for t in tokens {
        if let TokenKind::Comment(c) = &t.kind {
            comments.push(CommentAt {
                comment: c.clone(),
                line: t.line,
                sig_index: sig.len(),
            });
        } else {
            sig.push(t.clone());
        }
    }
    (sig, comments)
}

/// tokens → lossless CST. Accepts a comment-free stream (the compiler's path)
/// or a `WithComments` stream (`fmt`'s path). Comment tokens are split off up
/// front so the grammar walk over the significant tokens is unaffected; the
/// dropped-in-lowering trivia (`blank_before`, comment nodes, `trailing`,
/// `open_trailing`/`close_trailing`, doc runs) is attached by source position.
pub fn parse_cst(tokens: &[Token]) -> Result<Cst, CompileError> {
    let (sig, comments) = split_comments(tokens);
    let (items, _sink) = Parser {
        tokens: &sig,
        pos: 0,
        comments,
        cpos: 0,
        prev_end_line: 0,
        machine_seen: false,
        sink: None,
    }
    .file()?;
    Ok(Cst { items })
}

/// source → green syntax tree (docs/core.md (syntax trees)). Lexes
/// `WithComments` and hands both halves to [`parse_green_from_tokens`].
pub fn parse_green(source: &str) -> Result<Rc<GreenNode>, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    parse_green_from_tokens(source, &tokens)
}

/// Already-lexed tokens → green syntax tree, for callers that keep the
/// token stream alongside the tree. `tokens` MUST be a
/// `LexMode::WithComments` lex of `source`: [`crate::syntax::layout`]
/// reconstructs verbatim token text and trivia from the two together, so
/// a comment-free stream would lose every comment's own text and break
/// the `text() == source` law. An empty `tokens` slice panics — every
/// real lex result is EOF-terminated.
///
/// Runs the SAME grammar walk as [`parse_cst`] with a green sink
/// attached: identical acceptance, identical errors — the sink only
/// mirrors token consumption and node boundaries alongside the
/// unchanged parser logic (docs/core.md (syntax trees)).
pub fn parse_green_from_tokens(
    source: &str,
    tokens: &[Token],
) -> Result<Rc<GreenNode>, CompileError> {
    let entries = syntax::layout(source, tokens);
    let (sig, comments) = split_comments(tokens);
    let eof_pos = sig.len() - 1;
    let mut sink = GreenSink::new(entries);
    sink.start(TmcKind::Root);
    let (_items, sink) = Parser {
        tokens: &sig,
        pos: 0,
        comments,
        cpos: 0,
        prev_end_line: 0,
        machine_seen: false,
        sink: Some(sink),
    }
    .file()?;
    Ok(sink
        .expect("parse_green_from_tokens always seeds a sink before calling file()")
        .finish_tree(eof_pos))
}

/// Copy a CST into the flat [`Program`] — infallibly. Stamps each declaration's
/// enclosing `ns` path, splits the machine body into tapes + behavior, reduces
/// each doc run to a [`Doc`], and drops all trivia.
pub fn lower_cst(cst: &Cst) -> Program {
    let mut p = Program {
        imports: Vec::new(),
        alphabets: Vec::new(),
        routines: Vec::new(),
        graphs: Vec::new(),
        machine: None,
    };
    lower_items(&cst.items, &[], &mut p);
    p
}

fn lower_items(items: &[TopItem], ns: &[String], p: &mut Program) {
    for item in items {
        match &item.kind {
            TopKind::Comment(_) => {}
            TopKind::Import(u) => {
                for path in &u.paths {
                    p.imports.push(Import {
                        path: path.path.clone(),
                        alias: path.alias.clone(),
                        line: path.line,
                        ns: ns.to_vec(),
                        span: path.span,
                    });
                }
            }
            TopKind::Alphabet(a) => p.alphabets.push(lower_alphabet(a, ns)),
            TopKind::Namespace(n) => {
                let mut child = ns.to_vec();
                child.push(n.name.clone());
                lower_items(&n.items, &child, p);
            }
            TopKind::Reuse(r) => match r.carrier {
                ReuseCarrier::Routine => p.routines.push(lower_routine(r, ns)),
                ReuseCarrier::Graph => p.graphs.push(lower_graph(r, ns)),
            },
            TopKind::Machine(m) => p.machine = Some(lower_machine(m)),
        }
    }
}

fn lower_alphabet(a: &AlphabetCst, ns: &[String]) -> Alphabet {
    Alphabet {
        name: a.name.clone(),
        name_span: a.name_span,
        line: a.line,
        col: a.col,
        exported: a.exported,
        ns: ns.to_vec(),
        elems: a.elems.clone(),
        doc: reduce_doc_run(&a.doc_run),
    }
}

/// Split a world body's items into (tapes, states, grafts, binds), dropping
/// comments. Routine/graph bodies carry no tapes (parsing rejects a `tape`
/// there), so their tape vec is always empty.
fn lower_world_body(items: &[WorldItem]) -> (Vec<TapeDecl>, Vec<State>, Vec<Graft>, Vec<Bind>) {
    let mut tapes = Vec::new();
    let mut states = Vec::new();
    let mut grafts = Vec::new();
    let mut binds = Vec::new();
    for item in items {
        match &item.kind {
            WorldKind::Comment(_) => {}
            WorldKind::Tape(t) => tapes.push(TapeDecl {
                name: t.name.clone(),
                name_span: t.name_span,
                alphabet: t.alphabet.clone(),
                alphabet_span: t.alphabet_span,
                volatile: t.volatile,
                line: t.line,
                span: t.span,
            }),
            WorldKind::State(s) => states.push(lower_state(s)),
            WorldKind::Graft(g) => grafts.push(lower_graft(g)),
            WorldKind::Bind(b) => binds.push(lower_bind(b)),
        }
    }
    (tapes, states, grafts, binds)
}

fn lower_state(s: &StateCst) -> State {
    let rules = s
        .rules
        .iter()
        .filter_map(|ri| match &ri.kind {
            RuleKind::Rule(rc) => Some(rc.rule.clone()),
            RuleKind::Comment(_) => None,
        })
        .collect();
    State {
        entry: s.entry,
        name: s.name.clone(),
        name_span: s.name_span,
        line: s.line,
        col: s.col,
        rules,
        span: s.span,
        doc: reduce_doc_run(&s.doc_run),
    }
}

fn lower_graft(g: &GraftCst) -> Graft {
    Graft {
        entry: g.entry,
        target: g.target.clone(),
        args: g.args.clone(),
        as_name: g.as_name.as_ref().map(|(n, sp)| Ident {
            name: n.clone(),
            span: *sp,
        }),
        line: g.line,
        span: g.span,
        doc: reduce_doc_run(&g.doc_run),
    }
}

fn lower_bind(b: &BindCst) -> Bind {
    Bind {
        target: b.target.clone(),
        args: b.args.clone(),
        as_name: Ident {
            name: b.as_name.0.clone(),
            span: b.as_name.1,
        },
        line: b.line,
        span: b.span,
        doc: reduce_doc_run(&b.doc_run),
    }
}

fn lower_routine(r: &ReuseCst, ns: &[String]) -> Routine {
    let (_tapes, states, grafts, binds) = lower_world_body(&r.items);
    Routine {
        name: r.name.clone(),
        name_span: r.name_span,
        line: r.line,
        col: r.col,
        exported: r.exported,
        ns: ns.to_vec(),
        sig: r.sig.clone(),
        states,
        grafts,
        binds,
        doc: reduce_doc_run(&r.doc_run),
    }
}

fn lower_graph(r: &ReuseCst, ns: &[String]) -> Graph {
    let (_tapes, states, grafts, binds) = lower_world_body(&r.items);
    Graph {
        name: r.name.clone(),
        name_span: r.name_span,
        line: r.line,
        col: r.col,
        exported: r.exported,
        ns: ns.to_vec(),
        sig: r.sig.clone(),
        states,
        grafts,
        binds,
        doc: reduce_doc_run(&r.doc_run),
    }
}

fn lower_machine(m: &MachineCst) -> Machine {
    let (tapes, states, grafts, binds) = lower_world_body(&m.items);
    Machine {
        line: m.line,
        col: m.col,
        span: m.span,
        tapes,
        states,
        grafts,
        binds,
        doc: reduce_doc_run(&m.doc_run),
    }
}

/// Reduce a doc/attention run into a [`Doc`] — `None` for an empty run.
/// `?` lines join into paragraphs (an empty `?` line splits, leading/trailing
/// blanks produce no empty paragraph); a `[deprecated]` attention line becomes
/// `deprecated`; bare-prose `!` lines become `attention`; comments and empty
/// lines contribute nothing. Mirrors PM-1's `reduce_doc_run`. `pub(crate)`
/// rather than private: `crate::syntax::extract`'s own tests reduce a
/// green-side reparsed run the same way this module's callers above do, to
/// check equality where a comment-interleaved run makes raw item equality
/// impossible (a green reparse drops interleaved comments as trivia).
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

/// A comment lifted out of the stream during the split, remembering where it
/// sat relative to the significant tokens.
struct CommentAt {
    comment: Comment,
    /// The comment's own start line (for `blank_before` gaps).
    line: u32,
    /// Number of significant tokens preceding it.
    sig_index: usize,
}

/// A block loop's return shape: items, the block's `close_trailing` comment,
/// and the closing `}` token's span (both `None` at file level).
type TopItemsResult = Result<(Vec<TopItem>, Option<Comment>, Option<Span>), CompileError>;
type WorldItemsResult = Result<(Vec<WorldItem>, Option<Comment>, Option<Span>), CompileError>;

/// A comma-separated list's interior comments: pairs of (index of the entry
/// the comment precedes, comment) — see `Parser::interior_comments`
/// (docs/tmt/fmt.md (interior comments)).
type InteriorComments = Vec<(usize, Comment)>;

/// A binding list's `with map` interior comments, one level down from
/// [`InteriorComments`]: triples of (binding-arg index, index of the map
/// pair the comment precedes, comment) — see `Parser::binding_args`
/// (docs/tmt/fmt.md (interior comments)).
type MapInteriorComments = Vec<(usize, usize, Comment)>;

/// `Parser::rule`'s return shape: the rule itself, its transition's
/// interior comments (a `call`'s own binding list, then every `with map`
/// pair list nested inside it — both empty for a non-`call` transition),
/// then its pattern/write/move vectors' own interior comments, in that
/// order — see [`RuleCst`]'s matching side-car fields (docs/tmt/fmt.md
/// (interior comments)).
type RuleParse = (
    Rule,
    InteriorComments,
    MapInteriorComments,
    InteriorComments,
    InteriorComments,
    InteriorComments,
);

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
    /// Comments split out of the stream, in source order.
    comments: Vec<CommentAt>,
    /// Cursor into `comments`: everything before it is already attached.
    cpos: usize,
    /// End line of the last emitted CST element, for `blank_before`.
    prev_end_line: u32,
    /// A `machine` block has already been seen (multiplicity guard).
    machine_seen: bool,
    /// Green-tree emission, when this walk is [`parse_green`]'s rather
    /// than [`parse_cst`]'s: `bump()` mirrors every consumed token into
    /// it, and the `g_*` helpers below bracket node boundaries. `None`
    /// on every other path — those helpers are then no-ops, so the CST
    /// walk is byte-identical whether or not a sink is attached.
    sink: Option<GreenSink>,
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

    // ---- comment trivia helpers -------------------------------------------

    /// Attach every pending comment at or before the current position, as
    /// `(comment, start_line)` in source order.
    fn drain_pending(&mut self) -> Vec<(Comment, u32)> {
        let mut out = Vec::new();
        while self.cpos < self.comments.len() && self.comments[self.cpos].sig_index <= self.pos {
            let ca = &self.comments[self.cpos];
            out.push((ca.comment.clone(), ca.line));
            self.cpos += 1;
        }
        out
    }

    /// Capture comment(s) on the same physical line as a just-consumed `{`,
    /// before the first body item (`sig_index == pos`, so a comment BEFORE the
    /// brace is excluded even on the same line). Sets `prev_end_line`.
    fn capture_open_trailing(&mut self, brace_line: u32) -> Vec<Comment> {
        self.prev_end_line = brace_line;
        let mut out = Vec::new();
        while self.cpos < self.comments.len() {
            let ca = &self.comments[self.cpos];
            if ca.sig_index == self.pos && ca.line == brace_line {
                out.push(ca.comment.clone());
                self.cpos += 1;
            } else {
                break;
            }
        }
        if let Some(last) = out.last() {
            self.prev_end_line = brace_line + last.text.matches('\n').count() as u32;
        }
        out
    }

    /// Capture a comment on the same physical line as a just-consumed closing
    /// token (`}` or `;`) — `sig_index == pos` after the consume.
    fn capture_close_trailing(&mut self, close_line: u32) -> Option<Comment> {
        if self.cpos < self.comments.len() {
            let ca = &self.comments[self.cpos];
            if ca.sig_index == self.pos && ca.line == close_line {
                self.prev_end_line = close_line + ca.comment.text.matches('\n').count() as u32;
                let c = ca.comment.clone();
                self.cpos += 1;
                return Some(c);
            }
        }
        None
    }

    /// Take the one same-line trailing comment after a `;` (a non-own-line
    /// pending comment on `end_line`).
    fn take_trailing(&mut self, end_line: u32) -> Option<Comment> {
        if self.cpos < self.comments.len() {
            let ca = &self.comments[self.cpos];
            if ca.sig_index <= self.pos && !ca.comment.own_line && ca.line == end_line {
                let c = ca.comment.clone();
                self.cpos += 1;
                return Some(c);
            }
        }
        None
    }

    /// Drain every pending comment written before entry `index` of the list
    /// being parsed, tagging each with that index. Called at the top of each
    /// list-loop iteration and once more before the closer with
    /// `index = entries.len()`, which is how a comment after the last entry
    /// gets a home (docs/tmt/fmt.md (interior comments)).
    fn interior_comments(&mut self, index: usize, out: &mut InteriorComments) {
        while self.cpos < self.comments.len() && self.comments[self.cpos].sig_index <= self.pos {
            out.push((index, self.comments[self.cpos].comment.clone()));
            self.cpos += 1;
        }
    }

    // ---- doc runs ---------------------------------------------------------

    /// Collect a doc/attention run at the current position (the caller has
    /// confirmed the leading token is a `?`/`!` line). Fixed order: a `?` after
    /// the run's first `!` is `DocLineOrder`. Blanks and ordinary comments are
    /// tolerated within/after. Returns the items plus the run's first line span.
    fn doc_run(&mut self) -> Result<(Vec<DocRunItem>, Span), CompileError> {
        let first_span = self.peek().span();
        let mut items: Vec<DocRunItem> = Vec::new();
        let mut seen_attention = false;
        let mut seen_deprecated = false;
        let mut prev_end_line = self.prev_end_line;
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
            for (comment, cline) in self.drain_pending() {
                let blank_before = cline > prev_end_line + 1;
                prev_end_line = cline + comment.text.matches('\n').count() as u32;
                items.push(DocRunItem {
                    blank_before,
                    kind: DocRunKind::Comment(comment),
                });
            }
        }
        self.prev_end_line = prev_end_line;
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
    fn file(mut self) -> Result<(Vec<TopItem>, Option<GreenSink>), CompileError> {
        let (items, _, _) = self.top_items(&[], None)?;
        Ok((items, self.sink))
    }

    /// True iff the current token starts a declaration that accepts a doc run.
    fn next_is_top_doc_accepting(&self) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(w)
            if matches!(w.as_str(),
                "export" | "alphabet" | "routine" | "graph" | "machine" | "namespace"))
    }

    /// One namespace level's item loop.
    fn top_items(&mut self, ns: &[String], terminator: Option<&TokenKind>) -> TopItemsResult {
        let mut items: Vec<TopItem> = Vec::new();
        loop {
            for (comment, cline) in self.drain_pending() {
                let blank_before = cline > self.prev_end_line + 1;
                self.prev_end_line = cline + comment.text.matches('\n').count() as u32;
                items.push(TopItem {
                    blank_before,
                    kind: TopKind::Comment(comment),
                });
            }
            // Green checkpoint for whichever node this item turns out to
            // be: taken here, after the pending-comment drain and BEFORE
            // the doc run (if any) — mirrors PM's `fn_cp` placement
            // (crates/post-machine/src/parser.rs) so `g_start_at` below
            // retro-wraps the run, an `export` prefix when present, and
            // the header onward, all from one checkpoint. A declaration
            // retro-wraps its bound doc run by design — see this
            // crate's `syntax` module doc for the decision and its
            // reasoning (docs/core.md (syntax trees)). Unused only when
            // this token starts no valid top-level declaration at all;
            // harmless, a fresh checkpoint is taken every loop
            // iteration.
            let cp = self.g_checkpoint();
            let doc_run = if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                self.g_flush_start(TmcKind::DocRun);
                let (run, first_span) = self.doc_run()?;
                self.g_finish(); // DocRun
                if !self.next_is_top_doc_accepting() {
                    return Err(CompileError {
                        span: first_span,
                        kind: CompileErrorKind::DanglingDocRun,
                    });
                }
                run
            } else {
                Vec::new()
            };
            let t = self.peek().clone();
            match (&t.kind, terminator) {
                (TokenKind::Eof, None) => return Ok((items, None, None)),
                (TokenKind::Eof, Some(_)) => {
                    return Err(Self::expected(&t, "`}` to close the namespace block"));
                }
                (k, Some(term)) if k == term => {
                    let close_line = t.line;
                    self.prev_end_line = close_line;
                    self.bump();
                    let close_trailing = self.capture_close_trailing(close_line);
                    return Ok((items, close_trailing, Some(t.span())));
                }
                _ => {}
            }
            let saved = self.prev_end_line;
            let decl_line = t.line;
            let kind = match &t.kind {
                TokenKind::Ident(w) => match w.as_str() {
                    "use" => {
                        self.g_start_at(cp, TmcKind::Use);
                        let u = self.parse_use()?;
                        self.g_finish(); // Use — closes right after the `;`
                        TopKind::Import(u)
                    }
                    "alphabet" => {
                        self.g_start_at(cp, TmcKind::Alphabet);
                        let a = self.parse_alphabet(false, t.span().start, t.col, doc_run)?;
                        self.g_finish(); // Alphabet
                        TopKind::Alphabet(a)
                    }
                    "routine" => {
                        self.g_start_at(cp, TmcKind::Reuse);
                        let r = self.parse_reuse(
                            ReuseCarrier::Routine,
                            false,
                            t.span().start,
                            t.col,
                            doc_run,
                        )?;
                        self.g_finish(); // Reuse — closes right after the `}`
                        TopKind::Reuse(r)
                    }
                    "graph" => {
                        self.g_start_at(cp, TmcKind::Reuse);
                        let r = self.parse_reuse(
                            ReuseCarrier::Graph,
                            false,
                            t.span().start,
                            t.col,
                            doc_run,
                        )?;
                        self.g_finish(); // Reuse — closes right after the `}`
                        TopKind::Reuse(r)
                    }
                    "namespace" => {
                        self.g_start_at(cp, TmcKind::Namespace);
                        let n = self.parse_namespace(ns, doc_run)?;
                        self.g_finish(); // Namespace — closes right after the `}`
                        TopKind::Namespace(n)
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
                        let m = self.parse_machine(doc_run)?;
                        self.g_finish(); // Machine — closes right after the `}`
                        TopKind::Machine(m)
                    }
                    "export" => {
                        let export_start = t.span().start;
                        let export_col = t.col;
                        self.bump();
                        let t2 = self.peek().clone();
                        match &t2.kind {
                            TokenKind::Ident(w2) if w2 == "alphabet" => {
                                self.g_start_at(cp, TmcKind::Alphabet);
                                let a =
                                    self.parse_alphabet(true, export_start, export_col, doc_run)?;
                                self.g_finish(); // Alphabet — `export` included
                                TopKind::Alphabet(a)
                            }
                            TokenKind::Ident(w2) if w2 == "routine" => {
                                self.g_start_at(cp, TmcKind::Reuse);
                                let r = self.parse_reuse(
                                    ReuseCarrier::Routine,
                                    true,
                                    export_start,
                                    export_col,
                                    doc_run,
                                )?;
                                self.g_finish(); // Reuse — `export` included
                                TopKind::Reuse(r)
                            }
                            TokenKind::Ident(w2) if w2 == "graph" => {
                                self.g_start_at(cp, TmcKind::Reuse);
                                let r = self.parse_reuse(
                                    ReuseCarrier::Graph,
                                    true,
                                    export_start,
                                    export_col,
                                    doc_run,
                                )?;
                                self.g_finish(); // Reuse — `export` included
                                TopKind::Reuse(r)
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
            };
            let blank_before = decl_line > saved + 1;
            items.push(TopItem { blank_before, kind });
        }
    }

    fn parse_use(&mut self) -> Result<UseCst, CompileError> {
        let use_tok = self.peek().clone();
        self.bump(); // `use`
        let mut paths: Vec<UsePath> = Vec::new();
        let mut interior: Vec<(usize, Comment)> = Vec::new();
        let semi_line;
        loop {
            self.interior_comments(paths.len(), &mut interior);
            self.g_flush_start(TmcKind::UsePath);
            let (first, first_span) = self.name("an imported name")?;
            let mut path = vec![first];
            let mut end = first_span;
            while matches!(self.peek().kind, TokenKind::ColonColon) {
                self.bump();
                let (seg, seg_span) = self.name("a path segment")?;
                path.push(seg);
                end = seg_span;
            }
            let alias = if self.at_kw("as") {
                self.bump();
                let (a, _) = self.name("an alias")?;
                Some(a)
            } else {
                None
            };
            self.g_finish(); // UsePath — the alias, if any, is its last token
            paths.push(UsePath {
                path,
                alias,
                line: first_span.start.line,
                span: join(first_span, end),
            });
            let sep = self.peek().clone();
            match sep.kind {
                TokenKind::Comma => self.bump(),
                TokenKind::Semi => {
                    semi_line = sep.line;
                    // Drain interior comments HERE, before bumping past `;`:
                    // `interior_comments` claims everything at or before
                    // `self.pos`, so running it once the `;` has been
                    // consumed would also claim a comment that follows the
                    // statement — e.g. one documenting the *next* `use`
                    // (docs/tmt/fmt.md (interior comments)).
                    self.interior_comments(paths.len(), &mut interior);
                    self.bump();
                    break;
                }
                _ => return Err(Self::expected(&sep, "`,` or `;`")),
            }
        }
        self.prev_end_line = semi_line;
        let trailing = self.take_trailing(semi_line);
        let span = join(
            paths.first().expect("a use list has a path").span,
            paths.last().expect("a use list has a path").span,
        );
        Ok(UseCst {
            paths,
            interior,
            line: use_tok.line,
            span: join(use_tok.span(), span),
            trailing,
        })
    }

    fn parse_alphabet(
        &mut self,
        exported: bool,
        header_start: Pos,
        header_col: u32,
        doc_run: Vec<DocRunItem>,
    ) -> Result<AlphabetCst, CompileError> {
        self.bump(); // `alphabet`
        let (name, name_span) = self.name("an alphabet name")?;
        let brace = self.expect(&TokenKind::LBrace, "`{` to open the alphabet body")?;
        let open_trailing = self.capture_open_trailing(brace.line);
        let (elems, interior) = self.alphabet_elems()?;
        let close = self.expect(&TokenKind::RBrace, "`}` to close the alphabet body")?;
        self.prev_end_line = close.line;
        let close_trailing = self.capture_close_trailing(close.line);
        Ok(AlphabetCst {
            name,
            name_span,
            line: name_span.start.line,
            col: header_col,
            exported,
            elems,
            interior,
            span: Span {
                start: header_start,
                end: close.span().end,
            },
            doc_run,
            open_trailing,
            close_trailing,
        })
    }

    /// An alphabet body's element loop, from just past the opening `{`
    /// up to (not consuming) the closing `}` — the elements themselves
    /// and the list's own interior comments. Split out of
    /// [`Self::parse_alphabet`] so [`reparse_alphabet_elems`] runs this
    /// exact loop rather than a second copy of it: the comma/`}`
    /// separator rule and the "a comment after the last element still
    /// gets a home" final drain are grammar decisions with one owner
    /// (docs/tmt/fmt.md (comments inside a list)). An empty body (`{ }`)
    /// skips the loop and yields no elements, which is why the
    /// `RBrace` pre-check sits inside here rather than at the call
    /// site.
    fn alphabet_elems(&mut self) -> Result<(Vec<AlphabetElem>, InteriorComments), CompileError> {
        let mut elems: Vec<AlphabetElem> = Vec::new();
        let mut interior: InteriorComments = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBrace) {
            loop {
                self.interior_comments(elems.len(), &mut interior);
                elems.push(self.alphabet_elem()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RBrace => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `}`")),
                }
            }
        }
        self.interior_comments(elems.len(), &mut interior);
        Ok((elems, interior))
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

    fn parse_namespace(
        &mut self,
        ns: &[String],
        doc_run: Vec<DocRunItem>,
    ) -> Result<NamespaceCst, CompileError> {
        let ns_tok = self.peek().clone();
        self.bump(); // `namespace`
        let (name, name_span) = self.name("a namespace name")?;
        let brace = self.expect(&TokenKind::LBrace, "`{` to open the namespace body")?;
        let open_trailing = self.capture_open_trailing(brace.line);
        let mut child = ns.to_vec();
        child.push(name.clone());
        let (child_items, close_trailing, close_span) =
            self.top_items(&child, Some(&TokenKind::RBrace))?;
        Ok(NamespaceCst {
            name,
            name_span,
            line: ns_tok.line,
            span: Span {
                start: ns_tok.span().start,
                end: close_span
                    .expect("top_items with a terminator returns a close span")
                    .end,
            },
            items: child_items,
            doc_run,
            open_trailing,
            close_trailing,
        })
    }

    fn parse_reuse(
        &mut self,
        carrier: ReuseCarrier,
        exported: bool,
        header_start: Pos,
        header_col: u32,
        doc_run: Vec<DocRunItem>,
    ) -> Result<ReuseCst, CompileError> {
        self.bump(); // `routine` / `graph`
        let what = match carrier {
            ReuseCarrier::Routine => "a routine name",
            ReuseCarrier::Graph => "a graph name",
        };
        let (name, name_span) = self.name(what)?;
        let (sig, sig_interior) = self.signature()?;
        // WORLD wraps the `{ … }` body — the shape `machine`/`routine`/
        // `graph` share (docs/tmt/language.md (worlds)); see the
        // `syntax` module doc for why it gets its own node kind rather
        // than folding into REUSE/MACHINE directly. Opened before the
        // `{` so the brace itself is WORLD's first token.
        self.g_flush_start(TmcKind::World);
        let brace = self.expect(&TokenKind::LBrace, "`{` to open the body")?;
        let open_trailing = self.capture_open_trailing(brace.line);
        let (items, close_trailing, close_span) = self.world_body(false)?;
        self.g_finish(); // World — closes right after the closing `}`
        Ok(ReuseCst {
            carrier,
            name,
            name_span,
            line: name_span.start.line,
            col: header_col,
            exported,
            sig,
            sig_interior,
            items,
            span: Span {
                start: header_start,
                end: close_span.expect("world_body returns a close span").end,
            },
            doc_run,
            open_trailing,
            close_trailing,
        })
    }

    fn parse_machine(&mut self, doc_run: Vec<DocRunItem>) -> Result<MachineCst, CompileError> {
        let machine_tok = self.peek().clone();
        self.bump(); // `machine`
        // WORLD wraps the `{ … }` body — see `parse_reuse`'s matching
        // comment; the same shared shape, no signature to skip over here.
        self.g_flush_start(TmcKind::World);
        let brace = self.expect(&TokenKind::LBrace, "`{` to open the machine body")?;
        let open_trailing = self.capture_open_trailing(brace.line);
        let (items, close_trailing, close_span) = self.world_body(true)?;
        self.g_finish(); // World — closes right after the closing `}`
        Ok(MachineCst {
            line: machine_tok.line,
            col: machine_tok.col,
            items,
            span: Span {
                start: machine_tok.span().start,
                end: close_span.expect("world_body returns a close span").end,
            },
            doc_run,
            open_trailing,
            close_trailing,
        })
    }

    fn signature(&mut self) -> Result<(Signature, InteriorComments), CompileError> {
        let lp = self.expect(&TokenKind::LParen, "`(` to open the signature")?;
        let mut params: Vec<SigParam> = Vec::new();
        let mut interior: Vec<(usize, Comment)> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RParen) {
            loop {
                self.interior_comments(params.len(), &mut interior);
                // SIG_PARAM opens at the parameter's first significant
                // token, so a comment written between two parameters
                // flushes into the enclosing REUSE rather than into
                // either parameter (docs/core.md (syntax trees)).
                self.g_flush_start(TmcKind::SigParam);
                params.push(self.sig_param()?);
                self.g_finish(); // SigParam
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RParen => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `)`")),
                }
            }
        }
        self.interior_comments(params.len(), &mut interior);
        let rp = self.expect(&TokenKind::RParen, "`)` to close the signature")?;
        Ok((
            Signature {
                params,
                span: join(lp.span(), rp.span()),
            },
            interior,
        ))
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
    fn world_body(&mut self, in_machine: bool) -> WorldItemsResult {
        let mut items: Vec<WorldItem> = Vec::new();
        loop {
            for (comment, cline) in self.drain_pending() {
                let blank_before = cline > self.prev_end_line + 1;
                self.prev_end_line = cline + comment.text.matches('\n').count() as u32;
                items.push(WorldItem {
                    blank_before,
                    kind: WorldKind::Comment(comment),
                });
            }
            // Green checkpoint for whichever node this item turns out to
            // be: same placement rule as `top_items`'s `cp` — after the
            // pending-comment drain, before the doc run (if any) — so
            // `g_start_at` below retro-wraps the run and an `entry`
            // prefix, when present, alongside the header. TAPE never
            // accepts a doc run (`next_is_world_doc_accepting` excludes
            // it), so this same checkpoint sits at TAPE's own header
            // token whenever it fires there — no separate checkpoint
            // needed.
            let cp = self.g_checkpoint();
            let doc_run = if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                self.g_flush_start(TmcKind::DocRun);
                let (run, first_span) = self.doc_run()?;
                self.g_finish(); // DocRun
                if !self.next_is_world_doc_accepting() {
                    return Err(CompileError {
                        span: first_span,
                        kind: CompileErrorKind::DanglingDocRun,
                    });
                }
                run
            } else {
                Vec::new()
            };
            let t = self.peek().clone();
            if matches!(t.kind, TokenKind::RBrace) {
                let close_line = t.line;
                self.prev_end_line = close_line;
                self.bump();
                let close_trailing = self.capture_close_trailing(close_line);
                return Ok((items, close_trailing, Some(t.span())));
            }
            if matches!(t.kind, TokenKind::Eof) {
                return Err(Self::expected(&t, "`}` to close the body"));
            }
            let saved = self.prev_end_line;
            let item_line = t.line;
            let kind = if self.at_kw("entry") {
                let entry_tok = self.peek().clone();
                self.bump();
                let prefix = Some((entry_tok.span().start, entry_tok.col));
                if self.at_kw("state") {
                    self.g_start_at(cp, TmcKind::State);
                    let s = self.parse_state(true, prefix, doc_run)?;
                    self.g_finish(); // State — `entry` included
                    WorldKind::State(s)
                } else if self.at_kw("graft") {
                    self.g_start_at(cp, TmcKind::Graft);
                    let g = self.parse_graft(true, prefix, doc_run)?;
                    self.g_finish(); // Graft — `entry` included
                    WorldKind::Graft(g)
                } else {
                    return Err(Self::expected(
                        self.peek(),
                        "`state` or `graft` after `entry`",
                    ));
                }
            } else if self.at_kw("state") {
                self.g_start_at(cp, TmcKind::State);
                let s = self.parse_state(false, None, doc_run)?;
                self.g_finish(); // State
                WorldKind::State(s)
            } else if self.at_kw("graft") {
                self.g_start_at(cp, TmcKind::Graft);
                let g = self.parse_graft(false, None, doc_run)?;
                self.g_finish(); // Graft
                WorldKind::Graft(g)
            } else if self.at_kw("bind") {
                self.g_start_at(cp, TmcKind::Bind);
                let b = self.parse_bind(doc_run)?;
                self.g_finish(); // Bind
                WorldKind::Bind(b)
            } else if self.at_kw("volatile") {
                let lead = self.peek().clone();
                self.bump(); // `volatile`
                if !self.at_kw("tape") {
                    return Err(Self::expected(self.peek(), "`tape` after `volatile`"));
                }
                if in_machine {
                    self.g_start_at(cp, TmcKind::Tape); // `volatile` included
                    let tape = self.parse_tape(true, lead)?;
                    self.g_finish(); // Tape
                    WorldKind::Tape(tape)
                } else {
                    return Err(Self::err_at(&t, CompileErrorKind::TapeNotInMachine));
                }
            } else if self.at_kw("tape") {
                if in_machine {
                    let lead = self.peek().clone();
                    self.g_start_at(cp, TmcKind::Tape);
                    let tape = self.parse_tape(false, lead)?;
                    self.g_finish(); // Tape
                    WorldKind::Tape(tape)
                } else {
                    return Err(Self::err_at(&t, CompileErrorKind::TapeNotInMachine));
                }
            } else {
                return Err(Self::expected(
                    &t,
                    "a tape declaration, `state`, `graft`, or `bind`",
                ));
            };
            let blank_before = item_line > saved + 1;
            items.push(WorldItem { blank_before, kind });
        }
    }

    fn parse_tape(&mut self, volatile: bool, lead_tok: Token) -> Result<TapeCst, CompileError> {
        self.bump(); // `tape`
        let (name, name_span) = self.name("a tape name")?;
        self.expect(&TokenKind::Colon, "`:` after the tape name")?;
        let (alphabet, alphabet_span) = self.name("an alphabet name")?;
        let semi = self.expect(&TokenKind::Semi, "`;`")?;
        self.prev_end_line = semi.line;
        let trailing = self.take_trailing(semi.line);
        Ok(TapeCst {
            name,
            name_span,
            alphabet,
            alphabet_span,
            volatile,
            line: lead_tok.line,
            span: join(lead_tok.span(), semi.span()),
            trailing,
        })
    }

    fn parse_state(
        &mut self,
        entry: bool,
        prefix: Option<(Pos, u32)>,
        doc_run: Vec<DocRunItem>,
    ) -> Result<StateCst, CompileError> {
        let state_tok = self.peek().clone();
        self.bump(); // `state`
        let (name, name_span) = self.name("a state name")?;
        // `state name;` redirect form is not supported.
        if matches!(self.peek().kind, TokenKind::Semi) {
            return Err(Self::err_at(self.peek(), CompileErrorKind::StateRedirect));
        }
        let brace = self.expect(&TokenKind::LBrace, "`{` to open the state body")?;
        let open_trailing = self.capture_open_trailing(brace.line);
        let (rules, close_trailing, close_span) = self.state_rules()?;
        let (start, col) = prefix.unwrap_or((state_tok.span().start, state_tok.col));
        Ok(StateCst {
            entry,
            name,
            name_span,
            line: name_span.start.line,
            col,
            rules,
            span: Span {
                start,
                end: close_span.end,
            },
            doc_run,
            open_trailing,
            close_trailing,
        })
    }

    /// A state body's rule loop; returns rules, `close_trailing`, `}` span.
    fn state_rules(&mut self) -> Result<(Vec<RuleItem>, Option<Comment>, Span), CompileError> {
        let mut rules: Vec<RuleItem> = Vec::new();
        loop {
            for (comment, cline) in self.drain_pending() {
                let blank_before = cline > self.prev_end_line + 1;
                self.prev_end_line = cline + comment.text.matches('\n').count() as u32;
                rules.push(RuleItem {
                    blank_before,
                    kind: RuleKind::Comment(comment),
                });
            }
            let t = self.peek().clone();
            if matches!(t.kind, TokenKind::RBrace) {
                let close_line = t.line;
                self.prev_end_line = close_line;
                self.bump();
                let close_trailing = self.capture_close_trailing(close_line);
                return Ok((rules, close_trailing, t.span()));
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
            let saved = self.prev_end_line;
            let rule_line = t.line;
            self.g_flush_start(TmcKind::Rule);
            let (rule, call_args, map_pairs, pattern_cells, write_cells, move_cells) =
                self.rule()?;
            self.g_finish(); // Rule
            let trailing = self.take_trailing(self.prev_end_line);
            let blank_before = rule_line > saved + 1;
            rules.push(RuleItem {
                blank_before,
                kind: RuleKind::Rule(Box::new(RuleCst {
                    rule,
                    trailing,
                    call_args,
                    map_pairs,
                    pattern_cells,
                    write_cells,
                    move_cells,
                })),
            });
        }
    }

    // ---- rules ------------------------------------------------------------

    /// Parses one rule; also returns the transition's interior comments and
    /// each glyph vector's own — [`Rule`] is handed to the AST verbatim, so
    /// `state_rules` stores these on [`RuleCst`]'s side-car fields instead
    /// (docs/tmt/fmt.md (interior comments)).
    fn rule(&mut self) -> Result<RuleParse, CompileError> {
        let (pattern, pattern_cells) = self.pattern()?;
        self.expect(&TokenKind::Arrow, "`->` after the pattern")?;
        let debugger = if self.at_kw("debugger") {
            self.bump();
            true
        } else {
            false
        };
        let (write, write_cells) = if self.at_kw("write") {
            self.bump();
            let (w, cells) = self.write_vec()?;
            (Some(w), cells)
        } else {
            (None, InteriorComments::new())
        };
        let (mov, move_cells) = if self.at_kw("move") {
            self.bump();
            let (m, cells) = self.move_vec()?;
            (Some(m), cells)
        } else {
            (None, InteriorComments::new())
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
        let (transition, call_args, map_pairs) =
            if has_action && matches!(self.peek().kind, TokenKind::Semi) {
                (
                    Transition::Stay {
                        span: self.peek().span(),
                    },
                    InteriorComments::new(),
                    MapInteriorComments::new(),
                )
            } else {
                self.g_flush_start(TmcKind::Transition);
                let parsed = self.transition()?;
                self.g_finish(); // Transition
                parsed
            };
        let semi = self.expect(&TokenKind::Semi, "`;` to end the rule")?;
        self.prev_end_line = semi.line;
        // Char arithmetic is deliberately absent: a `{c±k}` on a glyph-bound
        // pattern name is rejected here, where the rule's bindings are known.
        self.check_char_arithmetic(&pattern, &write)?;
        Ok((
            Rule {
                pattern: pattern.clone(),
                debugger,
                write,
                mov,
                transition,
                line: pattern.span.start.line,
                span: join(pattern.span, semi.span()),
            },
            call_args,
            map_pairs,
            pattern_cells,
            write_cells,
            move_cells,
        ))
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

    /// Parses a bracketed pattern; also returns its interior comments — a
    /// [`Pattern`] is handed to the AST verbatim, so `rule` stores these on
    /// [`RuleCst`]'s side-car field instead (docs/tmt/fmt.md (interior
    /// comments)).
    fn pattern(&mut self) -> Result<(Pattern, InteriorComments), CompileError> {
        let lb = self.expect(&TokenKind::LBracket, "`[` to open the pattern")?;
        let mut cells: Vec<PatternCell> = Vec::new();
        let mut interior: InteriorComments = Vec::new();
        loop {
            self.interior_comments(cells.len(), &mut interior);
            cells.push(self.pattern_cell()?);
            match self.peek().kind {
                TokenKind::Comma => self.bump(),
                TokenKind::RBracket => break,
                _ => return Err(Self::expected(self.peek(), "`,` or `]`")),
            }
        }
        // Drain HERE, before consuming `]`: `interior_comments` claims
        // everything at or before `self.pos`, so running it after the `]`
        // has been consumed would also claim whatever comes next
        // (docs/tmt/fmt.md (interior comments)).
        self.interior_comments(cells.len(), &mut interior);
        let rb = self.expect(&TokenKind::RBracket, "`]` to close the pattern")?;
        Ok((
            Pattern {
                cells,
                span: join(lb.span(), rb.span()),
            },
            interior,
        ))
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

    /// Parses a bracketed write vector; also returns its interior comments —
    /// a [`WriteVec`] is handed to the AST verbatim, so `rule` stores these
    /// on [`RuleCst`]'s side-car field instead (docs/tmt/fmt.md (interior
    /// comments)).
    fn write_vec(&mut self) -> Result<(WriteVec, InteriorComments), CompileError> {
        // The node opens at `[`, not at the `write` keyword the caller
        // already consumed, so its extent is exactly `WriteVec::span`.
        // The keyword stays a token of the enclosing RULE — nothing has
        // to read it, since the node's own KIND says which vector this
        // is (docs/core.md (syntax trees)).
        self.g_flush_start(TmcKind::WriteVec);
        let lb = self.expect(&TokenKind::LBracket, "`[` to open the write vector")?;
        let mut cells: Vec<WriteCell> = Vec::new();
        let mut interior: InteriorComments = Vec::new();
        loop {
            self.interior_comments(cells.len(), &mut interior);
            cells.push(self.write_cell()?);
            match self.peek().kind {
                TokenKind::Comma => self.bump(),
                TokenKind::RBracket => break,
                _ => return Err(Self::expected(self.peek(), "`,` or `]`")),
            }
        }
        // Drain HERE, before consuming `]` — see `pattern`'s identical note.
        self.interior_comments(cells.len(), &mut interior);
        let rb = self.expect(&TokenKind::RBracket, "`]` to close the write vector")?;
        self.g_finish(); // WriteVec
        Ok((
            WriteVec {
                cells,
                span: join(lb.span(), rb.span()),
            },
            interior,
        ))
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

    /// Parses a bracketed move vector; also returns its interior comments —
    /// a [`MoveVec`] is handed to the AST verbatim, so `rule` stores these
    /// on [`RuleCst`]'s side-car field instead (docs/tmt/fmt.md (interior
    /// comments)).
    fn move_vec(&mut self) -> Result<(MoveVec, InteriorComments), CompileError> {
        // Opens at `[` — see `write_vec`'s identical note.
        self.g_flush_start(TmcKind::MoveVec);
        let lb = self.expect(&TokenKind::LBracket, "`[` to open the move vector")?;
        let mut cells: Vec<MoveCell> = Vec::new();
        let mut interior: InteriorComments = Vec::new();
        loop {
            self.interior_comments(cells.len(), &mut interior);
            cells.push(self.move_cell()?);
            match self.peek().kind {
                TokenKind::Comma => self.bump(),
                TokenKind::RBracket => break,
                _ => return Err(Self::expected(self.peek(), "`,` or `]`")),
            }
        }
        // Drain HERE, before consuming `]` — see `pattern`'s identical note.
        self.interior_comments(cells.len(), &mut interior);
        let rb = self.expect(&TokenKind::RBracket, "`]` to close the move vector")?;
        self.g_finish(); // MoveVec
        Ok((
            MoveVec {
                cells,
                span: join(lb.span(), rb.span()),
            },
            interior,
        ))
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

    /// Parses one transition; also returns its interior comments — a
    /// `call`'s own binding-list comments, and every `with map` pair-list
    /// comment nested inside that binding list — both empty for every
    /// non-`call` variant (docs/tmt/fmt.md (interior comments)).
    fn transition(
        &mut self,
    ) -> Result<(Transition, InteriorComments, MapInteriorComments), CompileError> {
        let t = self.peek().clone();
        match &t.kind {
            TokenKind::Ident(w) if w == "goto" => {
                self.bump();
                let (name, name_span) = self.name("a goto target")?;
                Ok((
                    Transition::Goto {
                        name,
                        explicit: true,
                        span: join(t.span(), name_span),
                    },
                    InteriorComments::new(),
                    MapInteriorComments::new(),
                ))
            }
            TokenKind::Ident(w) if w == "call" => {
                self.bump();
                let target = self.qual_name("a call target")?;
                let (args, call_args, map_pairs) = self.binding_args()?;
                self.expect_kw("then", "`then` after the call target")?;
                let then = self.continuation()?;
                let end = match &then {
                    Continuation::State { span, .. }
                    | Continuation::Return { span }
                    | Continuation::Stop { span }
                    | Continuation::Halt { span } => *span,
                };
                Ok((
                    Transition::Call {
                        target,
                        args,
                        then,
                        span: join(t.span(), end),
                    },
                    call_args,
                    map_pairs,
                ))
            }
            TokenKind::Ident(w) if w == "return" => {
                self.bump();
                Ok((
                    Transition::Return { span: t.span() },
                    InteriorComments::new(),
                    MapInteriorComments::new(),
                ))
            }
            TokenKind::Ident(w) if w == "stop" => {
                self.bump();
                Ok((
                    Transition::Stop { span: t.span() },
                    InteriorComments::new(),
                    MapInteriorComments::new(),
                ))
            }
            TokenKind::Ident(w) if w == "halt" => {
                self.bump();
                Ok((
                    Transition::Halt { span: t.span() },
                    InteriorComments::new(),
                    MapInteriorComments::new(),
                ))
            }
            TokenKind::Ident(w) if !RESERVED.contains(&w.as_str()) => {
                // Bare-name transition = goto sugar.
                self.bump();
                Ok((
                    Transition::Goto {
                        name: w.clone(),
                        explicit: false,
                        span: t.span(),
                    },
                    InteriorComments::new(),
                    MapInteriorComments::new(),
                ))
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

    /// Parses a `(args)` binding list; also returns the list's own interior
    /// comments and every `with map` pair-list comment nested inside one of
    /// its arguments, the latter re-keyed by the owning argument's index
    /// (docs/tmt/fmt.md (interior comments)).
    fn binding_args(
        &mut self,
    ) -> Result<(Vec<BindingArg>, InteriorComments, MapInteriorComments), CompileError> {
        self.expect(&TokenKind::LParen, "`(` to open the binding")?;
        let mut args: Vec<BindingArg> = Vec::new();
        let mut interior: InteriorComments = Vec::new();
        let mut map_interior: MapInteriorComments = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RParen) {
            loop {
                self.interior_comments(args.len(), &mut interior);
                let arg_index = args.len();
                // BINDING_ARG opens at the argument's own name IDENT, so
                // its extent is exactly `BindingArg::span` and a comment
                // between two arguments flushes into the enclosing
                // GRAFT/BIND/TRANSITION instead.
                self.g_flush_start(TmcKind::BindingArg);
                let (arg, arg_map_interior) = self.binding_arg()?;
                self.g_finish(); // BindingArg
                map_interior.extend(
                    arg_map_interior
                        .into_iter()
                        .map(|(pair_index, comment)| (arg_index, pair_index, comment)),
                );
                args.push(arg);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RParen => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `)`")),
                }
            }
        }
        self.interior_comments(args.len(), &mut interior);
        self.expect(&TokenKind::RParen, "`)` to close the binding")?;
        Ok((args, interior, map_interior))
    }

    /// Parses one `name = value` binding argument; also returns its map's
    /// interior comments, if `value` carries one (empty otherwise) — a
    /// [`BindingArg`] is handed to the AST verbatim, so `binding_args`
    /// re-keys these by the argument's own index before returning them
    /// (docs/tmt/fmt.md (interior comments)).
    fn binding_arg(&mut self) -> Result<(BindingArg, InteriorComments), CompileError> {
        let (name, name_span) = self.name("a binding argument name")?;
        self.expect(&TokenKind::Eq, "`=` in the binding argument")?;
        let t = self.peek().clone();
        let (value, end, map_interior) = match &t.kind {
            TokenKind::Ident(w) if w == "return" => {
                self.bump();
                (
                    BindingValue::Terminator {
                        kind: TermKind::Return,
                        span: t.span(),
                    },
                    t.span(),
                    InteriorComments::new(),
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
                    InteriorComments::new(),
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
                    InteriorComments::new(),
                )
            }
            TokenKind::Ident(w) if !RESERVED.contains(&w.as_str()) => {
                let target = w.clone();
                let target_span = t.span();
                self.bump();
                let (map, end, map_interior) = if self.at_kw("with") {
                    self.bump();
                    let (m, interior) = self.sym_map()?;
                    let sp = m.span;
                    (Some(m), sp, interior)
                } else {
                    (None, target_span, InteriorComments::new())
                };
                (
                    BindingValue::Named {
                        target,
                        target_span,
                        map,
                    },
                    end,
                    map_interior,
                )
            }
            _ => {
                return Err(Self::expected(
                    &t,
                    "a binding target: a tape/state name, `return`, `stop`, or `halt`",
                ));
            }
        };
        Ok((
            BindingArg {
                name,
                name_span,
                value,
                span: join(name_span, end),
            },
            map_interior,
        ))
    }

    /// `map { pairs }` after a consumed `with`; returns its own interior
    /// list, mirroring [`Parser::binding_args`].
    fn sym_map(&mut self) -> Result<(SymMap, InteriorComments), CompileError> {
        // Opens at `map`, not at the `with` the caller already consumed:
        // `SymMap::span` runs `map` → `}`, and keeping the node's extent
        // equal to that span is what lets extraction copy it rather than
        // recompute it.
        self.g_flush_start(TmcKind::SymMap);
        let map_tok = self.expect_kw_tok("map", "`map` after `with`")?;
        self.expect(&TokenKind::LBrace, "`{` to open the map")?;
        let mut pairs: Vec<MapPair> = Vec::new();
        let mut interior: InteriorComments = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBrace) {
            loop {
                self.interior_comments(pairs.len(), &mut interior);
                pairs.push(self.map_pair()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RBrace => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `}`")),
                }
            }
        }
        self.interior_comments(pairs.len(), &mut interior);
        let rb = self.expect(&TokenKind::RBrace, "`}` to close the map")?;
        self.g_finish(); // SymMap
        Ok((
            SymMap {
                pairs,
                span: join(map_tok.span(), rb.span()),
            },
            interior,
        ))
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

    fn parse_graft(
        &mut self,
        entry: bool,
        prefix: Option<(Pos, u32)>,
        doc_run: Vec<DocRunItem>,
    ) -> Result<GraftCst, CompileError> {
        let graft_tok = self.peek().clone();
        self.bump(); // `graft`
        let target = self.qual_name("a graft target")?;
        let (args, interior, map_pairs) = self.binding_args()?;
        let as_name = if self.at_kw("as") {
            self.bump();
            let (n, sp) = self.name("a graft instance name")?;
            Some((n, sp))
        } else {
            None
        };
        // A non-entry graft must be named.
        if !entry && as_name.is_none() {
            return Err(Self::err_at(&graft_tok, CompileErrorKind::GraftNeedsName));
        }
        let semi = self.expect(&TokenKind::Semi, "`;` to end the graft")?;
        self.prev_end_line = semi.line;
        let trailing = self.take_trailing(semi.line);
        let start = prefix
            .map(|(p, _)| p)
            .unwrap_or_else(|| graft_tok.span().start);
        Ok(GraftCst {
            entry,
            target,
            args,
            interior,
            map_pairs,
            as_name,
            line: graft_tok.line,
            span: Span {
                start,
                end: semi.span().end,
            },
            doc_run,
            trailing,
        })
    }

    fn parse_bind(&mut self, doc_run: Vec<DocRunItem>) -> Result<BindCst, CompileError> {
        let bind_tok = self.peek().clone();
        self.bump(); // `bind`
        let target = self.qual_name("a bind target")?;
        let (args, interior, map_pairs) = self.binding_args()?;
        self.expect_kw("as", "`as` (a bind needs an instance name)")?;
        let (n, sp) = self.name("a bind instance name")?;
        let semi = self.expect(&TokenKind::Semi, "`;` to end the bind")?;
        self.prev_end_line = semi.line;
        let trailing = self.take_trailing(semi.line);
        Ok(BindCst {
            target,
            args,
            interior,
            map_pairs,
            as_name: (n, sp),
            line: bind_tok.line,
            span: join(bind_tok.span(), semi.span()),
            doc_run,
            trailing,
        })
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
        comments: Vec::new(),
        cpos: 0,
        prev_end_line: 0,
        machine_seen: false,
        sink: None,
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
        .0
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
        .0
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
        .0
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
        .0
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
        .0
}

/// Retokenization reuse shim for a RULE's own MOVE_VEC node:
/// re-parses it through [`Parser::move_vec`] — the same `[`-opening
/// extent rule as [`reparse_write_vec`].
pub(crate) fn reparse_move_vec(tokens: &[Token]) -> MoveVec {
    bare_parser(tokens)
        .move_vec()
        .expect("reparse_move_vec: extraction only ever runs on an already-parsed tree")
        .0
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
        .0
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
/// `DocLine`/`AttentionLine` tokens through `Parser::doc_run` itself,
/// rather than a hand-rolled reimplementation of its item loop. Sound
/// even though `doc_run` also re-enforces the `DocLineOrder` ordering
/// check and the duplicate-/unknown-attribute checks: those checks
/// already passed once, over these exact tokens, during the original
/// parse that produced the tree this run was retokenized from, so
/// re-running them is redundant work, never a new failure mode.
/// Comments interleaved in the original run do not survive: they are
/// trivia `crate::syntax::extract::sig_tokens` filters out before this
/// ever runs, so a comment-bearing run retokenizes to a strictly
/// SHORTER item sequence than the original CST's.
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
/// wrong value disagrees with `lower_cst` on exactly that one field of
/// exactly that one item. Mirrors the sibling crate's own
/// `reparse_item(tokens, in_group)`, whose `in_group` flag is the same
/// shape of caller-supplied context an isolated retokenized node
/// cannot recover on its own.
pub(crate) fn reparse_doc_items(tokens: &[Token], prev_end_line: u32) -> Vec<DocRunItem> {
    let mut p = bare_parser(tokens);
    p.prev_end_line = prev_end_line;
    p.doc_run()
        .expect("reparse_doc_items: extraction only ever runs on an already-parsed tree")
        .0
}

#[cfg(test)]
mod tests;
