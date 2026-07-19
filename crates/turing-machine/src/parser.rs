//! `.tmc` recursive-descent parser (spec's language chapter): tokens → AST,
//! via a lossless CST. The front-end mirror of the `.pmc` parser in the
//! sibling PM-1 crate, using the same `parse = lower_cst ∘ parse_cst` seam:
//! `parse_cst` builds the [`crate::cst::Cst`] (which the phase-7 fmt/LSP walk
//! directly), and `lower_cst` copies it — infallibly — into the flat
//! [`Program`] the rest of the front end consumes. Every fatal is raised by
//! `parse_cst`; `lower_cst` never fails.
//!
//! The 24 reserved keywords live in one place, [`crate::lexer::RESERVED`]; the
//! parser is the sole enforcer — it rejects a keyword wherever a name is
//! expected. `deprecated` is contextual (an attribute word) and is not in that
//! set.

use mtc_core::diagnostics::{Pos, Span};

use crate::compiler::{CompileError, CompileErrorKind};
use crate::cst::{
    AlphabetCst, AttrCst, BindCst, Cst, DocRunItem, DocRunKind, GraftCst, MachineCst, NamespaceCst,
    ReuseCarrier, ReuseCst, RuleCst, RuleItem, RuleKind, StateCst, TapeCst, TopItem, TopKind,
    UseCst, UsePath, WorldItem, WorldKind,
};
use crate::lexer::{Comment, RESERVED, Token, TokenKind};

