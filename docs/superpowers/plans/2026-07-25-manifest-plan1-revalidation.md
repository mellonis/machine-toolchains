# Manifest Plan 1 — re-validation against master (2026-07-25)

Audit of `docs/superpowers/plans/2026-07-12-manifest-plan1-schema-and-build.md`
(8 tasks, 46 steps, 0 done) against master `ddb4d24`, after the TM-1/tmt arc,
the #31 range round, the pre-release polish, and the hygiene sweep all landed —
and after the driving spec
`docs/superpowers/specs/2026-07-12-project-manifest-and-build-design.md` was
amended 2026-07-21 for shipped TM-1.

**Verdict: rewrite, don't execute.** Nothing in the plan is wrong about PM-1,
but one task is already shipped, the docs task's anchors all moved, one
task-ordering assumption no longer holds, and every task needs a TM twin the
plan never contemplated.

## Per-task verdicts

| Task | Verdict | Evidence on master |
|---|---|---|
| 1. `LinkOptions.entry` in core | **obsolete** | Shipped by TM phase 5. `crates/core/src/linker/mod.rs:183` — `LinkOptions { relax, entry: Option<String>, call_mech }`. Delete the task; keep its `LinkError::NoEntrySymbol` assumption as a check. |
| 2. `project.rs` schema + validation | **anchors valid · needs TM twin** | No `project.rs` in either crate. `ConfigError` / `ProjectConfig` / `discover` / `load` are all `pub(crate)` in `crates/post-machine/src/config.rs`; `optimizer::OptLevel` and `lint::validate_allow` exist in both crates. Spec §168 puts the twin at `crates/turing-machine/src/project.rs`. |
| 3. One loader + per-section discovery | **anchors valid · needs TM twin** | `crates/turing-machine/src/config.rs` is a 356-line near-duplicate of PM's 426-line one (same `lint.allow`-only schema, same nearest-ancestor walk, same union merge). "One loader for the whole file" is now **two** loaders, one per crate — spec §168 rules core stays arch-agnostic and holds no manifest knowledge. |
| 4. `cli/driver.rs` argv mode | **anchors valid · needs TM twin** | No `cli/driver.rs` in either crate. All seven helpers the task wants widened to `pub(super)` are still present as private `fn` in `crates/post-machine/src/cli/build.rs` (`out_path` :35, `render_warnings` :42, `render_opt_report` :55, `take_disabled_passes` :175, `sidecar_path` :290, `read_object` :296, `find_library` :301). `asm::assemble(source, with_debug)`, `asm::link(objects, libraries, LinkOptions)`, `stdlib::object()` intact in both crates. TM's `cli/build.rs` additionally carries `parse_call_mech` (:274) that manifest mode must reuse rather than re-parse. |
| 5. Manifest mode + `--list-targets` | **anchors valid** | Depends only on Tasks 2–4. `LinkOptions.call_mech` already exists, so the §250 ruling (target-level key + project default, flag overrides, accepted in manifest mode) has a field to thread with no core change. |
| 6. `--run` + run.rs split | **anchors valid · needs TM twin** | Neither crate has the settings/execution split: PM `cli/run.rs` is `run()` :44 + `trace_to`/`parse_profile`/`initial_tape`/`drive`; TM's is `run()` :46 + `drive_traced` :193. Both need the same refactor. |
| 7. Shell completion `build` entry | **anchors valid · ORDERING BROKEN** | No `FilesOrTargets` variant; `PositionalHint::File(FileHint)` with `FileHint.dirs` exists in both registries, so the new hint composes as designed. But see finding 1 — this task can no longer be green on its own. Registry doc-comments also need a count bump: PM says "11 top-level subcommands" (`registry.rs:581`), TM says "ten … plus completions" (`registry.rs:591`). |
| 8. Documentation | **anchors stale** | Every path in the task moved or split. `docs/cli.md` → `docs/pmt/cli.md`; `docs/lint.md` → `docs/pmt/lint.md`; `docs/` now also has the shared root pages (`core.md`, `formats.md`, `history.md`, `lsp.md`) and a full `docs/tmt/` domain. Worse, the two toolchains document their project file in **different page kinds** today: `pmt.json` under `## Project file: pmt.json` in `docs/pmt/lint.md:20`, `tmt.json` under `## tmt.json` in `docs/tmt/cli.md:693`. See finding 2. |

