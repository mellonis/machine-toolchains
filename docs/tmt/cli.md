# The `tmt` command-line tool

`tmt` drives the TM-1 toolchain: `.tmc` source through the compiler to a
`.tmo` object, objects through the linker to a `.tmx` executable, and that
image through the VM over a multi-tape `.tmt` tape block. It is the sibling
of `pmt` (`docs/pmt/cli.md`) and deliberately mirrors its subcommand shapes,
so the two tools read the same way where the architectures allow it.

Like `pmt`, `tmt` follows the **thin-renderer rule** (`docs/core.md`): library
code never prints, every stage returns a structured report, and `-v` on the
relevant subcommand renders that report as text. Errors flow back as typed
values and are rendered in exactly one place. This is why an embedder can
call `compile` / `assemble` / `link` / `disassemble` / `Machine` directly and
get the same results without a subprocess.

```
tmt — Turing-machine toolchain (TM-1)

USAGE: tmt <SUBCOMMAND> [ARGS]

SUBCOMMANDS:
  compile      .tmc source -> .tmo object (-S for .tma, --emit-ir for world IR JSON)
  asm          .tma assembly -> .tmo object
  link         .tmo objects -> .tmx executable (+ .tmx.map sidecar)
  dis          disassemble a .tmo or .tmx (--listing for the address view)
  run          execute a .tmx on a multi-tape .tmt block
  tape         new/set/show .tmt tape-block snapshots
  ir           render --emit-ir JSON (ir graph -> Mermaid)
  lint         hygiene findings over .tmc and .tma sources
  fmt          canonical formatting for .tmc and .tma sources
  lsp          run the LSP server for .tmc and .tma on stdio
  completions  emit a shell completion script (zsh; bash/fish follow-on)

Run `tmt <SUBCOMMAND> --help` for details. `tmt --version` prints the version.
```

`tmt --version` prints three lines: `tmt <VERSION>` (the toolchain crate's
own version), `tmc language <VERSION>` (the `.tmc` language
acceptance-contract version — `docs/tmt/language.md`), and
`tma dialect (tm-1) <VERSION>` (the TM-1 `.tma` dialect version —
`docs/formats.md (assembly text)`). The three numbers move on independent
axes: a crate release with no grammar change repeats the same
language-version and dialect-version lines, and each grammar version only
bumps when its own grammar changes.

Every usage block on this page is the subcommand's real `--help` output,
quoted verbatim and checked against the binary by a test — this page is a
reference, not a paraphrase.

## `tmt compile`

```
USAGE: tmt compile INPUT.tmc [-o OUT.tmo] [FLAGS]

FLAGS:
  -g                 record debug info (labels + .tmc lines)
  -O0 | -O1          optimization level (default -O0)
  --strip-debugger   drop `brk` at codegen
  --debug            preset: -g -O0
  --release          preset: -O1 --strip-debugger
  -S                 emit the generated .tma instead of an object
  --emit-ir[=STAGE]  write the world-graph IR JSON next to the output
                     (STAGE: lowered | final | after:<pass> for a registered
                      pass; default final)
  --fno-<pass>       disable one optimizer pass (repeatable)
  --foutline         enable the default-off `outline` optimizer pass
  -Werror            treat warnings as errors
  -v                 render the compile report (passes, rounds)
```

Consumes one `.tmc` source; produces a `.tmo` object (or, with `-S`, the
generated `.tma` assembly text). Without `-o` the output takes the input's
name with the extension replaced.

`--debug` and `--release` are presets applied *before* the individual flags,
so `-O0` / `-O1` / `-g` / `--strip-debugger` can still override one piece of
a preset on the same command line. The default build (no flags) is `-O0`
with no debug info.

`-g` records the label/line debug section, which the linker carries into the
`.tmx.map` sidecar and `dis` / `run --trace` read back as real names.
`--strip-debugger` drops `brk` at codegen; note that `brk` is also an
observability barrier the optimizer will not move code across, so stripping
it and optimizing are related choices rather than independent ones
(`docs/tmt/isa.md`).

Compile warnings always print to stderr as `FILE:LINE:COL: warning: MESSAGE`.
`-v` additionally renders the optimizer's report — the number of fixpoint
rounds and, per round, each pass's change count per world. `-Werror` turns
every warning into a compile failure.

