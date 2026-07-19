//! `.tmc` compiler driver and shared diagnostics — the front-end mirror of
//! the `.pmc` compiler in the sibling PM-1 crate.
//!
//! This module grows across the phase-6a tasks (parser → resolution → IR →
//! codegen → compile orchestration); for now it hosts only the shared fatal
//! type every pipeline stage reports through — and the only stage that
//! exists so far is the lexer. Library code never prints: fatals flow as
//! span-carrying, coded values and the CLI is the sole renderer.

use mtc_core::diagnostics::Span;

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
    WrongTargetKind { name: String, expected: &'static str },
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
    WrongArgKind { name: String, expected: &'static str },
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
                write!(
                    f,
                    "unknown graph `{n}` — a graft needs the graph's source"
                )
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
}
