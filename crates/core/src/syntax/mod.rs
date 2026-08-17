//! Language-agnostic lossless syntax trees: an immutable green layer,
//! offset-carrying red cursors, a builder for recursive-descent
//! parsers, and the typed-view (`AstNode`) contract. Kinds are opaque
//! per-language `u16` spaces; core interprets none of them (the same
//! contract the VM core keeps for opcodes and the assembler for
//! dialects). Handles are `Rc`-based — single-threaded by design.

mod ast;
mod builder;
mod green;
mod line_index;
mod red;

pub use ast::{AstNode, child, children, token};
pub use builder::{Checkpoint, TreeBuilder};
pub use green::{GreenElement, GreenNode, GreenToken, SyntaxKind};
pub use line_index::LineIndex;
pub use red::{SyntaxElement, SyntaxNode, SyntaxToken, TextRange};
