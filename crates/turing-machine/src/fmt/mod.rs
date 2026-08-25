//! `.tmc` pretty-printer — the TM-1 twin of the PM-1 crate's `.pmc`
//! formatter, and a thin renderer in the same sense: [`format`] returns a
//! `Result` and never prints, never touches the filesystem; `cli/fmt.rs` is
//! the only place a diagnostic or a file write happens.
//!
//! This module is the door, not the printer. [`print`] is the printer — it
//! walks the lossless green syntax tree and states the whole layout
//! contract (canonical, idempotent, whitespace-only, trivia-preserving; the
//! state-block grid; the width threshold; blank lines and comments) in its
//! own module documentation. [`trivia`] owns the other half of that
//! printer's input: the derived facts that carry no parsed value — where a
//! comment lives, which brace it rides, where the author left a blank line.

use crate::compiler::CompileError;

mod print;
mod trivia;

#[cfg(test)]
mod tests;

/// One `writes { … }` or `preserves { … }` clause in canonical form,
/// re-exported so the LSP hover renderers (`lsp/navigate.rs`) can spell a
/// declared clause identically to this printer's output instead of keeping
/// a second copy of the same string in sync.
pub(crate) use print::contract_clause_text;

/// `.tmc` source → canonical text. Lexes with comments retained, builds the
/// green syntax tree, and prints it. A lex or parse error is returned, never
/// printed.
pub fn format(source: &str) -> Result<String, CompileError> {
    print::format(source)
}
