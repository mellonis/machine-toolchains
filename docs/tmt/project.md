# The project manifest — `tmt.json`

`tmt.json` is the toolchain's one project config file. It holds up to two
independent sections at its top level:

- `lint` — the hygiene allow-list, documented on `docs/tmt/lint.md`
  (`docs/tmt/lint.md (project file)`).
- `project` — the declared project model this page describes: shared and
  per-target sources, libraries, build profiles, the bound-call lowering,
  and named targets with their own output and run settings.

Either section may be present alone, both may be present together, or the
file may be `{}` (nothing set — a `tmt.json` that exists only to mark a
directory). Every key is validated strictly: an unrecognized key at any
level, or a value of the wrong shape, is a hard error naming the file and
the offending key. `tmt.json` itself carries no literal version marker;
this page and the release notes track the `project` section's schema as
its own versioned contract, currently **0.2**. The lint-only shape that
existed before the `project` section was added is retroactively **0.1**.

`tmt.json` and PM-1's `pmt.json` are separate contracts that happen to
share a version number. `tmt build` reads only `tmt.json` and `pmt build`
only `pmt.json`; the two files never merge, and neither tool looks at the
other's. At 0.2 the two schemas already diverge — `call-mech` and the
differently-shaped `run` block exist only here.

## Discovery

`tmt.json` is found by two independent ancestor walks, each starting from
a directory and stopping at its **first** match — neither walk ever
merges settings across more than one file:

- **Lint discovery** (unchanged): the nearest ancestor `tmt.json`,
  whether or not it has a `project` section.
- **Project discovery**: the nearest ancestor `tmt.json` that *has* a
  `project` key. A `tmt.json` with only a `lint` section, sitting between
  a source file and its project root, is invisible to this walk — the
  lint walk still stops there for lint purposes, but the project walk
  passes through it and keeps looking further up.

`tmt build` runs project discovery from the current directory; a build
started from a subdirectory still resolves against the manifest found
above it (see Path rules below). `tmt lint` and `tmt fmt`, invoked with
no file arguments, use the same discovery — see The declared source set.

### One loader

One loader validates the **whole file** — both sections, every key —
regardless of which one a given consumer asked for. A typo in `project`
still fails a lint-only load of the same file, and vice versa: the two
walks can never disagree about whether a `tmt.json` is well-formed.

## Example

```json
{
  "lint": { "allow": ["unused-exit"] },
  "project": {
    "stdlib": true,
    "sources": ["src/shared.tmc"],
    "libraries": { "dirs": ["libs"], "link": ["tables"] },
    "call-mech": "hybrid",
    "profiles": {
      "release": { "werror": true }
    },
    "targets": {
      "app": {
        "sources": ["src/app.tmc"],
        "output": "out/app.tmx",
        "run": { "tape": "tapes/app-in.tmt", "max-steps": 100000 }
      },
      "bench": {
        "sources": ["src/bench.tmc", "src/tables.tma"],
        "entry": "bench",
        "call-mech": "mono",
        "run": { "tape": "tapes/bench-in.tmt", "no-step-limit": true }
      }
    }
  }
}
```

Two targets, `app` and `bench`, share `src/shared.tmc` and the `tables`
library declared at project level, each adding its own sources and run
settings. `tmt build` (no arguments) builds both; `tmt build app` builds
only `app`; `tmt build --run bench` builds `bench` and then runs it
against `tapes/bench-in.tmt`. `bench` also pins its own lowering,
overriding the project-level `call-mech`.

## Schema reference

### Project-level keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `stdlib` | bool | `true` | `false` is the manifest form of `--nostdlib`: every target in this file links without the standard library. Project-level only — there is no per-target override. |
| `sources` | array of strings | `[]` | Source paths prepended to every target's own `sources`, in order. |
| `libraries` | object | `{}` | `dirs` (search directories) and `link` (library names), each prepended to every target's own list, in order. |
| `call-mech` | `"mono"` \| `"frames"` \| `"hybrid"` | absent | Default bound-call lowering for every target in this file — see Call mechanism below. |
| `profiles` | object | `{}` | Overrides for the `debug` and `release` profile bases — see Profiles below. |
| `targets` | object | — | **Required, at least one entry.** Named build targets — see Targets below. |

### Targets

Each key under `targets` is a target name matching
`[A-Za-z0-9][A-Za-z0-9_-]*` (dot-free, so a target name can never be
mistaken for a file positional). Targets are otherwise independent; the
map is read in alphabetical-by-name order, which is the documented
cross-target build order for a bare `tmt build`.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `sources` | array of strings | `[]` | Appended after the project-level `sources` to form this target's effective source list, in order. May be `.tmc`, `.tma`, or `.tmo` — the same per-extension dispatch (compile / assemble / load) `tmt build`'s argv mode uses. |
| `libraries` | object | `{}` | `dirs`/`link`, appended after the project-level lists to form this target's effective libraries. |
| `entry` | string | `"main"` | The linker's reachability root for this target (must not be an empty string) — see Entry below. |
| `output` | string | `"<target-name>.tmx"` | Output path, resolved against the manifest's directory. Two targets whose (normalized) output paths collide is a manifest error. |
| `call-mech` | `"mono"` \| `"frames"` \| `"hybrid"` | project-level value | This target's bound-call lowering, overriding the project default. |
| `run` | object | absent | Optional run settings for `tmt build --run` — see The `run` block below. |

