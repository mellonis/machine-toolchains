# Assembly Green Tree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `.pma`/`.tma` assembly languages join the syntax
framework: one green-tree model across all four languages, the
framework's lossless law replacing the hand-written CST's own — the
C2 follow-up tracked as "assembly CST onto the syntax framework".

**Architecture:** The hand-written `AsmCst` stays the semantic view
every consumer eats (the exact position `Program` holds for the source
languages) — this arc adds the green tree as the carrier UNDER it, not
a second parse: a `GreenSink<AsmKind>` is threaded through
`parse_asm_cst_with`'s own item loop, so the ONE shaping walk emits
both artifacts and drift between them is impossible by construction.
`parse_asm_green(source, caps) -> (AsmCst, Rc<GreenNode>)` is the new
pair entry; `parse_asm_cst_with` delegates and drops the tree, so no
consumer changes. Phase 2+ (later rounds): typed views for the asm
services, fmt reading green trivia, extraction-style field derivation.

**Tech Stack:** core syntax framework (`SigLayout`/`GreenSink` — the
generic pair hoisted in wave 4; the asm lexer's `AsmToken` already
carries the line/col/len facts the layout adapter needs).

**Spec:** design is this plan's §Design; the C2 spec declared the port
a follow-up and deferred all decisions here.

## Global Constraints

- Behavior-neutral: `parse_asm_cst`/`parse_asm_cst_with` outputs are
  UNCHANGED (the item sequence is the same object built by the same
  loop). PM-1 byte-identity, the everything-matrix, and the asm fmt
  sidecars must stay green untouched.
- The lossless law: for ANY input (the asm parse is total), the green
  tree's `text()` equals the source byte-for-byte — pinned over the
  shipped `.pma`/`.tma` corpus AND by a proptest noise sweep.
- Core neutrality: `AsmKind` is core's own (the asm framework is
  already core-owned); no arch knowledge enters.
- Gates per task: workspace tests, clippy `-D warnings`, fmt, no_std
  (the syntax framework is std-gated; asm's green emission must be
  `feature = "std"`-gated the same way or compiled around).

## Design

**Kind space.** `AsmKind` (u16, `crates/core/src/asm/kinds.rs`):
token kinds mirroring `AsmTokenKind` (Word, Number, Colon, Comma, At,
LBracket … Junk), trivia kinds (Whitespace, Comment), and node kinds
mirroring `AsmItemKind` (Line, Func, Raw, Section, TableDirective,
Rept, RoutineDirective, Volatile, FrameDirective) plus Root. One node
per item, tokens flat inside it (label/instr sub-nodes are phase-2
views, not phase-1 structure).

**Emission.** `parse_asm_cst_with` gains a sibling entry building the
sink schedule first: lex every line (`lex_line`, the same caps),
synthesize the end-of-text Eof fact, run core `layout` with a
comment→`Trivia(AsmKind::Comment)` classification, then drive the
existing item loop with an optional sink: each `items.push(...)` site
opens the item's node, emits the significant tokens of the records
that item consumed (the loop already knows its record range), and
closes it. Trailing/own-line comments are trivia and land BETWEEN
item nodes at root level — the source-language convention; per-item
association stays the consumers' own line-keyed logic, unchanged.
Blank lines are whitespace trivia (the `blank_before` field keeps its
role for consumers; the tree carries the actual bytes).

**Laws.** (a) text() == source: corpus + proptest arbitrary-string
noise (parse is total — any input must round-trip); (b) the pair
entry's `AsmCst` is IDENTICAL (PartialEq) to `parse_asm_cst_with`'s
on the same input — pinned, though it is the same code path by
construction.

---

### Task 1: AsmKind space
`crates/core/src/asm/kinds.rs`: the enum, `From<AsmKind> for
SyntaxKind`, `kind_name`, a distinctness test. Token-kind mapping
`token_green_kind(&AsmTokenKind) -> AsmKind` (total; Comment handled
as trivia by the adapter, never mapped here).

