# Language Tightening (Phase 4, Language Spec A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove every place where the toolchain silently picks one of two
things, silently accepts something it documents as an error, or reports a lint
the author cannot act on — one definition per fully qualified name at link and
at compile, four stricter checks, three lints plus one lint exemption, and a
graph digest that covers the set and map content a body names. No new syntax.

**Architecture:** The link half is arch-neutral and lives in `crates/core`
(`linker/resolve.rs` keeps every candidate definition until the reachability
walk decides whether the name is contested; `linker/interface.rs` replaces its
two first-wins exporter maps by one conflict-aware pass), fed input names by
both toolchains' drivers through a new `LinkOptions::labels` field. The compile
half lives in `crates/turing-machine`: one new module, `published.rs`, defines
the *published form* of a declaration and is the only comparison either
conflict check uses; one settle function in `header.rs` decides what a
declarations reading publishes and is shared by the lenient `.tmc` read and the
unit's own-definition check. The remaining checks and lints are local edits to
`compiler.rs`, `ir.rs`, `header.rs`, `asm/` and `lint/rules/`, each behind a
drift-guarded registry row.

**Tech Stack:** Rust (toolchain pinned in `rust-toolchain.toml`), `cargo
nextest`, the in-process CLI entries `mtc_turing_machine::cli::execute` /
`mtc_post_machine::cli::execute` for CLI tests, no new dependencies.

**Spec:** `docs/internal/specs/2026-09-30-language-tightening-design.md`
(the "Amendments from the code survey" section has the same weight as the body
and wins where they differ). Decision log:
`.superpowers/sdd/2026-09-30-phase-4/spec-a-notes.md`. Code anchors taken for
this plan: `.superpowers/sdd/2026-09-30-phase-4/anchors-{linker,declarations,checks,lints}.md`.

**This plan:** `docs/internal/plans/2026-09-30-language-tightening.md`,
approved by the maintainer 2026-10-11 together with the spec.

**Branch:** `phase-4-language-a`, cut from an UPDATED `origin/next`
(`git fetch origin next`, then branch from `origin/next`), in the
`binding-arc-4` worktree on a clean tree — never on top of another branch's
uncommitted work. Ledger and probes live
in `.superpowers/sdd/2026-09-30-language-tightening/` (git-ignored); probe files
under its `scratch/` subdirectory.

**Plan-internal notation.** This plan cites tracker numbers (#138, #127, #125),
`Task N` and spec sections freely because it is an internal artifact. **None of
that notation may appear in code, code comments, docs, test names or commit
messages** — see Global Constraints.

---

## Global Constraints

Every task's requirements implicitly include this section. Re-read it before
each task.

- **Test first, with the mutation seen red.** Every behaviour lands with a test
  written BEFORE the code, run and seen failing for the stated reason; every
  test states in its doc comment the one mutation that reddens it, and the
  implementer applies that mutation once after the code is green and sees the
  test fail. For each new error or lint code there is a firing fixture AND a
  near-miss fixture.
- **Documentation authority.** Code comments and published pages (`README.md`,
  `docs/**`, `CHANGELOG.md`) cite durable pages only, as `docs/<page>.md
  (keyword)`. No issue/PR numbers, no hosting URLs, no `Task N`, no `spec §N`,
  no ruling numbers — in code comments, docs, test names, or commit messages.
  Where the durable page does not exist yet, carry the substance in prose.
- **`crates/core` stays arch-neutral.** Core learns nothing about PM-1 or TM-1.
  Every core behaviour in this plan is tested against the fake dialect in
  `crates/core/tests/link_checks.rs` (`fake_syntax()`, arch `0x7E`) or the
  unit-test stubs in `crates/core/src/linker/resolve.rs` (`obj`,
  `variant_obj`) — never through a real architecture.
- **PM-1 byte identity.** No byte of `pmt` output moves except where a link is
  now refused. Every task touching `crates/core` runs
  `cargo nextest run -p mtc-post-machine --test golden_programs --test asm_volatile`
  in its gate.
- **Drift guards are set-compares in BOTH directions.** A new error code, lint
  rule or link-warning change lands with its docs row in the same task:
  `crates/turing-machine/tests/error_code_docs.rs`,
  `crates/core/tests/error_code_docs.rs`,
  `crates/turing-machine/src/lint/docs_drift.rs`, the witness list in
  `compiler.rs::tests::error_codes_are_pairwise_distinct_and_complete`, the
  `code_registry!` rows in `compiler.rs` and `asm/mod.rs`.
- **No CHANGELOG entry** — the CHANGELOG for the whole scope is assembled at
  the release cut. Each task that changes behaviour adds its line to the
  ledger's `changelog-notes.md` instead (breaking notes flagged).
- **No version constant moves**: `TMC_LANG_VERSION` stays 0.2, the TM-1 `.tma`
  dialect 0.5, `PMC_LANG_VERSION` 0.4, the object container 4, `TM_IR_VERSION`
  4, crates `0.6.0-dev`, the manifest schemas as they are.
- **Commits.** Conventional with scope. Implementer subagents commit their own
  task on `phase-4-language-a` once the user has granted it (the controller
  confirms before Task 1); merging and pushing stay the owner's. Messages are
  written to a file and committed with `git commit -S -F <file>` (signed), **no
  attribution or `Claude-Session:` lines** — after each commit run
  `git log -1 --format=%B` and, if a trailer was injected, `git commit --amend
  -S -F <file>` with the message restored. `git add` names explicit paths,
  never `-A` or `.`.
- **Cargo discipline.** One cargo invocation at a time, in the foreground,
  prefixed
  `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target`,
  run from the worktree root. Tests run with `cargo nextest run` (never `cargo
  test` alone), `-j 4`. Per task: focused test binaries while iterating, then
  the task's full crate(s) ONCE; the whole workspace ONCE at the end (Task 14).
  Mutations are applied against the one focused test binary, not a new full
  run. Before a long run after a reboot, check `ps` for `syspolicyd`.
- **Temp directories in tests** go through the per-file `ScratchDir` guard
  (fresh on create, removed on drop), named by PID plus a per-call atomic
  counter — copy `fn scratch` and `struct ScratchDir` verbatim from
  `crates/turing-machine/tests/cross_unit.rs` (locate by symbol name) into any
  new test file that writes to disk.
- **Probes** (hand runs of `tmt`/`pmt`, throwaway fixtures) live under
  `.superpowers/sdd/2026-09-30-language-tightening/scratch/`, never inside a
  tracked directory.
- **Anchors are bearings.** Every `file:line` and "near `fn name`" here was
  read on 2026-09-30; locate by symbol name. `crates/turing-machine/src/ir.rs`,
  `src/cli/` and `crates/wasm` were being edited by another branch on that day
  — anchors there are the most likely to have moved.
- **Fixtures are tool-verified.** Every source fixture below marked
  **[verified]** was run through the built `tmt`/`pmt` on 2026-09-30 and the
  TODAY column is the observed output. A column marked **[computed]** is a
  hand-counted `line:col` calibrated against a TODAY output of the same shape;
  if the implementation reports a different column for the right reason, that
  is a plan erratum to report in the task report — never a code change to make
  the number fit.

---

## Established facts this plan builds on

Each was verified against the tree on 2026-09-30 (anchors files hold the
excerpts). An implementer may rely on them without re-deriving.

1. **`resolve` drops losing definitions at insertion.** `linker/resolve.rs`
   near `pub(crate) fn resolve` builds `namespace: HashMap<&str, ColumnPair>`;
   user-vs-user repeats error at once (`DuplicateSymbol(name)`), library
   entries go through `namespace.entry(name).or_insert(pair)` and a loser is
   forgotten. `resolve_names` (`linker/mod.rs`) calls the same `resolve`.
   `object_columns(object, oi, strict)` keeps the within-one-library
   first-record-wins behaviour (`strict == false`), which stays.
2. **Link errors carry no input names today.** `LinkError::DuplicateSymbol(String)`
   is built only in `resolve.rs`; `check_graft_drift`/`check_imported_alphabets`
   (`linker/interface.rs`) name inputs through `LinkOptions::sources`, which
   `pmt link`/`tmt link` leave empty and the build drivers fill for units only.
   `sources` also feeds `MapFunction::source` (the map sidecar, DAP
   openability) and must NOT gain library entries.
3. **Four `LinkOptions` literals have no `..` spread** and break when a field
   is added: the `Default` impl (`linker/mod.rs`), TM `cli/build.rs` near
   `pub(super) fn link`, TM `cli/driver.rs` near `fn link_and_write` and near
   `fn link_and_write_argv`. Every other literal uses `..Default::default()`.
4. **TM header-only libraries contribute no object.** `cli/driver.rs` near
   `fn link_library_objects` flattens `Vec<Option<ObjectFile>>`; library names
   are lost there.
5. **The exporter maps are first-wins, object-level, ungated by reachability**
   and run in `link` right after `resolve`; `resolve_names` does not run them.
   `ExportedGraph`, `GraftProvenance`, `ExportedAlphabet`, `ImportedAlphabet`
   derive `Eq`.
6. **The declarations table has no duplicate detection.** `declarations.rs`
   holds `Vec<(Origin, Resolved)>`; each per-kind lookup is its own first-match
   loop. Local definitions always win silently (`Scopes::resolve` returns
   `kind: Some` for a local name; externals are consulted only on `kind: None`).
7. **The fixpoint reads each declaration source against its already-read peers
   plus the stdlib** (`header.rs` near `pub(crate) fn resolve_declarations`).
   So a check inside `resolve_program` that compares a unit's OWN definitions
   against its table fires while the SECOND of two conflicting modules is
   being READ, and renders against that module's file. A compile-wide
   table-versus-table conflict is reachable only through the in-crate API.
8. **A lenient `.tmc` read publishes less than its header today.**
   `header.rs` near `fn read_declarations_with_mode` leaves an uncontracted
   tape's `ResolvedTape::writes` at `None` (the whole alphabet) while the
   printed header carries the inferred set (probe in Task 4).
9. **A graph's digest is `crc32(graph_body_lines(..).join("\n"))`**
   (`header.rs` near `pub(crate) fn graph_digest`), names as written; the
   stdlib has no `set` and no named map, so appending nothing for a body that
   names none keeps every stdlib digest and `std.tmh` byte-identical.
10. **A nested graph from the SAME library module is not recorded.**
    `expand::expand_grafts_into` records a spliced graph into
    `spliced_external_graphs` only when its target misses the CURRENT
    owner's graph map. A graft nested from ANOTHER module is therefore
    recorded already (`library_grafts.rs::a_declared_graphs_nested_target_resolves_when_available`
    asserts two records), but `relay` → `walker` inside one library records
    `relay` only (probe: `.grafted relay, 36002832` and nothing else). On
    the exporting side, `header::exported_graph_digests` writes `.graph`
    lines for EXPORTED graphs only, although the header also prints every
    non-exported graph an exported body reaches. Task 13 closes both.
11. **`.tmc` has no repeat variable**; the only fold-readable name is a
    pattern binding (`as NAME`) read by `{…}` in the same rule's write vector.
12. **The corpus is not lint-silent** (`binding-product-threshold`,
    `unused-graft-name` fire on shipped files); silence is pinned per rule by
    sweep files (`tests/unused_set_sweep.rs` is the model).

---

## File structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/core/src/linker/mod.rs` | `LinkError` (`DuplicateSymbol` struct form, `ConflictingExport`, `UndeclaredExit`), `LinkOptions::labels`, `input_name`, `link`, `resolve_names`, `DIAGNOSTIC_CODES` | 1, 2a, 3, 10b |
| `crates/core/src/linker/resolve.rs` | namespace + contenders + reach-gated duplicate check | 1, 2a |
| `crates/core/src/linker/interface.rs` | export-conflict pass, drift checks by name | 1, 3 |
| `crates/core/src/linker/engine.rs`, `stamp.rs` | undeclared-exit scan moves from warning to link error | 10b |
| `crates/core/src/asm/{mod.rs,syntax.rs,assembler.rs}` | `AsmErrorKind::UndeclaredExit`, `SyntaxEntry::is_exit_return` | 10a |
| `crates/post-machine/src/cli/{build.rs,driver.rs}` | link labels; comments on the entry owner | 1, 2b |
| `crates/turing-machine/src/cli/{build.rs,driver.rs}` | link labels, library-name pairing | 1 |
| `crates/turing-machine/src/published.rs` (new) | `PublishedForm`, `module_forms`, `own_forms`, `check_own_against_table` | 4, 5 |
| `crates/turing-machine/src/declarations.rs` | `Origin::describe`, `Declarations::entries`, `check_conflicts` | 5 |
| `crates/turing-machine/src/header.rs` | `settle_declarations`, `printed_graph_names`, digest content, interface graphs | 4, 13 |
| `crates/turing-machine/src/expand.rs` | wrong-kind nested miss; every spliced library graph recorded | 9, 13 |
| `crates/turing-machine/src/compiler.rs` | new codes, return rule, shared arg check, wrong-kind lookups, `ResolvedAlphabet::exported`, `Resolved::from_object`, `Provenance` | 4–9 |
| `crates/turing-machine/src/ir.rs` | shrinks to ordering for declared externals | 8 |
| `crates/turing-machine/src/lint/rules/*` | three new rules, `unused-tape` transparent fix, export exemption | 11, 12 |
| `crates/turing-machine/tests/one_definition.rs` (new) | TM CLI tests for the link rule | 1, 2b, 3 |
| `crates/post-machine/tests/one_definition.rs` (new) | PM CLI tests for the link rule | 1, 2b |
| `crates/turing-machine/tests/lint_rule_sweeps.rs` (new) | corpus silence of the new and changed rules | 11, 12 |

---

### Task 0: Baseline and corpus probes

**Deliverable.** A ledger file
`.superpowers/sdd/2026-09-30-language-tightening/baseline.md` (git-ignored, not
committed) recording the numbers every later task compares against. No code.

**Why a ledger, not a test.** Each later task pins its own corpus property with
a committed sweep (Tasks 2b, 4, 10b, 11, 12). This task only records the
starting point so a reviewer can tell a new finding from an old one.

- [ ] **Step 1: Branch.** `git fetch origin next`, then create
  `phase-4-language-a` from `origin/next` in a fresh worktree (the controller
  may have done this). Confirm `git log -1 --format=%h origin/next` and write
  it into the ledger.
- [ ] **Step 2: Build once.** `CARGO_TARGET_DIR=… cargo build` (debug) so
  `target/debug/tmt` and `target/debug/pmt` are this branch's binaries.
- [ ] **Step 3: Baseline suite.** `CARGO_TARGET_DIR=… cargo nextest run --workspace -j 4`
  once. Record passed/skipped/failed counts (last recorded on `next`: 4570
  passed, 10 skipped). Any failure here is pre-existing — record it by name so
  later tasks do not chase it.
- [ ] **Step 4: What relies on shadowing.** Record the output of each command
  (all read-only):
  - `git grep -n 'namespace std' -- 'docs/examples' 'crates/*/tests/golden'` —
    expected: no hit outside the stdlib sources. A hit is a shipped unit that
    would be refused by Task 2b; stop and report.
  - For every shipped `.tmc` (`docs/examples/*/*.tmc`,
    `crates/turing-machine/tests/golden/*.tmc`): `target/debug/tmt build FILE -o scratch/X.tmx`;
    record rc per file. Every file that builds today must still build after
    Task 2b and Task 5 (it is re-checked in those tasks' gates).
  - For every shipped `.pmc` under `crates/post-machine/tests/golden/`:
    `target/debug/pmt build FILE -o scratch/X.pmx`; record rc per file.
  - `git grep -n 'stdlib::object()' -- 'crates/*/tests' 'crates/*/src'` —
    inspect every link that passes the embedded stdlib object as a USER
    object, puts it in both lists, or links a compiled `std.tmc`/`std.pmc`
    object together with the default stdlib (a CLI call without
    `--nostdlib`): each is a reached duplicate after Task 2a. Add each hit to
    the flip list.
  - The `link`/`build` lines of `docs/examples/*/example.sh`,
    `docs/examples/run.sh` and `docs/examples/_lib.sh`: any that links a unit
    defining a name the stdlib or another input also defines.
  - `git grep -n 'duplicate symbol' -- docs README.md` — stale example output
    in the old bare-name form, to be updated in Task 1.
- [ ] **Step 5: Lint baseline.** `target/debug/tmt lint --no-config` over the
  same `.tmc` set plus `crates/turing-machine/src/stdlib/std.tmc`; record
  every finding with its code (expected today: `binding-product-threshold` in
  `rpnhex`/`rpnreg`/`rpnwide`, `unused-graft-name` in
  `tests/golden/a6_graph_graft_multi_exit.tmc` and `nested_graft.tmc`, nothing
  else). Record separately the four `unused-tape-parameter` false positives
  Task 11 must avoid: `docs/examples/rpn/rpn.tmc` routines `incr`, `decr`,
  `seekStart`, `normalize` (each body only a transparent `call std::…()`).
- [ ] **Step 6: Tests expected to flip.** Copy the "Tests that flip" list at
  the end of this plan into the ledger, one line each, to be ticked by the task
  that flips it.

---

### Task 1: A link error names the inputs it comes from

**Property.** Every link error that concerns two inputs names both, by the
label the caller gave each input — for user objects, `-l` libraries and the
embedded standard library alike — and today's duplicate-symbol case (two user
objects, the only duplicate the linker refuses yet) is the first to use it.

**Files:**
- Modify: `crates/core/src/linker/mod.rs` (`LinkError::DuplicateSymbol`, its
  `Display` arm, `LinkOptions` + `Default`, new `input_name`, `link`,
  `resolve_names`)
- Modify: `crates/core/src/linker/resolve.rs` (`resolve` signature,
  `object_columns`, the user-vs-user error, the existing unit tests'
  `DuplicateSymbol` assertions)
- Modify: `crates/core/src/linker/interface.rs` (`check_graft_drift`,
  `check_imported_alphabets` take names)
- Modify: `crates/core/src/linker/layout.rs` tests, `crates/core/src/asm/disassembler.rs`
  tests — only if they construct `LinkOptions` without a spread (they do not
  today; confirm by compiling)
- Modify: `crates/post-machine/src/cli/build.rs` (`link`, new `link_labels`),
  `crates/post-machine/src/cli/driver.rs` (`link_and_write`,
  `link_and_write_argv` and their callers)
- Modify: `crates/turing-machine/src/cli/build.rs` (`link`, new `link_labels`),
  `crates/turing-machine/src/cli/driver.rs` (`link_and_write`,
  `link_and_write_argv`, `resolve_target_declarations`,
  `link_library_objects` + its unit test)
- Modify: `docs/core.md` ("### Linking", Resolution bullets)
- Create: `crates/turing-machine/tests/one_definition.rs`,
  `crates/post-machine/tests/one_definition.rs`
- Test: `crates/core/tests/link_checks.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (used by Tasks 2a, 2b, 3, 10b):
  ```rust
  // crates/core/src/linker/mod.rs
  pub enum LinkError {
      /// One exported name defined by two inputs (docs/core.md (linking)).
      /// `first`/`second` are the two inputs' names, as `input_name` renders
      /// them; equal when one input defines the name twice.
      DuplicateSymbol { name: String, first: String, second: String },
      // … every other variant unchanged
  }
  pub struct LinkOptions {
      pub relax: bool,
      pub entry: Option<String>,
      pub call_mech: CallMech,
      pub sources: Vec<Option<String>>,
      /// How a link error names each input (docs/core.md (linking)), indexed
      /// like `sources` — user objects, then libraries. `None`, or a vector
      /// shorter than the input list, falls back to the input's `sources`
      /// entry, then to `input #i`. Never written to the map sidecar.
      pub labels: Vec<Option<String>>,
  }
  /// The name a link error gives input `i`: `names[i]` when present,
  /// otherwise `input #i`.
  pub(super) fn input_name(names: &[Option<String>], i: usize) -> String;
  // crates/core/src/linker/resolve.rs
  pub(crate) fn resolve<'a>(
      objects: &'a [ObjectFile],
      libraries: &'a [ObjectFile],
      entry: &str,
      names: &[Option<String>],
  ) -> Result<Resolved<'a>, LinkError>;
  // crates/core/src/linker/interface.rs
  pub(super) fn check_graft_drift(objects: &[ObjectFile], libraries: &[ObjectFile], names: &[Option<String>]) -> Result<(), LinkError>;
  pub(super) fn check_imported_alphabets(objects: &[ObjectFile], libraries: &[ObjectFile], names: &[Option<String>]) -> Result<(), LinkError>;
  // crates/{post,turing}-machine/src/cli/build.rs
  pub(crate) fn link_labels(units: &[String], libraries: &[String], stdlib: bool) -> Vec<Option<String>>;
  // crates/turing-machine/src/cli/driver.rs
  fn link_library_objects(names: &[String], library_objects: Vec<Option<ObjectFile>>) -> Vec<(String, ObjectFile)>;
  ```

**Label rule (the one place it is decided).** A unit or object is labelled by
the path string it was given as (the command-line argument, or the manifest's
raw source string); a `-l NAME` library as ``library `NAME` ``; the embedded
standard library as `the standard library`. `link` computes
`names[i] = options.labels[i].or(options.sources[i])` for every input and
threads `&names` to `resolve` and both drift checks. `resolve_names` passes
`&[]` (its only callers are editor-faithfulness tests; `input #i` suffices).

**Falsifying tests.**

