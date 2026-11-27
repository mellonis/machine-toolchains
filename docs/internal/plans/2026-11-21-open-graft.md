# Open Graft (`*` in a graft's map) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a `graft`'s tape map end with `*` (an open map), so a graph written
over a small alphabet grafts onto a wider tape without collapsing the extra
symbols onto its own glyphs — the three binding constructs share one map
syntax and one meaning.

**Architecture:** The refusal lives in `build_tapemap`
(`crates/turing-machine/src/expand.rs`): an unlisted host symbol becomes a
read Hole, and `trap_read_rows` turns holes into `trap #0` rows that shadow the
graph's `*` rows. An open map instead makes unlisted host symbols **opaque** —
they name no graph glyph, are excluded from preimage AND holes, and only the
graph's `*` rows (already mapped `Wild → Wild`) match them. Everything happens
compiler-side in the TM crate; `crates/core`, the linker, the containers and
both formatters do not move.

**Tech Stack:** Rust (workspace, pinned toolchain), hand-written diagnostics via
`code_registry!`, derivation-first goldens and property tests.

**Spec:** `docs/internal/specs/2026-11-21-open-graft-design.md` (the plan argues
from the spec; executors read both).

## Global Constraints

- **`crates/core` does not change in this round** (neutrality gate: a TM-only
  round showing a `crates/core` diff is a design smell).
- **PM-1 byte-identity** — PM output must not move a byte. `crates/post-machine`
  is untouched; its goldens must pass unedited.
- **Closed-map behavior is byte-identical** — every map without `*` splices
  exactly as before. The existing pins
  `a_holey_graft_synthesizes_trap_read_rows` and
  `a_holey_graft_holes_an_unlisted_in_range_symbol` (expand.rs tests) must pass
  WITHOUT EDITS.
- **Openness changes only the READ side's treatment of UNLISTED host
  symbols.** The write half stays closed exactly as today (`close_unlisted` on
  unequal cards, identity completion + injectivity check on equal cards); the
  blank (index 0) stays pinned; `* as v` stays `wildcard-binding`.
- **Named map declarations still refuse the marker** (`check_named_map_decl`) —
  only SITE maps open.
- Published docs (README, `docs/`, code comments) stay ref-free and
  forge-agnostic; tracker numbers live in this plan, the spec, and GH comments.
- New code names their temp paths PID + per-call atomic counter
  (`scratch(...)`/`ScratchDir` convention in tests).
- Commits: conventional with scope (`feat(turing-machine): …`, `fix`, `test`,
  `docs`).

## Review Focus

These are the input classes a reasonable person would expect to work and the
plan's tasks must pin:

1. **An open map on an EQUAL-cardinality tape** must not silently alias an
   unlisted host symbol onto its own glyph number (today's identity completion
   is closed-map behavior). → Task 2 test
   `an_open_map_on_an_equal_card_tape_does_not_alias`.
2. **Two tapes open at once where per-position reading lies** — rows
   `[0, *]` and `[*, 1]` must be refused (the `ir.rs` doc example): the check is
   single-row cover, jointly, not "has a `*` row". → Task 3 test
   `two_open_tapes_need_joint_row_cover`.
3. **Nested open grafts** — an opaque symbol at the outer map must stay "the
   thing only `*` rows match" through a graft inside a graft, with no `trap`
   row anywhere. → Task 4 test `an_opaque_symbol_survives_a_graft_inside_a_graft`.
4. **One-way `=>` pairs under an open map** — the read side still collapses,
   the write side still never writes back through `=>`; openness must not
   resurrect write-back. → Task 4 test `a_one_way_pair_stays_closed_for_writes_under_an_open_map`.
5. **Two graft sites identical except for openness** must NOT dedup into one
   fragment and must NOT be a `duplicate-graft-instance` finding. → Task 4 test
   `openness_is_part_of_the_graft_instance_key`.

## File structure

- `crates/turing-machine/src/expand.rs` — `SymMap`/`TapeMap`/`Composite` (the
  symbol-map algebra), `build_tapemap`/`build_composite` (map legality),
  `map_rule`/`splice_state`/`trap_read_rows` (the splice), `graft_site_key_bytes`
  /`graft_instance_key` (dedup). All openness mechanics live here.