Library resolution is **first-wins** in declared order, and a user
definition of the same exported symbol silently shadows a library's — no
warning is emitted (`docs/core.md (linking)`). Linking is lazy by
reachability, so a declared library that no effective source references
contributes nothing to the output.

### Entry

`entry` names the symbol the program starts from and the root of the
reachability walk (`docs/tmt/cli.md (link)`). The manifest's job stops at
naming it: everything the name has to satisfy is enforced by the linker,
at link time, not by manifest validation.

That enforcement is heavier on TM-1 than the flat PM-1 case, because the
entry is what fixes the shape of the machine itself:

- **The entry's routine signature is the machine's tape arity and
  per-tape cardinalities.** Every reachable frame descriptor's
  physical-tape indices are validated against it.
- **In sectioned output — one carrying tables, or any signature at all,
  which is what the frames and hybrid lowerings produce — the entry
  must carry a routine signature.** An entry without one is a link error
  naming the function. The same applies to a reachable declarative bound
  call under any lowering: there is no machine signature to compose
  against without one.

A name no object defines, an unexported one, or one whose signature the
image needs and does not have, are therefore all link-time errors, not
manifest-validation ones. Two targets over the same sources with
different entries — a natural way to build several machines out of one
library of routines — is expected and supported.

### Call mechanism

`call-mech` selects how declarative bound calls are lowered
(`docs/tmt/cli.md (link)`, `docs/core.md (the composition engine)`):
`mono` stamps a specialized copy per call site, `frames` routes every
site through the generic compose directory, and `hybrid` classifies per
site — a completed bijection stamps like mono, anything holey or one-way
goes through frames.

It resolves in four steps, first match winning:

1. the `--call-mech` flag, if given;
2. the building target's own `call-mech` key;
3. the project-level `call-mech` key;
4. the linker's own default, `hybrid`.

`--call-mech` is the one link-side flag `tmt build`'s manifest mode
accepts rather than rejects. The reasoning is that the manifest records
the lowering a target is *committed* to, while the flag exists to
experiment against that commitment for a single build without editing
`tmt.json` (`docs/tmt/cli.md (build)`).

### Profiles

`profiles` holds at most two keys, `debug` and `release` — any other name
is rejected. Each names up to four overrides, all optional:

| Key | Type | Meaning |
|---|---|---|
| `opt` | `"O0"` \| `"O1"` | Optimization level. |
| `debug-info` | bool | Record debug info (labels + `.tmc` lines). |
| `strip-debugger` | bool | Drop `brk` at codegen. |
| `werror` | bool | Treat post-refinement warnings as errors. |

