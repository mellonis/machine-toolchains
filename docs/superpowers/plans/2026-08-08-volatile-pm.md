# Volatile PM + MO Multiversioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `volatile main()` in `.pmc`, two compiled columns (normal + gated) per function in every on-disk object, and a linker that picks the column by the program's volatile bit — spec `docs/superpowers/specs/2026-08-08-volatile-async-footprint-design.md` §4.3–4.6 (plan 3 of 5).

**Architecture:** The front end reserves `volatile` and accepts it only on top-level `main`. The compiler runs the optimizer twice over one lowered IR — the normal pipeline and a *gated* pipeline with every write-read-back-consuming pass disabled — and merges the two assembled columns into one `ObjectFile`, deduping byte-identical functions. MO v3 (unreleased, amended in place) gains a per-blob variant tag section and a program-volatile header flag bit. The linker's namespace becomes variant-aware: name-level first-wins as today, column choice by the program bit, missing column = counted fallback (never an error). PM IR does not change; MX/MT do not change; nothing reaches the assembler.

**Tech Stack:** Rust workspace (`mtc-core` formats/linker, `mtc-post-machine` front end/optimizer/CLI), proptest for format round-trips, no new dependencies.

**Branch:** `feat/volatile-pm` from master `ed9c6cc`.

## Global Constraints

