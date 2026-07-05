//! Arch-generic assembler/disassembler frameworks (spec §6.4). All
//! instruction knowledge arrives via [`ArchSyntax`] tables.

mod assembler;
mod parser;
mod syntax;

pub use assembler::assemble;
pub use syntax::{ArchSyntax, Flow, RelaxPair, SyntaxEntry};

#[derive(Debug, PartialEq, Eq)]
pub struct AsmError {
    pub line: usize,
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
    ShortOffsetOutOfRange { target: String },
    EncodeError(&'static str),
}

impl std::fmt::Display for AsmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {:?}", self.line, self.kind)
    }
}

impl std::error::Error for AsmError {}
