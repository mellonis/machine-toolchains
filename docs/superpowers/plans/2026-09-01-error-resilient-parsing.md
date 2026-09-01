# Error-Resilient Parsing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A broken `.pmc`/`.tmc` document still yields a lossless green
tree — broken regions wrapped in `Error` nodes — so the language
services answer symbols and navigation from the CURRENT text mid-edit
instead of the last good document version (closes the C2 follow-up
tracked as "error-resilient parsing on the green tree").

**Architecture:** A NEW resilient entry (`parse_green_resilient`) lives
beside the fatal one in each front end; the batch pipeline
(`parse_green_from_tokens`, the compiler, `fmt`) keeps the fatal
contract untouched — behavior-neutral outside the editor. Recovery
happens at the loop seams both parsers already share: each top-level
item iteration takes a green checkpoint before parsing; on
`CompileError` the recovery wrapper unwinds the sink to the loop's
depth, retro-wraps everything since the checkpoint plus the tokens
skipped to a sync point into an `Error` node, records the error, and
continues. The sink gains depth tracking in core to make the unwind
possible. Language services store the resilient tree in `DocState`
whenever the fatal is at the parse stage, and the green-tier features
(symbols) skip `Error` nodes.

**Tech Stack:** existing green/red syntax framework
(`crates/core/src/syntax/`), both crates' recursive-descent parsers,
the staged-analysis seam (`compiler::analyze_staged`), both
`LanguageService`s.

**Spec:** design is this plan's §Design (no separate spec; the C2 spec
`docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md`
declared this a follow-up and deferred all decisions here).

## Global Constraints

- The fatal-error contract of `parse_green_from_tokens`, `parse`,
  `compile`, and `fmt::format` does not change: first error = `Err`,
  no tree. `-O0` bit-identity, PM-1 byte-identity, and the
  compiled-stdlib gate must stay green untouched.
- The lossless law extends to broken input: for ANY input that LEXES,
  the resilient tree's `text()` equals the source byte-for-byte.
  (Input that fails the lexer stays fatal-only: no token stream, no
  tree — `DocState.green` stays `None` there, exactly as today.)
- The resilient parse reports the SAME first error the fatal parse
  reports (same kind, same span) — pinned property, so the editor's
  diagnostic never diverges from the CLI's.
- Core stays language-agnostic: depth tracking is generic sink
  mechanics; `Error` kinds live per crate.
- Quality gates per task: full workspace tests, clippy `-D warnings`,
  `cargo fmt --check`; the no_std build after any core change.

## Design