- **`.pmx`/`-S` byte-identity for non-volatile programs** — the normal column IS today's pipeline; every existing golden and `-S` listing must stay byte-identical. `.pmo` files change shape deliberately (the round's one format change, riding unreleased MO v3).
- **`-O0` bit-identity**: no optimizer artifact may leak into `-O0` output; at `-O0` the two columns are identical by construction and dedup collapses every function to one blob.
- **`crates/core` stays arch-agnostic**: variant records and column selection are container/linker features expressed without PM-1 knowledge; core tests use the crate-private fake arch.
- **PMC_LANG_VERSION `"0.3"` → `"0.4"`** (`crates/post-machine/src/parser.rs:30`) — the round's ONE released-space move. TM version spaces, MX v2, MT v2, `.pma` 0.2 dialect: untouched.
- **MO v3 is amended in place** (`OBJECT_FORMAT_VERSION_V3` stays 3 — v3 is unreleased).
- **Text-expressibility gate (ruled 2026-08-09):** everything the compiler can put in an object is expressible in hand-written assembly — `pmt dis` output of any compiler-produced object assembles back byte-identically (Task 8's `dis_roundtrips_a_two_column_object` is this round's proof). Declared exception: `-g` debug side-tables (line maps remapped to source files are compiler provenance, unproducible by hand — the pre-existing `object != assemble(pma, true)` clause). Any future feature that puts new content in objects must extend the assembly dialect to express it or explicitly extend this exception list.
- **`.pma` dialect 0.3 is amended in place** with the `.volatile` directive (ruled 2026-08-09; v0.2.0 released 0.2, master's 0.3 is unreleased — `PM1_PMA_DIALECT_VERSION` stays `"0.3"`, its doc comment gains the clause). **`.tma` is untouched and must NOT recognize the directive** — `.func` is a core directive shared by both dialects, so the PM-only gating needs a negative test on the TM side.
- **Fixed modifier order**: `volatile main()` / `volatile export main()` — `volatile` first.
- **Drift guards must stay green after every task that touches their domain**: error-code registry + docs inventory set-compares, completions registry (`completions_registry.rs` incl. `EXPECTED_TOP_LEVEL` and flag probing), editor grammars (`editor_grammar.rs`), `cli_docs.rs` USAGE quotes.
- **Published docs are forge-agnostic** (no issue/PR numbers, no `spec §N`, no `docs/superpowers/` citations in code or docs); code comments cite durable pages as `docs/<page>.md (keyword)`.
- **Goldens are derivation-first** — never regenerate from run output.
- All cargo gates run in the FOREGROUND with direct exit codes — no pipes, no background cargo.
- Conventional commits with scope; no Claude attribution anywhere.

**Interfaces locked across tasks** (later tasks rely on these exact names):

```rust
// crates/core/src/formats/object.rs
pub enum BlobVariant { Normal, Volatile, Both }
pub struct ObjectFile { /* existing fields */, pub variants: Option<Vec<BlobVariant>>, pub program_volatile: bool }
pub const FLAG_HAS_VARIANTS: u8;      // next free header-flags bit
pub const FLAG_PROGRAM_VOLATILE: u8;  // the bit after that

// crates/post-machine/src/optimizer/mod.rs
pub fn gated_pass_names() -> &'static [&'static str]

// crates/post-machine/src/compiler.rs
pub enum VariantColumns { Both, NormalOnly, VolatileOnly }   // CompileOptions.columns, Default = Both
// CompileOutput gains: pub ir_volatile: Option<IrProgram>   // None when the volatile column was skipped
// AST Function gains: pub volatile: bool                    // only ever true on top-level main

// crates/core/src/linker/mod.rs
// LinkReport gains: pub variant_fallbacks: Vec<String>      // sorted names that linked the other column
```

---

### Task 1: `.pmc` grammar — reserve `volatile`, accept it on `main` only

**Files:**
- Modify: `crates/post-machine/src/parser.rs` (CST fn node, lowering, `PMC_LANG_VERSION`, error kinds)
- Modify: `crates/post-machine/src/compiler.rs` (the `code_registry!` block at :129, error rendering)
- Test: parser tests in `crates/post-machine/src/parser.rs` (`mod tests`), compiler error tests wherever `reserved-name`/`nested-export` tests live today (survey; keep new tests beside them)

**Interfaces:**
- Consumes: nothing (first task).
- Produces: AST `Function.volatile: bool` (parser.rs, near `exported` at :79-82); CST fn node records the `volatile` token losslessly (follow the CST's existing convention for `export`, which Task 2's formatter will print); error codes `volatile-not-on-main` (new kind `CompileErrorKind::VolatileNotOnMain`) and the existing `reserved-name` widened to fire on `volatile` used as a function/namespace name.

**Steps:**

- [ ] **Step 1: Survey.** Read `parser.rs`'s CST function node and the contextual-keyword handling around :1129-1144 (`export` + identifier) and the `namespace | use | export` hint sites (:762, :879). Note how `ReservedName` is produced today and which names it covers. Read one existing `reserved-name` test to copy its shape.
- [ ] **Step 2: Write the failing tests** (parser `mod tests`):
  - `volatile_main_parses_and_sets_the_flag` — source `volatile main() { mark; }`: parse succeeds, top-level `main` has `volatile == true`, `exported == true`.
  - `volatile_export_main_parses_with_fixed_order` — `volatile export main() { mark; }` parses; `export volatile main() { … }` FAILS (the parser sees `export` + identifier `volatile` + `main` — assert the error is `reserved-name` or `unexpected-token`, whichever the grammar naturally produces, and pin it).
  - `volatile_on_a_non_main_function_errors` — `volatile foo() { mark; }` → error code `volatile-not-on-main`, message naming the rule (style-match neighboring messages; include the offending name).
  - `volatile_on_a_nested_function_errors` — `main() { volatile inner() { mark; } }` → `volatile-not-on-main` (nested `main` is not top-level `main`; if the grammar rejects it earlier for another reason, pin that code instead and note it).
  - `volatile_as_a_definition_name_is_reserved` — `volatile() { mark; }` and `namespace volatile { }` → `reserved-name`.
- [ ] **Step 3: Run them to confirm they fail** (`cargo test -p mtc-post-machine volatile_` — foreground).
- [ ] **Step 4: Implement.** CST: accept an optional leading `volatile` token on a top-level definition, before `export`. Lowering: stamp `Function.volatile`; emit `VolatileNotOnMain` when the carrier is not top-level `main`. Reservation: extend the existing reserved-name check with `"volatile"`. Register `CompileErrorKind::VolatileNotOnMain => "volatile-not-on-main"` in the `code_registry!` block and add the row to the error-code docs inventory the drift guard set-compares (find it via the guard's failure message).
- [ ] **Step 5: Bump `PMC_LANG_VERSION` to `"0.4"`** at parser.rs:30 and extend its doc comment with one clause naming the change (reserving `volatile`; the modifier on `main`), matching the 0.3 clause's voice.
- [ ] **Step 6: Run** the new tests, the full parser + compiler test slice, and the error-code drift guard. All green, foreground.
- [ ] **Step 7: Commit** `feat(post-machine): reserve volatile; accept it on top-level main only`.

---

### Task 2: fmt, TextMate grammar, LSP head renders

**Files:**
- Modify: `crates/post-machine/src/fmt.rs` (print the modifier), `editors/grammars/pmc.tmLanguage.json` (keyword), `crates/post-machine/src/lsp/` (only the surfaces that render a function head — survey)
- Test: `crates/post-machine/tests/editor_grammar.rs` (drift guard), fmt tests beside the existing function-header cases, LSP tests beside the existing hover/completion-detail cases

**Interfaces:**
- Consumes: `Function.volatile` / the CST token from Task 1.
- Produces: nothing later tasks call; this closes the tooling sweep so no display gap survives the round (the volatile-TM plan shipped its hover fix late — do the equivalent audit UP FRONT here).

**Steps:**

- [ ] **Step 1: Survey every surface that renders a `.pmc` function head or its keywords**: `fmt.rs` (the definition printer), `pmc.tmLanguage.json` (which pattern carries `export`/`namespace`/`use` — `volatile` joins the same tier), LSP completions (keyword completions? function detail strings?), LSP hover (does it render a signature/head line for a function?), semantic tokens (keyword scope?). List findings in your report; every render site found gets either a change + test or an explicit "renders no head, nothing to do" line.
- [ ] **Step 2: Write failing tests**: fmt round-trips `volatile main() { … }` and `volatile export main() { … }` verbatim (idempotent, modifier printed in fixed order); `editor_grammar.rs` asserts `volatile` in the pmc grammar's keyword set (extend the guard's expected list); one LSP test per rendering surface found in Step 1 (e.g. hover on `main` contains `volatile`, if hover renders heads).
- [ ] **Step 3: Confirm failures** (foreground).
- [ ] **Step 4: Implement**: fmt prints `volatile ` before `export`/name from the CST token; grammar adds `volatile` to the keyword pattern in the same scope tier as `export` (`keyword.control.pmc` — the tier the pre-release round settled for JetBrains rendering); LSP surfaces from Step 1.
- [ ] **Step 5: Run** fmt tests, `editor_grammar.rs`, the LSP test slice, and `cargo test -p mtc-post-machine` (foreground, all green).
- [ ] **Step 6: Commit** `feat(post-machine): volatile in fmt, the pmc grammar, and LSP head renders`.

---

### Task 3: the gated pass set — probe, pin, expose

**Files:**
- Modify: `crates/post-machine/src/optimizer/mod.rs` (`gated_pass_names()`)
- Test: new `crates/post-machine/src/optimizer/` unit tests or a dedicated `tests/gated_passes.rs` (implementer's call; one file, local helpers per repo convention)

**Interfaces:**
- Consumes: the existing `disabled_passes` mechanism (`CompileOptions.disabled_passes`, `pass_names()` at optimizer/mod.rs:104).
- Produces: `pub fn gated_pass_names() -> &'static [&'static str]` — the passes the volatile column disables. Task 5 unions it with the user's `disabled_passes`.

**Steps:**

- [ ] **Step 1: Probe every pass for write-read-back / access-reorder consumption.** Certain members (spec ruling): `cell-state`, `fuse-tape-ops`. For each OTHER pass (`inline`, `check-fold`, `jump-threading`, `branch-fold`, `tail-call`, `tail-merge`, `dce`), read its source plus `optimizer/dataflow.rs` and answer: does it (a) predict a cell's value from a write (write-read-back), or (b) drop/merge/reorder tape-access instructions? The stated dividing line: *MF as a register latched by a performed access stays sound; predicting MF from a written value assumes write-read-back and does not.* Prime suspect — PROBED AND CORRECTED (Task 3, review-confirmed): the dataflow consumer is **`branch-fold`** (`branch_fold.rs` imports `dataflow::Fact`; its fold off `Wr → Coupled(Some(i))` IS the write-read-back assumption — GATED). `check-fold` is nine lines of `Check{k,k} → Goto{k}` with no dataflow import, and a check is a register test on latched MF (`jm`/`jnm` → `JumpRelIf`, no bus transaction) — CLEAN. Task 9 writes the page from Task 3's verdict table, never from this paragraph's original wording. Passes that only rewire control flow between unchanged accesses (`jump-threading`, `tail-call`, `tail-merge`, `dce` of unreachable code, `inline`) are expected clean — but each verdict needs a test, not an argument.
- [ ] **Step 2: Write one discriminating test per non-obvious verdict.** Shape: a small IR (or `.pmc` source compiled at `-O1`) where the pass would change the *bus transaction sequence* if it ran. For a **gated** verdict: compile with only that pass enabled minus/plus the gate and assert the volatile column preserves the access sequence (count `wr`/`mov`/check ops in the emitted `.pma` or walk the final CFG). For a **clean** verdict: assert the pass's transform still fires in the volatile column (so the gate is not over-broad). Minimum set: the cell-state discriminator is the strict-cell program — a function writing the same symbol twice (`mark; mark`) faults on a `StrictTape` only if both writes survive; normal `-O1` loses the second write, the volatile column must keep it. The fuse-tape-ops discriminator: `mark; right;` fuses to `wrr` normally; the volatile column keeps two transactions.
- [ ] **Step 3: Confirm the discriminators fail** against a stub `gated_pass_names()` returning `&[]` (foreground).
- [ ] **Step 4: Implement** `gated_pass_names()` returning the pinned set, with a doc comment carrying the dividing-line rule in prose (cite `docs/pmt/optimizer.md (volatile builds)` — the page section lands in Task 9; carry the substance now). Add a unit test asserting `gated_pass_names() ⊆ pass_names()` (drift guard against pass renames).
- [ ] **Step 5: Run** the new tests + `cargo test -p mtc-post-machine --test opt_equivalence` (foreground, green).
- [ ] **Step 6: Commit** `feat(post-machine): the gated pass set for volatile builds`. Report MUST list the per-pass verdict table (pass → gated/clean → discriminating test name) — Task 9's optimizer.md section is written from it.

---

### Task 4: MO v3 variant records (core)

**Files:**
- Modify: `crates/core/src/formats/object.rs` (struct fields, flags, encode/decode), `docs/formats.md` deferred to Task 9
- Test: the format's existing unit + proptest round-trip suites (same file/module as today's object round-trips)

**Interfaces:**
- Consumes: the existing v3 header flags byte (object.rs:325-340; `FLAG_HAS_DEBUG`/`FLAG_HAS_SIGNATURES`/`FLAG_HAS_TABLES` occupy the low bits).
- Produces: `BlobVariant`, `ObjectFile.variants: Option<Vec<BlobVariant>>` (parallel to `blobs`), `ObjectFile.program_volatile: bool`, `FLAG_HAS_VARIANTS`, `FLAG_PROGRAM_VOLATILE` — exactly as pinned in Global Constraints. Decode of any object WITHOUT the flag yields `variants: None, program_volatile: false` (legacy reads as normal-only — a typed rule, not a string match).

**Steps:**

- [ ] **Step 1: Write failing round-trip tests**: (a) an object with `variants: Some(vec![Normal, Volatile, Both])` + `program_volatile: true` round-trips exactly; (b) a legacy encode (both new fields absent) decodes to `None`/`false` and — critical — **an object encoded with `variants: None, program_volatile: false` is byte-identical to the pre-change encoding** (assert against a byte string captured from the CURRENT encoder before you change it; this is the no-shape-drift-for-tag-free-objects pin); (c) proptest: arbitrary `variants` vectors (length == blobs length) survive the round trip; (d) malformed: a variants section whose length ≠ blob count → `FormatError::Malformed`; a tag byte outside `0..=2` → `Malformed`; pre-v3 version claiming `FLAG_HAS_VARIANTS` → `Malformed` (mirror the existing "pre-v3 objects must not claim v3 flags" check at :573-576).
- [ ] **Step 2: Confirm failures** (foreground).
- [ ] **Step 3: Implement.** Encoding: `FLAG_HAS_VARIANTS` gates a section of one u8 tag per blob (`0` Normal, `1` Volatile, `2` Both), placed with the other v3 sections in flag order; `FLAG_PROGRAM_VOLATILE` is a pure header bit (no section). Writers emit the section only when `variants` is `Some` — the PM compiler will always set it (Task 5); assembled/TM objects keep `None` and stay byte-identical. Decode is the mirror; CRC covers everything as today.
- [ ] **Step 4: Sweep every `ObjectFile { … }` construction site in the workspace** (core linker tests, TM crate, PM crate) — add the two fields (`variants: None, program_volatile: false`); the compiler learns real values in Task 5. `cargo build --workspace` green.
- [ ] **Step 5: Run** the format suites + `cargo test --workspace` (foreground).
- [ ] **Step 6: Commit** `feat(core): MO v3 per-blob variant tags and the program-volatile bit`.

---

### Task 5: two-column compilation + dedup (post-machine)

**Files:**
- Modify: `crates/post-machine/src/compiler.rs` (`CompileOptions.columns`, the compile pipeline after `ir::lower`, `CompileOutput.ir_volatile`, object merge)
- Test: compiler unit tests + a new `crates/post-machine/tests/variant_columns.rs`

**Interfaces:**
- Consumes: `gated_pass_names()` (Task 3), `ObjectFile.variants`/`program_volatile` (Task 4), `Function.volatile` (Task 1).
- Produces: `VariantColumns` on `CompileOptions` (default `Both`); `CompileOutput.object` is ONE merged object; `CompileOutput.pma` and `CompileOutput.ir` are the NORMAL column exactly as today; `CompileOutput.ir_volatile: Option<IrProgram>` is the volatile column's final CFG (None when skipped). Tasks 6-8 rely on all of these.

**Steps:**

- [ ] **Step 1: Write failing tests**:
  - `normal_column_output_is_byte_identical_to_today` — compile a fixture at `-O1` with `columns: Both`; `pma` and the normal column's blobs equal a `NormalOnly` compile of the same source (and existing `-S` goldens stay green — the suite-level proof).
  - `two_columns_differ_on_a_tape_touching_function_at_O1` — a function whose accesses fuse (`mark; right;`): merged object has TWO blobs for it tagged `Normal`/`Volatile`, two same-name symbols; the volatile blob is longer (unfused).
  - `identical_columns_dedup_to_both` — a tape-free function (pure control flow) at `-O1`: ONE blob tagged `Both`, one symbol.
  - `at_O0_every_function_dedups_to_both` — whole fixture at `-O0`: `variants` is all `Both`, blob count equals today's.
  - `volatile_main_sets_the_program_bit` — `volatile main() { … }` → `object.program_volatile == true`; plain `main` → `false`.
  - `single_column_compiles_skip_the_other_pipeline` — `NormalOnly`: `variants` all `Normal`, `ir_volatile` is `None`; `VolatileOnly`: all `Volatile`, and `pma` — pin the ruling: `pma` always renders the column that was built (`VolatileOnly` renders the volatile listing; `Both`/`NormalOnly` render normal — the `-S` byte-identity gate only ever binds the normal listing).
- [ ] **Step 2: Confirm failures** (foreground).
- [ ] **Step 3: Implement.** After `ir::lower`: clone the `IrProgram`; optimize copy A with the user's options (normal), copy B with `disabled_passes ∪ gated_pass_names()` (volatile); codegen + assemble each per `columns`. Merge: iterate functions in today's emission order — per function, compare the full per-blob record (code bytes, its relocations rebased blob-relative, its `BlobDebug`); identical → one blob tagged `Both`, one symbol; different → normal blob then volatile blob adjacent, two symbols with the SAME name and the same `Defined`/`Local` visibility, relocation/debug/blob indices renumbered coherently. `ir_snapshots` stay normal-column-only (the `--emit-ir` contract is unchanged; Task 7 adds variant selection for `pmt ir` from `ir`/`ir_volatile`). Amend `CompileOutput.pma`'s doc comment — it currently claims "the object is assembled from exactly this text", which a two-column object no longer satisfies; state the rendered-column rule (normal unless `VolatileOnly`) and that the object assembles from the per-column listings.
- [ ] **Step 4: Run** the new tests, `golden_programs`, `opt_equivalence`, `cli_programs` (foreground — golden/CLI suites prove the byte-identity gate; any golden diff is a defect in your merge, never a regen).
- [ ] **Step 5: Commit** `feat(post-machine): two-column compilation with digest dedup and the program bit`.

---

### Task 6: variant-aware linking + the fallback counter (core)

**Files:**
- Modify: `crates/core/src/linker/resolve.rs` (namespace + selection), `crates/core/src/linker/mod.rs` (`LinkReport.variant_fallbacks`)
- Test: the linker's existing unit tests (fake arch `0x7E`/`0x7F` object builders)

**Interfaces:**
- Consumes: `ObjectFile.variants`/`program_volatile` (Task 4).
- Produces: `LinkReport.variant_fallbacks: Vec<String>` (sorted names that linked the non-matching column). Selection rules pinned below; Task 7 renders the counter under `-v`, Task 8 exercises the legacy path end-to-end.

**Selection rules (the design, pinned):**
1. The program bit = `program_volatile` of the object defining the entry symbol (default `main`). No definer → `false`.
2. Namespace stays **name-level first-wins** exactly as today (user dup = error, libraries first-wins, shadowing silent). The winning definition for a name is the whole variant *pair* from one object — columns are never mixed across objects for one name.
3. Within the winner: prefer the column matching the program bit; `Both` matches both; missing column → take the other and record the name in `variant_fallbacks`. A tag-free legacy object is all-`Normal` (Task 4's decode rule), so a volatile program linking it counts fallbacks — visible, never an error.
4. Two same-name symbols in ONE object are legal **only** as a `{Normal, Volatile}` pair (tags must differ and not be `Both`); any other same-name pair keeps today's duplicate error.
5. BFS reachability, relaxation, emission, map sidecar: unchanged — they see one chosen blob per name, names stay clean.

**Steps:**

- [ ] **Step 1: Write failing tests** (extend the local `obj(…)` builders with variant tags): normal program + two-column lib links the normal column (assert by blob bytes in the image); volatile program (bit set on main's object) links the volatile column; volatile program + legacy (tag-free) lib links normal and reports `variant_fallbacks == ["lib_fn"]`; `Both`-tagged function links into either with zero fallbacks; same-name pair with equal tags in one object → duplicate error (unchanged code path); cross-object user-side same-name → duplicate error as today.
- [ ] **Step 2: Confirm failures** (foreground).
- [ ] **Step 3: Implement** per the pinned rules. Keep the change inside `resolve`'s namespace construction + definition lookup; `layout` stays untouched.
- [ ] **Step 4: Run** the linker suite + `cargo test -p mtc-core` + both arch crates' link-dependent suites (foreground).
- [ ] **Step 5: Commit** `feat(core): variant-aware symbol resolution with the counted fallback`.

---

### Task 7: CLI — `pmt ir --variant`, disk vs in-memory columns, stdlib

**Files:**
- Modify: `crates/post-machine/src/cli/inspect.rs` (`ir`), `cli/build.rs` (compile → disk = both columns), `cli/driver.rs` (in-memory needed-column + the pre-scan), `cli/run.rs` or wherever `-v` link reporting renders (the fallback counter line), `crates/post-machine/src/completions/registry.rs`, `crates/post-machine/src/stdlib/mod.rs`
- Test: `crates/post-machine/tests/completions_registry.rs`, `cli_programs.rs`, `build_driver.rs`-equivalent (survey the PM test names), stdlib test beside `stdlib` unit tests

**Interfaces:**
- Consumes: `VariantColumns` + `ir_volatile` (Task 5), `variant_fallbacks` (Task 6).
- Produces: the user-facing surface. Task 9 documents exactly what ships here.

**Steps:**

- [ ] **Step 1: Write failing tests**:
  - `pmt ir --variant volatile` on a fixture prints the volatile column's CFG (differs from default output on a fusing function); `--variant normal` ≡ no flag; `--variant bogus` → the CLI's standard unknown-value error.
  - Completions: registry entry for `--variant` with value choices `normal|volatile` (space-or-equals shape); `completions_registry.rs` probes it against the real parser — extend expectations, not the guard logic.
  - `pmt compile -o` emits a both-columns object (read it back: a fusing function has the `{Normal, Volatile}` pair); `pmt build --keep-objects` same.
  - In-memory `pmt build` of a **volatile** program: resulting `.pmx` byte-identical to the on-disk path (`compile -o` each unit, then `link`); repeat for a **normal** program (both directions of the needed-column rule). This is the spec's gate.
  - `-v` link output renders the fallback count (and names) when a volatile program links a legacy object; silent at zero (match `LinkReport` rendering style — thin-renderer rule: the count comes from the report, the CLI only formats).
  - `pmt dis` on a variant-tagged object is Task 10's surface (the `.volatile` directive) — this task leaves dis untouched.
  - Stdlib: a `volatile main()` calling a tape-touching `std::` routine links with `variant_fallbacks` empty (stdlib carries real volatile columns).
- [ ] **Step 2: Confirm failures** (foreground).
- [ ] **Step 3: Implement.** `ir`: select `ir` vs `ir_volatile` (compile with `columns: Both` for inspection — inspection is not the in-memory-build path). Disk rule: `compile -o`/`--keep-objects` force `columns: Both`. In-memory rule: the driver pre-scans its `.pmc` sources with a PARSE-ONLY pass to find top-level `main`'s volatile flag (disk `.pmo` inputs contribute their header bit instead); compiles every in-memory unit with the single needed column; no main found → normal. Stdlib: `stdlib::object()` builds with `columns: Both` (one `OnceLock` object serves both program kinds in-process; dedup keeps it small — note this in the OnceLock site comment, citing `docs/pmt/stdlib.md` only if that page gains the fact in Task 9, else plain prose).
- [ ] **Step 4: Run** the touched suites + `cargo test -p mtc-post-machine` (foreground).
- [ ] **Step 5: Commit** `feat(cli): ir --variant, the disk/in-memory column rule, stdlib columns`.

---

### Task 8: equivalence + byte-identity gate corpus

**Files:**
- Create: `crates/post-machine/tests/volatile_equivalence.rs` (local helpers per repo convention)

**Interfaces:**
- Consumes: everything shipped in Tasks 1-7. Produces: the round's standing proof, referenced by the cut audit.

**Steps:**

- [ ] **Step 1: Write the corpus test file** (these run against the REAL pipeline — no stubs):
  - `normal_and_volatile_columns_agree_on_observables` — for each of ≥4 programs (reuse/adapt the `opt_equivalence` corpus shapes: straight-line writes, branching on checks, a subroutine call chain, a loop): run the normal-column `.pmx` and the volatile-column `.pmx` (link the same source once with the bit off, once with `volatile main`) on the same tapes; assert same final tape and termination kind; assert the volatile run's tact count ≥ the normal run's on at least one fixture (the gated column pays for its transactions — a weak inequality pin that catches column-swap bugs).
  - `volatile_keeps_the_strict_cell_fault` — the Task 3 strict-cell program end-to-end through `pmt`-level plumbing: normal `-O1` run on a `StrictTape` completes (the double write was folded); volatile `-O1` run faults. (The honest `docs/pmt/isa.md` link, proven.)
  - `in_memory_and_on_disk_paths_agree` — already covered per-direction in Task 7; here add the mixed case: one unit as a disk `.pmo` (both columns) + one in-memory source, volatile program — `.pmx` equals the all-disk build.
  - `legacy_object_end_to_end` — assemble a hand-written DIRECTIVE-FREE `.pma` routine (`pmt asm` path → tag-free normal-only object), link into a volatile program: runs correctly, report counts one fallback.
  - `handwritten_volatile_column_links_without_fallback` — a hand-written `.pma` with a same-name pair (bare + `.volatile` bodies, Task 10's directive): a volatile program links the `.volatile` body (assert by image bytes), `variant_fallbacks` empty.
  - `dis_roundtrips_a_two_column_object` — `pmt dis` a Task 5 merged object (with the program bit set), assemble the printed text: the re-assembled object is byte-identical, tags and program bit included (Task 10's round-trip contract, proven end-to-end here).
  - `O0_matrix` — the first corpus fixture at `-O0` both columns: byte-identical `.pmx` to each other and to today's `-O0` build (the `-O0` bit-identity floor extended to the volatile world).
- [ ] **Step 2: Run the file; every test must pass against Tasks 1-7's implementation** — any failure here is a real integration defect, fix it in the owning module (with the owning task's test updated if its pin was wrong) before this task completes.
- [ ] **Step 3: Run the full gate set**: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build -p mtc-core --no-default-features` (foreground).
- [ ] **Step 4: Commit** `test(post-machine): the volatile equivalence and byte-identity corpus`.

---

### Task 9: docs sweep

**Files:**
- Modify: `docs/pmt/language.md` (volatile main, reservation, 0.4), `docs/pmt/optimizer.md` (volatile builds section + the worked example), `docs/pmt/isa.md` (strict cells), `docs/formats.md` (MO variant records + program bit + legacy rule), `docs/pmt/cli.md` (`ir --variant`, the disk/in-memory column rule, the `-v` fallback line), `docs/core.md` (linking: variant selection + fallback), `docs/pmt/stdlib.md` (both-columns note)
- Test: none new — every claim is verified against the tools before it is written

**Steps:**

- [ ] **Step 1: language.md** — a `### Volatile programs` section: the modifier, main-only rule with the `volatile-not-on-main` error, fixed order, reservation, the two-variant model in one paragraph (the author never picks a variant; the toolchain builds both, the linker chooses), version note 0.3 → 0.4 wherever the page states the language version.
- [ ] **Step 2: optimizer.md** — a `## Volatile builds` section from Task 3's verdict table: the gated set, the dividing-line rule (latched MF sound / predicted MF not), and the worked example — BUILD IT: write the spec's ~11-op pulse routine, compile both columns at `-O1`, quote the real instruction counts and `pmt run` tact totals from the tools (the spec predicts 4 fused ops vs the full sequence, 27 vs 54 tacts — if the real numbers differ, the REAL numbers go in the page).
- [ ] **Step 3: isa.md** — the strict-cells paragraph gains the simpler alternative: a volatile program cannot lose a strict-cell fault to `cell-state` by construction (keep the `--fno-cell-state` advice for programs that only want one pass off).
- [ ] **Step 4: formats.md** — TWO sections. The `.pmo` section documents the variant tag section (per-blob u8, the three tags, parallel to blobs), `FLAG_PROGRAM_VOLATILE`, and the typed legacy rule (no flag → normal-only); state the compatibility fact plainly: objects with variant records are MO v3, readable by this toolchain only (the cut's CHANGELOG carries the release-facing note). The `## .pma — assembly text` section (:345) documents the `.volatile` directive per Task 10's pinned semantics — presence-form, the two positions, the variant-aware duplicate rule, asm-side Both dedup, the dis byte-round-trip claim (verify it live), and the selection-metadata framing: a hand-written body is never optimized on either arch; the directive picks the link column, it does not protect the body.
- [ ] **Step 5: cli.md + core.md + stdlib.md** — `ir --variant` (choices, default), the disk-vs-in-memory column rule in `build`'s section (with the byte-identity guarantee stated), the `-v` fallback line with a real quoted transcript; core.md's linking section gains variant selection + name-level first-wins-then-choose + counted fallback; stdlib.md notes both columns ship embedded.
- [ ] **Step 6: Verify every quoted transcript by running it**; grep the touched pages for leftover pre-round claims (`0.3` language version, "one build per function" phrasings). Grep the whole `docs/` tree for `volatile` to catch pages that now under-claim (formats.md's dis paragraph, lint pages listing error codes).
- [ ] **Step 7: Run** `cargo test -p mtc-post-machine --test cli_docs` (USAGE quote guard) + `cargo test --workspace` (foreground).
- [ ] **Step 8: Commit** `docs(pmt): volatile programs — language, optimizer, formats, cli, linking`.

---

### Task 10: the `.pma` `.volatile` directive (dis round-trip)

Sequencing: execute AFTER Task 5 (needs `ObjectFile.variants` and merged two-column objects) and BEFORE Task 8 (whose corpus proves the round trip end-to-end). Ruled 2026-08-09 — the spec's §4.5 "Text form" paragraph is the authority.

**Files:**
- Modify: `crates/core/src/asm/` (directive recognition mechanism — survey; `.func` lives in core's CST as `FUNC_WORD`, cst.rs:35), `crates/post-machine/src/asm/mod.rs` (the PM dialect: directive semantics, variant-aware `duplicate-function`, asm-side dedup, `PM1_PMA_DIALECT_VERSION` doc clause), the PM disassembler (emit `.volatile` per tag; Both printed twice; program-bit position), `editors/grammars/pma.tmLanguage.json` (directive pattern)
- Test: PM asm/dis unit tests, `crates/post-machine/tests/editor_grammar.rs` (pma leg), a TM-side negative test (in the TM crate's `.tma` dialect tests), the directive drift guard (`recognized_directives` inventory)

**Interfaces:**
- Consumes: `BlobVariant`/`variants`/`program_volatile` (Task 4), merged objects (Task 5).
- Produces: the assemblable dis surface Task 8's `dis_roundtrips_a_two_column_object` and `handwritten_volatile_column_links_without_fallback` prove.

**Semantics (pinned by the ruling):**
1. Inside a `.func` block, a `.volatile` line tags that blob `Volatile`; absence = `Normal`.
2. Before the first `.func`, `.volatile` sets `program_volatile`.
3. `duplicate-function` becomes variant-aware: a same-name `.func` pair is legal iff exactly one member carries `.volatile`; two bare or two `.volatile` same-name blocks keep today's error.
4. Asm-side dedup mirrors the compiler's: a legal same-name pair whose assembled records come out byte-identical collapses to ONE blob tagged `Both`. (dis prints a `Both` function twice — bare + `.volatile` — so dis→asm restores the exact object.)
5. A directive-free file assembles exactly as today — byte-identical object, `variants: None` (legacy). This is the PM byte-identity gate for this task.
6. PM-dialect-only: `.tma` must NOT recognize `.volatile` (negative test — a `.tma` file with the directive fails with the dialect's standard unknown-directive error). Mechanism: implementer's choice between an arch-syntax-contributed directive table or a caps field only `pm1_syntax()` enables — whichever keeps `crates/core` arch-agnostic and the existing caps semantics intact; state the choice and its rationale in the report. Update the `recognized_directives` inventory + its bidirectional drift guard for whichever mechanism.

**Steps:**

- [ ] **Step 1: Survey** core's directive path (`FUNC_WORD`, `shape_line`, lower.rs's malformed-directive reporting, `recognized_directives(caps)` + drift guard) and the PM disassembler's `.func` emission. Pick the gating mechanism per pin 6.
- [ ] **Step 2: Write failing tests**: directive tags a blob (assemble → `variants` reflects it); file-level position sets the program bit; mid-file `.volatile` outside any `.func`... pin: after the first `.func` but outside a block — standard directive-position error, test it; variant-aware duplicate rules (legal pair; two-bare error; two-volatile error); asm dedup of an identical pair → one `Both` blob; directive-free byte-identity (capture today's object bytes first); dis emission of a merged object (directive per tag, Both twice, program bit line first); the TM negative; grammar drift guard.
- [ ] **Step 3: Confirm failures** (foreground).
- [ ] **Step 4: Implement** per the pins; extend the pma TextMate grammar (same tier as other directives) and the drift guard.
- [ ] **Step 5: Run** PM asm/dis suites, the TM dialect suite (negative), `editor_grammar.rs`, the directive drift guard, `cargo test --workspace` (foreground).
- [ ] **Step 6: Commit** `feat(post-machine): the .pma .volatile directive — assemblable variant columns`.

---

## Final gates (whole branch, before merge)

`cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` · `cargo build -p mtc-core --no-default-features` — all foreground, all exit 0. The final whole-branch review additionally checks the cross-task seams: normal-column byte-identity claims (Tasks 5/8) against the goldens, the Task 3 verdict table against the optimizer.md section (Task 9), the two flags bits against formats.md, and that no display surface found in Task 2's Step-1 survey was left unhandled.