## Finding 1 — Task 7 → Task 8 is no longer independent

The hygiene sweep's #52 work added a `--help` quote drift guard to **both**
crates (`crates/post-machine/tests/cli_docs.rs`,
`crates/turing-machine/tests/cli_docs.rs`). Its second assertion:

> every top-level subcommand the completion registry knows about has such a
> block on the page, so a NEW subcommand fails here instead of silently going
> undocumented.

So the moment `build_spec()` is registered (Task 7), `cli_docs` goes red until
`docs/pmt/cli.md` quotes `pmt build --help` **verbatim**. Plan 1 sequences
documentation last as a standalone task; post-#52 that sequence is not
executable. The rewrite must either fold the cli.md usage block into the
registry task, or register `build` only after the page block exists.

(Same guard, same constraint, on the TM side for `tmt build`.)

## Finding 2 — the doc home is a decision the rewrite must make

Plan 1 says "create `docs/project.md`". That predates the phase-8 docs split
and the second toolchain. Two coherent options:

- **(a) one shared root page** `docs/project.md`, alongside `core.md` /
  `formats.md` / `lsp.md`, with per-toolchain sections. Matches the "shared
  root pages cover both toolchains" precedent; one place to state the schema,
  discovery, and path rules that are genuinely identical. Cost: the TM-only
  `call-mech` key and the PM-only bits live in one page with conditionals, and
  it splits the project-file story away from the lint pages that already tell
  half of it.
- **(b) two per-toolchain pages** `docs/pmt/project.md` + `docs/tmt/project.md`,
  each absorbing the project-file section its lint/cli page carries today.
  Matches the per-toolchain domain structure and the spec's "each CLI reads
  only its own file — there is no cross-toolchain manifest" ruling. Cost:
  deliberate duplication of the identical schema prose.

The spec's own framing (§168: symmetric, per-crate, never merged) leans **(b)**.
Needs a maintainer ruling before the docs task is writable.

## Two things to confirm before writing the rewrite

- **Error-code participation.** Task 2 adds a `ConfigError::Invalid` variant.
  `ConfigError` *appears* not to participate in a `code_registry!` — PM's
  `config.rs` exposes `path()` + `detail()` and no `code()`, and the registries
  found on master are `CompileErrorKind::CODES` (`compiler.rs:111`) and
  `AsmErrorKind::CODES` (`core/src/asm/mod.rs:102`), each with a docs inventory
  table and a set-compare guard (`tests/error_code_docs.rs`, both crates).
  Confirm before adding the variant — if config errors *should* join the code
  namespace, that is a separate decision with a docs table and a guard attached.
- **The schema-version space.** Plan 1's "`pmt.json` schema version becomes 0.2
  (0.1 = retroactive lint-only shape)" is the plan's own invention: no "schema
  version" string exists in any lint or cli page on master. It now has to cover
  **both** `pmt.json` and `tmt.json`, and — per the version-spaces convention —
  earn a line in the release CHANGELOG's version block.

## What the rewrite must add wholesale

Per the amended spec, none of this exists in plan 1 (which contains zero
occurrences of `tmt` or TM-1):

- `crates/turing-machine/src/project.rs` — the schema/validation twin (§168).
- `tmt.json`'s `project` section, including the TM-only `call-mech` key at
  target level with a project-level default (§250).
- `tmt build`, both modes, as a first-class twin of `pmt build` (§373).
- TM `cli/driver.rs`, TM `run.rs` settings/execution split, TM completions
  `build` entry + its `cli_docs` block, TM docs.
- #28 (bare `pmt lint` / `pmt fmt` over the manifest's declared source set),
  which the release path folds into this round and plan 1 does not mention —
  and its TM counterpart.

Roughly: two toolchains × the same eight tasks, minus the one already shipped.
Task 1 being pre-shipped is evidence the re-read shrinks tasks as well as grows
them — the TM arc absorbed core-side prerequisites plan 1 still budgets for.
