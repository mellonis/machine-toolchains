# Footprint Engine + dead-map-pair Lint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-world per-tape write-set inference for TM-1 (the #53 inference half), with four consumers: the `dead-map-pair` lint (write-half only, demote quickfix), LSP hover write-set lines, `tmt ir footprints`, and the over-approximation property corpus — zero codegen impact.

**Architecture:** One new module `crates/turing-machine/src/footprint.rs` holds a `u128`-bitset symbol-set primitive and TWO walks sharing one write-back projection helper: an IR walk (`infer_ir`, monotone fixpoint following `CallThen` AND `TailCall`) for CLI/plan-5 consumers, and a source-level walk (`infer_resolved`, over `Resolved` worlds in each world's OWN alphabet frame) for the lint and hover — matching the house pattern where lints re-detect source-level (`unused-routine`, `binding-product-threshold` precedents) because the lint layer sees post-resolve only. NOTE a deliberate deviation from the spec's letter: §5.1 sketches the pre-splice computation "in `expand.rs`", but the lint cannot reach expand (the module contract at `lint/mod.rs:15-19`), and graphs contain no calls, so their own-frame write-sets derive directly from `Resolved` — the spec's SUBSTANCE (the write-set lives in the graph's own alphabet frame, the #53 alphabet-frame hazard) is honored; only the file placement moves. Footprints are derived data: never serialized into `.ir.json` or MO.

**Tech Stack:** Rust, no new dependencies. Driving spec: `docs/superpowers/specs/2026-08-08-volatile-async-footprint-design.md` §5 (this plan), §6 is plan 5 (NOT here — no grammar, no reserved words, no contracts).

**Base:** branch `feat/footprint` from master `3a8b56f` (post plan-3 merge).

## Global Constraints

- **Post-resolve lint boundary holds** (`lint/mod.rs:15-19`: "The lint runs only over the resolution stage's output, never `expand`/`lower`"): the lint consumes `infer_resolved`, never `Expanded` or `IrProgram`. `expand.rs` is NOT modified.
- **Conservatism direction: over-approximation ONLY.** Inferred ⊇ actual is the soundness contract; when a projection rule is uncertain, ADD to the set. The property test (Task 6) is the net. This is what makes the lint's "never writes b" conclusion sound: b ∉ inferred ⇒ b ∉ actual.
- **New default-on lint must be finding-free on `tests/golden/*.tmc` AND the embedded `std.tmc`** (`tmc_golden.rs::object()` fails on ANY diagnostic; `stdlib_golden.rs` pins the stdlib). Task 3 has an explicit checkpoint: any finding there → STOP, report to the controller, do not "fix" the corpus or stdlib yourself.
- **Zero codegen/object-byte impact:** this plan changes no compiler/optimizer/codegen path. Every existing byte-identity pin (`stdlib_object_bytes`, PM suites, the TM everything-matrix) must pass UNCHANGED — a diff there means this plan leaked into codegen, which is a bug.
- **`TM_IR_VERSION` stays 2**; `ir.rs::serde_tags_are_frozen` untouched (no new IR variants or fields).
- **Symbol sets are `u128` bitsets**: alphabets are capped at 127 glyphs (`compiler.rs:640` "at most 127 long"; the assembler's 127-symbol ceiling), so one `u128` holds any alphabet with a bit to spare.
- **Version spaces:** NOTHING moves. `TMC_LANG_VERSION` 0.1 (no grammar change in this plan), IR 2, containers untouched, PM untouched entirely (`git diff --stat -- crates/post-machine crates/core` must be empty at the end).
- Published docs forge-agnostic ref-free prose (no issue numbers, no `Task N`/`spec §N`, no superpowers paths); code comments cite durable pages only (`docs/tmt/lint.md (dead-map-pair)` style).
- Conventional commits with scope; never `--amend`; no AI attribution.
- Gates per task: touched suites foreground; full set at the end (three per-package `cargo test -q`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build -p mtc-core --no-default-features`).

---

### Task 1: the footprint module — SymSet + the IR walk

**Files:**
- Create: `crates/turing-machine/src/footprint.rs`
- Modify: `crates/turing-machine/src/lib.rs` (add `pub(crate) mod footprint;` — the module list at lib.rs is 18 lines of `mod` declarations, alphabetical)

**Interfaces (produced — later tasks rely on these exact names):**

```rust
/// A set of symbol indices on one tape. Alphabets are capped at 127
/// glyphs, so one u128 holds any alphabet (docs/tmt/language.md (alphabets)).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SymSet(u128);

impl SymSet {
    pub(crate) fn empty() -> Self;
    pub(crate) fn full(cardinality: u32) -> Self;     // bits 0..cardinality
    pub(crate) fn insert(&mut self, index: u32);
    pub(crate) fn contains(&self, index: u32) -> bool;
    pub(crate) fn union_with(&mut self, other: SymSet) -> bool; // true if grew — the fixpoint driver
    pub(crate) fn is_superset(&self, other: SymSet) -> bool;
    pub(crate) fn iter(&self) -> impl Iterator<Item = u32> + '_; // ascending
    pub(crate) fn len(&self) -> u32;
}

/// Write-sets for one world, one entry per tape in tape order.
pub(crate) struct WorldFootprint { pub(crate) tapes: Vec<SymSet> }

/// Keyed by mangled world name (IrWorld::name / ResolvedWorld key).
pub(crate) struct FootprintTable { pub(crate) worlds: std::collections::HashMap<String, WorldFootprint> }

pub(crate) fn infer_ir(program: &IrProgram) -> FootprintTable;
```

- Consumes: `ir.rs` types — `IrProgram`, `IrWorld`, `IrRule.write: Option<Vec<IrWrite>>` (`IrWrite::Index { index }`; `Keep` contributes nothing), `IrTransition::{CallThen { target, binding, then }, TailCall { target }}`, `IrTapeBinding { caller_tape, pairs }`, `IrMapPair { src, dst, one_way }`.

**The inference rules (pinned from the spec):**

1. Direct writes: every `IrWrite::Index { index }` in world W's rules adds `index` to W's set for that tape position (write vectors are arity-wide, per-tape by position; `write: None` = all-keep).
2. `CallThen { target, binding, .. }`: if `target` names a world in the program, project the callee's per-tape sets back: callee tape `k`'s set lands on caller tape `binding[k].caller_tape`, each callee symbol `dst` mapping to caller symbol `src` through the pairs with `one_way == false` (**one-way pairs never write back — their write half contributes nothing**). An EMPTY binding is identity: callee tape `k` ↔ caller tape `k`, symbols unchanged.
3. `TailCall { target }`: carries NO binding by construction (bindless calls only) → identity projection, all tapes.
4. Unresolvable `target` (not among `program.worlds` names — an external/library call): conservative — the FULL alphabet (`SymSet::full(tape.cardinality)`) on every bound caller tape (for `CallThen` with a binding: the `caller_tape`s it names; empty-binding `CallThen` and `TailCall`: every tape).
5. Recursion (direct or mutual): monotone fixpoint — iterate projection until no `union_with` grows any set. Termination: sets only grow, bounded by `full(card)`.
6. `Goto`/`Return`/`Stop`/`Halt`/`TrapRead`/`TrapWrite`: no contribution beyond the row's own write cells.

- [ ] **Step 1: Survey the one open semantic** — how `ir::lower` builds `IrMapPair` lists: is a non-empty binding's pair list EXHAUSTIVE (identity-completed at lowering) or sparse-with-identity-default? Read `ir.rs::lower`'s binding construction and one `.ir.json` from a golden fixture with an unequal-cardinality call (`a5_call_across_alphabets.tmc`). Record the answer as a doc comment on the projection helper AND pin it in a unit test (build the binding the lowerer builds, assert the projection result). If sparse-with-default: unlisted callee symbols write back identically on equal cardinality and contribute NOTHING on unequal (mirror of `expand.rs::close_unlisted`'s closed-on-unequal rule); when the reading is ambiguous, over-approximate (add identity images) and note it.
- [ ] **Step 2: Write failing unit tests** in `footprint.rs`'s `#[cfg(test)] mod tests` (build small `IrProgram`s by hand, the way `ir.rs::serde_tags_are_frozen` does):
  - `direct_writes_are_collected` — one world, two tapes, writes {1} and {2,3} → sets match, keep-only tape is empty.
  - `call_projects_through_the_binding_pairs` — caller calls callee with pairs `[{src:2,dst:1,one_way:false}, {src:3,dst:2,one_way:true}]`; callee writes {1,2} on its tape 0 → caller gains {2} (via the bidirectional pair), NOT 3 (one-way never writes back).
  - `empty_binding_is_identity` — callee writes {1} on tape 1 → caller tape 1 gains {1}.
  - `tail_call_is_identity_on_all_tapes`.
  - `mutual_recursion_reaches_a_fixpoint` — A calls B, B calls A, each with one direct write → both worlds' sets contain both writes; the test also guards termination by simply completing.
  - `unknown_target_is_conservatively_full` — a call to `"lib::helper"` (absent) with a binding naming caller tape 0 → caller tape 0 is `full(card)`.
  - `symset_full_iter_and_superset` — the primitive itself.
- [ ] **Step 3: Run to confirm red** (module doesn't exist): `cargo test -p mtc-turing-machine --lib footprint` — expect compile failure, then after stubbing, assertion failures.
- [ ] **Step 4: Implement** `SymSet` + `infer_ir` per rules 1-6. The fixpoint: seed all worlds with direct writes, then loop worlds×rules applying projections until an entire pass grows nothing. Doc comment on the module head carries the soundness contract (over-approximation) in prose.
- [ ] **Step 5: Green** — `cargo test -p mtc-turing-machine --lib footprint`, then `cargo test -q -p mtc-turing-machine` (nothing else may change), `cargo fmt --check`, `cargo clippy -p mtc-turing-machine --all-targets -- -D warnings`.
- [ ] **Step 6: Commit** `feat(turing-machine): footprint inference — SymSet and the IR walk`.

---

### Task 2: the source-level walk + the stdlib claim pinned

**Files:**
- Modify: `crates/turing-machine/src/footprint.rs`

**Interfaces:**
- Produces: `pub(crate) fn infer_resolved(resolved: &Resolved) -> FootprintTable` — same table shape, derived from source-form `ResolvedWorld` rules; each world's sets are in that world's OWN alphabet frame (glyph → index via `resolved.alphabets[&tape.alphabet].glyphs`, `glyphs[0]` = blank = index 0).
- Consumes: `compiler.rs` — `Resolved { worlds, alphabets, .. }`, `ResolvedWorld` (source-form rules, `calls`, `binds: Vec<ResolvedBind>`, `grafts: Vec<ResolvedGraft>`), `ResolvedAlphabet { glyphs }`, `lint/patterns.rs::glyph_label` for glyph↔label handling.

**Rules (mirror of Task 1 in source form):**
- Direct writes: a rule's write cells are glyph literals → indices in the world's own frame. Range/vector forms: reuse the label helpers `lint/patterns.rs` exposes rather than re-deriving.
- Graphs contain no calls (`GraftCallUnsupported` is a compile error), so a graph's footprint is DIRECT — no fixpoint needed for graphs.
- A routine's calls and binds project transitively (same fixpoint), pairs read from the SOURCE arg form (`BindingArg` with `MapArrow::Bidirectional` vs one-way); one-way contributes nothing; omitted map = identity; unresolvable target (external `use` path, `bind … external`) → full alphabet on the bound tapes.
- A world's GRAFTS also contribute: a grafted graph's writes land on the host's tapes through the graft's map (write-back direction: graph symbol `dst` of a bidirectional pair `src -> dst` writes back as host `src`; graph symbols with no bidirectional pair follow the identity-on-equal / nothing-on-unequal rule; when uncertain, over-approximate).

- [ ] **Step 1: Survey the source shapes** — the exact types of a source-form call transition's args and `ResolvedGraft.args` / `ResolvedBind.args` (`BindingArg`: param name, tape value, optional map with pair list + `MapArrow`), and where per-pair spans live (needed by Task 3 — record the answer in your report even though this task doesn't use spans). Read `compiler.rs:817-880` and the parser types it references.
- [ ] **Step 2: Write failing tests** (compile small sources through `compile`/`analyze` to get a `Resolved` — the lint rules' in-file tests show the pattern):
  - `source_walk_matches_the_ir_walk_on_a_call_chain` — one fixture with a two-routine call chain and a map: `infer_resolved` and `infer_ir` (over the `-O0` lowered program) agree on every world both compute (routine worlds; the source walk also has graph worlds the IR lacks).
  - `a_graph_footprint_is_direct_and_in_its_own_frame` — a graph writing `'1'` in a 3-glyph alphabet → set {index of `'1'`}, computed with no fixpoint involvement.
  - `a_graft_projects_into_the_host_frame` — host grafts a graph with `'^' => '_', '0' -> '0'`; graph writes `'0'` and `'_'` → host gains `'0'` (bidirectional) and NOT `'^'`/`'$'` (the blank write-back follows the pinned unlisted rule — assert whatever Step 1 of Task 1 established).
  - **`bare_invert_never_writes_a_blank`** — the spec's load-bearing stdlib claim, now checkable: `infer_resolved` over the embedded `std.tmc` → the set for `std::binaryNumbersBare::invertNumber` tape 0 equals exactly {index('0'), index('1')} and does NOT contain 0 (the blank). The doc line at `std.tmc:246-247` ("survive the call because bare invert never writes a blank") is the claim this test pins.
- [ ] **Step 3: Red, then implement, then green** — same gate set as Task 1 Step 5.
- [ ] **Step 4: Commit** `feat(turing-machine): source-level footprints in each world's own frame`.

---

### Task 3: the dead-map-pair lint

**Files:**
- Create: `crates/turing-machine/src/lint/rules/dead_map_pair.rs`
- Modify: `crates/turing-machine/src/lint/rules/mod.rs` (the `pub(crate) mod` list), `crates/turing-machine/src/lint/mod.rs` (`RULES` table — default-on, entry `("dead-map-pair", rules::dead_map_pair::check)`)
- Test: in-file `#[cfg(test)]` + `crates/turing-machine/tests/lint_programs.rs`

**Semantics (pinned by the spec):**
- Scope: every map-bearing binding site in a world — graft args, bind args, call args. For each **bidirectional** pair `a -> b`: resolve the callee (graft target = local graph, always resolvable; call/bind target = local routine, or unresolvable → **silent**); look up the callee's own-frame footprint (`infer_resolved`) for the bound callee tape; if `index(b) ∉ footprint` → the write-back half can never fire → finding.
- One-way pairs (`a => b`) are NEVER reported (no write half exists).
- Finding: `code: "dead-map-pair"`, span = the pair (or the arrow token if per-pair spans are narrower), message: `` the write-back half of `'^' -> '_'` never fires: `<callee>` never writes '_' `` (callee = the mangled target name).
- Quickfix: **demote** the arrow — one `Edit` replacing the pair's `->` token with `=>`; description `` demote to a one-way pair (`'^' => '_'`) ``; `Applicability::MachineApplicable`. Soundness: over-approximation means `b ∉ inferred ⇒ b` is truly never written, so the `wmap` entry (grafts) / the pair's write-back (calls) is never consulted — behavior is preserved by construction. NOT deletion (that would kill the live read half).

- [ ] **Step 1: Write failing tests** (in-file `findings(src)` helper filtered on the code, per house style):
  - `a_dead_bidirectional_pair_is_reported_on_a_graft` — host grafts a graph that never writes the pair's dst → one finding, the message above.
  - `a_live_pair_is_not_reported` — the graph writes dst → clean.
  - `a_one_way_pair_is_never_reported` — even when dst is unwritten.
  - `a_call_and_a_bind_are_scanned_too` — same shape through `call … with map` and a `bind`.
  - `an_unresolvable_target_is_silent` — a `use`-imported external target with a suspicious pair → clean.
  - `the_fix_demotes_the_arrow_only` — apply the fix (the `apply_fix` helper in `lint_programs.rs`), assert the source now reads `=>` at exactly that pair, re-lint clean, still compiles.
- [ ] **Step 2: Red** (rule module absent).
- [ ] **Step 3: Implement.** The rule computes `infer_resolved(ctx.resolved)` ONCE per run (the table is small; `run_rules` gives no cross-rule cache — note the cost decision in a comment). Arrow-token span: if Task 2's survey found per-pair spans in the source args, use them; otherwise recover the arrow token from `ctx.tokens` between the pair's two symbol spans (the `as_clause_span` token-recovery precedent in `unused_graft_name.rs`); `None` → finding without fix, per house rule.
- [ ] **Step 4: THE CORPUS CHECKPOINT** — run the lint over every `tests/golden/*.tmc` AND the embedded `std.tmc` (a quick in-file test: `lint(include_str!(...))` filtered on the code, plus the golden loop). **Expected: zero findings** (`std.tmc:257`'s digit pairs are live — bare invert writes both digits). ANY finding here → STOP, report BLOCKED to the controller with the finding verbatim; do not edit the corpus or stdlib.
- [ ] **Step 5: Integration tests in `lint_programs.rs`**: a dirty file exits 1 via the CLI route; `--allow dead-map-pair` suppresses (the `known_code` union picks the new name up automatically — assert it); the demote-fix corpus test from Step 1 repeated through `assert_deletion_fix`'s pattern but asserting the ARROW EDIT, plus the strongest form for a graft case: **apply the fix, recompile both sources, assert the two objects are byte-identical** (the dead `wmap` entry produced no spliced row, so demotion is compile-output-neutral for grafts).
- [ ] **Step 6: Green** — `cargo test -q -p mtc-turing-machine` (this includes `tmc_golden`'s zero-diagnostic assertion over the corpus — the real gate), fmt, clippy.
- [ ] **Step 7: Commit** `feat(turing-machine): the dead-map-pair lint with the demote quickfix`.

---

### Task 4: hover write-set lines

**Files:**
- Modify: `crates/turing-machine/src/lsp/navigate.rs` (`render`, `world_head` neighborhood — navigate.rs:658/742)
- Test: `crates/turing-machine/src/lsp/tests.rs`

**Format (pinned):** for a `Target::World` hover (routine / graph / machine), between the head line and the doc body, one line per signature tape in signature order:

```
routine std::binaryNumbersBare::invertNumber(tape num: symbols)

writes num: {'0', '1'}

? Flip every bit of a number. …
```

- `writes <tape>: {<glyphs, ascending index, single-quoted, comma+space>}`; the empty set renders `writes num: {}` — the empty set IS the marker-preservation signal, never omit it.
- Computed on demand: `render` calls `crate::footprint::infer_resolved(state.resolved.as_ref()?)` for the one hovered world's row. Per-hover recomputation is the deliberate choice (hover is rare, sources are small); note it in a comment. `DocState` is not widened.

- [ ] **Step 1: Write failing tests** in `lsp/tests.rs` beside the existing hover tests (:1219 region): `hovering_a_routine_shows_its_write_sets` (a routine writing `'1'` on one tape, nothing on another → both lines, one `{}`) and `hover_write_sets_project_through_a_call` (the routine's set includes a callee's write via the map).
- [ ] **Step 2: Red → implement → green** (`cargo test -p mtc-turing-machine --lib lsp`, then the crate suite, fmt, clippy).
- [ ] **Step 3: Commit** `feat(turing-machine): hover shows inferred per-tape write-sets`.

---

### Task 5: `tmt ir footprints`

**Files:**
- Modify: `crates/turing-machine/src/cli/inspect.rs` (`IR_USAGE` :625, the `ir` dispatcher :632, new leaf `ir_footprints`), `crates/turing-machine/src/completions/registry.rs` (new `CommandSpec` beside `ir_graph_spec` :614, blurbs :658/:675), `docs/tmt/cli.md` (the fenced `tmt ir` block — byte-identical to the new `IR_USAGE`; prose in Task 7)
- Test: `crates/turing-machine/tests/cli_programs.rs` (or a sibling — follow where `ir graph` is tested), `completions_registry.rs` auto-covers the new spec via `every_registry_flag_is_accepted_by_the_real_parser`; `cli_docs.rs` auto-guards the USAGE sync.

**Surface (ruled):** the spec sketches `tmt ir --footprints`; the `ir` group is subcommand-shaped (`graph` is a leaf, anything else prints usage), so the footprint report is a SIBLING LEAF, exactly as `--variant` landed on `ir graph` in the volatile round:

```
tmt ir footprints FILE.ir.json [--function NAME]
```

**Report format (pinned; stdout, exit 0):** per world in program order (filtered by `--function`), the IR is index-only by contract so the report prints indices:

```
world std::binaryNumbersBare::invertNumber
  tape 0 (num): writes {1, 2} of 3
```

Empty set: `writes {} of 3`. Errors mirror `ir_graph`'s: wrong arg count → `"ir footprints takes exactly one file\n\n{IR_USAGE}"`; unknown world → `"no world `NAME` in FILE"`.

- [ ] **Step 1: Write failing tests**: compile a fixture with `--emit-ir`, run `tmt ir footprints` on the JSON, assert the exact report lines (derive expected indices by hand from the fixture); `--function` filters; the error cases; bare `tmt ir` still prints the (updated) usage and exits 0.
- [ ] **Step 2: Update `IR_USAGE`** to list both leaves, and sync the fenced block in `docs/tmt/cli.md` byte-identically (the `cli_docs` guard forces this — run it to prove).
- [ ] **Step 3: Implement** the leaf (`IrProgram::from_json` → `footprint::infer_ir` → render). Registry: `ir_footprints_spec()` with `path: ["ir","footprints"]`, positional `File(ext(&["ir.json"]))`, the `--function` flag; add the blurb arm. Check `completions/zsh.rs`'s two-level group handling picks the sibling up (the `-C` re-basing comment at zsh.rs:176) — `completions_zsh.rs` must stay green.
- [ ] **Step 4: Green** — the new tests + `cli_docs` + `completions_registry` + `completions_zsh` + crate suite, fmt, clippy.
- [ ] **Step 5: Commit** `feat(cli): tmt ir footprints — the write-set report`.

---

### Task 6: the over-approximation property corpus

**Files:**
- Create: `crates/turing-machine/tests/footprint_property.rs` (local helpers per house style)

**The property:** for every corpus program: run it, record the set of symbols ACTUALLY WRITTEN per tape during the run, and assert the recorded set is CONTAINED in `infer_ir`'s set for the entry world — at BOTH `-O0` and `-O1` (the `-O1` leg is what proves `TailCall` edges are not lost; a dropped edge under-approximates and the containment fails).

- [ ] **Step 1: Survey the recording mechanism.** The VM is sans-I/O (core answers bus requests); pick the cheapest faithful recorder: either a test-local tape-device decorator in the `StrictTape` mold (wraps a device, forwards everything, records `(tape, symbol)` on every write request), or a `DebugSession` step loop inspecting write requests — whichever core's public surface supports with less code. Record the choice + the API used in the report.
- [ ] **Step 2: Write the corpus test**: fixtures = the seven `tests/golden/*.tmc` sources (each has known seeds in `opt_equivalence.rs`'s roster — reuse the same tape seeds via a local copy, per the no-shared-helpers convention) plus two locals: a mutual-recursion pair and a call-with-map chain. For each: build at `-O0` and `-O1`, run on the seeds with the recorder, assert per-tape containment (`inferred.is_superset(actual)`), and assert the inferred sets at the two levels are EACH sound (they need not be equal — the optimizer may change which calls exist; both must contain the actual writes).
- [ ] **Step 3: Non-vacuity**: one deliberate mutation quoted in the report — neutralize the `TailCall` arm in `infer_ir` (treat as terminator), re-run: the `-O1` leg of at least one call-chain fixture must FAIL containment (that is the "easy to lose" edge the spec names). Restore, re-run green.
- [ ] **Step 4: Green** — the new file + crate suite, fmt, clippy.
- [ ] **Step 5: Commit** `test(turing-machine): the footprint over-approximation property corpus`.

---

### Task 7: docs

**Files:**
- Modify: `docs/tmt/lint.md` (new `### dead-map-pair` under the `.tmc` rules, before the opt-in pair), `docs/lsp.md` (the hover section gains the write-set lines with the exact format), `docs/tmt/cli.md` (prose for `tmt ir footprints` around the already-synced block: what the report shows, the index-only note, `--function`), `docs/tmt/optimizer.md` (ONE sentence if absent: footprint inference and volatility are orthogonal — footprints answer WHICH symbols a body may write, volatility answers WHETHER writes may be elided/merged; check first, the volatile round may already carry it)

- [ ] **Step 1: Write the four pages' additions.** Every transcript quoted (the hover block, the CLI report) is RUN first and pasted from real output. `dead-map-pair`'s entry documents: write-half-only (the read direction is undecidable at compile time — it depends on the caller's writes and initial tape content), one-way pairs never reported, silent on unresolvable targets, the demote quickfix and WHY demotion is behavior-preserving, and the stdlib's `binaryNumbers::invertNumber` marker-collapse as the worked example.
- [ ] **Step 2: Sweep**: grep the touched pages for leftover claims; verify every code citation added in Tasks 1-6 (`docs/tmt/lint.md (dead-map-pair)`, `docs/lsp.md (hover)`, `docs/tmt/cli.md (tmt ir)`) resolves to a section carrying the content. Policy grep (no refs).
- [ ] **Step 3: Gates** — `cargo test -q -p mtc-turing-machine --test cli_docs` + the three per-package runs + clippy + fmt + no_std build, all foreground, exact exit codes.
- [ ] **Step 4: Commit** `docs(tmt): footprints — the lint, the hover lines, the ir report`.

---

## Final gates (whole branch, before merge)

`cargo test -q -p mtc-core` · `cargo test -q -p mtc-post-machine` · `cargo test -q -p mtc-turing-machine` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` · `cargo build -p mtc-core --no-default-features` — all foreground, all exit 0. The final whole-branch review additionally checks the cross-task seams: the two walks' rule-for-rule agreement (Task 2's cross-check test is the pin — confirm it discriminates), the lint's message/format against what Task 7's page quotes, `IR_USAGE` ↔ docs ↔ registry three-way agreement, the corpus checkpoint's zero-finding claim re-run, and that `git diff --stat -- crates/post-machine crates/core` is EMPTY (TM-only plan).
