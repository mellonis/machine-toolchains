# The project manifest — `pmt.json`

`pmt.json` is the toolchain's one project config file. It holds up to two
independent sections at its top level:

- `lint` — the hygiene allow-list, documented on `docs/pmt/lint.md`
  (`docs/pmt/lint.md (project file)`).
- `project` — the declared project model this page describes: shared and
  per-target sources, libraries, build profiles, and named targets with
  their own output and run settings.

Either section may be present alone, both may be present together, or the
file may be `{}` (nothing set — a `pmt.json` that exists only to mark a
directory). Every key is validated strictly: an unrecognized key at any
level, or a value of the wrong shape, is a hard error naming the file and
the offending key. `pmt.json` itself carries no literal version marker;
this page and the release notes track the `project` section's schema as
its own versioned contract, currently **0.2**. The lint-only shape that
existed before the `project` section was added is retroactively **0.1**.

## Discovery

`pmt.json` is found by two independent ancestor walks, each starting from
a directory and stopping at its **first** match — neither walk ever
merges settings across more than one file:

- **Lint discovery** (unchanged): the nearest ancestor `pmt.json`,
  whether or not it has a `project` section.
- **Project discovery**: the nearest ancestor `pmt.json` that *has* a
  `project` key. A `pmt.json` with only a `lint` section, sitting between
  a source file and its project root, is invisible to this walk — the
  lint walk still stops there for lint purposes, but the project walk
  passes through it and keeps looking further up.

`pmt build` runs project discovery from the current directory; a build
started from a subdirectory still resolves against the manifest found
above it (see Path rules below). `pmt lint` and `pmt fmt`, invoked with
no file arguments, use the same discovery — see The declared source set.

### One loader

One loader validates the **whole file** — both sections, every key —
regardless of which one a given consumer asked for. A typo in `project`
still fails a lint-only load of the same file, and vice versa: the two
walks can never disagree about whether a `pmt.json` is well-formed.

## Example

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

Two targets, `app` and `bench`, share `src/shared.pmc` and the `bitops`
library declared at project level, each adding its own source and run
settings. `pmt build` (no arguments) builds both; `pmt build app` builds
only `app`; `pmt build --run bench` builds `bench` and then runs it
against `tapes/bench-in.pmt`.

## Schema reference

### Project-level keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `stdlib` | bool | `true` | `false` is the manifest form of `--nostdlib`: every target in this file links without the standard library. Project-level only — there is no per-target override. |
| `sources` | array of strings | `[]` | Source paths prepended to every target's own `sources`, in order. |
| `libraries` | object | `{}` | `dirs` (search directories) and `link` (library names), each prepended to every target's own list, in order. |
| `profiles` | object | `{}` | Overrides for the `debug` and `release` profile bases — see Profiles below. |
| `targets` | object | — | **Required, at least one entry.** Named build targets — see Targets below. |

### Targets

Each key under `targets` is a target name matching
`[A-Za-z0-9][A-Za-z0-9_-]*` (dot-free, so a target name can never be
mistaken for a file positional). Targets are otherwise independent; the
map is read in alphabetical-by-name order, which is the documented
cross-target build order for a bare `pmt build`.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `sources` | array of strings | `[]` | Appended after the project-level `sources` to form this target's effective source list, in order. May be `.pmc`, `.pma`, or `.pmo` — the same per-extension dispatch (compile / assemble / load) `pmt build`'s argv mode uses. |
| `libraries` | object | `{}` | `dirs`/`link`, appended after the project-level lists to form this target's effective libraries. |
| `entry` | string | `"main"` | The linker's reachability root for this target (must not be an empty string). The named function must be exported — an unexported or missing entry is a link-time error, not a manifest-validation one. Two targets over the same sources with different entries is expected and supported. |
| `output` | string | `"<target-name>.pmx"` | Output path, resolved against the manifest's directory. Two targets whose (normalized) output paths collide is a manifest error. |
| `run` | object | absent | Optional run settings for `pmt build --run` — see The `run` block below. |

