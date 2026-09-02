//! `.tmc` compiler driver and shared diagnostics — the front-end mirror of
//! the `.pmc` compiler in the sibling PM-1 crate.
//!
//! It hosts the shared fatal type every pipeline stage reports through, the
//! resolution / flatten / world-check stage ([`analyze`]) that produces the
//! [`Resolved`] module graft + range expansion and IR lowering consume, the
//! full `compile()` orchestration (`analyze` → expand → lower → codegen), and
//! [`analyze_staged`] — the partial-results seam the language service drives
//! off. Library code never prints: fatals flow as span-carrying, coded values
//! and the CLI is the sole renderer.
//!
//! A few analysis-surface items ([`Analysis`]'s `tokens`/`program`, the whole
//! staged seam) are read only by the language-tooling layers rather than by
//! `compile()`; each carries its own `dead_code` allow with the reason.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mtc_core::diagnostics::{Diagnostic, Span};
use mtc_core::formats::object::ObjectFile;
use mtc_core::syntax::{GreenNode, SyntaxNode};

use crate::codegen::{CodegenOptions, emit_program};
use crate::footprint::SymSet;
use crate::ir::{IrProgram, lower, validate_world};
use crate::lexer::{LexMode, Token, lex_with};
use crate::optimizer::{OptLevel, OptOptions, OptReport, optimize};
use crate::parser::{
    Alphabet, AlphabetElem, Bind, BindingArg, BindingValue, Continuation, ContractClause, Doc,
    Graft, Machine, PatternCellKind, Program, QualName, Rule, SigParamKind, State, SymLit,
    Transition, parse_green_from_tokens,
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
    /// A `writes { … }` clause on a signature tape parameter written after
    /// that same parameter's `preserves` clause. The fixed order — `writes`
    /// then `preserves` — is a grammar rule, not an fmt convention: fmt is
    /// token-preserving and cannot reorder an author's clauses.
    ContractClauseOrder,
    /// A second `writes` or `preserves` clause on one signature tape
    /// parameter. `what` names the repeated keyword.
    DuplicateContractClause { what: &'static str },

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
    /// target, never a state (its own dedicated error).
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
    /// Two tape-parameter arguments of one `call`/`graft`/`bind` name the
    /// same caller tape, so one physical head would have to back two callee
    /// tapes read through two independent maps
    /// (docs/tmt/language.md (reuse)). `first` is the argument that claimed
    /// `target`; the error is reported at the second one.
    DuplicateTapeTarget {
        first: String,
        second: String,
        target: String,
    },
    /// A `call` on a world-local bind name carries binding arguments. A bind
    /// is already fully bound at its declaration, so a call on it takes none.
    BindCallArgs(String),
    /// A `writes`/`preserves` clause names a glyph the parameter's alphabet
    /// does not contain. A contract is stated in the tape's own frame, so
    /// every element must be a symbol of that alphabet. `clause` names which
    /// of the two clauses carried it.
    ContractSymbolUnknown {
        glyph: String,
        clause: &'static str,
        alphabet: String,
    },
    /// A world's inferred write footprint on one tape leaves the effective
    /// set its contract declares (`writes` minus `preserves`, or everything
    /// minus `preserves` when there is no `writes`). `glyphs` are the
    /// offending symbols, ascending. The inference OVER-approximates: a
    /// symbol outside the footprint provably never lands, while one inside it
    /// merely may — hence "may write".
    WritesOutsideContract {
        world: String,
        tape: String,
        glyphs: Vec<String>,
    },

    // -- graft + range expansion -------------------------------------------
    /// A graph definition graft-depends on itself (directly or through a
    /// cycle of definitions) — infinite expansion. `name` is one graph on the
    /// cycle. Instance-level cycles (continuation loops) stay legal.
    GraftCycle(String),
    /// A grafted graph's body contains a `call` (a routine call or a bind
    /// call) — splicing it into the host is not supported yet. The call's
    /// binding args still name the GRAPH's signature tapes and its `then`
    /// continuation is a graph-space state; rewriting both into host space
    /// needs the binding composition that is not implemented, so grafting
    /// such a graph is a clear error rather than silently-wrong output.
    /// `name` is the call's target. The check runs at SPLICE time: a graph
    /// that carries a call but is never grafted stays legal and dead (an
    /// ungrafted graph is never expanded — the same unreachable-graph
    /// posture the resolver takes).
    GraftCallUnsupported(String),
    /// A graft binding's symbol map references a glyph that is not in the tape
    /// it maps (the host tape for the `src`, the graph tape for the `dst`).
    MapSymbolNotInAlphabet(String),
    /// A graft binding maps the blank off itself — `'_'->X` (blank must read
    /// as blank) or a two-way `Y->'_'` (its write-back would un-pin blank; a
    /// read-only `Y=>'_'` collapse is the legal spelling).
    MapBlankPin,
    /// A graft binding maps one symbol to two different images in one
    /// direction — a read collision, or a two-way write-back collision.
    MapConflict { symbol: String },
    /// A graft binding on equal-size alphabets is not injective: identity
    /// completion collides (two symbols would read as one). `symbol` is the
    /// collision image.
    MapNotInjective { symbol: String },
    /// A graft binding omits the symbol map on tapes whose alphabets are not
    /// glyph-for-glyph equal — an omitted map means identity, which requires
    /// matching alphabets.
    IdentityGlyphMismatch,
    /// A write substitution folds to a value with no glyph in the tape's
    /// alphabet (an out-of-alphabet fold result). `name` is the message.
    FoldOutOfAlphabet(String),
    /// A `%` in a write-cell fold has a zero modulus (division by zero). The
    /// `%` semantics mirror the assembler's `.rept` substitution exactly
    /// (docs/tmt/language.md (substitution)).
    FoldZeroModulus,
    /// A `%` in a write-cell fold produces a negative remainder — reachable
    /// only when the left operand went negative through subtraction, which
    /// the assembler rejects rather than wrapping. When the modulus is a
    /// positive integer literal, `hint_modulus` carries it so the diagnostic
    /// can teach the `{(v+N-1)%N}` wrapping-decrement idiom.
    FoldNegativeRemainder { hint_modulus: Option<i64> },
    /// A write-cell fold overflows `i64` during evaluation.
    FoldOverflow,
    /// Two rules in one state match the same concrete tuple with neither
    /// carrying a wildcard — an exact-row disjointness violation. The two
    /// rendered patterns name both offenders.
    ExactRowConflict { first: String, second: String },
    /// A rule's pattern / write / move vector width does not equal the world's
    /// tape count (a signed-function width mismatch). `expected` is the arity.
    RowWidth { expected: usize, got: usize },

    // -- IR-lowering scope limits (composition engine not yet online) -------
    /// A `call`/`bind` binds tapes into a routine that is not defined in this
    /// compilation unit (an imported-to-external or `::`-absolute target). A
    /// tape-binding operand rewrites the callee's rows through the binding
    /// map, which needs the callee's tape signature — unknown for an external
    /// routine until the composition engine crosses object boundaries.
    /// `name` is the external target. A PLAIN external call (no tape binding)
    /// stays legal — the linker resolves it. The `graft-call-unsupported`
    /// analog for cross-object calls.
    ExternalBindingUnsupported(String),
    /// A routine body's `goto` (or continuation) targets one of the routine's
    /// own `state` parameters. Threading a state parameter to its call-site
    /// continuation is the composition engine's work; a routine that hands
    /// control to a `state` param cannot yet be lowered on its own. `name` is
    /// the state parameter.
    StateParamContinuationUnsupported(String),

    // -- codegen / assemble orchestration ----------------------------------
    /// A compiler-internal invariant broke: the codegen-produced `.tma`
    /// failed to assemble, or an IR world the compiler itself built failed
    /// [`crate::ir::validate_world`]. Never a user error — the message
    /// carries the underlying diagnostic. The `.pmc` compiler's `Internal`.
    Internal(String),
}

/// Binds each [`CompileErrorKind`] variant to its stable code exactly
/// once, expanding to BOTH the exhaustive `code()` match and the `CODES`
/// registry table, so the two cannot diverge: a new variant fails to
/// compile until it gets a row here, and the row lands in the table the
/// completeness and docs drift guards read (docs/tmt/cli.md (compile
/// errors)).
macro_rules! code_registry {
    ($($variant:pat => $code:literal,)+) => {
        /// Every code a [`CompileErrorKind`] can render, in declaration
        /// order — the registry the drift guards set-compare against
        /// the published inventory (docs/tmt/cli.md (compile errors)).
        pub const CODES: &[&str] = &[$($code),+];

        /// Stable kebab-case code, one per variant (docs/tmt/cli.md (compile
        /// errors)). Frozen once published — these are permanent
        /// user-visible identifiers: the CLI brackets them into every fatal
        /// rendering, and the language server carries them in the LSP
        /// diagnostic `code` field. The message itself stays the kind's
        /// own `Display`, which is why the `[code]` suffix lives on
        /// [`CompileError`]'s `Display`, not here.
        pub fn code(&self) -> &'static str {
            match self {
                $($variant => $code,)+
            }
        }
    };
}