- `crates/turing-machine/src/ir.rs` — owns
  `rows_admit_the_opaque_index(states: &[IrState], k: usize)`; the coverage
  predicate is generalized here and reused by `expand.rs`.
- `crates/turing-machine/src/compiler.rs` — `CompileErrorKind` +
  `code_registry!` (`open-graft-uncovered`; `open-graft-unsupported` leaves).
- `crates/turing-machine/src/lint/rules/enters_unmet.rs` +
  `crates/turing-machine/src/head_flow.rs` — the graft-site wording (Task 5).
- Tests: inline `#[cfg(test)]` in `expand.rs`; `tests/open_bindings.rs`,
  `tests/one_definition.rs`-style CLI entries, `tests/error_code_docs.rs`
  (drift guard), `tests/header_fmt_clean.rs`.
- Docs: `docs/tmt/language.md`, `docs/tmt/cli.md`, `CHANGELOG.md`, `CLAUDE.md`,
  `crates/turing-machine/src/parser.rs` (`TMC_LANG_VERSION`).

---

### Task 1: Opaque is a third read class in the symbol-map algebra

**Files:**
- Modify: `crates/turing-machine/src/expand.rs` — `TapeMap` (~line 245),
  `TapeMap::read_image`/`preimage`/`holes` (~260-291), `Composite::key`
  (~297-312), `build_tapemap` (~1091): the marker check at ~1129 stops refusing
  (full splice honesty is Task 2; this task only makes the data model able to
  say "open").
- Test: inline in `crates/turing-machine/src/expand.rs` `#[cfg(test)]`.

**Interfaces:**
- Consumes: `SymMap { pairs: BTreeMap<u16, u16>, holes: BTreeSet<u16> }`,
  `build_symbol_maps`, `check_injective_completion`, `close_unlisted` (all
  unchanged).
- Produces (Tasks 2-4 rely on these exact names):
  - `enum ReadClass { Image(u16), Opaque, Hole }` (private to `expand.rs`).
  - `TapeMap.open: bool` — set from the site map's `*` marker.
  - `TapeMap::read_class(&self, p: u16) -> ReadClass`: on a closed map exactly
    today's `read_image` classification (`Image(v)` / `Hole`); on an open map:
    blank → `Image(0)`, a listed source → `Image(dst)` bounds-checked, anything
    else → `Opaque`.
  - `TapeMap::read_image` keeps its signature: `Image(v) → Some(v)`, `Opaque` |
    `Hole → None`. `TapeMap::holes()` now returns only `Hole`s.
  - `Composite::key()` encodes per-tape openness (one byte per tape inside the
    existing per-tape key bytes) — the dedup law: two splices differing in
    openness or opaque membership cannot share a key.

- [ ] **Step 1: Write the failing tests** (inline, `expand.rs::tests`):
  `open_map_makes_unlisted_symbols_opaque_not_holes` (build a `TapeMap` from a
  site map `{ '0' -> '0', * }` over host card 4 / graph card 3: `read_class(0)
  == Image(0)`, `read_class(1..=2)` images their pairs' destinations,
  `read_class(3) == Opaque`, `holes()` is empty),
  `open_map_preimage_excludes_opaque` (the `preimage` of every graph symbol
  contains no unlisted host symbol),
  `openness_is_in_the_dedup_key` (`Composite::key()` differs between
  otherwise-identical open and closed tape maps).
- [ ] **Step 2: Run tests to verify they fail**

  Run: `cargo test -p mtc-turing-machine --lib expand::tests::open_`
  Expected: FAIL — `open` field / `read_class` do not exist.
- [ ] **Step 3: Implement `ReadClass`, `TapeMap.open`, `TapeMap::read_class` and redefine `holes()`**

  Minimal refactor: keep `read_image`/`preimage` as they are; add the flag to
  `TapeMap` and thread it through `build_tapemap` (remove the
  `OpenGraftUnsupported` early return — set `open: true` instead;
  `open-graft-unsupported`'s full retirement is Task 2's step).
  `check_injective_completion` on an open equal-card map validates the PAIRS
  only (no identity completion to protect).