### `-O0` and `-O1`

`-O0` runs no optimizer at all, and its output is byte-identical to plain
codegen — no optimizer artifact leaks into an unoptimized build. `-O1` runs
the full pass pipeline to a fixpoint. The pass list is owned by the
optimizer, and both `--fno-<pass>` and `--emit-ir=after:<pass>` read that
same list rather than a retyped copy, so a stage name and a disable flag can
never name a pass that does not exist. Ask the binary for the current set by
naming an unknown stage:

```
$ tmt compile prog.tmc --emit-ir=after:bogus
tmt: unknown IR stage `after:bogus` (lowered | final | after:inline |
after:outline | after:jump-threading | after:tail-call | after:tail-merge |
after:dce | after:dead-rows | after:dispatch-select)
```

### `--fno-<pass>` and `--foutline`

`--fno-<pass>` disables one optimizer pass and is repeatable — `--fno-inline
--fno-dce` disables both. It is a flag *family*: one full flag per pass name,
not a `name=value` pair.

`outline` is the one pass that defaults **off**, so it has flags with both
senses and both are real. `--foutline` turns it on; `--fno-outline` — which
the family renders because `outline` is a registered pass like any other —
keeps it off. `--foutline` takes effect only at `-O1`, because that is the
only level at which the optimizer runs.

### `--emit-ir`

`--emit-ir` writes the world-graph IR as JSON next to the output, at
`<output base>.ir.json`. The IR is a documented, versioned artifact rather
than an internal detail (`docs/formats.md (IR JSON)`).

The stage argument is **equals-only**: `--emit-ir` and `--emit-ir=STAGE` are
both accepted, but `--emit-ir STAGE` as two tokens is not — the stage would
be left behind and rejected as a stray positional. `STAGE` is one of the
pipeline bookends `lowered` / `final` (the default), or `after:<pass>` for
any registered pass. An unknown stage is rejected up front, with an error
naming every stage that does resolve, rather than late as a missing
snapshot. A stage label captured in several optimizer rounds resolves to the
last snapshot captured under it. The flag itself may appear only once per
command line; repeating it is an unknown-flag error.

### Compile errors

A fatal compile error stops the compile and renders as
`FILE:LINE:COL: error: MESSAGE [CODE]`. The bracketed suffix is a stable
kebab-case identifier for the error kind — safe to match in scripts and
editor integrations. The same rendering carries the same codes wherever a
fatal surfaces: `tmt compile` itself, and the per-file fatal lines of
`tmt lint` and `tmt fmt`.

## `tmt asm`

```
USAGE: tmt asm INPUT.tma [-o OUT.tmo] [-g]
```

Assembles hand-written or disassembled `.tma` text into a `.tmo` object;
`-g` records the label/line debug section. The TM-1 `.tma` dialect enables
the assembler's full capability set — sections, match and dispatch tables,
`.rept` macros, vector operands, `.routine` signatures, and frame
descriptors (`docs/formats.md (assembly text)`, `docs/tmt/isa.md`). A fatal
assembly error renders in the same `FILE:LINE:COL: error: MESSAGE [CODE]`
shape as a compile error, with the assembler's own stable codes
(`docs/core.md (error codes)`).

## `tmt link`

```
USAGE: tmt link INPUT.tmo... [-o OUT.tmx] [FLAGS]

FLAGS:
  --no-relax        keep every call site in far form
  --entry NAME      link NAME as the program entry (default: main)
  --call-mech MECH  bound-call lowering: mono | frames | hybrid (default: hybrid)
  --nostdlib        do not auto-link the embedded standard library
  -L DIR            add a library search directory (repeatable, in order)
  -l NAME           link NAME.tmo from the search path (repeatable)
  -v                render the link report (dropped functions, relaxation)

Writes OUT.tmx and the OUT.tmx.map sidecar (function ranges + table
section info; label/line info when the objects carry -g debug data).
```

Consumes one or more `.tmo` objects; produces a `.tmx` executable plus its
`.tmx.map` JSON sidecar. Without `-o` the output name derives from the first
input. Linking is two-phase — resolve (namespace plus reachability from the
entry, dropping unreachable functions) then layout, whose relaxation is a
shrink-only fixpoint narrowing far calls to short. `--no-relax` keeps every
site far. `docs/core.md (the linker)` has the mechanism.