impl CompileErrorKind {
    code_registry! {
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
        CompileErrorKind::ContractClauseOrder => "contract-clause-order",
        CompileErrorKind::DuplicateContractClause { .. } => "duplicate-contract-clause",
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
        CompileErrorKind::DuplicateTapeTarget { .. } => "duplicate-tape-target",
        CompileErrorKind::BindCallArgs(_) => "bind-call-args",
        CompileErrorKind::ContractSymbolUnknown { .. } => "contract-symbol-unknown",
        CompileErrorKind::WritesOutsideContract { .. } => "writes-outside-contract",
        CompileErrorKind::GraftCycle(_) => "graft-cycle",
        CompileErrorKind::GraftCallUnsupported(_) => "graft-call-unsupported",
        CompileErrorKind::MapSymbolNotInAlphabet(_) => "map-symbol-not-in-alphabet",
        CompileErrorKind::MapBlankPin => "map-blank-pin",
        CompileErrorKind::MapConflict { .. } => "map-conflict",
        CompileErrorKind::MapNotInjective { .. } => "map-not-injective",
        CompileErrorKind::IdentityGlyphMismatch => "identity-glyph-mismatch",
        CompileErrorKind::FoldOutOfAlphabet(_) => "fold-out-of-alphabet",
        CompileErrorKind::FoldZeroModulus => "zero-modulus",
        CompileErrorKind::FoldNegativeRemainder { .. } => "negative-remainder",
        CompileErrorKind::FoldOverflow => "fold-overflow",
        CompileErrorKind::ExactRowConflict { .. } => "exact-row-conflict",
        CompileErrorKind::RowWidth { .. } => "row-width",
        CompileErrorKind::ExternalBindingUnsupported(_) => "external-binding-unsupported",
        CompileErrorKind::StateParamContinuationUnsupported(_) => "state-param-continuation-unsupported",
        CompileErrorKind::Internal(_) => "internal-error",
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
            CompileErrorKind::ContractClauseOrder => {
                write!(f, "`writes` must come before `preserves`")
            }
            CompileErrorKind::DuplicateContractClause { what } => {
                write!(f, "duplicate `{what}` clause")
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
            CompileErrorKind::DuplicateTapeTarget {
                first,
                second,
                target,
            } => {
                write!(
                    f,
                    "binding arguments `{first}` and `{second}` both bind tape `{target}` — \
                     one caller tape cannot back two callee tapes"
                )
            }
            CompileErrorKind::BindCallArgs(n) => {
                write!(
                    f,
                    "`{n}` is a bind and already bound — a call on it takes no arguments"
                )
            }
            CompileErrorKind::ContractSymbolUnknown {
                glyph,
                clause,
                alphabet,
            } => {
                write!(
                    f,
                    "'{glyph}' in the `{clause}` clause is not a symbol of alphabet `{alphabet}`"
                )
            }
            CompileErrorKind::WritesOutsideContract {
                world,
                tape,
                glyphs,
            } => {
                let named = glyphs
                    .iter()
                    .map(|g| format!("'{g}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "`{world}` may write {named} on tape `{tape}`, which its contract forbids"
                )
            }
            CompileErrorKind::GraftCycle(n) => {
                write!(
                    f,
                    "graph `{n}` grafts itself (directly or through a cycle of graph definitions) — infinite expansion"
                )
            }
            CompileErrorKind::GraftCallUnsupported(name) => {
                write!(
                    f,
                    "this graft splices a graph whose body calls `{name}` — a call inside a grafted graph body is not supported yet; it awaits binding composition"
                )
            }
            CompileErrorKind::MapSymbolNotInAlphabet(g) => {
                write!(f, "map symbol `{g}` is not in the tape's alphabet")
            }
            CompileErrorKind::MapBlankPin => {
                write!(
                    f,
                    "a graft map may not move the blank off itself — blank reads and writes as blank (a `Y=>'_'` read-only collapse is allowed)"
                )
            }
            CompileErrorKind::MapConflict { symbol } => {
                write!(
                    f,
                    "graft map sends symbol `{symbol}` to two different images"
                )
            }
            CompileErrorKind::MapNotInjective { symbol } => {
                write!(
                    f,
                    "graft map on an equal-size alphabet is not injective — identity completion collides on `{symbol}`"
                )
            }
            CompileErrorKind::IdentityGlyphMismatch => {
                write!(
                    f,
                    "an omitted graft map means identity, which requires the two tapes to have glyph-for-glyph equal alphabets"
                )
            }
            CompileErrorKind::FoldOutOfAlphabet(m) => {
                write!(f, "{m}")
            }
            CompileErrorKind::FoldZeroModulus => {
                write!(f, "zero modulus in fold (`% 0`)")
            }
            CompileErrorKind::FoldNegativeRemainder { hint_modulus } => match hint_modulus {
                Some(n) => write!(
                    f,
                    "negative remainder in fold; for a wrapping decrement write {{(v+{})%{}}}",
                    n - 1,
                    n
                ),
                None => write!(f, "negative remainder in fold"),
            },
            CompileErrorKind::FoldOverflow => {
                write!(f, "fold arithmetic overflows i64")
            }
            CompileErrorKind::ExactRowConflict { first, second } => {
                write!(
                    f,
                    "two rules match the same input with no wildcard between them: `{first}` and `{second}`"
                )
            }
            CompileErrorKind::RowWidth { expected, got } => {
                write!(
                    f,
                    "a rule vector has {got} elements but the world has {expected} tapes"
                )
            }
            CompileErrorKind::ExternalBindingUnsupported(name) => {
                write!(
                    f,
                    "this call binds tapes into `{name}`, which needs `{name}`'s tape signature — unknown for a routine defined outside this compilation unit; compile `{name}` in the same unit (a plain call with no tape binding is fine — the linker resolves it)"
                )
            }
            CompileErrorKind::StateParamContinuationUnsupported(name) => {
                write!(
                    f,
                    "this routine hands control to its `state` parameter `{name}` — threading a state parameter to the call site is not supported yet"
                )
            }
            CompileErrorKind::Internal(m) => write!(f, "internal compiler error: {m}"),
        }
    }
}

impl std::error::Error for CompileError {}

// ---------------------------------------------------------------------------
// Alphabet resolution — elements to glyph vectors
// (docs/tmt/language.md (alphabets)).
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
pub(crate) fn glyph_label(s: &SymLit) -> String {
    match s {
        SymLit::Glyph { value, .. } => value.clone(),
        SymLit::Number { value, .. } => value.to_string(),
    }
}

/// Expand a range element into its glyph labels. Glyph ranges require
/// single-scalar endpoints and walk Unicode scalar succession; numeric ranges
/// mint each value's decimal string. Both are inclusive and ascending.
pub(crate) fn expand_range(
    lo: &SymLit,
    hi: &SymLit,
    span: Span,
) -> Result<Vec<String>, CompileError> {
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
// The resolved module — the front-end structure the graft/range expansion
// and the IR lowering consume.
// ---------------------------------------------------------------------------

/// The whole resolved module. Rules stay in SOURCE form (patterns unexpanded
/// — the graft/range expander owns expansion); every span is preserved.
/// Cross-world references (`call`/`graft`/`bind` targets, tape alphabets) are
/// resolved to mangled names; the worlds carry the rest verbatim.
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
    /// graft (the graft/range expander names it the spliced entry state) or
    /// a library-world with an entry that carries no addressable name.
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
/// from and that alphabet's cardinality — the frame every symbol index on
/// this tape, from a rule's write cells to a declared contract, is resolved
/// against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTape {
    pub name: String,
    pub name_span: Span,
    /// Mangled alphabet name (a key into `Resolved.alphabets`).
    pub alphabet: String,
    pub cardinality: usize,
    pub span: Span,
    pub volatile: bool,
    /// The declared `writes { … }` clause as symbol indices in THIS tape's
    /// alphabet frame; `None` when the parameter declares none (an absent
    /// clause permits every symbol, which is not the same as an empty one).
    /// Always `None` on a machine tape — only a signature parameter takes a
    /// contract.
    pub writes: Option<SymSet>,
    /// The declared `preserves { … }` clause, same frame and same `None`
    /// meaning: nothing is declared off-limits.
    pub preserves: Option<SymSet>,
}

/// A resolved graft declaration: the mangled graph target plus the raw
/// (source-form) binding args the graft/range expander applies at splice time.
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

/// The front half of the pipeline: everything expand / IR lowering (and the
/// batch lint layer) need. Mirrors the `.pmc` compiler's `AnalysisOutput`
/// shape.
///
/// The token stream and the flat program are here for the batch lint layer,
/// which is the only thing that reads them off this bundle — and reads them
/// with a constraint: `tokens` carries comment trivia, and the
/// adjacency-walking rules need [`crate::parser::significant_tokens`] of it
/// first (`crate::lint::lint` does the filtering, once, for all of them).
/// Nothing on the compile path past [`analyze`] touches either field. The
/// language service needs both too, but takes them from
/// [`TmcStagedAnalysis`], which retains every stage's outcome independently —
/// a partial-results shape this success-only bundle cannot express.
#[derive(Debug)]
pub(crate) struct Analysis {
    pub resolved: Resolved,
    pub diagnostics: Vec<Diagnostic>,
    /// The flat AST, extracted from the green tree. Retained so the lint
    /// layer can reach source-level detail the resolved module elides (e.g. a
    /// signature parameter's own span). Trivia-free: extraction flattens the
    /// tree and keeps no comments.
    pub program: Program,
    /// The lexed token stream, COMMENT-INCLUSIVE — the green parse needs the
    /// trivia, so this is the `LexMode::WithComments` stream it was built
    /// from. Retained for the lint layer's comment guard, which withholds a
    /// fix whose span holds a comment ([`crate::lint::LintContext`]'s
    /// `comment_tokens`). No rule reads a comment-free stream any more:
    /// every quickfix span is a range query over `green`.
    pub tokens: Vec<Token>,
    /// The green tree the program was extracted from (docs/core.md (syntax
    /// trees)) — the lint layer's quickfix spans are node ranges read off
    /// it, the same `Rc` the staged analysis retains for the editor.
    pub green: Rc<GreenNode>,
}

/// lex → green parse → extract → duplicate-binding check → resolve alphabets
/// → flatten + world checks. The `.tmc` analog of the `.pmc` compiler's
/// `analyze`; `compile` composes it with codegen. Fatals stop at the first
/// offending span; non-fatal findings (undeclared external, unused import)
/// accumulate as diagnostics.
///
/// The lex is `WithComments` because the green tree is built from the source
/// text and its trivia together (docs/core.md (syntax trees)) — a
/// comment-free stream could not reconstruct a comment's own text. What this
/// front accepts, and the exact kind and span of every error it rejects
/// with, are pinned by `tests/tmc_green_analyze.rs` over a deliberately
/// broken set.
pub(crate) fn analyze(source: &str) -> Result<Analysis, CompileError> {
    analyze_with(source, ExternalContracts::Stdlib)
}

/// [`analyze`] with an explicit choice of the external modules whose
/// declared write contracts the footprint inference believes.
pub(crate) fn analyze_with(
    source: &str,
    externals: ExternalContracts,
) -> Result<Analysis, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    let program = crate::syntax::extract_program(&SyntaxNode::new_root(Rc::clone(&green)), source);
    let (resolved, diagnostics) = resolve_program(&program, externals)?;
    Ok(Analysis {
        resolved,
        diagnostics,
        program,
        tokens,
        green,
    })
}

/// The resolution stage shared by [`analyze`] and [`analyze_staged`]:
/// everything after the parse — duplicate-binding check → scope build →
/// alphabet resolution → module resolution → per-world checks → unused-import
/// warnings. Returns the resolved module plus its accumulated non-fatal
/// diagnostics, or the first fatal at its offending span.
///
/// Only unused-import is raised here (unused-routine is raised during IR
/// lowering). The sibling unused-graph / unused-binding /
/// unused-graft-instance warnings of the same hygiene family are deliberately
/// deferred to the TM lint layer rather than shipped as compiler diagnostics.
fn resolve_program(
    program: &Program,
    externals: ExternalContracts,
) -> Result<(Resolved, Vec<Diagnostic>), CompileError> {
    check_duplicate_bindings(program)?;
    let scopes = Scopes::build(program)?;
    let alphabets = resolve_all_alphabets(program, &scopes)?;
    let resolved = resolve_module(program, &scopes, alphabets)?;
    let mut ctx = WorldCtx {
        scopes: &scopes,
        imports_used: vec![false; program.imports.len()],
        warned_undeclared: HashSet::new(),
        diagnostics: Vec::new(),
    };
    ctx.check_worlds(program, &resolved)?;
    check_contracts(&resolved, externals)?;
    let WorldCtx {
        imports_used,
        mut diagnostics,
        ..
    } = ctx;
    unused_import_warnings(program, &imports_used, &mut diagnostics);
    Ok((resolved, diagnostics))
}

/// A tape parameter's declared EFFECTIVE set (docs/tmt/language.md (contract
/// clauses)): `writes` — the whole alphabet when there is no `writes` clause —
/// minus `preserves`. A tape with neither clause therefore answers the whole
/// alphabet, which is also what an external callee with no contract
/// contributes to its callers.
pub(crate) fn declared_effective(tape: &ResolvedTape) -> SymSet {
    let declared = tape
        .writes
        .unwrap_or_else(|| SymSet::full(tape.cardinality as u32));
    let preserved = tape.preserves.unwrap_or_else(SymSet::empty);
    let mut allowed = SymSet::empty();
    for index in declared.iter() {
        if !preserved.contains(index) {
            allowed.insert(index);
        }
    }
    allowed
}

/// Check every declared write contract against the inferred write footprint.
///
/// A contract states what a world's body — and everything it calls or grafts —
/// may put on one of its tapes. The effective permission is `writes` (or, with
/// no `writes` clause, the whole alphabet) MINUS `preserves`; a symbol in both
/// clauses is redundancy rather than an error, and the subtraction settles it
/// in `preserves`' favour. A footprint reaching outside that set is fatal at
/// the parameter that declared the contract.
///
/// The inference OVER-approximates (`crate::footprint`'s soundness contract):
/// a symbol it excludes provably never lands on the tape, while one it
/// includes merely may. The finding is worded accordingly — a body can be
/// reported for a write no run performs, never the other way round.
///
/// The whole-module fixpoint runs at most once per compile, and only when some
/// world actually declares a clause: an uncontracted module pays nothing.
fn check_contracts(resolved: &Resolved, externals: ExternalContracts) -> Result<(), CompileError> {
    let contracted = resolved.worlds.iter().any(|w| {
        w.tapes
            .iter()
            .any(|t| t.writes.is_some() || t.preserves.is_some())
    });
    if !contracted {
        return Ok(());
    }
    let table = crate::footprint::infer_resolved_with(resolved, &externals.modules());
    // Worlds in resolved order, tapes in signature order, so the first finding
    // on a module with several is the source-first one.
    for world in &resolved.worlds {
        // Both sides key on the MANGLED world name (`main`, `ns::name`) —
        // `infer_resolved` builds its table from this very list, so a miss is
        // unreachable. Guarded rather than skipped quietly because the failure
        // mode is invisible: keying that drifted apart would silently stop
        // enforcing every contract in the module instead of breaking loudly.
        // A `debug_assert` because the totality is the footprint engine's
        // invariant to keep, not this module's to enforce at run time.
        let Some(footprint) = table.worlds.get(&world.name) else {
            debug_assert!(
                false,
                "no inferred footprint for world `{}` — the contract check's \
                 keying has drifted from the footprint table's",
                world.name
            );
            continue;
        };
        for (k, tape) in world.tapes.iter().enumerate() {
            if tape.writes.is_none() && tape.preserves.is_none() {
                continue;
            }
            let allowed = declared_effective(tape);
            let inferred = footprint.tapes.get(k).copied().unwrap_or_default();
            let glyphs = resolved
                .alphabets
                .get(&tape.alphabet)
                .map(|a| a.glyphs.as_slice())
                .unwrap_or_default();
            // Every inferred index is clamped below its own tape's
            // cardinality (`footprint.rs`'s soundness contract), so indexing
            // the glyph table directly is safe.
            let offending: Vec<String> = inferred
                .iter()
                .filter(|index| !allowed.contains(*index))
                .map(|index| glyphs[index as usize].clone())
                .collect();
            if !offending.is_empty() {
                return Err(CompileError {
                    span: tape.span,
                    kind: CompileErrorKind::WritesOutsideContract {
                        world: world.name.clone(),
                        tape: tape.name.clone(),
                        glyphs: offending,
                    },
                });
            }
        }
    }
    Ok(())
}

