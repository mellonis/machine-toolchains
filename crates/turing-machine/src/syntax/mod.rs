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
//! an inconsistency; and the hand-written CST this tree replaced
//! already made `doc_run` a FIELD of each doc-accepting declaration's
//! own struct rather than a free-standing item, so retro-wrapping
//! keeps that answer to "whose run is this" instead of contradicting
//! it.
//!
//! WORLD sits between MACHINE/REUSE and their body items (`docs/tmt/language.md
//! (worlds)`), never the tape/state/graft/bind items directly: a later
//! formatter port walking a world-producing declaration's children for
//! its body items descends through WORLD first, one level, before
//! reaching them. The braces are WORLD's own tokens, not MACHINE's
//! (`world.text()` starts at `{` and ends at `}`).
//!
//! A doc-run-free `machine` therefore has exactly one child NODE under
//! `SyntaxNode::children()` — WORLD. `children_with_tokens()` sees
//! tokens too, and its count is not fixed: `IDENT "machine"` and WORLD
//! are the only two entries always present, and every trivia token
//! written between the keyword and the brace sits between them as a
//! further direct child. Measured: `machine{ … }` two entries,
//! `machine { … }` three, `machine /* c */ { … }` five. A `routine`/
//! `graph` has the same shape one level busier: one child NODE per
//! signature parameter plus WORLD is fixed — `routine r(tape t: ab)
//! { … }` always yields two child nodes, SIG_PARAM and WORLD — but its
//! `children_with_tokens()` count is just as unfixed as MACHINE's:
//! `IDENT "routine"`, `IDENT "r"`, `L_PAREN`, SIG_PARAM and `R_PAREN`
//! are always present, and every whitespace or comment written around
//! them (after the keyword, before the parameter list, before the
//! brace, …) is a further direct child. Measured: `routine
//! r(tape t: ab){ … }` seven entries (no gap before the brace),
//! `routine r(tape t: ab) { … }` eight, `routine r(tape t: ab)
//! /* c */ { … }` ten. An accessor that wants "the body" therefore
//! looks WORLD up by kind rather than by position or count.
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
//! `next_sibling_or_token()`, never its children. The `.tmc` printer
//! does the same, for the same reason — `fmt::trivia` reads a trailing
//! comment off the node's next sibling rather than off its extent,
//! because the green tree's node boundary and the "who owns this
//! comment" answer are not the same question.
//!
//! # How extraction itself is checked
//!
//! The lossless law above (`text() == source`) cannot catch an
//! extraction bug — a tree can round-trip byte for byte and still be
//! read wrongly. `extract_program` (`extract.rs`) is a production path,
//! not a plan artifact: `compiler::analyze` and
//! `compiler::analyze_staged`, the `.tmc` compiler front, both build
//! their `Program` through it, so every golden program, lint fixture,
//! LSP test and formatter test in the crate runs on what it produces —
//! measured, one extraction shim at a time, in `extract.rs`'s own module
//! doc, where corrupting a single shim's return value reds between 3 and
//! 170 of them.
//!
//! What that crossfire cannot see is a field nothing downstream reads,
//! and those carry hand-written value tests in `extract.rs`'s own test
//! module instead: a `Transition`'s own `span`, and
//! `DocRunItem::blank_before` — the latter cannot reach a `Program` at
//! all, since `reduce_doc_run` folds a doc run over its `kind` alone, so
//! it is asserted directly on `extract_doc_items`' output.
//! `tests/tmc_property.rs` adds a per-program construct-coverage check
//! over generated programs: the set of constructs the generator WROTE
//! must equal the set extraction reports back.
//!
//! # `syntax::views` also has consumers outside `extract_program`
//!
//! The typed-view layer `extract_program` is built on is no longer only
//! an implementation detail of extraction: the `.tmc` language service
//! casts a document's own retained green tree straight to `RootView`
//! and walks it directly, without reparsing. `lsp/mod.rs`'s
//! `document_symbols`
//! (`a_documented_declarations_symbol_starts_at_its_keyword`,
//! `a_documented_world_members_symbol_starts_at_its_keyword`) reads
//! only the views — `tree_symbols` never touches `Program`.
//! `lsp/quickfix.rs`'s `state_stub`
//! (`a_documented_machines_state_stub_lands_past_its_bound_doc_run`)
//! reads the views AND the document's already-extracted `Program`
//! together: `enclosing_body` walks `RootView` to find the enclosing
//! MACHINE/REUSE span, then reads the arity out of `Program` — a REUSE's
//! by name, from `Program.routines`/`Program.graphs`; a top-level
//! MACHINE's own, from `Program.machine`'s tape count directly —
//! extraction and the views answer two different halves of the same
//! question, not one superseding the other. Both fixtures live in
//! `crates/turing-machine/src/lsp/tests.rs`. A change to a view's
//! accessor shape is therefore live-code-breaking from two independent
//! directions, not one.

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