`-v` renders which defined-but-unreachable functions were dropped and how
many sites relaxed short versus stayed far. When the image carries frames
content, a second line reports the composition-engine counters — composites,
stamps, compose-table bytes, dedup savings, synthesized trap rows, expanded
rows — so a frameless link keeps the single-line report.

### `--call-mech`

Selects how declarative binding calls are lowered. The three values are
different implementations of one contract, and a program's observable
behaviour is identical under all three:

| Value | Lowering |
|---|---|
| `mono` | Stamps a specialized copy of the callee per call site, with row rewriting, synthesized trap rows, and digest-named deduplication. No frame indirection at run time. |
| `frames` | Routes every site through the frames execution profile: the FR register plus a composite directory, so one copy of the callee serves every site. |
| `hybrid` | The default. Chooses per call site. |

The distinction is a size/indirection trade rather than a semantic one:
`mono` spends image space to avoid run-time frame lookup, `frames` spends a
compose table to avoid duplicated code. `docs/tmt/isa.md (call mechanisms)`
describes what each lowering means at the machine level, and
`docs/core.md (the composition engine)` the link-time algebra behind it.

The value set is closed and case-sensitive; anything else is rejected by
name:

```
$ tmt link prog.tmo --call-mech nope
tmt: unknown --call-mech `nope` (expected one of: mono, frames, hybrid)
```

### `--entry`

Names the symbol the program starts from and the root of the reachability
walk; the default is `main`. A name no object defines is an error
(`tmt: no `nosuch` entry symbol`). Because reachability is computed from the
entry, `--entry` changes not only where execution begins but which functions
survive into the image.

### `--nostdlib`

Linking always appends the embedded standard library as an implicit last
library unless `--nostdlib` is given (`docs/tmt/stdlib.md`). It is linked
*lazily*, through the same reachability pass as everything else, so a
program that calls nothing from it pays nothing — the stdlib's routines
simply appear in the dropped list under `-v`. Libraries are first-wins, so
command-line objects and explicit `-l` libraries shadow a stdlib definition
of the same name.

Explicit `-l NAME` resolves `NAME.tmo` against the `-L` directories in the
order given, and errors if it is not found on any of them. There is no
on-disk library directory to fall back to: the standard library is embedded
in the toolchain binary itself.

## `tmt dis`

```
USAGE: tmt dis FILE.tmo|FILE.tmx [--listing] [--map FILE.tmx.map]

Objects disassemble with real names from the symbol table. Executables
use the .tmx.map sidecar when present (FILE.tmx.map or --map), else
recursive-descent discovery (func_XXXX). --listing prints the debugger
code view: addresses + raw bytes, not reassembleable.
```

Accepts either a `.tmo` or a `.tmx` on the same command line, told apart by
magic sniffing rather than by extension (`docs/formats.md`). Handed a `.tmt`
tape block it says so and points at `tmt tape show`.

**Sidecar discovery:** an explicit `--map` always wins; failing that, `tmt`
looks for `FILE.tmx.map` beside the executable. A missing or unparsable
sidecar found by *implicit* discovery is silently ignored — a stale sidecar
must never break plain `dis` or `run`. An unparsable *explicit* `--map` is an
error.

**`--listing` vs canonical `dis`:** the default output is the canonical
`.tma` grid — valid, reassembleable assembler input, complete with the
`.routine` signature and table sections. `--listing` instead prints the
debugger code view: one line per instruction with its address and raw hex
bytes, every byte in the image accounted for including bytes no control-flow
path reaches, and branch/call targets resolved to `function` /
`function.label` names when a map is available. That view is not
reassembleable; it exists to inspect what a `.tmx` actually contains, byte
for byte. `--listing` applies to executables only.

## `tmt run`