Library resolution is **first-wins** in declared order, and a user
definition of the same exported symbol silently shadows a library's — no
warning is emitted (`docs/core.md (linking)`). Linking is lazy by
reachability, so a declared library that no effective source references
contributes nothing to the output.

### Profiles

`profiles` holds at most two keys, `debug` and `release` — any other name
is rejected. Each names up to four overrides, all optional:

| Key | Type | Meaning |
|---|---|---|
| `opt` | `"O0"` \| `"O1"` | Optimization level. |
| `debug-info` | bool | Record debug info (labels + `.pmc` lines). |
| `strip-debugger` | bool | Drop `brk` at codegen. |
| `werror` | bool | Treat post-refinement warnings as errors. |

An override layers on top of one of two fixed bases, which mirror the CLI
presets (`docs/pmt/cli.md`, `pmt compile`'s `--debug`/`--release`):

| Base | `opt` | `debug-info` | `strip-debugger` | `werror` |
|---|---|---|---|---|
| `debug` | `O0` | `true` | `false` | `false` |
| `release` | `O1` | `false` | `true` | `false` |

Profile selection happens once per `pmt build` invocation, not per
target: `--release` selects the `release` base for every target built in
that invocation, and omitting both `--release` and `--debug` selects the
`debug` base — there is no per-target profile name in the schema. This
means a bare `pmt build` (no flags at all) still resolves the full
`debug` base, including `debug-info: true` — unlike `pmt compile`'s own
true no-flags default, which carries no debug info. The individual
compile-side flags (`-g`, `-O0`/`-O1`, `--strip-debugger`, `-Werror`)
override the resolved profile's matching key for that invocation only;
the manifest is never rewritten (`docs/pmt/cli.md (build)`, "flags win").

### The `run` block

A target's optional `run` object is read only by `pmt build --run`
(`docs/pmt/cli.md (build)`); every key is optional:

| Key | Type | Meaning |
|---|---|---|
| `tape` | string | Inline glyph pattern, as `pmt run --tape`. |
| `tape-block` | string | Path to a `.pmt` snapshot, as `pmt run --tape-block`. |
| `head` | integer | Initial head position — only meaningful alongside `tape`. |
| `strict-cells` | bool | Trap on double-mark/double-unmark. |
| `max-steps` | non-negative integer | Step budget. |
| `max-tacts` | non-negative integer | Tact budget. |
| `tact-profile` | `[move, read, write]` (each a non-negative integer) | Device costs, as `pmt run --tact-profile`. |

Two rules are enforced at manifest-validation time: `tape` and
`tape-block` are mutually exclusive, and `head` requires `tape` (it has
no meaning against `tape-block`, which fixes its own head position). An
absent `run` block, and a present-but-empty `run: {}`, both fall back to
`pmt run`'s own defaults (empty tape, head 0, no step or tact limit
beyond `pmt run`'s usual 10,000,000-step default) rather than erroring —
the difference between the two is visible only in `pmt build
--list-targets`, whose `run` marker reflects whether the *key* is
present, not whether its resolved settings differ from the defaults.

## Schema rules

- **Unknown keys are rejected everywhere** — at the top of `project`,
  inside `libraries`, inside a profile, inside a target, and inside a
  `run` block — with an error naming the file and the offending key.
- **`targets` must have at least one entry.**
- **Target names** must match `[A-Za-z0-9][A-Za-z0-9_-]*`.
- **`entry`**, if given, must not be an empty string.
- **Duplicate effective sources**: if the same path (after path
  normalization, below) appears twice in one target's effective source
  list — once from the project level and once from the target, or twice
  within the target's own list — that target is a manifest error. This
  check is per target: the same shared source legitimately appearing in
  several targets' effective lists is normal and not an error.
- **Output collisions**: if two targets' (normalized) output paths are
  the same file, that is a manifest error, independent of the duplicate
  check above.
- **Existence is not checked here.** Manifest validation is a pure
  syntax/shape pass over the JSON; it does not touch the filesystem
  beyond reading `pmt.json` itself. A source, library directory, or
  `tape-block` path that doesn't exist on disk surfaces as an ordinary
  "cannot read" error only when something actually tries to read it —
  building the target, or (for sources) linting/formatting it.

## Path rules

Every path in the manifest — `sources`, `libraries.dirs`, a target's
`output`, and a run block's `tape-block` — resolves against **the
manifest's own directory**, never the process's current directory. A
build started from a subdirectory still writes outputs and reads sources
relative to where `pmt.json` was found, not where `pmt build` was
invoked from. (`libraries.link` entries are library *names*, not paths,
and are resolved against `libraries.dirs` at build time, not normalized
as paths.)

- **Absolute paths are rejected** — a manifest is a committed,
  portable artifact; encoding one machine's layout into it defeats that.
- **`../` is allowed** — traversing above the manifest's directory is a
  normal way to share sources or libraries across a tree of projects.
- **Normalization is lexical only**: `.` components are dropped, and an
  interior `..` cancels the path segment before it (`src/../m.pmc`
  normalizes to `m.pmc`, and `./out.pmx` normalizes to `out.pmx` — this
  is what the duplicate-source and output-collision checks compare
  against). A *leading* `..` cannot cancel anything and is kept as-is.
  This normalization does not consult the filesystem: two paths that
  reach the same file only through a symlink are not detected as
  duplicates. That is a documented limitation, not a planned fix.

## The declared source set

Every target's effective source list (project-level `sources` followed
by the target's own) can be unioned across the whole manifest into one
flat list — first occurrence wins in target-alphabetical, then
list order, deduplicated after path normalization, with `.pmo` entries
dropped (an object file carries no text, so there is nothing in it to
lint or format). This is **the declared source set**, and it is exactly
what a bare invocation — no file or directory arguments — of `pmt lint`
or `pmt fmt` operates on: `pmt lint` and `pmt fmt` discover the nearest
project manifest from the current directory (the same discovery `pmt
build` uses) and lint or format precisely its declared sources, never a
directory scan. A file sitting next to a declared source but not named
by any target's `sources` is invisible to a bare invocation, even though
it would be picked up by a directory scan or an explicit path argument.

A bare invocation with no manifest found upward from the current
directory is an error naming what was searched for — the same error
`pmt build` (with no target names, from an unmanifested directory)
gives.

`pmt lint --no-config` cannot be combined with a bare invocation: with no
file arguments, the manifest's declared set *is* the input, so skipping
project discovery would leave nothing to lint. `--no-config` still works
normally alongside explicit file or directory arguments, where it only
suppresses the per-file lint allow-list lookup. `pmt fmt` has no
`--no-config` flag at all — only `pmt lint` does — so this particular
conflict cannot arise for `fmt`; a bare `pmt fmt` with a discovered
manifest always formats its declared set.

## How `pmt build` consumes it

`pmt build` is the one manifest consumer among the CLI subcommands;
`compile`/`asm`/`link`/`run` stay purely argv-driven low-level tools.
Full flag reference, argv-mode-vs-manifest-mode dispatch, `--run`,
`--list-targets`, and the flags manifest mode rejects are all on
`docs/pmt/cli.md (build)` — this section only summarizes what's specific
to the manifest itself.

For each target it builds, `pmt build` compiles/assembles/loads that
target's effective sources, links them against its effective libraries
(plus the standard library unless `stdlib: false`) with its resolved
profile and its `entry`, and writes its `output` (plus the `.pmx.map`
sidecar) next to the manifest. The ordinary per-file "undeclared
external" compile warning — which fires on a bare call a given file
doesn't import — is refined the same way it is in argv mode: once a
target's whole effective source set is known, a bare call resolved by
some *other* file in that same target's set no longer warns. This
refinement is scoped per target, not across the whole manifest — a name
defined only in a sibling target's own (non-shared) source does not
refine another target's warnings. `pmt compile`, working one file at a
time with no visibility into any declared set, stays per-file honest and
always warns on a bare undeclared call.

## Editor resolution

The `project` section isn't only read by `pmt build`: `pmt lsp` reads the
same declared sources, libraries, and `stdlib` flag to resolve cross-file
names for an open document that belongs to a target here — the project
overlay documented at `docs/lsp.md` ("Cross-file resolution (the project
overlay)").
