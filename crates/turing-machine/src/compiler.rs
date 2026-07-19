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
    /// A reserved keyword used where a name is expected. `what` names the
    /// position ("an alphabet", "a routine", "a state", "a path segment", …).
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
                write!(f, "`{name}` is a reserved keyword and cannot name {what}")
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
                write!(
                    f,
                    "doc/attention run is not attached to a declaration"
                )
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
        ];
        // Update this count when a variant joins — the reminder to wire
        // `code()` and this list together.
        assert_eq!(all.len(), 15);
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
