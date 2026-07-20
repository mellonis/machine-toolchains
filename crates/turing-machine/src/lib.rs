//! TM-1: everything specific to the multi-tape Turing architecture, built
//! on the arch-agnostic mtc-core VM. The sibling of the PM-1 crate: where
//! PM-1 drives a single two-symbol tape, TM-1 drives up to sixteen tapes,
//! each with its own alphabet, and dispatches transitions through the
//! shared match/dispatch table engine.

pub mod arch;
pub mod asm;
pub mod cli;
pub mod codegen;
pub mod compiler;
pub mod completions;
mod config;
pub mod cst;
pub mod expand;
pub mod fmt;
pub mod ir;
pub mod lexer;
pub mod lint;
pub mod lsp;
pub mod optimizer;
pub mod parser;
pub mod stdlib;

pub use asm::{TM1_TMA_DIALECT_VERSION, tm1_syntax};
pub use compiler::{CompileError, CompileErrorKind};
pub use parser::TMC_LANG_VERSION;
