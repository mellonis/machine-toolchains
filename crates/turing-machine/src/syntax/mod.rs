//! The `.tmc` green-syntax layer over the core framework
//! (docs/core.md (syntax trees)): the kind space, the source-layout
//! pass, and the green-tree sink for now, with typed views following
//! in later tasks of the same migration.
//!
//! # A declaration retro-wraps its bound doc run
//!
//! Every doc-run-accepting declaration — `export`/`alphabet`/
//! `routine`/`graph`/`machine`/`namespace` at file/namespace level,
//! `entry`/`state`/`graft`/`bind` inside a world body — opens its
//! green node retroactively at a checkpoint taken BEFORE the run, so
//! the run (wrapped in its own DOC_RUN node) becomes the
//! declaration's own first child rather than an unwrapped sibling
//! before it. The checkpoint is taken right after the loop's
//! pending-comment drain and before the doc-run block — this exact
//! position, not the loop top, is what keeps leading-comment
//! attachment identical to the sibling `.pmc` parser's `fn_cp`
//! (`crates/post-machine/src/parser.rs`): a comment written before the
//! run still flushes into the PARENT, since the checkpoint carries no
//! node of its own to flush into until it is retroactively opened.
//!
//! This was a decision, not something the grammar dictated either way,
//! made for three reasons: the already-shipped `.pmc` formatter's
//! `blank_before_unit` (`crates/post-machine/src/fmt/trivia.rs`) walks
//! to a function node's own previous sibling to find the blank line
//! that precedes its documented unit, which only lands on the right
//! answer BECAUSE retro-wrapping already folded the bound doc run
//! inside the node — a later plan porting that logic to `.tmc` needs
//! the same shape under it; `.tmc` doc runs bind to a wider surface
//! than `.pmc`'s (namespaces included — a `.pmc` namespace rejects a
//! doc run outright as `DanglingDocRun`, `.tmc`'s does not), so there
//! is no exempt container that could stay unwrapped without becoming
//! an inconsistency; and the C1 CST already treats `doc_run` as a
//! FIELD of the declaration's own struct (`AlphabetCst`, `MachineCst`,
//! `NamespaceCst`, `ReuseCst`, `StateCst`, `GraftCst`, `BindCst`),
//! never a free-standing item, so the green shape now matches the
//! CST's own model of "whose run is this" instead of contradicting it.
//!
//! # The green tree splits what the CST keeps together
//!
//! A `trailing`/`close_trailing` comment — one riding the same source
//! line as a node's own last token (`;` or `}`) — is a FIELD on that
//! node's CST struct, but in the green tree it lands OUTSIDE the node:
//! `GreenSink::finish` closes a node without flushing trailing trivia,
//! so a same-line comment (trivia attached to whichever significant
//! token follows it) is only flushed once THAT token's own checkpoint
//! or flush runs — by which point the node has already closed and the
//! open node is its parent. `open_trailing` (a comment on the same
//! line as an opening `{`, before the first body item) has no such
//! gap: the node is still open when that comment's trivia flushes, so
//! it lands INSIDE, matching the CST. This is already how the PM
//! sibling's own printer reads a trailing comment — `trailing_comment`
//! in `crates/post-machine/src/fmt/trivia.rs` looks at the node's
//! `next_sibling_or_token()`, never its children. A later `.tmc` fmt
//! port reading node extents directly needs the same asymmetry: the
//! green tree's node boundary and the CST's "who owns this comment"
//! answer are not the same question.

mod emit;
mod kinds;
mod layout;

pub use emit::GreenSink;
pub use kinds::{TmcKind, kind_name};
pub use layout::{SigLayout, layout};
// `pub(crate)`, not `pub`: `token_kind` itself is `pub(crate)` (only the
// parser's `bump()` needs it), so re-exporting it any wider than
// `pub(crate)` here would be private-in-public.
pub(crate) use kinds::token_kind;