An override layers on top of one of two fixed bases, which mirror the CLI
presets (`docs/tmt/cli.md`, `tmt compile`'s `--debug`/`--release`):

| Base | `opt` | `debug-info` | `strip-debugger` | `werror` |
|---|---|---|---|---|
| `debug` | `O0` | `true` | `false` | `false` |
| `release` | `O1` | `false` | `true` | `false` |

Profile selection happens once per `tmt build` invocation, not per
target: `--release` selects the `release` base for every target built in
that invocation, and omitting both `--release` and `--debug` selects the
`debug` base — there is no per-target profile name in the schema. This
means a bare `tmt build` (no flags at all) still resolves the full
`debug` base, including `debug-info: true` — unlike `tmt compile`'s own
true no-flags default, which carries no debug info. The individual
compile-side flags (`-g`, `-O0`/`-O1`, `--strip-debugger`, `-Werror`)
override the resolved profile's matching key for that invocation only;
the manifest is never rewritten (`docs/tmt/cli.md (build)`, "flags win").

Two compile-side flags have **no profile key at all**: `--fno-<pass>` and
`--foutline`. Pass selection is not part of the schema, so those two
always come from the command line, layered on whichever profile is in
force.

### The `run` block

A target's optional `run` object is read only by `tmt build --run`
(`docs/tmt/cli.md (build)`):

| Key | Type | Meaning |
|---|---|---|
| `tape` | string | Path to a `.tmt` tape-band snapshot, as `tmt run --tape`. |
| `max-steps` | non-negative integer | Step budget. |
| `no-step-limit` | bool | Remove the step budget entirely. |
| `max-tacts` | non-negative integer | Tact budget. |

`max-steps` and `no-step-limit` are mutually exclusive, rejected at
manifest-validation time.

This block is deliberately smaller than PM-1's, because `tmt run` is a
narrower tool. It always drives a whole multi-tape band loaded from a
`.tmt` snapshot: there is no inline-glyph tape form, no initial head
position, no strict-cells decorator, and no tact-profile knob for it to
carry. That also makes `tape` the one key a runnable block cannot omit —
`tmt run` has no empty-tape default to fall back on, so **a target whose
`run` block declares no `tape`, or which has no `run` block at all,
cannot be `--run`** and names the target in a pointed error instead. This
is the one place the TM-1 manifest is stricter than PM-1's, where an
absent or empty `run` block silently falls back to that tool's own
defaults.

A block with no `tape` is still *valid* in the schema — it is only
`--run` that refuses it. `tmt build --list-targets`' `run` marker
reflects whether the `run` *key* is present, not whether the target can
actually be run.

## Schema rules

- **Unknown keys are rejected everywhere** — at the top of `project`,
  inside `libraries`, inside a profile, inside a target, and inside a
  `run` block — with an error naming the file and the offending key.
- **`targets` must have at least one entry.**
- **Target names** must match `[A-Za-z0-9][A-Za-z0-9_-]*`.
- **Profile names** must be `debug` or `release`.
- **`call-mech` values** must be `mono`, `frames`, or `hybrid`.
- **`entry`**, if given, must not be an empty string.
- **`max-steps` and `no-step-limit`** cannot both be set on one `run`
  block.
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
  beyond reading `tmt.json` itself. A source, library directory, or
  `tape` path that doesn't exist on disk surfaces as an ordinary "cannot
  read" error only when something actually tries to read it — building
  the target, running it, or (for sources) linting/formatting it.

## Path rules

Every path in the manifest — `sources`, `libraries.dirs`, a target's
`output`, and a run block's `tape` — resolves against **the manifest's
own directory**, never the process's current directory. A build started
from a subdirectory still writes outputs and reads sources relative to
where `tmt.json` was found, not where `tmt build` was invoked from.
(`libraries.link` entries are library *names*, not paths, and are
resolved against `libraries.dirs` at build time, not normalized as
paths.)

- **Absolute paths are rejected** — a manifest is a committed, portable
  artifact; encoding one machine's layout into it defeats that.
- **`../` is allowed** — traversing above the manifest's directory is a
  normal way to share sources or libraries across a tree of projects.
- **Normalization is lexical only**: `.` components are dropped, and an
  interior `..` cancels the path segment before it (`src/../m.tmc`
  normalizes to `m.tmc`, and `./out.tmx` normalizes to `out.tmx` — this
  is what the duplicate-source and output-collision checks compare
  against). A *leading* `..` cannot cancel anything and is kept as-is.
  This normalization does not consult the filesystem: two paths that
  reach the same file only through a symlink are not detected as
  duplicates. That is a documented limitation, not a planned fix.

## The declared source set

Every target's effective source list (project-level `sources` followed by
the target's own) can be unioned across the whole manifest into one flat
list — first occurrence wins in target-alphabetical, then list order,
deduplicated after path normalization, with `.tmo` entries dropped (an
object file carries no text, so there is nothing in it to lint or
format). This is **the declared source set**.

`.tma` entries stay in it. Both TM-1 languages have a lint layer and a
formatter, so a hand-written assembly source named by a target is as
much part of the project's text as a `.tmc` one — the only extension
dropped is the object.

The declared source set is exactly what a bare invocation — no file or
directory arguments — of `tmt lint` or `tmt fmt` operates on: both
discover the nearest project manifest from the current directory (the
same discovery `tmt build` uses) and lint or format precisely its
declared sources, never a directory scan. A file sitting next to a
declared source but not named by any target's `sources` is invisible to
a bare invocation, even though it would be picked up by a directory scan
or an explicit path argument.

A bare invocation with no manifest found upward from the current
directory is an error naming what was searched for — the same error
`tmt build` (with no target names, from an unmanifested directory)
gives.

`tmt lint --no-config` cannot be combined with a bare invocation: with no
file arguments, the manifest's declared set *is* the input, so skipping
project discovery would leave nothing to lint. `--no-config` still works
normally alongside explicit file or directory arguments, where it only
suppresses the per-file lint allow-list lookup. `tmt fmt` has no
`--no-config` flag at all — only `tmt lint` does — so this particular
conflict cannot arise for `fmt`; a bare `tmt fmt` with a discovered
manifest always formats its declared set.

## How `tmt build` consumes it

`tmt build` is the one manifest consumer among the CLI subcommands;
`compile`/`asm`/`link`/`run` stay purely argv-driven low-level tools.
Full flag reference, argv-mode-vs-manifest-mode dispatch, `--run`,
`--list-targets`, and the flags manifest mode rejects are all on
`docs/tmt/cli.md (build)` — this section only summarizes what's specific
to the manifest itself.

For each target it builds, `tmt build` compiles/assembles/loads that
target's effective sources, links them against its effective libraries
(plus the standard library unless `stdlib: false`) with its resolved
profile, its `entry`, and its resolved lowering, and writes its `output`
(plus the `.tmx.map` sidecar) next to the manifest. The ordinary per-file
"undeclared external" compile warning — which fires on a bare call a
given file doesn't import — is refined the same way it is in argv mode:
once a target's whole effective source set is known, a bare call resolved
by some *other* file in that same target's set no longer warns. This
refinement is scoped per target, not across the whole manifest — a name
defined only in a sibling target's own (non-shared) source does not
refine another target's warnings. `tmt compile`, working one file at a
time with no visibility into any declared set, stays per-file honest and
always warns on a bare undeclared call.
