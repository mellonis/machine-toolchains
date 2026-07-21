# Project manifest + `pmt build` / `tmt build` — design

Date: 2026-07-12
Status: approved 2026-07-12 (brainstorm walked section-by-section; audit
pass against the codebase folded in)
Tracker: [machine-toolchains#16](https://github.com/mellonis/machine-toolchains/issues/16)
(manifest), [machine-toolchains#11](https://github.com/mellonis/machine-toolchains/issues/11)
(`pmt build` / `tmt build`)

**Amended 2026-07-21:** extended to cover the shipped TM-1 toolchain
(`tmt build` as a first-class twin; parallel manifest schema in
`tmt.json`). This spec was written when only PM-1 (`pmt`) existed; the
TM-1 arc ([machine-toolchains#8](https://github.com/mellonis/machine-toolchains/issues/8))
has since shipped `tmt` in full. Execution of this design had not started
(only plan 1 of 3 was ever written), so the amendment is folded in place
rather than layered on top. The original PM-1 prose stands unchanged where
it is still accurate; the TM-1 material lands as marked subsections under
the manifest and build sections, plus deferral/parallel notes in the LSP,
editors, docs, testing, and follow-up sections. Everything TM-1 here was
verified against the built `tmt` binary and the crate source, not assumed
from the PM-1 shape — the divergences (a required `.tmt` run band, the
`--call-mech` lowering axis, an entry that fixes tape arity, an
already-shipped `LinkOptions.entry`) are called out where they bite. One
genuine design fork surfaced: where `call-mech` lives in the manifest —
written below as an explicit open question.

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
| Version space | `pmt.json`'s schema becomes a named versioned contract like `PMC_LANG_VERSION`: the lint-only shape is retroactively **0.1**; this design makes it **0.2**. It gets a row in the release-notes version block *(amended 2026-07-21: `tmt.json` gets a parallel, independently-versioned row — see Documentation and versioning)* |

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

## TM-1: `tmt.json` grows the same `project` section (amended 2026-07-21)

TM-1 (`tmt`) shipped in full after this spec was written, so the manifest
half applies to it symmetrically. Everything above holds for `tmt.json`
with one substitution and a short list of TM-1 divergences. **Each CLI
reads only its own file — `pmt build` reads `pmt.json`, `tmt build` reads
`tmt.json` — and there is no cross-toolchain manifest.** A repo holding
both a PM-1 and a TM-1 program carries both files (side by side, or in
separate subtrees); the two never merge and neither tool reads the other's.
`tmt.json` already exists exactly as `pmt.json` did
(`crates/turing-machine/src/config.rs`: `lint.allow`, nearest-ancestor
discovery, union-with-the-flag merge, strict unknown-key errors), so it
grows the identical `project` section under the same per-section discovery
rule. `project.rs` lives one crate over
(`crates/turing-machine/src/project.rs`), per-crate, exactly as each crate
already carries its own `config.rs` — core stays arch-agnostic and holds no
manifest knowledge. One loader validates the whole `tmt.json`; consumers
read only their section.

```json
{
  "project": {
    "call-mech": "hybrid",
    "sources": ["src/shared.tmc"],
    "targets": {
      "utm": {
        "sources": ["src/utm.tmc", "src/tables.tma"],
        "output": "out/utm.tmx",
        "run": { "tape": "tapes/bf-hello.tmt", "max-steps": 2000000 }
      }
    }
  }
}
```

Where TM-1 diverges from the PM-1 schema above:

- **Source kinds** — a target's sources may be `.tmc`, `.tma`, or `.tmo`.
  `.tma` is a first-class build input: `tmt asm` already assembles a `.tma`
  to a `.tmo` (the `.s`/`.pma` analogy holds exactly), so a hand-written
  assembly source is legal in a `sources` list and in the argv-mode
  positional list, dispatched by extension like everything else. **Settled,
  not a fork.**
- **`entry`** — same per-target key, same default `main`, same
  "must be exported" rule. It threads the **already-existing**
  `LinkOptions.entry` (`crates/core/src/linker/mod.rs`) — the core change
  the PM-1 half of this spec proposed is already in the tree, landed with
  the TM-1 arc's `tmt link --entry`, so `tmt build` needs no core work (and
  `pmt build` can now thread the same field without one either). Note a
  semantic the PM-1 design never faced: a TM-1 entry is not only the
  reachability-BFS root — it also fixes the machine's **tape arity** and,
  in sectioned/frames output, must carry a **routine signature** (the
  linker errors `entry function X has no routine signature to fill it`). A
  non-`main` TM-1 entry is therefore not a free pick; the manifest names
  it, the linker enforces the rest.
- **`output`** — default `<target-name>.tmx` next to the manifest.
- **`profiles`** — the same two names and the same four keys (`opt`,
  `debug-info`, `strip-debugger`, `werror`), same CLI-preset base values.
  TM-1's extra optimizer switches — `--foutline` (enable the default-off
  `outline` pass) and the `--fno-<pass>` family — stay **per-invocation
  flags only**, never manifest keys, exactly as `--fno-<pass>` is flag-only
  on the PM-1 side.
- **`call-mech`** — a NEW axis with no PM-1 analogue (`tmt link
  --call-mech mono | frames | hybrid`, default `hybrid`): the bound-call
  lowering strategy. It needs a manifest home; see the open question below.
- **`run`** — the block **mirrors `tmt run`, which differs sharply from
  `pmt run`**. `tmt run` always drives a whole multi-tape band loaded from a
  `.tmt` snapshot: its only tape flag is `--tape PATH.tmt` (**mandatory** —
  the tool errors `run needs --tape TAPES.tmt` without it; there is no
  inline-glyph form, no `--head`, no `--strict-cells`, no `--tact-profile`,
  no `--save-tape-block`). So a TM-1 `run` block is just `tape` (a `.tmt`
  path, **required** for the block to be runnable), `max-steps`,
  `no-step-limit` (bool), `max-tacts`. Consequence for `--run` below: a
  TM-1 target with no `run` block — or one lacking `tape` — cannot be
  `--run`, because `tmt run` has no empty-tape default to fall back on
  (PM-1 does); `tmt build --run` on such a target is a pointed error, not a
  default-tape run.
- **`stdlib`** — same project-level bool, default `true`; `false` is the
  manifest form of `tmt link --nostdlib`. TM-1's embedded stdlib twins
  (`std::binaryNumbers` / `std::binaryNumbersBare`) link lazily by
  reachability just like PM-1's.

### Open question — where `call-mech` lives (amended 2026-07-21)

`--call-mech mono | frames | hybrid` (default `hybrid`) is a link-time
lowering with no committed manifest home yet — the one genuine design fork
this amendment cannot settle neutrally. Three defensible placements:

- **(a) a target/project-level key like `entry`/`output`** (shareable at
  project level, overridable per target), defaulting to `hybrid`, with the
  `--call-mech` flag overriding per invocation (flags win, as the profile
  keys do). Treats call-mech as a structural-but-tunable property of the
  produced image — you can experiment with a lowering without editing the
  manifest. **(Recommended.)**
- **(b) a profile key** (`debug` uses `frames`, `release` uses `mono`,
  say). Plausible — a user may genuinely want different lowering per
  profile, so this is not a taxonomy edge case. Costs orthogonality:
  call-mech is independent of opt level, so folding it into the
  debug/release axis couples two unrelated choices.
- **(c) flag-only, no manifest home** — simplest, but the chosen lowering
  is then never a committed artifact, which cuts against the point of the
  manifest.

Recommending **(a)**; the maintainer should confirm or redirect. The
choice also decides `--call-mech`'s manifest-mode behavior: under (a) and
(c) it is *accepted* as an override (unlike `-o`/`-L`/`-l`/`--nostdlib`
/`--entry`, which are rejected for contradicting a structural declaration);
under (b) it would be rejected in manifest mode like the other structural
declarations, since the profile already fixes it.

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

## TM-1: `tmt build` (amended 2026-07-21)

`tmt build` is the exact twin of `pmt build` — same two-mode driver, same
positional-shape dispatch (any positional ending `.tmc`/`.tma`/`.tmo` ⇒
argv mode and the manifest is not read; otherwise positionals are target
names ⇒ manifest mode; mixing the two is an error; target names are
dot-free by schema so the dispatch is unambiguous). It becomes `tmt`'s
twelfth subcommand.

### argv mode (the cc driver)

```
tmt build main.tmc lib.tma extra.tmo [-o app.tmx] [FLAGS]
```

Each `.tmc` compiles, each `.tma` assembles, each `.tmo` loads (CRC + magic
sniff, never by extension), all to in-memory objects; then one link with
the implicit stdlib (unless `--nostdlib`). Default output: first input's
stem + `.tmx`, plus the `.tmx.map` sidecar. `--keep-objects` writes each
intermediate `.tmo` next to its source.

Flag union of `tmt compile` and `tmt link`, verified against the real
`--help`:

- compile side: `--debug`/`--release`, `-O0`/`-O1`, `-g`,
  `--strip-debugger`, `--fno-<pass>`, `--foutline`, `-Werror`
- link side: `--no-relax`, `--nostdlib`, `-L DIR`, `-l NAME`,
  `--call-mech mono|frames|hybrid`, `--entry NAME`
- `-v`

Deliberately **excluded** — per-file inspection / asm-emit artifacts that
stay `tmt compile`'s job, mirroring PM-1's exclusion of `-S`/`--emit-ir`:
`-S`, `--emit-ir`, and TM-1's `--stamped-asm`.

### manifest mode (the #16 consumer)

Identical shape to PM-1's manifest mode (`tmt build` builds every target
alphabetically; `tmt build <target>`; `tmt build --release <target>`;
`tmt build --run [TARGET]`; `tmt build --list-targets`), with these TM-1
specifics:

- Flags rejected in manifest mode: `-o`, `-L`, `-l`, `--nostdlib`,
  `--entry` — each contradicts a structural manifest declaration.
  `--call-mech` is *accepted* as an override under the recommended (a)
  placement above (rejected under (b)); the compile-side profile flags
  (`-g`, `-O0`/`-O1`, `--strip-debugger`, `--foutline`, `--fno-<pass>`,
  `-Werror`) override profile keys per invocation exactly as on the PM-1
  side, flags win.
- `--run` requires a run block **with a `tape`** (see the run-block
  divergence above): `tmt run` has no empty-tape default, so `--run` on a
  target with no run block or no `tape` is a pointed error, not a
  default-tape run. On a successful build the process then adopts `tmt
  run`'s exit codes (0 stopped / 2 hlt / 3 trap), identical to PM-1.
- Warning refinement applies identically: `.tmc` flatten emits the same
  bare `undeclared-external` warning (`compiler.rs::warn_undeclared`), and
  `tmt build` drops those the declared set resolves, in both modes;
  `-Werror` counts the post-filter set. The stdlib exports only
  `std::`-qualified names, so bare calls never match it — same caveat as
  PM-1.

### Plumbing

- A new `crates/turing-machine/src/cli/driver.rs`; the existing
  `cli/build.rs` (compile/asm/link) stays and the driver calls its
  internals. A `BuildReport` (per-input `CompileReport`s + `LinkReport` +
  optional `RunResult`), rendered only in `cli/` under `-v`.
- **No core change** — `LinkOptions.entry` already exists in
  `crates/core/src/linker/mod.rs` (it landed with the TM-1 arc's `tmt link
  --entry`), unlike the PM-1 half, which still proposes adding it. `pmt
  link` does not yet expose an `--entry` flag even though the core field is
  there; that is orthogonal pmt-reality, noted for the maintainer, not
  fixed here.
- Completions: the TM registry
  (`crates/turing-machine/src/completions/registry.rs`) gains a
  `build_spec()` in its own idiom — positional `File(FileHint { extensions:
  ["tmc","tma","tmo"], dirs: false })` combined with dynamic target names;
  the flag table above expressed as `FlagSpec`s (`--call-mech` as
  `Value(Choices(["mono","frames","hybrid"]))`, the `--fno-` family as
  `suffix_family(pass_names())`, `--foutline` as its own boolean,
  `--entry` as `Value(Text)`). A new positional kind renders the
  `_git`-pattern zsh helper that shells `tmt build --list-targets` at
  completion time. The existing drift guard
  (`crates/turing-machine/tests/completions_registry.rs`) auto-probes the
  new entry's flags against the real parser.
- `tmt` becomes twelve subcommands.

## LSP: manifest awareness + the cross-file overlay

**Amended 2026-07-21 — scope of this section.** The overlay design below
is PM-1-specific (`.pmc`/`.pma` sources, flatten's open namespaces). The
symmetric TM-1 concern — a manifest-aware `tmt lsp` overlay across `.tmc`
/`.tma` — is real but **deferred to when plan 2 (the LSP overlay plan) is
written**, and is not re-derived here: TM-1's cross-file model differs
(bindings/grafts and per-world state-graph IR rather than flatten's
namespace join), so its overlay is a separate design pass rather than a
mechanical restatement of the PM-1 one. The manifest awareness itself
(`tmt.json` project views invalidated on watch, load/validation errors
through the existing diagnostics channel) mirrors PM-1 directly and needs
no new design. The PM-1 overlay design stands as written.

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
- **TM-1 (amended 2026-07-21)**: the shipped TM plugin pair
  (`editors/vscode-tm` + `editors/jetbrains-tm`, currently 0.1.0 with a
  `MIN_TESTED_TMT` floor) gets the symmetric treatment against `tmt build
  --list-targets` — a `tmt build <t>` task per target plus `tmt build
  --run <t>` where a run block exists, refreshed on `tmt.json` watch
  events; a `$tmt` problem matcher; a JetBrains run-configuration recipe.
  All four plugins (both `-pm` and both `-tm`) bump their respective
  `MIN_TESTED_*` floors when this releases.

## Documentation and versioning

**Amended 2026-07-21 — `docs/` has since split into per-toolchain
domains** (`docs/pmt/` and `docs/tmt/`, each with language/isa/cli/lint
/fmt/stdlib), with shared root pages (`core.md`, `formats.md`,
`history.md`, `lsp.md`). The doc bullets below are corrected to the split
layout and duplicated per toolchain.

- New manifest-reference pages: `docs/pmt/project.md` and
  `docs/tmt/project.md` — schema, per-section discovery, path rules,
  profiles, run blocks, examples. Ref-free prose per the published-docs
  policy. (The schema is parallel; each page documents its toolchain's own
  source kinds, run block, and — for TM-1 — `call-mech`, so per-toolchain
  pages match the existing cli/lint/fmt split rather than one shared page.)
- `docs/pmt/cli.md` and `docs/tmt/cli.md` each gain `build`; the shared
  root `docs/lsp.md` gains the cross-file section (PM-1 now; TM-1 when
  plan 2 lands); each `docs/{pmt,tmt}/lint.md`'s project-file section
  points at that toolchain's `project.md` (`lint.allow` stays documented
  in lint.md).
- README quickstart gains the manifest example; each tool's subcommand
  count becomes twelve.
- Version block: the project-manifest schema is a versioned contract per
  toolchain, following house precedent that PM-1 and TM-1 contracts
  version independently (`.pma`/`.tma` dialects, `PMC_`/`TMC_LANG_VERSION`).
  Two rows, coincidentally both moving the same way: **`pmt.json` schema**
  0.1 (retroactive, the lint-only shape) → **0.2**, and **`tmt.json`
  schema** 0.1 → **0.2**. They are byte-identical at 0.1 and diverge here
  (`call-mech`, the differently-shaped run block) — exactly the
  "independent but happen to match" pattern. Crates bump at the release
  cut; `.pmc`/`.tmc` languages, the PM-1/TM-1 dialects, IR, and container
  formats unchanged. Note the TM-1 arc's own first release is separately
  pending (deferred by maintainer ruling until the range-expression work,
  [#31](https://github.com/mellonis/machine-toolchains/issues/31), lands),
  so `tmt.json`'s 0.2 bump rides that release and need not share a cut with
  `pmt.json`'s.

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
- **TM-1 (amended 2026-07-21)**: the same matrices run one crate over for
  `tmt` — `project.rs` validation (incl. `call-mech` value validation and
  the run-block `tape`-required rule), argv-mode `.tmc`/`.tma`/`.tmo`
  dispatch and default `.tmx` naming, manifest-mode multi-target build and
  contradicting-flag rejection, and the TM-specific `--run` behavior (a
  target with no run block or no `tape` errors rather than running an empty
  tape; exit codes 0/2/3 otherwise), plus the completions drift guard over
  the TM registry's `build_spec()`. The LSP overlay↔linker equivalence
  fixture is PM-1-only this round; its TM-1 counterpart rides plan 2.

- **Rename / find-references** — unblocked by the manifest, still
  needs the references index; v2.
- **Bare `pmt lint` / `pmt fmt` (and `tmt lint` / `tmt fmt`) over the
  declared source set** — a natural follow-up once the manifest exists;
  noticed during this design, deliberately not scoped in. Filed as
  [machine-toolchains#28](https://github.com/mellonis/machine-toolchains/issues/28),
  whose framing is PM-1-worded but applies symmetrically to `tmt` (its
  `.tmc`/`.tma` source set, its `--no-config` interaction) — amended
  2026-07-21 to cover both toolchains.
- **Glob sources, per-target profiles, an `out-dir` key, more profile
  names** — schema supersets, each addable without breaking 0.2.
- **JSONC comments in `pmt.json`** — stays deferred from the LSP
  ledger.
- **bash/fish completion rendering** — unchanged: recognized shell
  names with a clear not-yet-implemented error.
- **Symlink-aware duplicate detection** — lexical normalization only,
  documented.
- **JetBrains task/run-configuration plugin code** — recipe-only this
  round, for both `jetbrains-pm` and `jetbrains-tm`. Plugin-side per-target
  integration filed as
  [machine-toolchains#29](https://github.com/mellonis/machine-toolchains/issues/29)
  (PM-1-worded; applies symmetrically to the `-tm` plugin — amended
  2026-07-21).
- **`--keep-objects` stem collisions (amended 2026-07-21, flag not fix)** —
  the argv-mode claim that writing intermediates next to their source makes
  stem collisions impossible is shaky for *both* toolchains: `foo.tmc` and
  `foo.tma` (or `foo.pmc`/`foo.pma`) in one directory both want `foo.tmo`
  (`foo.pmo`). Pre-existing in the PM-1 text, out of this amendment's
  scope to redesign; noted so `--keep-objects`'s collision handling is
  settled when the driver is actually built.