/// True when every match cell of a rule's pattern is a wildcard (`[*, …]`) —
/// an all-wildcard catch-all that matches every input.
fn is_all_wildcard_rule(rule: &Rule) -> bool {
    !rule.pattern.cells.is_empty()
        && rule
            .pattern
            .cells
            .iter()
            .all(|c| matches!(c.kind, PatternCellKind::Wildcard))
}

/// Drop a state's second (and later) all-wildcard catch-all rule
/// (docs/tmt/language.md (rules)). Codegen re-bands a state into
/// `[exact] ++ [partial] ++ [catch-all]` and takes the first match in THAT
/// order, so an exact or partial rule written after a catch-all still sorts
/// ahead of it and stays reachable — only a SECOND all-wildcard rule is
/// genuinely dead, since the first catch-all already matches everything. Such a
/// rule warns (`unreachable-rule`) and is removed, so the assembler's
/// all-wildcard-row-must-be-last discipline can never be violated at codegen.
/// Detection is on the flattened (resolved, pre-expansion) rules, so both
/// hand-written states and the bodies of grafted graphs are covered. A world's
/// `calls` list holds one entry per `call` transition in source order; it is
/// filtered in tandem so it stays aligned with the surviving rules that
/// expansion later walks.
fn drop_unreachable_rules(resolved: &mut Resolved, diagnostics: &mut Vec<Diagnostic>) {
    for world in &mut resolved.worlds {
        let old_calls = std::mem::take(&mut world.calls);
        let mut kept_calls: Vec<ResolvedCall> = Vec::new();
        let mut call_ix = 0usize; // running index into `old_calls` (source order)
        for state in &mut world.states {
            let mut seen_catch_all = false;
            let old_rules = std::mem::take(&mut state.rules);
            let mut new_rules: Vec<Rule> = Vec::with_capacity(old_rules.len());
            for rule in old_rules {
                let is_call = matches!(rule.transition, Transition::Call { .. });
                let is_catch_all = is_all_wildcard_rule(&rule);
                if is_catch_all && seen_catch_all {
                    diagnostics.push(Diagnostic {
                        code: "unreachable-rule",
                        span: rule.span,
                        message: "this rule can never fire — an earlier all-wildcard rule in \
                                  this state already matches every input"
                            .to_string(),
                        fix: None,
                    });
                    if is_call {
                        call_ix += 1; // consume the dropped rule's call slot
                    }
                    continue;
                }
                seen_catch_all |= is_catch_all;
                if is_call {
                    kept_calls.push(old_calls[call_ix].clone());
                    call_ix += 1;
                }
                new_rules.push(rule);
            }
            state.rules = new_rules;
        }
        world.calls = kept_calls;
    }
}

/// The language-service pipeline entry (the `.pmc` compiler's `analyze_staged`
/// twin): every stage's outcome, retained independently, so a document that
/// fails partway through still serves whatever the earlier stages produced.
/// Fields go `None` past the first failure; `fatal` carries that one error.
///
/// The shape is broken out field-by-field rather than embedding a success-only
/// bundle: the flat `program` must survive a *resolve*-stage fatal (an editor
/// still highlights a program whose semantics don't check out), which a
/// bundle present only on full success could not express.
///
/// Consumed by the phase-7 `.tmc` language service (live diagnostics + the
/// completion / hover / go-to-definition surfaces), not by `compile()`.
#[derive(Debug)]
pub(crate) struct TmcStagedAnalysis {
    /// WithComments token stream — `None` only if lexing itself failed.
    pub tokens: Option<Vec<Token>>,
    /// Green syntax tree of the current text (docs/core.md (syntax
    /// trees)); `None` when lexing or parsing failed. Read by
    /// `document_symbols` and `quickfix.rs`'s `state_stub` (`lsp/mod.rs`,
    /// `lsp/quickfix.rs`), both indexing into it by byte range rather
    /// than reparsing.
    pub green: Option<Rc<GreenNode>>,
    /// The RESILIENT parse's tree (docs/core.md (syntax trees), error
    /// recovery), built exactly when `green` is `None` for a
    /// parse-stage fatal: lossless over the current text, broken
    /// regions wrapped in ERROR nodes. Green-tier features that can
    /// tolerate a partial tree (symbols) fall back to it; everything
    /// keyed to a CLEAN parse (formatting, `state_stub`) reads `green`
    /// alone.
    pub recovered_green: Option<Rc<GreenNode>>,
    /// The flat program, extracted from the green tree — present whenever
    /// the green parse succeeded, retained even when the resolve stage then
    /// fails.
    pub program: Option<Program>,
    /// The resolved module — `Some` only when the whole resolve stage ran
    /// clean; `Resolved.docs` carries the doc map hover / the deprecation lint
    /// read.
    pub resolved: Option<Resolved>,
    /// Non-fatal diagnostics produced so far. TM emits none before the resolve
    /// stage completes (unused-import is raised last), so this is empty at
    /// every failure break point and populated only alongside a `Some`
    /// `resolved` — but the field is always present, carrying whatever was
    /// produced.
    pub diagnostics: Vec<Diagnostic>,
    /// The first (only) fatal, at whichever stage produced it.
    pub fatal: Option<CompileError>,
}

/// lex (WithComments) → green parse → extract → the resolve stage, retaining
/// each stage's outcome instead of stopping at the first failure. The green
/// parse is the AUTHORITY: it produces both `program` (via
/// `syntax::extract_program`, infallible once the tree exists) and any parse
/// fatal; `green` retains that same tree — one `Rc` clone, not a second
/// parse — so the language service's tree-backed readers (`lsp/mod.rs`'s
/// `document_symbols`, `lsp/quickfix.rs`'s `state_stub`) index into it
/// directly instead of reparsing the token stream. Past the parse, the
/// resolve stage ([`resolve_program`]) is the only source of a fatal, and
/// its non-fatal diagnostics ride alongside a clean resolve. Additive:
/// [`analyze`] and [`compile`] are unchanged, so a partial fatal a document
/// recovers from never leaks into the batch pipeline.
///
/// Consumed by the phase-7 `.tmc` language service, not by `compile()`.
pub(crate) fn analyze_staged(source: &str) -> TmcStagedAnalysis {
    analyze_staged_with(source, ExternalContracts::Stdlib)
}

/// [`analyze_staged`] with an explicit choice of the external modules whose
/// declared write contracts the footprint inference believes.
pub(crate) fn analyze_staged_with(source: &str, externals: ExternalContracts) -> TmcStagedAnalysis {
    let tokens = match lex_with(source, LexMode::WithComments) {
        Ok(tokens) => tokens,
        Err(fatal) => {
            return TmcStagedAnalysis {
                tokens: None,
                green: None,
                recovered_green: None,
                program: None,
                resolved: None,
                diagnostics: Vec::new(),
                fatal: Some(fatal),
            };
        }
    };
    let green = match parse_green_from_tokens(source, &tokens) {
        Ok(green) => green,
        Err(fatal) => {
            // A parse-stage fatal no longer costs the editor its tree:
            // the resilient parse (docs/core.md (syntax trees), error
            // recovery) wraps the broken regions in ERROR nodes and
            // keeps the rest — the green-tier features answer from the
            // CURRENT text. The fatal itself is unchanged (it equals
            // the resilient parse's first error by that entry's own
            // contract), and every later stage stays degraded exactly
            // as before.
            let resilient = crate::parser::parse_green_resilient(source, &tokens);
            return TmcStagedAnalysis {
                tokens: Some(tokens),
                green: None,
                recovered_green: Some(resilient.green),
                program: None,
                resolved: None,
                diagnostics: Vec::new(),
                fatal: Some(fatal),
            };
        }
    };
    let green_retained = Some(Rc::clone(&green));
    let program = crate::syntax::extract_program(&SyntaxNode::new_root(green), source);
    match resolve_program(&program, externals) {
        Ok((resolved, diagnostics)) => TmcStagedAnalysis {
            tokens: Some(tokens),
            green: green_retained,
            recovered_green: None,
            program: Some(program),
            resolved: Some(resolved),
            diagnostics,
            fatal: None,
        },
        Err(fatal) => TmcStagedAnalysis {
            tokens: Some(tokens),
            green: green_retained,
            recovered_green: None,
            program: Some(program),
            resolved: None,
            diagnostics: Vec::new(),
            fatal: Some(fatal),
        },
    }
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

/// The alphabet name each of a signature's tape params references.
fn tape_alphabet_refs(sig: &crate::parser::Signature) -> Vec<&str> {
    sig.params
        .iter()
        .filter_map(|p| match &p.kind {
            SigParamKind::Tape { alphabet, .. } => Some(alphabet.as_str()),
            SigParamKind::State => None,
        })
        .collect()
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
    states: &[State],
    grafts: &[Graft],
    binds: &[Bind],
    scopes: &Scopes,
    alphabets: &HashMap<String, ResolvedAlphabet>,
) -> Result<ResolvedWorld, CompileError> {
    // Tapes: from the signature's tape params (routine/graph).
    let mut tapes: Vec<ResolvedTape> = Vec::new();
    let mut state_params: Vec<String> = Vec::new();
    for p in &sig.params {
        match &p.kind {
            SigParamKind::Tape {
                alphabet,
                volatile,
                writes,
                preserves,
                ..
            } => {
                let (full, card) =
                    resolve_tape_alphabet(alphabet, p.name_span, ns, scopes, alphabets)?;
                let glyphs = alphabets
                    .get(&full)
                    .map(|a| a.glyphs.as_slice())
                    .expect("a resolved tape alphabet is in the table");
                tapes.push(ResolvedTape {
                    name: p.name.clone(),
                    name_span: p.name_span,
                    alphabet: full.clone(),
                    cardinality: card,
                    span: p.span,
                    volatile: *volatile,
                    writes: resolve_contract_clause(writes.as_ref(), "writes", glyphs, &full)?,
                    preserves: resolve_contract_clause(
                        preserves.as_ref(),
                        "preserves",
                        glyphs,
                        &full,
                    )?,
                });
            }
            SigParamKind::State => state_params.push(p.name.clone()),
        }
    }
    let (grafts, binds, entry) = resolve_world_reuse(grafts, binds, states, ns, scopes)?;
    let calls = resolve_world_calls(states, &binds, ns, scopes);
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
        calls,
    })
}

/// Resolve every `call` transition in a world's rules to a [`ResolvedCall`]
/// (structure only; arg + kind validation is `check_worlds`). A single-segment
/// target naming a world-local bind is a bind-call; everything else resolves
/// as a routine, carrying its raw binding args and an `external` flag for a
/// target this module does not define.
fn resolve_world_calls(
    states: &[State],
    binds: &[ResolvedBind],
    ns: &[String],
    scopes: &Scopes,
) -> Vec<ResolvedCall> {
    let bind_names: HashSet<&str> = binds.iter().map(|b| b.name.as_str()).collect();
    let mut calls = Vec::new();
    for s in states {
        for rule in &s.rules {
            if let Transition::Call {
                target,
                args,
                then,
                span,
            } = &rule.transition
            {
                let joined = target.joined();
                let resolved = if target.segments.len() == 1 && bind_names.contains(joined.as_str())
                {
                    ResolvedCallTarget::Bind { name: joined }
                } else {
                    let (name, external) = match scopes.resolve(&joined, ns) {
                        Some(r) => (r.full, r.kind != Some(DefKind::Routine)),
                        None => (joined, true),
                    };
                    ResolvedCallTarget::Routine {
                        name,
                        external,
                        args: args.clone(),
                    }
                };
                calls.push(ResolvedCall {
                    span: *span,
                    target: resolved,
                    then: then.clone(),
                });
            }
        }
    }
    calls
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
            volatile: t.volatile,
            // A machine tape declaration has no contract grammar: the clauses
            // live on signature parameters, where a caller can read them.
            writes: None,
            preserves: None,
        });
    }
    let (grafts, binds, entry) = resolve_world_reuse(&m.grafts, &m.binds, &m.states, &[], scopes)?;
    let calls = resolve_world_calls(&m.states, &binds, &[], scopes);
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
        calls,
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

