//! Arch-generic assembler/disassembler frameworks (docs/formats.md
//! (assembly text)). All instruction knowledge arrives via [`ArchSyntax`]
//! tables.

mod assembler;
pub mod cst;
pub(crate) mod decode;
mod disassembler;
pub mod fmt;
pub(crate) mod lexer;
pub mod lint;
mod lower;
mod subst;
pub(crate) mod syntax;

pub use assembler::assemble;
pub use disassembler::{
    disassemble_executable, disassemble_object, grid_line, listing_executable, listing_line,
};
pub use fmt::format_asm;
pub use syntax::{ArchSyntax, AsmCaps, Flow, RelaxPair, SyntaxEntry};

use crate::diagnostics::Span;

/// A spanned, coded assembly diagnostic. The `span` points at the exact
/// offending text (docs/formats.md (assembly text)); the CLI renders it
/// as `FILE:LINE:COL: error: MESSAGE [CODE]` (docs/cli.md (error codes)).
#[derive(Debug, PartialEq, Eq)]
pub struct AsmError {
    pub span: Span,
    pub kind: AsmErrorKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AsmErrorKind {
    Syntax(&'static str),
    UnknownMnemonic(String),
    OutsideFunction,
    DuplicateFunction(String),
    DuplicateLabel(String),
    UnknownLabel(String),
    BadOperand(&'static str),
    ShortOffsetOutOfRange {
        target: String,
    },
    EncodeError(&'static str),
    /// A line that is not assembly-shaped (a CST Raw node): a disassembly
    /// listing row, a stray `<name>`, `A: 5`, and the like.
    RawLine,
    /// A `.rept v, lo, hi` whose bounds describe an empty range (`lo > hi`).
    BadRept,
    /// A `{expr}` substitution marker in a `.rept` body that failed to
    /// evaluate (bad grammar, unknown variable, unbalanced brace, …).
    /// The carried string is the evaluator's own message.
    BadSubstitution(String),
}

impl AsmErrorKind {
    /// Stable kebab-case code identifying the kind (docs/cli.md (error
    /// codes)). Permanent user-visible identifiers: the CLI brackets them
    /// into every fatal rendering and editor integrations match on them.
    pub fn code(&self) -> &'static str {
        match self {
            AsmErrorKind::Syntax(_) => "syntax",
            AsmErrorKind::UnknownMnemonic(_) => "unknown-mnemonic",
            AsmErrorKind::OutsideFunction => "outside-function",
            AsmErrorKind::DuplicateFunction(_) => "duplicate-function",
            AsmErrorKind::DuplicateLabel(_) => "duplicate-label",
            AsmErrorKind::UnknownLabel(_) => "unknown-label",
            AsmErrorKind::BadOperand(_) => "bad-operand",
            AsmErrorKind::ShortOffsetOutOfRange { .. } => "short-offset-out-of-range",
            AsmErrorKind::EncodeError(_) => "encode-error",
            AsmErrorKind::RawLine => "raw-line",
            AsmErrorKind::BadRept => "bad-rept",
            AsmErrorKind::BadSubstitution(_) => "bad-substitution",
        }
    }
}

impl std::fmt::Display for AsmErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The `&'static str` payloads are already lowercase human
            // messages (`takes one name`, `bad function name`, …).
            AsmErrorKind::Syntax(m)
            | AsmErrorKind::BadOperand(m)
            | AsmErrorKind::EncodeError(m) => {
                write!(f, "{m}")
            }
            AsmErrorKind::UnknownMnemonic(m) => write!(f, "unknown mnemonic `{m}`"),
            AsmErrorKind::OutsideFunction => write!(f, "code outside a function"),
            AsmErrorKind::DuplicateFunction(n) => write!(f, "duplicate function `{n}`"),
            AsmErrorKind::DuplicateLabel(l) => write!(f, "duplicate label `{l}`"),
            AsmErrorKind::UnknownLabel(l) => write!(f, "unknown label `{l}`"),
            AsmErrorKind::ShortOffsetOutOfRange { target } => {
                write!(f, "short jump to `{target}` is out of range")
            }
            AsmErrorKind::RawLine => write!(f, "not assembly text"),
            AsmErrorKind::BadRept => write!(f, "empty `.rept` range (lo > hi)"),
            AsmErrorKind::BadSubstitution(m) => write!(f, "invalid substitution: {m}"),
        }
    }
}

impl std::fmt::Display for AsmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}: {} [{}]",
            self.span.start.line,
            self.span.start.col,
            self.kind,
            self.kind.code()
        )
    }
}

impl std::error::Error for AsmError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_carries_span_message_and_bracketed_code() {
        let e = AsmError {
            span: Span::new(2, 9, 2, 14),
            kind: AsmErrorKind::UnknownMnemonic("bogus".to_string()),
        };
        assert_eq!(
            e.to_string(),
            "2:9: unknown mnemonic `bogus` [unknown-mnemonic]"
        );
    }

    #[test]
    fn raw_line_display_and_code() {
        let e = AsmError {
            span: Span::new(1, 1, 1, 10),
            kind: AsmErrorKind::RawLine,
        };
        assert_eq!(e.to_string(), "1:1: not assembly text [raw-line]");
    }

    #[test]
    fn short_offset_display_names_the_target() {
        let kind = AsmErrorKind::ShortOffsetOutOfRange {
            target: "END".to_string(),
        };
        assert_eq!(kind.to_string(), "short jump to `END` is out of range");
        assert_eq!(kind.code(), "short-offset-out-of-range");
    }

    #[test]
    fn every_kind_has_a_distinct_code() {
        // One representative per variant; `code()`'s match is exhaustive,
        // so this also pins that every variant is accounted for.
        let kinds = [
            AsmErrorKind::Syntax("x"),
            AsmErrorKind::UnknownMnemonic("x".into()),
            AsmErrorKind::OutsideFunction,
            AsmErrorKind::DuplicateFunction("x".into()),
            AsmErrorKind::DuplicateLabel("x".into()),
            AsmErrorKind::UnknownLabel("x".into()),
            AsmErrorKind::BadOperand("x"),
            AsmErrorKind::ShortOffsetOutOfRange { target: "x".into() },
            AsmErrorKind::EncodeError("x"),
            AsmErrorKind::RawLine,
            AsmErrorKind::BadRept,
            AsmErrorKind::BadSubstitution("x".into()),
        ];
        assert_eq!(kinds.len(), 12);
        let codes: std::collections::HashSet<&str> = kinds.iter().map(|k| k.code()).collect();
        assert_eq!(codes.len(), kinds.len(), "codes: {codes:?}");
    }
}
