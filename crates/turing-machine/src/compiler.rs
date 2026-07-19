//! `.tmc` compiler driver and shared diagnostics — the front-end mirror of
//! the `.pmc` compiler in the sibling PM-1 crate.
//!
//! This module grows across the phase-6a tasks (parser → resolution → IR →
//! codegen → compile orchestration). Today it hosts the shared fatal type
//! every pipeline stage reports through, plus the resolution / flatten /
//! world-check stage (`analyze`) that produces the [`Resolved`] module Task 5
//! (graft + range expansion) and Task 6 (IR lowering) consume. Library code
//! never prints: fatals flow as span-carrying, coded values and the CLI is
//! the sole renderer.
//!
//! `analyze` and the [`Resolved`] surface have no in-crate caller yet — Task 7
//! wires `compile()` over them — so the resolution machinery is allowed dead
//! for now (the in-module tests exercise it); the allow drops once `compile()`
//! lands.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::lexer::{Token, lex};
use crate::parser::{
    Alphabet, AlphabetElem, Bind, BindingArg, BindingValue, Continuation, Doc, Graft, Machine,
    Program, QualName, SigParamKind, State, SymLit, Transition, parse,
};

/// Fatal compile error at a real source span (1-based, char-counted,
/// end-exclusive; see `mtc_core::diagnostics`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub span: Span,
    pub kind: CompileErrorKind,
}

/// The ways a `.tmc` compile can fail fatally. The lexer's and parser's kinds
/// exist today; resolution / IR / codegen kinds join them in the later
/// phase-6a tasks, mirroring the `.pmc` compiler's kind set. Kept as its own
/// enum (not folded into `CompileError`) so the frozen `code()` discipline
/// lives in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileErrorKind {
    /// Lexical error — an unexpected character, an unterminated block
    /// comment, or a malformed glyph literal (unterminated / empty / bad
    /// escape). The message is the human-readable detail.
    Lex(String),
    /// The parser needed one thing and saw another. `what` names what was
    /// expected; `found` describes the token actually present.
    Expected { what: &'static str, found: String },
    /// A reserved keyword used where a name is expected. `what` is the noun
    /// phrase for the position ("a state name", "an alphabet name", "a path
    /// segment", …) — the same phrase the `Expected` error would use.
    ReservedName { name: String, what: &'static str },
    /// More than one `machine { … }` block in a single file — a program has
    /// exactly one; a library file has none. (The zero-in-a-program case is a
    /// later semantic check, not a parse error.)
    MultipleMachines,
    /// A `tape … ;` declaration inside a `routine`/`graph` body: those worlds
    /// take their tapes from the signature, never from tape decls (only the
    /// `machine` block declares tapes).
    TapeNotInMachine,
    /// A rule pattern written without its enclosing `[ … ]`. Single-tape
    /// bracket-less pattern sugar is deliberately absent in 0.1 — the brackets
    /// carry the tuple semantics and keep the arity visible.
    NakedPattern,
    /// `* as v` — a wildcard cannot bind. It would silently expand the
    /// cheapest row to alphabet size; write the range explicitly so the cost
    /// is visible.
    WildcardBinding,
    /// A range whose two endpoints are not the same kind (`'a'..3`). A range
    /// is `glyph..glyph` or `number..number`; there is no count form.
    RangeKindMismatch,
    /// Arithmetic on a glyph-bound substitution (`{c+1}`). Char arithmetic is
    /// deliberately absent in 0.1; only numeric bindings fold (`{v±k}`).
    CharArithmetic,
    /// A non-`entry` `graft` with no `as name`. Only an entry graft may omit
    /// the instance name (an unreferenced unnamed instance would be dead).
    GraftNeedsName,
    /// A `state name ;` redirect form. A state always has a `{ … }` body;
    /// there is one way to mark an entry (`entry state` / `entry graft`).
    StateRedirect,
    /// A doc/attention run not immediately followed by a declaration that
    /// accepts documentation. Span = the run's first line.
    DanglingDocRun,
    /// A `?` doc line appears after the run has already entered its `!` block
    /// — interleaved, or the whole run written `!`-then-`?`.
    DocLineOrder,
    /// An attention line's leading `[ident]` names something other than the
    /// v1 attribute vocabulary (`deprecated`).
    UnknownAttribute(String),
    /// A second `[deprecated]` attribute inside one run.
    DuplicateAttribute,

    // -- resolution / flatten / world checks (this task) -------------------
    /// An alphabet with no elements — a world needs at least one symbol
    /// (index 0 is always the blank).
    EmptyAlphabet,
    /// The same glyph appears twice in one alphabet. Uniqueness is per
    /// alphabet; the `name` is the repeated glyph.
    DuplicateGlyph(String),
    /// An alphabet resolves to more than 127 symbols. The compact family
    /// caps at 127; the multi-byte symbol family is a recorded deviation —
    /// named as not-yet-implemented rather than silently selected.
    AlphabetTooLarge(usize),
    /// A glyph range (`'a'..'c'`) whose endpoint is not a single Unicode
    /// scalar — char ranges walk scalar succession and need scalar ends.
    RangeEndpointNotScalar,
    /// A range whose low endpoint exceeds its high endpoint. Ranges are
    /// inclusive both ends and ascending; there is no descending form.
    RangeDescending,
    /// Two entities (alphabet / routine / graph / namespace) share one name
    /// in one scope. `what` names the EXISTING entity's kind.
    DuplicateName { name: String, what: &'static str },
    /// Two imports bind one bare name in one scope (post-alias). The same
    /// binding in different scopes is legal (inner shadows outer).
    DuplicateBinding(String),
    /// A world declares more than 16 tapes (a `machine` block's tape decls
    /// or a signature's tape params).
    TooManyTapes(usize),
    /// A tape (or signature tape param) names an alphabet no scope resolves.
    UnresolvedAlphabet(String),
    /// Two tapes share one name in one world.
    DuplicateTape(String),
    /// Two states (or a state and a graft instance) share one name in one
    /// world.
    DuplicateState(String),
    /// Two signature parameters share one name.
    DuplicateParam(String),
    /// A world's `entry` count is not exactly one (`found` = the count).
    EntryCount(usize),
    /// A `return` transition or continuation outside a routine body.
    ReturnOutsideRoutine,
    /// `goto` (or bare-name sugar) targeting a bind name — a bind is a call
    /// target, never a state (the dedicated GC9 error).
    GotoIntoBind(String),
    /// `goto` targeting a routine or graph — a reuse target, not a state.
    GotoNotAState(String),
    /// `goto`, a continuation, or a state argument naming a name that is not
    /// a state (or graft instance) in the world.
    UndefinedState(String),
    /// A `call`/`graft`/`bind` target resolves to the wrong entity kind.
    /// `expected` is the noun phrase for the required kind.
    WrongTargetKind {
        name: String,
        expected: &'static str,
    },
    /// A `graft` target names no graph in scope. A graft needs the graph's
    /// source, so an unresolved graft target is fatal (unlike a `call`).
    UndefinedGraph(String),
    /// A binding argument names a parameter the signature does not declare.
    UnknownArg(String),
    /// Two binding arguments share one parameter name.
    DuplicateArg(String),
    /// A signature parameter has no binding argument.
    MissingArg(String),
    /// A binding argument is the wrong kind for its parameter. `expected` is
    /// the noun phrase for the required kind.
    WrongArgKind {
        name: String,
        expected: &'static str,
    },
    /// A tape-parameter argument names a target that is not a tape in the
    /// enclosing world.
    UnresolvedTapeTarget(String),
}

impl CompileErrorKind {
    /// Stable kebab-case code, one per variant. Frozen once published —
    /// these are permanent user-visible identifiers: the CLI brackets them
    /// into every fatal rendering, and the language server carries them in
    /// the LSP diagnostic `code` field. The message itself stays the kind's
    /// own `Display`, which is why the `[code]` suffix lives on
    /// [`CompileError`]'s `Display`, not here.
    pub fn code(&self) -> &'static str {
        match self {
            CompileErrorKind::Lex(_) => "lex-error",
            CompileErrorKind::Expected { .. } => "unexpected-token",
            CompileErrorKind::ReservedName { .. } => "reserved-name",
            CompileErrorKind::MultipleMachines => "multiple-machines",
            CompileErrorKind::TapeNotInMachine => "tape-not-in-machine",
            CompileErrorKind::NakedPattern => "naked-pattern",
            CompileErrorKind::WildcardBinding => "wildcard-binding",
            CompileErrorKind::RangeKindMismatch => "range-kind-mismatch",
            CompileErrorKind::CharArithmetic => "char-arithmetic",
            CompileErrorKind::GraftNeedsName => "graft-needs-name",
            CompileErrorKind::StateRedirect => "state-redirect",
            CompileErrorKind::DanglingDocRun => "dangling-doc-run",
            CompileErrorKind::DocLineOrder => "doc-line-order",
            CompileErrorKind::UnknownAttribute(_) => "unknown-attribute",
            CompileErrorKind::DuplicateAttribute => "duplicate-attribute",
            CompileErrorKind::EmptyAlphabet => "empty-alphabet",
            CompileErrorKind::DuplicateGlyph(_) => "duplicate-glyph",
            CompileErrorKind::AlphabetTooLarge(_) => "alphabet-too-large",
            CompileErrorKind::RangeEndpointNotScalar => "range-endpoint-not-scalar",
            CompileErrorKind::RangeDescending => "range-descending",
            CompileErrorKind::DuplicateName { .. } => "duplicate-name",
            CompileErrorKind::DuplicateBinding(_) => "duplicate-binding",
            CompileErrorKind::TooManyTapes(_) => "too-many-tapes",
            CompileErrorKind::UnresolvedAlphabet(_) => "unresolved-alphabet",
            CompileErrorKind::DuplicateTape(_) => "duplicate-tape",
            CompileErrorKind::DuplicateState(_) => "duplicate-state",
            CompileErrorKind::DuplicateParam(_) => "duplicate-param",
            CompileErrorKind::EntryCount(_) => "entry-count",
            CompileErrorKind::ReturnOutsideRoutine => "return-outside-routine",
            CompileErrorKind::GotoIntoBind(_) => "goto-into-bind",
            CompileErrorKind::GotoNotAState(_) => "goto-not-a-state",
            CompileErrorKind::UndefinedState(_) => "undefined-state",
            CompileErrorKind::WrongTargetKind { .. } => "wrong-target-kind",
            CompileErrorKind::UndefinedGraph(_) => "undefined-graph",
            CompileErrorKind::UnknownArg(_) => "unknown-arg",
            CompileErrorKind::DuplicateArg(_) => "duplicate-arg",
            CompileErrorKind::MissingArg(_) => "missing-arg",
            CompileErrorKind::WrongArgKind { .. } => "wrong-arg-kind",
            CompileErrorKind::UnresolvedTapeTarget(_) => "unresolved-tape-target",
        }
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "line {}:{}: {} [{}]",
            self.span.start.line,
            self.span.start.col,
            self.kind,
            self.kind.code()
        )
    }
}

