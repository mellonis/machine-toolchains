//! Post-machine toolchain: PM-1 arch module, `.pmc` compiler, stdlib, `pmt`.

pub mod arch;
pub mod asm;
pub mod cli;
pub mod codegen;
pub mod compiler;
pub mod ir;
pub mod lexer;
pub mod lint;
pub mod optimizer;
pub mod parser;
pub mod stdlib;

pub use compiler::{
    CompileError, CompileErrorKind, CompileOptions, CompileOutput, CompileReport, compile,
};
pub use lint::{FixOutcome, LintError, LintOptions, LintReport, apply_fixes, lint};
pub use optimizer::{OptLevel, OptReport, PassChange};
pub use parser::PMC_LANG_VERSION;
