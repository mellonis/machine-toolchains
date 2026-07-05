//! `.pmc` compiler driver and shared diagnostics (spec §7).
//!
//! Every pipeline stage (lexer → parser → lowering → codegen) reports
//! fatals through [`CompileError`]; non-fatal findings accumulate as
//! [`Warning`]s — library code never prints (spec §10).

/// 1-based `line`; 1-based `col` counted in characters, or 0 when the
/// error is attributed to a whole line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub line: u32,
    pub col: u32,
    pub kind: CompileErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileErrorKind {
    /// Lexical error (unexpected character, unterminated comment, …).
    Lex(String),
    /// The parser needed one thing and saw another.
    Expected {
        what: &'static str,
        found: String,
    },
    /// A reserved word used as a function name.
    ReservedFunctionName(String),
    /// A bare identifier statement that is not a builtin (spec §3.3).
    UnknownCommand(String),
    /// `@` applied to a builtin name (`@left()`).
    BuiltinCalled(String),
    DuplicateFunction(String),
    DuplicateLabel(u32),
    /// `goto`/`check`/successor names a label the function never declares.
    UndefinedLabel(u32),
    /// `goto !` — spec §3.2: put `(!)` on the preceding command instead.
    GotoReturn,
    /// A comma-group position rule violated (spec §3.2, last table row).
    GroupPosition(&'static str),
    /// A label at the end of a function body binds to nothing.
    DanglingLabel(u32),
    /// The generated `.pma` failed to assemble — a compiler bug, not a
    /// user error; the message carries the assembler diagnostic.
    Internal(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.col > 0 {
            write!(f, "line {}:{}: ", self.line, self.col)?;
        } else {
            write!(f, "line {}: ", self.line)?;
        }
        match &self.kind {
            CompileErrorKind::Lex(m) => write!(f, "{m}"),
            CompileErrorKind::Expected { what, found } => {
                write!(f, "expected {what}, found {found}")
            }
            CompileErrorKind::ReservedFunctionName(n) => {
                write!(f, "`{n}` is a reserved word and cannot name a function")
            }
            CompileErrorKind::UnknownCommand(n) => {
                write!(
                    f,
                    "unknown command `{n}` (user functions are called `@{n}()`)"
                )
            }
            CompileErrorKind::BuiltinCalled(n) => {
                write!(f, "`{n}` is a builtin — write it without `@`")
            }
            CompileErrorKind::DuplicateFunction(n) => write!(f, "duplicate function `{n}`"),
            CompileErrorKind::DuplicateLabel(l) => write!(f, "duplicate label `{l}`"),
            CompileErrorKind::UndefinedLabel(l) => write!(f, "undefined label `{l}`"),
            CompileErrorKind::GotoReturn => {
                write!(
                    f,
                    "`goto !` is not allowed — put `(!)` on the preceding command"
                )
            }
            CompileErrorKind::GroupPosition(m) => write!(f, "{m}"),
            CompileErrorKind::DanglingLabel(l) => {
                write!(f, "label `{l}` at end of function binds to nothing")
            }
            CompileErrorKind::Internal(m) => write!(f, "internal compiler error: {m}"),
        }
    }
}

impl std::error::Error for CompileError {}

/// A non-fatal finding, reported (never printed) via `CompileReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub line: u32,
    pub message: String,
}
