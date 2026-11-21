# Named vector cells Implementation Plan (2026-11-22)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the named, sparse rule-vector spelling (`[king: '♔', file: 0..7 as f]`)
in `.tmc` — pure spelling over the positional form, with the three refusals
and the printing policy the spec fixes.

**Architecture:** the parser stores each cell as written (`CellName` before
the cell content; a named vector is sparse); resolution
(`resolve_world`/`resolve_machine_world`) folds names to tape indices and
fills unnamed positions with their defaults, so every downstream consumer
sees the ordinary full-width `Rule`; `tmt fmt` prints from the source form,
generated headers print the canonical positional form from both arms.

**Tech Stack:** Rust (workspace, `mtc-turing-machine`), cargo test.

**Spec:** `docs/internal/specs/2026-11-21-named-vector-cells-design.md`
(the binding authority; this plan argues from it).

## Global Constraints

- **No language version movement.** `.tmc` stays at the standing 0.3 on
  `next` — the once-per-cycle minimal bump already covers this acceptance
  addition (the spec's "The version rule"). The `CLAUDE.md` refinement of
  that rule is Task 1.
- Diff stays `crates/turing-machine/` + `docs/` + one `CLAUDE.md`
  sentence: PM byte-identity and core-neutrality hold by construction.
- `tmt fmt` stays whitespace-only, token-preserving and unconditionally
  idempotent; the never-move audit grows the new positions and holds.
- Source-arm and object-arm header output stay byte-identical
  (`std.tmh` untouched — the stdlib writes positional).
- Test comments and code comments carry no `Task N` / `spec §N` notation.
- House pins style: every invariant's named test records the mutation that
  reddens it (mutation checklist at the end); temp paths PID + atomic
  counter.

## Review Focus

- A graph body written named-sparse that arrives through `--extern` /
  declarations / a library must splice byte-identically to its positional
  twin — the declarations path must reach the same resolution
  (`Task 4`'s extern case).
- The `unused-*` / `binding-product-threshold` lints must see an untouched
  tape exactly as the positional twin does — filling defaults must neither
  fabricate activity nor hide it (`Task 3`/`Task 4` pins).
- fmt's clean-cell reprint path silently DROPS a `name:` prefix it cannot
  render — the token-preservation gate and a comment-free named fixture
  are the guard (`Task 5`).
- `graph_digest` / `GraftDrift`: a named source and its positional twin
  must print identical headers and graft without drift (`Task 6`).
- `row-width` semantics for positional vectors stay untouched; a named
  vector can never trigger it, whatever its written length (`Task 3`).

---

### Task 1: the standing version rule, stated

**Files:**
- Modify: `CLAUDE.md` — the "Version spaces and release notes" section

**Interfaces:** none.

- [ ] **Step 1:** In the `PMC_LANG_VERSION` paragraph ("…N bumps on ANY
  grammar change…"), add one sentence: the bump is per-release, and on
  `next` it happens ONCE relative to `master` — the minimal bump; further
  acceptance changes in the same cycle fold into it and move no version.
- [ ] **Step 2:** Commit `docs: a language version bumps once per cycle, the minimal bump`.

### Task 2: the parser spells names

**Files:**
- Modify: `crates/turing-machine/src/parser.rs` — new `CellName`
  (`pub struct CellName { pub name: String, pub span: Span }`), a
  `pub name: Option<CellName>` field on `PatternCell` (:435), `WriteCell`
  (:522) and `MoveCell` (:582), and the `IDENT Colon` lookahead in
  `pattern_cell` (:2345, decided BEFORE the `TokenKind::Ident` SetRef arm
  at :2365) / `write_cell` (:2475) / `move_cell` (:2622)
- Test: parser unit tests beside the existing `#[cfg(test)]` cases

**Interfaces:**
- Consumes: `TokenKind::{Ident, Colon}`, `Span::join`.
- Produces: `CellName`, the three `name: Option<CellName>` fields — the
  reshape (Task 3) and fmt (Task 5) read them.

- [ ] **Step 1: failing tests** — a named pattern/write/move cell parses
  to `name == Some(CellName { .. })` with the cell's `span` covering
  `name` THROUGH content; a positional cell keeps `name == None`; the
  SetRef lookahead holds (`[digits]`, `[digits as d]` stay set cells,
  `[king: roster]` is a named cell whose CONTENT is a SetRef);
  `[king: * as v]` stays `wildcard-binding`; `{v + 1}` content under a
  name keeps `char_arithmetic`'s span covering the name (the whole cell).
- [ ] **Step 2: watch them fail** (named parse is today an
  `unexpected-token` / set-name misparse).
- [ ] **Step 3: implement** — the one grammar change; both parse walks
  (the `reparse_*` shims `extract.rs` drives) inherit it because they
  call the same functions.
- [ ] **Step 4:** green; `cargo test -p mtc-turing-machine --lib` stays
  green (`text() == source` pins included).
- [ ] **Step 5:** Commit `feat(turing-machine): a rule vector cell may name its tape in the spelling`.

### Task 3: resolution folds names; three refusals

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs` — a helper
  `position_named_cells(states: &mut [State], tapes: &[ResolvedTape|…],
  state_params: &[String]) -> Result<(), CompileError>` (name the builder's
  choice; signature shape is free), called from `resolve_world` (:3804,
  beside `resolve_pattern_sets` :3900) and `resolve_machine_world`
  (:4035); new `CompileErrorKind` variants `DuplicateNamedCell { .. }`
  and `MixedCellForms { .. }`; `UnresolvedTapeTarget` reused for bad
  names; registry rows + witness entry at the same declaration positions,
  `Display` arms, `docs/tmt/cli.md` rows + the widened
  `unresolved-tape-target` row ("a binding argument — or a named cell")
- Test: `crates/turing-machine/tests/named_cells.rs` (new file, shared
  with Task 4) + compiler unit tests

**Interfaces:**
- Consumes: Task 2's `CellName` fields; `ResolvedWorld::{tapes,
  state_params}` locals of the two resolve fns.
- Produces: `position_named_cells` (Task 6 reuses it for headers);
  `ResolvedWorld.states` guaranteed full-width positional.

- [ ] **Step 1: failing tests** — each refusal with its code AND span:
  unknown name → `unresolved-tape-target` at the name; a name that is a
  `state` parameter → the same code; a repeated name → `duplicate-named-cell`
  at the SECOND name; mixing named with positional → `mixed-cell-forms` at
  the first offending cell. Plus one positive: `foo(tape a: X, tape b: Y)`
  with rule `[b: '1']` resolves (`Resolved`/disassembly) to positional
  `['*', '1']` for pattern and `-`/`.` defaults elsewhere.
- [ ] **Step 2: watch them fail.**
- [ ] **Step 3: implement** — the fold (fill defaults `*`/`-`/`.`, drop
  names, full width), the two new variants in their topical enum section
  with registry rows and witness entries at matching positions, Display
  arms, cli.md rows.
- [ ] **Step 4:** green, incl. the error-code witness and
  `tests/error_code_docs.rs` (both go red before the docs rows land —
  that is the drift guard doing its job).
- [ ] **Step 5:** Commit `feat(turing-machine): named vector cells resolve to the positional form; misuse is refused`.

### Task 4: named ≡ positional, byte for byte

**Files:**
- Test: `crates/turing-machine/tests/named_cells.rs` (grows)

**Interfaces:**
- Consumes: the whole chain; the pin idiom is
  `mono_row_order.rs::rows(dis(src))` (every `.row` operand in stored
  order) beside a `tmc_fold.rs`-style run-outcome comparison.

- [ ] **Step 1: the centered pin** — for each of a machine rule, a
  routine body and a grafted graph body, one program written named-sparse
  and its positional twin: `rows(dis(named)) == rows(dis(positional))`
  AND identical run outcomes on seeded tapes. The mutation check: swap
  two names in the named source (legal, named-to-different-tapes) —
  equality must break (it pins the mapping, not the row count).
- [ ] **Step 2: the extern case** (Review Focus 1) — a second unit whose
  graph is written named-sparse, read through the declarations path and
  grafted: its spliced rows equal the positional twin's.
- [ ] **Step 3: the lints-fidelity case** (Review Focus 2) — over a
  named 4-tape world touching 2 tapes: `tmt lint` findings equal the
  positional twin's exactly (same codes, same named tapes).
- [ ] **Step 4:** commit `test(turing-machine): the named spelling compiles to the positional rows, byte for byte`.

### Task 5: fmt keeps the spelling; the audit grows

**Files:**
- Modify: `crates/turing-machine/src/fmt/print.rs` — name arms in
  `pattern_cell_text` (:1049), `write_cell_text` (:1066),
  `move_cell_text` (:1120) and the vector widths (`vec_full_text` :1467,
  `pattern_full_text` :1589); the comment-bearing path already prints
  the source slice verbatim
- Modify: `crates/turing-machine/tests/comment_positions.rs` — new
  templates for name/colon/cell slots (both comment flavours), count
  bump in the module doc
- Test: `crates/turing-machine/tests/fmt_adversarial/` — one named-cell
  `.tmc`/`.fmt` pair + the `ADVERSARIAL` entry (`tests/fmt_tmc.rs` :748)

**Interfaces:**
- Consumes: Task 2's `name` fields; `insertions` none — the clean path
  reprints from extracted VALUES.

- [ ] **Step 1: failing tests** — a comment-free named cell round-trips
  through fmt WITH its name (the recon hazard: the clean reprint drops it
  today); named and positional twins both format idempotently; the
  token-preservation gate holds over the new fixtures.
- [ ] **Step 2: watch them fail** (name dropped on the clean path).
- [ ] **Step 3: implement** — the name arms + width; nothing else moves.
- [ ] **Step 4: comment-audit additions** — new positions style (name
  then `:`, `:` then cell, both comment flavours) join `never_moves`;
  each is green before the task ends, or its failure mode is recorded as
  an ignored group in the established style.
- [ ] **Step 5:** commit `feat(turing-machine): fmt keeps the named spelling`.

### Task 6: headers print the canonical form

**Files:**
- Modify: `crates/turing-machine/src/header.rs` — `graph_body_lines`
  (:1854) prints bodies from rules pushed through
  `position_named_cells` (the source arm; the object arm builds
  positional rules from rows already); `pattern_text` (:2266) /
  `rule_text` (:2225) unchanged
- Test: header pins grow one twin case in
  `crates/turing-machine/tests/header_roundtrip.rs` / `header_fmt_clean.rs`

**Interfaces:**
- Consumes: Task 3's `position_named_cells`.
- Produces: identical source/object headers; stable `graph_digest`.

- [ ] **Step 1: failing test** — a named-sparse source's `tmt interface`
  output equals its positional twin's exactly; round-trip reads back;
  the digest of both twins matches so a graft between them raises no
  `GraftDrift`.
- [ ] **Step 2: watch it fail** (a header printed from source-form rules
  keeps names/sparse and diverges from the object arm).
- [ ] **Step 3: implement** — apply the helper at print time; run the
  stdlib byte pins (`std.tmh` MUST stay byte-identical — the stdlib is
  positional) and `published.rs`'s source-vs-header parity sweep.
- [ ] **Step 4:** commit `polish(turing-machine): a generated header prints the positional form of a named body`.

### Task 7: docs and the release note

**Files:**
- Modify: `docs/tmt/language.md` — a "Named vector cells" section
  (spelling, the three defaults, the three refusals, "a name is a
  spelling, not a semantics", fmt keeps the form, headers print
  positional)
- Modify: `docs/tmt/fmt.md` — the never-move reference grows the named
  slots; `docs/tmt/cli.md` — rows landed in Task 3
- Modify: `CHANGELOG.md` — an `### Named vector cells` entry under
  `[Unreleased]` whose version block states **0.3, no movement** (the
  standing bump covers it; the once-per-cycle rule one line)
- Modify: `CLAUDE.md` stays Task 1

- [ ] **Step 1:** write the pages; every claim quoted from a run
  (`tmt fmt` / `tmt interface` output pasted verbatim).
- [ ] **Step 2:** run `cargo test -p mtc-turing-machine` — drift guards
  (`docs_drift`, error-code docs) green.
- [ ] **Step 3:** commit `docs(turing-machine): named vector cells` .

### Task 8: the round's gates

- [ ] **Step 1:** `cargo test --workspace`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo fmt --check` — all green.
- [ ] **Step 2:** diff audit — only `crates/turing-machine/`, `docs/`,
  `CLAUDE.md`; PM byte-identity and core-neutrality untouched by
  construction.

## Mutation checklist (recorded at execution)

| Mutation | Repaired by | Verification |
|---|---|---|
| a name that does not resolve | `unresolved-tape-target` pins (Task 3) | red — span at the name |
| resolve names to the WRONG index | Task 4's swap-mutation pin | red — rows differ from the twin |
| fill defaults as identity instead of `*`/`-`/`.` | Task 3's positive twin + Task 4 row equality | red — rows carry the wrong cells |
| drop `name:` on the clean reprint | Task 5 token-preservation fixture | red — fmt loses a token |
| print headers from source-form rules | Task 6 twin pin | red — source ≠ object |
| named form exempt from `check_width` differently than filled | Task 3 width tests + `row-width` cases unchanged | red |
