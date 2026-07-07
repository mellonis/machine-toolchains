//! Post-machine toolchain: PM-1 arch module, `.pmc` compiler, stdlib, `pmt`.

pub mod arch;
pub mod asm;
pub mod cli;
pub mod codegen;
pub mod compiler;
pub mod completions;
pub mod cst;
pub mod ir;
pub mod lexer;
pub mod lint;
pub mod optimizer;
pub mod parser;
/// Frozen pre-C1 parser, retained as the parity oracle for the CST
/// migration (`tests/parser_parity.rs`). Not part of the stable API.
#[doc(hidden)]
pub mod parser_legacy;
pub mod stdlib;

pub use compiler::{
    CompileError, CompileErrorKind, CompileOptions, CompileOutput, CompileReport, compile,
};
pub use lint::{FixOutcome, LintError, LintOptions, LintReport, apply_fixes, lint};
pub use optimizer::{OptLevel, OptReport, PassChange};
pub use parser::PMC_LANG_VERSION;