```
USAGE: tmt run APP.tmx --tape TAPES.tmt [FLAGS]

TAPE:
  --tape TAPES.tmt    load the initial tape band from an MT snapshot
                      (one band per image tape; alphabets sized per band)

LIMITS:
  --max-steps N       step budget (default 10000000)
  --no-step-limit     remove the step budget
  --max-tacts N       tact budget

OUTPUT:
  --trace             stream per-instruction listing lines to stderr, live,
                      each with post-state `; MF=<0|1> heads=[..]`
                      (a frames-profile image also appends ` FR=<n>`)
  -v                  no extra effect yet (stats always print)

EXIT CODE: 0 stopped | 2 halted (hlt) | 3 trapped | 1 tool error.
```

Consumes a `.tmx` image and a `.tmt` tape block; prints the outcome, the
step and tact counts, and every tape's final contents with its head marked.

`--tape` is **required** — unlike `pmt run`, which defaults to an empty tape,
a TM-1 image runs a whole band of tapes and there is no inline glyph-pattern
form to build one from. Mint a template with `tmt tape new --from` and fill
it in with `tmt tape set`. The block's band count must equal the image's tape
count; a mismatch is a tool error naming both numbers:

```
$ tmt run two-tape.tmx --tape one-tape.tmt
tmt: one-tape.tmt has 1 tape(s), but two-tape.tmx expects 2
```

Each band is driven through its own effective alphabet — its own embedded
glyph table if it has one, otherwise the block's.

`--max-steps` defaults to 10,000,000; `--no-step-limit` removes the budget
entirely, for a program you trust to terminate. `--max-tacts` has no default,
so tacts are unlimited unless set. Both budgets are enforced as traps, not as
tool errors — see the exit codes below. `-v` is accepted for symmetry with
the other subcommands but currently has no additional effect: the outcome and
stats print regardless.

**`--trace` format:** streams live to stderr, one line per retired
instruction, in the same address/bytes/mnemonic shape as `dis --listing`,
with a post-execution state suffix `; MF=<0|1> heads=[..]` listing every
head. The state shown is the one *after* that instruction's effect. An image
built on the frames profile appends ` FR=<n>`, the frame register; a
base-profile image's line is byte-identical without it.

### Exit codes

| Code | Outcome | What it means |
|---|---|---|
| `0` | `Stopped` | The program reached `stp` — a normal, successful end. |
| `2` | `Halted` | The program reached `hlt` — an abnormal end the program chose. |
| `3` | `Trapped` | The machine faulted, or a budget ran out. |
| `1` | tool error | Bad arguments, unreadable file, malformed container, band-count mismatch — never a program outcome. |

For a program author the useful split is between `2` and `3`. `hlt` is
*your* code deciding the input was unacceptable; a trap is the machine
saying the program did something it could not do — an unmapped read, a
`retx` past its exit count, an explicit `trap`, or a budget exhausted. The
outcome line names which:

```
$ tmt run prog.tmx --tape t.tmt --max-steps 1
outcome: Trapped(StepLimit)
```

Because budget exhaustion is a trap, exit `3` does not by itself mean the
program is wrong — it may only mean it needed a longer leash. Read the
outcome line before concluding. `docs/tmt/isa.md` covers trap kinds at the
machine level.

## `tmt tape`

```
USAGE: tmt tape new --from APP.tmx [-o OUT.tmt]
       tmt tape set IN.tmt (-o OUT.tmt | --in-place)
                    [--tape N] [--cells PATTERN] [--origin N] [--head N]
       tmt tape show FILE.tmt

new: a blank template sized to the executable's tape count, each tape's
alphabet the decimal labels 0..card-1 from the image's per-tape
cardinalities. set: clone IN.tmt, applying edits to tape N (default 0);
--cells maps each character through tape N's effective alphabet. show:
renders any .tmt with its own alphabet.
```

Three subcommands author and inspect `.tmt` tape-block snapshots without
hand-editing bytes. There is no `tape build`: PM-1's is glyph-pattern sugar
tied to a fixed two-symbol alphabet, and TM-1 tapes carry per-tape
alphabets, so cells are set through `set --cells` against a template minted
by `new --from`. Note that the group's children take no `--help` of their
own — run `tmt tape` bare for the usage above.

**`tape new --from APP.tmx`** writes a blank snapshot shaped to a specific
program: one empty band per tape the image expects, origin and head at 0,
each band carrying its own alphabet of the decimal labels `0..card-1` taken
from the image's per-tape cardinalities. `--from` must be an executable
image, magic-sniffed; anything else is an error. `-o` names the output
(default `blank.tmt`).