impl std::fmt::Display for CompileErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileErrorKind::Lex(m) => write!(f, "{m}"),
            CompileErrorKind::Expected { what, found } => {
                write!(f, "expected {what}, found {found}")
            }
            CompileErrorKind::ReservedName { name, what } => {
                write!(
                    f,
                    "`{name}` is a reserved keyword and cannot be used as {what}"
                )
            }
            CompileErrorKind::MultipleMachines => {
                write!(
                    f,
                    "a file has at most one `machine` block — a program has one, a library has none"
                )
            }
            CompileErrorKind::TapeNotInMachine => {
                write!(
                    f,
                    "a `tape` declaration is only allowed in a `machine` block — routines and graphs take their tapes from the signature"
                )
            }
            CompileErrorKind::NakedPattern => {
                write!(
                    f,
                    "a rule pattern must be bracketed (`[ … ]`) — bare single-tape patterns are not supported"
                )
            }
            CompileErrorKind::WildcardBinding => {
                write!(
                    f,
                    "`* as v` is not allowed — bind an explicit range so the expansion cost is visible"
                )
            }
            CompileErrorKind::RangeKindMismatch => {
                write!(
                    f,
                    "a range must be `glyph..glyph` or `number..number` — mixed endpoints and the count form (`'a'..3`) are not supported"
                )
            }
            CompileErrorKind::CharArithmetic => {
                write!(
                    f,
                    "arithmetic on a glyph binding is not supported — only numeric bindings fold (`{{v+1}}` / `{{v-1}}`)"
                )
            }
            CompileErrorKind::GraftNeedsName => {
                write!(
                    f,
                    "a non-entry `graft` needs an `as name` — only an `entry graft` may omit it"
                )
            }
            CompileErrorKind::StateRedirect => {
                write!(
                    f,
                    "a state has a `{{ … }}` body — the `state name;` redirect form is not supported"
                )
            }
            CompileErrorKind::DanglingDocRun => {
                write!(f, "doc/attention run is not attached to a declaration")
            }
            CompileErrorKind::DocLineOrder => {
                write!(
                    f,
                    "doc lines (`?`) must come before attention lines (`!`) in a run"
                )
            }
            CompileErrorKind::UnknownAttribute(name) => {
                write!(
                    f,
                    "unknown attribute `[{name}]` — the only recognized attribute is `[deprecated]`"
                )
            }
            CompileErrorKind::DuplicateAttribute => {
                write!(f, "duplicate `[deprecated]` attribute in the same run")
            }
            CompileErrorKind::EmptyAlphabet => {
                write!(f, "an alphabet needs at least one symbol")
            }
            CompileErrorKind::DuplicateGlyph(g) => {
                write!(f, "duplicate glyph `{g}` in the alphabet")
            }
            CompileErrorKind::AlphabetTooLarge(n) => {
                write!(
                    f,
                    "alphabet resolves to {n} symbols — more than 127 needs the multi-byte symbol family, which is not yet implemented"
                )
            }
            CompileErrorKind::RangeEndpointNotScalar => {
                write!(
                    f,
                    "a glyph range endpoint must be a single Unicode scalar (`'a'..'c'`)"
                )
            }
            CompileErrorKind::RangeDescending => {
                write!(
                    f,
                    "a range must ascend — its low endpoint cannot exceed its high endpoint"
                )
            }
            CompileErrorKind::DuplicateName { name, what } => {
                write!(
                    f,
                    "duplicate name `{name}` — already used by {what} in this scope"
                )
            }
            CompileErrorKind::DuplicateBinding(n) => {
                write!(
                    f,
                    "`{n}` is bound twice — qualify the reference (`ns::{n}`) or disambiguate with `as`"
                )
            }
            CompileErrorKind::TooManyTapes(n) => {
                write!(f, "{n} tapes — a world has at most 16")
            }
            CompileErrorKind::UnresolvedAlphabet(n) => {
                write!(f, "unknown alphabet `{n}`")
            }
            CompileErrorKind::DuplicateTape(n) => {
                write!(f, "duplicate tape `{n}` in this world")
            }
            CompileErrorKind::DuplicateState(n) => {
                write!(f, "duplicate state `{n}` in this world")
            }
            CompileErrorKind::DuplicateParam(n) => {
                write!(f, "duplicate signature parameter `{n}`")
            }
            CompileErrorKind::EntryCount(found) => {
                if *found == 0 {
                    write!(
                        f,
                        "this world has no entry — mark exactly one `entry state` or `entry graft`"
                    )
                } else {
                    write!(
                        f,
                        "this world has {found} entries — exactly one `entry` is allowed"
                    )
                }
            }
            CompileErrorKind::ReturnOutsideRoutine => {
                write!(f, "`return` is only allowed inside a routine")
            }
            CompileErrorKind::GotoIntoBind(n) => {
                write!(
                    f,
                    "`goto {n}` targets a bind — a bind is a call target, not a state"
                )
            }
            CompileErrorKind::GotoNotAState(n) => {
                write!(
                    f,
                    "`goto {n}` targets a routine or graph, not a state in this world"
                )
            }
            CompileErrorKind::UndefinedState(n) => {
                write!(f, "`{n}` is not a state in this world")
            }
            CompileErrorKind::WrongTargetKind { name, expected } => {
                write!(f, "`{name}` is not {expected}")
            }
            CompileErrorKind::UndefinedGraph(n) => {
                write!(f, "unknown graph `{n}` — a graft needs the graph's source")
            }
            CompileErrorKind::UnknownArg(n) => {
                write!(f, "`{n}` is not a parameter of this signature")
            }
            CompileErrorKind::DuplicateArg(n) => {
                write!(f, "duplicate binding argument `{n}`")
            }
            CompileErrorKind::MissingArg(n) => {
                write!(f, "missing binding argument for parameter `{n}`")
            }
            CompileErrorKind::WrongArgKind { name, expected } => {
                write!(f, "binding argument `{name}` must be {expected}")
            }
            CompileErrorKind::UnresolvedTapeTarget(n) => {
                write!(f, "`{n}` is not a tape in this world")
            }
        }
    }
}

impl std::error::Error for CompileError {}

// ---------------------------------------------------------------------------
// Alphabet resolution — elements to glyph vectors (docs/language.md
// (alphabets), once that page lands; substance in prose until then).
// ---------------------------------------------------------------------------

/// A resolved alphabet: its glyphs in position order (index = position; index
/// 0 is always the blank, whatever its glyph). Range elements are expanded;
/// the vector is unique and at most 127 long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedAlphabet {
    /// Mangled name (namespace `::` path); a key into `Resolved.alphabets`.
    pub name: String,
    pub name_span: Span,
    /// Glyph labels in position order; `glyphs[0]` is the blank.
    pub glyphs: Vec<String>,
}

impl ResolvedAlphabet {
    pub fn cardinality(&self) -> usize {
        self.glyphs.len()
    }
}

/// Resolve one alphabet's elements into its glyph vector, or fail with the
/// first offending element's span. Char ranges expand by scalar succession
/// (single-scalar endpoints required); numeric ranges mint decimal-string
/// glyphs of each value; glyphs are unique; blank is position 0 by
/// construction (the first element); an empty alphabet or one resolving to
/// more than 127 symbols is rejected.
fn resolve_alphabet_glyphs(a: &Alphabet) -> Result<Vec<String>, CompileError> {
    if a.elems.is_empty() {
        return Err(CompileError {
            span: a.name_span,
            kind: CompileErrorKind::EmptyAlphabet,
        });
    }
    let mut glyphs: Vec<String> = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for elem in &a.elems {
        match elem {
            AlphabetElem::Single(s) => {
                push_glyph(&mut glyphs, &mut seen, glyph_label(s), s.span())?;
            }
            AlphabetElem::Range { lo, hi, span } => {
                for label in expand_range(lo, hi, *span)? {
                    push_glyph(&mut glyphs, &mut seen, label, *span)?;
                }
            }
        }
    }
    if glyphs.len() > 127 {
        return Err(CompileError {
            span: a.name_span,
            kind: CompileErrorKind::AlphabetTooLarge(glyphs.len()),
        });
    }
    Ok(glyphs)
}

/// The glyph label a single symbol literal contributes. Numeric literals mint
/// the decimal string of their VALUE (`05` and `5` both label `"5"`) — a
/// numeric glyph's identity is its value, per the spec's numeric-range rule.
fn glyph_label(s: &SymLit) -> String {
    match s {
        SymLit::Glyph { value, .. } => value.clone(),
        SymLit::Number { value, .. } => value.to_string(),
    }
}

