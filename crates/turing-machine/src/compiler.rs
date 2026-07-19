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

/// The ways a `.tmc` compile can fail fatally. Only the lexer's kind exists
/// today; parser / resolution / IR / codegen kinds join it in the later
/// phase-6a tasks, mirroring the `.pmc` compiler's kind set. Kept as its own
/// enum (not folded into `CompileError`) so the frozen `code()` discipline
/// lives in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileErrorKind {
    /// Lexical error — an unexpected character, an unterminated block
    /// comment, or a malformed glyph literal (unterminated / empty / bad
    /// escape). The message is the human-readable detail.
    Lex(String),
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
        }
    }
}