- [ ] **Step 4: Run tests to verify they pass, and the closed-shape pins are green**

  Run: `cargo test -p mtc-turing-machine --lib expand::tests`
  Expected: PASS — including `a_holey_graft_synthesizes_trap_read_rows`,
  `a_holey_graft_holes_an_unlisted_in_range_symbol`,
  `two_identical_grafts_dedup_to_one_fragment` UNEDITED.
- [ ] **Step 5: Commit**

  ```bash
  git add crates/turing-machine/src/expand.rs
  git commit -m "feat(turing-machine): an open graft map makes its unlisted symbols opaque"
  ```

### Task 2: The splice honors openness; the marker is accepted

**Files:**
- Modify: `crates/turing-machine/src/expand.rs` (`map_rule`, `splice_state`,
  `trap_read_rows` — behavior follows from Task 1's `holes()`; verify, do not
  re-mechanize), `build_tapemap` marker handling and its doc comment.
- Modify: `crates/turing-machine/src/compiler.rs` — delete
  `CompileErrorKind::OpenGraftUnsupported`, its `code()` arm and its message.
- Test: `crates/turing-machine/tests/open_bindings.rs` (flip
  `an_open_map_at_a_graft_site_is_refused`), inline expand tests,
  `crates/turing-machine/tests/error_code_docs.rs` inventory.

**Interfaces:**
- Consumes: Task 1's `ReadClass`/`read_class`/`open`.
- Produces: `build_tapemap(map: Option<&Map>, …) -> Result<TapeMap, CompileError>`
  accepts `m.open`; the splice's runtime meaning (a pin the CLI can show):
  an unlisted host symbol reads through a `*` row as "anything else", never as
  a glyph and never as `trap #0`.

- [ ] **Step 1: Write the failing tests**

  CLI-level in `open_bindings.rs` (reuse its `run_one_tape`/`link_image`
  helpers — two tapes: host `wide { '_', '^', '$', '0', '1' }`, graph `bits {
  '_', '0', '1' }`, graft with `map { '0' -> '0', '1' -> '1', * }`):
  - `an_open_graft_lets_unlisted_symbols_match_only_wildcards` — the graph's
    `[*] -> move [>] goto s;` state walks over `'^'` and `'$'` unchanged and
    stops on `'_'`; final tape snapshot asserted.
  - `an_open_map_on_an_equal_card_tape_does_not_alias` (Review Focus 1) —
    equal-card alphabets `{ '_', '0', '1' }` and `{ '_', 'a', 'b' }` with
    `map { '_' -> '_', * }`: the unlisted `'0'`/`'1'` reach the graph only
    through `*` rows; a run that the old identity aliasing would corrupt is
    asserted byte-exact.
  Inline in `expand.rs::tests`:
  - `an_open_graft_synthesizes_no_trap_read_rows` — the spliced fragment of a
    reading state under an open map contains no `Transition2::TrapRead`.
  - `an_open_graft_holes_a_map_out_of_range_write` — openness does not change
    the write half: a graph write with no host image is still `TrapWrite` /
    `trap #1`.
- [ ] **Step 2: Run tests to verify they fail**

  Run: `cargo test -p mtc-turing-machine --test open_bindings`
  Expected: `an_open_map_at_a_graft_site_is_refused` fails on the missing error
  (it flips this step), the new tests fail on `open-graft-unsupported`.
- [ ] **Step 3: Accept the marker and retire `open-graft-unsupported`**

  Delete the error variant, its `code()` arm, the message text, and its row in
  `docs/tmt/cli.md`'s error table (the `error_code_docs.rs` drift guard turns
  red until the row is gone — run it to confirm it bites). Keep
  `a_named_map_declaration_still_refuses_the_marker` and
  `a_bound_wildcard_is_still_refused` green (no edits).
- [ ] **Step 4: Flip and rename the refusal test**

  `an_open_map_at_a_graft_site_is_refused` becomes
  `an_open_map_at_a_graft_site_is_accepted` asserting the splice builds and the
  map's `*` is load-bearing (the run differs from the closed-map run). Update
  `graft_err`/`graft_map_legality_errors`'s open case out of the error list.
- [ ] **Step 5: All green**

  Run: `cargo test -p mtc-turing-machine`
  Expected: PASS; the two `a_holey_graft_*` pins and `tmc_property.rs`'s fmt
  laws hold untouched.