/// Expand a range element into its glyph labels. Glyph ranges require
/// single-scalar endpoints and walk Unicode scalar succession; numeric ranges
/// mint each value's decimal string. Both are inclusive and ascending.
fn expand_range(lo: &SymLit, hi: &SymLit, span: Span) -> Result<Vec<String>, CompileError> {
    match (lo, hi) {
        (SymLit::Number { value: l, .. }, SymLit::Number { value: h, .. }) => {
            if l > h {
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::RangeDescending,
                });
            }
            Ok((*l..=*h).map(|v| v.to_string()).collect())
        }
        (SymLit::Glyph { value: l, .. }, SymLit::Glyph { value: h, .. }) => {
            let (Some(lc), Some(hc)) = (single_scalar(l), single_scalar(h)) else {
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::RangeEndpointNotScalar,
                });
            };
            if lc as u32 > hc as u32 {
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::RangeDescending,
                });
            }
            // Scalar succession: iterate code points, skipping the surrogate
            // gap (never a valid `char`). Endpoints being valid scalars, only
            // an oversized range crosses it — caught by the 127 cap.
            Ok((lc as u32..=hc as u32)
                .filter_map(char::from_u32)
                .map(|c| c.to_string())
                .collect())
        }
        // Mixed-kind endpoints are a parse-time `RangeKindMismatch`; this arm
        // is unreachable from parsed input.
        _ => Err(CompileError {
            span,
            kind: CompileErrorKind::RangeEndpointNotScalar,
        }),
    }
}

/// The single Unicode scalar of a glyph string, or `None` if it is not exactly
/// one scalar (empty or multi-scalar — the latter legal as a standalone glyph
/// but not as a range endpoint).
fn single_scalar(g: &str) -> Option<char> {
    let mut chars = g.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        Some(first)
    } else {
        None
    }
}

/// Append a glyph label, rejecting a repeat at `span`.
fn push_glyph(
    glyphs: &mut Vec<String>,
    seen: &mut HashMap<String, ()>,
    label: String,
    span: Span,
) -> Result<(), CompileError> {
    if seen.insert(label.clone(), ()).is_some() {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::DuplicateGlyph(label),
        });
    }
    glyphs.push(label);
    Ok(())
}

// ---------------------------------------------------------------------------
// The resolved module — the front-end structure Task 5 (graft + range
// expansion) and Task 6 (IR lowering) consume.
// ---------------------------------------------------------------------------

/// The whole resolved module. Rules stay in SOURCE form (patterns unexpanded
/// — Task 5 owns expansion); every span is preserved. Cross-world references
/// (`call`/`graft`/`bind` targets, tape alphabets) are resolved to mangled
/// names; the worlds carry the rest verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    /// Resolved alphabets, keyed by mangled name → glyph vector.
    pub alphabets: HashMap<String, ResolvedAlphabet>,
    /// Every world in source order: routines, graphs, then the machine
    /// (a program's entry) last if present.
    pub worlds: Vec<ResolvedWorld>,
    /// Index into `worlds` of the `machine` block, or `None` for a library.
    pub entry_world: Option<usize>,
    /// Doc runs keyed by the mangled name of a top-level alphabet / routine /
    /// graph (the `Analysis.docs` analog; hover + `deprecated-*` lint read
    /// it). World-local state / graft / bind docs ride on the worlds' AST
    /// nodes, not here.
    pub docs: HashMap<String, Doc>,
}

/// One resolved world (a `machine` block, a `routine`, or a `graph`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedWorld {
    pub kind: WorldKind,
    /// Mangled name — `main` for the machine (the linker's default entry;
    /// a program may not also define a top-level `main` routine/graph),
    /// `ns::name` for a routine/graph.
    pub name: String,
    pub name_span: Span,
    pub exported: bool,
    pub local: bool,
    /// Tape table in vector-position order (machine tape decls, or a
    /// routine/graph signature's tape params).
    pub tapes: Vec<ResolvedTape>,
    /// State-parameter names (routine/graph), in signature order — valid
    /// goto / continuation targets inside the body.
    pub state_params: Vec<String>,
    /// States, rules in SOURCE form.
    pub states: Vec<State>,
    /// Graft instances declared in this world.
    pub grafts: Vec<ResolvedGraft>,
    /// Bind instances declared in this world.
    pub binds: Vec<ResolvedBind>,
    /// The entry state / graft-instance name; `None` for an unnamed entry
    /// graft (Task 5 names it the spliced entry state) or a library-world
    /// with an entry that carries no addressable name.
    pub entry: Option<String>,
    /// Resolved `call` transitions in this world's rules, in source order.
    pub calls: Vec<ResolvedCall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorldKind {
    Machine,
    Routine,
    Graph,
}

/// A resolved tape: its world-local name plus the mangled alphabet it draws
/// from and that alphabet's cardinality (for index resolution in Task 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTape {
    pub name: String,
    pub name_span: Span,
    /// Mangled alphabet name (a key into `Resolved.alphabets`).
    pub alphabet: String,
    pub cardinality: usize,
    pub span: Span,
}

/// A resolved graft declaration: the mangled graph target plus the raw
/// (source-form) binding args Task 5 applies at splice time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedGraft {
    pub entry: bool,
    /// Mangled graph name (always a locally-defined graph — a graft needs
    /// the source).
    pub target: String,
    pub target_span: Span,
    pub as_name: Option<String>,
    pub args: Vec<BindingArg>,
    pub span: Span,
}

/// A resolved bind declaration: a named bound-call target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedBind {
    /// The bind instance name (world-local).
    pub name: String,
    /// Mangled routine name; `external` when the routine is not locally
    /// defined (resolved at link).
    pub target: String,
    pub external: bool,
    pub target_span: Span,
    pub args: Vec<BindingArg>,
    pub span: Span,
}

/// A resolved `call` transition inside a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCall {
    pub span: Span,
    pub target: ResolvedCallTarget,
    pub then: Continuation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedCallTarget {
    /// A direct routine call carrying its own binding args (source form).
    Routine {
        name: String,
        external: bool,
        args: Vec<BindingArg>,
    },
    /// A call on a world-local bind name (the bind carries the binding).
    Bind { name: String },
}

/// The front half of the pipeline: everything Task 5/6 (and a future lint /
/// LSP) needs. Mirrors the `.pmc` compiler's `AnalysisOutput` shape.
#[derive(Debug)]
pub(crate) struct Analysis {
    pub tokens: Vec<Token>,
    pub program: Program,
    pub resolved: Resolved,
    pub diagnostics: Vec<Diagnostic>,
}

/// lex → parse → duplicate-binding check → resolve alphabets → flatten +
/// world checks. The `.tmc` analog of the `.pmc` compiler's `analyze`; Task 7
/// composes it with codegen. Fatals stop at the first offending span; non-
/// fatal findings (undeclared external, unused import) accumulate as
/// diagnostics.
pub(crate) fn analyze(source: &str) -> Result<Analysis, CompileError> {
    let tokens = lex(source)?;
    let program = parse(&tokens)?;
    check_duplicate_bindings(&program)?;
    let scopes = Scopes::build(&program)?;
    let alphabets = resolve_all_alphabets(&program, &scopes)?;
    let resolved = resolve_module(&program, &scopes, alphabets)?;
    let mut ctx = WorldCtx {
        scopes: &scopes,
        resolved: &resolved,
        imports_used: vec![false; program.imports.len()],
        warned_undeclared: HashSet::new(),
        diagnostics: Vec::new(),
    };
    ctx.check_worlds(&program, &resolved)?;
    let WorldCtx {
        imports_used,
        mut diagnostics,
        ..
    } = ctx;
    unused_import_warnings(&program, &imports_used, &mut diagnostics);
    Ok(Analysis {
        tokens,
        program,
        resolved,
        diagnostics,
    })
}

/// Two imports binding one bare name in one scope collide — the `.pmc`
/// duplicate-binding check verbatim, keyed on `(ns, binding name)` after
/// aliasing; an exactly-duplicate `use` is tolerated (surfaces later as an
/// unused-import warning).
fn check_duplicate_bindings(program: &Program) -> Result<(), CompileError> {
    let mut seen: HashMap<(&[String], &str), &crate::parser::Import> = HashMap::new();
    for import in &program.imports {
        match seen.entry((import.ns.as_slice(), import.binding())) {
            std::collections::hash_map::Entry::Occupied(prev) => {
                let p = prev.get();
                if p.path != import.path || p.alias != import.alias {
                    return Err(CompileError {
                        span: import.span,
                        kind: CompileErrorKind::DuplicateBinding(import.binding().to_string()),
                    });
                }
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(import);
            }
        }
    }
    Ok(())
}

/// The full symbol name of a top-level entity: namespaces join with `::`; an
/// un-namespaced name has none. Mirrors the `.pmc` `full_name` formula.
pub(crate) fn full_name(ns: &[String], name: &str) -> String {
    if ns.is_empty() {
        name.to_string()
    } else {
        format!("{}::{}", ns.join("::"), name)
    }
}

/// The kind of a top-level referenceable entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefKind {
    Alphabet,
    Routine,
    Graph,
}

impl DefKind {
    fn noun(self) -> &'static str {
        match self {
            DefKind::Alphabet => "an alphabet",
            DefKind::Routine => "a routine",
            DefKind::Graph => "a graph",
        }
    }
}

/// A signature parameter's kind, for binding-argument checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParamKind {
    Tape,
    State,
}

struct SigInfo {
    /// Parameters in signature order: `(name, kind)`.
    params: Vec<(String, ParamKind)>,
}

/// Per-scope definition + import maps, the mangled-name index, and the
/// signature table — the immutable resolution substrate, plus the
/// duplicate-name check done while building it.
struct Scopes {
    /// ns-path → (bare name → def entry).
    defs: HashMap<Vec<String>, HashMap<String, DefEntry>>,
    /// ns-path → (bare name → (import index, full `::` path)).
    bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>>,
    /// Mangled name → kind (for absolute / imported reference kinding).
    by_full: HashMap<String, DefKind>,
    /// Mangled name → signature (routines and graphs).
    sigs: HashMap<String, SigInfo>,
}

struct DefEntry {
    full: String,
    kind: DefKind,
}