**`tape set IN.tmt`** has clone semantics: it reads the input, applies edits
to one band, and writes the result elsewhere; the source is never modified
unless you ask. Exactly one output destination is required — `-o OUT.tmt`
writes a new file, `--in-place` writes back over the input — and the two are
mutually exclusive. Supplying neither is an error; refusing the ambiguous
case is what keeps `set` from silently clobbering its input. Any subset of
the edit flags may be given, and with none `set` is a plain copy.

Edits target the band selected by `--tape N` (default `0`); an index past
the block's tape count is an error naming how many bands it has. `--origin`
and `--head` take an `i64`, negatives included. `--cells PATTERN` replaces
the band's cells, resolving each character through *that band's effective
alphabet* — its own glyph table if present, otherwise the block's — with the
leftmost character as cell 0. A character outside that alphabet is an error
listing the alphabet it was checked against:

```
$ tmt tape set t.tmt -o out.tmt --cells "Z"
tmt: bad cell character `Z` (alphabet: ["0", "1", "2"])
```

Only the flags you pass change; every other band and every unspecified field
is copied through untouched.

**`tape show FILE.tmt`** renders any snapshot through its own alphabets —
the block's fallback plus each band's override — so it works for tapes built
against a glyph set the reader knows nothing about.

## `tmt ir`

```
USAGE: tmt ir graph FILE.ir.json [--function NAME]

Renders --emit-ir output as a Mermaid flowchart (one per world). The filter
flag keeps pmt's `--function` name for cross-tool muscle memory; a TM world
IS the unit here (the `machine` block or a routine), so NAME is a world name.
```

Reads a `--emit-ir` JSON file and renders each world's state graph as a
Mermaid `flowchart TD`. `--function NAME` restricts output to one world;
naming a world the file does not contain is an error. As with `tape`, the
`graph` child takes no `--help` — run `tmt ir` bare for the usage above.

## `tmt lint`

```
USAGE: tmt lint PATH... [--exclude PATH]... [--allow CODE]... [--warn CODE]... [--no-config]

PATH is a .tmc or .tma file, or a directory; directories are walked
recursively for *.tmc and *.tma (sorted order, symlinks not followed,
dot-entries skipped). .tmc sources lint through the .tmc rule table;
.tma sources through the five arch-agnostic asm rules plus the TM-1
additions (shadowed rows, retx exit bounds, unused rept vars).

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs); exclusion
                  wins even over explicitly listed files
  --allow CODE    suppress a lint rule by code (repeatable; unknown codes
                  are an error)
  --warn CODE     enable an opt-in rule by code (repeatable; e.g.
                  state-may-trap, off unless named here)
  --no-config     ignore tmt.json project files
```

PATH is a `.tmc` or `.tma` file, or a directory. Directories are walked
recursively for `*.tmc` and `*.tma` in sorted order; symlinks are never
followed and dot-entries (`.git`, editor scratch) are skipped. A PATH that
yields no `.tmc` / `.tma` files is an error. `--exclude PATH` (repeatable)
skips a file or prunes a subtree; paths are compared as spelled, with no
globs — the shell covers the include side — and exclusion wins even over an
explicitly listed file.

Each file's extension picks its rule table, and the two tables share one
allow namespace, so a single allow-list works across a batch mixing both
languages. The rule catalog is `docs/tmt/lint.md`. An explicitly listed file
with neither extension is a per-file error and the batch continues; the
directory walk itself never collects any other extension, so this only fires
for a file named directly on the command line.

