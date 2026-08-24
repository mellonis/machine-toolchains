//! The `.tmc` green-syntax layer over the core framework
//! (docs/core.md (syntax trees)): the kind space, the source-layout
//! pass, the green-tree sink, and typed views over the resulting tree.
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
//! pending-comment drain and before the doc-run block, matching where
//! PM's own `fn_cp` sits (`crates/post-machine/src/parser.rs`). That
//! exact position is NOT behaviourally forced: `drain_pending` only
//! advances `self.cpos` (the CST's own comment-attachment cursor),
//! never `self.pos` — the position `g_checkpoint` flushes on — so a
//! checkpoint taken above the drain instead is an equivalent mutant,
//! not a bug (confirmed: every test in this crate still passes with
//! it moved there). The reason to keep it exactly where PM's `fn_cp`
//! sits is structural parity between the two languages' parsers, and
//! that reason is sufficient on its own — no mechanical one is needed
//! or claimed.
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
//! WORLD sits between MACHINE/REUSE and their body items (`docs/tmt/language.md
//! (worlds)`), never the tape/state/graft/bind items directly: a later
//! formatter port walking a world-producing declaration's children for
//! its body items descends through WORLD first, one level, before
//! reaching them. The braces are WORLD's own tokens, not MACHINE's
//! (`world.text()` starts at `{` and ends at `}`).
//!
//! A doc-run-free `machine` therefore has exactly one child NODE under
//! `SyntaxNode::children()` — WORLD — and three entries under
//! `children_with_tokens()`, which sees tokens too: `IDENT "machine"`,
//! the whitespace before the brace, and WORLD. A `routine`/`graph` has
//! one child node per signature parameter as well, so
//! `routine r(tape t: ab) { … }` yields two child nodes (SIG_PARAM,
//! WORLD) and eight `children_with_tokens()` entries: `IDENT "routine"`,
//! whitespace, `IDENT "r"`, `L_PAREN`, SIG_PARAM, `R_PAREN`, whitespace,
//! WORLD. An accessor that wants "the body" therefore looks WORLD up by
//! kind rather than taking the first child node.
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
//!
//! # The parity oracle a later plan must keep green
//!
//! The lossless law above (`text() == source`) cannot catch an
//! extraction bug — a tree can round-trip byte for byte and still be
//! read wrongly. `extract_program` (`extract.rs`) is checked against
//! that separately: `Program` is held struct-equal to
//! `lower_cst(parse_cst(...))` over every `.tmc` file the repo ships
//! (`tests/syntax_parity.rs::the_shipped_corpus_extracts_identically_on_both_paths`)
//! and over 2000 generated programs per run
//! (`tests/tmc_property.rs::generated_programs_extract_identically_on_both_paths`).
//! `DocRunItem::blank_before` sits outside this equality by
//! construction — `reduce_doc_run` folds a doc run over its `kind`
//! alone, so no value of that field changes a `Program` — and it is
//! pinned one level down in `extract.rs`'s own tests instead;
//! `tests/syntax_parity.rs`'s module doc names the divergences.
//! Nothing production-side calls `extract_program` yet; whichever plan
//! routes a real consumer onto it is the plan that has to keep both
//! halves of this oracle green, not just the corpus half.

mod emit;
pub(crate) mod extract;
mod kinds;
mod layout;
mod views;

pub use emit::GreenSink;
pub use extract::extract_program;
pub use kinds::{TmcKind, kind_name};
pub use layout::{SigLayout, layout};
pub use views::{
    AlphabetView, AttrView, BindView, BindingArgView, ContractClauseView, DocRunView, GraftView,
    MachineView, MoveVecView, NamespaceView, ReuseKind, ReuseView, RootView, RuleView,
    SigParamKind, SigParamView, StateView, SymMapView, TapeView, TopView, TransitionView,
    UsePathView, UseView, WorldView, WriteVecView,
};
// `pub(crate)`, not `pub`: `token_kind` itself is `pub(crate)` (only the
// parser's `bump()` needs it), so re-exporting it any wider than
// `pub(crate)` here would be private-in-public.
pub(crate) use kinds::token_kind;