impl Scopes {
    fn build(program: &Program) -> Result<Scopes, CompileError> {
        // Collect every top-level entity as (ns, name, kind, name_span).
        struct Ent<'a> {
            ns: &'a [String],
            name: &'a str,
            kind: DefKind,
            name_span: Span,
        }
        let mut ents: Vec<Ent> = Vec::new();
        for a in &program.alphabets {
            ents.push(Ent {
                ns: &a.ns,
                name: &a.name,
                kind: DefKind::Alphabet,
                name_span: a.name_span,
            });
        }
        for r in &program.routines {
            ents.push(Ent {
                ns: &r.ns,
                name: &r.name,
                kind: DefKind::Routine,
                name_span: r.name_span,
            });
        }
        for g in &program.graphs {
            ents.push(Ent {
                ns: &g.ns,
                name: &g.name,
                kind: DefKind::Graph,
                name_span: g.name_span,
            });
        }

        // Child-namespace names per scope, derived from entity ns-paths (an
        // entity at ns = S ++ [child, …] proves `child` is a namespace in S).
        let mut child_ns: HashMap<Vec<String>, HashSet<String>> = HashMap::new();
        for e in &ents {
            for k in 0..e.ns.len() {
                child_ns
                    .entry(e.ns[..k].to_vec())
                    .or_default()
                    .insert(e.ns[k].clone());
            }
        }

        let mut defs: HashMap<Vec<String>, HashMap<String, DefEntry>> = HashMap::new();
        for e in &ents {
            let scope = defs.entry(e.ns.to_vec()).or_default();
            if scope.contains_key(e.name) {
                let existing = &scope[e.name];
                return Err(CompileError {
                    span: e.name_span,
                    kind: CompileErrorKind::DuplicateName {
                        name: e.name.to_string(),
                        what: existing.kind.noun(),
                    },
                });
            }
            // An entity name colliding with a child namespace of the same
            // scope (namespace-vs-namespace merges, so is not checked here).
            if child_ns.get(e.ns).is_some_and(|s| s.contains(e.name)) {
                return Err(CompileError {
                    span: e.name_span,
                    kind: CompileErrorKind::DuplicateName {
                        name: e.name.to_string(),
                        what: "a namespace",
                    },
                });
            }
            scope.insert(
                e.name.to_string(),
                DefEntry {
                    full: full_name(e.ns, e.name),
                    kind: e.kind,
                },
            );
        }

        // A program's machine world mangles to `main` (the linker's default
        // entry); a top-level `main` routine/graph would clash.
        if program.machine.is_some()
            && let Some(clash) = defs.get(&Vec::new()).and_then(|s| s.get("main"))
        {
            return Err(CompileError {
                span: program
                    .machine
                    .as_ref()
                    .map(|m| Span::point(m.line, m.col))
                    .unwrap_or_else(|| Span::point(1, 1)),
                kind: CompileErrorKind::DuplicateName {
                    name: "main".to_string(),
                    what: clash.kind.noun(),
                },
            });
        }

        let mut by_full: HashMap<String, DefKind> = HashMap::new();
        for scope in defs.values() {
            for e in scope.values() {
                by_full.insert(e.full.clone(), e.kind);
            }
        }

        let mut bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>> = HashMap::new();
        for (i, imp) in program.imports.iter().enumerate() {
            // First-wins (exact duplicates warn as unused), mirroring `.pmc`.
            bindings
                .entry(imp.ns.clone())
                .or_default()
                .entry(imp.binding().to_string())
                .or_insert_with(|| (i, imp.full_path()));
        }

        let mut sigs: HashMap<String, SigInfo> = HashMap::new();
        for r in &program.routines {
            sigs.insert(full_name(&r.ns, &r.name), sig_info(&r.sig));
        }
        for g in &program.graphs {
            sigs.insert(full_name(&g.ns, &g.name), sig_info(&g.sig));
        }

        Ok(Scopes {
            defs,
            bindings,
            by_full,
            sigs,
        })
    }
}

fn sig_info(sig: &crate::parser::Signature) -> SigInfo {
    SigInfo {
        params: sig
            .params
            .iter()
            .map(|p| {
                let kind = match p.kind {
                    SigParamKind::Tape { .. } => ParamKind::Tape,
                    SigParamKind::State => ParamKind::State,
                };
                (p.name.clone(), kind)
            })
            .collect(),
    }
}

/// One reference's resolution: its mangled full name, the local kind (if the
/// module defines it), and the import index it went through (if any).
struct RefResolution {
    full: String,
    kind: Option<DefKind>,
    via_import: Option<usize>,
}

impl Scopes {
    /// Resolve a bare or qualified reference from namespace context `ns`. A
    /// name containing `::` is ABSOLUTE (verbatim, self-declaring, no scope
    /// walk, no import consumption); a bare name walks the scope chain
    /// innermost-out (each level's defs then its import bindings). `None` =
    /// a total miss (a bare name nothing resolves).
    fn resolve(&self, name: &str, ns: &[String]) -> Option<RefResolution> {
        if name.contains("::") {
            return Some(RefResolution {
                full: name.to_string(),
                kind: self.by_full.get(name).copied(),
                via_import: None,
            });
        }
        for k in (0..=ns.len()).rev() {
            let prefix = &ns[..k];
            if let Some(e) = self.defs.get(prefix).and_then(|d| d.get(name)) {
                return Some(RefResolution {
                    full: e.full.clone(),
                    kind: Some(e.kind),
                    via_import: None,
                });
            }
            if let Some((idx, full)) = self.bindings.get(prefix).and_then(|b| b.get(name)) {
                return Some(RefResolution {
                    full: full.clone(),
                    kind: self.by_full.get(full).copied(),
                    via_import: Some(*idx),
                });
            }
        }
        None
    }
}

/// Resolve every alphabet's glyph vector, keyed by mangled name.
fn resolve_all_alphabets(
    program: &Program,
    _scopes: &Scopes,
) -> Result<HashMap<String, ResolvedAlphabet>, CompileError> {
    let mut out = HashMap::new();
    for a in &program.alphabets {
        let glyphs = resolve_alphabet_glyphs(a)?;
        let full = full_name(&a.ns, &a.name);
        out.insert(
            full.clone(),
            ResolvedAlphabet {
                name: full,
                name_span: a.name_span,
                glyphs,
            },
        );
    }
    Ok(out)
}