Files lint independently: one that fails to parse is reported on stderr as a
fatal error line with its bracketed code, and the batch keeps going.
**Exit codes: 0 = every file clean, 1 = findings or errors anywhere** (tool
errors are also 1). This is a different convention from `tmt run`'s — `tmt
lint` and `tmt fmt` report on *sources* and only ever exit 0 or 1; the
0/2/3 outcome codes belong to running a program.

### Opt-in rules and `--warn`

Most rules are on by default. A rule that would be too noisy as a default is
**opt-in**: off unless `--warn CODE` names it. `--allow` and `--warn` draw
from the same namespace, and **allow beats warn** — a code that is both
allowed and warned stays suppressed. An unknown code named by either flag is
a whole-tool error that aborts the run before any file is touched, because
the flag applies to the entire run rather than to one input.

```
$ tmt lint prog.tmc
$ tmt lint prog.tmc --warn state-may-trap
prog.tmc:11:9: lint: state `orphan` may trap — its rules do not cover every
input and there is no catch-all
```

### Project configuration

For each input file, `tmt lint` discovers a `tmt.json` by walking up from
that file's directory and unions its `lint.allow` with any `--allow` flags.
`--no-config` skips that discovery for every file, leaving the run governed
by the flags alone. See [`tmt.json`](#tmtjson) below.

There is no `--fix` on `tmt lint`: no `.tmc` or `.tma` rule emits a
machine-applicable fix, so there is nothing for it to apply.

## `tmt fmt`

```
USAGE: tmt fmt PATH... [--exclude PATH]... [--check]
       tmt fmt - [--check] [--lang tmc|tma]

PATH is a .tmc or .tma file, or a directory; directories are walked
recursively for *.tmc and *.tma (sorted order, symlinks not followed,
dot-entries skipped). `-` reads one source from stdin and writes the
result to stdout; it cannot be combined with PATH arguments.

.tma sources format through the canonical assembly grid; .tmc sources
through the language's own canonical form (the state-block grid, the
80-column argument-list threshold). Both rewrites are whitespace-only.

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs)
  --check         do not write; with PATH..., list files that would be
                  reformatted and exit 1 if any would change; with -,
                  exit 1 if stdin would change (CI mode)
  --lang LANG     stdin's language: tmc (default) or tma; applies to
                  stdin (-) only — an error alongside PATH arguments,
                  whose language always comes from the file extension
```

The batch model is identical to `tmt lint`'s — the same walk, the same
sorted order, the same symlink and dot-entry rules, the same `--exclude`
semantics, the same per-file fatal that keeps the batch going. Each file's
extension picks its formatter. Both rewrites are whitespace-only and
idempotent, which is what makes `--check` a safe CI gate for either
language. The canonical styles themselves are `docs/tmt/fmt.md`.

By default `tmt fmt` rewrites each file in place, and only when its
formatted text differs from what is already on disk — an already-canonical
file is never rewritten, so a clean tree sees no spurious modification
times. `--check` writes nothing; it lists the path of every file whose
formatted text would differ and exits 1 if any did.

`-` reads one source from stdin and writes the formatted text to stdout
instead of walking directories; it cannot be combined with `PATH` arguments.
`--lang` picks stdin's language — `tmc` (the default) or `tma` — and is
meaningless with `PATH` arguments, where the extension already decides;
combining `--lang` with a `PATH` is an error. `- --check` mirrors the same
semantics against stdin: nothing is written either way, and the exit code
alone reports whether stdin would change.

Exit codes follow `tmt lint`'s convention: 0 = success (every input already
canonical, or rewritten in place); 1 = under `--check` at least one input
would change, or a lex/parse error occurred anywhere in the batch.

Unlike `tmt lint`, `tmt fmt` does **not** read `tmt.json` — formatting has no
configurable surface for a project file to set, and there is correspondingly
no `--no-config` flag on it.

## `tmt lsp`

```
USAGE: tmt lsp

Run the LSP server for .tmc and .tma on stdio until the client exits.
Exit code: 0 after shutdown/exit, 1 on exit without shutdown.
```

Runs one Language Server Protocol server for both `.tmc` and `.tma` on
stdio. `tmt lsp` is the only subcommand that hands real stdio to library
code — every protocol frame goes over stdin and stdout, exactly as the LSP
base protocol requires — so it is also the one subcommand that must not be
invoked casually from a script expecting it to return.

Two language services share the one process, routed per document by URI
extension and language id, with `.tmc` as the fallback a document binds to
when it matches neither. The process exit code follows the LSP lifecycle:
`0` after the client sends `shutdown` then `exit`; `1` if `exit` arrives
without a prior `shutdown`, or if the client disconnects without sending
either. `docs/lsp.md` has the capabilities table, the configuration
channels, and editor wiring.

## `tmt completions`

```
USAGE: tmt completions <SHELL>

