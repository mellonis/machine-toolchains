//! Post-machine toolchain: PM-1 arch module, `.pmc` compiler, stdlib, `pmt`.

pub mod arch;
pub mod asm;
pub mod codegen;
pub mod compiler;
pub mod ir;
pub mod lexer;
pub mod optimizer;
pub mod parser;

pub use compiler::{
    CompileError, CompileErrorKind, CompileOptions, CompileOutput, CompileReport, Warning, compile,
};
pub use optimizer::{OptLevel, OptReport, PassChange};