/// Build the resolved worlds (structure only; the cross-world checks run in a
/// second pass with the mutable diagnostic context). Docs are collected here.
fn resolve_module(
    program: &Program,
    scopes: &Scopes,
    alphabets: HashMap<String, ResolvedAlphabet>,
) -> Result<Resolved, CompileError> {
    let mut docs: HashMap<String, Doc> = HashMap::new();
    for a in &program.alphabets {
        if let Some(d) = &a.doc {
            docs.insert(full_name(&a.ns, &a.name), d.clone());
        }
    }
    for r in &program.routines {
        if let Some(d) = &r.doc {
            docs.insert(full_name(&r.ns, &r.name), d.clone());
        }
    }
    for g in &program.graphs {
        if let Some(d) = &g.doc {
            docs.insert(full_name(&g.ns, &g.name), d.clone());
        }
    }

    let mut worlds: Vec<ResolvedWorld> = Vec::new();
    for r in &program.routines {
        worlds.push(resolve_world(
            WorldKind::Routine,
            full_name(&r.ns, &r.name),
            r.name_span,
            r.exported,
            &r.ns,
            &r.sig,
            &[],
            &r.states,
            &r.grafts,
            &r.binds,
            scopes,
            &alphabets,
        )?);
    }
    for g in &program.graphs {
        worlds.push(resolve_world(
            WorldKind::Graph,
            full_name(&g.ns, &g.name),
            g.name_span,
            g.exported,
            &g.ns,
            &g.sig,
            &[],
            &g.states,
            &g.grafts,
            &g.binds,
            scopes,
            &alphabets,
        )?);
    }
    let mut entry_world = None;
    if let Some(m) = &program.machine {
        entry_world = Some(worlds.len());
        worlds.push(resolve_machine_world(m, scopes, &alphabets)?);
    }

    Ok(Resolved {
        alphabets,
        worlds,
        entry_world,
        docs,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_world(
    kind: WorldKind,
    name: String,
    name_span: Span,
    exported: bool,
    ns: &[String],
    sig: &crate::parser::Signature,
    machine_tapes: &[crate::parser::TapeDecl],
    states: &[State],
    grafts: &[Graft],
    binds: &[Bind],
    scopes: &Scopes,
    alphabets: &HashMap<String, ResolvedAlphabet>,
) -> Result<ResolvedWorld, CompileError> {
    let _ = machine_tapes;
    // Tapes: from the signature's tape params (routine/graph).
    let mut tapes: Vec<ResolvedTape> = Vec::new();
    let mut state_params: Vec<String> = Vec::new();
    for p in &sig.params {
        match &p.kind {
            SigParamKind::Tape { alphabet, .. } => {
                let (full, card) =
                    resolve_tape_alphabet(alphabet, p.name_span, ns, scopes, alphabets)?;
                tapes.push(ResolvedTape {
                    name: p.name.clone(),
                    name_span: p.name_span,
                    alphabet: full,
                    cardinality: card,
                    span: p.span,
                });
            }
            SigParamKind::State => state_params.push(p.name.clone()),
        }
    }
    let (grafts, binds, entry) = resolve_world_reuse(grafts, binds, states, ns, scopes)?;
    Ok(ResolvedWorld {
        kind,
        name,
        name_span,
        exported,
        local: !exported,
        tapes,
        state_params,
        states: states.to_vec(),
        grafts,
        binds,
        entry,
        calls: Vec::new(),
    })
}

fn resolve_machine_world(
    m: &Machine,
    scopes: &Scopes,
    alphabets: &HashMap<String, ResolvedAlphabet>,
) -> Result<ResolvedWorld, CompileError> {
    let mut tapes: Vec<ResolvedTape> = Vec::new();
    for t in &m.tapes {
        let (full, card) =
            resolve_tape_alphabet(&t.alphabet, t.alphabet_span, &[], scopes, alphabets)?;
        tapes.push(ResolvedTape {
            name: t.name.clone(),
            name_span: t.name_span,
            alphabet: full,
            cardinality: card,
            span: t.span,
        });
    }
    let (grafts, binds, entry) = resolve_world_reuse(&m.grafts, &m.binds, &m.states, &[], scopes)?;
    Ok(ResolvedWorld {
        kind: WorldKind::Machine,
        name: "main".to_string(),
        name_span: Span::point(m.line, m.col),
        exported: true,
        local: false,
        tapes,
        state_params: Vec::new(),
        states: m.states.to_vec(),
        grafts,
        binds,
        entry,
        calls: Vec::new(),
    })
}

/// Resolve a tape's alphabet reference to `(mangled name, cardinality)`. A
/// tape alphabet must resolve to a LOCAL alphabet (its cardinality is needed
/// for index resolution — external alphabets are unsupported in 0.1).
fn resolve_tape_alphabet(
    alphabet: &str,
    span: Span,
    ns: &[String],
    scopes: &Scopes,
    alphabets: &HashMap<String, ResolvedAlphabet>,
) -> Result<(String, usize), CompileError> {
    match scopes.resolve(alphabet, ns) {
        Some(r) if r.kind == Some(DefKind::Alphabet) => {
            let card = alphabets
                .get(&r.full)
                .map(ResolvedAlphabet::cardinality)
                .expect("a locally-defined alphabet was resolved");
            Ok((r.full, card))
        }
        _ => Err(CompileError {
            span,
            kind: CompileErrorKind::UnresolvedAlphabet(alphabet.to_string()),
        }),
    }
}

/// Resolve a world's graft targets (to mangled graph names) and bind targets
/// (to mangled routine names), and compute the entry name. Target-KIND and
/// arg checks run later in `check_worlds` (this pass only wires the
/// structure); an unresolved graft target is fatal here (a graft needs the
/// graph's source).
type WorldReuse = (Vec<ResolvedGraft>, Vec<ResolvedBind>, Option<String>);

fn resolve_world_reuse(
    grafts: &[Graft],
    binds: &[Bind],
    states: &[State],
    ns: &[String],
    scopes: &Scopes,
) -> Result<WorldReuse, CompileError> {
    let mut rgrafts = Vec::new();
    for g in grafts {
        let joined = g.target.joined();
        let target = match scopes.resolve(&joined, ns) {
            Some(r) if r.kind == Some(DefKind::Graph) => r.full,
            // A resolved-but-wrong-kind target (a routine/alphabet) — the
            // same distinction `call` draws.
            Some(r) if r.kind.is_some() => {
                return Err(CompileError {
                    span: g.target.span,
                    kind: CompileErrorKind::WrongTargetKind {
                        name: joined,
                        expected: "a graph",
                    },
                });
            }
            // Unresolved or external — a graft needs the graph's source.
            _ => {
                return Err(CompileError {
                    span: g.target.span,
                    kind: CompileErrorKind::UndefinedGraph(joined),
                });
            }
        };
        rgrafts.push(ResolvedGraft {
            entry: g.entry,
            target,
            target_span: g.target.span,
            as_name: g.as_name.as_ref().map(|i| i.name.clone()),
            args: g.args.clone(),
            span: g.span,
        });
    }
    let mut rbinds = Vec::new();
    for b in binds {
        let joined = b.target.joined();
        let (target, external) = match scopes.resolve(&joined, ns) {
            Some(r) if r.kind == Some(DefKind::Routine) => (r.full, false),
            Some(r) if r.kind.is_none() => (r.full, true),
            _ => (joined.clone(), true),
        };
        rbinds.push(ResolvedBind {
            name: b.as_name.name.clone(),
            target,
            external,
            target_span: b.target.span,
            args: b.args.clone(),
            span: b.span,
        });
    }
    // Entry name: the entry state's name, or the entry graft's instance name.
    let mut entry = None;
    for s in states {
        if s.entry {
            entry = Some(s.name.clone());
        }
    }
    for g in grafts {
        if g.entry {
            entry = g.as_name.as_ref().map(|i| i.name.clone());
        }
    }
    Ok((rgrafts, rbinds, entry))
}

/// Warn for imports whose binding resolved nothing.
fn unused_import_warnings(program: &Program, used: &[bool], diagnostics: &mut Vec<Diagnostic>) {
    for (i, imp) in program.imports.iter().enumerate() {
        if !used[i] {
            diagnostics.push(Diagnostic {
                code: "unused-import",
                span: imp.span,
                message: format!("unused import `{}`", imp.full_path()),
                fix: None,
            });
        }
    }
}

/// The mutable context threaded through the world-boundary checks.
struct WorldCtx<'a> {
    scopes: &'a Scopes,
    resolved: &'a Resolved,
    imports_used: Vec<bool>,
    warned_undeclared: HashSet<String>,
    diagnostics: Vec<Diagnostic>,
}

impl WorldCtx<'_> {
    /// Run every per-world check across all worlds, in source order.
    fn check_worlds(&mut self, program: &Program, resolved: &Resolved) -> Result<(), CompileError> {
        // Re-mark import usage for tape alphabets resolved during
        // `resolve_module` (that pass had no mutable context).
        self.mark_tape_alphabet_imports(program);
        for (idx, world) in resolved.worlds.iter().enumerate() {
            let is_routine = world.kind == WorldKind::Routine;
            // Signature params first: a duplicate tape PARAM is reported as
            // `duplicate-param` (its source), not as the `duplicate-tape` it
            // would also manifest as once params become the tape table.
            self.check_signature_params(program, world, idx)?;
            self.check_tape_count(world)?;
            self.check_duplicate_tapes(world)?;
            self.check_duplicate_states(world)?;
            self.check_entry(world)?;
            self.check_rules(world, is_routine)?;
            self.check_reuse_targets(world)?;
        }
        Ok(())
    }

    fn mark_tape_alphabet_imports(&mut self, program: &Program) {
        let mark = |alphabet: &str, ns: &[String], ctx: &mut Self| {
            if let Some(r) = ctx.scopes.resolve(alphabet, ns)
                && let Some(idx) = r.via_import
            {
                ctx.imports_used[idx] = true;
            }
        };
        for r in &program.routines {
            for p in &r.sig.params {
                if let SigParamKind::Tape { alphabet, .. } = &p.kind {
                    mark(alphabet, &r.ns, self);
                }
            }
        }
        for g in &program.graphs {
            for p in &g.sig.params {
                if let SigParamKind::Tape { alphabet, .. } = &p.kind {
                    mark(alphabet, &g.ns, self);
                }
            }
        }
        if let Some(m) = &program.machine {
            for t in &m.tapes {
                mark(&t.alphabet, &[], self);
            }
        }
    }

    fn check_tape_count(&self, world: &ResolvedWorld) -> Result<(), CompileError> {
        if world.tapes.len() > 16 {
            let span = world.tapes[16].span;
            return Err(CompileError {
                span,
                kind: CompileErrorKind::TooManyTapes(world.tapes.len()),
            });
        }
        Ok(())
    }

    fn check_duplicate_tapes(&self, world: &ResolvedWorld) -> Result<(), CompileError> {
        let mut seen: HashSet<&str> = HashSet::new();
        for t in &world.tapes {
            if !seen.insert(&t.name) {
                return Err(CompileError {
                    span: t.name_span,
                    kind: CompileErrorKind::DuplicateTape(t.name.clone()),
                });
            }
        }
        Ok(())
    }

    /// Duplicate signature parameter names (routine/graph). The machine
    /// world has no signature (`state_params` empty, tapes from decls).
    fn check_signature_params(
        &self,
        program: &Program,
        world: &ResolvedWorld,
        _idx: usize,
    ) -> Result<(), CompileError> {
        let sig = match world.kind {
            WorldKind::Machine => return Ok(()),
            WorldKind::Routine => program
                .routines
                .iter()
                .find(|r| full_name(&r.ns, &r.name) == world.name)
                .map(|r| &r.sig),
            WorldKind::Graph => program
                .graphs
                .iter()
                .find(|g| full_name(&g.ns, &g.name) == world.name)
                .map(|g| &g.sig),
        };
        let Some(sig) = sig else {
            return Ok(());
        };
        let mut seen: HashSet<&str> = HashSet::new();
        for p in &sig.params {
            if !seen.insert(&p.name) {
                return Err(CompileError {
                    span: p.name_span,
                    kind: CompileErrorKind::DuplicateParam(p.name.clone()),
                });
            }
        }
        Ok(())
    }

    /// Duplicate state names in one world — across state params, local
    /// states, and graft instances (they share the world's state-name space).
    fn check_duplicate_states(&self, world: &ResolvedWorld) -> Result<(), CompileError> {
        let mut seen: HashSet<&str> = HashSet::new();
        for p in &world.state_params {
            seen.insert(p.as_str());
        }
        for s in &world.states {
            if !seen.insert(&s.name) {
                return Err(CompileError {
                    span: s.name_span,
                    kind: CompileErrorKind::DuplicateState(s.name.clone()),
                });
            }
        }
        for g in &world.grafts {
            if let Some(name) = &g.as_name
                && !seen.insert(name)
            {
                return Err(CompileError {
                    span: g.target_span,
                    kind: CompileErrorKind::DuplicateState(name.clone()),
                });
            }
        }
        Ok(())
    }

    /// Exactly one `entry` per world (§10.5).
    fn check_entry(&self, world: &ResolvedWorld) -> Result<(), CompileError> {
        let entry_states: Vec<&State> = world.states.iter().filter(|s| s.entry).collect();
        let entry_grafts: Vec<&ResolvedGraft> = world.grafts.iter().filter(|g| g.entry).collect();
        let count = entry_states.len() + entry_grafts.len();
        if count == 1 {
            return Ok(());
        }
        // Zero → the world header; two-or-more → the second entry's span.
        let span = if count == 0 {
            world.name_span
        } else {
            // Order the entries by source span and point at the second.
            let mut spans: Vec<Span> = entry_states
                .iter()
                .map(|s| s.name_span)
                .chain(entry_grafts.iter().map(|g| g.span))
                .collect();
            spans.sort();
            spans[1]
        };
        Err(CompileError {
            span,
            kind: CompileErrorKind::EntryCount(count),
        })
    }

    /// The world's state-name space for goto / continuation / state-arg
    /// resolution: state params, local states, and graft instances.
    fn state_targets<'w>(&self, world: &'w ResolvedWorld) -> HashSet<&'w str> {
        let mut set: HashSet<&str> = HashSet::new();
        for p in &world.state_params {
            set.insert(p);
        }
        for s in &world.states {
            set.insert(&s.name);
        }
        for g in &world.grafts {
            if let Some(name) = &g.as_name {
                set.insert(name);
            }
        }
        set
    }

    fn bind_names<'w>(&self, world: &'w ResolvedWorld) -> HashSet<&'w str> {
        world.binds.iter().map(|b| b.name.as_str()).collect()
    }

    /// Walk this world's rules: `goto` / bare-name and `then` continuation
    /// resolution (same world only; `return` context), and `call` target +
    /// argument checks.
    fn check_rules(&mut self, world: &ResolvedWorld, is_routine: bool) -> Result<(), CompileError> {
        let states = self.state_targets(world);
        let binds = self.bind_names(world);
        let ns = self.world_ns(world);
        for s in &world.states {
            for rule in &s.rules {
                match &rule.transition {
                    Transition::Goto { name, span, .. } => {
                        self.check_state_target(name, *span, &states, &binds, &ns)?;
                    }
                    Transition::Return { span } => {
                        if !is_routine {
                            return Err(CompileError {
                                span: *span,
                                kind: CompileErrorKind::ReturnOutsideRoutine,
                            });
                        }
                    }
                    Transition::Stop { .. } | Transition::Halt { .. } => {}
                    Transition::Call { then, .. } => {
                        self.check_continuation(then, &states, &binds, &ns, is_routine)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn check_continuation(
        &mut self,
        cont: &Continuation,
        states: &HashSet<&str>,
        binds: &HashSet<&str>,
        ns: &[String],
        is_routine: bool,
    ) -> Result<(), CompileError> {
        match cont {
            Continuation::State { name, span } => {
                self.check_state_target(name, *span, states, binds, ns)
            }
            Continuation::Return { span } => {
                if is_routine {
                    Ok(())
                } else {
                    Err(CompileError {
                        span: *span,
                        kind: CompileErrorKind::ReturnOutsideRoutine,
                    })
                }
            }
            Continuation::Stop { .. } | Continuation::Halt { .. } => Ok(()),
        }
    }

    /// Resolve a `goto` / continuation state target: a same-world state
    /// (valid), a bind (`goto-into-bind`), a routine/graph in scope
    /// (`goto-not-a-state`), else `undefined-state`.
    fn check_state_target(
        &self,
        name: &str,
        span: Span,
        states: &HashSet<&str>,
        binds: &HashSet<&str>,
        ns: &[String],
    ) -> Result<(), CompileError> {
        if states.contains(name) {
            return Ok(());
        }
        if binds.contains(name) {
            return Err(CompileError {
                span,
                kind: CompileErrorKind::GotoIntoBind(name.to_string()),
            });
        }
        if let Some(r) = self.scopes.resolve(name, ns)
            && matches!(r.kind, Some(DefKind::Routine) | Some(DefKind::Graph))
        {
            return Err(CompileError {
                span,
                kind: CompileErrorKind::GotoNotAState(name.to_string()),
            });
        }
        Err(CompileError {
            span,
            kind: CompileErrorKind::UndefinedState(name.to_string()),
        })
    }

    /// Check `call` targets and their binding arguments, plus graft and bind
    /// targets. `call`s live inside rule transitions; grafts/binds are
    /// declarations.
    fn check_reuse_targets(&mut self, world: &ResolvedWorld) -> Result<(), CompileError> {
        let states = self.state_targets(world);
        let binds = self.bind_names(world);
        let tapes: HashSet<&str> = world.tapes.iter().map(|t| t.name.as_str()).collect();
        let ns = self.world_ns(world);

        // call transitions
        for s in &world.states {
            for rule in &s.rules {
                if let Transition::Call {
                    target, args, span, ..
                } = &rule.transition
                {
                    let joined = target.joined();
                    // A single-segment target naming a world-local bind is a
                    // bind-call (the bind carries the binding).
                    if target.segments.len() == 1 && binds.contains(joined.as_str()) {
                        continue;
                    }
                    self.check_call_like(
                        &joined,
                        target,
                        args,
                        *span,
                        DefKind::Routine,
                        "a routine",
                        &states,
                        &tapes,
                        &ns,
                    )?;
                }
            }
        }

        // graft declarations — the graph target is already resolved to a
        // local graph (`resolve_world_reuse`); check its binding args.
        for g in &world.grafts {
            self.check_binding_args(
                &g.target,
                &g.target,
                &g.args,
                DefKind::Graph,
                &states,
                &tapes,
            )?;
        }

        // bind declarations
        for b in &world.binds {
            if b.external {
                self.warn_undeclared_if_bare(&b.target, b.target_span, b.external);
                continue;
            }
            self.check_target_kind(&b.target, b.target_span, DefKind::Routine, "a routine")?;
            self.check_binding_args(
                &b.name,
                &b.target,
                &b.args,
                DefKind::Routine,
                &states,
                &tapes,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn check_call_like(
        &mut self,
        joined: &str,
        target: &QualName,
        args: &[BindingArg],
        _span: Span,
        want: DefKind,
        expected_noun: &'static str,
        states: &HashSet<&str>,
        tapes: &HashSet<&str>,
        ns: &[String],
    ) -> Result<(), CompileError> {
        match self.scopes.resolve(joined, ns) {
            Some(r) if r.kind == Some(want) => {
                if let Some(idx) = r.via_import {
                    self.imports_used[idx] = true;
                }
                self.check_binding_args(joined, &r.full, args, want, states, tapes)
            }
            Some(r) if r.kind.is_some() => Err(CompileError {
                span: target.span,
                kind: CompileErrorKind::WrongTargetKind {
                    name: joined.to_string(),
                    expected: expected_noun,
                },
            }),
            Some(r) => {
                // Absolute-external, or imported-to-external routine — allowed,
                // resolved at link; no arg check (no local signature). Mark
                // the import used if it went through one.
                if let Some(idx) = r.via_import {
                    self.imports_used[idx] = true;
                }
                Ok(())
            }
            None => {
                // Bare undeclared external: warn once, stays external.
                self.warn_undeclared(joined, target.span);
                Ok(())
            }
        }
    }

    fn check_target_kind(
        &self,
        full: &str,
        span: Span,
        want: DefKind,
        expected_noun: &'static str,
    ) -> Result<(), CompileError> {
        match self.scopes.by_full.get(full) {
            Some(k) if *k == want => Ok(()),
            Some(_) => Err(CompileError {
                span,
                kind: CompileErrorKind::WrongTargetKind {
                    name: full.to_string(),
                    expected: expected_noun,
                },
            }),
            None => Ok(()),
        }
    }

    /// Arity + argument-KIND checks against a locally-defined signature. Tape
    /// params take tape targets (world tapes); state params take state names
    /// (same-world states) or terminators. Map LEGALITY (glyph sets, etc.) is
    /// Task 5's — this only checks the kind.
    fn check_binding_args(
        &self,
        _target_desc: &str,
        sig_key: &str,
        args: &[BindingArg],
        want: DefKind,
        states: &HashSet<&str>,
        tapes: &HashSet<&str>,
    ) -> Result<(), CompileError> {
        let _ = want;
        let Some(sig) = self.scopes.sigs.get(sig_key) else {
            return Ok(());
        };
        // arg name -> param kind, with duplicate + unknown detection.
        let mut arg_seen: HashSet<&str> = HashSet::new();
        for a in args {
            if !arg_seen.insert(&a.name) {
                return Err(CompileError {
                    span: a.name_span,
                    kind: CompileErrorKind::DuplicateArg(a.name.clone()),
                });
            }
            let Some((_, kind)) = sig.params.iter().find(|(n, _)| *n == a.name) else {
                return Err(CompileError {
                    span: a.name_span,
                    kind: CompileErrorKind::UnknownArg(a.name.clone()),
                });
            };
            self.check_arg_kind(a, *kind, states, tapes)?;
        }
        // Every parameter must be bound.
        for (pname, _) in &sig.params {
            if !arg_seen.contains(pname.as_str()) {
                // Point at the first arg (or a zero span if argless).
                let span = args.first().map(|a| a.span).unwrap_or(Span::point(1, 1));
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::MissingArg(pname.clone()),
                });
            }
        }
        Ok(())
    }

    fn check_arg_kind(
        &self,
        arg: &BindingArg,
        kind: ParamKind,
        states: &HashSet<&str>,
        tapes: &HashSet<&str>,
    ) -> Result<(), CompileError> {
        match kind {
            ParamKind::Tape => match &arg.value {
                BindingValue::Named { target, .. } => {
                    if tapes.contains(target.as_str()) {
                        Ok(())
                    } else {
                        Err(CompileError {
                            span: arg.span,
                            kind: CompileErrorKind::UnresolvedTapeTarget(target.clone()),
                        })
                    }
                }
                BindingValue::Terminator { .. } => Err(CompileError {
                    span: arg.span,
                    kind: CompileErrorKind::WrongArgKind {
                        name: arg.name.clone(),
                        expected: "a tape target",
                    },
                }),
            },
            ParamKind::State => match &arg.value {
                // A `with map` makes it definitively a tape target — wrong.
                BindingValue::Named {
                    target, map: None, ..
                } => {
                    if states.contains(target.as_str()) {
                        Ok(())
                    } else {
                        Err(CompileError {
                            span: arg.span,
                            kind: CompileErrorKind::UndefinedState(target.clone()),
                        })
                    }
                }
                BindingValue::Named { .. } => Err(CompileError {
                    span: arg.span,
                    kind: CompileErrorKind::WrongArgKind {
                        name: arg.name.clone(),
                        expected: "a state or terminator",
                    },
                }),
                BindingValue::Terminator { .. } => Ok(()),
            },
        }
    }

    fn warn_undeclared(&mut self, name: &str, span: Span) {
        if self.warned_undeclared.insert(name.to_string()) {
            self.diagnostics.push(Diagnostic {
                code: "undeclared-external",
                span,
                message: format!(
                    "reference to undeclared external `{name}` — declare it with `use {name};`"
                ),
                fix: None,
            });
        }
    }

    fn warn_undeclared_if_bare(&mut self, name: &str, span: Span, external: bool) {
        if external && !name.contains("::") {
            self.warn_undeclared(name, span);
        }
    }

    fn world_ns(&self, world: &ResolvedWorld) -> Vec<String> {
        // The machine is file-level; a routine/graph's ns is its mangled
        // name minus the last `::` segment.
        match world.kind {
            WorldKind::Machine => Vec::new(),
            _ => {
                let mut segs: Vec<&str> = world.name.split("::").collect();
                segs.pop();
                segs.into_iter().map(str::to_string).collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `CompileErrorKind` code is a stable kebab identifier, and no two
    /// variants share one — the CLI and the language server key on these.
    /// One representative of each variant is listed here; the list length is
    /// asserted so a newly added variant forces a test update (the count
    /// doubles as a "did you wire `code()`?" reminder).
    #[test]
    fn error_codes_are_pairwise_distinct_and_complete() {
        let all = [
            CompileErrorKind::Lex("x".into()),
            CompileErrorKind::Expected {
                what: "x",
                found: "y".into(),
            },
            CompileErrorKind::ReservedName {
                name: "x".into(),
                what: "a state",
            },
            CompileErrorKind::MultipleMachines,
            CompileErrorKind::TapeNotInMachine,
            CompileErrorKind::NakedPattern,
            CompileErrorKind::WildcardBinding,
            CompileErrorKind::RangeKindMismatch,
            CompileErrorKind::CharArithmetic,
            CompileErrorKind::GraftNeedsName,
            CompileErrorKind::StateRedirect,
            CompileErrorKind::DanglingDocRun,
            CompileErrorKind::DocLineOrder,
            CompileErrorKind::UnknownAttribute("x".into()),
            CompileErrorKind::DuplicateAttribute,
            CompileErrorKind::EmptyAlphabet,
            CompileErrorKind::DuplicateGlyph("x".into()),
            CompileErrorKind::AlphabetTooLarge(200),
            CompileErrorKind::RangeEndpointNotScalar,
            CompileErrorKind::RangeDescending,
            CompileErrorKind::DuplicateName {
                name: "x".into(),
                what: "an alphabet",
            },
            CompileErrorKind::DuplicateBinding("x".into()),
            CompileErrorKind::TooManyTapes(17),
            CompileErrorKind::UnresolvedAlphabet("x".into()),
            CompileErrorKind::DuplicateTape("x".into()),
            CompileErrorKind::DuplicateState("x".into()),
            CompileErrorKind::DuplicateParam("x".into()),
            CompileErrorKind::EntryCount(0),
            CompileErrorKind::ReturnOutsideRoutine,
            CompileErrorKind::GotoIntoBind("x".into()),
            CompileErrorKind::GotoNotAState("x".into()),
            CompileErrorKind::UndefinedState("x".into()),
            CompileErrorKind::WrongTargetKind {
                name: "x".into(),
                expected: "a routine",
            },
            CompileErrorKind::UndefinedGraph("x".into()),
            CompileErrorKind::UnknownArg("x".into()),
            CompileErrorKind::DuplicateArg("x".into()),
            CompileErrorKind::MissingArg("x".into()),
            CompileErrorKind::WrongArgKind {
                name: "x".into(),
                expected: "a tape target",
            },
            CompileErrorKind::UnresolvedTapeTarget("x".into()),
        ];
        // Update this count when a variant joins — the reminder to wire
        // `code()` and this list together.
        assert_eq!(all.len(), 39);
        let mut codes: Vec<&str> = all.iter().map(|k| k.code()).collect();
        codes.sort_unstable();
        let mut deduped = codes.clone();
        deduped.dedup();
        assert_eq!(codes, deduped, "duplicate CompileErrorKind code: {codes:?}");
        // Every code is non-empty kebab-case (ascii lowercase + hyphens).
        for c in &codes {
            assert!(!c.is_empty());
            assert!(
                c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '-'),
                "code `{c}` is not kebab-case"
            );
        }
    }

    /// The rendered `Display` carries the `line:col: … [code]` house style.
    #[test]
    fn error_display_uses_the_house_style() {
        let e = CompileError {
            span: Span::new(3, 5, 3, 6),
            kind: CompileErrorKind::WildcardBinding,
        };
        let s = e.to_string();
        assert!(s.starts_with("line 3:5: "), "{s}");
        assert!(s.ends_with("[wildcard-binding]"), "{s}");
    }

    // -- test helpers -------------------------------------------------------

    fn ok(src: &str) -> Analysis {
        analyze(src).unwrap_or_else(|e| panic!("expected analyze to succeed: {e}"))
    }

    fn err(src: &str) -> CompileError {
        analyze(src).expect_err("expected analyze to fail")
    }

    fn code(src: &str) -> &'static str {
        err(src).kind.code()
    }

    /// Diagnostic codes an analysis produced (empty for a clean one).
    fn diag_codes(src: &str) -> Vec<&'static str> {
        ok(src).diagnostics.iter().map(|d| d.code).collect()
    }

    // -- alphabet resolution -----------------------------------------------

    #[test]
    fn alphabet_glyphs_are_positions_blank_is_index_zero() {
        let a = ok("alphabet ab { '_', 'a', 'b' }");
        assert_eq!(a.resolved.alphabets["ab"].glyphs, vec!["_", "a", "b"]);
        // Blank is whatever index 0 is, glyph or not:
        let a = ok("alphabet w { 'X', 'a', 'b' }");
        assert_eq!(a.resolved.alphabets["w"].glyphs[0], "X");
    }

    #[test]
    fn char_range_expands_by_scalar_succession() {
        let a = ok("alphabet r { '_', 'a'..'c' }");
        assert_eq!(a.resolved.alphabets["r"].glyphs, vec!["_", "a", "b", "c"]);
    }

    #[test]
    fn numeric_range_mints_decimal_string_glyphs() {
        let a = ok("alphabet n { 0..3 }");
        assert_eq!(a.resolved.alphabets["n"].glyphs, vec!["0", "1", "2", "3"]);
        // The A.4 alphabet: 127 symbols, glyph of 126 is the string "126".
        let a = ok("alphabet bytes { 0..126 }");
        assert_eq!(a.resolved.alphabets["bytes"].cardinality(), 127);
        assert_eq!(a.resolved.alphabets["bytes"].glyphs[126], "126");
    }

    #[test]
    fn alphabet_cap_is_127_inclusive() {
        // 127 symbols is exactly the compact family — accepted.
        assert!(analyze("alphabet ok { 0..126 }").is_ok());
        // 128 symbols overflows it — the recorded multi-byte-family deviation.
        let e = err("alphabet big { 0..127 }");
        assert_eq!(e.kind.code(), "alphabet-too-large");
        assert!(matches!(e.kind, CompileErrorKind::AlphabetTooLarge(128)));
    }

    #[test]
    fn empty_alphabet_is_rejected() {
        assert_eq!(code("alphabet e { }"), "empty-alphabet");
    }

    #[test]
    fn duplicate_glyph_is_rejected_at_the_repeat() {
        let e = err("alphabet d { 'a', 'b', 'a' }");
        assert_eq!(e.kind.code(), "duplicate-glyph");
        // The span points at the SECOND 'a' (line 1, the third element).
        assert_eq!(e.span.start.line, 1);
        assert!(matches!(e.kind, CompileErrorKind::DuplicateGlyph(g) if g == "a"));
        // A numeric glyph collides with the same-valued quoted digit.
        assert_eq!(code("alphabet m { '0', 0 }"), "duplicate-glyph");
    }

    #[test]
    fn range_endpoints_must_be_single_scalars_and_ascend() {
        assert_eq!(
            code("alphabet z { 'ab'..'c' }"),
            "range-endpoint-not-scalar"
        );
        assert_eq!(code("alphabet z { 'c'..'a' }"), "range-descending");
        assert_eq!(code("alphabet z { 5..3 }"), "range-descending");
    }

    // -- flatten: namespaces, use, mangling, visibility --------------------

    const A5: &str = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b', '0', '1' }

namespace mylib {
  export routine plusOne(tape num: bits) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*]   -> write ['1'] return;
    }
  }
}

use mylib::plusOne;

machine {
  tape ctl:  bits;
  tape data: wide;

  entry state main {
    ['1', *] -> call plusOne(num = data with map { '0'->'0', '1'->'1' }) then done;
    [*, *]   -> move [>, .] goto main;
  }

  state done { [*, *] -> stop; }
}
";

    const A6: &str = "\
alphabet marks { '_', 'x', 'y', 'z' }

export graph findX(tape t: marks, state found, state missing) {
  entry state walk {
    ['x'] -> found;
    ['_'] -> missing;
    [*]   -> move [>] goto walk;
  }
}

machine {
  tape work: marks;

  entry graft findX(t = work, found = celebrate, missing = giveUp) as seek;

  state celebrate { [*] -> write ['_'] stop; }
  state giveUp    { [*] -> halt; }
}
";

    #[test]
    fn routine_mangles_with_namespace_and_resolves_via_import() {
        let a = ok(A5);
        // The routine mangles `mylib::plusOne`; the machine world is `main`.
        assert!(a.resolved.alphabets.contains_key("bits"));
        assert!(
            a.resolved
                .worlds
                .iter()
                .any(|w| w.name == "mylib::plusOne" && w.kind == WorldKind::Routine && w.exported)
        );
        let machine = a
            .resolved
            .worlds
            .iter()
            .find(|w| w.kind == WorldKind::Machine)
            .unwrap();
        assert_eq!(machine.name, "main");
        assert_eq!(a.resolved.entry_world, Some(a.resolved.worlds.len() - 1));
        // A.5 resolves cleanly (the import IS used → no unused-import).
        assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
        // The call resolved to the mangled routine.
        assert!(machine.calls.is_empty()); // calls are wired in Task 6; T4 checks only
    }

    #[test]
    fn a5_resolves_both_the_import_and_the_absolute_spelling() {
        // Import spelling (A.5 verbatim) already covered; the direct
        // `::`-absolute spelling resolves without the `use` line.
        let direct = A5
            .replace("use mylib::plusOne;\n\n", "")
            .replace("call plusOne(", "call mylib::plusOne(");
        let a = ok(&direct);
        assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
        assert!(a.resolved.worlds.iter().any(|w| w.name == "mylib::plusOne"));
    }

    #[test]
    fn duplicate_binding_and_duplicate_name_are_fatal() {
        assert_eq!(
            code(
                "use a::plusOne; use b::plusOne; machine { tape t: x; entry state s { [*] -> stop; } }"
            ),
            "duplicate-binding"
        );
        // Two routines share a name in one scope.
        assert_eq!(
            code(
                "routine f(tape t: x) { entry state s { [*] -> return; } } graph f(tape t: x) { entry state s { [*] -> stop; } }"
            ),
            "duplicate-name"
        );
    }

    #[test]
    fn docs_are_keyed_by_mangled_name() {
        let src = "\
namespace mylib {
? increments a binary number
export routine plusOne(tape bits: b) {
  entry state s { [*] -> return; }
}
}
alphabet b { '_', '0', '1' }
";
        let a = ok(src);
        assert!(a.resolved.docs.contains_key("mylib::plusOne"));
        assert_eq!(
            a.resolved.docs["mylib::plusOne"].paragraphs,
            vec!["increments a binary number".to_string()]
        );
    }

    #[test]
    fn undeclared_external_warns_once_and_unused_import_warns() {
        // A bare call target nothing declares: undeclared-external (once).
        let src = "machine { tape t: b; entry state s { [*] -> call ghost() then s; } } alphabet b { '_', '0' }";
        let codes = diag_codes(src);
        assert_eq!(
            codes
                .iter()
                .filter(|c| **c == "undeclared-external")
                .count(),
            1,
            "{codes:?}"
        );
        // An import whose name is never referenced: unused-import.
        let src = "use lib::helper; alphabet b { '_', '0' } machine { tape t: b; entry state s { [*] -> stop; } }";
        assert!(diag_codes(src).contains(&"unused-import"));
    }

    // -- world checks -------------------------------------------------------

    fn machine_body(body: &str) -> String {
        format!("alphabet b {{ '_', '0', '1' }}\nmachine {{\n{body}\n}}\n")
    }

    #[test]
    fn tape_must_resolve_and_world_caps_at_16_tapes() {
        assert_eq!(
            code("machine { tape t: nope; entry state s { [*] -> stop; } }"),
            "unresolved-alphabet"
        );
        // 17 tapes.
        let tapes: String = (0..17).map(|i| format!("  tape t{i}: b;\n")).collect();
        let pat: String = std::iter::repeat_n("*", 17).collect::<Vec<_>>().join(", ");
        let src = format!(
            "alphabet b {{ '_', '0' }}\nmachine {{\n{tapes}  entry state s {{ [{pat}] -> stop; }}\n}}\n"
        );
        assert_eq!(code(&src), "too-many-tapes");
    }

    #[test]
    fn duplicate_tape_and_state_and_param_names() {
        assert_eq!(
            code(&machine_body(
                "  tape a: b;\n  tape a: b;\n  entry state s { [*, *] -> stop; }"
            )),
            "duplicate-tape"
        );
        assert_eq!(
            code(&machine_body(
                "  tape a: b;\n  entry state s { [*] -> stop; }\n  state s { [*] -> stop; }"
            )),
            "duplicate-state"
        );
        assert_eq!(
            code(
                "alphabet b { '_', '0' } routine f(tape t: b, tape t: b) { entry state s { [*, *] -> return; } }"
            ),
            "duplicate-param"
        );
    }

    #[test]
    fn entry_multiplicity_is_exactly_one() {
        // Zero entries.
        assert_eq!(
            code(&machine_body("  tape t: b;\n  state s { [*] -> stop; }")),
            "entry-count"
        );
        // Two entries.
        let e = err(&machine_body(
            "  tape t: b;\n  entry state a { [*] -> goto b; }\n  entry state b { [*] -> stop; }",
        ));
        assert_eq!(e.kind.code(), "entry-count");
        assert!(matches!(e.kind, CompileErrorKind::EntryCount(2)));
    }

    #[test]
    fn return_only_inside_a_routine() {
        // `return` in a machine rule is rejected.
        assert_eq!(
            code(&machine_body(
                "  tape t: b;\n  entry state s { [*] -> return; }"
            )),
            "return-outside-routine"
        );
        // A routine may return.
        assert!(
            analyze(
                "alphabet b { '_', '0' } routine f(tape t: b) { entry state s { [*] -> return; } }"
            )
            .is_ok()
        );
    }

    // -- cross-world checks -------------------------------------------------

    #[test]
    fn goto_stays_in_the_same_world() {
        // goto a nonexistent state.
        assert_eq!(
            code(&machine_body(
                "  tape t: b;\n  entry state s { [*] -> goto nope; }"
            )),
            "undefined-state"
        );
        // goto a routine (a reuse target, not a state).
        let src = "alphabet b { '_', '0' }\nroutine helper(tape t: b) { entry state s { [*] -> return; } }\nmachine { tape t: b; entry state s { [*] -> goto helper; } }";
        assert_eq!(code(src), "goto-not-a-state");
        // goto a bind name (GC9).
        let src = "alphabet b { '_', '0' }\nroutine helper(tape t: b) { entry state s { [*] -> return; } }\nmachine { tape t: b; bind helper(t = t) as h; entry state s { [*] -> goto h; } }";
        assert_eq!(code(src), "goto-into-bind");
    }

    #[test]
    fn call_target_must_be_a_routine_graft_must_be_a_graph() {
        // call targets a graph → wrong-target-kind.
        let src = "alphabet b { '_', '0' }\ngraph g(tape t: b) { entry state s { [*] -> stop; } }\nmachine { tape t: b; entry state s { [*] -> call g(t = t) then s; } }";
        assert_eq!(code(src), "wrong-target-kind");
        // graft targets a routine → wrong-target-kind (routine is not a graph).
        let src = "alphabet b { '_', '0' }\nroutine r(tape t: b) { entry state s { [*] -> return; } }\nmachine { tape t: b; entry graft r(t = t) as x; }";
        assert_eq!(code(src), "wrong-target-kind");
        // graft an unknown graph → undefined-graph (a graft needs source).
        let src = "alphabet b { '_', '0' }\nmachine { tape t: b; entry graft nope(t = t) as x; }";
        assert_eq!(code(src), "undefined-graph");
    }

    #[test]
    fn binding_argument_arity_and_kind_checks() {
        let prelude = "alphabet b { '_', '0' }\ngraph g(tape t: b, state done) { entry state s { ['0'] -> done; [*] -> move [>] goto s; } }\n";
        // An unknown argument name.
        let src = format!(
            "{prelude}machine {{ tape t: b; entry graft g(t = t, done = celebrate, bogus = t) as x; state celebrate {{ [*] -> stop; }} }}"
        );
        assert_eq!(code(&src), "unknown-arg");
        // A duplicate argument name.
        let src = format!(
            "{prelude}machine {{ tape t: b; entry graft g(t = t, t = t, done = celebrate) as x; state celebrate {{ [*] -> stop; }} }}"
        );
        assert_eq!(code(&src), "duplicate-arg");
        // A missing argument.
        let src = format!("{prelude}machine {{ tape t: b; entry graft g(t = t) as x; }}");
        assert_eq!(code(&src), "missing-arg");
        // A tape param handed a non-tape target.
        let src = format!(
            "{prelude}machine {{ tape t: b; entry graft g(t = nope, done = celebrate) as x; state celebrate {{ [*] -> stop; }} }}"
        );
        assert_eq!(code(&src), "unresolved-tape-target");
        // A state param handed a `with map` (definitively a tape target).
        let src = format!(
            "{prelude}machine {{ tape t: b; entry graft g(t = t, done = t with map {{ '0'->'0' }}) as x; }}"
        );
        assert_eq!(code(&src), "wrong-arg-kind");
        // A tape param handed a terminator.
        let src = format!(
            "{prelude}machine {{ tape t: b; entry graft g(t = stop, done = celebrate) as x; state celebrate {{ [*] -> stop; }} }}"
        );
        assert_eq!(code(&src), "wrong-arg-kind");
    }

    // -- the canonical examples resolve end-to-end -------------------------

    #[test]
    fn appendix_a_examples_resolve_cleanly() {
        for (name, src) in [("A5", A5), ("A6", A6)] {
            let a = analyze(src).unwrap_or_else(|e| panic!("{name} failed: {e}"));
            assert!(a.diagnostics.is_empty(), "{name}: {:?}", a.diagnostics);
        }
        // A.6's graft carries the resolved graph target + entry instance.
        let a = ok(A6);
        let machine = a
            .resolved
            .worlds
            .iter()
            .find(|w| w.kind == WorldKind::Machine)
            .unwrap();
        assert_eq!(machine.entry.as_deref(), Some("seek"));
        assert_eq!(machine.grafts.len(), 1);
        assert_eq!(machine.grafts[0].target, "findX");
    }

    #[test]
    fn a_library_source_compiles_with_no_entry_world() {
        // No `machine` block: a legal library (mirrors `.pmc`'s mainless
        // sources). analyze succeeds and `entry_world` is None.
        let a = ok(
            "alphabet b { '_', '0' }\nexport routine r(tape t: b) { entry state s { [*] -> return; } }",
        );
        assert_eq!(a.resolved.entry_world, None);
        assert!(
            a.resolved
                .worlds
                .iter()
                .all(|w| w.kind != WorldKind::Machine)
        );
    }
}