### Task 2: layout adapter + pair entry with flat emission
`asm/cst.rs`: `parse_asm_green(source, caps) -> (AsmCst,
Rc<GreenNode>)`. Build the full-file token facts (per-line lex +
Eof), run core `layout`, thread `Option<GreenSink<AsmKind>>` through
the item loop (each push site brackets its node; a None sink is the
existing entry's no-op). Laws (a)+(b) as `tests/asm_green.rs`:
corpus walk over every shipped `.pma`/`.tma` + proptest noise.

### Task 3: arc bookkeeping
docs/core.md (assembly framework + syntax trees) sentences; the
tracker issue comment stating phase 1 and the phase-2+ boundary
(views, fmt trivia, services) — the issue stays open.

---

## Phase 2: the tree locates items; the services stop reconstructing lines

**Goal:** every CST item — comments included — gets its byte range
FROM the green tree, and both asm language services take item lines
from that pairing instead of rebuilding them (the `.pma` service's
non-blank-line zip, the `.tma` service's cursor walk over `.rept`
bodies). The tree's lossless law replaces the CST's own position
reconstruction — the substance of the tracker issue's "framework's
lossless law replacing the CST's own".

**Architecture:** three additions in core, then two consumer swaps.
(1) `.rept` blocks nest: the REPT node holds its header tokens, then
one child node per body item (comment-only bodies stay trivia), then
the `.endr` token — emitted by the same listener, recursing over the
body items zipped with their records (`shape_body` is 1:1). (2) Typed
views (`asm/views.rs`): `RootView`/`ReptView` with `items()`/`body()`
yielding an `ItemView` enum over the nine item node kinds — the tree's
typed entry, written with core's `ast_node!`. (3) `locate_items(cst,
root) -> Vec<LocatedItem>` pairs the flattened item walk (a `.rept`
header followed by its spliced body — the `.tma` service's own order)
with tree elements: a node per non-comment item, an OWN-LINE comment
token per comment item (own-line = the previous sibling is nothing or
a whitespace run containing a newline; a trailing comment's previous
sibling is a significant token, a node, or newline-free whitespace).
Mismatch is a parser bug and panics, like the framework's balance
errors — the laws below pin it away over the corpus and a noise sweep.

**Laws (added to `crates/core/tests/asm_green.rs`):**
- (c) **position parity** — for every located item with a CST span,
  `TextLineIndex::line_col(range.start) == span.start` (line AND
  column: the tree's offsets agree with the lexer-built spans); for a
  comment, the token's text equals the comment's and its column
  matches; a `.rept`'s range ends where its `endr_span` ends; a
  continued list's range ends on its last continued line.
- (d) **completeness** — located items are exactly the flattened CST
  items, in order (count + kind-discriminant sequence).
- both over the flagship + per-shape fixtures + proptest noise under
  both cap profiles; the structure test gains the nested `.rept` shape.

**Consumers:** `.tma`'s `flatten` becomes locate + `TextLineIndex`
(delete `walk`/`advance_past`); `.pma`'s `item_lines` is deleted and
`PmaDocState` carries `lines` computed once per update. Every existing
service test stays byte-identical — they are the behavioral pin.

### Task 4: `.rept` bodies nest in the tree

**Files:** modify `crates/core/src/asm/cst.rs` (`impl GreenEmit for
Emit`); modify `crates/core/tests/asm_green.rs`.

- [x] Test: `a_rept_block_nests_its_body_items` — over
  `".rept v, 0, 1\n        wr [{v}] ; t\n; own\n        mov [>]\n.endr ; done\n        stp\n"`
  under `all_caps()`, root children kinds are `["REPT", "LINE"]`, the
  REPT node's child NODES are `["LINE", "LINE"]`, its text starts with
  `.rept` and ends with `.endr` (the `; done` comment is root trivia),
  and the two law checks still hold.
- [x] Run: `cargo test -p mtc-core --test asm_green` — the new test
  FAILS (REPT has no child nodes).
- [x] Implement: in `Emit::item`, on `AsmItemKind::Rept(r)`: flush,
  start REPT, emit `records[0]`'s significant tokens, then
  `for (body_item, rec) in r.body.iter().zip(&records[1..records.len()-1])
  { self.item(&body_item.kind, core::slice::from_ref(rec)) }`, then
  the last record's tokens, finish. The default arm stays as is.
- [x] Run the asm_green test file + `cargo test -p mtc-core` — green.

### Task 5: typed views and `locate_items`

**Files:** create `crates/core/src/asm/views.rs` (std-gated in
`asm/mod.rs` next to `kinds`); modify `crates/core/tests/asm_green.rs`.

**Produces:**
```rust
pub enum ItemView { Line(LineView), Func(FuncView), Raw(RawView), Section(SectionView),
    TableDirective(TableDirectiveView), Rept(ReptView), RoutineDirective(RoutineDirectiveView),
    Volatile(VolatileView), FrameDirective(FrameDirectiveView) }