/// The `.tmc` language acceptance-contract version (the spec's language
/// chapter). Pre-1.0 the version is `0.N` and N bumps on ANY grammar change;
/// at a declared 1.0 the axes activate (major = breaking, minor = additive).
/// There is no patch digit — spec-text corrections are errata and
/// implementation-conformance fixes never move it. This is the language's
/// first cut, so `0.1` (mirrors PM-1's `PMC_LANG_VERSION` discipline).
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigParamKind {
    Tape {
        alphabet: String,
        alphabet_span: Span,
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
    /// A substitution `{name}` (delta 0), `{name+k}`, or `{name-k}`. The
    /// binding-kind legality (arithmetic is numeric-only) is checked at parse
    /// time; the fold itself happens during range expansion.
    Subst {
        name: String,
        name_span: Span,
        delta: i64,
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

/// tokens → lossless CST. Accepts a comment-free stream (the compiler's path)
/// or a `WithComments` stream (fmt/LSP's path). Comment tokens are split off up
/// front so the grammar walk over the significant tokens is unaffected; the
/// dropped-in-lowering trivia (`blank_before`, comment nodes, `trailing`,
/// `open_trailing`/`close_trailing`, doc runs) is attached by source position.
pub fn parse_cst(tokens: &[Token]) -> Result<Cst, CompileError> {
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
    let items = Parser {
        tokens: &sig,
        pos: 0,
        comments,
        cpos: 0,
        prev_end_line: 0,
        machine_seen: false,
    }
    .file()?;
    Ok(Cst { items })
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
/// lines contribute nothing. Mirrors PM-1's `reduce_doc_run`.
fn reduce_doc_run(doc_run: &[DocRunItem]) -> Option<Doc> {
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
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn bump(&mut self) {
        if !matches!(self.tokens[self.pos].kind, TokenKind::Eof) {
            self.pos += 1;
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
                    self.bump();
                    seen_attention = true;
                    let attr = Self::parse_attr(&text, &t);
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

    fn file(mut self) -> Result<Vec<TopItem>, CompileError> {
        self.top_items(&[], None).map(|(items, _, _)| items)
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
            let doc_run = if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                let (run, first_span) = self.doc_run()?;
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
                    "use" => TopKind::Import(self.parse_use()?),
                    "alphabet" => TopKind::Alphabet(self.parse_alphabet(
                        false,
                        t.span().start,
                        t.col,
                        doc_run,
                    )?),
                    "routine" => TopKind::Reuse(self.parse_reuse(
                        ReuseCarrier::Routine,
                        false,
                        t.span().start,
                        t.col,
                        doc_run,
                    )?),
                    "graph" => TopKind::Reuse(self.parse_reuse(
                        ReuseCarrier::Graph,
                        false,
                        t.span().start,
                        t.col,
                        doc_run,
                    )?),
                    "namespace" => TopKind::Namespace(self.parse_namespace(ns, doc_run)?),
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
                        TopKind::Machine(self.parse_machine(doc_run)?)
                    }
                    "export" => {
                        let export_start = t.span().start;
                        let export_col = t.col;
                        self.bump();
                        let t2 = self.peek().clone();
                        match &t2.kind {
                            TokenKind::Ident(w2) if w2 == "alphabet" => TopKind::Alphabet(
                                self.parse_alphabet(true, export_start, export_col, doc_run)?,
                            ),
                            TokenKind::Ident(w2) if w2 == "routine" => {
                                TopKind::Reuse(self.parse_reuse(
                                    ReuseCarrier::Routine,
                                    true,
                                    export_start,
                                    export_col,
                                    doc_run,
                                )?)
                            }
                            TokenKind::Ident(w2) if w2 == "graph" => {
                                TopKind::Reuse(self.parse_reuse(
                                    ReuseCarrier::Graph,
                                    true,
                                    export_start,
                                    export_col,
                                    doc_run,
                                )?)
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
        let semi_line;
        loop {
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
            span: Span {
                start: header_start,
                end: close.span().end,
            },
            doc_run,
            open_trailing,
            close_trailing,
        })
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
        let sig = self.signature()?;
        let brace = self.expect(&TokenKind::LBrace, "`{` to open the body")?;
        let open_trailing = self.capture_open_trailing(brace.line);
        let (items, close_trailing, close_span) = self.world_body(false)?;
        Ok(ReuseCst {
            carrier,
            name,
            name_span,
            line: name_span.start.line,
            col: header_col,
            exported,
            sig,
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
        let brace = self.expect(&TokenKind::LBrace, "`{` to open the machine body")?;
        let open_trailing = self.capture_open_trailing(brace.line);
        let (items, close_trailing, close_span) = self.world_body(true)?;
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

    fn signature(&mut self) -> Result<Signature, CompileError> {
        let lp = self.expect(&TokenKind::LParen, "`(` to open the signature")?;
        let mut params: Vec<SigParam> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RParen) {
            loop {
                params.push(self.sig_param()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RParen => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `)`")),
                }
            }
        }
        let rp = self.expect(&TokenKind::RParen, "`)` to close the signature")?;
        Ok(Signature {
            params,
            span: join(lp.span(), rp.span()),
        })
    }

    fn sig_param(&mut self) -> Result<SigParam, CompileError> {
        let t = self.peek().clone();
        if self.at_kw("tape") {
            self.bump();
            let (name, name_span) = self.name("a tape parameter name")?;
            self.expect(&TokenKind::Colon, "`:` after the tape parameter name")?;
            let (alphabet, alphabet_span) = self.name("an alphabet name")?;
            Ok(SigParam {
                kind: SigParamKind::Tape {
                    alphabet,
                    alphabet_span,
                },
                name,
                name_span,
                span: join(t.span(), alphabet_span),
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
            let doc_run = if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                let (run, first_span) = self.doc_run()?;
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
                    WorldKind::State(self.parse_state(true, prefix, doc_run)?)
                } else if self.at_kw("graft") {
                    WorldKind::Graft(self.parse_graft(true, prefix, doc_run)?)
                } else {
                    return Err(Self::expected(
                        self.peek(),
                        "`state` or `graft` after `entry`",
                    ));
                }
            } else if self.at_kw("state") {
                WorldKind::State(self.parse_state(false, None, doc_run)?)
            } else if self.at_kw("graft") {
                WorldKind::Graft(self.parse_graft(false, None, doc_run)?)
            } else if self.at_kw("bind") {
                WorldKind::Bind(self.parse_bind(doc_run)?)
            } else if self.at_kw("tape") {
                if in_machine {
                    WorldKind::Tape(self.parse_tape()?)
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

    fn parse_tape(&mut self) -> Result<TapeCst, CompileError> {
        let tape_tok = self.peek().clone();
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
            line: tape_tok.line,
            span: join(tape_tok.span(), semi.span()),
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
            let rule = self.rule()?;
            let trailing = self.take_trailing(self.prev_end_line);
            let blank_before = rule_line > saved + 1;
            rules.push(RuleItem {
                blank_before,
                kind: RuleKind::Rule(Box::new(RuleCst { rule, trailing })),
            });
        }
    }

    // ---- rules ------------------------------------------------------------

    fn rule(&mut self) -> Result<Rule, CompileError> {
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
        let transition = self.transition()?;
        let semi = self.expect(&TokenKind::Semi, "`;` to end the rule")?;
        self.prev_end_line = semi.line;
        // Char arithmetic is deliberately absent: a `{c±k}` on a glyph-bound
        // pattern name is rejected here, where the rule's bindings are known.
        self.check_char_arithmetic(&pattern, &write)?;
        Ok(Rule {
            pattern: pattern.clone(),
            debugger,
            write,
            mov,
            transition,
            line: pattern.span.start.line,
            span: join(pattern.span, semi.span()),
        })
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
            if let WriteCellKind::Subst {
                name,
                name_span,
                delta,
            } = &cell.kind
                && *delta != 0
                && glyph_bound.contains(&name.as_str())
            {
                return Err(CompileError {
                    span: *name_span,
                    kind: CompileErrorKind::CharArithmetic,
                });
            }
        }
        Ok(())
    }

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

    fn write_vec(&mut self) -> Result<WriteVec, CompileError> {
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
                let (name, name_span) = self.name("a substitution binding name")?;
                let delta = match self.peek().kind {
                    TokenKind::Plus => {
                        self.bump();
                        self.subst_delta(1)?
                    }
                    TokenKind::Dash => {
                        self.bump();
                        self.subst_delta(-1)?
                    }
                    _ => 0,
                };
                let rb = self.expect(&TokenKind::RBrace, "`}` to close the substitution")?;
                Ok(WriteCell {
                    kind: WriteCellKind::Subst {
                        name,
                        name_span,
                        delta,
                    },
                    span: join(t.span(), rb.span()),
                })
            }
            _ => Err(Self::expected(
                &t,
                "a write element (glyph, number, `{binding}`, or `-`)",
            )),
        }
    }

    /// The magnitude after a substitution's `+`/`-`, signed by `sign`.
    fn subst_delta(&mut self, sign: i64) -> Result<i64, CompileError> {
        let t = self.peek().clone();
        let TokenKind::Number(n, _) = &t.kind else {
            return Err(Self::expected(&t, "a number after the substitution sign"));
        };
        let n = *n;
        self.bump();
        Ok(sign * n as i64)
    }

    fn move_vec(&mut self) -> Result<MoveVec, CompileError> {
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

    fn binding_args(&mut self) -> Result<Vec<BindingArg>, CompileError> {
        self.expect(&TokenKind::LParen, "`(` to open the binding")?;
        let mut args: Vec<BindingArg> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RParen) {
            loop {
                args.push(self.binding_arg()?);
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

    /// `map { pairs }` after a consumed `with`.
    fn sym_map(&mut self) -> Result<SymMap, CompileError> {
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

    fn parse_graft(
        &mut self,
        entry: bool,
        prefix: Option<(Pos, u32)>,
        doc_run: Vec<DocRunItem>,
    ) -> Result<GraftCst, CompileError> {
        let graft_tok = self.peek().clone();
        self.bump(); // `graft`
        let target = self.qual_name("a graft target")?;
        let args = self.binding_args()?;
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
        let args = self.binding_args()?;
        self.expect_kw("as", "`as` (a bind needs an instance name)")?;
        let (n, sp) = self.name("a bind instance name")?;
        let semi = self.expect(&TokenKind::Semi, "`;` to end the bind")?;
        self.prev_end_line = semi.line;
        let trailing = self.take_trailing(semi.line);
        Ok(BindCst {
            target,
            args,
            as_name: (n, sp),
            line: bind_tok.line,
            span: join(bind_tok.span(), semi.span()),
            doc_run,
            trailing,
        })
    }
}

#[cfg(test)]
mod tests;