Emits a shell completion script to stdout for the given SHELL (zsh; bash
and fish are recognized but not yet implemented).

  tmt completions zsh > ~/.zfunc/_tmt
```

`tmt` is hand-rolled with no argument-parsing framework, so a completion
script cannot be generated by one. Instead the whole CLI surface — every
subcommand, each one's flags with their value shapes, exclusive groups, and
each positional's file-extension filter — is described once in an in-crate
registry, and each shell renderer reads that. A drift-guard test probes the
real parser with every registry entry and cross-checks the `--fno-<pass>`
and `--emit-ir=after:<pass>` choices against the optimizer's own pass list,
so the generated script cannot quietly fall out of step with the flags the
parser accepts.

`zsh` completes subcommand names (including the nested `tape new` / `set` /
`show` and `ir graph`), each subcommand's flags, `-O0` / `-O1` as an
either/or pair, `--call-mech`'s three values, `--lang`'s two, the known
`--emit-ir` stages, and file arguments filtered to the extension the
subcommand actually reads — with directories offered alongside for `lint`
and `fmt`, which walk them. `bash` and `fish` are recognized shell names, so
naming one gives a message that says so rather than rejecting it as unknown;
neither renders a script.

## `tmt.json`

`tmt.json` is the TM toolchain's project file, and a **strict twin** of
PM-1's `pmt.json` (`docs/pmt/lint.md`): the same tiny schema, the same
discovery rule, the same merge semantics. Only the filename differs, which
means a repository holding both `.pmc` and `.tmc` sources keeps two separate
project files rather than one shared file with per-language sections.

The whole schema is one key:

```json
{
  "lint": {
    "allow": ["state-may-trap"]
  }
}
```

An empty object `{}` is valid — a `tmt.json` need not set anything to be
worth having, since its mere presence marks a subtree root. Any key outside
this schema is rejected by name (`unknown key `lnt``), as is an
`allow` entry naming no rule in the shared namespace. Validation is a manual
walk rather than a blanket deserialize, so a typo in a hand-authored file
points at the offending key instead of failing the whole document
generically.

Note what is **not** in the schema: there is no `warn` key. A `tmt.json`
can suppress a rule but cannot turn an opt-in rule on; that is the `--warn`
flag's job on the command line, and the editor-settings channel's in an IDE.

### Discovery: nearest ancestor, never a cascade

For each input file, discovery walks up from that file's directory to the
filesystem root and takes the **first** `tmt.json` it finds. A `tmt.json`
further up the tree is then not read at all — configuration does not
cascade, and settings do not accumulate down a path. Two files under
different nearest configs in one run may end up with entirely different
allow-lists, by design: a subtree opts into its own configuration by having
its own file. Relative paths are absolutized before the walk, so a run
started from a subdirectory still discovers a `tmt.json` above the working
directory.

### Union with editor settings

An editor supplies a second configuration channel of its own. The two are
combined as a **union**, project file first, and again never as a cascade:
codes from the discovered `tmt.json` and codes from editor settings both
take effect, and neither channel replaces or overrides the other. On the
command line the same rule applies between `tmt.json` and `--allow` — the
effective allow-list is the union of both.

The practical consequence is that removing a code from one channel does not
necessarily re-enable the rule; it stays suppressed if the other channel
still names it.

### Which surfaces read it

| Surface | Reads `tmt.json` |
|---|---|
| `tmt lint` | Yes — per input file, unioned with `--allow`; `--no-config` opts out. |
| `tmt lsp` (both `.tmc` and `.tma` services) | Yes — per document, mtime-cached, unioned with editor settings; both services watch `**/tmt.json` so an edit re-resolves. |
| `tmt fmt` | No. |
| every other subcommand | No. |

A `tmt.json` that fails to parse or validate is a **per-file fatal**, exactly
like a source file that fails to parse: it is reported on stderr with its own
path, the file it would have configured is skipped, and the batch continues.

```
$ tmt lint proj/sub/prog.tmc
/tmp/proj/tmt.json: error: unknown lint rule `nope` in lint.allow
```

That differs from an unknown code named directly by `--allow`, which is a
whole-tool error — that flag applies to the entire run, so there is no single
file to skip. In the language server the same failure surfaces as an
invalid-configuration diagnostic rather than stopping the session.
