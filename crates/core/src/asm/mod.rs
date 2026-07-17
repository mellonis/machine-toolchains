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
pub use fmt::{format_asm, format_asm_with};
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
    /// A `[..]` vector operand that does not parse (bad element, empty
    /// vector) or carries an element illegal in its context (a move
    /// marker in a match row, …).
    BadVector(&'static str),
    /// A section/table structural violation: a table directive outside
    /// `.section tables`, a function inside it, an unreferenced or
    /// multiply-referenced table, a run without a label, ….
    BadTable(&'static str),
    /// A match-table discipline violation (docs/formats.md (assembly
    /// text)): exact rows first — sorted, pairwise disjoint; wildcard
    /// rows after in source order; an all-wildcard catch-all only last;
    /// all rows one width. The span is the offending row's.
    TableDiscipline(&'static str),
    /// A table-space label that does not resolve: an operand naming no
    /// table, a dispatch target defined in no function, or dispatch
    /// targets that do not all live in the one owning function.
    UnknownTableLabel(String),
    /// A `.routine` signature problem: a duplicate directive for one
    /// function, a directive preceding no `.func` of its name, tapes
    /// outside 1..=16, a zero alphabet cardinality, an alpha list whose
    /// length differs from tapes, or a function left unsigned in a file
    /// that signs any (the MO signature section is parallel to the
    /// blobs — all or none). The carried string is the full message.
    BadSignature(String),
    /// A `.frame`/`.map`/`.exits` directive-family violation
    /// (docs/formats.md (frame descriptors)): a duplicate `.map k`, a
    /// tape index `k` at or past the frame arity, a tapes list empty or
    /// over 16, a map index/value past `0xFFFE`, a second `.exits`, an
    /// orphan `.map`/`.exits` with no open `.frame`, a map pair that
    /// breaks blank↔blank, or an exit label absent from the owning
    /// function. The carried string is the full message.
    BadFrame(String),
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
            AsmErrorKind::BadVector(_) => "bad-vector",
            AsmErrorKind::BadTable(_) => "bad-table",
            AsmErrorKind::TableDiscipline(_) => "table-discipline",
            AsmErrorKind::UnknownTableLabel(_) => "unknown-table-label",
            AsmErrorKind::BadSignature(_) => "bad-signature",
            AsmErrorKind::BadFrame(_) => "bad-frame",
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
            | AsmErrorKind::EncodeError(m)
            | AsmErrorKind::BadVector(m)
            | AsmErrorKind::BadTable(m)
            | AsmErrorKind::TableDiscipline(m) => {
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
            AsmErrorKind::UnknownTableLabel(l) => write!(f, "unknown table label `{l}`"),
            // Signature messages are composed in full at the raise site
            // (they usually name the function).
            AsmErrorKind::BadSignature(m) => write!(f, "{m}"),
            // Frame-directive messages are composed in full at the raise
            // site (they often name the offending `k`, label, or bound).
            AsmErrorKind::BadFrame(m) => write!(f, "{m}"),
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
            AsmErrorKind::BadVector("x"),
            AsmErrorKind::BadTable("x"),
            AsmErrorKind::TableDiscipline("x"),
            AsmErrorKind::UnknownTableLabel("x".into()),
            AsmErrorKind::BadSignature("x".into()),
            AsmErrorKind::BadFrame("x".into()),
        ];
        assert_eq!(kinds.len(), 18);
        let codes: std::collections::HashSet<&str> = kinds.iter().map(|k| k.code()).collect();
        assert_eq!(codes.len(), kinds.len(), "codes: {codes:?}");
    }
}