/// Resolve one `writes`/`preserves` clause into a symbol-index set in its
/// tape's own alphabet frame. `None` in, `None` out — an absent clause is not
/// an empty one: it declares nothing, where `writes {}` declares that nothing
/// is written.
///
/// A clause body is the alphabet-body element grammar, so it resolves the same
/// way: a range expands by [`expand_range`] (inheriting its ascending and
/// single-scalar-endpoint rules) and every resulting label must name a symbol
/// of the alphabet. A repeat is harmless — a set absorbs it.
fn resolve_contract_clause(
    clause: Option<&ContractClause>,
    which: &'static str,
    glyphs: &[String],
    alphabet: &str,
) -> Result<Option<SymSet>, CompileError> {
    let Some(clause) = clause else {
        return Ok(None);
    };
    let mut set = SymSet::empty();
    let mut take = |label: String, span: Span| match glyphs.iter().position(|g| *g == label) {
        Some(index) => {
            set.insert(index as u32);
            Ok(())
        }
        None => Err(CompileError {
            span,
            kind: CompileErrorKind::ContractSymbolUnknown {
                glyph: label,
                clause: which,
                alphabet: alphabet.to_string(),
            },
        }),
    };
    for elem in &clause.elems {
        match elem {
            AlphabetElem::Single(s) => take(glyph_label(s), s.span())?,
            AlphabetElem::Range { lo, hi, span } => {
                for label in expand_range(lo, hi, *span)? {
                    take(label, *span)?;
                }
            }
        }
    }
    Ok(Some(set))
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
            // Imported-to-external or `::`-absolute — resolved at link.
            Some(r) if r.kind.is_none() => (r.full, true),
            // A resolved-but-wrong-kind LOCAL target (a graph/alphabet): kept
            // local (NOT external) so `check_target_kind` reports the
            // wrong-target-kind error, mirroring how the call path defers to
            // `check_call_like`. Flagging it external let it slip through as a
            // misleading bare `undeclared-external` warning instead.
            Some(r) => (r.full, false),
            None => (joined.clone(), true),
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

// ---------------------------------------------------------------------------
// compile() — the end-to-end driver. Mirrors the `.pmc` compiler's
// `compile()` field-for-field, with `.tma` text where PM-1 has `.pma`.
// ---------------------------------------------------------------------------

/// Which modules outside the compilation unit the write-footprint inference
/// may believe (docs/tmt/language.md (contract clauses)): a callee found in
/// one of them contributes its DECLARED effective set, projected through
/// the binding; a callee found nowhere contributes the whole alphabet. The
/// standard library is the default; its own analysis passes `None`, which
/// is also what keeps the once-per-process stdlib cache from initializing
/// itself recursively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExternalContracts {
    #[default]
    Stdlib,
    None,
}

impl ExternalContracts {
    pub(crate) fn modules(self) -> Vec<&'static Resolved> {
        match self {
            ExternalContracts::Stdlib => vec![crate::stdlib::resolved()],
            ExternalContracts::None => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompileOptions {
    /// Whose declared write contracts the footprint inference believes.
    pub externals: ExternalContracts,
    /// `-g`: record label/line debug info in the object, remapped to `.tmc`.
    pub debug_info: bool,
    /// `--strip-debugger`: drop `brk` at codegen. The optimizer runs BEFORE
    /// stripping, so the `brk` barrier always holds.
    pub strip_debugger: bool,
    /// `-O0` (default) or `-O1` (runs the optimizer pass pipeline).
    pub opt_level: OptLevel,
    /// Pass names to disable (`--fno-<pass>`).
    pub disabled_passes: Vec<String>,
    /// Capture per-stage IR snapshots (`--emit-ir=<stage>` backing): the
    /// `"lowered"` / `"final"` bookends plus `after:<pass>` for each pass that
    /// changed the IR.
    pub capture_ir: bool,
    /// `--foutline`: enable the default-OFF `outline` optimizer pass. Inert
    /// unless `-O1` is also set (the optimizer runs only at `-O1`).
    pub outline: bool,
    /// `--stamped-asm`: skip the `.rept` re-detection pass and emit the raw
    /// stamped assembly codegen produced (docs/tmt/cli.md (compile)). Implied
    /// when `-g` is set — the debug line map cannot survive the rewrite.
    pub stamped_asm: bool,
    /// Override `inline`'s rule-count cap (`None` = the shipped constant). A
    /// measurement knob for the optimizer sweep, not CLI surface — no flag,
    /// no completions entry.
    pub inline_cap: Option<usize>,
}

/// Structured stage report — `tmt -v` renders it; the library never prints
/// (the same thin-renderer rule as the linker's `LinkReport`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileReport {
    pub diagnostics: Vec<Diagnostic>,
    pub opt: OptReport,
}

/// One tape of the `machine` block: its source name and the glyphs of the
/// alphabet it draws from, in position order. The tape-block CLI reads this
/// to mint a block whose bands carry a program's real glyphs rather than
/// index labels (docs/tmt/cli.md (tape-block provenance)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeLayout {
    pub name: String,
    pub glyphs: Vec<String>,
}

/// Resolve `source` far enough to report the `machine` block's tape table,
/// in vector-position order. Analysis only — no expansion, lowering,
/// optimization, or codegen runs, so a program that fully compiles is not
/// required, only one that resolves.
///
/// `Ok(None)` means the source declares no `machine` block: a library takes
/// its tapes from each routine's signature, so there is no single band to
/// describe. That is a legitimate source, not a compile error, so the caller
/// decides whether it can proceed (docs/tmt/cli.md (tape-block provenance)).
pub fn machine_tape_layout(source: &str) -> Result<Option<Vec<TapeLayout>>, CompileError> {
    let analysis = analyze(source)?;
    let resolved = &analysis.resolved;
    let Some(index) = resolved.entry_world else {
        return Ok(None);
    };
    let layout = resolved.worlds[index]
        .tapes
        .iter()
        .map(|tape| {
            let alphabet = resolved
                .alphabets
                .get(&tape.alphabet)
                .expect("resolution guarantees every tape's alphabet exists");
            TapeLayout {
                name: tape.name.clone(),
                glyphs: alphabet.glyphs.clone(),
            }
        })
        .collect();
    Ok(Some(layout))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileOutput {
    pub object: ObjectFile,
    /// The generated assembly (`-S` output). The object is assembled from
    /// exactly this text, so the code bytes can never disagree; under `-g`
    /// the object's debug LINES are additionally remapped to `.tmc` sources.
    pub tma: String,
    /// The FINAL IR (post-optimizer at `-O1`; the lowered IR at `-O0`, where
    /// the optimizer is skipped and the two coincide).
    pub ir: IrProgram,
    /// Per-stage IR snapshots when `capture_ir` was set; empty otherwise.
    pub ir_snapshots: Vec<(String, IrProgram)>,
    pub report: CompileReport,
}

/// `.tmc` source → object file: analyze → expand → lower → validate →
/// optimize → emit `.tma` → assemble. Diagnostics accumulate in pipeline
/// order (analyze's, then expansion's, then IR lowering's).
///
/// Two failure modes report as [`CompileErrorKind::Internal`] — both are
/// compiler bugs, never user errors: an IR world the compiler built failing
/// [`validate_world`] (the T6 invariant check, run here where `.pmc` runs
/// `validate_function`), and the generated `.tma` failing to assemble.
pub fn compile(source: &str, options: CompileOptions) -> Result<CompileOutput, CompileError> {
    let mut analysis = analyze_with(source, options.externals)?;
    // Drop rules a second catch-all shadows before expansion (docs/tmt/
    // language.md (rules)) so codegen never emits two all-wildcard match rows.
    // Done here, on the compile path only — the language service and the batch
    // lint keep the full source rules (the `dead-rule` lint reports them
    // instead).
    let mut unreachable_diags = Vec::new();
    drop_unreachable_rules(&mut analysis.resolved, &mut unreachable_diags);
    let expanded = crate::expand::expand(&analysis.resolved)?;
    let (mut ir, ir_warnings) = lower(&expanded, &analysis.resolved)?;

    // Validate every compiler-produced world before codegen relies on the
    // invariants (dense ids, in-bounds indices, arity-wide rows, traps only on
    // synthesized rows). A failure here is an internal error.
    validate_ir(&ir)?;

    let mut ir_snapshots = Vec::new();
    if options.capture_ir {
        ir_snapshots.push(("lowered".to_string(), ir.clone()));
    }
    let opt = optimize(
        &mut ir,
        &OptOptions {
            level: options.opt_level,
            disabled: options.disabled_passes.iter().cloned().collect(),
            capture: options.capture_ir,
            outline: options.outline,
            inline_cap: options.inline_cap,
        },
        &mut ir_snapshots,
    );
    if options.capture_ir {
        ir_snapshots.push(("final".to_string(), ir.clone()));
    }

    let tma = emit_program(
        &ir,
        CodegenOptions {
            strip_debugger: options.strip_debugger,
        },
    );
    let assemble = |text: &str| {
        crate::asm::assemble(text, options.debug_info).map_err(|e| CompileError {
            span: Span::point(0, 0),
            kind: CompileErrorKind::Internal(format!("generated .tma failed to assemble: {e}")),
        })
    };
    // Fold arithmetic families in the codegen text back into `.rept` loops for
    // the `-S` artifact (docs/tmt/cli.md (compile)), reusing the object the
    // emitter assembled for its self-check so no third assemble happens. Skip
    // it under `--stamped-asm`, and under `-g` — the codegen debug map is keyed
    // by stamped physical lines, so the rewrite cannot preserve it; those paths
    // assemble the stamped text exactly as before.
    let (tma_text, mut object) = if options.stamped_asm || options.debug_info {
        let object = assemble(&tma.text)?;
        (tma.text, object)
    } else {
        let (text, _report, reused) =
            crate::rept_emit::compress_asm_with_object(&tma.text, &crate::asm::tm1_syntax());
        let object = match reused {
            Some(o) => o,
            None => assemble(&text)?,
        };
        (text, object)
    };
    if options.debug_info {
        remap_debug_lines(&mut object, &tma.line_map);
    }

    let mut diagnostics = analysis.diagnostics;
    diagnostics.extend(unreachable_diags);
    diagnostics.extend(expanded.diagnostics);
    diagnostics.extend(ir_warnings);

    Ok(CompileOutput {
        object,
        tma: tma_text,
        ir,
        ir_snapshots,
        report: CompileReport { diagnostics, opt },
    })
}

/// Run [`validate_world`] over every world of a compiler-produced IR,
/// wrapping any failure as an [`CompileErrorKind::Internal`] — the T6
/// invariant gate `compile()` runs before codegen (the `.pmc`
/// `validate_function` analog). Valid compiler output always passes; a
/// failure means an upstream stage broke an invariant.
fn validate_ir(ir: &IrProgram) -> Result<(), CompileError> {
    for w in &ir.worlds {
        validate_world(w).map_err(|m| CompileError {
            span: Span::point(0, 0),
            kind: CompileErrorKind::Internal(format!("IR validation failed: {m}")),
        })?;
    }
    Ok(())
}

/// The assembler recorded `(code_offset, tma_line)`; compose with the
/// codegen's `(tma_line, tmc_line)` map so debug info speaks `.tmc`. Offsets
/// with no source correspondence are dropped. Mirrors the `.pmc` remap.
fn remap_debug_lines(object: &mut ObjectFile, line_map: &[(u32, u32)]) {
    let to_tmc: HashMap<u32, u32> = line_map.iter().copied().collect();
    if let Some(per_blob) = &mut object.debug {
        for d in per_blob {
            d.lines = d
                .lines
                .iter()
                .filter_map(|&(off, tma_line)| to_tmc.get(&tma_line).map(|&l| (off, l)))
                .collect();
        }
    }
}

/// The mutable context threaded through the world-boundary checks.
struct WorldCtx<'a> {
    scopes: &'a Scopes,
    imports_used: Vec<bool>,
    warned_undeclared: HashSet<String>,
    diagnostics: Vec<Diagnostic>,
}