- [ ] **Step 6: Commit**

  ```bash
  git add -A crates/turing-machine docs/tmt
  git commit -m "feat(turing-machine): an open graft map is legal; its unlisted symbols read as other"
  ```

### Task 3: The coverage refusal — `open-graft-uncovered`

**Files:**
- Modify: `crates/turing-machine/src/ir.rs` — generalize
  `rows_admit_the_opaque_index`'s single-row-cover body so the same function
  answers for expanded rows (keep the name and its doc; add a row-slice
  accessor variant, e.g. `pub(crate) fn rows_admit_opaque_index_over_rows(
  rows: &[impl AsRef<[[Cell-like]]>…` — exact generic is the implementer's,
  ONE implementation is the constraint).
- Modify: `crates/turing-machine/src/expand.rs` — the check at the instance
  splice (where the composite and the graph's expanded states are both in
  hand): for every open-bound graph tape (signature index `k`), assert
  single-row cover over every state; name the first state that fails.
- Modify: `crates/turing-machine/src/compiler.rs` — `CompileErrorKind::
  OpenGraftUncovered { state: String, tape: String }`, `code()` →
  `"open-graft-uncovered"`, message: the state, the tape parameter, and what
  would fix it ("give the state a row whose `<tape>` cell is `*`, or name the
  symbols in the map").
- Test: inline expand tests; `tests/error_code_docs.rs` (+ row in
  `docs/tmt/cli.md`).

**Interfaces:**
- Consumes: Task 1's `Composite` open flags (which graph tapes are open),
  `expand.rs`'s expanded body (`ExpandedState.rules[].pattern: Vec<Cell>`).
- Produces: `CompileErrorKind::OpenGraftUncovered`; the shared predicate used
  by both `ir.rs` (routine opacity) and `expand.rs` (this check) — the two MUST
  not drift.

- [ ] **Step 1: Write the failing tests** (inline, `expand.rs::tests`):
  - `a_state_without_a_covering_wildcard_is_refused` — graph with states
    `['0'] -> …; ['_'] -> …` (no `*` row) grafted with `*` → error
    `open-graft-uncovered` naming the state and the tape.
  - `two_open_tapes_need_joint_row_cover` (Review Focus 2) — the `ir.rs` doc
    example rows `[0, *]` and `[*, 1]` over two open-bound tapes → refused.
  - `a_vacuously_opaque_graph_is_accepted` — a world with no reading state
    passes (the predicate's vacuous truth).
- [ ] **Step 2: Run to verify they fail**

  Run: `cargo test -p mtc-turing-machine --lib expand::tests`
  Expected: FAIL — variant missing.
- [ ] **Step 3: Generalize the predicate and implement the check**

  Extract `ir.rs::rows_admit_the_opaque_index`'s inner cover relation so it
  runs over pattern slices; both callers use it. `ir.rs`'s own doc-example test
  stays and now also pins the graft-side behavior (one implementation).
- [ ] **Step 4: Registry + docs**

  Add the variant to `code_registry!`, the row to `docs/tmt/cli.md`'s table;
  run `cargo test -p mtc-turing-machine --test error_code_docs` — green.
- [ ] **Step 5: All green + commit**

  Run: `cargo test -p mtc-turing-machine` — PASS.

  ```bash
  git add crates/turing-machine/src docs/tmt
  git commit -m "feat(turing-machine): a graft whose graph would decide a row on an opaque symbol is refused"
  ```

### Task 4: Nesting, the dedup key, and the write half

**Files:**
- Modify: `crates/turing-machine/src/expand.rs` (only if Task 1's key encoding
  needs the nested thread-through), `tests/` inline.
- Test: inline expand tests; a `duplicate-graft-instance` lint pin in
  `crates/turing-machine/tests/` (the lint reads `graft_instance_key` — its
  module doc's "can never disagree with the expander" contract).

**Interfaces:**
- Consumes: `graft_site_key_bytes`, `graft_instance_key`, `Composite::key`
  (Task 1), the executor of nested grafts (`expand_grafts_into`, memoized
  per `(module_id, name)`).
- Produces: the pinned laws — nested opacity, key discrimination, closed write
  half. No new API.

- [ ] **Step 1: Write the failing tests:**
  - `an_opaque_symbol_survives_a_graft_inside_a_graft` (Review Focus 3) —
    host wide; `outer` grafs `inner` with an open map; `inner` is grafted into
    `outer`'s own small-alphabet tape with an open map too. Feed host symbols
    unlisted at BOTH maps: assert the run reaches the graph's `*` behavior at
    every depth and never `trap #0`/`#1` (run-level assertion + a fragment
    assertion that no `TrapRead` row exists). Also assert the reverse order of
    nesting (graft-of-graft-through-a-header shape) stays correct — reuse the
    `nested_graft.tmc` golden's shape.
  - `openness_is_part_of_the_graft_instance_key` (Review Focus 5) — two sites
    identical except `*` produce two fragments
    (`two_identical_grafts_dedup_to_one_fragment` inverted), and `tmt lint`
    emits NO `duplicate-graft-instance` for the pair.
  - `a_one_way_pair_stays_closed_for_writes_under_an_open_map` (Review Focus 4)
    — `map { '0' => '0', * }` (one-way): read admits opaque; a graph write of
    an unlisted symbol still `trap #1`; no write-back through `=>`.
- [ ] **Step 2: Run to verify they fail** — `cargo test -p mtc-turing-machine`
  Expected: FAIL on the new names.
- [ ] **Step 3: Implement what the pins require** (thread openness through the
  instance key if Task 1 left any path out; no mechanism changes otherwise).
- [ ] **Step 4: All green + commit**

  Run: `cargo test -p mtc-turing-machine` — PASS.

  ```bash
  git add crates/turing-machine
  git commit -m "test(turing-machine): opaque symbols survive a graft inside a graft, and openness is part of the instance key"
  ```

### Task 5: What the diagnostics say at an open graft

**Files:**
- Modify: `crates/turing-machine/src/lint/rules/enters_unmet.rs` (+ possibly
  `crates/turing-machine/src/head_flow.rs` for gap classification).
- Test: `crates/turing-machine/src/lint/rules/enters_unmet.rs`'s existing test
  module; `tests/header_fmt_clean.rs` if the printer needs a case.

**Interfaces:**
- Consumes: `enters_gaps`, `EntersGap { offending, opaque, … }`, the
  `offenders()` renderer (its open-map wording
  "a symbol this open map sends to the callee's opaque index" already exists
  for call/bind).
- Produces: at an open GRaFT site the finding names the class in graft words —
  "a symbol this open graft sends past the graph's alphabet" (exact wording the
  implementer may adjust to the message's grammar, but it must name the opaque
  class and join offending glyphs with the existing `", or "` rule).

- [ ] **Step 1: Write the failing test** —
  `an_open_graft_names_the_opaque_class_in_enters_unmet`: a graph declaring
  `enters { '0' }` on a tape, grafted with `*`, at a site where the head may be
  on an unlisted symbol; the finding is warn-tier and its message names the
  opaque class (not "a glyph").
- [ ] **Step 2: Verify it fails** (the gap is unclassified or named as a glyph).
- [ ] **Step 3: Implement the classification/wording**; confirm
  `--allow enters-unmet` still suppresses, and that call/bind wording is
  unchanged (the existing tests pass unedited).
- [ ] **Step 4: `empty-expansion` clause** — check the warning's wording against
  an open map (a rule whose cells all die by preimage is the same warning);
  adjust one clause in the message ONLY if it reads wrong for open maps, and
  pin. If it reads true, leave it and say so in the commit body.
- [ ] **Step 5: Header round-trip pin** — one case in `header_fmt_clean.rs`:
  a unit whose exported graph body carries an open graft prints a header that
  `tmt fmt`-canonicalizes back byte-identically (the printer already routes
  through the formatter; this is a pin, not a mechanism).
- [ ] **Step 6: Commit**

  ```bash
  git add -A crates/turing-machine
  git commit -m "fix(turing-machine): the open-graft opaque class is named in the diagnostics that mention it"
  ```

### Task 6: The version, and the docs that carry the rule

**Files:**
- Modify: `crates/turing-machine/src/parser.rs:47` — `TMC_LANG_VERSION`
  `"0.2"` → `"0.3"` (+ its doc comment: one line that 0.3 admits open graft
  maps).
- Modify: `docs/tmt/language.md` — a "(open grafts)"-named subsection under the
  graft/binding prose: the marker's meaning at a graft site, the opaque class,
  `open-graft-uncovered`, writes stay closed, named maps still refuse.
- Modify: `docs/tmt/cli.md` — codes table rows (Task 2 removed one, Task 3
  added one; this task checks the walk through `docs/tmt/lint.md`'s
  `HEADER_RULES`-style inventories needs nothing).
- Modify: `CHANGELOG.md` — `## [Unreleased]` entry in ref-free prose: the
  behavior change plus the version block line `.tmc 0.2 → 0.3` with the other
  spaces marked `unchanged`.
- Modify: `CLAUDE.md` — standing state: the contract-table `.tmc` row
  (0.2 → 0.3) and, in the `.tmc` front-end bullet list, one clause that an open
  graft map makes unlisted host symbols opaque (replacing the current silence).

- [ ] **Step 1: Make each edit above**; every code comment this round added
  cites the durable page + keyword (`docs/tmt/language.md (open grafts)`), not
  this plan or the spec.
- [ ] **Step 2: Drift guards prove the docs** —
  `cargo test -p mtc-turing-machine --test error_code_docs`,
  `cargo test -p mtc-turing-machine --test completions_registry`,
  and grep the docs for the two code names.
- [ ] **Step 3: Run the docs' own example** — the language.md open-graft
  example is a real file; build and run it with `target/debug/tmt` and paste
  nothing fabricated into the page.
- [ ] **Step 4: Commit**

  ```bash
  git add -A
  git commit -m "docs(turing-machine): open grafts; .tmc dialect 0.2 -> 0.3"
  ```

### Task 7: The whole-branch gate

- [ ] **Step 1: Full suite** — `cargo test --workspace`. Record counts.
- [ ] **Step 2: Quality gates** — `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo fmt --check`. Both clean.
- [ ] **Step 3: Standing regression gates that this round can move:**
  PM goldens (byte-identity) unedited and green; `opt_equivalence` (both crates)
  green — expansion changes rows, so `-O0` bit-identity and the `brk` barrier
  must hold; everything-matrix green (spliced rows flow through all three
  call mechanisms only via nested calls — fine, but do not skip).
- [ ] **Step 4: Negative controls the round actually needs** — the
  compiled-stdlib byte-identity gate at BOTH opt levels ONLY IF any formatter
  corpus text changed (expected: no. If no fmt file changed, say so in the
  report rather than running it for ritual).
- [ ] **Step 5: Report** — working tree clean, every task's commit listed, the
  run transcript quoted in the completion report. No plan-box ticking without
  this evidence.

---

## Tests that flip

| Test | Disposition |
|---|---|
| `open_bindings.rs::an_open_map_at_a_graft_site_is_refused` | flipped in Task 2 → `an_open_map_at_a_graft_site_is_accepted` |
| `expand.rs::tests::graft_map_legality_errors` (its open case) and the `graft_err` helper's expectations | the open map leaves the error list; every OTHER case stays |
| `error_code_docs.rs` inventory ↔ `docs/tmt/cli.md` table | `open-graft-unsupported` row deleted (Task 2), `open-graft-uncovered` row added (Task 3) — the guard must go red before each docs edit lands |
| `compiler.rs` tests asserting `OpenGraftUnsupported` | deleted with the variant (search `open-graft-unsupported` everywhere: message text, `PEER_CHECK_CODES`, lint allow-lists) |

## Self-review

1. **Spec coverage:** goals (marker legal; opaque not aliased/holed; dedup key
   law; coverage refusal; runs) → Tasks 1-4; the two decisions (refusal not
   trap; `.tmc` 0.3) → Tasks 3, 6; `enters-unmet` question → Task 5; builder
   items (key representation, nested law, `empty-expansion`, header printing,
   blank handling) → Tasks 1, 4, 5. No gap.
2. **Step scan:** each step names tests, commands, or exact edit targets; no
   "handle edge cases"; no body transcribed where the signature and pins
   decide.
3. **Type consistency:** `ReadClass`, `TapeMap::read_class`, `Composite::key`,
   `OpenGraftUncovered { state, tape }`, `rows_admit_the_opaque_index` reused
   under one implementation — names consistent across tasks.
4. **Review Focus:** all five lines have a named pin in their owning task.
5. **Proportion:** plan ≈ code; the splice mechanism is prescribed, its bodies
   are not.
