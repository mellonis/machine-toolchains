//! Arch-generic assembler/disassembler frameworks (docs/formats.md
//! (assembly text)). All instruction knowledge arrives via [`ArchSyntax`]
//! tables.

mod assembler;
pub(crate) mod decode;
mod disassembler;
mod parser;
pub(crate) mod syntax;

pub use assembler::assemble;
pub use disassembler::{
    disassemble_executable, disassemble_object, grid_line, listing_executable, listing_line,
};
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