| # | Fixture | Command / API | TODAY | AFTER | Test | Mutation that reddens it |
|---|---|---|---|---|---|---|
| 1 | core `A` = `.func main` / `call helper` / `stp` / `.func helper` / `ret`; `B` = `.func helper` / `ret` | `link(&fake_syntax(), &[asm(A), asm(B)], &[], LinkOptions { labels: [a.o, b.o], .. })` | `DuplicateSymbol("helper")` (tuple form) | `DuplicateSymbol { name: "helper", first: "a.o", second: "b.o" }`, displays ``duplicate symbol `helper`: defined by a.o and by b.o`` | `link_checks.rs::a_duplicate_symbol_names_both_inputs` | render the error without `first`/`second` (keep the old text) |
| 2 | same objects, `labels` empty, `sources = [Some("src/a.x"), None]` | same | — | `first: "src/a.x"`, `second: "input #1"` | `link_checks.rs::a_link_error_falls_back_to_sources_then_position` | `input_name` ignoring `sources` |
| 3 | TM `r1.tmc`, `r2.tmc`, `app.tmc` below **[verified]** | `tmt link app.tmo r1.tmo r2.tmo --nostdlib -o uu.tmx` | `tmt: duplicate symbol: lib::r`, rc 1 | Err containing ``duplicate symbol `lib::r`: defined by `` + r1 path + ` and by ` + r2 path | `one_definition.rs::a_duplicate_symbol_between_two_objects_names_both` | `tmt link` leaving `labels` empty (message says `input #1`/`input #2`) |
| 4 | TM `g1.tmc`, `g2.tmc`, `gapp.tmc` below **[verified]**; `gapp` compiled `--extern g1.tmc` | `tmt link gapp.tmo --nostdlib -L DIR -l g2 -o gd.tmx` | ``tmt: `lib::walk` was grafted into input #0 from a header that does not match input #1``, rc 1 | Err containing `` `lib::walk` was grafted into `` + gapp path + `` from a header that does not match library `g2` `` | `one_definition.rs::a_graft_drift_names_the_consumer_and_the_library` | the drift check still reading `sources` only |
| 5 | PM `pa.pmc` = `export f() { right; }` / `main() { @f(); }`, `pb.pmc` = `export f() { left; }` **[verified]** | `pmt link pa.pmo pb.pmo -o pab.pmx` | `pmt: duplicate symbol: f`, rc 1 | Err containing ``duplicate symbol `f`: defined by `` + pa path + ` and by ` + pb path | `post-machine/tests/one_definition.rs::a_duplicate_symbol_between_two_objects_names_both` | `pmt link` leaving `labels` empty |
| 6 | `link_library_objects(&["a","h","b"], vec![Some(a), None, Some(b)])` | unit test | returns `[a, b]` | returns `[("a", a), ("b", b)]` | `driver.rs::tests::link_library_objects_drops_a_header_only_entry` (rewritten) | pairing names by position in the flattened list (drops `b`'s name onto `h`) |

The TM fixtures (shared by Tasks 1–3; put them in `one_definition.rs` as
`const`s):

```rust
const R1: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab) {
    entry state s { [*] -> write ['a'] return; }
  }
}
";

const R2: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab) {
    entry state s { [*] -> write ['_'] return; }
  }
}
";

/// Calls `lib::r`; compiled with `--extern r1.tmc` so it also records an
/// import of `lib::ab`.
const APP: &str = "\
use lib::ab;
machine {
  tape t: ab;
  entry state s { [*] -> call lib::r(t = t) then stop; }
}
";

const G1: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export graph walk(tape t: ab, state done) {
    entry state w { [*] -> write ['a'] goto done; }
  }
}
";

const G2: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export graph walk(tape t: ab, state done) {
    entry state w { [*] -> write ['_'] goto done; }
  }
}
";

/// Grafts `lib::walk`; compiled with `--extern g1.tmc`, so it records
/// `.grafted lib::walk, 2183574135` (g1's digest; g2's is 328822984).
const GAPP: &str = "\
use lib::ab;
machine {
  tape t: ab;
  entry graft lib::walk(t = t, done = stop) as w;
}
";
```

Helpers for `crates/turing-machine/tests/one_definition.rs` (copy `scratch` and
`ScratchDir` from `tests/cross_unit.rs` verbatim, then add):

```rust
use std::path::{Path, PathBuf};
use mtc_turing_machine::cli::{CliOutput, execute};

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn run(argv: &[&str]) -> Result<CliOutput, String> {
    execute(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

/// `tmt compile SRC -o DIR/OUT --nostdlib [--extern E]…`, which must succeed.
fn compile_ok(dir: &Path, src: &Path, out: &str, externs: &[&Path]) -> PathBuf {
    let obj = dir.join(out);
    let mut argv = vec!["compile".to_string(), src.to_str().unwrap().into(), "--nostdlib".into()];
    for e in externs {
        argv.push("--extern".into());
        argv.push(e.to_str().unwrap().into());
    }
    argv.push("-o".into());
    argv.push(obj.to_str().unwrap().into());
    execute(&argv).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
    obj
}

/// The r1/r2/app trio, compiled into `dir`: returns (app, r1, r2) objects.
fn routine_trio(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let r1 = write(dir, "r1.tmc", R1);
    let r2 = write(dir, "r2.tmc", R2);
    let app = write(dir, "app.tmc", APP);
    (
        compile_ok(dir, &app, "app.tmo", &[&r1]),
        compile_ok(dir, &r1, "r1.tmo", &[]),
        compile_ok(dir, &r2, "r2.tmo", &[]),
    )
}

/// The g1/g2/gapp trio, compiled into `dir`: returns (gapp, g1, g2) objects.
fn graph_trio(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let g1 = write(dir, "g1.tmc", G1);
    let g2 = write(dir, "g2.tmc", G2);
    let gapp = write(dir, "gapp.tmc", GAPP);
    (
        compile_ok(dir, &gapp, "gapp.tmo", &[&g1]),
        compile_ok(dir, &g1, "g1.tmo", &[]),
        compile_ok(dir, &g2, "g2.tmo", &[]),
    )
}
```

Test 3 and test 4 bodies:

```rust
/// Two user objects defining `lib::r`: the error names both by the paths
/// they were given as. Mutation: `tmt link` leaves `LinkOptions::labels`
/// empty — the message then says `input #1` and `input #2`.
#[test]
fn a_duplicate_symbol_between_two_objects_names_both() {
    let dir = scratch("one_def_user_user");
    let (app, r1, r2) = routine_trio(&dir);
    let out = dir.join("uu.tmx");
    let err = run(&[
        "link", app.to_str().unwrap(), r1.to_str().unwrap(), r2.to_str().unwrap(),
        "--nostdlib", "-o", out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains("duplicate symbol `lib::r`: defined by "), "{err}");
    assert!(err.contains(&format!("{} and by {}", r1.display(), r2.display())), "{err}");
}

/// A graft drift names the consumer by its path and the exporting library
/// by its `-l` name. Mutation: the drift check keeps naming inputs from
/// `sources` alone — `tmt link` passes none, so the message says `input #0`.
#[test]
fn a_graft_drift_names_the_consumer_and_the_library() {
    let dir = scratch("one_def_drift_names");
    let (gapp, _g1, _g2) = graph_trio(&dir);
    let out = dir.join("gd.tmx");
    let err = run(&[
        "link", gapp.to_str().unwrap(), "--nostdlib",
        "-L", dir.to_str().unwrap(), "-l", "g2", "-o", out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(
        err.contains(&format!(
            "`lib::walk` was grafted into {} from a header that does not match library `g2`",
            gapp.display()
        )),
        "{err}"
    );
}
```

The PM test file mirrors it (copy `scratch`/`ScratchDir` from
`crates/post-machine/tests/build_driver.rs`):

```rust
use std::path::{Path, PathBuf};
use mtc_post_machine::cli::execute;

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn run(argv: &[&str]) -> Result<mtc_post_machine::cli::CliOutput, String> {
    execute(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

fn compile_ok(dir: &Path, name: &str, src: &str) -> PathBuf {
    let input = write(dir, name, src);
    let obj = input.with_extension("pmo");
    run(&["compile", input.to_str().unwrap(), "-o", obj.to_str().unwrap()])
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    obj
}

const PA: &str = "export f() { right; }\nmain() { @f(); }\n";
const PB: &str = "export f() { left; }\n";

/// Mutation: `pmt link` leaves `LinkOptions::labels` empty — `input #0/#1`.
#[test]
fn a_duplicate_symbol_between_two_objects_names_both() {
    let dir = scratch("pm_one_def_user_user");
    let pa = compile_ok(&dir, "pa.pmc", PA);
    let pb = compile_ok(&dir, "pb.pmc", PB);
    let out = dir.join("pab.pmx");
    let err = run(&["link", pa.to_str().unwrap(), pb.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .unwrap_err();
    assert!(
        err.contains(&format!(
            "duplicate symbol `f`: defined by {} and by {}",
            pa.display(),
            pb.display()
        )),
        "{err}"
    );
}
```

(Check `CliOutput`'s public path in the PM crate with `git grep -n 'pub struct CliOutput' crates/post-machine`; adjust the `use` if it is re-exported elsewhere.)

Core tests 1 and 2 (`crates/core/tests/link_checks.rs`, beside the drift tests):

```rust
const DUP_A: &str = "\
.func main
        call    helper
        stp
.func helper
        ret
";

const DUP_B: &str = "\
.func helper
        ret
";

/// Mutation it catches: render `DuplicateSymbol` without its two input
/// names — the equality and the text assertion both fail.
#[test]
fn a_duplicate_symbol_names_both_inputs() {
    let err = link(
        &fake_syntax(),
        &[asm(DUP_A), asm(DUP_B)],
        &[],
        LinkOptions {
            labels: vec![Some("a.o".into()), Some("b.o".into())],
            ..Default::default()
        },
    )
    .expect_err("two user definitions of one name");
    assert_eq!(
        err,
        LinkError::DuplicateSymbol {
            name: "helper".into(),
            first: "a.o".into(),
            second: "b.o".into(),
        }
    );
    assert_eq!(err.to_string(), "duplicate symbol `helper`: defined by a.o and by b.o");
}

/// Mutation it catches: `input_name` ignoring `sources` — `first` becomes
/// `input #0`.
#[test]
fn a_link_error_falls_back_to_sources_then_position() {
    let err = link(
        &fake_syntax(),
        &[asm(DUP_A), asm(DUP_B)],
        &[],
        LinkOptions {
            sources: vec![Some("src/a.x".into()), None],
            ..Default::default()
        },
    )
    .expect_err("two user definitions of one name");
    assert_eq!(
        err,
        LinkError::DuplicateSymbol {
            name: "helper".into(),
            first: "src/a.x".into(),
            second: "input #1".into(),
        }
    );
}
```

**Implementation.**

1. `linker/mod.rs`: change the variant as in Interfaces. `Display`:
   ```rust
   Self::DuplicateSymbol { name, first, second } if first == second => {
       write!(f, "duplicate symbol `{name}`: defined twice by {first}")
   }
   Self::DuplicateSymbol { name, first, second } => {
       write!(f, "duplicate symbol `{name}`: defined by {first} and by {second}")
   }
   ```
   Add `labels: Vec::new()` to `Default`. Add `input_name`:
   ```rust
   pub(super) fn input_name(names: &[Option<String>], i: usize) -> String {
       names
           .get(i)
           .and_then(Option::as_ref)
           .cloned()
           .unwrap_or_else(|| format!("input #{i}"))
   }
   ```
   In `link`, before `resolve`:
   ```rust
   let names: Vec<Option<String>> = (0..objects.len() + libraries.len())
       .map(|i| {
           options.labels.get(i).cloned().flatten()
               .or_else(|| options.sources.get(i).cloned().flatten())
       })
       .collect();
   ```
   and pass `&names` to `resolve::resolve`, `interface::check_graft_drift`,
   `interface::check_imported_alphabets`. `resolve_names` passes `&[]`.
2. `resolve.rs`: `object_columns` gains `names: &[Option<String>]` and on a
   strict repeat returns `DuplicateSymbol { name, first: input_name(names, oi),
   second: input_name(names, oi) }`. The user-vs-user arm in `resolve` needs
   the FIRST definer: record `owner: HashMap<&str, usize>` beside
   `namespace` (it becomes the contender list in Task 2a — you may introduce
   `contenders: HashMap<&str, Vec<usize>>` now and read `contenders[name][0]`).
   Update the existing unit-test assertions (`DuplicateSymbol("f".into())` →
   the struct form with `input #0`/`input #1`, or `input #0` twice for the
   in-object cases) — their meaning does not change. Every existing
   `resolve(…, "main")` call in `resolve.rs`'s unit tests (about 25) gains a
   trailing `&[]`; the compiler lists them.
3. `interface.rs`: replace both `name_of` closures by `super::input_name(names, i)`.
4. Both CLIs: add `link_labels` to `cli/build.rs` (PM and TM, identical):
   ```rust
   /// How a link error names each input (docs/core.md (linking)): a unit or
   /// object by the path it was given as, a `-l` library as ``library `NAME` ``,
   /// the embedded standard library as `the standard library` — in the
   /// linker's own input order: units, then libraries, then the stdlib.
   pub(crate) fn link_labels(units: &[String], libraries: &[String], stdlib: bool) -> Vec<Option<String>> {
       units
           .iter()
           .map(|u| Some(u.clone()))
           .chain(libraries.iter().map(|l| Some(format!("library `{l}`"))))
           .chain(stdlib.then(|| Some("the standard library".to_string())))
           .collect()
   }
   ```
   Fill `labels` at every link site: `pmt link`/`tmt link` (`inputs`,
   `lib_names`, `!nostdlib`); `pmt build`/`tmt build` manifest mode (the raw
   `manifest.effective_sources(target)` strings; the library names that
   contributed an OBJECT; `manifest.stdlib`); argv mode (`files` as given;
   `flags.lib_names` — TM: the names paired by `link_library_objects`;
   `!flags.nostdlib`). TM: `link_library_objects(names, objects)` pairs each
   object with its name before flattening, `resolve_target_declarations`
   returns `Vec<(String, ObjectFile)>` for its third element, and each caller
   splits it into `libraries` and `library_names`. Rewrite the unit test
   `link_library_objects_drops_a_header_only_entry` to assert the pairs.
   Leave `sources` exactly as it is at every site.
5. `docs/core.md` "### Linking", Resolution: add one bullet after the
   duplicate bullet — "A link error that concerns two inputs names both: a
   driver labels each input it hands the linker (a path, a library name, the
   standard library), and an unlabelled input is named by its position." (The
   shadowing bullets change in Task 2b.)

**Steps.**
- [ ] Before editing: `git grep -n 'LinkError::' -- crates/wasm/src crates/post-machine/src crates/turing-machine/src`
  — any exhaustive `match` on `LinkError` outside core must gain the new shape
  (and, in Tasks 3 and 10b, the new variants); list what you find.
- [ ] Write tests 1–6; run `cargo nextest run -p mtc-core --test link_checks -E 'test(/duplicate_symbol|falls_back/)'` — compile error on the struct variant is the expected first red; then (after adding the variant only) the assertion red.
- [ ] Implement 1–4. Update any `duplicate symbol: NAME` example output the
  Task 0 grep found in `docs/`.
- [ ] Focused green: core `link_checks`, core lib (`resolve` tests), TM `--test one_definition`, PM `--test one_definition`, TM lib (`link_library_objects…`).
- [ ] Mutations: empty labels in `tmt link` → test 3 red; `input_name` ignoring sources → test 2 red.
- [ ] Gate: `cargo nextest run -p mtc-core -p mtc-post-machine -p mtc-turing-machine -j 4` once; PM byte identity (`--test golden_programs --test asm_volatile`) is inside the PM run; then `cargo nextest run -p mtc-wasm -j 4` once (the browser binding links through the same `LinkOptions` and `LinkError`).
- [ ] Commit: `feat(core): a link error names the inputs it comes from`.

---

### Task 2a: One definition per code symbol at link — the core rule

**Property.** In `crates/core`, a code symbol defined by two inputs is refused
whenever it involves a library and the program reaches it — user object
against library, library against library — while two user objects stay
refused whether reached or not, two libraries that share an UNREACHED name
still link, and ONE library object defining a name twice keeps its first
record (compiler output cannot produce that). "Reached" is the linker's
existing BFS: a name is contested exactly when a reference that ends up in
the program resolves to it (the entry is always reached).

**Scope of 2a, and what stays red until 2b.** 2a changes `crates/core` only
and is gated on the core crate plus PM-1 byte identity. The toolchain tests
that pinned the old shadowing — PM `visibility_programs.rs::deliberate_interposition_via_namespace_injection`,
`stdlib_programs.rs::user_namespace_injection_overrides_a_std_routine`, and
the two `faithfulness` tests in each of PM and TM `lsp/overlay.rs` — go red at
2a and are rewritten in 2b, which lands immediately after on the same branch.
The 2a reviewer checks that exactly these six are red after 2a and nothing
else in `mtc-post-machine`/`mtc-turing-machine` (one run of each crate,
recorded in the report).

**Files:**
- Modify: `crates/core/src/linker/resolve.rs` (module doc; `Resolved::dropped`
  doc; `object_columns` doc; `resolve`: keep `contenders`, the reach-gated
  check after the BFS; rewrite four unit tests, add four)
- Modify: `crates/core/src/linker/mod.rs` (`LinkError::DuplicateSymbol` doc)
- Modify: `crates/core/tests/link_cross_object.rs` (doc wording of
  `a_symbolic_binding_resolves_against_a_library_callee` — "first-wins library
  namespace" → "library namespace"; behaviour unchanged)
- Test: `crates/core/src/linker/resolve.rs` (unit), `crates/core/tests/link_checks.rs`

**Interfaces:**
- Consumes (Task 1): `LinkError::DuplicateSymbol { name, first, second }`,
  `input_name(names, i)`, `resolve(objects, libraries, entry, names)`,
  `LinkOptions::labels`, `DUP_A`/`DUP_B` in `link_checks.rs`.
- Produces (used by 2b): no new names. `resolve` keeps its Task 1 signature;
  its contract changes as in Property. `resolve_names` inherits it (an editor
  fixture with a reached sibling-versus-library collision now gets
  `Err(DuplicateSymbol)`).

**Falsifying tests.**

| # | Fixture | Command / API | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | unit: user `obj(&[("main",&["go"]),("go",&[])])`, lib `obj(&[("go",&[]),("unused_pulls_nothing",&["ghost"])])` | `resolve(&[user], &[lib], "main", &[])` | Ok, order `[main, go]` | `Err(DuplicateSymbol { name: "go", first: "input #0", second: "input #1" })` | `resolve.rs::a_reached_name_a_user_object_and_a_library_define_is_a_duplicate` | drop the contenders check (first-wins restored) |
| 2 | unit: user `main` only calls `go`; lib as above | same | Ok, 2 functions, dropped `[unused_pulls_nothing]` | unchanged | `resolve.rs::libraries_resolve_lazily` (the second half of the old test, kept) | report every contended name, reached or not |
| 3 | unit: user `a` = main + `f` (unreached); libs `lib1`,`lib2` each define `f` | `resolve(&[a], &[lib1, lib2], "main", &[])` | Ok | Ok (unreached); and with main calling `f`: `Err(DuplicateSymbol { name: "f", first: "input #0", second: "input #1" })` | `resolve.rs::duplicate_user_symbols_error_even_unreached_and_library_duplicates_only_when_reached` (replaces `duplicate_user_symbols_error_but_library_shadowing_does_not`, whose user-vs-user half it keeps) | report library duplicates regardless of reach (the unreached half reddens) |
| 4 | unit: the old `a_shadowing_normal_only_pair_forces_a_counted_fallback` fixtures | `resolve(..)` | Ok, fallback counted, origin 0 / 1 | part 1 `Err(DuplicateSymbol { name: "f", first: "input #0", second: "input #1" })`; part 2 `Err(DuplicateSymbol { name: "g", first: "input #1", second: "input #2" })` | `resolve.rs::a_second_input_offering_a_reached_name_is_a_duplicate_whatever_its_columns` | count a name's contenders per COLUMN instead of per input — part 2 pits a Normal-only library `g` against a Volatile-only one, which that mutation would let through |
| 5 | unit: user and lib both define `main` | `resolve(&[u], &[l], "main", &[])` | Ok (user's) | `Err(DuplicateSymbol { name: "main", .. })` | `resolve.rs::an_entry_two_inputs_define_is_a_duplicate` | gate on "reached through a relocation" instead of on the chosen site being in the order (the entry is reached by no relocation) |
| 6 | unit: `resolve_names` with user `dup` + lib `dup`, main calls dup | `resolve_names(..)` | Ok, origin `Object(0)` | `Err(DuplicateSymbol { name: "dup", .. })` | `resolve.rs::resolve_names_refuses_a_reached_name_two_inputs_define` (replaces `resolve_names_user_definition_shadows_library`) | `resolve_names` building its own namespace |
| 7 | core `DUP_A` (Task 1) as a user object, `DUP_B` as a library, labels `[a.o, b.lib]` | `link(..)` | Ok | `DuplicateSymbol { name: "helper", first: "a.o", second: "b.lib" }` | `link_checks.rs::a_reached_name_a_library_also_defines_is_refused` | the contenders check reading only user objects |
| 8 | unit: library `obj(&[("f",&[]),("f",&[])])` (two untagged records of `f` in ONE library object), user `main` calls `f` | `resolve(&[user], &[lib], "main", &[])` | Ok, `f` linked from the library's FIRST blob | unchanged | `resolve.rs::one_library_defining_a_name_twice_keeps_its_first_record` | count contenders per RECORD instead of per input (one library then contests itself) |

Unit-test bodies for the rewritten `resolve.rs` tests (the `obj`/`variant_obj`
helpers exist in that module):

```rust
/// A user object and a library both define `go`, and `main` reaches it:
/// one name, two definitions in the program — refused, naming both.
/// Mutation: restore `or_insert` first-wins without the contenders check —
/// this resolves Ok again.
#[test]
fn a_reached_name_a_user_object_and_a_library_define_is_a_duplicate() {
    let user = obj(0x7E, &[("main", &["go"]), ("go", &[])]);
    let lib = obj(0x7E, &[("go", &[]), ("unused_pulls_nothing", &["ghost"])]);
    let e = resolve(
        std::slice::from_ref(&user),
        std::slice::from_ref(&lib),
        "main",
        &[],
    )
    .unwrap_err();
    assert_eq!(
        e,
        LinkError::DuplicateSymbol {
            name: "go".into(),
            first: "input #0".into(),
            second: "input #1".into(),
        }
    );
}

/// Libraries resolve lazily: only what `main` reaches is linked, and a
/// library's broken, unreached function never errors. Mutation: report
/// every name two inputs define, reached or not — not applicable here (one
/// definer each); this pins the lazy half the rewritten test above lost.
#[test]
fn libraries_resolve_lazily() {
    let needy = obj(0x7E, &[("main", &["go"])]);
    let lib = obj(0x7E, &[("go", &[]), ("unused_pulls_nothing", &["ghost"])]);
    let r = resolve(
        std::slice::from_ref(&needy),
        std::slice::from_ref(&lib),
        "main",
        &[],
    )
    .unwrap();
    assert_eq!(r.order.len(), 2);
    assert_eq!(r.dropped, vec!["unused_pulls_nothing".to_string()]);
}

/// Two user objects defining one name are refused even unreached; two
/// libraries sharing an unreached name link; the same two libraries are
/// refused once `main` reaches the name. Mutation: report library
/// duplicates regardless of reach — the middle assertion fails.
#[test]
fn duplicate_user_symbols_error_even_unreached_and_library_duplicates_only_when_reached() {
    let a = obj(0x7E, &[("main", &[]), ("f", &[])]);
    let b = obj(0x7E, &[("f", &[])]);
    assert_eq!(
        resolve(&[a, b], &[], "main", &[]).unwrap_err(),
        LinkError::DuplicateSymbol {
            name: "f".into(),
            first: "input #0".into(),
            second: "input #1".into(),
        }
    );
    let lone = obj(0x7E, &[("main", &[])]);
    let lib1 = obj(0x7E, &[("f", &[])]);
    let lib2 = obj(0x7E, &[("f", &[])]);
    let r = resolve(std::slice::from_ref(&lone), &[lib1.clone(), lib2.clone()], "main", &[])
        .expect("an unreached name two libraries share links");
    assert_eq!(r.dropped, vec!["f".to_string()]);
    let caller = obj(0x7E, &[("main", &["f"])]);
    assert_eq!(
        resolve(std::slice::from_ref(&caller), &[lib1, lib2], "main", &[]).unwrap_err(),
        LinkError::DuplicateSymbol {
            name: "f".into(),
            first: "input #1".into(),
            second: "input #2".into(),
        }
    );
}

/// A second input offering a reached name is a duplicate whatever columns
/// either offers — the two-column rule never completes a pair across
/// inputs, and now never picks one either. Mutation: count contenders per
/// column rather than per input — the Normal-against-Volatile shape passes.
#[test]
fn a_second_input_offering_a_reached_name_is_a_duplicate_whatever_its_columns() {
    let mut user = variant_obj(
        0x7E,
        &[
            ("main", BlobVariant::Normal, &["f"][..]),
            ("main", BlobVariant::Volatile, &["f"][..]),
            ("f", BlobVariant::Normal, &[][..]),
        ],
    );
    user.program_volatile = true;
    let lib = variant_obj(
        0x7E,
        &[
            ("f", BlobVariant::Normal, &[][..]),
            ("f", BlobVariant::Volatile, &[][..]),
        ],
    );
    assert_eq!(
        resolve(std::slice::from_ref(&user), std::slice::from_ref(&lib), "main", &[]).unwrap_err(),
        LinkError::DuplicateSymbol {
            name: "f".into(),
            first: "input #0".into(),
            second: "input #1".into(),
        }
    );

    let mut caller = variant_obj(0x7E, &[("main", BlobVariant::Volatile, &["g"][..])]);
    caller.program_volatile = true;
    let first = variant_obj(0x7E, &[("g", BlobVariant::Normal, &[][..])]);
    let second = variant_obj(0x7E, &[("g", BlobVariant::Volatile, &[][..])]);
    assert_eq!(
        resolve(std::slice::from_ref(&caller), &[first, second], "main", &[]).unwrap_err(),
        LinkError::DuplicateSymbol {
            name: "g".into(),
            first: "input #1".into(),
            second: "input #2".into(),
        }
    );
}

/// The entry is always reached. Mutation: gate on "a relocation or bound
/// call resolved to it" instead of on the chosen site being in the order —
/// the entry has no such reference and passes.
#[test]
fn an_entry_two_inputs_define_is_a_duplicate() {
    let user = obj(0x7E, &[("main", &[])]);
    let lib = obj(0x7E, &[("main", &[])]);
    assert!(matches!(
        resolve(std::slice::from_ref(&user), std::slice::from_ref(&lib), "main", &[]),
        Err(LinkError::DuplicateSymbol { name, .. }) if name == "main"
    ));
}

/// `resolve_names` runs the same rule. Mutation: a `resolve_names` that
/// builds its own first-wins namespace — Ok again.
#[test]
fn resolve_names_refuses_a_reached_name_two_inputs_define() {
    let a = obj(0x7E, &[("main", &["dup"]), ("dup", &[])]);
    let lib = obj(0x7E, &[("dup", &[])]);
    assert!(matches!(
        resolve_names(std::slice::from_ref(&a), std::slice::from_ref(&lib), "main"),
        Err(LinkError::DuplicateSymbol { name, .. }) if name == "dup"
    ));
}
```

Core link-level test 7 (`link_checks.rs`, reusing Task 1's `DUP_A`/`DUP_B`):

```rust
/// Mutation it catches: the contenders check consulting user objects only.
#[test]
fn a_reached_name_a_library_also_defines_is_refused() {
    let err = link(
        &fake_syntax(),
        &[asm(DUP_A)],
        &[asm(DUP_B)],
        LinkOptions {
            labels: vec![Some("a.o".into()), Some("b.lib".into())],
            ..Default::default()
        },
    )
    .expect_err("a reached name two inputs define");
    assert_eq!(
        err,
        LinkError::DuplicateSymbol {
            name: "helper".into(),
            first: "a.o".into(),
            second: "b.lib".into(),
        }
    );
}
```

Row 8's body:

```rust
/// One library object carrying two records of `f` keeps the first, as it
/// always did — it is one input, and the rule counts inputs. Mutation:
/// count contenders per record — the library then contests itself and this
/// becomes a `DuplicateSymbol`.
#[test]
fn one_library_defining_a_name_twice_keeps_its_first_record() {
    let user = obj(0x7E, &[("main", &["f"])]);
    let mut lib = obj(0x7E, &[("f", &[]), ("f", &[])]);
    lib.blobs[1].push(0x0E); // tell the two bodies apart
    let r = resolve(std::slice::from_ref(&user), std::slice::from_ref(&lib), "main", &[])
        .expect("one library is one input");
    let f = r.order.iter().find(|func| func.name == "f").expect("f is reached");
    assert_eq!(f.blob.as_ref(), lib.blobs[0].as_slice(), "the first record is the one linked");
}
```


**Implementation (`resolve.rs`).** Beside `namespace`, build
`contenders: HashMap<&str, Vec<usize>>` — for every input (user objects,
then libraries) and every name `object_columns` returns for it, push the
input index once. The user loop keeps its immediate error (a second user
definer): `first = contenders[name][0]`, `second = oi`. The library loop keeps
`namespace.entry(name).or_insert(pair)` (the first definer stays the one the
BFS walks, so unreached-name behaviour and `dropped` are unchanged) and only
records the contender. After the BFS and the existing `Unresolved` check:

```rust
// One definition per name (docs/core.md (linking)): a name two inputs
// define is refused once the program reaches it — the BFS walked the
// first definer's chosen column, so "reached" is that site being in the
// order. Names are checked in sorted order so the error is deterministic.
let mut contested: Vec<(&str, &Vec<usize>)> = contenders
    .iter()
    .filter(|(_, definers)| definers.len() > 1)
    .map(|(name, definers)| (*name, definers))
    .collect();
contested.sort_by_key(|(name, _)| *name);
for (name, definers) in contested {
    let (site, _) = namespace[name].choose(program_volatile);
    if index_of.contains_key(&site) {
        return Err(LinkError::DuplicateSymbol {
            name: name.to_string(),
            first: super::input_name(names, definers[0]),
            second: super::input_name(names, definers[1]),
        });
    }
}
```

Rewrite the module doc's first sentence ("user duplicates error, libraries
first-wins and shadowed silently by user definitions") to: user duplicates
error always; a name any two inputs define is an error once reached; one
library defining a name twice keeps its first record. `Resolved::dropped`'s
doc: "Sorted names whose chosen definition went unreached (a name several
inputs define is listed once)". `object_columns`' doc: drop "exactly as
`first-wins` already silently shadows a second library's copy".


**Steps.**
- [ ] Write rows 1–8 and the rewritten unit tests; run
  `cargo nextest run -p mtc-core --lib -E 'test(/resolve::/)'` and
  `--test link_checks`; see 1, 3 (reached half), 4, 5, 6, 7 red; 2 and 8
  green.
- [ ] Implement the contenders check; update the module and field docs.
- [ ] Focused green; mutations (drop the check → 1 red; ignore reach → 3's
  middle assertion red; per-column counting → 4 red; per-record counting →
  8 red).
- [ ] Gate: `cargo nextest run -p mtc-core -j 4` once;
  `cargo nextest run -p mtc-post-machine --test golden_programs --test asm_volatile -j 4`;
  then one run of `-p mtc-post-machine` and `-p mtc-turing-machine`
  confirming that the ONLY red tests are the six named under "Scope of 2a".
- [ ] Commit: `feat(core): a name two inputs define is refused once the program reaches it`.
  Ledger `changelog-notes.md`: BREAKING — a library definition can no longer
  be shadowed; "Overriding std" is gone, `--nostdlib` replaces the library
  wholesale.

---

### Task 2b: One definition at link — the toolchains' surfaces and docs

**Property.** Both toolchains show 2a's rule end to end: `pmt`/`tmt` `link`
and `build` refuse a reached name that a user object and a library, two
libraries, or a unit and the embedded standard library both define, naming
both by their labels; an unreached shared name links; the tests that pinned
shadowing now pin the refusal; and every page and comment that promised
first-wins shadowing says one definition instead.

**Files:**
- Modify: `crates/post-machine/tests/visibility_programs.rs`
  (`deliberate_interposition_via_namespace_injection` → rewritten),
  `crates/post-machine/tests/stdlib_programs.rs`
  (`user_namespace_injection_overrides_a_std_routine` → rewritten)
- Modify: `crates/post-machine/src/lsp/overlay.rs` (`faithfulness` module:
  `overlay_resolution_matches_linker_resolution_with_provenance`,
  `overlay_resolution_matches_linker_resolution_for_a_shadowed_std_name`)
- Modify: `crates/turing-machine/src/lsp/overlay.rs` (the same two tests)
- Modify: `crates/post-machine/src/cli/driver.rs` (doc of
  `entry_owner_is_volatile`; the unit test
  `a_library_may_own_the_entry_and_supply_the_bit`, whose comment says the
  library copy is "silently shadowed": rewrite the comment to say the helper
  still answers "units first, then the first library" for the column
  choice, and that two inputs defining the entry is a link error the linker
  reports after this helper runs — its assertions are about the helper, not
  the link, and stay), `crates/turing-machine/src/cli/build.rs` (comment
  above the stdlib push in `link`)
- Modify: `crates/wasm/src/inner/program.rs` (doc comment near line 267 on
  `"user"` provenance for a routine "shadowing a `std::` name")
- Modify docs: `docs/core.md` (Linking: Resolution bullets; Build columns
  bullet), `docs/pmt/stdlib.md` (Linking semantics: "Symbol resolution",
  "Overriding std", "Interposition vs optimization"), `docs/pmt/language.md`
  (the **Interposition** paragraph), `docs/tmt/stdlib.md` (the twins'
  "shadowed by a same-named definition" sentence; "Symbol resolution"
  bullet), `docs/pmt/project.md` and `docs/tmt/project.md` (the "Library
  resolution is first-wins…" paragraph), `docs/tmt/cli.md` (`### --nostdlib`
  last sentence), `docs/lsp.md` (Resolution order paragraph; the
  faithfulness sentence ending "including the shadowed-stdlib case above";
  the `namespace std { export routine … }` sentence near the stdlib
  section; a new Caveats bullet), `docs/wasm.md` (the `"user"` sentence),
  `CLAUDE.md` (Linker paragraph: "Libraries are first-wins and silently
  shadowed by user definitions."; Editor integration bullet
  "first-source-wins")
- Test: `crates/turing-machine/tests/one_definition.rs`,
  `crates/post-machine/tests/one_definition.rs`

**Interfaces:**
- Consumes (Task 1): `link_labels`, the `one_definition.rs` helpers
  (`routine_trio`, `run`, `write`, `compile_ok`) and fixtures `R1`, `R2`,
  `APP`, `PA`, `PB`. Consumes (2a): the rule.
- Produces: fixtures `APP0`, `OWNSAME` (TM) and `OVERRIDE`, `PNEAR` (PM);
  Task 5 reuses `OWNSAME`.

**Falsifying tests.**

| # | Fixture | Command / API | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | TM trio **[verified]** | `tmt link app.tmo r1.tmo --nostdlib -L DIR -l r2` | rc 0 | Err ``duplicate symbol `lib::r`: defined by `` + r1 path + `` and by library `r2` `` | `one_definition.rs::a_reached_name_an_object_and_a_library_both_define_is_refused` | TM `tmt link` not labelling `-l` libraries |
| 2 | TM trio **[verified]** | `tmt link app.tmo --nostdlib -L DIR -l r1 -l r2` | rc 0 | Err ``… defined by library `r1` and by library `r2` `` | `one_definition.rs::a_reached_name_two_libraries_define_is_refused` | library-vs-library left first-wins |
| 3 | TM `APP0` below (imports `lib::ab`, calls nothing) **[verified]** | `tmt link app0.tmo --nostdlib -L DIR -l r1 -l r2` | rc 0 | rc 0 | `one_definition.rs::an_unreached_name_two_libraries_share_still_links` | the check ignoring reach |
| 4 | TM `OWNSAME` below **[verified]**: defines `std::binaryNumbers::goToNumber` with the stdlib's exact published signature | `tmt build ownsame.tmc -o ownsame.tmx` (stdlib on) | rc 0 (the user body silently wins) | Err ``duplicate symbol `std::binaryNumbers::goToNumber`: defined by `` + ownsame path + ` and by the standard library` | `one_definition.rs::a_user_definition_of_a_reached_std_routine_is_refused` | `tmt build` not labelling the stdlib (`input #1`) |
| 5 | PM `OVERRIDE` below **[verified]** | `pmt build override.pmc -o ov.pmx` | rc 0 | Err ``duplicate symbol `std::goToEnd`: defined by `` + override path + ` and by the standard library` | `post-machine/tests/one_definition.rs::a_user_definition_of_a_reached_std_routine_is_refused` | PM driver not labelling the stdlib |
| 6 | PM `PNEAR` below **[verified]**: defines `std::goToEnd`, never calls it | `pmt build pnear.pmc -o pn.pmx` | rc 0 | rc 0 | `post-machine/tests/one_definition.rs::an_unreached_user_definition_of_a_std_name_still_links` | the check ignoring reach |
| 7 | PM `pa.pmo` user, `pb.pmo` via `-L DIR -l pb` **[verified]** | `pmt link pa.pmo -L DIR -l pb -o pl.pmx` | rc 0 | Err ``duplicate symbol `f`: defined by `` + pa path + `` and by library `pb` `` | `post-machine/tests/one_definition.rs::a_reached_name_an_object_and_a_library_both_define_is_refused` | PM `pmt link` not labelling `-l` |

```rust
// crates/turing-machine/tests/one_definition.rs — additional fixtures
const APP0: &str = "\
use lib::ab;
machine {
  tape t: ab;
  entry state s { [*] -> stop; }
}
";

/// The stdlib's own `goToNumber` signature, re-defined by a user unit:
/// `tmt interface` prints it identically to `std.tmh`'s line, so the
/// compile-time one-declaration rule lets it through and the link must
/// refuse it.
const OWNSAME: &str = "\
namespace std {
  namespace binaryNumbers {
    export routine goToNumber(tape num: std::binaryNumbers::symbols writes {} leaves { '$' }) {
      entry state walk {
        ['$'] -> return;
        [*]   -> move [>] goto walk;
      }
    }
  }
}

machine {
  tape num: std::binaryNumbers::symbols;
  entry state s { [*] -> call std::binaryNumbers::goToNumber(num = num) then stop; }
}
";
```

```rust
// crates/post-machine/tests/one_definition.rs — additional fixtures
const OVERRIDE: &str = "\
namespace std { export goToEnd() { 1: left(!); } }
use std::goToEnd;
main() { @goToEnd(!); }
";

const PNEAR: &str = "\
namespace std { export goToEnd() { 1: left(!); } }
main() { right; }
";
```

TM CLI tests 1–4 follow Task 1's test 3 shape (`routine_trio`, `run`,
`unwrap_err()`/`expect`); test 4 writes `OWNSAME` and runs
`run(&["build", path, "-o", out])` with the stdlib on, asserting
`err.contains(&format!("duplicate symbol `std::binaryNumbers::goToNumber`: defined by {} and by the standard library", path.display()))`.
Test 3 compiles `APP0` with `--extern r1.tmc` and expects `Ok` with
`code == 0`. PM tests 5–7 follow the PM file's shape; 5 and 6 run
`run(&["build", path, "-o", out])`.

**Rewrites of the flipping tests** (keep each test's property; restate its
premise):

- `crates/post-machine/tests/visibility_programs.rs::deliberate_interposition_via_namespace_injection`
  → rename `namespace_injection_of_a_reached_library_name_is_refused`; same two
  compiles; assert `link(&[user.object], &[lib.object], LinkOptions::default())`
  is `Err(LinkError::DuplicateSymbol { name, .. })` with `name == "std::step"`.
  Doc: the user and the library define one name the program reaches, which is
  a link error (docs/pmt/stdlib.md (linking semantics)).
- `crates/post-machine/tests/stdlib_programs.rs::user_namespace_injection_overrides_a_std_routine`
  → rename `a_user_definition_of_a_std_routine_it_calls_is_refused`; compile the
  same source, `link(&[out.object], std::slice::from_ref(stdlib::object()), LinkOptions::default())`
  must be `Err(DuplicateSymbol { name: "std::goToEnd", .. })`.
- Both `faithfulness` modules (PM and TM `lsp/overlay.rs`):
  - `overlay_resolution_matches_linker_resolution_with_provenance`: delete
    `ns::dup` from the LIBRARY's source (PM `bitops` compiles
    `"export bit_only() { right; }\n"`; TM the matching `bitops` source keeps
    only `bit_only`), drop the "THE discriminating assertion" block and its
    comment, and keep every other provenance assertion. The overlay's pick of
    `ns::dup` (now single-definer) is still asserted.
  - `overlay_resolution_matches_linker_resolution_for_a_shadowed_std_name` →
    rename `a_sibling_defining_a_reached_std_name_is_a_link_error_the_overlay_does_not_yet_report`.
    Keep the overlay half verbatim (the overlay still picks the sibling — the
    overlay's own duplicate diagnostic is the editor spec's). Replace the
    linker half: `resolve_names(&objects, &libraries, "main")` must be
    `Err(LinkError::DuplicateSymbol { name, .. })` naming the std routine (TM:
    drop the `crate::asm::link(..).expect(..)` call, which now errors too).
    **TM fixture change, required by Task 5 as well:** the TM `SHARED` source
    must declare the stdlib's exact signature, or Task 5 refuses to compile it
    before the linker is reached:
    ```
    namespace std {
    namespace binaryNumbers {
    export routine goToNumber(tape num: std::binaryNumbers::symbols writes {} leaves { '$' }) { entry state s { [*] -> return; } }
    }
    }
    ```
    and the TM `APP` becomes
    ```
    machine {
      tape t: std::binaryNumbers::symbols;
      entry state s { [*] -> call std::binaryNumbers::goToNumber() then done; }
      state done { [*] -> stop; }
    }
    ```
    Mutation for the rewritten test: first-wins restored in `resolve` —
    `resolve_names` returns Ok and the test reddens.
- `crates/core/src/linker/resolve.rs`: the four tests named in the table.
  `foreign_locals_are_unresolvable_and_locals_never_shadow`,
  `locals_bind_directly_and_may_repeat_across_objects`,
  `columns_of_one_name_never_span_two_objects`,
  `only_a_normal_volatile_pair_shares_one_name_in_one_object` keep their
  behaviour (only Task 1's variant shape changed in them).


**Docs (substance to write).**
- `docs/core.md` Linking, Resolution: replace the two bullets
  "Duplicate exported symbols across user objects are a link-time error." /
  "Libraries resolve **first-wins** and may be shadowed…" by: "An exported
  name has one definition in a link. Two user objects defining one name are a
  link-time error whether or not the program reaches it; a name a library also
  defines — against a user object or another library, the embedded standard
  library included — is an error once the program reaches it, so two libraries
  that happen to share a name nothing calls still link together. Inside ONE
  library object a name recorded twice keeps its first record; compiler output
  never produces that shape." Build columns bullet: "A name's two columns
  always come from ONE input: a definition is one input's column pair, and a
  second input offering the same name is a duplicate whatever its columns."
- `docs/pmt/stdlib.md` Linking semantics: "Symbol resolution" → one
  definition per name (as above, in PM words); **delete "Overriding std"** and
  write in its place: "**Replacing std:** a library routine cannot be
  replaced one at a time — defining a `std::` name the program calls is a
  duplicate-symbol link error. To replace the library, link without it
  (`--nostdlib`) and supply your own set." Replace the "Interposition vs
  optimization" bullet by one sentence: the library's own internal calls are
  bound inside the library object and nothing can interpose on them.
- `docs/pmt/language.md` **Interposition** paragraph: delete it (nothing can
  shadow a library definition any more); if a sentence about `-O1` binding
  intra-module calls is needed elsewhere, keep only that clause.
- `docs/tmt/stdlib.md`: the twins sentence ends "…it links, it is reachable or
  dropped, and a definition of your own under its name is a duplicate-symbol
  link error once the program calls it, exactly as for the routine it
  mirrors."; the "Symbol resolution" bullet becomes "The stdlib is appended
  *last*. A qualified name has one definition in a link: defining a routine
  under a `std::` name your program calls is a duplicate-symbol error naming
  your unit and the standard library. To replace the library, build with
  `--nostdlib` and link your own set (`docs/tmt/cli.md (--nostdlib)`)."
- `docs/pmt/project.md` and `docs/tmt/project.md`: "A name a declared library
  defines may not also be defined by a source, or by an earlier library, once
  the program reaches it (`docs/core.md (linking)`). Linking is lazy by
  reachability, so a declared library that no effective source references
  contributes nothing to the output."
- `docs/tmt/cli.md` `### --nostdlib`: replace "Libraries are first-wins, so
  command-line objects and explicit `-l` libraries shadow a stdlib definition
  of the same name." by "A command-line object or an explicit `-l` library
  defining a name the stdlib also defines is a duplicate-symbol error once the
  program reaches it; `--nostdlib` is the way to replace the library."
- `docs/lsp.md`: Resolution order — "…then the declared libraries, and the
  embedded standard library **last**. The order only decides what the editor
  shows when two of them define one name, which the linker refuses once the
  program reaches it (`docs/core.md (linking)`)." Replace "Because the
  standard library resolves last, a project that legitimately shadows it…"
  by "A project that defines a `std::` name of its own sees its own
  definition in the editor; the linker refuses the pair once the program
  reaches the name." Faithfulness sentence: "…compares them file-for-file,
  and pins that a reached sibling-versus-standard-library collision is a link
  error." The stdlib-section sentence "…shadows the embedded routine of the
  same mangled name, exactly as the linker resolves it" → "…is what the editor
  shows for that name; the linker refuses the pair once the program reaches
  it." Caveats: add "**Duplicate definitions are not reported yet.** A name a
  sibling and a library (or the standard library) both define resolves to
  the sibling here, while the linker refuses the program that reaches it."
- `docs/wasm.md`: "…so a user routine shadowing a `std::` name is `"user"`" →
  "…so every function reports the one input that defined it (a user routine
  may not share a `std::` name the program reaches — that link is refused)".
- `CLAUDE.md` Linker paragraph: "Libraries are first-wins and silently
  shadowed by user definitions." → "An exported name has one definition per
  link: user duplicates always error, a name a library also defines errors
  once reached." Editor integration bullet: keep the order sentence, append
  "; a reached duplicate is a link error the overlay does not report yet".


**Steps.**
- [ ] Write rows 1–7 and the rewrites; see 1, 2, 4, 5, 7 red (they link or
  build today); 3 and 6 green; the six tests 2a left red now pass.
- [ ] Comments and doc comments (driver, cli, wasm).
- [ ] Docs as above. Re-run Task 0's shipped-build sweep (every file that
  built must still build); record in the ledger.
- [ ] Mutations: `tmt link` without `-l` labels → 1 red; `tmt build` without
  the stdlib label → 4 red; PM driver without the stdlib label → 5 red.
- [ ] Gate: `cargo nextest run -p mtc-core -p mtc-post-machine -p mtc-turing-machine -j 4`
  once (PM byte identity inside); `cargo nextest run -p mtc-wasm -j 4` once.
- [ ] Commits: `test(cli): both toolchains refuse a reached name two inputs define`
  (the rewrites and the new CLI tests) and
  `docs(core): one definition per name at link, no shadowing of a library`
  (docs, comments, `CLAUDE.md`).

---

### Task 3: Conflicting graph and alphabet exports

**Property.** Two inputs exporting one graph or one alphabet with IDENTICAL
records are one export; with DIFFERENT records (a different digest, a
different glyph list) they are an error whenever some input consumes the name
(a recorded graft of the graph, a recorded import of the alphabet) — and
always when both differing exporters are user objects. The drift checks then
compare each consumer against the one export that remains; a name nobody
exports stays unchecked (header-only libraries).

**Exported and private graph records (ruled 2026-09-30).** A graph record
carries whether the graph is EXPORTED. Only exported records enter the
conflict pass: a PRIVATE record never conflicts with anything, between any
pair of inputs — a private graph is not a definition in the shared
namespace, exactly as a local symbol is neither exported nor shadowable.
Private records serve the DRIFT check only: a consumer's recorded graft of a
graph that no input EXPORTS must carry the digest of at least one input's
private record of that name (its provider may be any of them); a graph no
input records at all stays unchecked, as today. This task adds the flag and
the spelling (every existing `.graph` line is exported; nothing produces a
private record until Task 13).

**Files:**
- Modify: `crates/core/src/formats/object/mod.rs` (`ExportedGraph` gains
  `pub exported: bool`), `crates/core/src/formats/object/read.rs` and the
  writer beside it (the graph entry carries the flag — one byte after the
  digest, `1` exported, `0` private; the container stays at its unreleased
  revision 4), `crates/core/src/formats/object/tests.rs` (the round trip and
  the never-panics-on-noise property cover the new byte)
- Modify: `crates/core/src/asm/cst.rs`, `crates/core/src/asm/lower.rs`
  (`.graph <name>, <digest>` is exported; `.graph <name>, <digest>, private`
  is a private record; any other third operand is `syntax`),
  `crates/core/src/asm/disassembler.rs` (prints `, private` for a private
  record, so dis→asm stays byte-identical — the text-expressibility gate)
- Modify: every other `ExportedGraph { .. }` construction (`asm/lower.rs`
  tests, `disassembler.rs` tests, `formats/object/tests.rs`,
  `crates/turing-machine/src/header.rs` near `exported_graph_digests`, which
  sets `exported: true` until Task 13)
- Modify: `crates/core/src/linker/interface.rs` (`check_graft_drift`,
  `check_imported_alphabets` and their docs)
- Modify: `crates/core/src/linker/mod.rs` (new `LinkError::ConflictingExport`
  + `Display`)
- Modify: `docs/core.md` ("### Graft drift" section: the graph and imported
  alphabet paragraphs, "first-wins over the same namespace order"),
  `docs/formats.md` (the paragraph "At link, every recorded `.grafted` digest
  is checked… first-wins in the linker's own namespace order, exactly as a
  symbol resolves."; the `.graph` directive's grammar and the MO graph
  entry's layout gain the `private` flag), `docs/tmt/asm.md` (the `.graph`
  line's optional `, private`)
- Test: `crates/core/tests/link_checks.rs` (rewrite
  `a_user_export_shadows_a_library_export_for_the_check`, add six),
  `crates/turing-machine/tests/one_definition.rs`

**Interfaces:**
- Consumes (Task 1): `input_name`, the `names` parameter of both checks,
  `graph_trio`, `routine_trio`, fixtures `G1`, `G2`, `GAPP`, `R1`, `APP`.
- Produces:
  ```rust
  pub enum LinkError {
      /// Two inputs export one graph or one alphabet with different records
      /// — digests or glyph lists — while some input consumes the name, or
      /// while both are user objects (docs/core.md (graft drift)). `what` is
      /// `"graph"` or `"alphabet"`.
      ConflictingExport { what: &'static str, name: String, first: String, second: String },
      // …
  }
  // crates/core/src/formats/object/mod.rs
  pub struct ExportedGraph { pub name: String, pub digest: u32, pub exported: bool }
  ```
  Display: ``the {what} `{name}` is exported differently by {first} and by {second}``.

**Falsifying tests.**

TM fixture `R3` (add to `one_definition.rs`):
```rust
/// `lib::ab` with a third glyph, and a routine under another name, so a
/// link with `R1` contests only the alphabet export.
const R3: &str = "\
namespace lib {
  export alphabet ab { '_', 'a', 'b' }
  export routine q(tape t: ab) {
    entry state s { [*] -> write ['b'] return; }
  }
}
";
```

| # | Fixture | Command / API | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | graph trio **[verified]** | `tmt link gapp.tmo --nostdlib -L DIR -l g1 -l g2` | rc 0 (first-wins agrees) | Err ``the graph `lib::walk` is exported differently by library `g1` and by library `g2` `` | `one_definition.rs::two_libraries_exporting_a_grafted_graph_differently_are_refused` | keep first-wins exporter map (Ok again) |
| 2 | same | `… -l g2 -l g1` | ``tmt: `lib::walk` was grafted into input #0 from a header that does not match input #1`` | Err ``… exported differently by library `g2` and by library `g1` `` | `one_definition.rs::the_export_conflict_is_reported_before_drift` | run the drift check before the conflict pass |
| 3 | graph trio + `g1copy.tmo` = a byte copy of `g1.tmo` **[verified]** | `… -l g1 -l g1copy` | rc 0 | rc 0 | `one_definition.rs::identical_graph_exports_are_one_export` | compare exporters by input instead of by record |
| 4 | routine trio + graph trio **[verified]** | `tmt link app.tmo r1.tmo --nostdlib -L DIR -l g1 -l g2` (nothing grafts `lib::walk`) | rc 0 | rc 0 | `one_definition.rs::differing_library_graph_exports_nobody_grafts_link` | refuse every differing pair regardless of consumption |
| 5 | same objects, all as user objects | `tmt link app.tmo r1.tmo g1.tmo g2.tmo --nostdlib` | rc 0 | Err ``… exported differently by `` + g1 path + ` and by ` + g2 path | `one_definition.rs::two_user_objects_exporting_one_graph_differently_are_refused` | the "both user objects" arm missing |
| 6 | routine trio + `r3.tmo` **[verified]** | `tmt link app.tmo --nostdlib -L DIR -l r1 -l r3` | rc 0 | Err ``the alphabet `lib::ab` is exported differently by library `r1` and by library `r3` `` | `one_definition.rs::two_libraries_exporting_an_imported_alphabet_differently_are_refused` | alphabet map left first-wins |
| 7 | same | `… -l r3 -l r1` | ``tmt: `lib::ab` as imported by input #0 differs from input #1's own declaration, first at position 2`` | Err ``… exported differently by library `r3` and by library `r1` `` | (second half of test 6) | — |
| 8 | core: `consumer(42)` user; `exporter(42)` and `exporter_b(7)` libraries | `link(..Frames)` | Ok | `ConflictingExport { what: "graph", name: "lib::findA", first: "input #1", second: "input #2" }` | `link_checks.rs::two_library_graph_exports_that_differ_are_refused_when_grafted` | keep first-wins |
| 9 | core: rewritten `a_user_export_shadows_a_library_export_for_the_check` fixtures (user `consumer(42)` + `USER_EXPORTER` 42, library `exporter(7)`) | same | Ok | `ConflictingExport { what: "graph", name: "lib::findA", first: "input #1", second: "input #2" }` | `link_checks.rs::a_user_export_and_a_library_export_that_differ_are_refused_when_grafted` (rename) | user exports silently preferred |
| 10 | core: `exporter(42)` and `exporter_b(42)` libraries, `consumer(42)` | same | Ok | Ok | `link_checks.rs::identical_graph_exports_are_one_export` | per-input comparison |
| 11 | core: user `plain_main()` (no `.grafted`), libraries `exporter(42)`, `exporter_b(7)` | same | Ok | Ok | `link_checks.rs::differing_graph_exports_nothing_grafts_link` | consumption ignored |
| 12 | core: user objects `exporter(42)` and `exporter_b(7)` plus `plain_main()` | same | Ok | `ConflictingExport { what: "graph", .. }` | `link_checks.rs::two_user_objects_exporting_one_graph_differently_are_refused` | user arm missing |
| 13 | core: alphabet — `exporter(1)` and `exporter_b(1)` libraries with `lib::bits` pushed as `['_','0','1']` and `['_','1','0']`, `consumer(1)` importing `['_','0','1']` | same | Ok | `ConflictingExport { what: "alphabet", name: "lib::bits", .. }` | `link_checks.rs::two_library_alphabet_exports_that_differ_are_refused_when_imported` | alphabet map first-wins |
| 14 | core: `.graph lib::step, 7, private` | `assemble` then `disassemble_object` then `assemble` | does not assemble (a third `.graph` operand is unknown today) | assembles; `Interface::graphs == [ExportedGraph { name: "lib::step", digest: 7, exported: false }]`; the disassembly prints `.graph lib::step, 7, private` and reassembles byte-identically | `link_checks.rs::a_private_graph_record_round_trips_through_text` | the disassembler dropping `, private` (the reassembled record is exported) |
| 15 | core: user objects `plain_main()`, `private_exporter("a", 1)`, `private_exporter("b", 2)` (each `.graph lib::step, D, private` and its own routine) — nothing grafts `lib::step` | `link(..Frames)` | does not assemble today (row 14's spelling is new) | Ok | `link_checks.rs::differing_private_graphs_of_two_user_objects_link` | feed private records into `settle_exports` (the user-user arm refuses) |
| 16 | the near miss: the same two graphs EXPORTED (`exporter(1)` / `exporter_b(2)`-shaped, `.graph lib::step`) as user objects | same | Ok | `ConflictingExport { what: "graph", name: "lib::step", first: "input #1", second: "input #2" }` | `link_checks.rs::differing_exported_graphs_of_two_user_objects_are_refused` | exported records ignored too |
| 17 | core: consumer `.grafted lib::step, 2` + libraries `private_exporter("a", 1)`, `private_exporter("b", 2)` | same | — | Ok (a private record of the name matches) | `link_checks.rs::a_graft_of_a_private_graph_matches_any_provider` | compare only against the FIRST private record |
| 18 | core: consumer `.grafted lib::step, 3` + the same two libraries | same | — | `GraftDrift { graph: "lib::step", consumer: "input #0", library: "input #1" }` | `link_checks.rs::a_graft_of_a_private_graph_no_provider_matches_is_drift` | private records left unchecked |

```rust
/// A library-shaped object carrying a PRIVATE record of `lib::step` and a
/// routine `lib::<tag>`, so two of them contest no symbol.
fn private_exporter(tag: &str, digest: u32) -> String {
    format!(
        "\
.graph lib::step, {digest}, private
.routine lib::{tag}, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func lib::{tag}
        ret
"
    )
}
```

(Row 16's exported variant is the same text without `, private`. Rows 17
and 18: the consumer is `consumer(D)` with `.grafted lib::findA` replaced by
`.grafted lib::step` and its `call lib::facade` replaced by `stp`, so it
calls nothing the two objects define.)

Core helpers to add beside `exporter`/`consumer`:

```rust
/// `exporter`'s twin under another routine name, so two of them in one link
/// contest only the graph export, never a symbol.
fn exporter_b(digest: u32) -> String {
    format!(
        "\
.graph lib::findA, {digest}
.routine lib::other, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func lib::other
        ret
"
    )
}

/// An entry that grafts nothing and calls nothing.
fn plain_main() -> String {
    "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func main
        stp
"
    .to_string()
}
```

Test 8 body (the others follow the same shape, changing inputs and the
expected result):

```rust
/// Two libraries export `lib::findA` with different digests and the
/// consumer grafts it: the link cannot tell which body the consumer
/// spliced. Mutation it catches: keep the first-wins exporter map — the
/// consumer agrees with the first library and the link succeeds.
#[test]
fn two_library_graph_exports_that_differ_are_refused_when_grafted() {
    let err = link(
        &fake_syntax(),
        &[asm(&consumer(42))],
        &[asm(&exporter(42)), asm(&exporter_b(7))],
        opts(CallMech::Frames),
    )
    .expect_err("two differing exports of a grafted graph");
    assert_eq!(
        err,
        LinkError::ConflictingExport {
            what: "graph",
            name: "lib::findA".into(),
            first: "input #1".into(),
            second: "input #2".into(),
        }
    );
}
```

**Implementation (`interface.rs`).** One helper serves both record kinds:

```rust
/// The single export each name resolves to for the drift checks, or the
/// conflict that stops the link (docs/core.md (graft drift)). `exports`
/// lists every (input, name, record) in input order; `consumed` holds every
/// name some input consumes; `users` is how many inputs are user objects.
fn settle_exports<'a, R: PartialEq>(
    exports: Vec<(usize, &'a str, R)>,
    consumed: &HashSet<&str>,
    users: usize,
    what: &'static str,
    names: &[Option<String>],
) -> Result<HashMap<&'a str, (usize, R)>, LinkError>
```

Group `exports` by name in input order. For each name (sorted, for a
deterministic error): the first record is the export; find the first later
record that differs; if one does and (`consumed.contains(name)` or both inputs
are `< users`), return `ConflictingExport { what, name, first:
input_name(names, i_first), second: input_name(names, i_diff) }`; if one does
but neither holds, drop the name from the map (nothing to check against, the
same standing as a header-only library). `check_graft_drift` builds its
`exports` from `Interface::graphs` (record = `digest`), `consumed` from every
input's `ObjectFile::grafts`; `check_imported_alphabets` from
`Interface::alphabets` (record = `&Vec<String>`), `consumed` from every
`Interface::imports`. Each then runs its existing comparison against the
settled map. `check_graft_drift` feeds `settle_exports` the EXPORTED graph
records only, and keeps the private ones in a second map
`private: HashMap<&str, Vec<(usize, u32)>>` (every input's private record,
in input order). For each consumer graft: an entry in the settled (exported)
map is compared as today; otherwise, if `private` holds the name, the graft
passes when any listed digest equals its own and is
`GraftDrift { graph, consumer, library: input_name(names, first private recorder) }`
when none does; otherwise it is unchecked. Rewrite both doc comments: drop
"first-wins… shadows a library exactly as it shadows a symbol". The "both user objects" arm is tested for
graphs only (rows 5 and 12): both record kinds go through the one
`settle_exports`, so an alphabet row would test the same branch twice.

**Docs.** `docs/core.md` (graft drift): "The exporter is the one input that
exports the graph; two inputs exporting it with identical digests count as
one. Two inputs exporting it with DIFFERENT digests are a link error when any
input grafts it, and always when both are user objects — the link could not
say which body the consumer spliced." Same for the imported-alphabet
paragraph ("first-wins over the same namespace order" → "against the one
export, two identical exports counting as one; differing exports of an
imported alphabet are the same link error"). `docs/formats.md`: "…checked
against the `.graph` digest of the input that exports it, when one is
present in the link; identical exports from several inputs count as one, and
differing ones are a link error when the graph is grafted
(`docs/core.md (graft drift)`)." Both pages, one more paragraph: "A
`.graph` line may be marked `private`: the unit prints that graph in its
header without exporting it. A private record never conflicts with another
input's record of the same name; it only lets the link check a consumer that
spliced the graph, which must match at least one input's private record when
no input exports the name."

**Steps.**
- [ ] Tests 1–18 red where the table says the result changes (14–18 fail
  to assemble until the spelling exists).
- [ ] Implement the flag (format, assembler, disassembler, constructions),
  then `settle_exports`, the private drift arm, and the two callers.
- [ ] Focused green; mutations (first-wins → 1, 6, 8 red; consumption ignored
  → 4, 11 red; user arm dropped → 5, 12 red; drift first → 2 red; private
  records in the conflict pass → 15 red; first-private-only → 17 red;
  private records unchecked → 18 red; `, private` not printed → 14 red).
- [ ] Core format guards: the object round-trip and noise property tests in
  `formats/object/tests.rs` green with the new byte; the dis→asm
  byte-identity tests green (`graph_and_grafted_lines_reassemble_byte_identically`
  in the TM crate among them).
- [ ] Docs.
- [ ] Gate: `cargo nextest run -p mtc-core -p mtc-post-machine -p mtc-turing-machine -j 4` once.
- [ ] Commit: `feat(core): two inputs exporting one graph or alphabet differently are refused`.

---

### Task 4: A source read publishes what its header publishes, and the published form

**Property.** Reading a `.tmc` for its declarations (a sibling in `tmt build`,
an `--extern` `.tmc`) believes exactly what reading its generated header
believes: for every unit the repository ships, the two readings have equal
published forms. In particular an uncontracted tape publishes its INFERRED
write set on both routes, never the whole alphabet on one of them.

This task also introduces the *published form* — the comparison Task 5's
conflict checks use — because the guard needs it first.

**Files:**
- Create: `crates/turing-machine/src/published.rs` (module doc, types,
  `module_forms`, the per-item form functions, the corpus guard as a
  `#[cfg(test)]` module)
- Modify: `crates/turing-machine/src/lib.rs` (`pub(crate) mod published;`)
- Modify: `crates/turing-machine/src/compiler.rs` (`ResolvedAlphabet` gains
  `pub exported: bool`; its two constructions: `resolve_all_alphabets` sets
  `exported: a.exported`, the external copy in `resolve_tape_alphabet` sets
  `exported: false`; `Resolved` gains `pub from_object: bool`, `false` at
  its construction in `resolve_module` and in `header::empty_resolved`)
- Modify: `crates/turing-machine/src/header.rs` (extract `settle_declarations`
  from `read_declarations_with_mode`; `map_pair_list_text` becomes
  `pub(crate)`; `empty_resolved` sets `from_object: false`;
  `declarations_from_object` sets `from_object = true` on the module it
  returns)
- Modify: `docs/tmt/language.md` ("### Declarations": the write-set
  sentences), `docs/tmt/cli.md` (`### --extern and --nostdlib`: "a routine's
  body contributes only its `noreturn`"), `docs/tmt/project.md`
  ("### Declaration derivation": "its body contributes one fact only")
- Test: `crates/turing-machine/tests/extern_declarations.rs`,
  `crates/turing-machine/src/published.rs` (guard)

**Interfaces:**
- Consumes: nothing from Tasks 1–3.
- Produces (used by Tasks 5, 12, 13):
  ```rust
  // crates/turing-machine/src/published.rs
  /// A declaration as another unit sees it (docs/tmt/language.md
  /// (declarations)): what two readings of one name must agree on.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub(crate) enum PublishedForm {
      Alphabet { glyphs: Vec<String> },
      Set { members: Vec<String> },
      /// Source and target alphabets as glyph lists, pairs as the header
      /// prints them.
      Map { src: Vec<String>, dst: Vec<String>, pairs: String },
      Routine { params: Vec<ParamForm>, noreturn: bool },
      Graph { digest: Option<u32> },
  }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub(crate) enum ParamForm {
      Tape {
          name: String,
          glyphs: Vec<String>,
          writes: Vec<String>,
          enters: Option<Vec<String>>,
          leaves: Option<Vec<String>>,
      },
      /// `None` for a reading taken from a compiled object, whose
      /// interface carries no state-parameter names (the object arm of
      /// `tmt interface` mints `exit0`, `exit1`, … for them).
      State { name: Option<String> },
  }
  impl PublishedForm {
      /// THE comparison of two published forms — the only one the corpus
      /// guard, `Declarations::check_conflicts` and
      /// `check_own_against_table` use. Equality, except that two state
      /// parameters agree by position alone when either name is `None`
      /// (a routine's parameter lists must still have the same length and
      /// agree element by element). Never `==` between forms of two
      /// readings.
      pub(crate) fn agrees(&self, other: &Self) -> bool;
  }
  /// Every EXPORTED alphabet, set, map, routine and graph `module` declares,
  /// keyed by qualified name. A module with `from_object` set yields
  /// `State { name: None }` for every state parameter.
  pub(crate) fn module_forms(module: &Resolved) -> BTreeMap<String, PublishedForm>;
  pub(crate) fn alphabet_form(a: &ResolvedAlphabet) -> PublishedForm;
  pub(crate) fn set_form(s: &ResolvedSet) -> PublishedForm;
  pub(crate) fn map_form(m: &ResolvedMapDecl, module: &Resolved) -> PublishedForm;
  pub(crate) fn routine_form(w: &ResolvedWorld, module: &Resolved) -> PublishedForm;
  pub(crate) fn graph_form(w: &ResolvedWorld) -> PublishedForm;
  // crates/turing-machine/src/header.rs
  /// Settle what a declarations reading of `program` publishes
  /// (docs/tmt/language.md (declarations)): stamp every graph's digest,
  /// settle every routine's `noreturn` from its body, and — for a
  /// `ReadMode::Program` read only — publish each routine/graph tape's
  /// write set as the header prints it. The ONE place both the lenient
  /// read and the unit's own-definition check settle these facts.
  pub(crate) fn settle_declarations(program: &Program, resolved: &mut Resolved, externals: &Declarations, mode: ReadMode);
  pub(crate) fn map_pair_list_text(pairs: &[MapPair]) -> String; // visibility only
  // crates/turing-machine/src/compiler.rs
  pub(crate) struct ResolvedAlphabet { pub name: String, pub name_span: Span, pub glyphs: Vec<String>, pub exported: bool }
  pub(crate) struct Resolved { /* existing fields */ pub from_object: bool }
  ```

  `agrees`, written out:
  ```rust
  impl PublishedForm {
      pub(crate) fn agrees(&self, other: &Self) -> bool {
          match (self, other) {
              (
                  PublishedForm::Routine { params: a, noreturn: na },
                  PublishedForm::Routine { params: b, noreturn: nb },
              ) => na == nb && a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.agrees(y)),
              _ => self == other,
          }
      }
  }
  impl ParamForm {
      fn agrees(&self, other: &Self) -> bool {
          match (self, other) {
              (ParamForm::State { name: Some(a) }, ParamForm::State { name: Some(b) }) => a == b,
              (ParamForm::State { .. }, ParamForm::State { .. }) => true,
              _ => self == other,
          }
      }
  }
  ```

**Decided in the plan — what the published form covers.**
- A tape parameter is compared by its **glyph list**, never its alphabet's
  name: the object arm synthesizes alphabet names (`header.rs` near
  `fn resolve_object_alphabet`), and a call binds by index anyway.
- A routine's **state parameter names are part of the form when both
  readings carry real names** (a caller binds them by name). The object arm
  mints `exit0…` for them, so an object-derived reading carries NO names:
  `ParamForm::State { name: Option<String> }` (`None` from the object arm),
  and two forms compare state parameters by COUNT where either side is
  `None`. Ruled by the controller on 2026-09-30, so that a unit read once
  from its object and once from its source or header in one compile does
  not conflict.
- `volatile` and `opaque` are not published and not compared; `never writes`
  is folded into `writes` (the effective set); doc lines and comments are not
  compared; `noreturn` compares as a fact (`declared_noreturn.is_some()`),
  never by span.
- Only EXPORTED declarations of a module are its published forms. Alphabets
  copied into a module's `alphabets` from other modules carry
  `exported: false` and are therefore never published twice.

**Falsifying tests.**

| # | Fixture | Command / API | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | `U` and `UC` below **[verified]** | `tmt compile uc.tmc --nostdlib --extern u.tmc` | ``uc.tmc:3:11: error: `c` may write '_', 'y' on tape `t`, which its contract forbids [writes-outside-contract]``, rc 1 | rc 0 (only the `unused-routine` warning, exactly as through `u.tmh`) | `extern_declarations.rs::a_source_read_believes_the_write_set_its_header_prints` | skip the writes step of `settle_declarations` |
| 2 | same, the header route | `tmt compile uc.tmc --nostdlib --extern u.tmh` (u.tmh = `tmt interface u.tmc --nostdlib`) | rc 0 | rc 0 | (same test, control half) | — |
| 3 | `U` and `UC_NONE` below (caller promises `writes {}`) **[verified]** | both routes | `.tmh`: ``uc_none.tmc:3:11: error: `c` may write 'x' on tape `t`…``; `.tmc`: ``… may write '_', 'x', 'y' …``; both rc 1 | both refuse, both naming `'x'` only | `extern_declarations.rs::a_source_read_still_refuses_what_the_header_refuses` | publish the EMPTY set for an uncontracted tape (the strict-read hazard) |
| 4 | every shipped `.tmc` | in-crate: `module_forms(read_extern(x.tmc)) == module_forms(read_extern(x.tmh))` | fails on `tests/golden/a5_call_across_alphabets.tmc`'s uncontracted `mylib::plusOne` (whole alphabet vs `{'0','1'}`) | agree for every unit; ≥ 14 units read; ≥ 40 forms compared, AND the count equals the number of exported declarations counted a second way (from the source's `Program`) | `published::tests::every_shipped_unit_publishes_the_same_forms_from_source_and_header` | skip the writes step (a5 differs); an empty `module_forms` (the second count disagrees) |
| 5 | unit: two modules declaring `lib::r(tape t: ab)` over alphabets named `ab` and `other` with equal glyphs | `routine_form` of each | — | equal | `published::tests::a_tape_is_compared_by_its_glyphs_not_its_alphabet_name` | compare `tape.alphabet` |
| 6 | unit: a module whose `lib::marks` alphabet was copied in from another module (`exported: false`) | `module_forms` | — | no `lib::marks` key | `published::tests::an_alphabet_copied_from_another_module_is_not_published` | select alphabets without the `exported` flag |
| 7 | unit: `Routine` forms `[Tape t, State done]` against `[Tape t, State None]`; `[State done]` against `[State fin]`; `[State done]` against `[State None, State None]` | `agrees` | — | true; false; false | `published::tests::state_parameters_agree_by_name_or_by_position_when_one_is_unnamed` | derive the comparison (`==`): the first pair disagrees; or ignore length: the third agrees |

```rust
// crates/turing-machine/tests/extern_declarations.rs — fixtures
const U: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  export routine h2(tape t: marks) {
    entry state s { [*] -> write ['x'] return; }
  }
}
";

const UC: &str = "\
use lib::marks;
use lib::h2;
routine c(tape t: marks writes { 'x' }) {
  entry state s { [*] -> call h2(t = t) then done; }
  state done { [*] -> return; }
}
";

const UC_NONE: &str = "\
use lib::marks;
use lib::h2;
routine c(tape t: marks writes {}) {
  entry state s { [*] -> call h2(t = t) then done; }
  state done { [*] -> return; }
}
";

/// A `.tmc` read for its declarations believes the inferred write set its
/// generated header prints, so a caller whose contract matches that set
/// compiles through either route. Mutation: skip the writes step of
/// `header::settle_declarations` — the `.tmc` route believes the whole
/// alphabet again and fails `writes-outside-contract`.
#[test]
fn a_source_read_believes_the_write_set_its_header_prints() {
    let dir = scratch("source_read_writes");
    let u = write(&dir, "u.tmc", U);
    let header = execute(&[
        "interface".into(),
        u.to_str().unwrap().into(),
        "--nostdlib".into(),
    ])
    .expect("interface");
    let u_tmh = write(&dir, "u.tmh", &header.stdout);
    let uc = write(&dir, "uc.tmc", UC);
    for route in [&u_tmh, &u] {
        let out = compile(&dir, &uc, &["--nostdlib", "--extern", route.to_str().unwrap()]);
        assert!(out.is_ok(), "{}: {:?}", route.display(), out.err());
    }
}

/// The near miss: a caller promising to write nothing is refused through
/// both routes. Mutation: publish the EMPTY set for an uncontracted tape —
/// the `.tmc` route then accepts it.
#[test]
fn a_source_read_still_refuses_what_the_header_refuses() {
    let dir = scratch("source_read_refuses");
    let u = write(&dir, "u.tmc", U);
    let header = execute(&[
        "interface".into(),
        u.to_str().unwrap().into(),
        "--nostdlib".into(),
    ])
    .expect("interface");
    let u_tmh = write(&dir, "u.tmh", &header.stdout);
    let uc = write(&dir, "uc.tmc", UC_NONE);
    for route in [&u_tmh, &u] {
        let err = compile(&dir, &uc, &["--nostdlib", "--extern", route.to_str().unwrap()])
            .unwrap_err();
        assert!(err.contains("[writes-outside-contract]"), "{}: {err}", route.display());
    }
}
```

The corpus guard (in `published.rs`'s `#[cfg(test)] mod tests`):

```rust
/// For every unit the repository ships, reading its source for its
/// declarations and reading its generated header yield equal published
/// forms — the precondition of the one-declaration rule, which would
/// otherwise refuse a unit compiled beside its own up-to-date header.
/// Mutation: skip the writes step of `header::settle_declarations` —
/// `tests/golden/a5_call_across_alphabets.tmc`'s uncontracted
/// `mylib::plusOne` then differs (whole alphabet against its inferred set).
#[test]
fn every_shipped_unit_publishes_the_same_forms_from_source_and_header() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    for dir in ["tests/golden", "src/stdlib", "../../docs/examples"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let paths: Vec<_> = if path.is_dir() {
                std::fs::read_dir(&path).unwrap().flatten().map(|e| e.path()).collect()
            } else {
                vec![path]
            };
            sources.extend(paths.into_iter().filter(|p| p.extension().is_some_and(|e| e == "tmc")));
        }
    }
    sources.sort();
    let (mut units, mut forms) = (0usize, 0usize);
    for path in &sources {
        let text = std::fs::read_to_string(path).unwrap();
        // The embedded stdlib's own source reads against nothing; every
        // other unit against the embedded stdlib, as `tmt interface` does.
        let context = if path.ends_with("src/stdlib/std.tmc") {
            Declarations::none()
        } else {
            Declarations::stdlib()
        };
        let Ok(header) = crate::header::from_source(&text, &context) else { continue };
        let from_source = crate::header::read_extern(std::path::Path::new("unit.tmc"), &text, &context)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let from_header = crate::header::read_extern(std::path::Path::new("unit.tmh"), &header, &context)
            .unwrap_or_else(|e| panic!("{}: header: {e}", path.display()));
        // Both readings are TEXT, so every state name is `Some` and
        // `agrees` is plain equality here; it is used anyway, as the one
        // comparison.
        let (a, b) = (module_forms(&from_source), module_forms(&from_header));
        assert_eq!(
            a.keys().collect::<Vec<_>>(),
            b.keys().collect::<Vec<_>>(),
            "{}: the two readings publish different names",
            path.display()
        );
        for (name, form) in &a {
            assert!(
                form.agrees(&b[name]),
                "{}: `{name}` differs between the source and its header:\n{form:?}\n{:?}",
                path.display(),
                b[name]
            );
        }
        // A second, independent count of what the unit exports, from its
        // AST — so an empty or truncated `module_forms` cannot pass.
        let program = crate::compiler::analyze_with(&text, &context)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .program;
        let exported = program.alphabets.iter().filter(|x| x.exported).count()
            + program.sets.iter().filter(|x| x.exported).count()
            + program.maps.iter().filter(|x| x.exported).count()
            + program.routines.iter().filter(|x| x.exported).count()
            + program.graphs.iter().filter(|x| x.exported).count();
        assert_eq!(a.len(), exported, "{}: published forms against exported declarations", path.display());
        units += 1;
        forms += a.len();
    }
    assert!(units >= 14, "only {units} shipped units were read");
    // The corpus exports 48 declarations today (std 42, named_map_foreign_alphabets 4,
    // a5 1, a6 1); the floor leaves room for small corpus edits.
    assert!(forms >= 40, "only {forms} forms were compared");
}
```

(If `header::from_source` or `read_extern` has a different visibility or
error type than `pub(crate) … -> Result<_, CompileError>`, adapt the call, not
the property. Check the AST field names — `exported` on `Alphabet`,
`SetDecl`, `MapDecl`, `Routine`, `Graph` — against `parser.rs`. Record the
actual `units`/`forms` numbers in the task report.)

**Known limit (record, do not fix).** The fixpoint never re-reads a source
once it has read clean. A sibling A read BEFORE its peer B believes B's
routines write their whole alphabet and publishes that wider inferred set,
while A's header — printed with B available — publishes the narrow one. Two
readings of one unit in one table can therefore disagree depending on
fixpoint order. This needs a unit given twice (as a source and as its
header) with an unread peer dependency, and is left as a recorded limit in
the ledger.

**Implementation.**
1. `ResolvedAlphabet::exported` as in Interfaces.
2. `header.rs`: move the body of `read_declarations_with_mode` after
   `analyze_with_mode` into `settle_declarations`, in this order:
   (a) `footprint = footprint::infer_resolved_with(resolved, &externals.modules())`;
   (b) `returns = inferred_returns(program, resolved, externals)`;
   (c) the existing `noreturn` loop over routine worlds;
   (d) `stamp_graph_digests(program, resolved, &footprint)` — BEFORE (e), so
   the digest is computed from the tapes as the source declared them (the
   printed signature goes through `published_writes` either way, so the order
   cannot change a digest; stamping first makes that independent of (e));
   (e) only when `mode == ReadMode::Program`: for every `Routine` and `Graph`
   world, for each tape at index `i`:
   ```rust
   let inferred = footprint.worlds.get(&world.name).and_then(|wf| wf.tapes.get(i).copied());
   tape.writes = Some(published_writes(tape, inferred));
   tape.never_writes = None;
   ```
   Never for `DeclarationsOnly`: a bodiless routine's inferred set is empty,
   and publishing it would claim the routine writes nothing.
   `read_declarations_with_mode` becomes `analyze_with_mode` + `settle_declarations`.
   `declarations_from_object` sets `from_object = true` on the module it
   returns (the field is `false` everywhere else).
3. `published.rs`: the types; `module_forms` iterates `alphabets` /
   `sets` / `maps` filtered by `exported`, and `worlds` filtered by
   `exported && kind != Machine`; `routine_form` zips `param_kinds` with
   `tapes`/`state_params` (the order `compiler::SigInfo::declared` uses), a
   tape's glyphs from `module.alphabets.get(&tape.alphabet)` — **never an
   index expression**: this runs inside `resolve_program` on every compile,
   the editor's included, and a missing entry must not panic; when it is
   missing, use `vec![tape.alphabet.clone()]` as the glyph list, so the form
   stays defined and compares by name in that one case (the same `.get`
   rule applies to `map_form`'s two alphabets) — `writes =
   symset_glyphs(declared_effective(tape), glyphs)`, `enters`/`leaves =
   clause_glyphs(tape.enters|leaves, glyphs)`, `noreturn =
   w.declared_noreturn.is_some()`, and each state parameter
   `State { name: (!module.from_object).then(|| name.clone()) }`;
   `graph_form` is `Graph { digest: w.digest }`; `agrees` as written in
   Interfaces;
   `map_form` takes both alphabets' glyph lists from `module.alphabets` and
   `pairs = header::map_pair_list_text(&m.pairs)`; `set_form` is the
   expanded `glyphs` in order. Module doc cites `docs/tmt/language.md
   (declarations)`.
4. Docs:
   - `docs/tmt/language.md` "### Declarations": replace "The write set is a
     DECLARED fact here, never an inferred one: a routine that declares no
     `writes` clause is read as able to write its whole alphabet." with "A
     header states each tape's write set. A `.tmc` read for its declarations
     publishes the same set its header would print: the declared effective
     set where the tape declares `writes` or `never writes`, and otherwise
     the set its body is inferred to write — so the two readings of one unit
     never disagree." In the `.tmc` bullet, "A routine's body contributes its
     inferred `noreturn` and nothing else." → "A routine's body contributes
     its inferred `noreturn` and its uncontracted tapes' inferred write sets."
   - `docs/tmt/cli.md` `--extern`: "a routine's body contributes only its
     `noreturn`, inferred exactly as `tmt interface` infers it" → "a routine's
     body contributes its `noreturn` and its uncontracted tapes' write sets,
     inferred exactly as `tmt interface` infers them".
   - `docs/tmt/project.md` Declaration derivation, the lenient-read paragraph:
     "its body contributes one fact only — its `noreturn`…" → "its body
     contributes what its generated header would print — its `noreturn` and
     its uncontracted tapes' inferred write sets…".

**Steps.**
- [ ] Tests 1–7 (1, 3 in `extern_declarations.rs`; 4–7 in `published.rs`).
  Run: test 1 red (the TODAY error), 4 red on a5, 5–7 red (functions missing).
- [ ] Implement 1–3; tests green.
- [ ] Mutations (skip (e) → 1 and 4 red; empty set for uncontracted → 3 red;
  `agrees` replaced by `==` → 7 red).
- [ ] Docs (4).
- [ ] Gate: `cargo nextest run -p mtc-turing-machine -j 4` once. Expected
  flips: none known. Any red test gets a one-line disposition in the report
  (a test that relied on a sibling `.tmc` being believed to write its whole
  alphabet is the only plausible kind). Compiled-stdlib byte identity at
  `-O0` and `-O1` (`tests/interface_emission.rs` stdlib object tests) must
  hold — this task changes no codegen path.
- [ ] Commit: `fix(turing-machine): a source read publishes the write sets its header prints`.

---

### Task 5: `conflicting-declaration` — one declaration per name in a compile

**Property.** In one compile a qualified name has one published form. Two
declaration modules that publish one name identically merge silently; with
different forms the compile fails `conflicting-declaration`, naming the name
and both origins. The same holds between the unit's OWN definition (of any
kind, exported or not) and a table declaration of that name — reported at the
unit's definition. Table order no longer decides anything.

**Where it surfaces (Established fact 7).** Every CLI path builds its table
through the fixpoint, so a conflict between two modules is found while the
later-read one is READ, as that module's own-definition conflict, and renders
against that module's file (e.g. `l2.tmh:3:18: … by this unit and by
\`l1.tmh\``). The compile-wide table-versus-table check (reported at
`FILE:0:0`, the existing precedent for compile-wide errors) is the backstop for
tables built through the in-crate API, and is tested there.

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs` (variant, registry row,
  `Display`, witness; two calls in `resolve_program`; new `Provenance`,
  threaded from `analyze_with_mode`/new `analyze_object_declarations` into
  `resolve_program`)
- Modify: `crates/turing-machine/src/header.rs` (`read_declarations_with_mode`
  gains a `Provenance` argument: `Text` from `read_extern`, `Object` from
  `declarations_from_object`)
- Modify: `crates/turing-machine/src/declarations.rs` (`Origin::describe`,
  `Declarations::entries`, `Declarations::check_conflicts`; the doc of
  `routine` and `origin_of` — `origin_of` loses `#[allow(dead_code)]` only if
  used; otherwise leave it)
- Modify: `crates/turing-machine/src/published.rs` (`own_forms`,
  `check_own_against_table`)
- Modify doc comments that promise first-wins: `footprint.rs` near
  `pub(crate) fn find_external` ("a user's own `--extern std.tmh` shadows the
  built-in `std`…"), `cli/build.rs` near `pub(super) fn read_externals`
  (same sentence), `header.rs` near `pub(crate) fn resolve_declarations` (the
  "peer's own declarations are about to shadow" sentence)
- Modify docs: `docs/tmt/cli.md` (compile-error table row; `--extern`
  paragraph: the order/shadow sentences), `docs/tmt/language.md` ("###
  Declarations": a new paragraph), `docs/tmt/project.md` ("### Declaration
  derivation": "Only the FINAL table's precedence is fixed…")
- Test: `crates/turing-machine/tests/extern_declarations.rs` (new tests;
  rewrite `extern_order_is_command_line_order_then_stdlib`),
  `crates/turing-machine/tests/build_derives_declarations.rs` (rewrite
  `a_peer_shadowing_a_stdlib_name_is_believed_over_the_builtin`),
  `crates/turing-machine/tests/library_grafts.rs` (the two
  `shadowed_nested_graft` tests; the four tests that collide with a library
  name — see "Rewrites"), `crates/turing-machine/src/published.rs`
  (unit tests)

**Interfaces:**
- Consumes (Task 4): `PublishedForm`, `PublishedForm::agrees`, `ParamForm`,
  `module_forms`, `alphabet_form`, `set_form`, `map_form`, `routine_form`,
  `graph_form`, `header::settle_declarations`, `ResolvedAlphabet::exported`,
  `Resolved::from_object`.
- Produces:
  ```rust
  // compiler.rs — declared immediately after `DuplicateName { .. }`
  /// Two declarations of one qualified name disagree (docs/tmt/language.md
  /// (declarations)). `first`/`second` describe the two origins — "this
  /// unit", a quoted path, ``library `NAME` ``, or "the standard library".
  ConflictingDeclaration { name: String, first: String, second: String },
  // registry row, immediately after the `DuplicateName { .. }` row:
  CompileErrorKind::ConflictingDeclaration { .. } => "conflicting-declaration",
  // declarations.rs
  impl Origin { pub(crate) fn describe(&self) -> String }
  impl Declarations {
      pub(crate) fn entries(&self) -> impl Iterator<Item = (&Origin, &Resolved)>;
      /// Two modules publishing one name differently (reported at 0:0).
      pub(crate) fn check_conflicts(&self) -> Result<(), CompileError>;
  }
  // published.rs
  /// The unit's own definitions — every alphabet, set, map, routine and
  /// graph `program` declares, exported or not — each with its name span
  /// and published form, settled exactly as a declarations reading of it
  /// would be (`header::settle_declarations`).
  /// With `Provenance::Object` every state parameter's name is `None`.
  pub(crate) fn own_forms(program: &Program, resolved: &Resolved, externals: &Declarations, mode: ReadMode, provenance: Provenance) -> BTreeMap<String, (Span, PublishedForm)>;
  pub(crate) fn check_own_against_table(program: &Program, resolved: &Resolved, externals: &Declarations, mode: ReadMode, provenance: Provenance) -> Result<(), CompileError>;
  // compiler.rs
  /// Where the text a reading parses came from: written, or printed from a
  /// source (`Text`), or rendered from a compiled object's interface
  /// (`Object`), whose state-parameter names are minted, not declared.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum Provenance { Text, Object }
  /// `analyze_with_mode` in `ReadMode::DeclarationsOnly` for an
  /// object-rendered header — the one caller that passes `Provenance::Object`.
  pub(crate) fn analyze_object_declarations(source: &str, externals: &Declarations) -> Result<Analysis, CompileError>;
  fn resolve_program(program: &Program, externals: &Declarations, mode: ReadMode, provenance: Provenance) -> Result<(Resolved, Vec<Diagnostic>), CompileError>;
  ```
  `Display`: ``"`{name}` is declared differently by {first} and by {second}"``.
  `Origin::describe`: `Stdlib` → `the standard library`; `Extern(p)` and
  `Sibling(p)` → `` format!("`{}`", p.display()) ``; `Library(n)` →
  `` format!("library `{n}`") ``. The own-definition side is the literal
  `this unit`.

**Implementation.**
1. `check_conflicts`: return `Ok(())` when there are fewer than two modules
   (the editor's per-keystroke path has one). Otherwise walk `entries()` in
   table order, `module_forms` of each; keep the first `(origin, form)` per
   name; the first later module whose form does not `agrees` with it →
   `Err(CompileError { span: Span::point(0, 0), kind: ConflictingDeclaration {
   name, first: first_origin.describe(), second: origin.describe() } })`.
   Names within a module are visited in `BTreeMap` order.
2. `check_own_against_table`: collect the unit's own qualified names from
   `program.alphabets`, `program.sets`, `program.maps`, `program.routines`,
   `program.graphs` (`full_name(ns, name)`). **Cheap gate first:** if no
   module declares any of them under any kind (a map/set/alphabet key or a
   world name), return `Ok(())` — this keeps the editor's keystroke path free
   of the settle cost. Otherwise compute `own_forms` (clone `resolved`, set
   `clone.from_object = provenance == Provenance::Object`, run
   `header::settle_declarations(program, &mut clone, externals, mode)`, then
   the per-item form functions on the clone, span from each item's
   `name_span`) and, for each own name in sorted order that some module
   publishes, compare with the FIRST publishing module's form by `agrees`
   (a later disagreeing module is `check_conflicts`' business); a
   disagreement →
   `Err(CompileError { span: own_span, kind: ConflictingDeclaration { name,
   first: "this unit".into(), second: origin.describe() } })`.
3. `resolve_program`: `externals.check_conflicts()?;` as its first line, and
   `crate::published::check_own_against_table(program, &resolved, externals, mode, provenance)?;`
   right after `check_head_contracts(&resolved)?`. `analyze_with_mode` and
   `analyze_staged_with` pass `Provenance::Text`; `analyze_object_declarations`
   passes `Provenance::Object`; `header::declarations_from_object` reads
   through it (via `read_declarations_with_mode(…, Provenance::Object)`),
   and `read_extern` through `Provenance::Text`.
3b. **The published boundary, by design.** The own-definition check compares
   a unit's names only against what a module PUBLISHES — its exported
   declarations. A header also PRINTS non-exported alphabets, sets, maps and
   graphs an exported body needs, but does not publish them, so a unit
   defining its own `lib::marks` beside a library whose `lib::marks` is
   printed-but-private stays silent. The linker never sees such a name
   either; it is a boundary of the rule, not a gap, and the language page
   says so (Docs).
4. The registry row, the `Display` arm, the witness
   `CompileErrorKind::ConflictingDeclaration { name: "x".into(), first: "a".into(), second: "b".into() }`
   immediately after the `DuplicateName` witness in
   `error_codes_are_pairwise_distinct_and_complete`. Do NOT add the code to
   `cli/build.rs`'s `DECLARATIONS_MISSING_CODES`: it is a defect, not a
   missing declaration.

**Falsifying tests.**

| # | Fixture | Command / API | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | `L1_TMH`, `L2_TMH`, `C8` below **[verified]** | `tmt compile c8.tmc --nostdlib --extern l1.tmh --extern l2.tmh` | rc 0 (l1 wins) | Err containing `l2.tmh:3:18: error:` **[computed]**, ``is declared differently by this unit and by `` + l1 path, `[conflicting-declaration]` | `extern_declarations.rs::two_headers_declaring_one_routine_differently_conflict` | drop `check_own_against_table` |
| 2 | same, reversed | `… --extern l2.tmh --extern l1.tmh` | ``c8.tmc:5:19: error: missing binding argument for parameter `done` [missing-arg]`` | Err at `l1.tmh:3:18:` **[computed]** naming the l2 path, `[conflicting-declaration]` | (second half of test 1) | — |
| 3 | `L1_TMH` twice (same path given twice) and `L1_TMH` + a byte copy `l1copy.tmh` | `… --extern l1.tmh --extern l1copy.tmh` | rc 0 | rc 0 | `extern_declarations.rs::one_unit_declared_twice_merges` | compare by origin instead of by form |
| 4 | `OWN` below (defines `lib::r` with an extra `state done`) **[verified]** | `tmt compile own.tmc --nostdlib --extern l1.tmh` | rc 0 (local wins) | Err at `own.tmc:3:18:` **[computed]** ``… by this unit and by `` + l1 path | `extern_declarations.rs::a_unit_defining_a_name_its_table_declares_differently_is_refused` | skip the `check_own_against_table` call in `resolve_program` |
| 5 | `OWN` beside its own header (`tmt interface own.tmc --nostdlib > own.tmh`) **[verified]** | `tmt compile own.tmc --nostdlib --extern own.tmh` | rc 0 | rc 0 | `extern_declarations.rs::a_unit_beside_its_own_header_compiles` | own form computed without `settle_declarations` — `own.tmh` prints `r` as `noreturn` **[verified]**, the unsettled own form says it may return |
| 6 | `OWN` beside a STALE header: `own_stale.tmh` = `l1.tmh` content | `… --extern own_stale.tmh` | rc 0 | Err at `own.tmc:3:18:` `[conflicting-declaration]` | (second half of test 5) | — |
| 7 | `OWNSTD` below (`std::binaryNumbers::goToNumber` with an extra `state done`) **[verified]** | `tmt compile ownstd.tmc -o x.tmo` (stdlib on) | rc 0 | Err at `ownstd.tmc:3:20:` **[computed]** ``… by this unit and by the standard library [conflicting-declaration]`` | `extern_declarations.rs::a_unit_redefining_a_std_routine_differently_is_refused` | stdlib excluded from the own-vs-table table |
| 8 | Task 2b's `OWNSAME` (identical signature) | `tmt compile ownsame.tmc -o x.tmo` | rc 0 | rc 0 (the LINK refuses it — Task 2b test 4) | `extern_declarations.rs::a_unit_redefining_a_std_routine_identically_compiles` | refuse every own name the table declares, without comparing forms |
| 9 | `NLIB`, `CONS`, `FAKESTD` below **[verified]**, stdlib on | `tmt compile cons.tmc --extern nlib.tmc --extern fakestd.tmc` | ``cons.tmc:3:53: error: missing binding argument for parameter `extra` [missing-arg]`` | Err containing `fakestd.tmc:4:18:` **[computed]**, `` `std::binaryNumbers::goToNumberGraph` is declared differently by this unit and by the standard library ``, `[conflicting-declaration]` | `library_grafts.rs::a_user_std_graph_beside_the_embedded_one_conflicts` | stdlib excluded |
| 10 | in-crate: `Declarations` with `read_extern("l1.tmh")` and `read_extern("l2.tmh")` pushed as `Origin::Extern("l1.tmh")`, `Origin::Extern("l2.tmh")` (each read against `Declarations::none()`), source `C8` | `compiler::analyze_with(C8, &decls)` | Ok | `Err(CompileError { span: Span::point(0, 0), kind: ConflictingDeclaration { name: "lib::r", first: "`l1.tmh`", second: "`l2.tmh`" } })` | `published::tests::two_table_modules_publishing_one_name_differently_conflict` | drop `check_conflicts` |
| 11 | in-crate: the same with l1 twice | same | Ok | Ok | `published::tests::two_table_modules_publishing_one_name_identically_merge` | per-origin comparison |
| 12 | in-crate: own alphabet `lib::r { '_', 'a' }` vs table routine `lib::r` | `analyze_with` | Ok | `ConflictingDeclaration` at the alphabet's name | `published::tests::an_own_name_of_another_kind_conflicts` | compare only same-kind forms |
| 13 | in-crate: `OWN`'s compiled object read by `header::declarations_from_object` (names `state exit0`) and `read_extern("own.tmc", OWN)` (names `state done`) in one table — probed: `tmt interface own.tmo` and `own.tmh` differ ONLY in `state exit0` vs `state done` | `analyze_with(C_OWN, &table)` | Ok (first wins) | Ok — they merge | `published::tests::an_object_reading_and_a_source_reading_of_one_unit_merge` | compare by derived `==` (conflicts on `done` vs `exit0`) |
| 14 | in-crate: `declarations_from_object(&own_object, &table_holding_own_tmc)` — the object read's OWN-definition check against a text reading of the same unit | the call | Ok | Ok | `published::tests::an_object_read_beside_its_source_reading_is_not_a_conflict` | `own_forms` ignoring `Provenance` (the object read's own names are then `Some("exit0")`) |
| 15 | in-crate: `OWN` and `OWN_FIN` (the same unit with its state parameter named `fin`) — two TEXT readings | `analyze_with(C_OWN_FIN, &table)` | Ok | `Err` `ConflictingDeclaration { name: "lib::r", first: "`own.tmc`", second: "`own_fin.tmc`" }` at 0:0 | `published::tests::two_text_readings_differing_in_a_state_name_conflict` | treat every state name as optional (count only) |
| 16 | **GUARD** (green when Task 5 starts; its red-making mutation belongs to Task 4 — skipping `settle_declarations`' writes step, Task 4 rows 1 and 4) — Task 4's `U`, `UC`: a unit's SOURCE and its HEADER given to one compile | `tmt compile uc.tmc --nostdlib --extern u.tmc --extern u.tmh` | ``uc.tmc:3:11: error: `c` may write '_', 'y' on tape `t`, which its contract forbids [writes-outside-contract]``, rc 1 **[verified]** (the source reading comes first and believes the whole alphabet) | rc 0 (one declaration seen twice) | `extern_declarations.rs::a_units_source_and_its_header_together_merge` | Task 4's mutation (skip the writes step): the source reading then publishes the whole alphabet and conflicts with the header — apply it here once to see this guard red |
| 17 | in-crate: `L1` and `L1` with a `? doc` line above `export routine r` | `analyze_with(C8, &table)` | Ok | Ok | `published::tests::a_doc_line_is_not_part_of_a_declaration` | put doc lines into the form |

```rust
// crates/turing-machine/tests/extern_declarations.rs — fixtures
const L1_TMH: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab writes { 'a' });
}
";

const L2_TMH: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab writes { 'a' }, state done);
}
";

const C8: &str = "\
use lib::ab;
use lib::r;
machine {
  tape t: ab;
  entry state s { [*] -> call r(t = t) then stop; }
}
";

const OWN: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab writes { 'a' }, state done) {
    entry state s { [*] -> goto done; }
  }
}
machine {
  tape t: lib::ab;
  entry state s { [*] -> call lib::r(t = t, done = stop) then stop; }
}
";

const OWNSTD: &str = "\
namespace std {
  namespace binaryNumbers {
    export routine goToNumber(tape num: std::binaryNumbers::symbols, state done) {
      entry state s { [*] -> goto done; }
    }
  }
}
machine {
  tape num: std::binaryNumbers::symbols;
  entry state s { [*] -> call std::binaryNumbers::goToNumber(num = num, done = stop) then stop; }
}
";
```

Test bodies (`extern_declarations.rs` already has `scratch`, `write`,
`compile(dir, input, flags)` and `execute`):

```rust
/// Two headers declaring `lib::r` with different signatures: the one read
/// second is refused against the first, in either order. Mutation: skip
/// the `check_own_against_table` call — the compile returns to first-wins
/// (Ok one way, `missing-arg` the other).
#[test]
fn two_headers_declaring_one_routine_differently_conflict() {
    let dir = scratch("conflict_l1_l2");
    let l1 = write(&dir, "l1.tmh", L1_TMH);
    let l2 = write(&dir, "l2.tmh", L2_TMH);
    let c8 = write(&dir, "c8.tmc", C8);
    for (first, second) in [(&l1, &l2), (&l2, &l1)] {
        let err = compile(
            &dir,
            &c8,
            &["--nostdlib", "--extern", first.to_str().unwrap(), "--extern", second.to_str().unwrap()],
        )
        .unwrap_err();
        assert!(err.contains(&format!("{}:3:18: error:", second.display())), "{err}");
        assert!(
            err.contains(&format!(
                "`lib::r` is declared differently by this unit and by `{}`",
                first.display()
            )),
            "{err}"
        );
        assert!(err.contains("[conflicting-declaration]"), "{err}");
    }
}

/// One unit declared twice — the same file given twice, or a byte copy —
/// is one declaration. Mutation: compare by origin instead of by form.
#[test]
fn one_unit_declared_twice_merges() {
    let dir = scratch("conflict_merge");
    let l1 = write(&dir, "l1.tmh", L1_TMH);
    let copy = write(&dir, "l1copy.tmh", L1_TMH);
    let c8 = write(&dir, "c8.tmc", C8);
    for other in [&l1, &copy] {
        let out = compile(
            &dir,
            &c8,
            &["--nostdlib", "--extern", l1.to_str().unwrap(), "--extern", other.to_str().unwrap()],
        );
        assert!(out.is_ok(), "{:?}", out.err());
    }
}

/// Mutation: skip the `check_own_against_table` call.
#[test]
fn a_unit_defining_a_name_its_table_declares_differently_is_refused() {
    let dir = scratch("conflict_own");
    let l1 = write(&dir, "l1.tmh", L1_TMH);
    let own = write(&dir, "own.tmc", OWN);
    let err = compile(&dir, &own, &["--nostdlib", "--extern", l1.to_str().unwrap()]).unwrap_err();
    assert!(err.contains("own.tmc:3:18: error:"), "{err}");
    assert!(err.contains(&format!("by this unit and by `{}`", l1.display())), "{err}");
    assert!(err.contains("[conflicting-declaration]"), "{err}");
}

/// A unit compiled beside its own up-to-date header is fine; beside a
/// stale one it is refused. Mutation: own forms computed without
/// `settle_declarations` — the first half fails on `r`'s `noreturn`.
#[test]
fn a_unit_beside_its_own_header_compiles() {
    let dir = scratch("conflict_own_header");
    let own = write(&dir, "own.tmc", OWN);
    let printed = execute(&[
        "interface".into(),
        own.to_str().unwrap().into(),
        "--nostdlib".into(),
    ])
    .expect("interface");
    let fresh = write(&dir, "own.tmh", &printed.stdout);
    let ok = compile(&dir, &own, &["--nostdlib", "--extern", fresh.to_str().unwrap()]);
    assert!(ok.is_ok(), "{:?}", ok.err());
    let stale = write(&dir, "own_stale.tmh", L1_TMH);
    let err = compile(&dir, &own, &["--nostdlib", "--extern", stale.to_str().unwrap()]).unwrap_err();
    assert!(err.contains("[conflicting-declaration]"), "{err}");
}

/// Mutation: leave the embedded stdlib out of the table the own-definition
/// check reads.
#[test]
fn a_unit_redefining_a_std_routine_differently_is_refused() {
    let dir = scratch("conflict_ownstd");
    let src = write(&dir, "ownstd.tmc", OWNSTD);
    let err = compile(&dir, &src, &[]).unwrap_err();
    assert!(err.contains("ownstd.tmc:3:20: error:"), "{err}");
    assert!(
        err.contains("`std::binaryNumbers::goToNumber` is declared differently by this unit and by the standard library"),
        "{err}"
    );
}

/// The identical redefinition compiles (the LINK refuses it). Mutation:
/// refuse every own name the table declares without comparing forms.
#[test]
fn a_unit_redefining_a_std_routine_identically_compiles() {
    let dir = scratch("conflict_ownsame");
    let src = write(&dir, "ownsame.tmc", OWNSAME);
    let out = compile(&dir, &src, &[]);
    assert!(out.is_ok(), "{:?}", out.err());
}
```

(`OWNSAME` is Task 2b's fixture; copy the constant into
`extern_declarations.rs`.) Row 16's body writes `U` as `u.tmc`, prints
`u.tmh` with `tmt interface u.tmc --nostdlib`, writes `UC`, and compiles
with `["--nostdlib", "--extern", u_tmc, "--extern", u_tmh]`, asserting
`is_ok()`.

`library_grafts.rs::a_user_std_graph_beside_the_embedded_one_conflicts`
calls `shadowed_nested_graft(tag, "tape extra: symbols, tape num: symbols, state done", false)`
— the existing `SHADOW_FAKESTD` with `PARAMS` spliced, WITHOUT `--nostdlib`
— and asserts for both routes: code `Some(1)`, the text contains
`fakestd.tmc:4:18:`,
`` `std::binaryNumbers::goToNumberGraph` is declared differently by this unit and by the standard library ``
and `[conflicting-declaration]`. Give `shadowed_nested_graft` a third
parameter, `nostdlib: bool`, which adds `--nostdlib` to both routes. Its doc
states why the GRAPH is the name reported: the fake's `symbols`
(`'_','0','1','$','^'`) also differs from the stdlib's (`'_','^','$','0','1'`),
and own names are checked in sorted order, `…goToNumberGraph` before
`…symbols`. (Today's source-route message says `cons.tmc:3:53` for a
position inside `nlib.tmc` — a pre-existing file mislabel, not this task's.)

In-crate tests (`published.rs`, `#[cfg(test)] mod tests`):

```rust
use std::path::{Path, PathBuf};
use crate::compiler::{CompileErrorKind, analyze_with};
use crate::declarations::{Declarations, Origin};
use crate::header::read_extern;
use mtc_core::diagnostics::Span;

const L1: &str = "namespace lib {\n  export alphabet ab { '_', 'a' }\n  export routine r(tape t: ab writes { 'a' });\n}\n";
const L2: &str = "namespace lib {\n  export alphabet ab { '_', 'a' }\n  export routine r(tape t: ab writes { 'a' }, state done);\n}\n";
const C8: &str = "use lib::ab;\nuse lib::r;\nmachine {\n  tape t: ab;\n  entry state s { [*] -> call r(t = t) then stop; }\n}\n";

fn table(modules: &[(&str, &str)]) -> Declarations {
    let mut decls = Declarations::none();
    for (path, text) in modules {
        let resolved = read_extern(Path::new(path), text, &Declarations::none()).expect("reads");
        decls.push(Origin::Extern(PathBuf::from(path)), resolved);
    }
    decls
}

/// Mutation: drop `Declarations::check_conflicts` from `resolve_program`.
#[test]
fn two_table_modules_publishing_one_name_differently_conflict() {
    let err = analyze_with(C8, &table(&[("l1.tmh", L1), ("l2.tmh", L2)])).unwrap_err();
    assert_eq!(err.span, Span::point(0, 0));
    assert_eq!(
        err.kind,
        CompileErrorKind::ConflictingDeclaration {
            name: "lib::r".into(),
            first: "`l1.tmh`".into(),
            second: "`l2.tmh`".into(),
        }
    );
}

/// Mutation: compare modules by origin instead of by form.
#[test]
fn two_table_modules_publishing_one_name_identically_merge() {
    assert!(analyze_with(C8, &table(&[("l1.tmh", L1), ("l1b.tmh", L1)])).is_ok());
}

/// A name of another kind is another declaration. Mutation: compare only
/// forms of the same kind.
#[test]
fn an_own_name_of_another_kind_conflicts() {
    let src = "namespace lib {\n  alphabet r { '_', 'a' }\n}\nmachine {\n  tape t: lib::r;\n  entry state s { [*] -> stop; }\n}\n";
    let err = analyze_with(src, &table(&[("l1.tmh", L1)])).unwrap_err();
    assert!(
        matches!(&err.kind, CompileErrorKind::ConflictingDeclaration { name, first, .. }
            if name == "lib::r" && first == "this unit"),
        "{err:?}"
    );
    assert_eq!(err.span.start.line, 2);
}

const OWN: &str = "namespace lib {\n  export alphabet ab { '_', 'a' }\n  export routine r(tape t: ab writes { 'a' }, state done) {\n    entry state s { [*] -> goto done; }\n  }\n}\nmachine {\n  tape t: lib::ab;\n  entry state s { [*] -> call lib::r(t = t, done = stop) then stop; }\n}\n";
const OWN_FIN: &str = "namespace lib {\n  export alphabet ab { '_', 'a' }\n  export routine r(tape t: ab writes { 'a' }, state fin) {\n    entry state s { [*] -> goto fin; }\n  }\n}\nmachine {\n  tape t: lib::ab;\n  entry state s { [*] -> call lib::r(t = t, fin = stop) then stop; }\n}\n";
const C_OWN: &str = "use lib::ab;\nuse lib::r;\nmachine {\n  tape t: ab;\n  entry state s { [*] -> call r(t = t, done = stop) then stop; }\n}\n";

fn own_object() -> mtc_core::formats::object::ObjectFile {
    crate::compiler::compile(
        OWN,
        crate::compiler::CompileOptions { externals: Declarations::none(), ..Default::default() },
    )
    .expect("OWN compiles alone")
    .object
}

/// The object reading names its exit `exit0`, the source reading `done`;
/// they are one unit and must merge. The TEXT reading goes first so the
/// consumer's `done = …` binds against a named parameter. Mutation: compare
/// forms by derived `==` — they conflict on the state name.
#[test]
fn an_object_reading_and_a_source_reading_of_one_unit_merge() {
    let mut decls = table(&[("own.tmc", OWN)]);
    let from_object = crate::header::declarations_from_object(&own_object(), &Declarations::none())
        .expect("the object reads");
    decls.push(Origin::Library("own".into()), from_object);
    assert!(analyze_with(C_OWN, &decls).is_ok());
}

/// The object read's OWN-definition check, against a table that already
/// holds the source reading. Mutation: `own_forms` ignoring `Provenance`.
#[test]
fn an_object_read_beside_its_source_reading_is_not_a_conflict() {
    let context = table(&[("own.tmc", OWN)]);
    assert!(crate::header::declarations_from_object(&own_object(), &context).is_ok());
}

/// Two TEXT readings disagree on a state name: that is a real difference
/// (a caller binds by name). Mutation: compare state parameters by count
/// even when both are named.
#[test]
fn two_text_readings_differing_in_a_state_name_conflict() {
    let err = analyze_with(C8, &table(&[("own.tmc", OWN), ("own_fin.tmc", OWN_FIN)])).unwrap_err();
    assert_eq!(
        err.kind,
        CompileErrorKind::ConflictingDeclaration {
            name: "lib::r".into(),
            first: "`own.tmc`".into(),
            second: "`own_fin.tmc`".into(),
        }
    );
}

/// Mutation: doc lines inside the published form.
#[test]
fn a_doc_line_is_not_part_of_a_declaration() {
    let documented = L1.replace("  export routine r", "  ? Documented here.\n  export routine r");
    assert!(analyze_with(C8, &table(&[("l1.tmh", L1), ("l1doc.tmh", &documented)])).is_ok());
}
```

(`C8` in row 15 is only a vehicle — the conflict is in the table, reported
before `C8` is resolved. If `declarations_from_object`'s error type is
`UnresolvedSource`, `.is_ok()` / `.expect` still apply. Adjust `Span::point`'s import path and `Declarations::push`'s visibility
if needed — both are `pub(crate)`/crate-visible today.)

Columns: `L1_TMH`/`L2_TMH` line 3 is `  export routine r(…` — `r` at column 18.
`OWN` line 3 likewise, column 18. `OWNSTD` line 3 is
`    export routine goToNumber(…` — column 20. `FAKESTD` line 4 is
`    export graph goToNumberGraph(…` — column 18. All **[computed]** by the
same count that gives `b4.tmc:9:51` (Task 7) its observed column.

Library-graft fixtures: `SHADOW_NLIB_TMC`, `SHADOW_CONSUMER` and
`SHADOW_FAKESTD` already exist in `library_grafts.rs` (they equal `NLIB`,
`CONS` and `FAKESTD` with `PARAMS` spliced); no new constant is needed.

Two new library constants for the four collision tests below
(**[verified]**: both print as shown by `tmt interface --nostdlib`, and every
consumer named below compiles against them today exactly as it does against
`LIB_TMC`):

```rust
/// `LIB_TMC`'s `walker` over a PRIVATE `lib::marks`: the header prints the
/// alphabet but does not publish it, so a consumer's own `lib::marks` is no
/// second declaration of a published name.
const ALPHA_COLLISION_LIB_TMC: &str = "\
namespace lib {
  alphabet marks { '_', 'x', 'y' }
  export map flip: marks -> marks { 'x' -> 'y', 'y' -> 'x' }

  export graph walker(tape t: marks, state done) {
    entry state w {
      ['x'] -> write ['y'] goto done;
      [*]   -> goto done;
    }
  }
}
";

/// `LIB_TMC`'s `relay` nesting a PRIVATE `walker`: printed by the header,
/// not published, so a consumer's own `lib::walker` is no second
/// declaration of a published name.
const MEMO_LIB_TMC: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  export map flip: marks -> marks { 'x' -> 'y', 'y' -> 'x' }

  graph walker(tape t: marks, state done) {
    entry state w {
      ['x'] -> write ['y'] goto done;
      [*]   -> goto done;
    }
  }

  export graph relay(tape a: marks, state done) {
    entry graft walker(t = a with map flip, done = done) as inner;
  }
}
";
```

**Rewrites of the flipping tests.**
- `extern_declarations.rs::extern_order_is_command_line_order_then_stdlib`
  → rename `two_extern_headers_declaring_one_name_differently_conflict_in_either_order`;
  same `ORDER_A`/`ORDER_B` files, flags `--nostdlib --extern A --extern B` and
  the reverse; each is `Err` containing `[conflicting-declaration]` and
  `std::binaryNumbers::goToNumbersStart`, the first naming `order_a.tmh` as
  the other origin, the second `order_b.tmh`. (With the stdlib on, each of
  `order_a.tmh`/`order_b.tmh` alone already conflicts with the real
  `goToNumbersStart` — that is test 7's shape.) Mutation: skip the own-vs-table
  call — both compiles return to first-wins.
- `build_derives_declarations.rs::a_peer_shadowing_a_stdlib_name_is_believed_over_the_builtin`
  → rename `a_library_header_redeclaring_a_stdlib_routine_differently_is_refused`;
  same files and build; `out.status` fails and stderr contains
  `[conflicting-declaration]`, `std::binaryNumbers::minusOne`, `mystd.tmh`
  and `the standard library`. Keep
  `with_no_peer_the_builtin_stdlib_item_is_still_found` unchanged.
- `library_grafts.rs::a_nested_library_graft_is_checked_against_the_graph_it_splices`
  and `…_missing_a_later_parameter_is_missing_arg`: pass `--nostdlib` to both
  routes in `shadowed_nested_graft` (the fake is then the only
  `std::binaryNumbers` in the compile). Their assertions (code 1,
  ``missing binding argument for parameter `extra` ``, `[missing-arg]`, no
  panic) hold unchanged **[verified: `nlib.tmc:3:53` / `nlib.tmh:4:7`, rc 1]**;
  the error is now raised while `nlib` is READ, so update both doc comments'
  mutation to "drop the graft-argument check of a declarations read
  (`WorldCtx::check_reuse_targets`' graft arm)". The splice-time
  `check_graft_args` call in `expand::build_composite` stays as a defensive
  check (the graph the splice finds is now always the one the library was
  read against).
- `crates/turing-machine/src/lsp/tests.rs::a_shadowing_sibling_wins_over_the_stdlib_at_every_leg`
  **may** flip: its sibling defines a `std::` routine with a signature that
  differs from the stdlib's, which the editor's analysis now refuses. If it
  reds, change the sibling's routine to the stdlib's exact signature (the
  `OWNSAME` shape); the test's property — the editor routes a `std::` name
  through the overlay first — is unaffected.
- `crates/turing-machine/src/lsp/overlay.rs`'s std-name faithfulness test
  already uses the identical signature since Task 2b.
- **The four `library_grafts.rs` tests whose consumer defines a name the
  library publishes differently** (the own-definition rule refuses them as
  written; their properties stay reachable by making the colliding library
  name private, which the header prints but does not publish — 3b above):
  - `a_colliding_local_alphabet_is_diagnosed_not_silently_captured` and
    `an_unrelated_colliding_local_alphabet_does_not_refuse_a_valid_graft`:
    build the library from `ALPHA_COLLISION_LIB_TMC` instead of `LIB_TMC`.
    Assertions unchanged — `[identity-glyph-mismatch]` for the permuted local
    `lib::marks` (**[verified today against that library: `colliding.tmc:7:22`
    `[identity-glyph-mismatch]`]**), success for the wider one, and the
    `CONSUMER_WALKER` near miss still compiles (importing the private
    alphabet by `use lib::marks` works — verified).
  - `a_consumers_own_graph_does_not_leak_into_a_librarys_nested_graft` and
    `a_librarys_nested_graft_does_not_leak_into_a_consumers_own_graph`:
    build the library (header, `compile_alone`, and the drifted copy) from
    `MEMO_LIB_TMC` instead of `LIB_TMC` (the drift replacement string still
    matches `walker`'s body). The text assertions hold (verified: the local
    body `wrmv    [-, -], [<, .]` and the library's `djmp` both appear); the
    `grafts.len() == 1` assertion holds until Task 13, which changes it (see
    there).
  Every other `LIB_TMC` user keeps `LIB_TMC`.

**Docs.**
- `docs/tmt/cli.md` compile-error table, after the `duplicate-name` row:
  ``| `conflicting-declaration` | Two declarations of one qualified name disagree: two declaration sources publish it differently, or this unit defines a name its declarations publish differently (the message names both origins). Identical declarations — a unit's source and its up-to-date header, one library reached twice — are one declaration. |``
- `docs/tmt/cli.md` `--extern` paragraph: replace "The flag is repeatable,
  and order is meaningful: … This mirrors the linker's own first-wins rule for
  a name declared in more than one object." by "The flag is repeatable. Two
  files declaring one name must declare it identically — a unit's source and
  its header, one library given twice — or the compile fails
  `conflicting-declaration`; the same holds against the embedded standard
  library, which is read last, and against the unit's own definitions
  (`docs/tmt/language.md (declarations)`)."
- `docs/tmt/language.md` "### Declarations", new paragraph after the list
  of what a reading yields: "**One declaration per name.** A qualified name
  has one declaration in a compile. Two declaration sources that publish
  it identically — the same signature and contract clauses and `noreturn`
  for a routine, the same digest for a graph, the same glyphs for an
  alphabet, the same pairs for a map, the same members for a set — are one
  declaration seen twice; anything else is `conflicting-declaration`,
  naming both. The unit's own definition counts: a unit defining a name its
  declarations publish differently is refused at that definition, which is
  how a stale header is caught at the first compile. Doc lines and comments
  are not part of a declaration. A header also PRINTS the non-exported
  declarations an exported body needs; those are not published, and a
  unit's own definition of the same name is not a second declaration of
  anything the header offers."
- `docs/tmt/project.md` Declaration derivation: "Only the FINAL table's
  precedence is fixed: …" → "The FINAL table lists every OTHER source of the
  target, then each declared library, then the embedded standard library
  unless opted out; since two of them may declare one name only identically
  (`docs/tmt/language.md (declarations)`), the order decides nothing."

**Steps.**
- [ ] Write tests 1–17 and the rewrites; run the focused binaries; the new
  tests red for the stated reasons (16 is a guard: green at the start of this task, red only under Task 4's
  mutation; 13, 14, 17 are green today and must stay green).
- [ ] Implement 1–4; focused green.
- [ ] Mutations (own-vs-table call removed → 1, 4, 6, 7, 9 red; `check_conflicts`
  removed → 10 red; settle skipped in `own_forms` → 5 red, because `own.tmh`
  prints `r` as `noreturn` and the unsettled own form does not; forms not
  compared → 3, 5, 8 red; derived `==` → 13 red; `Provenance` ignored → 14
  red; counts only → 15 red; writes step skipped → 16 red).
- [ ] Re-run Task 0's shipped-build sweep and
  `cargo nextest run -p mtc-turing-machine --test stdlib_header --test header_roundtrip --test header_fmt_clean --test interface_emission`
  first (these compile `std.tmc` against the default table: a red here means
  the Task 4 guard missed a difference — stop and report).
- [ ] Docs; `error_code_docs.rs::the_published_compile_catalog_lists_exactly_the_registry_codes` green.
- [ ] Gate: `cargo nextest run -p mtc-turing-machine -j 4` once;
  `cargo nextest run -p mtc-wasm -j 4` once (the browser binding compiles
  sources against the embedded stdlib).
- [ ] Commit: `feat(turing-machine): a name declared differently twice is a compile error`.
  Ledger: BREAKING — `--extern` order no longer shadows; a unit redefining a
  `std::` routine differently is refused at compile.

---

### Task 6: `duplicate-pattern-binding`

**Property.** A binding name repeated within ONE rule's pattern is a compile
error naming the name, both cells and the state the rule is in, pointing at
the second binding; the same name in two different rules is fine. Today the
rule compiles and the rightmost cell silently wins in every write.

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs` (variant, registry row,
  `Display`, witness; the check in `WorldCtx::check_rules`)
- Modify: `docs/tmt/cli.md` (compile-error table), `docs/tmt/language.md`
  ("### Pattern cells", the `as NAME` paragraph)
- Test: `crates/turing-machine/tests/pattern_bindings.rs` (new; no disk
  writes — it uses `mtc_turing_machine::compiler::compile` in memory)

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  // compiler.rs — declared immediately after `DuplicateBinding(String)`
  /// One rule's pattern binds a name twice (docs/tmt/language.md (pattern
  /// cells)). Cells are counted from 1, in pattern order.
  DuplicatePatternBinding { name: String, first: usize, second: usize, state: String },
  // registry row, immediately after the `DuplicateBinding(_)` row:
  CompileErrorKind::DuplicatePatternBinding { .. } => "duplicate-pattern-binding",
  ```
  `Display`: ``"`{name}` is bound by cell {first} and again by cell {second} of one rule in state `{state}` — each binding in a pattern needs its own name"``.
  Witness (after `DuplicateBinding("x".into())`):
  `CompileErrorKind::DuplicatePatternBinding { name: "x".into(), first: 1, second: 2, state: "s".into() }`.

**"Names the rule".** The spec's error names the name, both cells and the
rule. The span points at the second binding, which lies inside the rule; the
message adds the state the rule belongs to. That is how the rule is named.

**Where the check lives, and why.** In `WorldCtx::check_rules`, which already
walks every rule of every world (routine, graph, machine) in source form —
not in the parser, so the parser's pinned fatals and its resilient-parse
contract (`tests/tmc_green_analyze.rs`) are untouched. For each `rule` of
each `state`: walk `rule.pattern.cells` with their 1-based index; keep a
`HashMap<&str, usize>` of names already bound in this rule; the first repeat
returns `Err(CompileError { span: binding.span, kind: DuplicatePatternBinding
{ name, first, second, state: state.name.clone() } })`. Run it before the
transition match in the same loop.

**Falsifying tests.**

| # | Fixture | Command / API | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | `A1` below **[verified]** | `compile(A1, CompileOptions::default())` | Ok (64 rows, both writes take the second cell's value) | `Err` at 6:25 **[computed: the second `x`]**, `DuplicatePatternBinding { name: "x", first: 1, second: 2, state: "s" }`, code `duplicate-pattern-binding` | `pattern_bindings.rs::a_name_bound_twice_in_one_pattern_is_refused` | drop the check |
| 2 | `A1_NEAR` below **[verified]** (`x` and `y`) | same | Ok | Ok | `pattern_bindings.rs::distinct_names_in_one_pattern_compile` | compare every binding with itself (any binding flagged) |
| 3 | `A1_TWO_RULES` below (`x` in two rules of one state) | same | Ok | Ok | `pattern_bindings.rs::one_name_in_two_rules_compiles` | keep the name map across rules of a state |
| 4 | `A1_IN_GRAPH` below (the repeat inside a graph body grafted by the machine) | same | Ok | `Err` code `duplicate-pattern-binding`, `state: "w"` | `pattern_bindings.rs::a_repeat_inside_a_graph_body_is_refused` | run the check on the machine world only |

```rust
use mtc_turing_machine::compiler::{CompileOptions, compile};

const A1: &str = "\
alphabet d { '0', '1', '2', '3', '4', '5', '6', '7' }
machine {
  tape a: d;
  tape b: d;
  entry state s {
    [0..7 as x, 0..7 as x] -> write [{x}, {x}] stop;
  }
}
";

const A1_NEAR: &str = "\
alphabet d { '0', '1', '2', '3', '4', '5', '6', '7' }
machine {
  tape a: d;
  tape b: d;
  entry state s {
    [0..7 as x, 0..7 as y] -> write [{y}, {x}] stop;
  }
}
";

const A1_TWO_RULES: &str = "\
alphabet d { '0', '1', '2', '3' }
machine {
  tape a: d;
  entry state s {
    [0..1 as x] -> write [{x}] move [>] goto s;
    [2..3 as x] -> write [{x}] stop;
  }
}
";

const A1_IN_GRAPH: &str = "\
alphabet d { '0', '1', '2', '3' }
graph g(tape a: d, tape b: d, state done) {
  entry state w {
    [0..3 as x, 0..3 as x] -> write [{x}, {x}] goto done;
  }
}
machine {
  tape a: d;
  tape b: d;
  entry graft g(a = a, b = b, done = stop) as gg;
}
";

/// Mutation: drop the repeat check in `check_rules` — the program compiles
/// again, the right cell silently winning.
#[test]
fn a_name_bound_twice_in_one_pattern_is_refused() {
    let err = compile(A1, CompileOptions::default()).unwrap_err();
    assert_eq!(err.kind.code(), "duplicate-pattern-binding");
    assert_eq!((err.span.start.line, err.span.start.col), (6, 25));
    assert_eq!(
        err.kind.to_string(),
        "`x` is bound by cell 1 and again by cell 2 of one rule in state `s` — each binding in a pattern needs its own name"
    );
}

/// Mutation: a check that flags any binding (comparing a binding with itself).
#[test]
fn distinct_names_in_one_pattern_compile() {
    compile(A1_NEAR, CompileOptions::default()).expect("x and y are two names");
}

/// Mutation: the name map kept across the rules of one state.
#[test]
fn one_name_in_two_rules_compiles() {
    compile(A1_TWO_RULES, CompileOptions::default()).expect("one binding per rule");
}

/// Mutation: the check run over the machine world only.
#[test]
fn a_repeat_inside_a_graph_body_is_refused() {
    let err = compile(A1_IN_GRAPH, CompileOptions::default()).unwrap_err();
    assert_eq!(err.kind.code(), "duplicate-pattern-binding");
    assert!(err.kind.to_string().contains("state `w`"), "{}", err.kind);
}
```

(All four fixtures **[verified]**: each compiles today, rc 0.)

**Docs.** `docs/tmt/cli.md`, after the `duplicate-binding` row:
``| `duplicate-pattern-binding` | One rule's pattern binds a name twice (`[0..7 as x, 0..7 as x]`) — the message names both cells and the state; give each binding its own name. |``
`docs/tmt/language.md` "### Pattern cells", after "A cell may bind what it
matched with `as NAME`…": "A name is bound once per pattern:
`[0..7 as x, 0..7 as x]` is `duplicate-pattern-binding`, naming both cells.
The same name in two different rules is two unrelated bindings."

**Steps.**
- [ ] Tests red (1, 4); 2, 3 green.
- [ ] Implement; green; mutation (drop the check) → 1, 4 red.
- [ ] Docs; `error_code_docs.rs` and the witness test green.
- [ ] Gate: `cargo nextest run -p mtc-turing-machine -j 4` once. The corpus
  has 139 bindings, none repeated within one rule (Task 0 scan) — no flip
  expected; `tmc_property.rs` generates patterns: if it now produces
  duplicate names and reds, its generator must mint distinct names per rule
  (report as a disposition).
- [ ] Commit: `feat(turing-machine): a binding name repeated in one pattern is an error`.
  Ledger: BREAKING — a pattern binding a name twice no longer compiles.

---

### Task 7: `return` handed to a state parameter outside a routine

**Property.** Every argument list written in a `machine` or `graph` world —
a `call`, a `graft`, a `bind`, used or not, local or across units, with or
without the callee's declarations — refuses a `return` terminator with
`return-outside-routine` at the terminator, exactly as a `return` transition
or `then return` already is. A `return` argument inside a routine stays
legal. The stray `unreachable-state` warning of the graft shape disappears
because its cause does: a return-bound exit made the machine's own state dead.

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs` (new
  `refuse_return_outside_routine`; calls in `WorldCtx::check_reuse_targets`
  for every call, graft and bind argument list; the same call in
  `check_graft_args`)
- Modify: `docs/tmt/language.md` (the state-argument `return` sentence under
  "### `state` parameters"), `docs/tmt/cli.md` (`return-outside-routine`
  row: "…or `state` argument…" → "…or argument of a `call`, `graft` or
  `bind`…")
- Test: `crates/turing-machine/tests/state_params.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  /// A `return` terminator in an argument list is legal only when the list
  /// is written in a routine (docs/tmt/language.md (`state` parameters)):
  /// it binds the callee's exit to the host routine's own return. Checked
  /// by the host's kind alone, before any signature is consulted, so it
  /// holds for a callee whose declarations were never given.
  pub(crate) fn refuse_return_outside_routine(args: &[BindingArg], host: WorldKind) -> Result<(), CompileError>;
  ```
  It returns `Err(CompileError { span, kind: ReturnOutsideRoutine })` for the
  first `BindingValue::Terminator { kind: TermKind::Return, span }` when
  `host != WorldKind::Routine`.

**Decided in the plan.** The rule applies to a `return` in ANY argument
position, not only to a declared state parameter: without declarations the
parameter's kind is unknown, and `return` means "the host routine returns"
in every position. A `return` handed to a TAPE parameter in a machine is
therefore `return-outside-routine`, not `wrong-arg-kind` (no existing test
pins the latter).

**Falsifying tests** (all fixtures from the survey's probe set, **[verified]**
TODAY; columns **[computed]**, calibrated by `b4.tmc:9:51`, which is the
terminator's own column):

| # | Fixture | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|
| 1 | `B1` (machine graft, `miss = return`) | rc 0, silent | `13:50` `return-outside-routine` | `state_params.rs::return_in_a_machine_graft_is_refused` | skip the graft lists |
| 2 | `B2` (graft inside a graph body) | rc 0, silent | `12:47` `return-outside-routine` | `state_params.rs::return_in_a_graph_body_graft_is_refused` | check machine worlds only |
| 3 | `B3` (both exits `return`, a dead `done`) | ``b3.tmc:14:9: warning: state `done` is unreachable in `main` [unreachable-state]``, rc 0 | `13:37` `return-outside-routine` (the first `return`), no warning | `state_params.rs::the_unreachable_state_symptom_is_now_its_cause` | — |
| 4 | `B7` (unused `bind` in a machine) | rc 0 (only `unused-routine`) | `9:27` `return-outside-routine` | `state_params.rs::return_in_an_unused_machine_bind_is_refused` | skip the bind lists |
| 5 | `B8_LIB` (library graph `std::binaryNumbers::goToNumberGraph`, stdlib on) | rc 0 | `3:67` `return-outside-routine` | `state_params.rs::return_in_a_machine_graft_of_a_library_graph_is_refused` | check only grafts of local graphs |
| 6 | `B4` (call in a machine) | ``b4.tmc:9:51: error: `return` is only allowed inside a routine [return-outside-routine]`` | unchanged (now raised by the compiler's check rather than `ir::resolve_exits`; same span) | `state_params.rs::return_in_a_machine_call_is_refused` (new — no test pins this code today) | — (calibration of the column count) |
| 7 | `B9_ROUTINE` (graft in a ROUTINE with `miss = return`) | rc 0 | rc 0 | `state_params.rs::return_in_a_routine_graft_compiles` | refuse `return` in every world |

```rust
const B1: &str = "\
alphabet ab { '_', 'a', 'b' }

graph findA(tape t: ab, state hit, state miss) {
  entry state s {
    ['a'] -> hit;
    ['_'] -> miss;
    [*]   -> move [>] goto s;
  }
}

machine {
  tape main: ab;
  entry graft findA(t = main, hit = done, miss = return) as seek;
  state done { [*] -> stop; }
}
";

const B2: &str = "\
alphabet ab { '_', 'a', 'b' }

graph findA(tape t: ab, state hit, state miss) {
  entry state s {
    ['a'] -> hit;
    ['_'] -> miss;
    [*]   -> move [>] goto s;
  }
}

graph outer(tape t: ab, state done) {
  entry graft findA(t = t, hit = done, miss = return) as seek;
}

machine {
  tape main: ab;
  entry graft outer(t = main, done = d) as o;
  state d { [*] -> stop; }
}
";

const B3: &str = "\
alphabet ab { '_', 'a', 'b' }

graph findA(tape t: ab, state hit, state miss) {
  entry state s {
    ['a'] -> hit;
    ['_'] -> miss;
    [*]   -> move [>] goto s;
  }
}

machine {
  tape main: ab;
  entry graft findA(t = main, hit = return, miss = return) as seek;
  state done { [*] -> stop; }
}
";

const B4: &str = "\
alphabet ab { '_', 'a', 'b' }

routine r(tape t: ab, state done) {
  entry state s { [*] -> done; }
}

machine {
  tape main: ab;
  entry state go { [*] -> call r(t = main, done = return) then stop; }
}
";

const B7: &str = "\
alphabet ab { '_', 'a', 'b' }

routine r(tape t: ab, state done) {
  entry state s { [*] -> done; }
}

machine {
  tape main: ab;
  bind r(t = main, done = return) as b;
  entry state go { [*] -> stop; }
}
";

const B8_LIB: &str = "\
machine {
  tape t: std::binaryNumbers::symbols;
  entry graft std::binaryNumbers::goToNumberGraph(num = t, done = return) as g;
}
";

const B9_ROUTINE: &str = "\
alphabet ab { '_', 'a', 'b' }

graph findA(tape t: ab, state hit, state miss) {
  entry state s {
    ['a'] -> hit;
    ['_'] -> miss;
    [*]   -> move [>] goto s;
  }
}

routine seek(tape t: ab, state found) {
  entry graft findA(t = t, hit = found, miss = return) as g;
}

machine {
  tape main: ab;
  entry state go { [*] -> call seek(t = main, found = done) then done; }
  state done { [*] -> stop; }
}
";

/// One helper for the table: compile in memory with the embedded stdlib and
/// return the error's code and position.
fn refused_at(src: &str) -> (String, u32, u32) {
    let err = compile(src, CompileOptions::default()).expect_err("must be refused");
    (err.kind.code().to_string(), err.span.start.line, err.span.start.col)
}

/// Mutation: skip the graft argument lists in `check_reuse_targets`.
#[test]
fn return_in_a_machine_graft_is_refused() {
    assert_eq!(refused_at(B1), ("return-outside-routine".into(), 13, 50));
}
```

Tests 2–6 are the same one-line assertion with their fixture and `(line,
col)`: B2 `(12, 47)`, B3 `(13, 37)`, B7 `(9, 27)`, B8_LIB `(3, 67)`, B4
`(9, 51)`. Test 3 additionally asserts nothing else: `compile(B3, …)` is an
`Err` (the warning path is never reached). Test 7:
`compile(B9_ROUTINE, CompileOptions::default()).expect("a routine may bind an exit to its own return")`.
(`state_params.rs` imports `compile`/`CompileOptions` already; check the
line/col field types of `Span` — they are `u32` today.)

**Implementation.** Add `refuse_return_outside_routine`. In
`WorldCtx::check_reuse_targets`, at the top of each of the three loops (call
transitions, graft declarations, bind declarations — the bind loop BEFORE its
`if b.external { … continue; }`), call it with `world.kind`. In
`check_graft_args`, call it with `host.kind` before `check_binding_list` (the
splice-time repeat of the same rule). Leave `ir.rs`'s own
`ReturnOutsideRoutine` arm in `resolve_exits` as it is — it becomes
unreachable from source, a defensive backstop.

**Docs.** `docs/tmt/language.md` "### `state` parameters": state that a
`return` argument is legal only in a routine, for a `call`, a `graft` and a
`bind` alike — "a `graft` in a `machine` or a `graph` body, or a `bind` in a
machine whether or not anything calls it, is `return-outside-routine`";
`docs/tmt/cli.md` row as in Files.

**Steps.**
- [ ] Tests 1–7; red: 1, 2, 3, 4, 5.
- [ ] Implement; green; mutations per table.
- [ ] Docs.
- [ ] Gate: `cargo nextest run -p mtc-turing-machine -j 4` once. The corpus
  uses `= return` only inside routines (`std.tmc`, `facade_proof.rs`,
  `library_grafts.rs` routine fixtures) — no flip expected.
- [ ] Commit: `fix(turing-machine): return is refused as an argument outside a routine`.
  Ledger: BREAKING (minor) — a graft or bind in a machine or graph body that
  hands `return` to an exit no longer compiles.

---

### Task 8: One argument check for `call`/`bind` across units

**Property.** A `call` or `bind` into a routine whose declarations the compile
was given, with a NON-EMPTY argument list, is checked by the same
`check_binding_list` a local call or a graft is checked by, against the
declared signature — so a terminator handed to a tape parameter is
`wrong-arg-kind` (today `missing-arg`, or silently accepted when every
argument is a terminator), and a `missing-arg` on a `bind` points at the bind.
An argument-LESS call or bind into another unit is the transparent form and
is not checked here (its state parameters, if any, stay required by
`ir::resolve_exits`). A callee with no declarations is unchanged: the check is
deferred to the linker.

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs`
  (`find_external_routine`; `WorldCtx::check_call_like`'s out-of-unit arm;
  `WorldCtx::check_reuse_targets`' external-bind arm; the doc comments of
  `check_call_like`, `check_binding_args`, `signature` that say the check
  "runs later in `ir::resolve_binding`")
- Modify: `crates/turing-machine/src/ir.rs` (`resolve_binding`'s external
  arm keeps only the ORDERING; the duplicate / unknown / alias checks move out;
  its doc comment)
- Modify: `docs/tmt/language.md` ("### Calls across units", the "With
  declarations" bullet: add "a terminator handed to a tape parameter is
  `wrong-arg-kind`" and "a `bind` is checked where it is declared")
- Test: `crates/turing-machine/tests/cross_unit.rs`

**Interfaces:**
- Consumes: Task 7's `refuse_return_outside_routine` runs before this check
  on the same lists (a `return` in a machine is that error first).
- Produces:
  ```rust
  /// The routine world `name` names in the declarations table, first module
  /// first — kind-filtered, unlike `Declarations::routine`.
  pub(crate) fn find_external_routine<'a>(externals: &[&'a Resolved], name: &str) -> Option<(&'a Resolved, &'a ResolvedWorld)>;
  ```
  (Task 9 re-implements it over its shared lookup without changing the
  signature.)

**Implementation.**
- `check_call_like`, arm `Some(_)` with `kind == None`: bind the resolution
  (`Some(r)`) and
  ```rust
  if args.is_empty() {
      // The transparent form: tapes bind by position, nothing to check by
      // name (docs/tmt/language.md (calls across units)).
      return Ok(());
  }
  match find_external_routine(self.externals, &r.full) {
      Some((_, callee)) => check_binding_list(&SigInfo::declared(callee), args, states, tapes, span),
      // No declarations: the linker checks the list against the object.
      None => Ok(()),
  }
  ```
- `check_reuse_targets`, bind loop: for `b.external`, after
  `warn_undeclared_if_bare`, when `!b.args.is_empty()` and
  `find_external_routine(self.externals, &b.target)` is `Some((_, callee))`,
  run `check_binding_list(&SigInfo::declared(callee), &b.args, &states,
  &tapes, b.target_span)?` before `continue`.
- `ir.rs::resolve_binding`, external arm with `Some(sig)`: delete the
  `seen`/`tape_seen` loop (the compiler has already run the same checks on
  the same list); keep `order_by_callee_tapes(sig.tapes…, &named_args, site)?`
  and everything after. Rewrite the function's doc comment accordingly ("its
  tape order and parameter names are checked by the compiler's shared
  argument check; this only orders the entries").

**Falsifying tests** (fixtures **[verified]**; `C1_TMH` is the `--extern`
header; columns **[computed]** against `c1.tmc`'s observed rule column 19):

| # | Fixture | Command | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | `C1` | `tmt compile c1.tmc --nostdlib --extern c1.tmh` | ``c1.tmc:7:19: error: missing binding argument for parameter `a` [missing-arg]`` | ``c1.tmc:7:36: error: binding argument `a` must be a tape target [wrong-arg-kind]`` | `cross_unit.rs::a_terminator_for_a_declared_tape_parameter_is_wrong_arg_kind` | keep the IR copy as the only external check |
| 2 | `C3` (all terminators) | same | rc 0 | ``c3.tmc:7:36: … `a` must be a tape target [wrong-arg-kind]`` | `cross_unit.rs::an_all_terminator_call_into_a_declared_routine_is_checked` | skip the check when no argument is `Named` |
| 3 | `C2` (a `bind` missing `b`) | same | ``c2.tmc:8:19: error: missing binding argument for parameter `b` [missing-arg]`` | ``c2.tmc:7:10: … `b` [missing-arg]`` (the bind's first argument) | `cross_unit.rs::a_missing_argument_on_a_bind_points_at_the_bind` | check external binds only at their call site |
| 4 | `C_TRANSPARENT` (argless call, stdlib on) — Task 11's `ut.tmc` | `tmt compile` | rc 0 | rc 0 | `cross_unit.rs::a_transparent_call_into_a_declared_routine_compiles` | run the shared check on an empty list (`missing-arg num`) |
| 5 | `C_OK` (all tape arguments correct) | `--extern c1.tmh` | rc 0 | rc 0 | `cross_unit.rs::a_correct_call_into_a_declared_routine_compiles` | — (control for 1–3) |

**Until Task 9 lands**, the "no declarations" branch above also lets through
a call or bind naming a declared GRAPH (`find_external_routine` misses it);
Task 9 adds the kind check in front of it.

```rust
const C1_TMH: &str = "\
namespace lib {
  export alphabet dec { '_', '0', '1' }
  export routine r(tape a: dec writes {}, state done);
  export routine q(tape a: dec writes {}, tape b: dec writes {});
}
";

const C1: &str = "\
use lib::dec;
use lib::q;

machine {
  tape t: dec;
  tape u: dec;
  entry state s { [*, *] -> call q(a = stop, b = u) then stop; }
}
";

const C2: &str = "\
use lib::dec;
use lib::q;

machine {
  tape t: dec;
  tape u: dec;
  bind q(a = t) as qq;
  entry state s { [*, *] -> call qq() then stop; }
}
";

const C3: &str = "\
use lib::dec;
use lib::q;

machine {
  tape t: dec;
  tape u: dec;
  entry state s { [*, *] -> call q(a = stop, b = halt) then stop; }
}
";

const C_OK: &str = "\
use lib::dec;
use lib::q;

machine {
  tape t: dec;
  tape u: dec;
  entry state s { [*, *] -> call q(a = t, b = u) then stop; }
}
";

const C_TRANSPARENT: &str = "\
alphabet bin { '_', '^', '$', '0', '1' }
machine {
  tape t: bin;
  entry state go { [*] -> call std::binaryNumbers::plusOne() then stop; }
}
";

/// Mutation: keep `ir::resolve_binding` as the only external check — the
/// error is `missing-arg` at the rule again.
#[test]
fn a_terminator_for_a_declared_tape_parameter_is_wrong_arg_kind() {
    let dir = scratch("xunit_terminator_tape");
    let header = write(&dir, "c1.tmh", C1_TMH);
    let input = write(&dir, "c1.tmc", C1);
    let err = compile(&dir, &input, "c1.tmo", &["--nostdlib", "--extern", header.to_str().unwrap()])
        .unwrap_err();
    assert!(err.contains("c1.tmc:7:36: error: binding argument `a` must be a tape target [wrong-arg-kind]"), "{err}");
}
```

Tests 2 and 3 have the same body with `C3` / `C2` and the AFTER texts
from the table. Tests 4 and 5 assert `is_ok()` (test 4 without `--nostdlib`
and without `--extern`; test 5 with the header). The no-declarations path is
already pinned by
`cross_unit.rs::a_missing_parameter_is_a_link_error_not_a_compile_error`,
which must stay green unchanged.

**One intended span move.** A `missing-arg` for a declared callee's STATE
parameter, on a call that passes other arguments, was raised by
`ir::resolve_exits` at the rule span; `check_binding_list` raises it at the
call's first argument. If a pinned test asserts the rule column for that
shape, that pin is this task's intended change: update it and list it in the
report. Any OTHER moved pin is a stop-and-report.

**Tests pinned today that must stay green unchanged** (same code, same
spans — both implementations blame the same argument span): `cross_unit.rs`
`an_unrecognized_argument_name_is_a_compile_error_with_declarations`,
`a_duplicate_argument_name_is_a_compile_error_with_declarations`,
`two_arguments_naming_one_tape_are_a_compile_error_with_declarations`
(`app.tmc:4:57:`), `two_bind_arguments_naming_one_tape_…` (`app.tmc:4:33:`),
`two_arguments_naming_one_non_tape_are_an_unresolved_target_with_declarations`,
`two_state_arguments_sharing_one_state_compile_with_declarations`,
`with_declarations_the_site_reorders_to_the_callees_tape_order`,
`a_missing_parameter_is_a_link_error_not_a_compile_error`;
`state_params.rs::an_external_exit_bearing_call_without_declarations_is_refused`.
If one moves, the order of checks differs between the two copies — report it
rather than re-pinning.

**Steps.**
- [ ] Tests 1–5; red: 1, 2, 3.
- [ ] Implement; green; mutations per table.
- [ ] Docs.
- [ ] Gate: `cargo nextest run -p mtc-turing-machine -j 4` once.
- [ ] Commit: `fix(turing-machine): a call into a declared routine takes the shared argument check`.
  Ledger: BREAKING (minor) — stricter argument kinds across units.

---

### Task 9: A cross-unit name of the wrong kind says what it is

**Property.** When a name reached through `use` or a qualified path resolves,
in the declarations given to the compile, to a declaration of ANOTHER kind,
the message says what it is instead of claiming its declarations were not
given — where an alphabet, a map or a graph was expected, as it already does
where a set was expected. A `call` or `bind` naming a declared GRAPH is
`wrong-target-kind`, exactly as a local one is, and both messages now name
the kind found (``"`NAME` is a graph, not a routine"``). Codes do not
change. The one question "what kind is this external name" is answered by
one function the per-kind lookups share.

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs`:
  - `DefKind` and `DefKind::noun` become `pub(crate)` (today module-private):
    `ExternalDecl::kind()` returns a `DefKind` from a `pub(crate)` item,
    which the `private_interfaces` lint refuses otherwise; `external_kind`
    becomes `pub(crate)` for `expand.rs`;
  - new `ExternalDecl`, `external_decls`; `external_kind`,
    `find_external_alphabet`, `find_external_map`, `find_external_graph`,
    `find_external_routine` rewritten over it; `resolve_set_ref`'s inline
    lookup replaced;
  - new `AlphabetMiss::WrongKind`, `MapMiss::WrongKind`,
    `GraphMiss::WrongKind` and their `Display` arms; the three miss sites
    `resolve_tape_alphabet`, `resolve_map_ref`, `resolve_world_reuse`;
  - `CompileErrorKind::WrongTargetKind` gains `found: &'static str`; its
    `Display`; every constructor (`resolve_set_ref`, `resolve_world_reuse`,
    `WorldCtx::check_call_like`, `WorldCtx::check_target_kind`) passes the
    kind it already holds (`r.kind` / the `by_full` entry) through `noun()`;
    the witness in `error_codes_are_pairwise_distinct_and_complete`;
  - the fourth site: `WorldCtx::check_call_like`'s out-of-unit arm and
    `WorldCtx::check_reuse_targets`' external-bind arm.
- Modify: `crates/turing-machine/src/declarations.rs` (`Declarations::routine`
  filters `WorldKind::Routine`, so `ir.rs` can no longer take a graph's
  signature for a routine's; after this task's compiler check the case is
  unreachable from source anyway)
- Modify: `crates/turing-machine/src/expand.rs` (the nested-graft miss near
  the `GraphMiss::DeclarationsNotGiven` construction in the splice)
- Modify: `docs/tmt/cli.md` (rows `unresolved-alphabet`, `undefined-map`,
  `undefined-graph`, `wrong-target-kind`), `docs/tmt/language.md` (the
  three-way miss split near "Alphabets, maps and graphs across units"; the
  `call`/`bind` target sentence in "### Calls across units")
- Test: `crates/turing-machine/tests/glyph_sets.rs` (beside the existing
  `a_cross_unit_alphabet_named_as_a_set_says_what_it_is`),
  `crates/turing-machine/tests/cross_unit.rs` (the graph-as-call-target
  rows), `crates/turing-machine/src/compiler.rs` (unit test)

**Interfaces:**
- Consumes (Task 8): `find_external_routine` — signature unchanged.
- Produces:
  ```rust
  /// One declaration of a name in the declarations table.
  pub(crate) enum ExternalDecl<'a> {
      Alphabet(&'a ResolvedAlphabet),
      Set(&'a ResolvedSet),
      Map(&'a ResolvedMapDecl),
      World(&'a Resolved, &'a ResolvedWorld),
  }
  impl ExternalDecl<'_> { pub(crate) fn kind(&self) -> DefKind }
  /// Every declaration of the mangled `name` in the table, module by module
  /// in table order, and within a module alphabet, set, map, then its
  /// routine or graph world (a machine is never a declaration). The ONE
  /// walk every cross-unit lookup reads: a per-kind lookup takes the first
  /// of its kind, and a miss asks the first of any kind what the name is.
  pub(crate) fn external_decls<'a>(externals: &[&'a Resolved], name: &str) -> Vec<ExternalDecl<'a>>;
  pub(crate) fn external_kind(externals: &[&Resolved], name: &str) -> Option<DefKind>;
  pub(crate) enum DefKind { Alphabet, Set, Map, Routine, Graph } // visibility only
  pub enum AlphabetMiss { …, WrongKind { name: String, found: &'static str } }
  pub enum MapMiss { …, WrongKind { name: String, found: &'static str } }
  pub enum GraphMiss { …, WrongKind { name: String, found: &'static str } }
  pub enum CompileErrorKind { …, WrongTargetKind { name: String, expected: &'static str, found: &'static str }, … }
  ```
  `external_kind(externals, name)` becomes
  `external_decls(externals, name).first().map(ExternalDecl::kind)`; each
  `find_external_*` becomes a `find_map` over `external_decls` matching its
  variant (and, for worlds, its `WorldKind`). `DefKind::noun` supplies
  `found` everywhere.

**Messages** (codes unchanged):
- `AlphabetMiss::WrongKind`: ``"`{name}` is {found} in the declarations given to this compile, not an alphabet — a tape's type must be an alphabet"``
- `MapMiss::WrongKind`: ``"`{name}` is {found} in the declarations given to this compile, not a named map"``
- `GraphMiss::WrongKind`: ``"`{name}` is {found} in the declarations given to this compile, not a graph — a graft needs a graph's source"``
- `WrongTargetKind` (local and cross-unit alike): ``"`{name}` is {found}, not {expected}"`` — e.g. ``"`g` is a graph, not a routine"``.

At each of the three miss sites, where today the code builds
`DeclarationsNotGiven(r.full)` after the per-kind lookup failed, build
`match external_kind(&ext_modules, &r.full) { Some(k) => …::WrongKind { name: r.full, found: k.noun() }, None => …::DeclarationsNotGiven(r.full) }`
— the same shape `resolve_set_ref` has. The nested-graft miss in
`expand.rs` does the same.

**The fourth site (a declared GRAPH as a call/bind target).** In
`WorldCtx::check_call_like`'s out-of-unit arm, BEFORE Task 8's argument-less
exemption and its `find_external_routine` lookup:
```rust
if find_external_routine(self.externals, &r.full).is_none()
    && let Some(k) = external_kind(self.externals, &r.full)
{
    return Err(CompileError {
        span: target.span,
        kind: CompileErrorKind::WrongTargetKind {
            name: joined.to_string(),
            expected: expected_noun,
            found: k.noun(),
        },
    });
}
```
and the same in `check_reuse_targets`' external-bind arm at `b.target_span`
with `expected: "a routine"`, before its `continue`.

**Falsifying tests** (`LIB_TMH` is the survey's `lib.tmh`, `GLIB_TMH` the
pre-flight's graph header; fixtures **[verified]**, spans unchanged from TODAY
for rows 1–4; rows 6–9 **[computed]** from the observed local `7:31`):

| # | Fixture | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|
| 1 | `D1` (`tape t: someset`, a set), `--nostdlib --extern lib.tmh` | ``d1.tmc:4:11: error: alphabet `lib::someset` is declared by `use` … declarations were not given … [unresolved-alphabet]`` | ``d1.tmc:4:11: error: `lib::someset` is a glyph set in the declarations given to this compile, not an alphabet — a tape's type must be an alphabet [unresolved-alphabet]`` | `glyph_sets.rs::a_cross_unit_set_named_as_an_alphabet_says_what_it_is` | miss site not consulting `external_kind` |
| 2 | `D2` (`with map lib::dec`, an alphabet), same flags | ``d2.tmc:6:51: error: map `lib::dec` is declared by `use` … [undefined-map]`` | ``d2.tmc:6:51: error: `lib::dec` is an alphabet in the declarations given to this compile, not a named map [undefined-map]`` | `glyph_sets.rs::a_cross_unit_alphabet_named_as_a_map_says_what_it_is` | same, map site |
| 3 | `D3` (`graft skip`, a routine), same flags | ``d3.tmc:6:15: error: graph `lib::skip` is declared by `use` … [undefined-graph]`` | ``d3.tmc:6:15: error: `lib::skip` is a routine in the declarations given to this compile, not a graph — a graft needs a graph's source [undefined-graph]`` | `glyph_sets.rs::a_cross_unit_routine_grafted_as_a_graph_says_what_it_is` | same, graft site |
| 4 | `D1` with NO `--extern` | ``d1.tmc:4:11: error: alphabet `lib::someset` is declared by `use` … declarations were not given … [unresolved-alphabet]`` | unchanged | (second half of test 1) | build `WrongKind` whenever the per-kind lookup misses, without asking whether anything declares the name |
| 5 | unit: `external_decls` over two modules where module 1 declares `x` as a set and module 2 as a graph | `find_external_graph` → module 2's graph; `external_kind` → `Set` | same | `compiler.rs::tests::the_per_kind_lookups_and_the_kind_query_walk_one_list` | a per-kind lookup with its own loop that stops at the first module |
| 6 | `CALLG` (`call lib::g(t = t, done = fin)`, `lib::g` a declared graph), `--nostdlib --extern glib.tmh` | rc 0 silently; linking it with the compiled library then fails ``tmt: unresolved symbols: lib::g`` | ``callg.tmc:4:31: error: `lib::g` is a graph, not a routine [wrong-target-kind]`` | `cross_unit.rs::a_call_naming_a_declared_graph_is_wrong_target_kind` | no kind check (rc 0 and a link-time `unresolved symbols` again) |
| 7 | `CALLG_BARE` (`call lib::g()`), same flags | ``callg_bare.tmc:4:19: error: missing binding argument for parameter `done` [missing-arg]`` | ``callg_bare.tmc:4:31: … `lib::g` is a graph, not a routine [wrong-target-kind]`` | `cross_unit.rs::an_argless_call_naming_a_declared_graph_is_wrong_target_kind` | the kind check placed after the argument-less exemption |
| 8 | `BINDG_FULL` (`bind lib::g(t = t, done = fin) as b`), same flags | rc 0 silently | ``bindg_full.tmc:4:8: … `lib::g` is a graph, not a routine [wrong-target-kind]`` | `cross_unit.rs::a_bind_naming_a_declared_graph_is_wrong_target_kind` | no kind check on the external-bind arm |
| 9 | `LOCALG` (a local graph `g`, `call g(t = t)`) | ``localg.tmc:7:31: error: `g` is not a routine [wrong-target-kind]`` | ``localg.tmc:7:31: error: `g` is a graph, not a routine [wrong-target-kind]`` | `cross_unit.rs::a_call_naming_a_local_graph_says_what_it_is` | `found` left out of the local message |

(`D2` and `D3` without `--extern` fail earlier, at their `tape t: dec` —
``d2.tmc:5:11`` / ``d3.tmc:5:11`` `[unresolved-alphabet]` **[verified]** —
so tests 2 and 3 have no second half. Task 8's `C_OK` — a call into a
declared ROUTINE — is the near miss for rows 6–8 and must stay green.)

```rust
// glyph_sets.rs
const LIB_TMH: &str = "\
namespace lib {
  export alphabet dec { '_', '0', '1' }
  export set someset { '0', '1' }
  export map m: dec -> dec { '0' -> '0', '1' -> '1' }
  export routine skip(tape t: dec writes {});
}
";

const D1: &str = "\
use lib::someset;

machine {
  tape t: someset;
  entry state s { [*] -> stop; }
}
";

const D2: &str = "\
use lib::dec;
use lib::skip;

machine {
  tape t: dec;
  entry state s { [*] -> call skip(t = t with map lib::dec) then stop; }
}
";

const D3: &str = "\
use lib::dec;
use lib::skip;

machine {
  tape t: dec;
  entry graft skip(t = t) as g;
}
";
```

```rust
// cross_unit.rs
const GLIB_TMH: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export graph g(tape t: ab, state done) {
    entry state w { [*] -> write ['a'] goto done; }
  }
}
";

const CALLG: &str = "\
use lib::ab;
machine {
  tape t: ab;
  entry state s { [*] -> call lib::g(t = t, done = fin) then fin; }
  state fin { [*] -> stop; }
}
";

const CALLG_BARE: &str = "\
use lib::ab;
machine {
  tape t: ab;
  entry state s { [*] -> call lib::g() then fin; }
  state fin { [*] -> stop; }
}
";

const BINDG_FULL: &str = "\
use lib::ab;
machine {
  tape t: ab;
  bind lib::g(t = t, done = fin) as b;
  entry state s { [*] -> call b() then fin; }
  state fin { [*] -> stop; }
}
";

const LOCALG: &str = "\
alphabet ab { '_', 'a' }
graph g(tape t: ab, state done) {
  entry state w { [*] -> write ['a'] goto done; }
}
machine {
  tape t: ab;
  entry state s { [*] -> call g(t = t) then fin; }
  state fin { [*] -> stop; }
}
";

/// Mutation: no kind check on the out-of-unit call arm — the call compiles
/// and only the link says `unresolved symbols: lib::g`.
#[test]
fn a_call_naming_a_declared_graph_is_wrong_target_kind() {
    let dir = scratch("xunit_call_graph");
    let header = write(&dir, "glib.tmh", GLIB_TMH);
    let input = write(&dir, "callg.tmc", CALLG);
    let err = compile(&dir, &input, "callg.tmo", &["--nostdlib", "--extern", header.to_str().unwrap()])
        .unwrap_err();
    assert!(
        err.contains("callg.tmc:4:31: error: `lib::g` is a graph, not a routine [wrong-target-kind]"),
        "{err}"
    );
}
```

Rows 7 and 8 have the same body with their fixture and AFTER text; row 9
compiles `LOCALG` with `--nostdlib` and no header.

Test 1 (in `glyph_sets.rs`, which has `scratch`, `write`, `args` and
`execute`); tests 2 and 3 are the same body with `D2`/`D3` and their AFTER
texts and no second half:

```rust
/// Mutation: the alphabet miss site not asking `external_kind` — the
/// message claims the declarations were not given.
#[test]
fn a_cross_unit_set_named_as_an_alphabet_says_what_it_is() {
    let dir = scratch("glyph_sets_set_as_alphabet");
    let header = write(&dir, "lib.tmh", LIB_TMH);
    let input = write(&dir, "d1.tmc", D1);
    let out = dir.join("d1.tmo");
    let err = execute(&args(&[
        "compile", input.to_str().unwrap(), "--nostdlib",
        "--extern", header.to_str().unwrap(), "-o", out.to_str().unwrap(),
    ]))
    .expect_err("a set is not an alphabet");
    assert!(
        err.contains(
            "d1.tmc:4:11: error: `lib::someset` is a glyph set in the declarations given to \
             this compile, not an alphabet — a tape's type must be an alphabet [unresolved-alphabet]"
        ),
        "{err}"
    );
    assert!(!err.contains("declarations were not given"), "{err}");

    // Without the declarations the other reading stands.
    let err = execute(&args(&[
        "compile", input.to_str().unwrap(), "--nostdlib", "-o", out.to_str().unwrap(),
    ]))
    .expect_err("nothing declares it");
    assert!(err.contains("declarations were not given"), "{err}");
    assert!(err.contains("[unresolved-alphabet]"), "{err}");
}
```

The existing `a_cross_unit_alphabet_named_as_a_set_says_what_it_is` must
stay green unchanged — it pins the set site `external_decls` now also
serves. The two `compiler.rs` unit tests that match
`WrongTargetKind { name, .. }` (near "graft targets a routine") keep
matching with the new field.

Test 5 is a unit test in `compiler.rs`'s `mod tests`; build the two
modules with `crate::header::read_extern` over two small headers
(`namespace n { export alphabet ab { '_', 'a' } export set x { 'a' } }` and
`namespace n { export alphabet ab { '_', 'a' } export graph x(tape t: ab, state d) { entry state s { [*] -> goto d; } } }`)
read against `Declarations::none()`, then call the lookups on
`[&m1, &m2]` directly and assert both answers. Its doc says that a real
compile would refuse this table first (`conflicting-declaration`, Task 5),
so the test drives the lookups, not `analyze_with`. Mutation it catches:
re-growing an independent first-module loop in `find_external_graph` that
stops at module 1 — `None` instead of module 2's graph.

**Docs.** `docs/tmt/cli.md`: extend `unresolved-alphabet`, `undefined-map`
and `undefined-graph` rows with the third reading, in `undefined-set`'s
words: "A CROSS-UNIT name that the declarations given to this compile
declare as another kind is this code too; the message names the kind
found." `wrong-target-kind` row: "A `call`/`graft`/`bind` target … resolves
to the wrong entity kind — a local one, or a `call`/`bind` naming a graph
the declarations given to this compile declare; the message names the kind
found." `docs/tmt/language.md`: the same sentence for the alphabet, map and
graph cases in the three-way split, and in "### Calls across units": "A
`call` or `bind` naming a GRAPH another unit declares is
`wrong-target-kind` — a graph is grafted, never called."

**Steps.**
- [ ] Tests 1–9; red: 1, 2, 3, 6, 7, 8, 9 (texts), 5 (functions missing).
- [ ] Implement (visibility first, then `external_decls`, the miss sites, the
  `found` field, the fourth site, `Declarations::routine`'s filter); green;
  mutations per table.
- [ ] Docs; `error_code_docs.rs` and the witness test green.
- [ ] Gate: `cargo nextest run -p mtc-turing-machine -j 4` once.
- [ ] Commit: `fix(turing-machine): a cross-unit name of the wrong kind says what it is`.

---

### Task 10a: `undeclared-exit` refused at assembly

**Property.** A routine whose `.routine` line declares `exits=N` and whose
body executes `retx #k` with `k ≥ N` does not assemble: the assembler
reports `undeclared-exit` at that instruction. It reads the one arch-neutral
predicate the linker's copy path already reads; it learns no opcode. The
link-time behaviour does not change in this task.

**Why the fixture rebuild lands HERE, not in 10b.** From this task on, the
tests that build such a body through the assembler panic in their `asm()`
helper. To end the task green, 10a rebuilds those fixtures WITHOUT the
assembler (assemble with one more declared exit, then lower the count in the
object) and keeps every one of their assertions as they are today; 10b then
changes only what they assert.

**Files:**
- Modify: `crates/core/src/asm/syntax.rs` (`SyntaxEntry::is_exit_return`)
- Modify: `crates/core/src/asm/mod.rs` (`AsmErrorKind::UndeclaredExit`,
  registry row, `Display`)
- Modify: `crates/core/src/asm/assembler.rs` (the check in
  `assemble_lowered`, before the per-function loop)
- Modify: `crates/core/src/linker/stamp.rs` (delete the private
  `is_exit_return`; its two callers use the method — no behaviour change)
- Modify docs: `docs/core.md` (error-codes table row), `docs/tmt/asm.md`
  (the `retx` immediate bullet), `docs/tmt/lint.md` (`### retx-exit-bounds`:
  one sentence on the division of labour)
- Test: `crates/core/tests/link_checks.rs` (its `fake_syntax()` has `retx`
  and `trap`, so the assembler tests live there; fixtures rebuilt),
  `crates/core/tests/link_exits.rs` (fixture rebuilt),
  `crates/turing-machine/tests/link_warnings.rs` (a new assembler test; the
  CLI link test's fixture rebuilt)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (used by 10b):
  ```rust
  // crates/core/src/asm/syntax.rs
  impl SyntaxEntry {
      /// A multi-exit return: an `Imm8` operand naming the exit, with
      /// `Stop` flow — the flow is what tells it from a trap carrying the
      /// same operand kind (docs/core.md (linking)). The assembler's
      /// `undeclared-exit` check, the linker's, and the copy path's
      /// rewrite all read this one predicate.
      pub fn is_exit_return(&self) -> bool { self.operand == OperandKind::Imm8 && self.flow == Flow::Stop }
  }
  // crates/core/src/asm/mod.rs — last variant, last registry row
  UndeclaredExit { routine: String, exit: u8, declared: u8 },
  AsmErrorKind::UndeclaredExit { .. } => "undeclared-exit",
  ```
  `Display`: ``"`{routine}` returns through exit {exit}, but its signature declares {declared} exit(s)"``
  (the link warning's text, word for word).
  Test helpers `declaring_exits(src, routine, exits) -> ObjectFile` (in
  `link_checks.rs`, `link_exits.rs`, `link_warnings.rs`, each with its own
  `asm`), `three_exit_callee(last: &str) -> String` (in `link_checks.rs`).

**Implementation.** In `assemble_lowered`, when `lowered.interface` is
`Some(iface)`, for each `(i, function)` in `functions.iter().enumerate()`
with `declared = iface.routines[i].exits`, for each
`SourceItem::Instr { span, opcode, operand: SourceOperand::Imm(k), .. }` in
`function.items` where
`syntax.by_opcode(*opcode).is_some_and(SyntaxEntry::is_exit_return)` and
`*k >= declared`: `return Err(AsmError { span: *span, kind:
AsmErrorKind::UndeclaredExit { routine: function.name.clone(), exit: *k,
declared } })`. It runs before the per-function loop, so the first offending
function in source order is reported. `stamp.rs`: replace `is_exit_return(e)`
by `e.is_exit_return()` at both call sites and delete the free function.

**Fixture technique** (the helpers, written out):

```rust
/// Assemble `src`, then set `routine`'s declared exit count to `exits` in
/// the object's interface — the one way a body returning through an
/// undeclared exit reaches the linker now that the assembler refuses it.
fn declaring_exits(src: &str, routine: &str, exits: u8) -> ObjectFile {
    let mut obj = asm(src);
    let blob = obj
        .symbols
        .iter()
        .find_map(|s| match s.def {
            SymbolDef::Defined { blob } | SymbolDef::Local { blob } if s.name == routine => Some(blob),
            _ => None,
        })
        .expect("the routine is defined");
    obj.interface.as_mut().expect("the object has an interface").routines[blob as usize].exits = exits;
    obj
}

/// `two_exit_callee`, its callee declaring THREE exits: a body returning
/// through exit 2 then assembles, and `declaring_exits(…, "sub", 2)` lowers
/// the count back to what the call site's two labels bind.
fn three_exit_callee(last: &str) -> String {
    two_exit_callee(last).replace("exits=2", "exits=3")
}
```

(Core files: add `SymbolDef` to the `mtc_core::formats::object` import. The
TM file uses `mtc_turing_machine::asm::assemble(src, false).unwrap()` in
place of `asm`.) A site's exit vector must match the LOWERED count, so the
variant keeps the site's labels as they are. Probed on the TM twin: a
three-exit declaration with a two-label site assembles (rc 0) and, unpatched,
links only as ``bad binding … the call site supplies 2 exit(s), but `sub` declares 3``
— the patch step is mandatory.

**Falsifying tests.**

| # | Fixture | Command / API | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | core: `.routine sub, tapes=1, alpha=(3), exits=2` / `.param p, ('_', '0', '1')` / `.section code` / `.func sub` / `        retx    #2` | `assemble(&fake_syntax(), 0x7E, SRC, false)` | Ok (the TM-1 twin `sub2.tma` assembles, rc 0 **[verified]**) | `Err(AsmError { kind: UndeclaredExit { routine: "sub", exit: 2, declared: 2 }, span.start.line == 5 })` | `link_checks.rs::a_return_through_an_undeclared_exit_does_not_assemble` | drop the assembler check |
| 2 | same with `retx #1` | same | Ok | Ok | `link_checks.rs::a_return_through_a_declared_exit_assembles` | off-by-one (`k + 1 >= declared`) |
| 3 | same with `trap #2` | same | Ok | Ok | `link_checks.rs::a_trap_with_an_exit_sized_immediate_assembles` | classify by operand kind alone |
| 4 | TM `UNDECLARED_EXIT` (unchanged text) **[verified]** | `tmt asm fire.tma -o fire.tmo` | rc 0 | Err ``fire.tma:15:9: error: `sub` returns through exit 2, but its signature declares 2 exit(s) [undeclared-exit]`` **[computed: the `retx` line, 8-space indent]** | `link_warnings.rs::the_assembler_refuses_a_return_through_an_undeclared_exit` | drop the assembler check |
| 5 | rebuilt fixtures (next list) | their existing assertions | green | green, UNCHANGED assertions | the four tests named below | build the fixture through the assembler again (the test panics) |

Fixtures rebuilt, assertions untouched:
- `link_checks.rs::a_return_through_an_undeclared_exit_warns`:
  `declaring_exits(&three_exit_callee("retx    #2"), "sub", 2)` in place of
  `asm(&two_exit_callee("retx    #2"))`.
- `link_checks.rs::an_undeclared_exit_merges_with_site_warnings_in_offset_order`:
  its `sub` gains `exits=1` on its `.routine` line (its body's `retx #0` then
  assembles) and the object goes through `declaring_exits(SRC, "sub", 0)`.
  (Its main calls `sub` plainly, so no site vector constrains the count.)
- `link_exits.rs::a_body_returning_through_an_undeclared_exit_is_refused`:
  `BODY_OVERRUNS_ITS_EXITS` declares `exits=2` (the site's `exits=(won)`
  unchanged) and the object is `declaring_exits(BODY_OVERRUNS_ITS_EXITS, "sub", 1)`.
- `link_warnings.rs::a_return_through_an_undeclared_exit_warns_on_the_cli`:
  instead of `tmt asm fire.tma`, write
  `declaring_exits(&UNDECLARED_EXIT.replace("exits=2, noreturn", "exits=3, noreturn"), "sub", 2).to_bytes()`
  to `fire.tmo` — byte-identical to what `tmt asm` produced before this task
  (only the operand of `retx` and the interface count ever differed, and the
  patch restores the count).

Test 1 body (tests 2 and 3 replace the last line and assert `is_ok()`):

```rust
/// Mutation it catches: drop the check in `assemble_lowered` — the file
/// assembles.
#[test]
fn a_return_through_an_undeclared_exit_does_not_assemble() {
    const SRC: &str = "\
.routine sub, tapes=1, alpha=(3), exits=2
.param p, ('_', '0', '1')
.section code
.func sub
        retx    #2
";
    let err = assemble(&fake_syntax(), ARCH, SRC, false).unwrap_err();
    assert_eq!(
        err.kind,
        mtc_core::asm::AsmErrorKind::UndeclaredExit { routine: "sub".into(), exit: 2, declared: 2 }
    );
    assert_eq!(err.span.start.line, 5);
}
```

**Docs (substance).**
- `docs/core.md` error-codes table, last row:
  ``| `undeclared-exit` | tables | a routine's body returns through an exit (`retx #k`) at or above the `exits=` count its `.routine` line declares |``.
- `docs/tmt/asm.md`, the `retx` immediate bullet: "In a routine whose
  `.routine` line declares `exits=N`, `retx #k` needs `k < N`; otherwise the
  file does not assemble (`undeclared-exit`)."
- `docs/tmt/lint.md` `### retx-exit-bounds`: "A routine whose own `.routine`
  line declares its exits is checked by the assembler itself
  (`undeclared-exit`, `docs/core.md (error codes)`); this rule covers the
  returns a frame descriptor governs."

**Steps.**
- [ ] Before editing: `git grep -n 'AsmErrorKind::' -- crates/wasm crates/*/src`
  — any exhaustive match there gains the arm.
- [ ] Tests 1–4; see 1 and 4 red. Rebuild the four fixtures; their tests
  stay green today (the patch is a no-op on behaviour).
- [ ] Implement; green; mutations per table.
- [ ] Drift guards: core `error_code_docs.rs::the_published_catalog_lists_exactly_the_registry_codes`
  and `every_capability_cell_names_a_real_assembler_capability`.
- [ ] Gate: `cargo nextest run -p mtc-core -p mtc-post-machine -p mtc-turing-machine -j 4` once
  (PM byte identity inside; PM has no `.routine` interface, so nothing there
  can change); `cargo nextest run -p mtc-wasm -j 4` once (the browser
  binding surfaces assembler errors through `build` on `tma`).
- [ ] Commit: `feat(core): a return through an undeclared exit does not assemble`.
  Ledger: BREAKING — a hand-written `.tma` returning through an undeclared
  exit no longer assembles.

---

### Task 10b: `undeclared-exit` refused at link

**Property.** An object carrying a routine whose body returns through an exit
at or above its declared count (it can only be built without the assembler)
is refused by the linker under every mechanism — even when the entry function
carries no signature. The code leaves the link-warning table, and `--allow
undeclared-exit` or a manifest `lint.allow` naming it becomes an
unknown-code error.

**Files:**
- Modify: `crates/core/src/linker/stamp.rs` (new `check_declared_exits`;
  the splice refusal's comment near "the body returns through exit" — now a
  defensive check)
- Modify: `crates/core/src/linker/engine.rs` (delete `undeclared_exits` and
  its call in `check_sites`; `check_sites`' doc)
- Modify: `crates/core/src/linker/mod.rs` (`LinkError::UndeclaredExit` +
  `Display`; the `undeclared-exit` row leaves `DIAGNOSTIC_CODES`; `link`
  calls `stamp::check_declared_exits` right after the two drift checks)
- Modify docs: `docs/core.md` (the "A third check grades a routine's BODY…"
  paragraph moves into "### Linking" as a refusal; the `undeclared-exit`
  link-warning row goes; "Every finding is raised once…" stays for the
  remaining three), `docs/tmt/cli.md` (the `undeclared-exit` paragraph and
  its link-warning row go; a sentence in `## tmt link`), `docs/tmt/language.md`
  (the grammar-history bullet that calls it "the `undeclared-exit` link
  warning")
- Test: `crates/core/tests/link_checks.rs`, `crates/core/tests/link_exits.rs`,
  `crates/turing-machine/tests/link_warnings.rs`

**Interfaces:**
- Consumes (10a): `SyntaxEntry::is_exit_return`, `declaring_exits`,
  `three_exit_callee`, the rebuilt fixtures. (Task 1): `LinkError` as
  reshaped.
- Produces:
  ```rust
  // crates/core/src/linker/mod.rs
  UndeclaredExit { routine: String, exit: u8, declared: u8 },
  // crates/core/src/linker/stamp.rs
  pub(super) fn check_declared_exits(syntax: &ArchSyntax, order: &[FuncRef]) -> Result<(), LinkError>;
  ```
  `Display`: the same text as the assembler's.

**Implementation.** `check_declared_exits` walks `order` (the resolved,
PRE-stamp records — a stamped copy carries no interface; locals included),
and for each function with `interface: Some(iface)`, for each `(offset, k)`
of `body_exits(syntax, &f.blob)` with `k >= iface.exits`, returns
`UndeclaredExit { routine: f.name.to_string(), exit: k, declared:
iface.exits }` — first function in order, first offset. `link` calls it
after `check_imported_alphabets` and BEFORE `validate_frame_phys` and the
engine, which makes it unconditional (the old scan lived in the engine,
which an unsigned entry skips; such a link is refused later with
`MissingSignature`, so the change is WHICH error an unsigned link reports).
Delete `engine::undeclared_exits` and the `DIAGNOSTIC_CODES` row;
`crates/turing-machine/src/lint/mod.rs::known_code` needs no edit (it reads
the table), and the manifest loader validates `lint.allow` through the same
`known_code`.

**Falsifying tests.**

| # | Fixture | Command / API | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | core: `declaring_exits(&three_exit_callee("retx    #2"), "sub", 2)` | `link(..)` for each of `MECHS` | Frames: Ok + warning; Mono/Hybrid: `BadBinding` | `Err(UndeclaredExit { routine: "sub", exit: 2, declared: 2 })` under all three | `link_checks.rs::a_return_through_an_undeclared_exit_is_refused_under_every_mechanism` (replaces `…_warns`) | drop the link check (frames links, the others say `BadBinding`) |
| 2 | core: `two_exit_callee("retx    #1")` / `("trap    #2")` | `link(..)` for each mech | Ok | Ok (assert `.expect("links")` — the `undeclared_exits` helper goes) | `a_return_through_a_declared_exit_is_silent`, `a_trap_with_the_same_immediate_is_not_an_exit` (kept) | — |
| 3 | core: `an_undeclared_exit_merges_with_site_warnings_in_offset_order` | — | pins the merge order of the warning | **deleted** — the finding is no longer a warning (one line in the commit body) | — | — |
| 4 | core `link_exits.rs`: the 10a-rebuilt `BODY_OVERRUNS_ITS_EXITS` object | `link(..)` Mono/Hybrid/Frames | Mono/Hybrid `BadBinding("the body returns through exit 1…")` | `UndeclaredExit { routine: "sub", exit: 1, declared: 1 }` under all three | `a_body_returning_through_an_undeclared_exit_is_refused` (assertion rewritten; the splice-level `get(k)` refusal is now defensive and unreachable — say so in its doc) | drop the link check |
| 5 | TM: the 10a-rebuilt `fire.tmo` | `tmt link fire.tmo --nostdlib --call-mech M`, M ∈ mono/frames/hybrid | frames: ``sub+0x000e: warning: `sub` returns through exit 2, but its signature declares 2 exit(s) [undeclared-exit]``, rc 0; hybrid: ``tmt: bad binding to `sub`: the body returns through exit 2, but the call site supplies 2 exit(s)``, rc 1 **[verified]** | Err `` `sub` returns through exit 2, but its signature declares 2 exit(s) `` under all three | `link_warnings.rs::the_linker_refuses_an_object_carrying_one_under_every_mechanism` (replaces `a_return_through_an_undeclared_exit_warns_on_the_cli`) | drop the link check |
| 6 | TM: `MAIN_PLAIN` + `SUB_OVER` (unsigned entry, plain call), SUB patched to 2 | `tmt link main_plain.tmo sub.tmo --nostdlib` | ``tmt: entry function `main` has no routine signature to fill the sectioned executable header``, rc 1 **[verified with the body assembled directly before 10a]** | Err `` `sub` returns through exit 2, but its signature declares 2 exit(s) `` | `link_warnings.rs::the_link_check_does_not_need_a_signed_entry` | keep the scan inside the engine |
| 7 | TM: `UNDECLARED_EXIT` with `retx #2` → `retx #1` (legal) | `tmt link fire_ok.tmo --nostdlib --allow undeclared-exit` | rc 0 **[verified]** | Err ``unknown lint rule `undeclared-exit` `` | `link_warnings.rs::allowing_undeclared_exit_is_an_unknown_code` | leave the row in `DIAGNOSTIC_CODES` |
| 8 | TM manifest: `tmt.json` = `{ "lint": { "allow": ["undeclared-exit"] }, "project": { "targets": { "app": { "sources": ["a.tmc"] } } } }`, `a.tmc` = `alphabet a { '_' }` / `machine { tape t: a; entry state s { [*] -> move [>] stop; } }` | `tmt build` in that directory | rc 0 **[verified]** (with `bogus-code` in place of the code today: ``tmt: …/tmt.json: unknown lint rule `bogus-code` in lint.allow``, rc 1 **[verified]**) | Err containing ``unknown lint rule `undeclared-exit` in lint.allow`` | `link_warnings.rs::a_manifest_allowing_undeclared_exit_is_refused` | the manifest loader keeping its own copy of the code list |
| 9 | TM sweep | `no_shipped_program_returns_through_an_undeclared_exit` | counts `undeclared-exit` warnings | counts assembly errors and `LinkError::UndeclaredExit` (both code `undeclared-exit`) as findings instead of skipping them; floors unchanged | (rewritten in place) | grade every multi-exit return — the compiled two-exit control is refused and becomes a finding |

```rust
// crates/turing-machine/tests/link_warnings.rs — new fixtures
const MAIN_PLAIN: &str = "\
.func main
        call    sub
        stp
";

/// Declares three exits so it assembles; the test lowers the count to two.
const SUB_OVER: &str = "\
.routine sub, tapes=1, alpha=(3), exits=3
.param p, ('_', '0', '1')
.func sub
        retx    #2
";
```

Test 5 body:

```rust
/// The linker refuses an object whose routine returns through an exit its
/// interface does not declare, under every mechanism. Mutation: drop
/// `stamp::check_declared_exits` from `link` — frames links again and the
/// copying mechanisms say `bad binding` instead.
#[test]
fn the_linker_refuses_an_object_carrying_one_under_every_mechanism() {
    let dir = scratch("undeclared_exit_link");
    let src = UNDECLARED_EXIT.replace("exits=2, noreturn", "exits=3, noreturn");
    let obj = declaring_exits(&src, "sub", 2);
    let path = dir.join("fire.tmo");
    std::fs::write(&path, obj.to_bytes()).unwrap();
    for mech in ["mono", "frames", "hybrid"] {
        let err = execute(&args(&[
            "link", path.to_str().unwrap(), "--nostdlib", "--call-mech", mech,
            "-o", dir.join(format!("a_{mech}.tmx")).to_str().unwrap(),
        ]))
        .expect_err("an undeclared exit stops the link");
        assert!(
            err.contains("`sub` returns through exit 2, but its signature declares 2 exit(s)"),
            "{mech}: {err}"
        );
    }
}
```

Test 8 body (uses the file's `tmt()` `Command` helper, which the existing
`build` tests in `link_warnings.rs` already use with `current_dir`):

```rust
/// Mutation: the manifest's `lint.allow` validated against a list of its
/// own that still holds the old warning code.
#[test]
fn a_manifest_allowing_undeclared_exit_is_refused() {
    let dir = scratch("undeclared_exit_manifest_allow");
    std::fs::write(
        dir.join("tmt.json"),
        r#"{ "lint": { "allow": ["undeclared-exit"] }, "project": { "targets": { "app": { "sources": ["a.tmc"] } } } }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("a.tmc"),
        "alphabet a { '_' }\nmachine { tape t: a; entry state s { [*] -> move [>] stop; } }\n",
    )
    .unwrap();
    let out = tmt().args(["build"]).current_dir(&dir).output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unknown lint rule `undeclared-exit` in lint.allow"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
```

The sweep's loop becomes:

```rust
let obj = if path.extension().and_then(|s| s.to_str()) == Some("tma") {
    match assemble(&src, false) {
        Ok(o) => o,
        Err(e) if e.kind.code() == "undeclared-exit" => {
            findings.push(format!("{}: {}", path.display(), e.kind));
            continue;
        }
        Err(_) => continue,
    }
} else { /* unchanged */ };
match link(&syntax, std::slice::from_ref(&obj), std::slice::from_ref(&stdlib), LinkOptions { call_mech: CallMech::Frames, ..Default::default() }) {
    Ok(_) => {
        linked += 1;
        control_linked |= path == control;
    }
    Err(e @ LinkError::UndeclaredExit { .. }) => findings.push(format!("{}: {e}", path.display())),
    Err(_) => continue,
}
```

**Docs (substance).**
- `docs/core.md` "### Linking": a bullet "A routine whose body returns
  through an exit index at or above the exit count its interface declares is
  refused, under every mechanism and whether or not the entry carries a
  signature — the assembler already refuses such a body, so only an object
  built another way can carry one. An exit return is the instruction whose
  operand is an `Imm8` exit index and whose flow is `Stop`; a trap carrying
  the same operand kind falls through, which is what tells the two apart. A
  function with no interface declares nothing and is not checked." Delete
  the "A third check grades a routine's BODY…" paragraph and the
  `undeclared-exit` row from the link-warning table.
- `docs/tmt/cli.md`: delete the `undeclared-exit` paragraph and row under
  link warnings; add to `## tmt link`: "A routine returning through an exit
  its `.routine` line does not declare is a link error (the assembler refuses
  the same body in a `.tma`)." — and a note that `undeclared-exit` is no
  longer a link-warning code, so `--allow undeclared-exit` and a
  `lint.allow` entry naming it are unknown-code errors.
- `docs/tmt/language.md` grammar-history bullet: "…and `undeclared-exit`,
  which refuses at assembly and at link a routine returning through an exit
  its signature does not declare".

**Steps.**
- [ ] Before editing: `git grep -n 'LinkError::\|undeclared-exit\|DIAGNOSTIC_CODES' -- crates/wasm crates/*/src`
  — any exhaustive match or pinned code list there changes with this task.
- [ ] Tests 1–9 as in the table; see 1, 4, 5, 6, 7, 8 red.
- [ ] Implement.
- [ ] Focused green; mutations per table.
- [ ] Drift guards: core `error_code_docs.rs::the_published_link_warning_catalog_lists_exactly_the_registry_codes`,
  TM `error_code_docs.rs::the_published_link_warning_catalog_lists_exactly_the_registry_codes`,
  TM `lint/mod.rs::validate_allow_also_accepts_link_warning_codes`.
- [ ] Gate: `cargo nextest run -p mtc-core -p mtc-post-machine -p mtc-turing-machine -j 4` once;
  `cargo nextest run -p mtc-wasm -j 4` once.
- [ ] Commit: `feat(core): a return through an undeclared exit is refused at link`.
  Ledger: BREAKING — `undeclared-exit` is a link error under every
  mechanism; `--allow undeclared-exit` and a `lint.allow` entry naming it
  are now unknown-code errors.

---

### Task 11: Three lints, and the transparent call touches every tape

**Property.**
- `unused-tape` and the new `unused-tape-parameter` treat an argument-less
  `call`, `graft` or `bind` into ANOTHER unit as touching every tape of the
  world it is written in (it binds tapes by position). Today a machine whose
  only rule is `call std::binaryNumbers::plusOne()` is falsely reported.
- `unused-tape-parameter`: a routine's or graph's tape parameter the body
  never reads, writes, moves or passes on (the definition `unused-tape`
  uses) is reported at the parameter's name, whether or not the world is
  exported. No fix.
- `unused-fold-variable`: a pattern binding (`as NAME`) that nothing in its
  rule's write vector reads — a read inside an arithmetic fold counts. No fix.
- `inert-never-writes`: with both clauses written, a `never writes` element
  naming glyphs `writes` does not name is reported at that element, naming
  the inert glyphs. No fix; not on a header (`HEADER_RULES` unchanged).
- All three are default-on and silent over the shipped corpus.

**Files:**
- Modify: `crates/turing-machine/src/lint/rules/unused_tape.rs` (shared
  helpers become `pub(crate)`; the transparent-reuse rule)
- Create: `crates/turing-machine/src/lint/rules/unused_tape_parameter.rs`,
  `unused_fold_variable.rs`, `inert_never_writes.rs`
- Modify: `crates/turing-machine/src/lint/rules/mod.rs` (three module lines,
  alphabetical), `crates/turing-machine/src/lint/mod.rs` (three `RULES` rows)
- Modify: `crates/turing-machine/src/lint/rules/contract_clause_overlap.rs`
  (`elem_span`, `elem_indices`, `ascending_glyphs`, `quoted_list`,
  `world_sig` become `pub(super)`; add `never_writes_clause_for`)
- Modify: `docs/tmt/lint.md` (three `### code` headings with text and an
  example line, in `## The .tmc rules`; the `### unused-tape` paragraph gains
  the transparent sentence)
- Create: `crates/turing-machine/tests/lint_rule_sweeps.rs`
- Test: each rule file's own `#[cfg(test)] mod tests` (the `findings(src)`
  helper shape of `unused_set.rs`), `crates/turing-machine/tests/lint_rule_sweeps.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks (Task 6's `duplicate-pattern-binding`
  makes a repeated name a compile error first, so `unused-fold-variable`
  never sees one).
- Produces (all in `unused_tape.rs`):
  ```rust
  /// Whether `world` reuses another unit with no argument list — a
  /// `call`/`bind` whose target is external, or a `graft` whose target is
  /// not one of this unit's graphs — which binds its tapes by position and
  /// may therefore touch every one of them (docs/tmt/lint.md (unused-tape)).
  pub(crate) fn makes_transparent_reuse(ctx: &LintContext, world: &ResolvedWorld) -> bool;
  /// Every tape name `world` hands to a reuse by name.
  pub(crate) fn bound_tape_names(world: &ResolvedWorld) -> HashSet<&str>;
  /// True when position `pos` is never distinguished, written or moved.
  pub(crate) fn position_is_inert(world: &ResolvedWorld, pos: usize) -> bool;
  ```
  Rule registry rows (append to `RULES`):
  ```rust
  ("unused-tape-parameter", rules::unused_tape_parameter::check),
  ("unused-fold-variable", rules::unused_fold_variable::check),
  ("inert-never-writes", rules::inert_never_writes::check),
  ```
  Messages:
  - ``tape parameter `{tape}` of `{world}` is never read, written, moved, or passed on``
  - ``pattern binding `{name}` is never read by this rule's write vector``
  - ``{glyphs} in `never writes` is not in `writes`, so excluding it changes nothing``
    (`{glyphs}` = `quoted_list(ascending_glyphs(..))`, e.g. `'2'`)

**Implementation notes.**
- `makes_transparent_reuse`: `world.calls` with `ResolvedCallTarget::Routine
  { external: true, args, .. }` and `args.is_empty()`; `world.binds` with
  `external && args.is_empty()`; `world.grafts` with `args.is_empty()` and
  `ctx.resolved.worlds.iter().all(|w| w.name != g.target)`.
  `unused_tape::check` returns early for the machine world when it is true.
- `unused_tape_parameter::check`: for each world with `kind != Machine` that
  has a body (`!world.states.is_empty() || !world.grafts.is_empty()`), skip
  it entirely if `makes_transparent_reuse`; for each `(pos, tape)` not in
  `bound_tape_names` and `position_is_inert(world, pos)`, push a finding at
  `tape.name_span`, `fix: None`. The module doc states why no fix
  (removing a parameter changes the signature every caller binds).
- `unused_fold_variable::check`: for each world, state, rule: the reads are
  every `FoldExprKind::Var(name)` reachable (recursively through
  `FoldExprKind::Bin { lhs, rhs, .. }`) from `WriteCellKind::Subst { expr }`
  cells of `rule.write`; for each `cell.binding` not in the reads, a finding
  at `binding.span`, `fix: None` (a binding may be positional — deleting it
  changes nothing, but the rule does not guess).
- `inert_never_writes::check`: the `contract_clause_overlap::check` shape —
  same `SetScope` construction; for each world with a `world_sig`, each tape
  with `(Some(writes), Some(_))`, the AST `never writes` clause
  (`never_writes_clause_for`, mirroring `writes_clause_for` over
  `SigParamKind::Tape { never_writes, .. }`); for each element,
  `elem_indices`; the inert indices are those `!writes.contains(ix)`; if any,
  a finding at `elem_span(elem)` naming them. `never writes {}` has no
  element and never fires.

**Falsifying tests** (fixtures **[verified]**: each compiles, and `tmt lint
--no-config` prints the TODAY column; columns **[computed]** by the count that
gives `inw_near.tmc`'s observed overlap finding its `2:47`):

| # | Fixture | TODAY (`tmt lint`) | AFTER | Test | Mutation |
|---|---|---|---|---|---|
| 1 | `UT` | ``ut.tmc:3:3: lint: tape `t` is never read, written, or moved`` | silent | `unused_tape.rs::tests::a_transparent_call_into_another_unit_touches_every_tape` | drop the transparent early return |
| 2 | `UT_NEAR` (a BOUND external call names `t`, `u` untouched) | ``ut_near.tmc:4:3: lint: tape `u` is never read, written, or moved`` | unchanged | `unused_tape.rs::tests::a_bound_call_touches_only_the_tapes_it_names` | treat every external call as transparent |
| 3 | `UTP` | silent | `2:28` ``tape parameter `u` of `r` is never read, written, moved, or passed on`` | `unused_tape_parameter.rs::tests::an_untouched_tape_parameter_is_reported` | the rule itself (never pushes) |
| 4 | `UTP_NEAR` (`byname`: passed on; `bymove`: moved; `transparent`: argless external call) | silent | silent | `unused_tape_parameter.rs::tests::a_parameter_passed_on_moved_or_reached_transparently_is_used` | forget one of the three uses (each routine is one) |
| 5 | `UTP` with `export routine r` | silent | fires as in 3 | `unused_tape_parameter.rs::tests::an_exported_routine_is_reported_too` | skip exported worlds |
| 6 | `UFV` | silent | `4:28` ``pattern binding `x` is never read by this rule's write vector`` | `unused_fold_variable.rs::tests::an_unread_binding_is_reported` | the rule itself |
| 7 | `UFV_NEAR` (`{x + 1}` and `{y}`) | silent | silent | `unused_fold_variable.rs::tests::a_binding_read_inside_a_fold_is_used` | collect only top-level `Var` reads (`x` inside `x + 1` missed) |
| 8 | `INW` | silent | `2:60` ``'2' in `never writes` is not in `writes`, so excluding it changes nothing`` | `inert_never_writes.rs::tests::a_never_writes_glyph_writes_does_not_name_is_reported` | the rule itself |
| 9 | `INW_NEAR` (`both`: `'2'` in both — the overlap rule's case; `only`: no `writes`) | ``inw_near.tmc:2:47: lint: '2' is in both `writes` and `never writes`; `never writes` wins, so the `writes` entry is inert`` | unchanged (no `inert-never-writes` finding) | `inert_never_writes.rs::tests::an_effective_or_writes_less_never_writes_is_silent` | fire without requiring `writes` / on overlapping glyphs |
| 10 | corpus sweeps | — | zero findings of each of the four codes | `lint_rule_sweeps.rs::no_shipped_tmc_reports_{unused_tape,unused_tape_parameter,unused_fold_variable,inert_never_writes}` | for `unused-tape-parameter`: drop transparent handling — `docs/examples/rpn/rpn.tmc`'s `incr`, `decr`, `seekStart`, `normalize` fire |

```rust
const UT: &str = "\
alphabet bin { '_', '^', '$', '0', '1' }
machine {
  tape t: bin;
  entry state go { [*] -> call std::binaryNumbers::plusOne() then stop; }
}
";

const UT_NEAR: &str = "\
alphabet bin { '_', '^', '$', '0', '1' }
machine {
  tape t: bin;
  tape u: bin;
  entry state go { [*, *] -> call std::binaryNumbers::plusOne(num = t) then stop; }
}
";

const UTP: &str = "\
alphabet ab { '_', 'a' }
routine r(tape t: ab, tape u: ab) {
  entry state s { ['a', *] -> write ['_', -] return; [*, *] -> return; }
}
machine {
  tape x: ab;
  tape y: ab;
  entry state go { [*, *] -> call r(t = x, u = y) then stop; }
}
";

const UTP_NEAR: &str = "\
alphabet ab { '_', 'a' }
routine leaf(tape v: ab) {
  entry state s { ['a'] -> write ['_'] return; [*] -> return; }
}
routine byname(tape t: ab, tape u: ab) {
  entry state s { ['a', *] -> call leaf(v = u) then done; [*, *] -> return; }
  state done { [*, *] -> return; }
}
routine bymove(tape t: ab, tape u: ab) {
  entry state s { ['a', *] -> move [., >] return; [*, *] -> return; }
}
routine transparent(tape t: ab, tape u: ab) {
  entry state s { [*, *] -> call std::binaryNumbersBare::plusOne() then return; }
}
machine {
  tape x: ab;
  tape y: ab;
  entry state go { [*, *] -> call byname(t = x, u = y) then b; }
  state b { [*, *] -> call bymove(t = x, u = y) then c; }
  state c { [*, *] -> call transparent(t = x, u = y) then stop; }
}
";

const UFV: &str = "\
alphabet digit { '_', '0', '1', '2' }
machine {
  tape t: digit;
  entry state go { ['1' as x] -> write ['0'] move [>] goto go; [*] -> stop; }
}
";

const UFV_NEAR: &str = "\
alphabet digit { '_', '0', '1', '2' }
machine {
  tape t: digit;
  tape u: digit;
  entry state go {
    [0..1 as x, 0..2 as y] -> write [{x + 1}, {y}] move [>, >] goto go;
    [*, *] -> stop;
  }
}
";

const INW: &str = "\
alphabet digit { '_', '0', '1', '2' }
routine r(tape u: digit writes { '0', '1' } never writes { '2' }) {
  entry state e { ['1'] -> write ['0'] return; [*] -> return; }
}
machine {
  tape t: digit;
  entry state go { [*] -> call r(u = t) then stop; }
}
";

const INW_NEAR: &str = "\
alphabet digit { '_', '0', '1', '2' }
routine both(tape u: digit writes { '0', '1', '2' } never writes { '2' }) {
  entry state e { ['1'] -> write ['0'] return; [*] -> return; }
}
routine only(tape u: digit never writes { '2' }) {
  entry state e { ['1'] -> write ['0'] return; [*] -> return; }
}
machine {
  tape t: digit;
  entry state go { [*] -> call both(u = t) then b; }
  state b { [*] -> call only(u = t) then stop; }
}
";
```

Each rule-file test uses the `unused_set.rs` shape:

```rust
fn findings(src: &str) -> Vec<Diagnostic> {
    lint(src, LintOptions::default())
        .unwrap()
        .diagnostics
        .into_iter()
        .filter(|d| d.code == "unused-tape-parameter")
        .collect()
}

/// Mutation: a `check` that never pushes.
#[test]
fn an_untouched_tape_parameter_is_reported() {
    let ds = findings(UTP);
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!((ds[0].span.start.line, ds[0].span.start.col), (2, 28));
    assert_eq!(ds[0].message, "tape parameter `u` of `r` is never read, written, moved, or passed on");
    assert!(ds[0].fix.is_none());
}
```

(Import `lint`, `LintOptions` and `Diagnostic` the way `unused_set.rs`'s
tests do.) `lint_rule_sweeps.rs` copies `repo_root`, `corpus` and `collect`
from `tests/unused_set_sweep.rs` verbatim and has one test per code with its
body:

```rust
fn offenders(code: &str) -> Vec<String> {
    let corpus = corpus();
    assert!(corpus.len() >= 5, "the corpus roots resolved to almost nothing: {corpus:?}");
    let mut out = Vec::new();
    for path in &corpus {
        let source = fs::read_to_string(path).expect("a readable source");
        let Ok(report) = lint(&source, LintOptions::default()) else { continue };
        for d in report.diagnostics.iter().filter(|d| d.code == code) {
            out.push(format!("{}:{}:{}: {}", path.display(), d.span.start.line, d.span.start.col, d.message));
        }
    }
    out
}

/// Mutation: drop the transparent-reuse rule — rpn.tmc's four adapters fire.
#[test]
fn no_shipped_tmc_reports_unused_tape_parameter() {
    let found = offenders("unused-tape-parameter");
    assert!(found.is_empty(), "{}", found.join("\n"));
}
```

The `inert-never-writes` sweep's doc says it is vacuous today (the four
`never writes` clauses in the corpus carry no `writes`), a regression guard
only.

**Docs.** `docs/tmt/lint.md`, in `## The .tmc rules` after `### unused-tape`:

- `### unused-tape-parameter` — "A `routine` or `graph` tape parameter its
  body never reads, writes or moves, and never passes on to a `call`,
  `graft` or `bind` — the definition `unused-tape` uses for a machine tape. A
  body that reuses another unit with no argument list (a transparent call)
  binds tapes by position and counts as touching every one. It fires
  regardless of `export`: nobody else can use a parameter. Example line
  ``b.tmc:2:28: lint: tape parameter `u` of `r` is never read, written, moved, or passed on``.
  `fix: None` — removing a parameter changes the signature every caller
  binds."
- `### unused-fold-variable` — "A pattern binding (`as NAME`) its rule's
  write vector never reads — a read inside an arithmetic substitution
  (`{x + 1}`) counts. Example line
  ``b.tmc:4:28: lint: pattern binding `x` is never read by this rule's write vector``.
  `fix: None` — a binding may be kept for readability."
- `### inert-never-writes` — "With both `writes` and `never writes` written,
  a glyph `never writes` names but `writes` does not changes nothing: the
  effective set is `writes` minus `never writes`. Reported per element.
  Source only — a header publishes each tape's effective set as one `writes`
  clause and never prints `never writes`. Example line
  ``b.tmc:2:60: lint: '2' in `never writes` is not in `writes`, so excluding it changes nothing``.
  `fix: None`."
- `### unused-tape`: add "An argument-less `call`, `graft` or `bind` into
  another unit binds the machine's tapes by position, so a machine making one
  touches every tape."

**Steps.**
- [ ] Rule-file tests and the sweep file; see 1, 3, 5, 6, 8 red; 2, 4, 7, 9
  green.
- [ ] Implement the transparent rule first; see 1 go green; commit it alone:
  `fix(turing-machine): a transparent call into another unit touches every tape`.
- [ ] Implement the three rules and their registry rows; docs headings
  (`lint::docs_drift::the_tmc_rule_headings_match_the_registry_both_directions`
  must pass).
- [ ] Mutations per table; the sweep with transparent handling dropped
  reports exactly rpn's four adapters (record them).
- [ ] Gate: `cargo nextest run -p mtc-turing-machine -j 4` once. Tests that
  assert an EMPTY diagnostics list from `lint` over a fixture with an unread
  binding, an untouched parameter or a both-clauses tape would flip — fix the
  FIXTURE (read the binding, touch the tape) and list each in the report.
- [ ] Commit: `feat(turing-machine): unused tape parameters, unread bindings and inert never-writes entries are lints`.

---

### Task 12: Exported alphabets, maps and sets are never "unused"

**Property.** `unused-alphabet`, `unused-map` and `unused-set` skip a
declaration marked `export` — another unit may import it through the header,
and the deletion fix would remove something a consumer needs — while a
non-exported unused declaration is still reported with its fix.
`unused-tape`, `unused-graft-name` and `unused-exit` do not change.

**Files:**
- Modify: `crates/turing-machine/src/lint/rules/unused_alphabet.rs`,
  `unused_map.rs`, `unused_set.rs` (the skip; the module docs that call the
  rules export-independent)
- Modify: `crates/turing-machine/src/lint/mod.rs` (the `HEADER_UNUSED_SET`
  fixture of `a_body_rule_construct_in_a_header_is_silent`: `export set ones`
  → `set ones`, and its doc)
- Modify: `docs/tmt/lint.md` (`### unused-alphabet`, `### unused-map`,
  `### unused-set` paragraphs; "Linking a header" sentence on `unused-*`),
  `CLAUDE.md` (the bullet "`unused-alphabet`/`unused-tape` and
  `unused-graft-name`/`unused-exit` are export-independent by design.")
- Test: the three rule files' tests; `crates/turing-machine/tests/lint_rule_sweeps.rs`

**Interfaces:**
- Consumes (Task 4): `ResolvedAlphabet::exported` (own alphabets carry the
  source's flag; alphabets copied in from other modules carry `false` and are
  "used" by construction). `ResolvedSet::exported` and
  `ResolvedMapDecl::exported` exist already.
- Produces: nothing new.

**Falsifying tests.**

| # | Fixture | TODAY (`tmt lint --no-config`) **[verified]** | AFTER | Test | Mutation |
|---|---|---|---|---|---|
| 1 | `EXP` below | ``exp.tmc:2:17`` alphabet `deadA`; ``3:10`` alphabet `deadB`; ``4:12`` set `deadS`; ``5:5`` set `deadT`; ``6:12`` map `deadM`; ``7:5`` map `deadN` | only `deadB` (3:10), `deadT` (5:5), `deadN` (7:5), each still with a fix | `unused_alphabet.rs::tests::an_exported_alphabet_is_not_reported`, `unused_set.rs::tests::an_exported_set_is_not_reported`, `unused_map.rs::tests::an_exported_map_is_not_reported` (each asserts its own two lines) | drop the `exported` skip in that rule (the exported one returns) |
| 2 | `HEADER_UNUSED_SET` with `set ones` | the positive control fires | still fires (non-exported) | `lint/mod.rs::a_body_rule_construct_in_a_header_is_silent` (fixture edited) | — |
| 3 | corpus | — | zero `unused-alphabet`/`unused-map`/`unused-set` | extend `lint_rule_sweeps.rs` with the three codes (the `unused-set` one already has `tests/unused_set_sweep.rs`; do not duplicate it) | — |

```rust
const EXP: &str = "\
alphabet digit { '_', '0', '1' }
export alphabet deadA { '_', 'x' }
alphabet deadB { '_', 'y' }
export set deadS { '0' }
set deadT { '1' }
export map deadM: digit -> digit { '0' -> '1', '1' -> '0' }
map deadN: digit -> digit { '0' -> '1', '1' -> '0' }
machine {
  tape t: digit;
  entry state go { ['0'] -> write ['1'] stop; [*] -> stop; }
}
";
```

**Docs.**
- `docs/tmt/lint.md` `### unused-alphabet`: replace the "Unlike
  `unused-routine`/`unused-graph`, an **exported** alphabet is flagged too…
  silence it with `--allow unused-alphabet`…" passage by: "Like
  `unused-routine`/`unused-graph`, an **exported** alphabet is never
  reported: another unit may import it through this unit's header or sibling
  source (`docs/tmt/language.md (alphabets, maps and graphs across units)`),
  which this rule cannot see, and the deletion fix would remove what a
  consumer needs." `### unused-map` and `### unused-set`: the same sentence
  in place of "Export-independent, exactly as…".
- "Linking a header": keep the statement that the `unused-*` rules are off on
  a header; add "(an exported declaration is never reported in a source
  either)".
- `CLAUDE.md`: "**`unused-tape`, `unused-graft-name` and `unused-exit` are
  export-independent by design; `unused-alphabet`, `unused-map` and
  `unused-set` skip exported declarations.**"

**Steps.**
- [ ] Tests; the exported halves red.
- [ ] Implement the three skips; module docs.
- [ ] Mutations; docs.
- [ ] Gate: `cargo nextest run -p mtc-turing-machine -j 4` once (the
  `lint_fix_comment_guard.rs` / `lint_quickfix_comments.rs` fixtures are
  non-exported — no flip expected).
- [ ] Commit: `feat(turing-machine): an exported alphabet, map or set is never reported unused`.

---

### Task 13: A graph's digest covers what its body names, and every spliced graph is recorded

**Property.**
- A graph's digest changes when the members of a glyph set named in one of
  its pattern cells change, or the pairs of a named map at a
  `graft`/`bind`/`call` argument inside it change — on the exporter's side
  and the consumer's side alike — so the link reports graft drift. A body
  that names no set and no named map digests exactly as today (every
  standard-library digest and `std.tmh` stay byte-identical).
- The consumer records EVERY library graph a splice pulled in — whatever
  module it came from, exported or not, nested at any depth — each once, with
  the digest of the body spliced. Today a graph nested from the SAME library
  module (`relay` → `walker`) is not recorded (Established fact 10).
- A library's interface section records every graph its header PRINTS — an
  exported graph (`exported: true`) and every graph an exported body reaches
  (`exported: false`, a PRIVATE record, spelled `.graph NAME, D, private`) —
  so the drift check can compare any recorded graft by name. Private records
  never conflict (Task 3): two libraries with differing private graphs under
  one name link together, and a consumer's graft of one is checked against
  the private records of that name, passing when any provider matches.

**Files:**
- Modify: `crates/turing-machine/src/header.rs` (`graph_digest`; new
  `printed_graph_names`, factored out of `render_source`'s
  `referenced_graphs` walk and used by it; `exported_graph_digests` renamed
  `interface_graph_digests` and driven by `printed_graph_names`)
- Modify: `crates/turing-machine/src/compiler.rs` (the one call of
  `exported_graph_digests` in `compile`)
- Modify: `crates/turing-machine/src/expand.rs` (`expand_grafts_into` and its
  callers: record a spliced graph whenever its owning module is not the unit
  being compiled — threaded as a `root: &Resolved` pointer compared with
  `std::ptr::eq` against the `graph_owner` it resolves)
- Modify: `docs/tmt/language.md` (delete the "The digest covers what the body
  SPELLS, which has one limit worth knowing…" paragraph; one sentence saying
  what the digest covers; the graft-drift sentence in "### Grafting a graph
  from another unit" / the drift bullet near "A grafted graph's body is
  digested on both sides"), `docs/formats.md` (`.graph` lines: "every graph
  the unit's header prints", not "every exported graph"; `.grafted` lines:
  "every library graph spliced, nested ones included")
- Test: `crates/turing-machine/tests/library_grafts.rs`

**Interfaces:**
- Consumes (Task 4/5): `settle_declarations` and `own_forms` call
  `stamp_graph_digests` → `graph_digest`, so the own-definition check and
  every declarations read pick the digest change up without edits.
  (Task 4): `header::map_pair_list_text` is `pub(crate)`.
  (Task 5): `MEMO_LIB_TMC`, the rewritten memo tests.
  (Task 3): `ExportedGraph::exported`, the `, private` spelling, and the
  drift arm for private records — this task is the first producer of a
  private record.
- Produces:
  ```rust
  // header.rs — signature UNCHANGED:
  pub(crate) fn graph_digest(graph: &Graph, world: &ResolvedWorld, resolved: &Resolved, footprint: &FootprintTable) -> u32;
  /// Every graph this unit's header prints: each exported graph and, by
  /// mangled name, every local graph an exported body reaches through its
  /// grafts, transitively (docs/tmt/language.md (headers)).
  pub(crate) fn printed_graph_names(program: &Program, resolved: &Resolved) -> BTreeSet<String>;
  /// `.graph` records for `interface_graph_digests(program, resolved, footprint)`:
  /// every printed graph with its digest.
  pub(crate) fn interface_graph_digests(program: &Program, resolved: &Resolved, footprint: &FootprintTable) -> Vec<ExportedGraph>;
  ```

**Implementation.**
1. **Digest content.** The content is on the resolved world already, on both
   sides: a set cell carries `PatternCellKind::SetRef { resolved:
   Some(SetCellMembers { set, members }), .. }` with members expanded (a set
   built from sets is flattened, so "transitively" costs nothing), and a
   named map argument carries `SymMap { named: Some((mangled, _)), pairs, .. }`
   with its pairs filled by `expand_named_maps`. In `graph_digest`:

   ```rust
   let mut text = graph_body_lines(graph, world, resolved, footprint).join("\n");
   // The content a body names only by name (docs/tmt/language.md (grafting a
   // graph from another unit)): every glyph set a pattern cell names and every
   // named map an argument names, each once, by qualified name — so a change to
   // a member or a pair moves the digest. Nothing is appended for a body that
   // names neither, which keeps every such digest where it was.
   let mut named: BTreeMap<String, String> = BTreeMap::new();
   for state in &world.states {
       for rule in &state.rules {
           for cell in &rule.pattern.cells {
               if let PatternCellKind::SetRef { resolved: Some(m), .. } = &cell.kind {
                   let members: Vec<String> =
                       m.members.iter().map(|g| crate::patterns::label_spelling(&g.label)).collect();
                   named.insert(m.set.clone(), format!("set {} {{ {} }}", m.set, members.join(", ")));
               }
           }
       }
   }
   let mut take_args = |args: &[BindingArg]| {
       for a in args {
           if let BindingValue::Named { map: Some(m), .. } = &a.value
               && let Some((name, _)) = &m.named
           {
               let open = if m.open.is_some() { ", *" } else { "" };
               named.insert(name.clone(), format!("map {} {{ {}{open} }}", name, map_pair_list_text(&m.pairs)));
           }
       }
   };
   for g in &world.grafts { take_args(&g.args); }
   for b in &world.binds { take_args(&b.args); }
   for c in &world.calls {
       if let ResolvedCallTarget::Routine { args, .. } = &c.target { take_args(args); }
   }
   if !named.is_empty() {
       text.push('\n');
       text.push_str(&named.into_values().collect::<Vec<_>>().join("\n"));
   }
   crc32(text.as_bytes())
   ```
   (`SetMember { label: String }` and `SymMap { open: Option<Span>, .. }` are
   the field names today.)
2. **Exporter records every printed graph.** Factor `render_source`'s
   `referenced_graphs`/`graph_frontier` walk into `printed_graph_names`
   (exported graphs plus the local graphs they reach, transitively; an
   external target is not this unit's to print, as today) and call it from
   `render_source`. `interface_graph_digests` iterates `program.graphs`
   filtered by membership in `printed_graph_names` (not by `exported`), with
   the same `graph_digest`, and sets `ExportedGraph::exported` to the
   graph's own `exported` flag; codegen then prints `, private` for the
   private ones (`codegen.rs` near its `.graph` line). `compile` calls it in place of
   `exported_graph_digests`. The stdlib prints only exported graphs (its
   graphs are all exported), so its object and `STD_GRAPH_DIGESTS` do not
   move.
3. **Consumer records every spliced library graph.** Today
   `expand_grafts_into` pushes `graft.target` into `spliced_external` only
   when the target misses the CURRENT owner's `graphs` map. Change the
   condition to "the module the graph is resolved from is not the unit
   being compiled": thread the root `&Resolved` (the one `expand::expand`
   was called with) into `expand_grafts_into` and the nested expansion that
   calls it, and push (deduplicated, first-encountered order) whenever
   `!std::ptr::eq(graph_owner, root)`. `compile`'s digest lookup
   (`find_external_graph(&ext_modules, name).and_then(|(_, w)| w.digest)`)
   already finds a non-exported graph of a declarations module (no
   `exported` filter) and every graph world of a declarations module has a
   stamped digest (`stamp_graph_digests` walks all of `program.graphs`), so
   it needs no change; after Task 5 a name has one published form, so the
   first match is the spliced one.

**Falsifying tests.** Fixtures are the survey's `lib.tmc`/`cons.tmc` shape:

```rust
/// Exports `walker` (its cells name the non-exported set `hid` and the
/// exported set `vis`) and `relay` (its body grafts `walker` through the
/// non-exported named map `hmap`). `HID` and `HMAP` are substituted.
const DIGEST_LIB: &str = "\
export alphabet marks { '_', 'x', 'y', 'z' }
set hid { HID }
export set vis { 'z', '_' }
map hmap: marks -> marks { HMAP }
export graph walker(tape t: marks, state done) {
  entry state w {
    [hid] -> write ['z'] goto done;
    [vis] -> goto done;
    [*]   -> goto done;
  }
}
export graph relay(tape a: marks, state done) {
  entry graft walker(t = a with map hmap, done = done) as inner;
}
";

const GRAFTS_WALKER: &str = "\
use marks;
use walker;
machine {
  tape t: marks;
  entry graft walker(t = t, done = stop) as w;
}
";

const GRAFTS_RELAY: &str = "\
use marks;
use relay;
machine {
  tape t: marks;
  entry graft relay(a = t, done = stop) as r;
}
";
```

| # | Fixture | Command | TODAY | AFTER | Test | Mutation |
|---|---|---|---|---|---|---|
| 1 | `DIGEST_LIB` with `HID = 'x', 'y'`, `HMAP = 'x' -> 'z', 'z' -> 'x'` vs `HID = 'x'` (same `HMAP`) | compile each with `--nostdlib`, read `Interface::graphs` for `walker` | equal digests, `1096248304` both ways **[verified]** | different digests | `library_grafts.rs::a_set_member_change_moves_the_digest` | append nothing (today's digest) |
| 2 | same, `HMAP` changed from `'x' -> 'z', 'z' -> 'x'` to the 3-cycle `'x' -> 'z', 'z' -> 'y', 'y' -> 'x'`, `HID = 'x', 'y'`, graph `relay` | same | equal `relay` digests, `36002832` both **[verified]** | different | `library_grafts.rs::a_named_map_pair_change_moves_the_digest` | append sets only |
| 3 | exporter compiled with `HID = 'x'`; consumer `GRAFTS_WALKER` compiled against the header of the `'x', 'y'` version | link the two objects, `CallMech::Frames` | links, rc 0 (no drift) **[verified]** | `LinkError::GraftDrift { graph: "walker", .. }` | `library_grafts.rs::a_consumer_built_against_an_older_set_is_refused_at_link` | exporter and consumer computing the digest differently (one side appends, the other not) |
| 4 | consumer `GRAFTS_RELAY` against the header of the `'x', 'y'` version | its object's `grafts` | `[relay]` only (``.grafted relay, 36002832``) **[verified by the pre-flight]** | `[relay, walker]`, each with the header-read digest of the body spliced | `library_grafts.rs::a_graph_nested_from_the_same_library_is_recorded` | record only on a miss of the current owner's map (today's rule) |
| 5 | the same consumer linked with the `HID = 'x'` exporter | link, `CallMech::Frames` | links, rc 0 **[verified by the pre-flight]** | `LinkError::GraftDrift { graph: "walker", .. }` (`relay`'s own digest does not move: it names `walker` by name and its signature is unchanged) | `library_grafts.rs::a_set_change_inside_a_nested_graph_is_refused_at_link` | the nested record missing, or the exporter not recording `walker` |
| 6 | `MEMO_LIB_TMC` (Task 5; its `walker` is NOT exported) | `tmt compile mlib.tmc --nostdlib -S` | `.graph lib::relay, 3719903725` only **[verified]** | also `.graph lib::walker, D, private` (the header prints `walker`) | `library_grafts.rs::a_printed_private_graph_is_recorded_by_its_exporter` | `interface_graph_digests` filtering by `exported`; or recording it as exported |
| 9 | consumer `MEMO_CONSUMER` below (grafts `lib::relay`, which splices the private `walker`) against `MEMO_LIB_TMC`'s header; linked with `MEMO_LIB_TMC`'s object, then with `memo_lib_walker_edit()`'s | link, `CallMech::Frames` | both link today (the private `walker` is neither recorded nor checked) | first links; second `GraftDrift { graph: "lib::walker", .. }` — the edit adds a `['y'] -> goto done;` row to `walker`, which leaves `relay`'s digest at `3719903725` **[verified]**, so only `walker` can drift | `library_grafts.rs::a_change_inside_a_private_nested_graph_is_refused_at_link` | the private drift arm missing (the second link succeeds) |
| 10 | two libraries: `MEMO_LIB_TMC` and `OTHER_PRIVATE_WALKER_LIB` (its own private `lib::walker`, a different body, reached by its exported `lib::relay2`; compiles, `.graph lib::relay2, 2073867095` today **[verified]**); `MEMO_CONSUMER` against `MEMO_LIB_TMC`'s header | link consumer + both libraries, `CallMech::Frames` | links today | links: the two private `lib::walker` records do not conflict, and the consumer's `lib::walker` record matches the provider it came from | `library_grafts.rs::differing_private_graphs_of_two_libraries_link_and_match_their_provider` | private records fed into the conflict pass (`ConflictingExport`), or compared only against the first private record (the order `[OTHER, MEMO]` then drifts — link with that library order too) |
| 7 | the stdlib | `tests/header_fmt_clean.rs::the_stdlib_exporter_digests_are_unmoved`, `…::a_graft_through_the_committed_stdlib_header_records_the_unmoved_digest`, `tests/stdlib_header.rs::std_tmh_is_what_tmt_interface_prints` | green | green, unchanged pins (a consumer of a nesting std graph gains `.grafted` lines for the nested ones; the header test looks each record up by name) | (existing) | append a separator even when nothing is named (every std digest moves) |
| 8 | a graph naming NO set/map (`tests/library_grafts.rs`'s existing `lib::walker` fixtures) | `…::the_graft_digest_matches_the_exporters`, `a_doc_line_edit_does_not_move_the_digest` | green | green | (existing) | — |

**Why row 2's map change and not another.** `relay`'s printed signature
carries its tape's INFERRED write set, which is the preimage under `hmap` of
what `walker` writes (`'z'`). A map change that moves that preimage already
moves the digest today — `'x' -> 'y', 'y' -> 'x'` gives `3963371141`
**[verified]** — so it would not discriminate. The 3-cycle keeps the
preimage of `'z'` at `'x'` (the signature still prints `writes { 'x' }`) and
changes only pairs the body names by name.

Test 1 body:

```rust
/// Mutation: `graph_digest` appending nothing — the two exporters digest
/// `walker` identically again.
#[test]
fn a_set_member_change_moves_the_digest() {
    let dir = scratch("digest_set_members");
    let digest_of = |hid: &str, name: &str| -> u32 {
        let src = DIGEST_LIB.replace("HID", hid).replace("HMAP", "'x' -> 'z', 'z' -> 'x'");
        let path = write_file(&dir, &format!("{name}.tmc"), &src);
        let obj = dir.join(format!("{name}.tmo"));
        execute(&args(&["compile", path.to_str().unwrap(), "--nostdlib", "-o", obj.to_str().unwrap()]))
            .expect("compiles");
        let object = ObjectFile::from_bytes(&std::fs::read(&obj).unwrap()).unwrap();
        object.interface.unwrap().graphs.iter().find(|g| g.name == "walker").unwrap().digest
    };
    assert_ne!(digest_of("'x', 'y'", "wide"), digest_of("'x'", "narrow"));
}
```

Test 4 body:

```rust
/// A graph grafted inside another graph of the SAME library is spliced into
/// the consumer too, so it is recorded with the digest the consumer read.
/// Mutation: record a splice only when the target misses the current
/// owner's graph map — `walker` disappears from the list.
#[test]
fn a_graph_nested_from_the_same_library_is_recorded() {
    let dir = scratch("digest_nested_record");
    let lib = write_file(
        &dir,
        "lib.tmc",
        &DIGEST_LIB.replace("HID", "'x', 'y'").replace("HMAP", "'x' -> 'z', 'z' -> 'x'"),
    );
    let header = interface(&dir, &lib, "lib.tmh");
    let consumer = compile_extern(&dir, "relay_app", GRAFTS_RELAY, &header);
    let names: Vec<&str> = consumer.grafts.iter().map(|g| g.graph.as_str()).collect();
    assert_eq!(names, ["relay", "walker"]);
    let exporter = compile_alone(&std::fs::read_to_string(&lib).unwrap());
    for g in &consumer.grafts {
        assert_eq!(g.digest, graph_digest_of(&exporter, &g.graph), "{}", g.graph);
    }
}
```

Fixtures for rows 9 and 10:

```rust
const MEMO_CONSUMER: &str = "\
use lib::marks;
use lib::relay;
machine {
  tape t: marks;
  entry graft relay(a = t, done = fin) as r;
  state fin { [*] -> stop; }
}
";

/// `MEMO_LIB_TMC` with one more row in its private `walker`: `walker`'s
/// digest moves, `relay`'s does not (its inferred write set is unchanged).
fn memo_lib_walker_edit() -> String {
    MEMO_LIB_TMC.replace(
        "['x'] -> write ['y'] goto done;\n      [*]   -> goto done;",
        "['x'] -> write ['y'] goto done;\n      ['y'] -> goto done;\n      [*]   -> goto done;",
    )
}

/// Another library with its OWN private `lib::walker` (a different body),
/// reached only by its exported `lib::relay2`.
const OTHER_PRIVATE_WALKER_LIB: &str = "\
namespace lib {
  alphabet marks { '_', 'x', 'y' }
  map flip: marks -> marks { 'x' -> 'y', 'y' -> 'x' }

  graph walker(tape t: marks, state done) {
    entry state w {
      ['y'] -> write ['x'] goto done;
      [*]   -> goto done;
    }
  }

  export graph relay2(tape a: marks, state done) {
    entry graft walker(t = a with map flip, done = done) as inner;
  }
}
";
```

(Row 10 links `[consumer]` with libraries `[memo, other]` and again with
`[other, memo]`; both must succeed.)

(`library_grafts.rs` has `scratch`, `write_file`, `interface`,
`compile_extern`, `compile_extern_err`, `compile_alone`, `graph_digest_of`,
`graft_digest_of`; confirm `args`/`execute`/`ObjectFile`/`link`/`LinkError`
imports by symbol and add what is missing. `compile_alone` compiles against
`Declarations::none()`, which `DIGEST_LIB` needs nothing beyond.) Test 3: compile the
`'x', 'y'` exporter, print its header with `interface(&dir, &lib, "lib.tmh")`,
`compile_extern` `GRAFTS_WALKER` against it, `compile_alone` the `HID = 'x'`
exporter, and
`link(&[consumer, narrow], &[], LinkOptions { call_mech: CallMech::Frames, ..Default::default() })`
must be `Err(LinkError::GraftDrift { graph, .. })` with `graph == "walker"`.
Test 5: the same with `GRAFTS_RELAY`. Test 6: compile `MEMO_LIB_TMC` with
`compile_alone` and call `graph_digest_of(&lib, "lib::walker")` — the helper
panics on a missing record, which is the failure today.

**Tests that flip with this task** (dispositions):
- `library_grafts.rs::a_diamond_shaped_shared_target_is_not_a_cycle`
  asserts `grafts.len() == 1` and documents that `diamond_b/c/d` are not
  separate records. Under the recording rule they are: assert the recorded
  names, in first-encountered order, are exactly
  `["diamond_a", "diamond_b", "diamond_d", "diamond_c"]` (each once —
  `diamond_d`, reached twice, is recorded once), rewrite its doc to say
  so, and keep its property (a diamond is not a cycle: it compiles).
  Confirm the order by running it; the SET is the property, the order is
  first-encountered.
- `library_grafts.rs::a_consumers_own_graph_does_not_leak_into_a_librarys_nested_graft`
  and `a_librarys_nested_graft_does_not_leak_into_a_consumers_own_graph`
  (built on `MEMO_LIB_TMC` since Task 5): `grafts.len() == 1` becomes
  `grafts == [lib::relay, lib::walker]` where `lib::walker`'s digest equals
  `graph_digest_of(&lib, "lib::walker")` — the LIBRARY's walker, not the
  consumer's local one of the same name. That is the same property, stated
  more sharply; rewrite the "must not be recorded" message to "is recorded
  with the library's digest". The drift half of the first one still reports
  `GraftDrift { graph: "lib::relay" }` first (grafts are checked in
  recorded order, and the drifted `walker` body moves `relay`'s inferred
  signature too).
- No other test pins a consumer's `grafts.len()` (the pre-flight grep:
  `library_grafts.rs` 1258, 1320, 1371, 1453 only; 1371 already expects
  two). Consumer objects that graft a nesting std graph gain `.grafted`
  lines; no test byte-compares one (the pre-flight grep of
  `crates/*/tests`, `docs/examples` and `crates/turing-machine/src`).

**Docs.** `docs/tmt/language.md`: delete the "The digest covers what the body
SPELLS, which has one limit worth knowing…" paragraph and add to the
paragraph before it: "It also covers what the body names: the members of
every glyph set a pattern cell names and the pairs of every named map an
argument names, so changing either moves the digest." In the drift bullet
("A grafted graph's body is digested on both sides…"): "every graph a splice
pulls in is recorded, a graph nested inside another included, and a library
records every graph its header prints, so each is checked by name."
`docs/formats.md`: the `.graph`/`.grafted` sentences as in Files.

**Steps.**
- [ ] Tests 1–6, 9 red; 7, 8, 10 green today (10 must stay green); the
  three flipping tests red for the stated reason.
- [ ] Implement 1, then 2, then 3; after each, run
  `cargo nextest run -p mtc-turing-machine --test library_grafts`.
- [ ] Rewrite the three flipping tests; all green.
- [ ] Mutations per table.
- [ ] Gate: `cargo nextest run -p mtc-turing-machine -j 4` once; the
  compiled-stdlib byte identity at `-O0`/`-O1`
  (`tests/interface_emission.rs` stdlib tests) must hold — say so in the
  report with the test names; `cargo nextest run -p mtc-wasm -j 4` once.
- [ ] Commits: `feat(turing-machine): a graph's digest covers the sets and maps its body names`
  (implementation 1, tests 1–3) and
  `feat(turing-machine): every graph a splice pulls in is recorded and checked`
  (implementation 2–3, tests 4–6 and the three rewrites).
  Ledger: graph digests move for every graph that names a set or a named
  map; objects gain `.graph` lines for printed private graphs and `.grafted`
  lines for nested ones (unreleased formats; no migration).

---

### Task 14: The head-contract statements, and the whole-branch gate

**Property.** The language page states that `enters`/`leaves` constrain the
GLYPH under the head at entry and exit, not its position — and shows the
idiom that states "returns home" today. Then every standing gate is green on
the finished branch.

**Files:**
- Modify: `docs/tmt/language.md` ("#### Head-position clauses", right after
  the paragraph that ends "…in the alphabet's band order, duplicates
  collapsed ("Headers").")

**Text to add** (verify each claim against the built tool before
committing — the example must compile):

> `enters` and `leaves` constrain the **glyph** under the head, not the
> head's position: a routine that returns one cell short of where it
> entered satisfies them, provided the glyph there is one the clause names.
> To state "returns to the cell it started on", mark the home cell with a
> glyph nothing else on the tape carries and promise to leave on it:
>
> ```
> alphabet sym { '_', 'a', 'b', 'H' }
>
> ? Enters on the home marker and walks right; comes back and leaves on it.
> routine sweep(tape t: sym enters { 'H' } leaves { 'H' }) {
>   entry state out {
>     ['_'] -> move [<] goto back;
>     [*]   -> move [>] goto out;
>   }
>   state back {
>     ['H'] -> return;
>     [*]   -> move [<] goto back;
>   }
> }
> ```

**[verified]**: this routine plus `machine { tape t: sym; entry state go {
[*] -> call sweep(t = t) then stop; } }` compiles, rc 0, with the stdlib on.
Re-run it with the branch's final binary before committing; if a static check
objects, adjust the example — never the claim.

**The whole-branch gate** (run in this order, one at a time):
- [ ] `cargo fmt --check`
- [ ] `CARGO_TARGET_DIR=… cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `CARGO_TARGET_DIR=… cargo build -p mtc-core --no-default-features`
- [ ] `CARGO_TARGET_DIR=… cargo build --workspace --lib --target wasm32-unknown-unknown`
- [ ] `scripts/build-wasm-bundle.sh` (WITHOUT the target-dir prefix) then
  `node scripts/wasm-smoke.mjs target/wasm-bundle/dist`
- [ ] `CARGO_TARGET_DIR=… cargo nextest run --workspace -j 4` — the one
  workspace run of the plan. Compare the count with the Task 0 baseline:
  every difference is a test this plan added, deleted or renamed (list them).
- [ ] Standing gates named explicitly (they are inside the workspace run;
  cite their names in the report): PM-1 byte identity
  (`golden_programs`, `asm_volatile`); core neutrality (no TM/PM knowledge
  in the `crates/core` diff — `git diff origin/next -- crates/core` read by
  the reviewer); the everything-matrix
  (`opt_equivalence.rs::everything_matrix_is_green`), `mode_equivalence`,
  `link_matrix`; compiled-stdlib byte identity at both opt levels
  (`interface_emission.rs` stdlib tests); every drift guard named in Global
  Constraints.
- [ ] Re-run Task 0's shipped-build and lint sweeps with the final binaries;
  every file that built still builds; the lint findings equal the baseline
  (no new code fires on shipped files).
- [ ] Docs audit: `git grep -n 'first-wins\|shadow' -- docs README.md CLAUDE.md`
  — every remaining hit is about rows, imports or locals, not symbols or
  declarations; `git grep -nE '#[0-9]{2,3}\b|Task [0-9]|spec §' -- docs crates ':!docs/internal'`
  shows nothing this branch added.
- [ ] Commit (docs): `docs(turing-machine): head contracts constrain the glyph under the head, not its position`.

---
## Tests that flip (copy into the ledger in Task 0)

| Task | Test | Disposition |
|---|---|---|
| 1 | `crates/core/src/linker/resolve.rs`: every `LinkError::DuplicateSymbol("…".into())` assertion (`a_variant_pair_in_one_object_is_not_a_duplicate_symbol`, `only_a_normal_volatile_pair_shares_one_name_in_one_object`, `columns_of_one_name_never_span_two_objects`, and the user-vs-user half of `duplicate_user_symbols_error_but_library_shadowing_does_not`); every `resolve(…)` call gains `&[]` | variant shape only (`{ name, first, second }` with `input #i`) |
| 1 | `crates/turing-machine/src/cli/driver.rs::tests::link_library_objects_drops_a_header_only_entry` | asserts `(name, object)` pairs |
| 2a | `resolve.rs::libraries_resolve_lazily_and_users_shadow` | split: `a_reached_name_a_user_object_and_a_library_define_is_a_duplicate` + `libraries_resolve_lazily` |
| 2a | `resolve.rs::duplicate_user_symbols_error_but_library_shadowing_does_not` | replaced by `duplicate_user_symbols_error_even_unreached_and_library_duplicates_only_when_reached` |
| 2a | `resolve.rs::a_shadowing_normal_only_pair_forces_a_counted_fallback` | replaced by `a_second_input_offering_a_reached_name_is_a_duplicate_whatever_its_columns` |
| 2a | `resolve.rs::resolve_names_user_definition_shadows_library` | replaced by `resolve_names_refuses_a_reached_name_two_inputs_define` |
| 2a → 2b | PM `visibility_programs.rs::deliberate_interposition_via_namespace_injection`, `stdlib_programs.rs::user_namespace_injection_overrides_a_std_routine`, PM and TM `lsp/overlay.rs::faithfulness::{overlay_resolution_matches_linker_resolution_with_provenance, overlay_resolution_matches_linker_resolution_for_a_shadowed_std_name}` | red after 2a (the six named in 2a's scope), rewritten in 2b |
| 2b | PM `cli/driver.rs::tests::a_library_may_own_the_entry_and_supply_the_bit` | comment rewritten, assertions (about the helper) kept |
| 3 | `core/tests/link_checks.rs::a_user_export_shadows_a_library_export_for_the_check` | renamed, asserts `ConflictingExport` |
| 5 | `extern_declarations.rs::extern_order_is_command_line_order_then_stdlib` | renamed, both orders conflict (`--nostdlib`) |
| 5 | `build_derives_declarations.rs::a_peer_shadowing_a_stdlib_name_is_believed_over_the_builtin` | renamed, asserts `conflicting-declaration` |
| 5 | `library_grafts.rs::a_nested_library_graft_is_checked_against_the_graph_it_splices`, `…_missing_a_later_parameter_is_missing_arg` | `--nostdlib` (via `shadowed_nested_graft`'s new parameter); assertions unchanged; mutation note updated |
| 5 | `library_grafts.rs::a_colliding_local_alphabet_is_diagnosed_not_silently_captured`, `an_unrelated_colliding_local_alphabet_does_not_refuse_a_valid_graft` | library built from `ALPHA_COLLISION_LIB_TMC` (its `lib::marks` private: printed, not published); assertions unchanged |
| 5 | `library_grafts.rs::a_consumers_own_graph_does_not_leak_into_a_librarys_nested_graft`, `a_librarys_nested_graft_does_not_leak_into_a_consumers_own_graph` | library built from `MEMO_LIB_TMC` (its `walker` private); assertions unchanged until Task 13 |
| 5 | `turing-machine/src/lsp/tests.rs::a_shadowing_sibling_wins_over_the_stdlib_at_every_leg` | MAY flip; fixture takes the stdlib's exact signature |
| 8 | any test pinning a declared callee's missing STATE argument at the rule column | intended move to the first argument's column |
| 9 | `compiler.rs` tests matching `WrongTargetKind { name, .. }` | keep matching (new field `found`); any doc/test quoting "`X` is not a routine" text updates to "`X` is a graph, not a routine" |
| 10a | `core/tests/link_checks.rs::a_return_through_an_undeclared_exit_warns`, `an_undeclared_exit_merges_with_site_warnings_in_offset_order`; `core/tests/link_exits.rs::a_body_returning_through_an_undeclared_exit_is_refused`; `turing-machine/tests/link_warnings.rs::a_return_through_an_undeclared_exit_warns_on_the_cli` | fixtures rebuilt without the assembler; assertions unchanged |
| 10b | `link_checks.rs::a_return_through_an_undeclared_exit_warns` | replaced by `…_is_refused_under_every_mechanism` |
| 10b | `link_checks.rs::an_undeclared_exit_merges_with_site_warnings_in_offset_order` | deleted (the finding is no longer a warning) |
| 10b | `link_checks.rs::a_return_through_a_declared_exit_is_silent`, `a_trap_with_the_same_immediate_is_not_an_exit` | the `undeclared_exits` helper goes; assert the link succeeds |
| 10b | `link_exits.rs::a_body_returning_through_an_undeclared_exit_is_refused` | asserts `UndeclaredExit` under all three mechanisms |
| 10b | `link_warnings.rs::a_return_through_an_undeclared_exit_warns_on_the_cli` | replaced by the linker test over all mechanisms |
| 10b | `link_warnings.rs::no_shipped_program_returns_through_an_undeclared_exit` | counts refusals instead of warnings |
| 11 | any test asserting an EMPTY `lint` report over a fixture with an unread binding, an untouched parameter or a both-clauses tape | fix the fixture |
| 12 | `turing-machine/src/lint/mod.rs::tests::a_body_rule_construct_in_a_header_is_silent` | `export set ones` → `set ones` |
| 13 | `library_grafts.rs::a_diamond_shaped_shared_target_is_not_a_cycle` | asserts the four names `diamond_a`, `diamond_b`, `diamond_d`, `diamond_c`, each once |
| 13 | `library_grafts.rs::a_consumers_own_graph_does_not_leak_into_a_librarys_nested_graft`, `a_librarys_nested_graft_does_not_leak_into_a_consumers_own_graph` | `grafts == [lib::relay, lib::walker]`, `lib::walker` with the LIBRARY's digest (its private record) |
| 3 | every `ExportedGraph { .. }` literal in tests (`asm/lower.rs`, `asm/disassembler.rs`, `formats/object/tests.rs`) | gains `exported: true`; no behaviour change |

(`tmc_property.rs` never compiles — it parses and formats only — so Task 6
cannot flip it; the earlier "may flip" entry is withdrawn.)

---

## Pre-flight

### Pairs of tasks that touch the same file or interface

| Tasks | Shared surface | Producer says | Consumer uses | Agree? |
|---|---|---|---|---|
| 1 → 2a | `resolve.rs::resolve` | `resolve(objects, libraries, entry, names: &[Option<String>])` | 2a keeps the signature, adds `contenders` | yes |
| 1 → 2a, 3, 10b | `LinkError` | `DuplicateSymbol { name, first, second }` | 2a builds it; 3/10b add sibling variants `ConflictingExport`, `UndeclaredExit` | yes |
| 1 → 2a, 3 | `input_name(names: &[Option<String>], i) -> String` | `pub(super)` in `linker/mod.rs` | `resolve.rs` and `interface.rs` (both `linker` children) | yes |
| 1 → 3 | `check_graft_drift` / `check_imported_alphabets` | take `names: &[Option<String>]` | 3 rewrites their bodies, keeps the signatures | yes |
| 1 → 10b | `link` order | resolve → drift checks | 10b inserts `check_declared_exits` after the drift checks, before `validate_frame_phys` | yes |
| 1 → 2b, 3 | `one_definition.rs` helpers | `write`, `run`, `compile_ok`, `routine_trio`, `graph_trio`, consts `R1 R2 APP G1 G2 GAPP` (TM), `PA PB` (PM) | 2b adds `APP0`, `OWNSAME`, `OVERRIDE`, `PNEAR`; 3 adds `R3` | yes |
| 2a → 2b | the rule | reach-gated `DuplicateSymbol` | 2b's CLI tests and the six rewrites expect it | yes |
| 2b → 5 | TM overlay std-name fixture | 2b gives the sibling the stdlib's exact signature | 5's own-definition check lets it compile | yes |
| 2b → 5 | `OWNSAME` | link-refused in 2b (test 4) | compile-accepted in 5 (test 8) | yes |
| 3 → 13 | `ExportedGraph::exported`, `.graph …, private`, `settle_exports` over EXPORTED records, the private drift arm | 3 (no producer of a private record yet) | 13's `interface_graph_digests` emits private records; rows 9–10 exercise both arms end to end | yes |
| 4 → 5 | `published.rs` | `PublishedForm`, `ParamForm::State { name: Option<String> }`, `PublishedForm::agrees`, `module_forms`, per-item form fns | `check_conflicts`, `own_forms`, `check_own_against_table` compare only through `agrees` | yes |
| 4 → 5 | `Resolved::from_object` | set by `declarations_from_object` | `module_forms` (4); `own_forms` sets it on its clone from `Provenance` (5) | yes |
| 4 → 5 | `header::settle_declarations(program, &mut resolved, externals, mode)` | the one settle | `own_forms` settles a clone through it | yes |
| 4 → 5 | `U`, `UC` in `extern_declarations.rs` | 4's fixtures | 5's row 16 (`--extern u.tmc --extern u.tmh`) | yes |
| 4 → 5, 12 | `ResolvedAlphabet::exported` | set from the source, `false` when copied in | `module_forms` (4), `unused_alphabet` (12) | yes |
| 4 → 13 | `header::map_pair_list_text` | `pub(crate)` | `graph_digest` | yes |
| 4/5 → 13 | `stamp_graph_digests` → `graph_digest` | unchanged signature | 13 changes only its body; every settle path picks it up | yes |
| 5 → 13 | `MEMO_LIB_TMC` and the two memo tests | 5 rebuilds them on a private `walker`, `grafts.len() == 1` | 13 changes the assertion to the two records | yes |
| 5 ↔ 6 | `compiler.rs` enum/registry/witness order | `ConflictingDeclaration` after `DuplicateName`; `DuplicatePatternBinding` after `DuplicateBinding` | the witness list follows declaration order | yes |
| 5 ↔ 9 | `compiler.rs` `resolve_program`, lookups | 5 adds two calls and `Provenance`; 9 rewrites the lookups over `external_decls` | disjoint edits; 9's `find_external_*` keep their signatures | yes |
| 7 → 8 | `WorldCtx::check_reuse_targets` | 7 adds `refuse_return_outside_routine(args, world.kind)` at the top of each loop | 8 adds the external-call/bind shared check after it; `check_binding_list` untouched by both | yes |
| 8 → 9 | `find_external_routine(externals, name) -> Option<(&Resolved, &ResolvedWorld)>` | 8 | 9 re-implements over `external_decls` (same signature) and puts the graph-kind check IN FRONT of 8's argument-less exemption | yes |
| 9 | `DefKind`, `DefKind::noun`, `external_kind` | become `pub(crate)` | `ExternalDecl::kind`, `expand.rs` | yes |
| 6 → 11 | pattern bindings | a name bound twice in one rule is a compile error | `unused-fold-variable` may assume one binding per name per rule | yes |
| 10a → 10b | `SyntaxEntry::is_exit_return`; `declaring_exits`, `three_exit_callee`; rebuilt fixtures | 10a | 10b's link check and its assertions | yes |
| 11 → 12 | `tests/lint_rule_sweeps.rs` | created with `offenders(code)` and four tests | 12 adds `unused-alphabet`, `unused-map` tests (not `unused-set`: it has its own sweep) | yes |
| 2b, 5, 9, 10a, 10b, 12, 13 → 14 | docs | each task edits its pages | 14's audit greps them | yes |

### Each task's tests against its own implementation text

| Task | Check |
|---|---|
| 1 | Test 1's expected text equals the `Display` arm given; test 2's `input #1` equals `input_name`'s fallback; test 6's pairs match `link_library_objects`' new return type. |
| 2a | Every `resolve(..)` call in the tests passes `&[]`, so names read `input #i`; row 4's part 2 uses a Normal-only and a Volatile-only library (the per-column mutation's case); row 5 relies on the entry being in `index_of` (it is minted there); row 8 counts one input once. |
| 2b | Labels in expected texts follow `link_labels` (path as given; ``library `NAME` ``; `the standard library`). |
| 3 | Core tests use `exporter_b`/`private_exporter(tag, …)` so no symbol is contested; input numbering follows `objects` then `libraries`; rows 15–18 match the ruled rule (private: never a conflict, any-provider drift). |
| 4 | Row 4's mutation is reachable (`a5` exports an uncontracted routine); the second count comes from the AST, so an empty `module_forms` fails; row 7 pins `agrees`' three branches. |
| 5 | CLI conflicts render in the SECOND-read module's file (fixpoint); in-crate rows 10, 15 render at `0:0`; row 13 puts the text reading first so `done = …` binds; row 16 is a guard, green at Task 5's start, reddened only by Task 4's writes-step mutation. |
| 6 | The span is the second binding's `Binding::span`, `6:25` for `A1`. |
| 7 | The span is the terminator's own span, `9:51` for `B4`. |
| 8 | `MissingArg` on a bind at `args.first().span` (`7:10`); `WrongArgKind` at `arg.span` (`7:36`); rows numbered 1–5. |
| 9 | Rows 6–8 put the graph-kind check before the argument-less exemption (row 7 is argless); row 9 pins the local message. |
| 10a | The four rebuilt fixtures keep their assertions; the TM patched object equals what `tmt asm` produced before. |
| 10b | Every linker fixture is built by lowering a declared count; row 8's expected text matches the verified manifest message shape. |
| 11 | Spans: parameter `name_span` (`2:28`), binding span (`4:28`), clause element span (`2:60`). |
| 12 | Only exported declarations are skipped; the three non-exported ones keep their fixes. |
| 13 | Nothing is appended for a body naming neither kind (row 7 holds); row 2's map change keeps the inferred write set (verified); rows 5 and 9 edit only the nested graph, whose change leaves `relay`'s digest unchanged (verified), so the drift names `walker`; row 10 links in both library orders. |

---

## Spec coverage

| Spec section / amendment / ruling | Task |
|---|---|
| Goal; "What is in" (1)–(5) | 1–14 |
| §1 the rule; why a ban | 2a, 2b, 3, 5 |
| §1.1 user vs user: unchanged, always | 1 (named), 2a (row 3 keeps it) |
| §1.1 user vs library / library vs library, when reached | 2a (core), 2b (both toolchains) |
| §1.1 graph/alphabet exports: identical = one; different = error when consumed; always between two user objects | 3 |
| §1.1 "reached" = BFS; unrelated libraries sharing an unused name link | 2a (rows 2, 3), 2b (rows 3, 6) |
| §1.1 inside ONE library object first-record-wins stays | 2a (row 8) |
| §1.1 error names the symbol and both inputs | 1 |
| §1.1 escape = `--nostdlib` | 2b (docs) |
| §1.1 two-column rule; second input a duplicate whatever its columns | 2a (row 4) |
| §1.1 core arch-neutral, fake arch; PM-1 output unmoved | 1, 2a, 3, 10a, 10b (fake-dialect tests; PM gates) |
| §1.2 identical published forms merge (a unit's header and its source; one library by two paths) | 5 (rows 3, 13, 16) |
| §1.2 different forms → `conflicting-declaration` naming both origins | 5 (rows 1, 2, 10, 15) |
| §1.2 published form per kind; doc lines/comments excluded; table order decides nothing | 4 (definition, `agrees`), 5 (row 17, use) |
| §1.2 own definition vs table; std redefinition; own up-to-date header fine; stale header refused | 5 (rows 4–9) |
| §1.2 guard: every shipped unit's source vs header declarations identical | 4 (row 4) |
| §1.3 `.pmc` link-only, grammar unchanged | 2b (PM tests), Versions constraint |
| §1.4 baseline; the three core tests to rewrite | 0, 2a |
| §2.1 `duplicate-pattern-binding` | 6 |
| §2.2 `return` to a state parameter outside a routine; stray warning root-caused | 7 |
| §2.3 one argument check; `wrong-arg-kind`; bind points at the bind; no declarations unchanged | 8 |
| §2.4 `undeclared-exit` at assembly | 10a |
| §2.4 at link; leaves the warning table; `--allow` no longer applies; reuses the predicate | 10b |
| §2.5 wrong-kind names across units; one shared lookup; codes unchanged | 9 |
| §3.1 `unused-tape-parameter`, `unused-fold-variable` | 11 |
| §3.2 `inert-never-writes`, source only | 11 |
| §3.3 exported alphabets/maps/sets exempt; docs and standing notes rewritten | 12 |
| §4 digest covers set/map content; limit deleted | 13 |
| §5 the two documentation statements | 14 |
| Amendment §1.1 naming both inputs (libraries and stdlib labelled) | 1 |
| Amendment §1.1 "Overriding std" removed; overlay faithfulness tests updated | 2b |
| Amendment §1.2 a source read publishes what its header publishes | 4 |
| Amendment §1.2 where a conflict is reported | 5 (Established fact 7 + rows 10, 15) |
| Amendment §2.1 the code is `duplicate-pattern-binding` | 6 |
| Amendment §2.2 every state argument: graft in a machine, graft in a graph body, unused bind | 7 |
| Amendment §2.3 the all-terminator bypass closes | 8 (row 2) |
| Amendment §2.5 third site: grafting a declared routine | 9 (row 3; plus the nested-graft splice site) |
| Amendment §2.4 unconditional link scan; `--allow` and manifest `lint.allow` → unknown code; tests construct objects directly | 10a (fixtures), 10b (rows 6–8) |
| Amendment §3.1 transparent call touches every tape; "fold variable" = pattern binding | 11 |
| Amendment §3.3 alphabets gain the export flag | 4 (field), 12 (use) |
| Amendment §4 the stdlib does not move | 13 (row 7) |
| Ruling 1 — state names, `None` from an object, count fallback | 4 (`ParamForm`, `agrees`, `from_object`, row 7), 5 (`Provenance`, rows 13–15) |
| Ruling 2 — private own definitions compared | 5 (implementation 2; the four `library_grafts` rewrites; 3b's published boundary) |
| Ruling 3 — every spliced library graph recorded; exporters record every printed graph | 13 (implementation 2–3, rows 4–6, the three flips) |
| Ruling 4 — a `call`/`bind` naming a declared graph is `wrong-target-kind`, message names the kind | 9 (rows 6–9) |
| Ruling 5 — spelling-sensitive map digests accepted | 13 (no change needed) |
| Ruling 6 (revised) — private graph records never conflict; they serve drift, any provider matching | 3 (flag, spelling, rule, rows 14–18), 13 (producer, rows 6, 9, 10) |
| Ruling 7 — the fixpoint-order limit accepted | 4 (recorded), Decided 17 |
| Versions | Global Constraints |
| Testing and gates; order of work | Global Constraints, 0, 14; task order follows the spec's |

---

## Open questions

### Decided in the plan

1. **Link labels are a new field** (`LinkOptions::labels`), never mixed into
   `sources`, which feeds the map sidecar and a debugger's openability.
2. **Label wording**: a path as given; ``library `NAME` ``; `the standard
   library`; unlabelled inputs fall back to their `sources` entry, then to
   `input #i`. One input defining a name twice reads "defined twice by X".
3. **Order of link refusals**: `Unresolved` first (as today), then the
   reach-gated duplicate check, names sorted; export conflicts before drift;
   the undeclared-exit check after the drift checks and before the engine.
4. **Differing exports nobody consumes** (between a library and anything)
   link and are left unchecked, the header-only standing.
5. **Published form**: tape glyph lists, not alphabet names; state-parameter
   names included when both readings carry them, compared by position where
   an object reading carries none (ruling 1); `volatile`/`opaque` excluded;
   `never writes` folded; a module publishes its EXPORTED declarations; the
   unit's own definitions are compared whatever their export status
   (ruling 2); a name a header prints but does not publish is outside the
   rule.
6. **An identical redefinition of a `std::` routine compiles** and is refused
   at link — the same comparison that lets a unit compile beside its own
   up-to-date header.
7. **A conflict renders against the later-read module's file** (the fixpoint
   reads each module against its read peers), phrased "by this unit and by
   X"; the compile-wide table-versus-table conflict renders at `FILE:0:0`,
   the existing precedent for compile-wide errors.
8. **`return` in ANY argument position outside a routine** is
   `return-outside-routine`, checked before any signature.
9. **The transparent (argument-less) cross-unit call or bind is exempt** from
   the shared argument check — but not from Task 9's kind check.
10. **`duplicate-pattern-binding` lives in `check_rules`**, pointing at the
    second binding's name; the message names the state (the rule is located
    by the span).
11. **`undeclared-exit`** keeps the warning's message text for both the
    assembler and the link error; its catalog capability is `tables`. The
    fixtures that need such a body are rebuilt in 10a (so 10a ends green),
    their assertions changed in 10b.
12. **Lint spans**: a parameter's name; a binding's name; a `never writes`
    element. No fixes.
13. **Digest content** is read off the resolved world (expanded set members,
    filled named-map pairs), appended as `set NAME { … }` / `map NAME { … }`
    lines in qualified-name order only when non-empty.
14. **Wrong-kind lookups** also cover the nested-graft miss at the splice
    (`expand.rs`), not only the sites the spec names.
15. **`WrongTargetKind` gains `found`**, and the local message changes with
    the cross-unit one ("`g` is a graph, not a routine"); the code stays.
16. **`DefKind`, `DefKind::noun` and `external_kind` become `pub(crate)`** so
    `ExternalDecl::kind()` passes the `private_interfaces` lint.
17. **Known limit, recorded not fixed**: two readings of one unit in one table
    can disagree on an inferred write set depending on fixpoint order (Task 4).

### Ruled by the controller (2026-09-30) — these bind the tasks named

1. **State-parameter names in the published form** (Tasks 4, 5):
   `ParamForm::State { name: Option<String> }`, `None` for a reading taken
   from an object (`Resolved::from_object`, `Provenance::Object`), compared by
   name where both are `Some` and by position otherwise, through
   `PublishedForm::agrees` — never derived `==`.
2. **Private own definitions** (Task 5): compared whatever their export
   status. The four `library_grafts.rs` tests that collided keep their
   properties by making the colliding library name private (printed, not
   published).
3. **Every spliced library graph is recorded** (Task 13): the consumer
   records every library graph a splice pulled in — from any module,
   exported or not, nested at any depth — with the digest of the body
   spliced; a library's interface section records every graph its header
   prints; the drift check compares them by name.
6. **Private graph records** (revised, Tasks 3 and 13): a graph record says
   whether the graph is exported. The export-conflict check considers
   EXPORTED records only; a private record never conflicts with anything,
   between any pair of inputs — a private graph is not a definition in the
   shared namespace, as a local symbol is neither exported nor shadowable.
   Private records serve the drift check only: a graft of a graph no input
   exports must match at least one input's private record of the name; no
   record at all is unchecked, as today.
7. **The fixpoint-order limit** (Task 4, Decided 17): accepted as a known
   limit.
4. **A `call`/`bind` naming a declared GRAPH** (Task 9): `wrong-target-kind`,
   the message naming the kind found; `WrongTargetKind` gains the kind, the
   local message reads the same way; the code stays.
5. **Spelling-sensitive map digests** (Task 13): accepted.

### For the maintainer

None open.

---

## Self-review

1. **Spec coverage** — every section, amendment and ruling maps to a task
   (table above); no gap.
2. **Placeholder scan** — no "TBD"/"similar to Task N"; every test body
   described as "the same body with fixture X" has its fixture, command and
   exact expected text in that task's table. `three_exit_callee` is defined
   in 10a.
3. **Type consistency** — names used across tasks (`input_name`,
   `LinkOptions::labels`, `DuplicateSymbol { name, first, second }`,
   `ConflictingExport { what, name, first, second }`,
   `UndeclaredExit { routine, exit, declared }` (link and asm),
   `PublishedForm`, `ParamForm::State { name: Option<String> }`,
   `PublishedForm::agrees`, `Resolved::from_object`, `Provenance`,
   `module_forms`, `settle_declarations`, `own_forms`,
   `check_own_against_table`, `ConflictingDeclaration { name, first, second }`,
   `DuplicatePatternBinding { name, first, second, state }`,
   `refuse_return_outside_routine`, `find_external_routine`, `ExternalDecl`,
   `external_decls`, `external_kind`, `WrongTargetKind { name, expected, found }`,
   `makes_transparent_reuse`, `printed_graph_names`, `interface_graph_digests`,
   `declaring_exits`, `three_exit_callee`) are defined once and used with the
   same signature everywhere (pre-flight table).
4. **Fixture verification** — every **[verified]** fixture was run through
   `target/debug/tmt`/`pmt` on 2026-09-30 (47 re-run by the pre-flight, all
   matching; the new ones added in this revision — the two collision
   libraries, the private-graph exporter, the manifest `lint.allow`, the
   source-plus-header compile — run by the planner). Not run: the core
   fake-dialect assembly sources (there is no CLI for the fake dialect);
   their TM-1 twins were run where a twin exists (10a's `sub2.tma`, 10b's
   `main_plain.tma`), and each core test's TODAY column says so.