**Error node.** One new kind per crate (`PmcKind::Error`,
`TmcKind::Error`), registered in `kind_name`. An `Error` node wraps:
whatever the failed item parse emitted before failing (already-closed
partial nodes and tokens since the iteration's checkpoint), plus every
token skipped during resynchronization. Nothing downstream looks
INSIDE an `Error` node; extraction and views skip it wholesale.

**Sink unwind.** `GreenSink` (core, generic) gains `open_depth() ->
usize` (incremented by `start`/`start_at`, decremented by `finish`)
and `finish_to(depth)` (finish until depth). A failed item parse may
leave nodes open; the recovery wrapper closes them (their partial
children remain valid closed nodes), then `start_at(checkpoint,
Error)` retro-wraps the whole region — valid because the folds all
happened at or after the checkpoint, so it still addresses a child
boundary of the loop's own frame.

**Sync points.** After wrapping, the parser skips tokens (emitting
each into the open `Error` node with its total-mapped kind) until a
token that can start the next item at the current level, the level's
terminator (`}` — NOT consumed; left for the loop), or Eof:
- PM top level: `use` / `namespace` / `export` / `volatile` /
  a doc/attention line / Eof / the terminator. And, since a top-level
  statement is valid `.pmc`, a `;` is consumed INTO the error region
  and ends it (the next token starts fresh).
- TM top level: `use` / `alphabet` / `tape` / `routine` / `graph` /
  `machine` / `namespace` / `export` / a doc/attention line / Eof /
  the terminator; a `;` likewise ends the region.
The sync sets are conservative supersets of each loop's own dispatch
tokens — a wrong guess costs one more error region, never a lost
token.

**Nested recovery (round 2 — LANDED).** v1 recovered at TOP-LEVEL
item boundaries; round 2 added the same wrapper at the inner loops —
`.pmc` function-body statements (`fn_body_item`/`skip_to_stmt_sync`)
and `.tmc` world items (`world_item`/`skip_to_world_sync`). The skips
are brace-depth-aware — an interior `;`/`}` belongs to the region —
with STRONG sync shapes (item keywords, doc lines, a name followed by
`(`) ending the region at any depth, so an unbalanced `{` in the
broken region cannot swallow the rest of the file. Rule-level recovery
INSIDE a `.tmc` state stays out of scope (finer than the issue's
statement grain; revisit only with incremental-reparse measurements).

**Entry.** Per crate:
```rust
pub struct ResilientParse {
    pub green: Rc<GreenNode>,
    /// Every recovery's error, in source order; empty = clean parse.
    pub errors: Vec<CompileError>,
}
pub fn parse_green_resilient(source: &str, tokens: &[Token]) -> ResilientParse
```
On a clean document it returns the identical tree the fatal entry
builds (pinned byte-for-byte over the corpus).

**Service integration.** `analyze_staged` (both crates) additionally
runs the resilient parse when the fatal is at the parse stage, and its
`Analysis.green` carries the resilient tree (today: `None`). The
fatal, tokens, program, and every later-stage field are UNCHANGED — a
parse-stage fatal still means `program: None`. `document_symbols`
(both services) then answers from the tree, skipping `Error` nodes;
everything else keeps its current degradation. `fmt` stays fatal on
broken input BY RULING: canonical layout over an error region is
undefined, and silently reformatting around broken text on save is
worse than refusing — recorded in docs/lsp.md.

**Out of scope:** completions from partial extraction (last-good
scopes stay), formatting broken documents, incremental reparse (its
issue stays gated), the asm CST port (its own arc).

---

### Task 1: Sink depth tracking (core)

**Files:**
- Modify: `crates/core/src/syntax/sink.rs`

**Interfaces:**
- Produces: `GreenSink::open_depth(&self) -> usize`,
  `GreenSink::finish_to(&mut self, depth: usize)`.

- [ ] Failing test in sink.rs's fake-kind module: open two nested
  nodes, `finish_to` the outer depth, assert `open_depth` returns and
  the tree closes balanced.
- [ ] Implement: `open: usize` field; `start`/`start_at` increment,
  `finish` decrements (debug_assert on underflow); `finish_to` loops
  `finish` while `open > depth`.
- [ ] Green + gates; commit `feat(core): the green sink tracks its open depth`.

### Task 2: PM resilient parse

**Files:**
- Modify: `crates/post-machine/src/syntax/kinds.rs` (Error kind +
  total `token_kind(&TokenKind) -> PmcKind` mirroring TM's),
  `crates/post-machine/src/parser.rs`
- Test: `crates/post-machine/tests/resilient_parse.rs`

**Interfaces:**
- Produces: `parser::ResilientParse`, `parser::parse_green_resilient`.

- [ ] Failing tests: (a) a file `use ;\nmain() { 1: right; }\n` yields
  a tree with `text() == source`, one error (same kind+span as the
  fatal entry's), and `main` still visible as a FUNCTION node;
  (b) total junk yields one Error region, lossless; (c) clean corpus
  → byte-identical tree to the fatal entry, empty errors;
  (d) property: random token-level mutations of corpus sources —
  every parse yields a lossless tree and first-error equality with
  the fatal entry.
- [ ] Implement: `top_items` grows a `recovering: Option<&mut
  Vec<CompileError>>` mode (a field on the Parser, set only by the
  resilient entry); the loop body moves into a closure whose `Err` —
  in resilient mode — triggers: record error (first only per region),
  `sink.finish_to(loop_depth)`, `start_at(cp, Error)`, skip+emit to a
  sync token, `finish`. Fatal mode propagates unchanged.
- [ ] Green + gates; commit
  `feat(post-machine): a broken document still yields a lossless tree`.

### Task 3: TM resilient parse

Same shape as Task 2 over `crates/turing-machine/src/parser.rs` and
`syntax/kinds.rs` (Error kind; `token_kind` already exists), tests in
`crates/turing-machine/tests/resilient_parse.rs` with `.tmc` fixtures
(`alphabet { }` broken header; junk between declarations; the graft
corpus mutated). Commit
`feat(turing-machine): a broken document still yields a lossless tree`.

### Task 4: Services answer from the resilient tree

**Files:**
- Modify: `crates/post-machine/src/compiler.rs` (`analyze_staged`),
  `crates/turing-machine/src/compiler.rs` (same),
  both `src/lsp/mod.rs` (`document_symbols` Error-skip; DocState doc),
  `docs/lsp.md` (staged analysis + the fmt ruling)
- Test: service-level pins in both `lsp` test modules: a document
  broken mid-declaration still lists the OTHER declarations' symbols
  from the CURRENT text (a renamed function shows its new name while
  a later declaration is broken).

- [ ] Failing service pins.
- [ ] `analyze_staged`: on a parse-stage fatal, run
  `parse_green_resilient` and carry its tree in `green` (fatal and
  every other field unchanged); on a lex fatal, `green` stays `None`.
- [ ] `document_symbols`/`tree_symbols`: skip `Error`-kind children.
- [ ] Green + gates; docs; commit
  `feat(lsp): symbols answer from the current text across a broken declaration`.

### Task 5: Arc close

- [ ] Full workspace gates; PM-1 byte-identity untouched (no codegen
  path change — state it in the commit); docs/core.md syntax-trees
  paragraph mentions the resilient entry; close the tracker issue
  with the v1/deferred-inner-recovery boundary stated.
