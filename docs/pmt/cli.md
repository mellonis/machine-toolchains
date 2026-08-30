# The `pmt` command-line tool

`pmt` follows the **thin-renderer rule**: library code never prints, and
every stage returns a structured report (compiler warnings and an
optimizer report; the linker's dropped-functions and relaxation report);
`pmt -v` on the relevant subcommand renders that report as text. This is
why the CLI mirrors the library shape so closely, and why a future
embedder can consume `compile`/`assemble`/`link`/`disassemble`/`Machine`
directly without going through a subprocess at all.

```
pmt — Post-machine toolchain

USAGE: pmt <SUBCOMMAND> [ARGS]

SUBCOMMANDS:
  compile      .pmc source -> .pmo object (-S for .pma, --emit-ir for CFG JSON)
  asm          .pma assembly -> .pmo object
  link         .pmo objects -> .pmx executable (+ .pmx.map sidecar)
  build        compile+link driver: .pmc/.pma/.pmo inputs or manifest targets
  lint         lint .pmc/.pma sources (hygiene findings; docs/pmt/lint.md)
  fmt          format .pmc/.pma sources in place (--check to preview; -)
  dis          disassemble a .pmo or .pmx (--listing for the address view)
  run          execute a .pmx on a tape
  tape-block   build/new/set/show .pmt tape-block snapshots
  ir           render --emit-ir JSON (ir graph -> Mermaid)
  lsp          run the LSP server on stdio
  dap          run the DAP debug-adapter server on stdio
  completions  emit a shell completion script (zsh; bash/fish follow-on)

Run `pmt <SUBCOMMAND> --help` for details. `pmt --version` prints the version.
```

`pmt --version` prints three lines: `pmt <VERSION>` (the toolchain crate's
own version), `pmc language <VERSION>` (the `.pmc` language
acceptance-contract version — `docs/pmt/language.md`), and `pma dialect
(pm-1) <VERSION>` (the PM-1 `.pma` dialect version — `docs/pmt/asm.md`). The
three numbers move on independent axes: a crate release with no grammar
change repeats the same language-version and dialect-version lines, and
each of the two grammar versions only bumps when its own grammar changes.

Every flag below appears verbatim in the corresponding subcommand's
`--help` text; this page is a reference, not a paraphrase.

## `pmt compile`

```
USAGE: pmt compile INPUT.pmc [-o OUT.pmo] [FLAGS]

FLAGS:
  -g                 record debug info (labels + .pmc lines)
  -O0 | -O1          optimization level (default -O0)
  --strip-debugger   drop `brk` at codegen
  --debug            preset: -g -O0
  --release          preset: -O1 --strip-debugger
  -S                 emit the generated .pma instead of an object
  --emit-ir[=STAGE]  write the CFG IR JSON next to the output
                     (STAGE: lowered | after:<pass> | final; default final;
                      repeated stages resolve last-wins)
  --fno-<pass>       disable one optimizer pass (repeatable)
  -Werror            treat warnings as errors
  -v                 render the compile report (passes, rounds)
```

`--debug` and `--release` are presets, applied before the individual flags
so `-O0`/`-O1`/`-g`/`--strip-debugger` can still override a piece of a
preset on the same command line. The default build (no flags) is `-O0`, no
debug info. Compile warnings (undeclared externals, unused imports, unused
functions — `docs/pmt/language.md (visibility)`) always print to stderr as
`FILE:LINE:COL: warning: MESSAGE` (the column is new; `-Werror` semantics are
unchanged by it); `-v` additionally prints the optimizer's per-pass round
report; `-Werror` turns every warning into a compile failure. `--emit-ir`
writes `<output base>.ir.json` — see
`docs/pmt/language.md (the IR artifact)` and `docs/formats.md (IR JSON)`.
"Repeated stages resolve last-wins" refers to snapshot labels, not the
flag: a stage label captured in several optimizer rounds (e.g.
`after:inline`) resolves to the last captured snapshot, while the
`--emit-ir` flag itself may appear only once per command line —
repeating it is an unknown-flag error. A pass that never changed
anything captured no snapshot at all, so `--emit-ir=after:<pass>` for it
is an error rather than a silent fall-back.

The pass names `--fno-<pass>` and `--emit-ir=after:<pass>` accept, what
each pass does, and the contracts the whole pipeline holds to are
`docs/pmt/optimizer.md (passes)`; that page also works each pass through
a before/after IR example built with these two flags.

### Compile errors

A fatal compile error stops the compile and renders as
`FILE:LINE:COL: error: MESSAGE [CODE]`. The bracketed suffix is one of
this page's **error codes** — a stable kebab-case identifier for the
error kind — every fatal rendering carries it, wherever the fatal
surfaces (`pmt compile` itself, and the per-file fatal lines of
`pmt lint` and `pmt fmt`). Codes are permanent identifiers: they never
change meaning and are safe to match in scripts and editor
integrations.

| Code | Meaning |
|---|---|
| `lex-error` | The source failed to tokenize: an unexpected character, an unterminated comment, or similar lexical defect. |
| `unexpected-token` | The parser needed one construct and saw another. |
| `reserved-name` | A reserved word used to name a function, namespace, or path segment. |
| `unknown-command` | A bare identifier statement that is not a builtin — user functions are called `@name()`. |
| `builtin-called` | `@` applied to a builtin name (`@left()`) — builtins are written without `@`. |
| `empty-builtin-parens` | Empty `()` on a tape builtin — parens on a builtin, if present, must carry a successor; omit them or write `name(N)` / `name(!)`. |
| `duplicate-name` | A definition reuses a name already taken by a function or namespace in the same scope. |
| `duplicate-label` | The same label declared twice in one function. |
| `undefined-label` | `goto`, `check`, or a successor names a label the function never declares. |
| `goto-return` | `goto !` — put `(!)` on the preceding command instead. |
| `group-position` | A comma-group position rule violated (`docs/pmt/language.md`, the statement table's last row). |
| `dangling-label` | A label at the end of a function body binds to nothing. |
| `internal-error` | The generated assembly failed to assemble — a compiler bug, not a source error; please report it. |
| `nested-export` | `export` on a nested definition — nested functions are always local. |
| `duplicate-binding` | Two imports bind the same bare name in one scope — qualify the call or disambiguate with `as`. |
| `keyword-needs-name` | `namespace`, `use`, or `export` with no name after it. |
| `keyword-in-body` | `use` or `namespace` inside a function body — imports and namespaces live at file or namespace level. |
| `single-colon-in-path` | A single `:` in a name path where the `::` separator was meant. |
| `top-level-statement` | A command or call at top level — statements live inside function bodies. |
| `dangling-doc-run` | A doc/attention run (`docs/pmt/language.md` (doc lines)) not immediately followed by a function declaration at its scope. |
| `doc-line-order` | A `?` doc line appears after the run has already entered its `!` block — interleaved, or the whole run written `!`-then-`?`. |
| `unknown-attribute` | An attention line's leading `[ident]` names something other than the recognized attribute vocabulary (`deprecated`). |
| `duplicate-attribute` | A second `[deprecated]` attribute inside one run. |
| `volatile-not-on-main` | `volatile` on a definition other than the un-namespaced top-level `main`. |

## `pmt asm`

```
USAGE: pmt asm INPUT.pma [-o OUT.pmo] [-g]
```

Assembles hand-written or disassembled `.pma` text into a `.pmo` object;
`-g` records the label/line debug section (`docs/formats.md`). Hand-written
text can author build columns: `.volatile` tags a `.func` block as the
gated column, or sets the object's program bit when it precedes the first
`.func` (`docs/pmt/asm.md (the .volatile directive)`).

### Assembly errors

A fatal assembly error stops the assemble and renders the same shape as a
compile error: `FILE:LINE:COL: error: MESSAGE [CODE]`. The bracketed code
is a stable kebab-case identifier — permanent, safe to match in scripts
and editor integrations, same contract as `pmt compile` (compile errors)
above. The catalog is the assembler framework's shared namespace,
tabulated once in `docs/core.md (error codes)`; of the assembler's
capability extensions the PM-1 dialect enables only `volatile` — which
adds a directive, not a code — so still only the rows marked reachable
in every dialect can fire from `pmt asm`, and a capability-gated
directive such as `.rept` is not recognized and surfaces through the
base codes instead.

## `pmt link`

```
USAGE: pmt link INPUT.pmo... [-o OUT.pmx] [FLAGS]

FLAGS:
  --no-relax    keep every symbol site in far form
  --nostdlib    do not link the built-in std
  -L DIR        add a library search directory (repeatable, in order)
  -l NAME       link NAME.pmo from the search path (repeatable)
  -v            render the link report (dropped functions, relaxation)

Writes OUT.pmx and the OUT.pmx.map sidecar (function ranges; label/line
info when the objects carry -g debug data).
```

Linking always adds the built-in standard library as an implicit last
library unless `--nostdlib` is given (`docs/pmt/stdlib.md`); explicit `-l NAME`
resolves `NAME.pmo` against the `-L` directories, in the order given, and
errors if it isn't found on any of them — there is no on-disk library
directory to fall back to; the standard library is embedded in the
toolchain binary itself. `-v` renders which defined-but-unreachable
functions were dropped and how many call/jump sites relaxed to their short
form versus stayed far.

**Build columns under `-v`.** Every link also picks a build column per
name — the one matching the program's volatile bit, read off the object
that defines the entry symbol (`docs/core.md (linking)`,
`docs/pmt/language.md (volatile programs)`). Where a reached name ships
only the other column, the linker takes what it has and `-v` says so on
a second line, naming every such name. A hand-assembled volatile program
is the clearest case: `.volatile` before the first `.func` sets the
program bit while tagging no blob, so every reached name is counted.

```asm
; hand.pma
.volatile
.func main
        wr      1
        stp
```

```
$ pmt asm hand.pma -o hand.pmo
$ pmt link -v --nostdlib hand.pmo -o hand.pmx
link: dropped []; 0 site(s) relaxed short, 0 far
link: 1 name(s) with no volatile column linked normal [main]
```

The sentence is direction-aware, because column selection is symmetric —
a plain program reaching a volatile-only body falls back exactly the same
way, and says the opposite. Here `app.pmc` is
`use util; main() { @util(); }` and `util.pma` defines `util` in the
volatile column only:

```asm
; util.pma
.func util
.volatile
        wr      1
        ret
```

```
$ pmt compile app.pmc -o app.pmo && pmt asm util.pma -o util.pmo
$ pmt link -v --nostdlib app.pmo util.pmo -o app.pmx
link: dropped []; 1 site(s) relaxed short, 0 far
link: 1 name(s) with no normal column linked volatile [util]
```

A link whose reached names all offer the selected column prints no such
line at all. The same wording is used by `pmt link -v` and by both of
`pmt build`'s modes, so the three never drift apart.

## `pmt build`

```
USAGE: pmt build [INPUT.pmc|.pma|.pmo ...] [-o OUT.pmx] [FLAGS]   (argv mode)
       pmt build [TARGET ...] [FLAGS]                             (manifest mode)

Argv mode compiles/assembles/loads every input in memory, links with
the stdlib, and writes OUT.pmx (+ .pmx.map). Manifest mode discovers
the nearest pmt.json with a `project` section from the current
directory and builds its targets (all of them when none is named).

COMPILE FLAGS (argv mode; manifest mode: override the profile):
  --debug | --release   presets (manifest mode: profile selection)
  -O0 | -O1             optimization level
  -g                    record debug info
  --strip-debugger      drop `brk` at codegen
  --fno-<pass>          disable one optimizer pass (repeatable)
  -Werror               treat (post-refinement) warnings as errors

LINK FLAGS (argv mode only; the manifest declares these):
  --nostdlib            do not link the built-in std
  -L DIR / -l NAME      library search dir / library (repeatable)
  -o OUT.pmx            output path

COMMON:
  --no-relax            keep every symbol site in far form
  --keep-objects        write each intermediate .pmo next to its source
  --run [TARGET]        manifest mode: build, then run the target's run block
  --list-targets        manifest mode: print `NAME[\trun]` per target
  -v                    render the build report
```

`pmt build` is the compile+link driver, dispatching between two modes by
looking at the shape of its own positional arguments — the manifest is
consulted only when it needs to be. Any positional ending `.pmc`, `.pma`,
or `.pmo` selects **argv mode**: every input is compiled, assembled, or
loaded from disk as needed, held in memory, linked against the standard
library (or an explicit `-L`/`-l` set), and written to `OUT.pmx`; no
`pmt.json` is read at all in this mode. Otherwise every positional is
read as a **target name**, selecting **manifest mode**: `pmt build`
discovers the nearest `pmt.json` carrying a `project` section by walking
up from the current directory, and builds the named targets (or every
declared target when none is named). Mixing the two positional shapes on
one command line — a source path alongside a target name — is an error;
a build is either fully argv-driven or fully manifest-driven.

**Flag table**, split by which mode reads which flag:

- **Compile-side** (`--debug`/`--release`, `-O0`/`-O1`, `-g`,
  `--strip-debugger`, `--fno-<pass>`, `-Werror`) apply in argv mode
  directly; in manifest mode they **override** the corresponding key of
  the selected profile for this invocation only — the manifest itself is
  never rewritten. `-S` and `--emit-ir` are deliberately absent from
  `pmt build`: per-file inspection of generated `.pma` or CFG JSON stays
  `pmt compile`'s job, not the multi-file driver's.
- **Link-side, argv mode only** (`--nostdlib`, `-L`, `-l`, `-o`): the
  manifest already declares the equivalent information itself (linked
  libraries, standard-library opt-out, per-target output path), so
  manifest mode **rejects** `-o`, `-L`, `-l`, and `--nostdlib` outright
  rather than silently ignoring them.
- **Common to both modes** (`--no-relax`, `--keep-objects`, `-v`).
- **Manifest mode only** (`--run`, `--list-targets`): argv mode has no
  notion of a target or a declared run block for either flag to act on.

**Profile selection (manifest mode):** there is no per-target profile
name in the manifest schema — selection happens once per invocation, the
same base for every target that invocation builds. `--release` selects
the `release` base; omitting both `--release` and `--debug` selects
`debug`. The individual compile-side flags above then layer on top of
that base's keys for this invocation only — an individual flag always
wins over whatever the resolved profile declares
(`docs/pmt/project.md (schema reference)` has the two bases' key
tables).

**`--run [TARGET]`:** builds first, then runs the target's declared run
block (the same tape/limits shape `pmt run` reads), reached only after a
successful build. Exit codes mirror `pmt run`: `0` the program stopped
(`stp`), `2` the program halted abnormally (`hlt`), `3` the program
trapped; a build failure short-circuits before any of these apply.

**`--list-targets`:** manifest mode only; prints one line per declared
target — `NAME`, a tab, then `run` when that target carries a run block
(omitted otherwise) — machine-readable, one target per line.

**`--keep-objects`:** in both modes, writes each intermediate `.pmo`
object next to its source file instead of discarding it once linked in
memory.

**Build columns: disk carries both, memory carries one.** A `.pmc`
compilation builds every function twice, and which columns end up in an
object depends on whether that object outlives the invocation
(`docs/pmt/language.md (volatile programs)`). A `.pmo` written to disk —
by `pmt compile`, or by `pmt build --keep-objects` — always carries both
columns, because it cannot know which program will link it later. An
object `pmt build` holds only in memory dies inside a link whose program
kind is already decided, so the driver compiles only the column that
link will select and the other one is provably dead work.

The program kind is decided the way the linker decides it: by the ONE
input that defines the entry symbol, taking the inputs in the linker's
own order — the units first, then the `-l` libraries, first definition
wins — not by a union over the inputs. A non-entry input carrying the
program bit does not flip the columns of a program the linker then
resolves as normal.

The rule is an optimization with no observable effect: the linker
selects one column either way, so **the `.pmx` from the in-memory path
is byte-identical to the one built through on-disk objects**, and
`--keep-objects` changes what is written beside the sources, never what
is linked.

```
$ pmt build -O1 pulse-v.pmc -o mem.pmx
$ pmt compile -O1 pulse-v.pmc -o disk.pmo && pmt link disk.pmo -o disk.pmx
$ cmp mem.pmx disk.pmx && echo identical
identical
```

**Undeclared-external refinement:** the ordinary "undeclared external"
compile warning fires per file, on a bare call whose name that file
never imports. `pmt build` sees the whole declared set for the build —
every input in argv mode, every target's declared sources in manifest
mode — so it drops that warning wherever the name turns out to be
defined somewhere else in the same build. `pmt compile`, working one
file at a time, has no such visibility and stays per-file honest,
warning on every bare undeclared call regardless of what a sibling file
happens to define.

See `docs/pmt/project.md` for the manifest's `project` section itself —
the schema, target and profile shapes, and the discovery rule.

## `pmt lint`

```
USAGE: pmt lint PATH... [--exclude PATH]... [--allow CODE]... [--fix [--force]] [--no-config]

PATH is a .pmc or .pma file, or a directory; directories are walked
recursively for *.pmc and *.pma (sorted order, symlinks not followed,
dot-entries skipped). Omitting PATH uses the nearest manifest's declared
source set (docs/pmt/project.md (the declared source set)); requires a
`pmt.json` project and is incompatible with --no-config. .pmc sources
lint through the pmc rule table; .pma sources lint through core's
arch-agnostic asm rule table over the PM-1 syntax. --allow CODE draws
from the union of both tables.

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs); exclusion
                  wins even over explicitly listed files
  --allow CODE    suppress a lint rule by code (repeatable;
                  unknown codes are an error)
  --fix           apply machine-applicable fixes in place, then re-lint;
                  the report and exit code reflect what REMAINS
  --force         with --fix: also apply the gated fixes (deletions and
                  rewrites whose diagnosis may have another reading)
  --no-config     ignore pmt.json project files
```

PATH is a `.pmc` or `.pma` file, or a directory. Directories are walked
recursively for `*.pmc` and `*.pma` in sorted order; symlinks are never
followed and dot-entries (`.git`, editor scratch) are skipped. A PATH
that yields no `.pmc`/`.pma` files is an error. `--exclude PATH`
(repeatable) skips a file or prunes a directory subtree; paths are
compared as spelled (no globs — the shell covers the include side), and
exclusion wins even over explicitly listed files.

Each file's extension picks its rule table: `.pmc` lints through the
pmc-specific rules (`docs/pmt/lint.md`), `.pma` through core's
arch-agnostic assembly rule table read against the PM-1 syntax
(`docs/pmt/lint.md`'s `.pma` rule list). `--allow CODE` draws from the
union of both tables, so one allow-list works across a batch mixing
both languages. An explicitly listed file with neither extension is a
per-file error (`PATH: error: unknown source extension (expected .pmc
or .pma)`) and the batch continues — the directory walk itself never
collects any other extension, so this only fires for a file named
directly on the command line.

Files lint independently: a file that fails to parse is reported on
stderr — as a fatal error line with its bracketed code (`pmt compile`
(compile errors) for `.pmc`, `pmt asm` (assembly errors) for `.pma`) —
and the batch continues. Exit codes: 0 = every file clean, 1 = findings
or errors anywhere (tool errors are also 1).

For each input file, `pmt lint` also discovers a `pmt.json` project
file by walking up from that file's directory (nearest ancestor wins,
never a cascade — `docs/pmt/lint.md`) and unions its allow-list with any
`--allow` flags. `--no-config` skips that discovery for every file, so
the run is governed by `--allow` alone — but it is rejected outright on a
*bare* `pmt lint` (no PATH arguments), where the manifest's declared
source set is the input itself and skipping discovery would leave nothing
to lint. A `pmt.json` that fails to
parse or validate is a per-file fatal, exactly like a source file that
fails to parse: reported on stderr as `PATH/pmt.json: error: MESSAGE`,
the file it would have configured is skipped, and the batch continues.
This differs from an unknown code named directly by `--allow`, which is
a whole-tool error (that flag applies to the entire run, not to one
input file, so there is no single file to skip).

`--fix` applies safe fixes in place and lints the result again — the
report and exit code reflect what remains. `--fix --force` also
applies the gated fixes (deletions and rewrites whose diagnosis may
have another reading). `--force` without `--fix` is an error. A file
with a fatal error is never written. The rule catalog and per-rule fix
behavior live in `docs/pmt/lint.md`.

## `pmt fmt`

```
USAGE: pmt fmt PATH... [--exclude PATH]... [--check]
       pmt fmt - [--check] [--lang pmc|pma]

PATH is a .pmc or .pma file, or a directory; directories are walked
recursively for *.pmc and *.pma (sorted order, symlinks not followed,
dot-entries skipped). Omitting PATH uses the nearest manifest's declared
source set (docs/pmt/project.md (the declared source set)); requires a
`pmt.json` project. `-` reads one source from stdin and writes the
result to stdout; it cannot be combined with PATH arguments.

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs)
  --check         do not write; with PATH..., list files that would be
                  reformatted and exit 1 if any would change; with -,
                  exit 1 if stdin would change (CI mode)
  --lang LANG     stdin's language: pmc (default) or pma; applies to
                  stdin (-) only — an error alongside PATH arguments,
                  whose language always comes from the file extension
```

PATH is a `.pmc` or `.pma` file, or a directory, walked the same way as
`pmt lint`'s batch: directories recurse for `*.pmc` and `*.pma` in
sorted order, symlinks are never followed, dot-entries are skipped, and
`--exclude PATH` (repeatable, no globs) skips a file or prunes a
subtree. Each file's extension picks its formatter: `.pmc` through the
pmc pretty-printer (`docs/pmt/fmt.md`), `.pma` through core's canonical-grid
printer (`docs/formats.md (assembly text)`). An explicitly listed file
with neither extension is a per-file error (`PATH: error: unknown
source extension (expected .pmc or .pma)`), same shape and same
batch-continues behavior as `pmt lint`'s unknown-extension route. Files
format independently: a file that fails to lex or parse is reported on
stderr — as a fatal error line with its bracketed code (`pmt compile`
(compile errors) for `.pmc`, `pmt asm` (assembly errors) for `.pma`) —
and the batch continues.

By default `pmt fmt` rewrites each file in place, and only when its
formatted text differs from what's already on disk — an
already-canonical file is never rewritten, so a clean tree sees no
spurious modification times. `--check` writes nothing; instead it lists
the path of every file whose formatted text would differ and exits 1 if
any did, 0 otherwise — the CI-friendly mode. `-` reads one source from
stdin and writes the formatted text to stdout instead of running a
directory walk; it cannot be combined with `PATH` arguments. `--lang`
picks stdin's language — `pmc` (the default) or `pma` — and is
meaningless with `PATH` arguments, where the extension already decides;
combining `--lang` with a `PATH` is an error. `- --check` mirrors the
same semantics against stdin: nothing is written either way, and the
exit code alone reports whether stdin would change.

Exit codes: 0 = success (every input already canonical, or rewritten in
place); 1 = under `--check`, at least one input would change, or a
lex/parse error occurred anywhere in the batch. The `.pmc` canonical
style itself — indentation, label/command alignment, comma-group
layout, blank lines, comment handling, and the token-spacing table — is
`docs/pmt/fmt.md`; the `.pma` canonical grid is `docs/formats.md (assembly
text)`.

## `pmt dis`

```
USAGE: pmt dis FILE.pmo|FILE.pmx [--listing] [--map FILE.pmx.map]

Objects disassemble with real names from the symbol table. Executables
use the .pmx.map sidecar when present (FILE.pmx.map or --map), else
recursive-descent discovery (func_XXXX). --listing prints the debugger
code view: addresses + raw bytes, not reassembleable.
```

**Sidecar discovery:** an explicit `--map` always wins; failing that,
`pmt` looks for `FILE.pmx.map` beside the executable. A missing or
unparsable sidecar (implicit discovery only) is silently ignored — a stale
sidecar must never break plain `dis`/`run`. An unparsable *explicit*
`--map`, by contrast, is an error (`docs/formats.md`). `dis` accepts either
a `.pmo` or a `.pmx` on the same command line via magic sniffing;
`--listing` applies to executables only.

**`--listing` vs canonical `dis`:** the default `dis` output is the
canonical `.pma` grid (`docs/formats.md (assembly text)`) — valid,
reassembleable assembler input. `--listing` instead prints the debugger
code view: one line per instruction, address and raw hex bytes plus the
mnemonic, every byte in the image accounted for (including bytes no
control-flow path reaches), branch/call targets resolved to
`function`/`function.label` names when a map is available. This view is
not reassembleable — it exists to inspect what a `.pmx` actually contains,
byte for byte, not to round-trip it.

**Mnemonic width follows from who picks it.** A symbol site the linker
narrowed prints in the canonical view under its **far** mnemonic —
`call`, never `call.s` — because far is the only form the assembler
accepts, and re-linking that text re-derives the same narrowing
(`docs/core.md (relaxation)`). An intra-function jump keeps its own
spelling, short form included, because the assembler owns that width.
`--listing` shows the encoding either way.

The renderer wraps a row that will not fit — bytes after five per line,
the operand within its own column — but no PM-1 instruction is wide
enough to reach either limit: the widest encoding is five bytes, and a
resolved `function.label` name is never broken however long it runs. Every
PM-1 listing row is therefore a single line. The wrapping matters on
architectures with vector operands; `docs/tmt/cli.md (wide instructions)`
describes what it looks like there.

## `pmt tape-block`

```
USAGE: pmt tape-block build " * * *" [--head N] [-o OUT.pmt]
       pmt tape-block new [--from APP.pmx] [-o OUT.pmt] [EDITS]
       pmt tape-block set IN.pmt (-o OUT.pmt | --in-place) [EDITS]
       pmt tape-block show FILE.pmt [--dense | --separated]

EDITS (repeatable; KEY is a tape index):
  --alphabet KEY=GLYPHS   repin the block's glyphs (relabels; same cardinality)
  --cells    KEY=GLYPHS   set tape KEY's cells
  --head     KEY=N        set tape KEY's head
  --origin   KEY=N        set tape KEY's origin

build: cell characters are the PM-1 glyphs (space = blank, * = mark); the
leftmost character is cell 0. GLYPHS is alphabet notation: ' ','*'.
```

Four subcommands author and inspect `.pmt` tape-block snapshots without
hand-editing bytes. The unit is the **block**; PM-1 is a one-tape-device
architecture, so a PM block holds a single band and a single alphabet.

Because PM-1 is single-tape, `pmt tape-block set` has no shape edits —
there is only ever one band to add, remove, or reorder; reshaping
multi-band blocks lives in `tmt tape-block set` (`docs/tmt/cli.md (tmt
tape-block)`).

### Edit flags

`new` and `set` take four **keyed, repeatable** edit flags, so one invocation
sets up the whole block:

```
--alphabet KEY=GLYPHS   repin the block's glyphs
--cells    KEY=GLYPHS   set tape KEY's cells
--head     KEY=N        set tape KEY's head
--origin   KEY=N        set tape KEY's origin
```

`KEY` is always a tape index here. Tape names are a source-language
construct, and `pmt` has no source-provenance path — PM-1's alphabet is fixed
at two glyphs, so a `.pmc` has nothing to contribute that the arch does not
already supply. (`tmt` accepts names, from a `.tmc`.) Repeating a flag for
the same tape is an error rather than last-wins.

`GLYPHS` is alphabet notation — quoted symbols, comma-separated, with
inclusive `lo..hi` ranges. `--alphabet` applies before `--cells`, so cells
resolve against the glyphs just pinned. An alphabet is a set (unique, at most
127 glyphs); cells are a sequence, so repeats are ordinary.

### `tape-block build`

Unchanged fixed-alphabet sugar: cell characters are the PM-1 glyphs (space =
blank, `*` = mark), leftmost character is cell 0. It only ever creates
blocks.

### `tape-block new`

`new` mints a block and applies this invocation's edits. `--from APP.pmx`
takes the band count from the image header; without it, the `--alphabet`
keys size the block, and with no flags at all it is a single empty band on
PM-1's default glyphs.

```
$ pmt tape-block new --alphabet "0=' ','*'" --cells "0='*','*',' '" -o t.pmt
$ pmt tape-block show t.pmt
tape 0: origin 0, head 0 reads '*', alphabet [" ", "*"]
|** |
```

### `tape-block set`

Clone semantics: read the input, apply the edits, write the result out.
Exactly one output destination is required — `-o OUT.pmt` or `--in-place`,
mutually exclusive — and supplying neither is an error, which keeps `set`
from silently clobbering its input.

`--alphabet` **relabels, it never re-maps**: cell indices are untouched and
only the glyph table changes. Because a PM block is single-tape and
single-alphabet, a repin writes the **block** table and leaves the per-tape
override unset, so the file stays MT version 1:

```
$ pmt tape-block build " ** " -o t.pmt
$ pmt tape-block set t.pmt --in-place --alphabet "0='0','1'"
$ pmt tape-block show t.pmt
tape 0: origin 0, head 0 reads '0', alphabet ["0", "1"]
|0110|
```

A repin must keep the tape's effective cardinality exactly — cells are
validated against that alphabet on read, so a narrowing repin would strand an
out-of-range cell:

```
$ pmt tape-block set t.pmt --in-place --alphabet "0='a','b','c'"
pmt: --alphabet `0`: tape 0 has cardinality 2, the given alphabet has 3 glyphs
```

### `tape-block show`

`show` renders each band through **its own** effective alphabet — its glyph
table if present, otherwise the block's fallback — and prints that alphabet
per band. `.pmt` and `.tmt` are one container, so a block authored by `tmt`
with per-band tables reads correctly here too.

Cells are delimited adaptively: dense when every glyph is a single character,
separated when any is longer, so `|011|` can never be ambiguous between three
cells and two. PM-1's fixed pair is single-character, so PM tapes stay dense.
`--dense` and `--separated` force either form; passing both is an error.

The head line **names the glyph under the head** — `head 4 reads '*'` — rather
than marking it with a caret line beneath the span. A caret must be padded
from column zero out to the head, so a head resting far from the origin costs
a line as long as the span itself: on a megacell tape that one line doubles
the output while carrying a single character of information. The cell's index
is already on the same line, and its offset within the span is `head - origin`.
A head outside the stored span reads blank — the span is a window on an
unbounded tape, not the whole of it.

## `pmt run`

```
USAGE: pmt run APP.pmx [FLAGS]

TAPE (default: empty, head 0):
  --tape-block IN.pmt        load the initial tape from a snapshot
  --tape-cells " * *" [--head N]  build the initial tape inline
  --save-tape-block OUT.pmt  write the final tape as a snapshot

LIMITS AND SEMANTICS:
  --max-steps N       step budget (default 10000000)
  --no-step-limit     remove the step budget
  --max-tacts N       tact budget
  --strict-cells      trap on double-mark/double-unmark
  --tact-profile M,R,W  device costs (move,read,write; default 1,1,1)

OUTPUT:
  --trace             stream per-instruction listing lines to stderr,
                      live, each with post-state `; MF=<0|1> head=<n>`
  -v                  no extra effect yet (stats always print)

EXIT CODE: 0 stopped | 2 halted (hlt) | 3 trapped | 1 tool error.
```

`--tape-block` and `--tape-cells` are mutually exclusive; with neither, the
initial tape is empty with the head at 0. `--max-steps` defaults to
10,000,000 (`--no-step-limit` removes the budget entirely — use with a
program you trust to terminate); `--max-tacts` has no default (unset =
unlimited). `--tact-profile` sets device costs as `move,read,write`
(electronic default `1,1,1`; a slower "mechanical" profile can model a
physical tape's motion cost — `docs/pmt/isa.md (timing model)`).

**`--trace` format:** streams live, one line per retired instruction, in
the same address/bytes/mnemonic shape as `dis --listing`, with a
post-execution state suffix: `; MF=<0|1> head=<n>` — reflecting the state
*after* that instruction's effect (so the head/MF shown are what the
instruction just produced, in the Delphi step-view tradition;
`docs/history.md`). `-v` is accepted for symmetry with the other
subcommands but currently has no additional effect: `run`'s outcome and
stats print unconditionally regardless of `-v`.

### Exit codes

| Code | Outcome | What it means |
|---|---|---|
| `0` | `Stopped` | The program reached `stp` — a normal, successful end. |
| `2` | `Halted` | The program reached `hlt` — an abnormal end the program chose. |
| `3` | `Trapped` | The machine faulted, or a budget ran out. |
| `1` | tool error | Bad arguments, unreadable file, malformed container — never a program outcome. |

## `pmt ir`

```
USAGE: pmt ir graph FILE.ir.json|FILE.pmc [--function NAME]
                    [--variant normal|volatile] [-O0|-O1]

Renders --emit-ir output as a Mermaid flowchart (one per function). A
.pmc input is compiled in memory first: --variant picks which build
column's CFG is rendered (default normal) and -O0/-O1 the optimization
level (default -O0, as in `pmt compile`). Both flags need a .pmc input —
a .ir.json file already holds exactly one column.
```

Renders each function's control-flow graph as a Mermaid `flowchart TD`:
block contents (labels, ops, terminal instruction) become node text, and
`check` terminators become a pair of `MF`/`!MF` edges. `--function NAME`
restricts the output to one function.

The input is either a `--emit-ir` JSON file (`docs/formats.md (IR JSON)`)
or a `.pmc` source, and the two accept different flags. A JSON file
already holds exactly one compiled column at one optimization level, so
`--variant` and `-O0`/`-O1` are errors on it. A `.pmc` input is compiled
in memory first, and those two flags say what to compile:

- **`--variant normal|volatile`** picks the build column
  (`docs/pmt/language.md (volatile programs)`); the default is `normal`.
  This is an inspection tool, so it shows either column of any program:
  asking for `volatile` on a program with no `volatile main` renders a
  CFG that program would never link, and asking for `normal` on a
  volatile program renders the one it never links. Which column actually
  ships is the linker's choice, from the program bit.
- **`-O0`/`-O1`** sets the optimization level, defaulting to `-O0` as in
  `pmt compile`. Writing both is not an error and `-O1` wins regardless
  of the order they appear in — also as in `pmt compile`.

```
$ pmt ir graph two.pmc -O1
%% main
flowchart TD
    B0["wr 1<br/>ret"]

$ pmt ir graph two.pmc -O1 --variant volatile
%% main
flowchart TD
    B0["wr 1<br/>wr 1<br/>ret"]
```

(`two.pmc` is `main() { mark; mark; }`: the gated column keeps the
idempotent second write that `cell-state` drops from the other one.)

`pmt compile --emit-ir` has no such flag — it writes the **normal**
column, for a volatile program too, exactly as `-S` renders the normal
column's assembly. Both describe one compilation rather than one link;
`--variant volatile` here, or `pmt dis` on the linked `.pmx`, is how the
gated column is read.

## `pmt lsp`

```
USAGE: pmt lsp

Run the LSP server for .pmc and .pma on stdio until the client exits.
Exit code: 0 after shutdown/exit, 1 on exit without shutdown.
```

Runs one Language Server Protocol server for both `.pmc` and `.pma` on
stdio: `pmt lsp` is the only subcommand that hands real stdio to
library code — every protocol frame goes over stdin/stdout, exactly as
the LSP spec's base protocol requires. It serves publish diagnostics (compile fatals,
compile warnings, and lint findings, merged and sorted), completions,
go-to-definition (including into a materialized copy of the standard
library), quickfix code actions from lint's fixes, semantic tokens, a
document-symbol outline, and whole-document formatting identical to
`pmt fmt`. The process exit code follows the LSP lifecycle: `0` after
the client sends `shutdown` then `exit`; `1` if `exit` arrives without
a prior `shutdown`, or if the client disconnects without sending
either. See `docs/lsp.md` for the capabilities table, editor wiring
samples, and the configuration and materialized-standard-library
details.

## `pmt dap`

```
USAGE: pmt dap

Run the DAP debug-adapter server for a .pmx program on stdio until the client disconnects.
Exit code: 0 after a clean disconnect, 1 on transport EOF before one.
```

Runs the Debug Adapter Protocol server on stdio, mirroring `pmt lsp`'s
role: the other subcommand that hands real stdio to library code, every
protocol frame going over stdin/stdout. Two `launch` shapes are
recognized, named by which of `"program"`/`"target"` the request's
arguments carry (giving exactly one is required):

- **Program mode** names a prebuilt `.pmx` executable (`"program"`) and
  an optional `.pmt` tape snapshot (`"tape"` — the empty tape is PM's
  default); `"strictCells": true` mirrors `pmt run --strict-cells`.
- **Target mode** names a `pmt.json` manifest target (`"target"`, an
  optional `"project"` path override) and builds it in process through
  the same path `pmt build TARGET` runs, always with debug info forced
  on. The tape comes from the target's own `run` settings.

Both modes cover the full v1 session lifecycle (`initialize`/
`disconnect`), `stopOnEntry`, `configurationDone`, run control
(`continue`/`pause`), source and instruction breakpoints, stepping at
line or instruction granularity, the stack/scopes/variables surface over
PM-1's registers and its tape, `setVariable`, `disassemble`, and the
opt-in `"trace"` output stream. Termination renders a summary `output`
event (the same steps/tacts numbers `pmt run` prints) followed by
`terminated` and `exited`, with the same 0/2/3 exit-code mapping as
`pmt run`'s stopped/halted/trapped outcomes. See `docs/dap.md` for the
full launch-config schema, the closed output-events list, the
writable-state contract, and the degradation rules.

## `pmt completions`

```
USAGE: pmt completions <SHELL>

Emits a shell completion script to stdout for the given SHELL (zsh; bash
and fish are recognized but not yet implemented).

  pmt completions zsh > ~/.zfunc/_pmt
```

The subcommand's own flag/positional surface, and every other
subcommand's flags and file-extension-filtered positionals, are driven
from one in-crate registry rather than hand-written per shell — this is
what keeps the generated script from drifting out of sync with the
flags the parser actually accepts as subcommands and flags change over
time. `zsh` completes subcommand names (including the nested `tape
build`/`tape show` and `ir graph`), each subcommand's flags (long and
short forms, `-O0`/`-O1` as an either/or pair, `--emit-ir`'s known
stages), and file arguments filtered to the extension the subcommand
actually reads. `bash` and `fish` are recognized shell names so the
error names them explicitly rather than rejecting them as unknown, but
neither renders yet.
