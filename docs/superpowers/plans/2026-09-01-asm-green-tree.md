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