impl WorldCtx<'_> {
    /// Run every per-world check across all worlds, in source order.
    fn check_worlds(&mut self, program: &Program, resolved: &Resolved) -> Result<(), CompileError> {
        // Mark import usage for every reference (tape alphabets, call /
        // graft / bind targets) — `resolve_module` had no mutable context, so
        // usage is tallied here over the AST, which still carries the original
        // (pre-mangling) reference names.
        self.mark_reference_imports(program);
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

    fn mark_reference_imports(&mut self, program: &Program) {
        let mark = |name: &str, ns: &[String], ctx: &mut Self| {
            if let Some(r) = ctx.scopes.resolve(name, ns)
                && let Some(idx) = r.via_import
            {
                ctx.imports_used[idx] = true;
            }
        };
        // World-body references share one shape across routine/graph/machine.
        let mark_world = |sig_alphas: &[&str],
                          states: &[State],
                          grafts: &[Graft],
                          binds: &[Bind],
                          ns: &[String],
                          ctx: &mut Self| {
            for a in sig_alphas {
                mark(a, ns, ctx);
            }
            for s in states {
                for rule in &s.rules {
                    if let Transition::Call { target, .. } = &rule.transition {
                        mark(&target.joined(), ns, ctx);
                    }
                }
            }
            for g in grafts {
                mark(&g.target.joined(), ns, ctx);
            }
            for b in binds {
                mark(&b.target.joined(), ns, ctx);
            }
        };
        for r in &program.routines {
            let alphas: Vec<&str> = tape_alphabet_refs(&r.sig);
            mark_world(&alphas, &r.states, &r.grafts, &r.binds, &r.ns, self);
        }
        for g in &program.graphs {
            let alphas: Vec<&str> = tape_alphabet_refs(&g.sig);
            mark_world(&alphas, &g.states, &g.grafts, &g.binds, &g.ns, self);
        }
        if let Some(m) = &program.machine {
            let alphas: Vec<&str> = m.tapes.iter().map(|t| t.alphabet.as_str()).collect();
            mark_world(&alphas, &m.states, &m.grafts, &m.binds, &[], self);
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

    /// Exactly one `entry` per world.
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
                    // An omitted transition is a self-goto — the current state
                    // is always a valid target, so there is nothing to check.
                    Transition::Stay { .. } => {}
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
                        // The bind is already fully bound; arguments on the
                        // call are a contradiction. Point at the first one.
                        if let Some(first) = args.first() {
                            return Err(CompileError {
                                span: first.span,
                                kind: CompileErrorKind::BindCallArgs(joined),
                            });
                        }
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
                g.target_span,
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
                b.target_span,
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
        span: Span,
        want: DefKind,
        expected_noun: &'static str,
        states: &HashSet<&str>,
        tapes: &HashSet<&str>,
        ns: &[String],
    ) -> Result<(), CompileError> {
        // Import usage is tallied centrally (`mark_reference_imports`); this
        // pass only validates target kind + binding args and warns.
        match self.scopes.resolve(joined, ns) {
            Some(r) if r.kind == Some(want) => {
                self.check_binding_args(joined, &r.full, args, want, states, tapes, span)
            }
            Some(r) if r.kind.is_some() => Err(CompileError {
                span: target.span,
                kind: CompileErrorKind::WrongTargetKind {
                    name: joined.to_string(),
                    expected: expected_noun,
                },
            }),
            Some(_) => {
                // Absolute-external, or imported-to-external routine — allowed,
                // resolved at link; no arg check (no local signature).
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
    /// the graft/range expander's — this only checks the kind.
    ///
    /// Also the aliasing check (docs/tmt/language.md (reuse)): within ONE
    /// argument list no two TAPE parameters may name the same caller tape.
    /// One physical head cannot serve two callee tapes read and written
    /// through two independent maps, and the lowering mechanisms disagree
    /// about what it would mean. State parameters are exempt — two
    /// continuations legitimately share one target state. Every `call`,
    /// `graft`, and `bind` funnels through here, so one check covers all
    /// three; a `.tmc` bound call always has a local signature to check
    /// against, since binding tapes into another compilation unit is
    /// rejected outright (`external-binding-unsupported`).
    #[allow(clippy::too_many_arguments)]
    fn check_binding_args(
        &self,
        _target_desc: &str,
        sig_key: &str,
        args: &[BindingArg],
        want: DefKind,
        states: &HashSet<&str>,
        tapes: &HashSet<&str>,
        // Where a MissingArg points when the call/graft/bind is argless: the
        // call/graft/bind site itself (there is no first arg to blame).
        fallback_span: Span,
    ) -> Result<(), CompileError> {
        let _ = want;
        let Some(sig) = self.scopes.sigs.get(sig_key) else {
            return Ok(());
        };
        // arg name -> param kind, with duplicate + unknown detection.
        let mut arg_seen: HashSet<&str> = HashSet::new();
        // Caller tape target -> the argument that claimed it. Scoped to this
        // one argument list: the same tape in two DIFFERENT calls is legal.
        let mut tape_seen: HashMap<&str, &str> = HashMap::new();
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
            // Kind first: a target that is not a tape at all is that error,
            // not an alias.
            self.check_arg_kind(a, *kind, states, tapes)?;
            if *kind == ParamKind::Tape
                && let BindingValue::Named { target, .. } = &a.value
                && let Some(first) = tape_seen.insert(target.as_str(), a.name.as_str())
            {
                return Err(CompileError {
                    span: a.span,
                    kind: CompileErrorKind::DuplicateTapeTarget {
                        first: first.to_string(),
                        second: a.name.clone(),
                        target: target.clone(),
                    },
                });
            }
        }
        // Every parameter must be bound.
        for (pname, _) in &sig.params {
            if !arg_seen.contains(pname.as_str()) {
                // Point at the first arg, or the call/graft/bind site when
                // there are none to blame.
                let span = args.first().map(|a| a.span).unwrap_or(fallback_span);
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

/// The name inside the first backtick pair of an `undeclared-external`
/// message — this function's own fixed format ("reference to undeclared
/// external `NAME` — declare it with `use NAME;`"), pinned by
/// `undeclared_name_matches_the_warning_format` below.
pub(crate) fn undeclared_name(message: &str) -> Option<&str> {
    let start = message.find('`')? + 1;
    let rest = &message[start..];
    Some(&rest[..rest.find('`')?])
}

/// The build driver and the language server refine this warning the same
/// way wherever a full link set is declared: a bare reference the
/// declared set defines stops warning (docs/tmt/cli.md
/// (undeclared-external)).
pub(crate) fn refine_undeclared(diags: &mut Vec<Diagnostic>, defined: &HashSet<String>) {
    diags.retain(|d| {
        !(d.code == "undeclared-external"
            && undeclared_name(&d.message).is_some_and(|n| defined.contains(n)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    // `analyze` no longer calls either directly: `lex` derives the
    // comment-free token stream a few tests compare `analyze`'s filtered
    // `WithComments` stream against, and `parser::parse` — itself now a
    // source-in green-tree convenience wrapper — is the plain entry point
    // checked for agreement with the tiered `analyze_staged` path.
    use crate::lexer::lex;
    use crate::parser::parse;
    use proptest::prelude::*;

    /// Every `CompileErrorKind` code is a stable kebab identifier, and no two
    /// variants share one — the CLI and the language server key on these.
    /// One representative of each variant is listed here, in declaration
    /// order. The `code_registry!` expansion already ties every variant to
    /// a `CODES` row structurally (one list feeds both the match and the
    /// table); this witness list additionally proves the rows come out in
    /// declaration order and stay pairwise distinct.
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
            CompileErrorKind::ContractClauseOrder,
            CompileErrorKind::DuplicateContractClause { what: "writes" },
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
            CompileErrorKind::DuplicateTapeTarget {
                first: "p".into(),
                second: "q".into(),
                target: "t".into(),
            },
            CompileErrorKind::BindCallArgs("x".into()),
            CompileErrorKind::ContractSymbolUnknown {
                glyph: "x".into(),
                clause: "writes",
                alphabet: "a".into(),
            },
            CompileErrorKind::WritesOutsideContract {
                world: "w".into(),
                tape: "t".into(),
                glyphs: vec!["x".into()],
            },
            CompileErrorKind::GraftCycle("x".into()),
            CompileErrorKind::GraftCallUnsupported("x".into()),
            CompileErrorKind::MapSymbolNotInAlphabet("x".into()),
            CompileErrorKind::MapBlankPin,
            CompileErrorKind::MapConflict { symbol: "x".into() },
            CompileErrorKind::MapNotInjective { symbol: "x".into() },
            CompileErrorKind::IdentityGlyphMismatch,
            CompileErrorKind::FoldOutOfAlphabet("x".into()),
            CompileErrorKind::FoldZeroModulus,
            CompileErrorKind::FoldNegativeRemainder { hint_modulus: None },
            CompileErrorKind::FoldOverflow,
            CompileErrorKind::ExactRowConflict {
                first: "x".into(),
                second: "y".into(),
            },
            CompileErrorKind::RowWidth {
                expected: 2,
                got: 3,
            },
            CompileErrorKind::ExternalBindingUnsupported("x".into()),
            CompileErrorKind::StateParamContinuationUnsupported("x".into()),
            CompileErrorKind::Internal("x".into()),
        ];
        let witnessed: Vec<&str> = all.iter().map(|k| k.code()).collect();
        assert_eq!(
            witnessed,
            CompileErrorKind::CODES,
            "the witness list and the CODES registry disagree"
        );
        let mut codes = witnessed.clone();
        codes.sort_unstable();
        let mut deduped = codes.clone();
        deduped.dedup();
        assert_eq!(codes, deduped, "duplicate CompileErrorKind code: {codes:?}");
        // Every code is non-empty kebab-case (ascii lowercase + digits +
        // interior hyphens).
        for c in CompileErrorKind::CODES {
            assert!(
                !c.is_empty()
                    && !c.starts_with('-')
                    && !c.ends_with('-')
                    && !c.contains("--")
                    && c.chars()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'),
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
        // The `call plusOne(…)` resolved to the mangled routine (via import).
        assert_eq!(machine.calls.len(), 1);
        match &machine.calls[0].target {
            ResolvedCallTarget::Routine { name, external, .. } => {
                assert_eq!(name, "mylib::plusOne");
                assert!(!external);
            }
            other => panic!("expected a routine call, got {other:?}"),
        }
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
        // One entry state + one entry graft in the same world is still two.
        let e = err(
            "alphabet b { '_', '0' }\ngraph g(tape t: b) { entry state s { [*] -> stop; } }\nmachine { tape t: b; entry state a { [*] -> stop; } entry graft g(t = t) as x; }",
        );
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
        // goto a bind name — a call target, not a state.
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
    fn bind_target_must_be_a_routine() {
        // bind → a LOCAL graph: wrong-target-kind, NOT a misleading external
        // warning. The error points at the bind target `g` on line 3.
        let src = "alphabet b { '_', '0' }\ngraph g(tape t: b) { entry state s { [*] -> stop; } }\nmachine { tape t: b; bind g(t = t) as x; entry state s { [*] -> call x() then s; } }";
        let e = err(src);
        assert_eq!(e.kind.code(), "wrong-target-kind");
        assert_eq!(e.span.start.line, 3);
        assert!(matches!(e.kind, CompileErrorKind::WrongTargetKind { name, .. } if name == "g"));
        // bind → a LOCAL alphabet: also wrong-target-kind.
        let src = "alphabet b { '_', '0' }\nmachine { tape t: b; bind b(t = t) as x; entry state s { [*] -> call x() then s; } }";
        let e = err(src);
        assert_eq!(e.kind.code(), "wrong-target-kind");
        assert!(matches!(e.kind, CompileErrorKind::WrongTargetKind { name, .. } if name == "b"));
        // bind → an imported (genuine external) routine: no error, no warning
        // (resolved at link — the `::` name is not a bare undeclared).
        let src = "alphabet b { '_', '0' }\nuse lib::helper;\nmachine { tape t: b; bind helper(t = t) as h; entry state s { [*] -> call h() then s; } }";
        let a = ok(src);
        assert!(
            !a.diagnostics
                .iter()
                .any(|d| d.code == "undeclared-external"),
            "{:?}",
            a.diagnostics
        );
        // bind → an undeclared BARE name: undeclared-external (today's behavior).
        let src = "alphabet b { '_', '0' }\nmachine { tape t: b; bind ghost(t = t) as h; entry state s { [*] -> call h() then s; } }";
        assert!(diag_codes(src).contains(&"undeclared-external"));
    }

    /// Pins the extraction against this module's REAL warning format — if
    /// the message ever changes shape, this fails here rather than
    /// silently breaking the refinement (moved from `cli/driver.rs`, which
    /// now delegates to `refine_undeclared` here).
    #[test]
    fn undeclared_name_matches_the_warning_format() {
        let src = "alphabet ab { '_', 'a' }\nmachine { tape t: ab; entry state s { [*] -> call go() then s; } }";
        let out = compile(src, CompileOptions::default()).unwrap();
        let diag = out
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == "undeclared-external")
            .expect("bare call go() warns");
        assert_eq!(undeclared_name(&diag.message), Some("go"));
    }

    #[test]
    fn refine_undeclared_drops_only_defined_undeclared_externals() {
        // Both diagnostics come from a real compile — not hand-typed
        // strings — so this test cannot silently drift away from the
        // compiler's actual message shapes the way a copied literal could.
        let src = "alphabet bits { '_', '1' }\nmachine { tape t: bits; entry state s { ['_'] -> call a() then g; ['1'] -> call b() then g; } state g { [*] -> stop; } }";
        let out = compile(src, CompileOptions::default()).unwrap();
        let mut diags = out.report.diagnostics;
        assert_eq!(
            diags
                .iter()
                .filter(|d| d.code == "undeclared-external")
                .count(),
            2,
            "both bare calls should warn undeclared: {diags:?}"
        );

        let defined: HashSet<String> = ["a".to_string()].into_iter().collect();
        refine_undeclared(&mut diags, &defined);

        assert!(
            !diags
                .iter()
                .any(|d| d.code == "undeclared-external" && d.message.contains("`a`")),
            "{diags:?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == "undeclared-external" && d.message.contains("`b`")),
            "b stays undeclared — the live positive control: {diags:?}"
        );
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

    // -- aliased tape arguments ---------------------------------------------

    /// The aliasing rule (docs/tmt/language.md (reuse)): within ONE argument
    /// list two tape parameters may not name the same caller tape. This is
    /// the source-level half; the linker backstops hand-assembled objects.
    ///
    /// The `call` fixture is a three-tape callee whose `p` and `q` both bind
    /// machine tape `t` through different maps across unequal alphabets — the
    /// shape that made mono and frames disagree (one projects each callee
    /// tape through its own map at run time; the other writes per-position
    /// expectations into a stamped row, where the second projection
    /// overwrites the first).
    #[test]
    fn two_tape_arguments_naming_one_caller_tape_are_rejected() {
        let call = "\
alphabet outer { '_', 'a', 'b', 'c' }
alphabet inner { '_', 'y', 'x' }
routine callee(tape p: inner, tape q: inner, tape r: inner) {
  entry state go {
    ['y', 'x', 'y'] -> move [., ., >] return;
    [*, *, *]       -> return;
  }
}
machine {
  tape t: outer;
  tape u: inner;
  entry state start {
    [*, *] -> call callee(p = t with map { 'a' -> 'y' },
                          q = t with map { 'b' -> 'x' },
                          r = u) then fin;
  }
  state fin { [*, *] -> stop; }
}
";
        let e = err(call);
        assert_eq!(e.kind.code(), "duplicate-tape-target");
        assert_eq!(
            e.kind.to_string(),
            "binding arguments `p` and `q` both bind tape `t` — \
             one caller tape cannot back two callee tapes"
        );
        // The second argument is what is blamed, not the call site.
        assert_eq!(e.span.start.line, 14);

        // A graft aliases through the same funnel.
        let graft = "\
alphabet b { '_', '0' }
graph g(tape x: b, tape y: b, state done) {
  entry state s { ['0', '0'] -> done; [*, *] -> move [>, >] goto s; }
}
machine {
  tape t: b;
  tape u: b;
  entry graft g(x = t, y = t, done = celebrate) as seek;
  state celebrate { [*, *] -> stop; }
}
";
        assert_eq!(code(graft), "duplicate-tape-target");

        // So does a `bind` declaration.
        let bind = "\
alphabet b { '_', '0' }
routine r(tape x: b, tape y: b) { entry state s { [*, *] -> return; } }
machine {
  tape t: b;
  tape u: b;
  bind r(x = t, y = t) as h;
  entry state s { [*, *] -> call h() then fin; }
  state fin { [*, *] -> stop; }
}
";
        assert_eq!(code(bind), "duplicate-tape-target");

        // And a routine aliasing its OWN parameter into a nested call — the
        // shape that would otherwise smuggle an alias in transitively.
        let nested = "\
alphabet b { '_', '0' }
routine inner(tape x: b, tape y: b) { entry state s { [*, *] -> return; } }
routine outer(tape a: b, tape c: b) {
  entry state s { [*, *] -> call inner(x = a, y = a) then return; }
}
machine {
  tape t: b;
  tape u: b;
  entry state s { [*, *] -> call outer(a = t, c = u) then fin; }
  state fin { [*, *] -> stop; }
}
";
        assert_eq!(code(nested), "duplicate-tape-target");
    }

    /// The rule is per argument list, and only about TAPE parameters.
    /// Everything else the wording permits must keep compiling — a seen-set
    /// hoisted out of the call scope, or one that ignores the parameter
    /// kind, would fail here while the negative tests above still passed.
    #[test]
    fn the_aliasing_rule_is_scoped_to_one_argument_list_and_to_tape_params() {
        // Same tape, two DIFFERENT calls.
        ok("\
alphabet b { '_', '0' }
routine r(tape x: b) { entry state s { [*, *] -> return; } }
machine {
  tape t: b;
  tape u: b;
  entry state s { [*, *] -> call r(x = t) then two; }
  state two   { [*, *] -> call r(x = t) then fin; }
  state fin   { [*, *] -> stop; }
}
");
        // Different tapes in ONE call — the Appendix-B callee, unaliased.
        ok("\
alphabet b { '_', '0' }
routine r(tape x: b, tape y: b) { entry state s { [*, *] -> return; } }
machine {
  tape t: b;
  tape u: b;
  entry state s { [*, *] -> call r(x = t, y = u) then fin; }
  state fin { [*, *] -> stop; }
}
");
        // Two STATE parameters sharing one continuation: legal, and must not
        // be mistaken for an alias.
        ok("\
alphabet b { '_', '0' }
graph g(tape x: b, state hit, state miss) {
  entry state s { ['0', *] -> hit; [*, *] -> miss; }
}
machine {
  tape t: b;
  tape u: b;
  entry graft g(x = t, hit = done, miss = done) as seek;
  state done { [*, *] -> stop; }
}
");
    }

    #[test]
    fn missing_arg_on_an_argless_call_points_at_the_call() {
        // `helper` needs one tape arg; the call supplies none. The MissingArg
        // must point at the call site (line 3), not the bogus (1,1) fallback.
        let src = "alphabet b { '_', '0' }\nroutine helper(tape t: b) { entry state s { [*] -> return; } }\nmachine { tape t: b; entry state s { [*] -> call helper() then s; } }";
        let e = err(src);
        assert_eq!(e.kind.code(), "missing-arg");
        assert_eq!(e.span.start.line, 3);
    }

    // -- declared write contracts -------------------------------------------

    #[test]
    fn contract_clauses_resolve_to_index_sets_in_the_tapes_own_frame() {
        let a = ok("\
alphabet bits { '_', '0', '1' }
routine r(tape t: bits writes {'0'..'1'} preserves {'_'}) {
  entry state s { [*] -> return; }
}
");
        let world = a.resolved.worlds.iter().find(|w| w.name == "r").unwrap();
        let tape = &world.tapes[0];
        // A clause range expands exactly as an alphabet body's does — by
        // scalar succession — and lands as positions in the tape's alphabet.
        assert_eq!(
            tape.writes.unwrap().iter().collect::<Vec<_>>(),
            vec![1, 2],
            "`'0'..'1'` is positions 1 and 2 of `{{'_', '0', '1'}}`"
        );
        assert_eq!(tape.preserves.unwrap().iter().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn a_tape_with_no_clause_carries_no_contract() {
        let a = ok("\
alphabet bits { '_', '0' }
routine r(tape t: bits) { entry state s { [*] -> return; } }
machine { tape m: bits; entry state s { [*] -> stop; } }
");
        let routine = a.resolved.worlds.iter().find(|w| w.name == "r").unwrap();
        assert!(routine.tapes[0].writes.is_none());
        assert!(routine.tapes[0].preserves.is_none());
        // A machine tape declaration has no clause grammar at all, so the
        // machine world's tapes are unconditionally uncontracted.
        let machine = a.resolved.worlds.iter().find(|w| w.name == "main").unwrap();
        assert!(machine.tapes[0].writes.is_none());
        assert!(machine.tapes[0].preserves.is_none());
    }

    #[test]
    fn a_satisfied_writes_contract_compiles() {
        ok("\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits writes {'0','1'}) {
  entry state s { [*] -> write ['1'] return; }
}
");
    }

    #[test]
    fn a_violated_writes_contract_names_the_world_the_glyph_and_the_tape() {
        let e = err("\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits writes {'0'}) {
  entry state s { [*] -> write ['1'] return; }
}
");
        assert_eq!(e.kind.code(), "writes-outside-contract");
        assert_eq!(
            e.kind.to_string(),
            "`mark` may write '1' on tape `t`, which its contract forbids"
        );
        // The finding lands on the PARAMETER that declared the contract, not
        // on the rule that writes.
        assert_eq!(e.span.start.line, 2);
    }

    #[test]
    fn a_caller_of_a_contracted_library_routine_may_declare_its_own_writes() {
        // `std::binaryNumbers::goToNumbersStart` declares `writes {}`, so a
        // caller reaching it transparently writes only what its own body
        // writes — and may say so. Before the inference believed external
        // contracts, the library call answered with the whole alphabet and
        // no caller through the library could declare anything narrower.
        ok("\
alphabet bin { '_', '^', '$', '0', '1' }
routine setZero(tape n: bin writes { '$' }) {
  entry state start { [*] -> call std::binaryNumbers::goToNumbersStart() then put; }
  state put         { [*] -> write ['$'] return; }
}
");
    }

    #[test]
    fn a_violated_preserves_contract_errors() {
        let e = err("\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits preserves {'1'}) {
  entry state s { [*] -> write ['1'] return; }
}
");
        assert_eq!(e.kind.code(), "writes-outside-contract");
        assert_eq!(
            e.kind.to_string(),
            "`mark` may write '1' on tape `t`, which its contract forbids"
        );
    }

    #[test]
    fn preserves_subtracts_from_writes_where_the_two_overlap() {
        // `'1'` in BOTH clauses is redundancy, not an error — and `preserves`
        // wins the subtraction, so a body writing it violates the contract.
        let overlapping = "\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits writes {'0','1'} preserves {'1'}) {
  entry state s { [*] -> write ['1'] return; }
}
";
        assert_eq!(code(overlapping), "writes-outside-contract");
        // The same declaration over a body writing only `'0'` is satisfied.
        ok("\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits writes {'0','1'} preserves {'1'}) {
  entry state s { [*] -> write ['0'] return; }
}
");
    }

    #[test]
    fn an_empty_writes_clause_forbids_every_write() {
        ok("\
alphabet bits { '_', '0', '1' }
routine walk(tape t: bits writes {}) {
  entry state s { [*] -> move [>] return; }
}
");
        assert_eq!(
            code(
                "\
alphabet bits { '_', '0', '1' }
routine walk(tape t: bits writes {}) {
  entry state s { [*] -> write ['0'] return; }
}
"
            ),
            "writes-outside-contract"
        );
    }

    #[test]
    fn every_forbidden_glyph_is_named_ascending() {
        let e = err("\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits writes {'_'}) {
  entry state s {
    ['0'] -> write ['1'] return;
    [*]   -> write ['0'] return;
  }
}
");
        assert_eq!(
            e.kind.to_string(),
            "`mark` may write '0', '1' on tape `t`, which its contract forbids"
        );
    }

    #[test]
    fn an_unknown_glyph_in_a_contract_clause_is_rejected_per_clause() {
        let e = err("\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits writes {'x'}) {
  entry state s { [*] -> return; }
}
");
        assert_eq!(e.kind.code(), "contract-symbol-unknown");
        assert_eq!(
            e.kind.to_string(),
            "'x' in the `writes` clause is not a symbol of alphabet `bits`"
        );
        let e = err("\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits preserves {'x'}) {
  entry state s { [*] -> return; }
}
");
        assert_eq!(e.kind.code(), "contract-symbol-unknown");
        assert_eq!(
            e.kind.to_string(),
            "'x' in the `preserves` clause is not a symbol of alphabet `bits`"
        );
    }

    #[test]
    fn the_unknown_glyph_message_names_the_resolved_alphabet() {
        // The clause resolves against the alphabet the tape actually draws
        // from, so the message names it by its mangled name — the identity
        // `ResolvedTape.alphabet` carries — not the reference as written.
        let e = err("\
namespace lib {
  export alphabet bits { '_', '0', '1' }
  export routine mark(tape t: bits writes {'x'}) {
    entry state s { [*] -> return; }
  }
}
");
        assert_eq!(
            e.kind.to_string(),
            "'x' in the `writes` clause is not a symbol of alphabet `lib::bits`"
        );
    }

    #[test]
    fn a_contract_in_a_namespace_is_checked_under_its_mangled_name() {
        // The footprint table is keyed by MANGLED world name, and so is the
        // lookup that reads it. A namespaced world is the only shape where the
        // two names differ, so it is the only shape that can catch the keying
        // drifting apart — everything top-level would pass either way.
        let e = err("\
namespace lib {
  export alphabet bits { '_', '0', '1' }
  export routine mark(tape t: bits writes {'0'}) {
    entry state s { [*] -> write ['1'] return; }
  }
}
");
        assert_eq!(e.kind.code(), "writes-outside-contract");
        assert_eq!(
            e.kind.to_string(),
            "`lib::mark` may write '1' on tape `t`, which its contract forbids"
        );
    }

    #[test]
    fn a_contract_on_a_graph_parameter_is_checked() {
        // A graph is an inferred world like any other — its signature tapes
        // take contracts and the same check applies.
        let e = err("\
alphabet bits { '_', '0', '1' }
graph g(tape t: bits writes {'0'}, state done) {
  entry state s { ['0'] -> done; [*] -> write ['1'] goto s; }
}
");
        assert_eq!(e.kind.code(), "writes-outside-contract");
        assert_eq!(
            e.kind.to_string(),
            "`g` may write '1' on tape `t`, which its contract forbids"
        );
    }

    #[test]
    fn a_contract_sees_what_a_call_writes_back_through_its_map() {
        // `outer` writes nothing itself; `inner` writes `'b'`, which the
        // binding maps back onto the caller's `'1'`. The inference is
        // transitive, so the contract sees it.
        let e = err("\
alphabet bits  { '_', '0', '1' }
alphabet marks { '_', 'a', 'b' }

routine inner(tape u: marks) {
  entry state s { [*] -> write ['b'] return; }
}

routine outer(tape t: bits writes {'0'}) {
  entry state s { [*] -> call inner(u = t with map { '0'->'a', '1'->'b' }) then done; }
  state done { [*] -> return; }
}
");
        assert_eq!(e.kind.code(), "writes-outside-contract");
        assert_eq!(
            e.kind.to_string(),
            "`outer` may write '1' on tape `t`, which its contract forbids"
        );
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
    fn graft_and_bind_targets_reached_via_import_mark_the_import_used() {
        // A graph imported and grafted, plus a routine imported and bound —
        // neither reference should leave the import looking unused.
        let src = "\
alphabet b { '_', '0', '1' }
namespace lib {
  export graph g(tape t: b, state done) { entry state s { ['0'] -> done; [*] -> move [>] goto s; } }
  export routine r(tape t: b) { entry state s { [*] -> return; } }
}
use lib::g;
use lib::r;
machine {
  tape t: b;
  bind r(t = t) as rr;
  entry graft g(t = t, done = fin) as x;
  state fin { [*] -> stop; }
}
";
        let a = ok(src);
        assert!(
            a.diagnostics.iter().all(|d| d.code != "unused-import"),
            "{:?}",
            a.diagnostics
        );
    }

    #[test]
    fn a_call_on_a_bind_name_resolves_to_a_bind_call() {
        let src = "alphabet b { '_', '0' }\nroutine helper(tape t: b) { entry state s { [*] -> return; } }\nmachine { tape t: b; bind helper(t = t) as h; entry state s { [*] -> call h() then s; } }";
        let a = ok(src);
        let machine = a
            .resolved
            .worlds
            .iter()
            .find(|w| w.kind == WorldKind::Machine)
            .unwrap();
        assert_eq!(machine.binds.len(), 1);
        assert_eq!(machine.binds[0].target, "helper");
        assert_eq!(machine.calls.len(), 1);
        assert!(matches!(
            &machine.calls[0].target,
            ResolvedCallTarget::Bind { name } if name == "h"
        ));
    }

    #[test]
    fn a_call_on_a_bind_passes_no_arguments() {
        // A bind is already fully bound — arguments on the call are a
        // contradiction. The error points at the offending arg on line 3.
        let src = "alphabet b { '_', '0' }\nroutine helper(tape t: b) { entry state s { [*] -> return; } }\nmachine { tape t: b; bind helper(t = t) as h; entry state s { [*] -> call h(x = t) then s; } }";
        let e = err(src);
        assert_eq!(e.kind.code(), "bind-call-args");
        assert_eq!(e.span.start.line, 3);
        assert!(matches!(e.kind, CompileErrorKind::BindCallArgs(n) if n == "h"));
    }

    #[test]
    fn a_bare_name_resolves_innermost_out() {
        // `foo` is a routine at the top level AND inside `lib`; a call to
        // `foo` from within `lib::caller` binds the INNER `lib::foo`, never
        // the top-level one (the scope walk is innermost-out).
        let src = "\
alphabet b { '_', '0' }
routine foo(tape t: b) { entry state s { [*] -> return; } }
namespace lib {
  routine foo(tape t: b) { entry state s { [*] -> return; } }
  routine caller(tape t: b) { entry state s { [*] -> call foo(t = t) then s; } }
}
";
        let a = ok(src);
        let caller = a
            .resolved
            .worlds
            .iter()
            .find(|w| w.name == "lib::caller")
            .expect("lib::caller world");
        assert_eq!(caller.calls.len(), 1);
        match &caller.calls[0].target {
            ResolvedCallTarget::Routine { name, external, .. } => {
                assert_eq!(name, "lib::foo");
                assert!(!external);
            }
            other => panic!("expected a routine call, got {other:?}"),
        }
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

    // -- compile() orchestration -------------------------------------------

    const A1: &str = "\
alphabet ab { '_', 'a', 'b' }
machine {
  tape main: ab;
  entry state scan {
    ['b'] -> write ['a'] move [>] goto scan;
    ['a'] ->            move [>] goto scan;
    ['_'] -> stop;
  }
}";

    #[test]
    fn compile_object_equals_assembly_of_its_emitted_tma() {
        // The object is assembled from exactly the `.tma` text the output
        // carries, so a fresh assemble of that text is byte-identical (no
        // debug info → no line remap to diverge the side table).
        let out = compile(A1, CompileOptions::default()).unwrap();
        let direct = crate::asm::assemble(&out.tma, false).unwrap();
        assert_eq!(out.object, direct);
        assert!(
            out.report.diagnostics.is_empty(),
            "{:?}",
            out.report.diagnostics
        );
    }

    #[test]
    fn strip_debugger_reaches_the_object_bytes() {
        let src = "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state s { [*] -> debugger move [>] stop; }
}";
        let kept = compile(src, CompileOptions::default()).unwrap();
        assert!(
            kept.object.blobs[0].contains(&crate::arch::opcodes::BRK),
            "brk should be present"
        );
        let stripped = compile(
            src,
            CompileOptions {
                strip_debugger: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !stripped.object.blobs[0].contains(&crate::arch::opcodes::BRK),
            "brk should be stripped from the bytes"
        );
    }

    #[test]
    fn o1_is_byte_identical_to_o0_with_empty_pipelines() {
        // The do-no-harm floor: with no passes registered, `-O1` runs one
        // empty round and produces the same bytes as `-O0`. This lock stays
        // green as passes land — each new pass keeps identity under
        // `--fno-<that-pass>`, and the floor here pins the all-empty base.
        let o0 = compile(A1, CompileOptions::default()).unwrap();
        let o1 = compile(
            A1,
            CompileOptions {
                opt_level: OptLevel::O1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(o0.object, o1.object);
        assert_eq!(o0.tma, o1.tma);
        assert_eq!(o1.report.opt.rounds, 1);
        assert!(o1.report.opt.changes.is_empty());
    }

    #[test]
    fn foutline_threads_through_and_is_inert_on_a_program_without_sharing() {
        // `--foutline` sets `CompileOptions::outline`, which `compile()` threads
        // into `OptOptions::outline`. A1 has no repeated exit-free subgraph, so
        // the registered `outline` pass finds nothing to hoist — the object is
        // byte-identical to the same `-O1` compile without the flag. The
        // on/off-with-real-sharing check lives in the opt-equivalence matrix.
        let plain = compile(
            A1,
            CompileOptions {
                opt_level: OptLevel::O1,
                ..Default::default()
            },
        )
        .unwrap();
        let outlined = compile(
            A1,
            CompileOptions {
                opt_level: OptLevel::O1,
                outline: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(plain.object, outlined.object);
        assert_eq!(plain.tma, outlined.tma);
    }

    #[test]
    fn inline_cap_widens_what_the_program_pass_admits() {
        // `big` is a leaf routine with 8 rows across an 8-state chain
        // (> INLINE_MAX_RULES = 6). Default (`None`) keeps the shipped cap,
        // so the call survives; `Some(12)` widens the size arm and it
        // splices. End-to-end through `compile()`, proving
        // `CompileOptions.inline_cap` actually reaches
        // `OptOptions.inline_cap` — `optimizer::inline::run` is tested
        // directly in optimizer/inline.rs; this pins the compiler-level
        // plumbing, mirroring `foutline_threads_through_...` above.
        let src = "\
alphabet ab { '_', 'a' }
routine big(tape t: ab) {
  entry state s0 { [*] -> move [>] goto s1; }
  state s1 { [*] -> move [>] goto s2; }
  state s2 { [*] -> move [>] goto s3; }
  state s3 { [*] -> move [>] goto s4; }
  state s4 { [*] -> move [>] goto s5; }
  state s5 { [*] -> move [>] goto s6; }
  state s6 { [*] -> move [>] goto s7; }
  state s7 { [*] -> return; }
}
machine {
  tape t: ab;
  entry state m { [*] -> call big(t = t) then done; }
  state done     { [*] -> stop; }
}";

        let default_out = compile(
            src,
            CompileOptions {
                opt_level: OptLevel::O1,
                ..Default::default()
            },
        )
        .unwrap();
        let main = default_out
            .ir
            .worlds
            .iter()
            .find(|w| w.name == "main")
            .unwrap();
        assert!(
            main.states.iter().flat_map(|s| &s.rules).any(|r| matches!(
                &r.transition,
                crate::ir::IrTransition::CallThen { target, .. } if target == "big"
            )),
            "default cap keeps the call to the oversize leaf callee"
        );

        let widened_out = compile(
            src,
            CompileOptions {
                opt_level: OptLevel::O1,
                inline_cap: Some(12),
                ..Default::default()
            },
        )
        .unwrap();
        let main = widened_out
            .ir
            .worlds
            .iter()
            .find(|w| w.name == "main")
            .unwrap();
        assert!(
            main.states
                .iter()
                .flat_map(|s| &s.rules)
                .all(|r| !matches!(&r.transition, crate::ir::IrTransition::CallThen { .. })),
            "CompileOptions.inline_cap: Some(12) reaches the optimizer and admits the callee"
        );
    }

    #[test]
    fn capture_ir_yields_lowered_and_final_identical_stages() {
        let out = compile(
            A1,
            CompileOptions {
                capture_ir: true,
                ..Default::default()
            },
        )
        .unwrap();
        let stages: Vec<&str> = out.ir_snapshots.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(stages, vec!["lowered", "final"]);
        // -O0 skips the optimizer, so the two snapshots are identical.
        assert_eq!(out.ir_snapshots[0].1, out.ir_snapshots[1].1);
    }

    #[test]
    fn debug_info_remaps_object_lines_to_tmc_sources() {
        let out = compile(
            A1,
            CompileOptions {
                debug_info: true,
                ..Default::default()
            },
        )
        .unwrap();
        let debug = out.object.debug.as_ref().expect("debug info recorded");
        let lines = &debug[0].lines;
        // scan's rd/mtc/djmp derive from the state decl (tmc line 4); the
        // stop rule from line 7. Both must surface as `.tmc` lines, never the
        // `.tma` line numbers.
        assert!(lines.iter().any(|&(_, l)| l == 4), "{lines:?}");
        assert!(lines.iter().any(|&(_, l)| l == 7), "{lines:?}");
        // No debug line points past the `.tmc` source (9 lines).
        assert!(lines.iter().all(|&(_, l)| l <= 9), "{lines:?}");
    }

    #[test]
    fn validate_ir_rejects_a_bad_world_as_an_internal_error() {
        // compile()'s pre-codegen gate. Compiler-produced IR always passes, so
        // mutate a compiled world to be invalid (a dangling goto) and confirm
        // the gate wraps the failure as `internal-error` — never a user error.
        let out = compile(A1, CompileOptions::default()).unwrap();
        let mut bad = out.ir.clone();
        bad.worlds[0].states[0].rules[0].transition = crate::ir::IrTransition::Goto { state: 999 };
        let e = validate_ir(&bad).expect_err("a dangling goto must fail validation");
        assert_eq!(e.kind.code(), "internal-error");
        assert!(
            matches!(&e.kind, CompileErrorKind::Internal(m) if m.contains("validation")),
            "{:?}",
            e.kind
        );
    }

    #[test]
    fn internal_error_renders_in_the_house_style() {
        let e = CompileError {
            span: Span::point(0, 0),
            kind: CompileErrorKind::Internal("boom".into()),
        };
        assert!(e.to_string().ends_with("[internal-error]"), "{e}");
        assert!(
            e.to_string().contains("internal compiler error: boom"),
            "{e}"
        );
    }

    #[test]
    fn compile_handles_a_multi_world_binding_call_and_a_graft() {
        // A5: a machine + an exported routine, called across
        // alphabets — codegen emits the binding-call operand, the assembler
        // records a BoundCall (the linker resolves it once the composition
        // engine lands). compile() returns a well-formed object with both
        // worlds' code.
        let a5 = "\
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
}";
        let out = compile(a5, CompileOptions::default()).unwrap();
        // Two blobs (routine + machine); a bound-call record forces v3 shape.
        assert_eq!(out.object.blobs.len(), 2);
        assert_eq!(out.object.bound_calls.len(), 1);

        // A6: an entry graft splices the graph into the machine —
        // one emitted world, its entry the spliced instance.
        let a6 = "\
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
}";
        let out = compile(a6, CompileOptions::default()).unwrap();
        assert_eq!(out.object.blobs.len(), 1);
        assert!(out.tma.contains(".func main"), "{}", out.tma);
    }

    /// `analyze` is WIRED to the comment-bearing front end, not merely
    /// equivalent to one.
    ///
    /// The corpus/broken-source parity in `tests/tmc_green_analyze.rs` pins
    /// that the two front-end RECIPES agree; by construction it passes
    /// against the pre-migration tree too, because it computes both recipes
    /// itself and never calls `analyze`. This is the assertion that fails if
    /// `analyze`'s body is reverted to `lex` + `parse`.
    ///
    /// The lever is the lex mode: on the reverted body `Analysis.tokens` is a
    /// comment-free stream and this `assert!` fails. That is the half of the
    /// wiring carrying the hazard — a comment-bearing `tokens` is exactly why
    /// `lint()` must filter through `significant_tokens`
    /// (`tests/lint_quickfix_comments.rs`). The parse half is NOT observable
    /// from `Analysis` at all — `analyze` keeps no tree — so no assertion
    /// here could see it; what pins that half is the crate's own downstream
    /// crossfire over `extract_program` (`syntax::extract`'s module doc).
    #[test]
    fn analyze_keeps_comment_trivia_in_its_token_stream() {
        let src = format!("// leading comment\n{A1}");
        let a = analyze(&src).expect("A1 with a leading comment analyzes");
        assert!(
            a.tokens
                .iter()
                .any(|t| matches!(t.kind, crate::lexer::TokenKind::Comment(_))),
            "`analyze` must lex WithComments — the green parse reconstructs \
             trivia from the token stream, and `lint()` filters this same \
             stream back down for the adjacency-walking rules"
        );
        // …and the filtered view is still exactly the old comment-free lex,
        // so nothing downstream of that filter sees a different neighbourhood.
        assert_eq!(
            crate::parser::significant_tokens(&a.tokens),
            lex(&src).expect("lexes")
        );
    }

    // -- staged analysis (the language-service substrate) ------------------

    #[test]
    fn analyze_staged_agrees_with_analyze_on_valid_source() {
        // A leading comment proves `staged.tokens` is a genuine WithComments
        // stream (not merely coincidentally equal to the comment-free stream
        // because the fixture had nothing to filter). On a clean source the
        // staged path's success fields reproduce the batch path's output
        // exactly, and `compile()` still succeeds — the seam is purely
        // additive.
        //
        // The lex/parse halves compare against `lex`/`parse` — the PRE-green
        // front end, which neither `analyze` nor `analyze_staged` calls any
        // more; both now build `program` the same way, off the green parse
        // (`analyze_staged` additionally retains that same tree as `green`,
        // one `Rc` clone rather than a second parse — see that field's own
        // doc).
        //
        // What the `program` comparison below is, precisely: `parse` now
        // runs the same three-stage route `analyze_staged` builds `program`
        // by — `lex_with(WithComments)` → `parse_green_from_tokens` →
        // `syntax::extract_program`. So this checks that `analyze_staged`'s
        // staged construction — the tiering, the intermediate early
        // returns, the `Rc::clone` that retains `green` — reproduces the
        // same extracted `Program` a plain, unstaged `parse` of the same
        // source would. It does NOT pin which recipe `parse` runs: the two
        // sides are computed the same way on purpose, so this stays green
        // for whatever `parse` is defined as. `analyze` itself keeps
        // neither the pre-green tokens nor the green tree, so the resolved
        // module and the diagnostics are what it has left to agree on.
        let src = format!("// leading comment\n{A1}");
        let staged = analyze_staged(&src);
        assert!(staged.fatal.is_none(), "{:?}", staged.fatal);
        let tokens = staged.tokens.as_ref().expect("lexing succeeded");
        assert!(staged.green.is_some(), "parsing succeeded");
        let program = staged.program.as_ref().expect("lowering succeeded");
        let resolved = staged.resolved.as_ref().expect("resolve succeeded");

        let a = analyze(&src).unwrap();

        // WithComments genuinely in effect: the leading comment must surface
        // as a Comment token, or the filter below is a no-op.
        assert!(
            tokens
                .iter()
                .any(|t| matches!(t.kind, crate::lexer::TokenKind::Comment(_))),
            "leading comment should surface as a Comment token"
        );
        // The comment-filtered WithComments stream is byte-identical to a
        // WithoutComments lex of the same source. (`analyze` no longer keeps
        // one — it keeps the WithComments stream and lets `lint()` filter it
        // — so `lex` below is the pre-green front end, not a second view of
        // what `analyze` returned.)
        let significant: Vec<_> = tokens
            .iter()
            .filter(|t| !matches!(t.kind, crate::lexer::TokenKind::Comment(_)))
            .map(|t| t.kind.clone())
            .collect();
        let batch_tokens = lex(&src).unwrap();
        let expected: Vec<_> = batch_tokens.iter().map(|t| t.kind.clone()).collect();
        assert_eq!(significant, expected);

        assert_eq!(program, &parse(&src).unwrap());
        assert_eq!(resolved, &a.resolved);
        assert_eq!(staged.diagnostics, a.diagnostics);
        // The remaining field, compared directly rather than each side
        // against a third party: both entries now lex WithComments and keep
        // that stream, so `tokens` agrees token for token, spans included.
        // Without this line nothing in the crate compares the two token
        // streams to each other at all.
        assert_eq!(tokens, &a.tokens);

        assert!(compile(&src, CompileOptions::default()).is_ok());
    }

    /// `analyze` and `analyze_staged` build `program` by the SAME route
    /// (`lex_with(WithComments)` → `parse_green_from_tokens` →
    /// `syntax::extract_program`) rather than by two independently-proven
    /// equal ones. This source carries the three shapes the two lex modes
    /// used to differ on before `analyze` moved onto the green tree: a
    /// comment, a doc run, and a namespace. Verified clean against the real
    /// CLI (`tmt lint`) before being trusted here.
    #[test]
    fn analyze_staged_and_analyze_agree_on_program_with_comments_docs_and_a_namespace() {
        let src = "\
alphabet bits { '_', '0', '1' }

namespace mylib {
  // a helper routine
  ? adds one to the tape
  export routine plusOne(tape t: bits) { entry state g { [*] -> stop; } }
}

use mylib::plusOne;

machine {
  tape m: bits;
  entry state s { [*] -> call plusOne(t = m) then stop; }
}
";
        let staged = analyze_staged(src);
        assert!(staged.fatal.is_none(), "{:?}", staged.fatal);
        let staged_program = staged.program.as_ref().expect("program survives");

        let a = analyze(src).expect("analyzes clean");

        assert_eq!(staged_program, &a.program);
    }

    #[test]
    fn analyze_staged_agrees_with_analyze_at_every_broken_stage() {
        // Each source breaks at exactly one stage; the fatal `analyze_staged`
        // reports at its final stage agrees with what the all-or-nothing
        // `analyze` and the full `compile` report — as a WHOLE
        // `CompileError`, kind and span alike, not merely by `code()`.
        //
        // Comparing whole errors is what makes "the two entries agree" a
        // pinned claim rather than a structural expectation: on `code()`
        // alone, a one-column shift in either entry's parse fatal survives
        // the entire crate suite (measured — that is why this compares more
        // than it used to).
        let cases = [
            // Unterminated block comment — a lexical fatal.
            ("lex", "/* never closed"),
            // A bare closing brace at the top level — lexes, then fails the
            // grammar walk.
            ("parse", "}"),
            // A `goto` at an undefined state — parses, fails the world checks.
            (
                "resolve",
                "alphabet b { '_' }\nmachine { tape t: b; entry state s { [*] -> goto missing; } }",
            ),
        ];
        for (stage, src) in cases {
            let staged_fatal = analyze_staged(src).fatal;
            let analyze_fatal = analyze(src).err();
            let compile_fatal = compile(src, CompileOptions::default()).err();
            assert!(
                staged_fatal.is_some(),
                "{stage}: staged should carry a fatal"
            );
            assert_eq!(staged_fatal, analyze_fatal, "{stage}: staged vs analyze");
            assert_eq!(staged_fatal, compile_fatal, "{stage}: staged vs compile");
        }
    }

    #[test]
    fn analyze_staged_degrades_partially_at_each_break_point() {
        // lex-fail: nothing survives.
        let s = analyze_staged("/* never closed");
        assert!(s.tokens.is_none());
        assert!(s.green.is_none());
        assert!(s.program.is_none());
        assert!(s.resolved.is_none());
        assert_eq!(s.fatal.unwrap().kind.code(), "lex-error");

        // parse-fail: tokens survive, nothing past the green tree.
        let s = analyze_staged("}");
        assert!(s.tokens.is_some(), "lexing still succeeded");
        assert!(s.green.is_none());
        assert!(s.program.is_none());
        assert!(s.resolved.is_none());
        assert!(s.fatal.is_some());

        // resolve-fail: tokens + the green tree + the flat program survive;
        // the resolved module does not, and no diagnostics leak out of a
        // mid-resolve fatal.
        let s = analyze_staged(
            "alphabet b { '_' }\nmachine { tape t: b; entry state s { [*] -> goto missing; } }",
        );
        assert!(s.tokens.is_some());
        assert!(s.green.is_some());
        assert!(s.program.is_some(), "program survives a resolve fatal");
        assert!(s.resolved.is_none());
        assert!(s.diagnostics.is_empty());
        assert_eq!(s.fatal.unwrap().kind.code(), "undefined-state");

        // success: every stage's product is present.
        let s = analyze_staged(A1);
        assert!(s.tokens.is_some());
        assert!(s.green.is_some());
        assert!(s.program.is_some());
        assert!(s.resolved.is_some());
        assert!(s.fatal.is_none());
    }

    /// A modest fragment vocabulary spanning the `.tmc` grammar's keywords,
    /// punctuation, and a few literal glyphs — enough that a random join
    /// tokenizes into something the parser and resolver actually walk (arbitrary
    /// `String`s mostly die at the lexer).
    const TMC_FRAGMENTS: &[&str] = &[
        "alphabet",
        "machine",
        "routine",
        "graph",
        "namespace",
        "use",
        "export",
        "tape",
        "state",
        "entry",
        "graft",
        "bind",
        "call",
        "then",
        "goto",
        "move",
        "write",
        "return",
        "stop",
        "halt",
        "as",
        "debugger",
        "{",
        "}",
        "[",
        "]",
        "(",
        ")",
        ";",
        ",",
        ":",
        "->",
        "=>",
        "..",
        "*",
        "::",
        "'_'",
        "'a'",
        "'0'",
        "0",
        "1",
        "t",
        "s",
        "b",
        "// c\n",
        "? doc\n",
        "! attn\n",
    ];

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// `analyze_staged` returns a staged result on any input — it never
        /// panics (no unwrap, no slice, no overflow) on arbitrary text.
        #[test]
        fn analyze_staged_never_panics_on_arbitrary_input(src in any::<String>()) {
            let _ = analyze_staged(&src);
        }

        /// The same, on random joins of `.tmc` fragments — inputs that reach
        /// deeper into the parser and resolver than raw noise does.
        #[test]
        fn analyze_staged_never_panics_on_tmc_fragments(
            frags in proptest::collection::vec(prop::sample::select(TMC_FRAGMENTS), 0..48),
        ) {
            let src = frags.join(" ");
            let _ = analyze_staged(&src);
        }
    }
}
