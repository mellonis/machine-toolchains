# Project manifest + `pmt build` — design

Date: 2026-07-12
Status: approved 2026-07-12 (brainstorm walked section-by-section; audit
pass against the codebase folded in)
Tracker: [machine-toolchains#16](https://github.com/mellonis/machine-toolchains/issues/16)
(manifest), [machine-toolchains#11](https://github.com/mellonis/machine-toolchains/issues/11)
(`pmt build`)

## Context

There is no manifest today, by design: `.pmc` compiles per file,
namespaces are open and join only at link time, directories mean
nothing, and the link set exists only as `pmt link`'s argv — `-L DIR` /
`-l NAME`, where first-wins resolution makes library *order*
semantically significant, a shell-history fact rather than a committed
artifact. `pmt.json` exists but is deliberately tiny and lint-only
(`lint.allow`; nearest-ancestor discovery; union merge across sources,
never across files). The LSP keeps cross-file namespaces deliberately
invisible — completion and navigation offer only what the compiler can
prove from the current file plus the embedded stdlib — because guessing
a link set from workspace scans was considered and rejected; a manifest
supersedes that speculation. The VS Code extension ships its
compile→link→run pipeline as a documented `tasks.json` snippet
precisely because link and tape choices cannot be inferred from a lone
`.pmc`; JetBrains run configurations have the same gap.

This spec grows `pmt.json` into the declared project model and adds
`pmt build`, the cc-style driver that consumes it. It is a philosophy
change to a deliberately manifest-free toolchain, scoped as its own
design cycle exactly as the manifest issue demanded.

## Decisions (settled during brainstorming, 2026-07-12)

| Decision | Choice |
|---|---|
| Round scope | full stack in one spec: manifest schema, `pmt build` (both modes), LSP cross-file exactness, editor tasks. Implementation may stage into multiple plans (the LSP-milestone precedent: three plans off one spec) |
| File identity | grow `pmt.json`; **per-section discovery** — lint config = nearest `pmt.json` (unchanged); project = nearest `pmt.json` *containing a `project` key*. Both walks stop at their first hit; nothing ever merges across files. A subtree lint override no longer severs files below it from their project. (Single-walk was rejected as a silent-disconnection footgun; a separate manifest file was rejected to keep "one project config file" true) |
| Target shape | named `targets` map — shared sources/libraries at project level, per-target sources/libraries/entry/output/run. (Single-program-per-manifest was rejected in favor of the superset) |
| Source style | explicit relative paths only, no globs. The manifest error-checks its file list; the link set is a committed artifact — touching the manifest when adding a file is the point |
| Path rules | paths resolve against the manifest's directory; `../` traversal is **allowed** (portable within a tree layout; forbidding it is theater — symlinks defeat it); **absolute paths are rejected** (they encode one machine). Duplicate detection is lexical normalization only; symlink aliases are not detected, documented not solved |
| CLI split | `pmt build` is the **sole manifest consumer**. `compile` / `asm` / `link` / `run` stay purely flag-driven low-level tools — the cc/ld layer under a cargo-like driver. `pmt build FILES...` also works manifest-free (the cc-driver mode). (Manifest-reading `pmt link` was rejected: one place with discovery semantics; the bottom layer stays a pure function of its argv) |
| Build options | two profiles mirroring the existing CLI presets: `debug` (`-g -O0`, default) and `release` (`-O1 --strip-debugger`), selected by `pmt build --release`; the manifest may override individual keys per profile; per-invocation flags override profile keys (flags win) |
| Run config | optional per-target `run` block (tape, head, budgets, strict-cells, tact profile) + `pmt build --run`. `pmt run` stays a pure argv tool; ad-hoc runs remain its job |
| LSP architecture | **merged per-file analyses + resolution overlay** (Approach A): each file keeps today's per-file pipeline; sibling exports merge into an overlay consulted after local resolution. (Real-link-in-the-server was rejected: all-or-nothing on broken siblings mid-edit, and the linker works on objects where spans are gone; workspace scanning stays rejected) |
| Faithfulness contract | overlay resolution ≡ `linker::resolve` on the declared set, pinned by an equivalence test. Linked symbol names ARE the qualified names (flatten mangles `::`/`.` onto `Function::name`), so no name mapping is needed |
| Undeclared-external refinement | the "undeclared external" **compile warning** (flatten's, fires on bare calls only — qualified calls are self-declaring and never warn) is refined wherever a full link set is declared: `pmt build` post-filters in **both modes** (its argv or the manifest is always a complete declared set); the LSP does the same via the overlay. `pmt compile` / single-file lint stay per-file honest — same input scope, same diagnostics |
| Shell completion | the completions registry gains the `build` entry (the shell-completion design doc's parked sketch, now activated); target names complete **dynamically** — the rendered zsh function shells out to `pmt build --list-targets` at completion time, the `_git` pattern |
| Version space | `pmt.json`'s schema becomes a named versioned contract like `PMC_LANG_VERSION`: the lint-only shape is retroactively **0.1**; this design makes it **0.2**. It gets a row in the release-notes version block |

## The manifest: `pmt.json` grows a `project` section

```json
{
  "lint": { "allow": ["unused-label"] },
  "project": {
    "stdlib": true,
    "sources": ["src/shared.pmc"],
    "libraries": { "dirs": ["libs"], "link": ["bitops"] },
    "profiles": {
      "release": { "werror": true }
    },
    "targets": {
      "app": {
        "sources": ["src/app.pmc"],
        "output": "out/app.pmx",
        "run": { "tape": " * * *", "head": 0, "strict-cells": true }
      },
      "bench": {
        "sources": ["src/bench.pmc"],
        "entry": "bench::start",
        "run": { "tape-block": "tapes/bench-in.pmt", "max-tacts": 500000 }
      }
    }
  }
}
```

### Discovery

Per-section, two walks, both nearest-of-kind, both stop at their first
hit, neither ever merges across files:

- **lint config**: nearest ancestor `pmt.json` — byte-for-byte today's
  contract, unchanged.
- **project**: nearest ancestor `pmt.json` that *has* a `project` key.
  A lint-only `pmt.json` between a source file and its project root is
  transparent to the project walk.

`pmt build`'s manifest mode discovers from **cwd**; the LSP discovers
from the open document's directory (its existing per-file walk).

### Schema rules

- `targets` — required, at least one entry. Names match
  `[A-Za-z0-9][A-Za-z0-9_-]*` (dot-free: names become default output
  filenames and must never look like file positionals). `serde_json`'s
  alphabetical map order is the documented cross-target build order —
  targets are independent, so order carries no semantics.
- **Effective source list** of a target = project-level `sources` ++
  the target's own, order preserved. The same shared++own rule applies
  to `libraries.dirs` and `libraries.link` (each optional at either
  level). A path appearing twice in an effective list (after lexical
  normalization) is a manifest error.
- Sources may be `.pmc`, `.pma`, or `.pmo` — same dispatch as `pmt
  build`'s argv mode (compile / assemble / load). Every listed path
  must exist when consumed; missing is an error naming the manifest.
- `libraries.link` entries are the manifest form of `-l NAME`: search
  the declared `dirs` in order for `NAME.pmo`, link it as a *library*
  — first-wins, silently shadowed by user definitions, lazily linked
  by reachability (the existing linker contract). The array order is
  the committed search-and-shadow order.
- `entry` — optional per target, default `main`; the fully-qualified
  linked symbol name the reachability BFS starts from. The named
  function must be exported. Two targets over identical sources with
  different entries is the feature's point.
- `output` — optional, default `<target-name>.pmx` next to the
  manifest. Two targets resolving to the same normalized output path
  is a manifest error.
- `profiles` — only the two names `debug` and `release`. Keys per
  profile, each optional: `opt` (`"O0"` | `"O1"`), `debug-info`
  (bool), `strip-debugger` (bool), `werror` (bool, default false in
  both). Base values mirror the CLI presets: debug = `{opt: O0,
  debug-info: true, strip-debugger: false}`, release = `{opt: O1,
  debug-info: false, strip-debugger: true}`.
- `run` — optional per target: exactly one of `tape` (inline glyph
  string) or `tape-block` (`.pmt` path); `head` only alongside `tape`;
  plus `strict-cells`, `max-steps`, `max-tacts`, `tact-profile`
  `[move, read, write]`. Absent keys fall back to `pmt run` defaults.
- `stdlib` — project-level bool, default `true`; `false` is the
  manifest form of `--nostdlib` (compilation never involves the
  stdlib; it is a link-time library, so this key only gates linking
  and `std::` resolution).
- **Strict unknown-key errors everywhere**, extending `config.rs`'s
  posture and its precise "unknown key `X`" messages to the new
  section.

### Plumbing

Manifest types + validation live in a new
`crates/post-machine/src/project.rs`: schema structs, a
`serde_json::Value` walk in `config.rs`'s precise-error style, path
normalization, and per-section discovery reusing `config::discover`'s
ancestor walk. Shared by the CLI and the LSP. **One loader validates
the whole file** — both sections, every key — regardless of which
consumer asked; consumers then read only their section. A lint-driven
load of a file with a broken `project` section still errors (and vice
versa): a typo never silently does nothing, and the two walks can
never disagree about whether a given `pmt.json` is well-formed.

## `pmt build`: one subcommand, two modes

**Mode dispatch** is by positional shape, never by flags: if any
positional ends in `.pmc`/`.pma`/`.pmo`, it is **argv mode** (the
manifest is not read at all); otherwise positionals are **target
names** (or absent) and it is **manifest mode**. Mixing file paths and
target names is an error. Target names are dot-free by schema, so the
dispatch is unambiguous.

### argv mode (the cc driver)

```
pmt build main.pmc util.pmc extra.pmo [-o app.pmx] [FLAGS]
```

Each `.pmc` compiles, each `.pma` assembles, each `.pmo` loads (CRC +
magic sniff as everywhere) — all to in-memory objects, no disk
intermediates. Then one link with the implicit stdlib (unless
`--nostdlib`). Default output: first input's stem + `.pmx`, plus the
`.pmx.map` sidecar. `--keep-objects` opts into writing each
intermediate `.pmo` next to its source (stem collisions are impossible
there; an output directory would reintroduce them).

Flags are the documented union of compile's and link's:
`--debug`/`--release`, `-O0`/`-O1`, `-g`, `--strip-debugger`,
`--fno-<pass>`, `-Werror` (compile side); `--no-relax`, `--nostdlib`,
`-L DIR`, `-l NAME` (link side); `-v`. Deliberately *not* included:
`-S` and `--emit-ir` — per-file inspection artifacts stay `pmt
compile`'s job.

### manifest mode (the #16 consumer)

```
pmt build                  # nearest project manifest from cwd, build ALL targets
pmt build app              # one target
pmt build --release app    # profile selection
pmt build --run [TARGET]   # build, then run the target's declared run block
pmt build --list-targets   # machine-readable target listing
```

- Bare `pmt build` builds every target, alphabetically. No manifest on
  the walk is an error naming what was searched for.
- Profile selection: `--release` picks release, default is debug.
  Individual compile flags (`-g`, `-O0`/`-O1`, `--strip-debugger`,
  `--fno-<pass>`, `-Werror`) override profile keys per invocation —
  flags win. `--no-relax`, `--keep-objects` (same next-to-source
  placement), and `-v` also apply.
- Flags that contradict the declared model are **rejected** in
  manifest mode with a pointed error: `-o`, `-L`, `-l`, `--nostdlib` —
  those are exactly what the manifest declares.
- `--run` requires an unambiguous target: the named one, or the sole
  target when only one exists — otherwise "name a target". Run
  settings come from the manifest's `run` block only (a target without
  one runs on `pmt run` defaults); ad-hoc variations remain `pmt
  run`'s argv job.
- `--list-targets` prints one target per line: the name, then a tab
  and the word `run` when the target has a run block. Consumed by
  editor task providers and the zsh completion function.
- **Warning refinement**: after compiling the declared set, bare
  "undeclared external" compile warnings whose name the declared set
  resolves (a sibling's exported symbol or a declared library's — the
  stdlib exports only `std::`-qualified names, which bare calls never
  match) are dropped from the report; `-Werror` counts the post-filter
  set. Argv mode refines identically over its argv-given set — same
  input scope, same diagnostics.

**Exit codes**: 0/1 for build; with `--run`, after a successful build
the process adopts `pmt run`'s codes (0 = `stp`, 2 = `hlt`, 3 = trap)
so scripts and editor tasks see the machine outcome.

### Plumbing

- The subcommand lives in a new `cli/driver.rs` — `cli/build.rs`
  (compile/asm/link) stays as-is; the driver calls the same internals.
  Thin-renderer rule: a new `BuildReport` (per-file `CompileReport`s +
  `LinkReport` + optional `RunResult`), every byte rendered in `cli/`
  under `-v`.
- One small core change: `LinkOptions.entry: Option<String>` (default
  `main`). The linker's BFS root is hardwired in exactly one spot
  (`resolve`'s namespace lookup); the missing-entry error names the
  configured entry. No format change — MX already stores an entry
  *offset*.
- Completions: the registry gains the `build` entry (flag table as
  above; positional = extension filter `.pmc`/`.pma`/`.pmo` combined
  with dynamic target names). A new registry positional kind renders a
  zsh helper that calls `pmt build --list-targets 2>/dev/null` at
  completion time (correct cwd by construction). The registry drift
  guard auto-probes the new entry's flags against the real parser; the
  `zsh -n`/`compinit` test covers the rendered script including the
  dynamic function.
- `pmt` becomes twelve subcommands — README/CLAUDE.md counts update at
  implementation.

## LSP: manifest awareness + the cross-file overlay

### Manifest awareness

`project.rs` is shared with the CLI. The server already watches
`**/pmt.json`; a change now also invalidates project views and
re-resolves affected documents. Manifest load/validation errors
surface through the existing `invalid-config` diagnostics channel on
the governed source documents (the server still does not serve
`pmt.json` itself). Same mtime-cache pattern as the lint config cache.

### The project view (overlay)

For an open `.pmc` document: absolutize its path → project walk to its
manifest → **membership** = normalized path ∈ a target's effective
sources → **overlay set** = the union of the file sets of every target
containing it. A file in no target — or an untitled/unsaved document,
which has no path — keeps today's single-file view. A file reachable
only via `../` from a manifest it has no ancestor relationship with
does not discover that project from its own directory (honest
limitation, mirrors the CLI; exact features apply when the workspace
also contains the manifest and the file is opened through it).

Sibling files get the same per-file parse → flatten → analysis
pipeline, sourced from open-editor text when the sibling is open, disk
otherwise, cached by (path, mtime/version). A sibling that fails to
parse contributes nothing while everything else keeps working — the
robustness property that decided Approach A.

Overlay contents honor **visibility** — only exported symbols cross
files: `.pmc` `export` functions (and the auto-exported un-namespaced
top-level `main`), `.pma` plain `.func name` (not `.func name local`)
with real CST spans, `.pmo` library symbols name-only (completion yes,
navigation null). Both **bare** top-level exports and **namespaced**
exports participate — namespaces are open and join across files, and
bare externals link against sibling exports, so the overlay mirrors
the linker's namespace join exactly. Resolution order: local first
(unchanged), then declared sources in effective order, then declared
libraries (first-wins), then the stdlib — unless `"stdlib": false`,
which removes `std::` resolution and the materialized-stdlib jump.

### What lights up

- **Completion**: namespace members, qualified paths, and `use` paths
  across the declared set; declared-library and stdlib names as today.
- **Go-to-definition**: plain `file:` URIs into sibling sources
  (`.pmc` and `.pma` both carry spans).
- **Hover**: the sibling's doc lines and deprecation callouts —
  `Analysis.docs` is already qualified, the overlay carries it.
- **Semantic tokens**: overlay-resolved call sites tokenize as
  `function` (previously deliberately untokenized when unresolved);
  `defaultLibrary` stays std-only. Falls out of resolution for free.
- **Diagnostics**: the bare undeclared-external compile warning is
  dropped when the overlay resolves the name — the LSP mirror of `pmt
  build`'s refinement, so the editor and the build tell one story.

Rename and find-references stay parked: the manifest unblocks them,
but they need the references index and remain the v2 candy the LSP
spec's ledger already names.

### The faithfulness contract

Overlay resolution ≡ `linker::resolve` on the declared set. Because
linked symbol names are the qualified names, the equivalence test
needs no mapping: for fixture projects, every qualified and bare call
the overlay resolves must point at the definition the linker actually
picks (same file, same symbol — navigation spans are orthogonal;
name-only `.pmo` resolutions compare by symbol provenance), and every
call the overlay leaves unresolved must be one the linker also fails
to resolve. This test is the reason Approach A's semantics duplication
is acceptable.

## Editors

- **VS Code**: the task provider grows from three single-file tasks to
  per-target tasks by shelling `pmt build --list-targets` at the
  manifest's directory (reusing `pmt.path`): a `pmt build <t>` task
  per target, plus `pmt build --run <t>` where a run block exists;
  refreshed on `pmt.json` watch events. The README's hand-written
  pipeline snippet demotes to a "custom pipelines" note. The `$pmt`
  problem matcher already fits build's `file:line:col` error format.
- **JetBrains**: no plugin code this round — LSP features arrive
  through the server; the README gains a documented run-configuration
  recipe around `pmt build`.
- Both plugins bump `MIN_TESTED_PMT` when this releases.

## Documentation and versioning

- New `docs/project.md`: the manifest reference — schema, per-section
  discovery, path rules, profiles, run blocks, examples. Ref-free
  prose per the published-docs policy.
- `docs/cli.md` gains `build`; `docs/lsp.md` gains the cross-file
  section; `docs/lint.md`'s project-file section points at
  `project.md` (lint.allow stays documented in lint.md).
- README quickstart gains the manifest example; subcommand count
  becomes twelve.
- Version block: new **pmt.json schema** space — 0.1 (retroactive,
  the lint-only shape) → **0.2**. Crates bump to 0.3.0 at the release
  cut; `.pmc` language, PM-1 dialect, IR, and container formats
  unchanged.

## Testing

- `project.rs` unit tests: the validation matrix (unknown keys at
  every level, duplicate paths, absolute-path rejection, profile-name
  and profile-key validation, `tape` XOR `tape-block`, `head` without
  `tape`, target-name charset, duplicate outputs, missing `targets`),
  per-section discovery (nested lint-only `pmt.json` under a project
  root is transparent to the project walk; the lint walk still stops
  at it).
- Driver E2E in the `cli_programs.rs` style: argv mode (mixed inputs,
  default naming, `--keep-objects` placement, `--nostdlib`), manifest
  mode (multi-target alphabetical build, profile selection and
  flag-override precedence, contradicting-flag rejection, `--run` exit
  codes 0/2/3, `--list-targets` format, no-manifest error), warning
  refinement (bare external resolved by a declared sibling stops
  warning; `-Werror` counts the post-filter set).
- LSP integration fixtures: cross-file completion / definition /
  hover, manifest-edit republish, broken-sibling degradation,
  `stdlib: false` behavior, `../`-membership, untitled-document
  fallback, undeclared-external refinement.
- The overlay↔linker equivalence fixture test (the faithfulness
  contract).
- Completions: registry drift guard auto-covers `build`; the
  `zsh -n`/`compinit` test covers the rendered script including the
  dynamic target function.

## Out of scope (the follow-up ledger)

- **Rename / find-references** — unblocked by the manifest, still
  needs the references index; v2.
- **Bare `pmt lint` / `pmt fmt` over the declared source set** — a
  natural follow-up once the manifest exists; noticed during this
  design, deliberately not scoped in. Filed as
  [machine-toolchains#28](https://github.com/mellonis/machine-toolchains/issues/28).
- **Glob sources, per-target profiles, an `out-dir` key, more profile
  names** — schema supersets, each addable without breaking 0.2.
- **JSONC comments in `pmt.json`** — stays deferred from the LSP
  ledger.
- **bash/fish completion rendering** — unchanged: recognized shell
  names with a clear not-yet-implemented error.
- **Symlink-aware duplicate detection** — lexical normalization only,
  documented.
- **JetBrains task/run-configuration plugin code** — recipe-only this
  round. Plugin-side per-target integration filed as
  [machine-toolchains#29](https://github.com/mellonis/machine-toolchains/issues/29).