impl ItemView { pub fn syntax(&self) -> &SyntaxNode; pub fn cast(node: SyntaxNode) -> Option<ItemView> }
impl RootView { pub fn items(&self) -> impl Iterator<Item = ItemView> + '_ }
impl ReptView { pub fn body(&self) -> impl Iterator<Item = ItemView> + '_ }
pub struct LocatedItem<'a> { pub item: &'a AsmItem, pub range: TextRange }
pub fn locate_items<'a>(cst: &'a AsmCst, root: &SyntaxNode) -> Vec<LocatedItem<'a>>
```

- [x] Tests (asm_green.rs): `located_items_agree_with_the_cst_spans`
  (law c over a fixture with a leading comment, a func, a labeled line
  with trailing comment, an own-line comment, a `.rept` with a comment
  body line, a continued `.targets`, a `.volatile`), `every_flattened_item_is_located`
  (law d), fold both into `holds_both_laws` so the corpus + proptest
  sweep pins them, plus `views_walk_the_item_nodes` (RootView items
  kinds and ReptView body kinds over the Task 4 fixture).
- [x] Run — FAILS (module missing).
- [x] Implement `views.rs`: `ast_node!` per node kind; `cast_item`;
  `RootView::items`/`ReptView::body` = `children().filter_map(ItemView::cast)`;
  `locate_items` = `pair(root, &cst.items, &mut out)` where `pair`
  walks `level.children_with_tokens()` with a cursor over `items`:
  Comment item → advance to the next own-line COMMENT token; otherwise
  → next node (assert kind matches), and for `Rept` recurse into the
  node with `r.body`. `is_own_line(tok)`: `prev_sibling_or_token()` is
  `None`, or a `Whitespace` token whose text contains `'\n'`.
- [x] Run asm_green + `cargo test -p mtc-core` — green; `cargo build -p
  mtc-core --no-default-features` — green.

### Task 6: the `.tma` service takes lines from the tree

**Files:** modify `crates/turing-machine/src/lsp/tma/mod.rs`.

- [x] `did_update`: `let (cst, green) = parse_asm_green(text, caps)`;
  `flatten(text, &cst, green)` builds `FlatItem`s from
  `locate_items(&cst, &SyntaxNode::new_root(green))` with
  `TextLineIndex::new(text).line_col(range.start).0`. Delete `walk`,
  `advance_past`; keep `item_span` (document_symbols reads it).
  Rewrite the `FlatItem`/`flatten` doc comments: lines now come from
  the tree, comments included.
- [x] Run `cargo test -p mtc-turing-machine` — green, unchanged tests
  (`rept_bodies_are_flattened_onto_their_own_source_lines`,
  `a_comment_after_a_rept_block_does_not_displace_later_items` are the
  pins).

### Task 7: the `.pma` service takes lines from the tree

**Files:** modify `crates/post-machine/src/lsp/pma/{mod,complete,navigate}.rs`.

- [x] `PmaDocState` gains `lines: Vec<u32>` (parallel to `cst.items`;
  PM-1 enables no `rept`, so the flattened walk IS the item list —
  `debug_assert_eq!` the lengths). `did_update` parses with
  `parse_asm_green`. Delete `item_lines`; `document_symbols(state)`
  and the two call sites in complete.rs/navigate.rs read `&state.lines`.
- [x] Run `cargo test -p mtc-post-machine` — green, unchanged tests.

### Task 8: docs + gates

- [x] docs/core.md (syntax trees): the assembly paragraph gains the
  nested `.rept` shape, the views, and `locate_items` as the pairing
  the services read lines from. docs/lsp.md: if it describes the
  `.tma`/`.pma` line recovery, restate it as tree-derived.
- [x] Full gates: `cargo test --workspace`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo fmt --check`,
  `cargo build -p mtc-core --no-default-features`.
