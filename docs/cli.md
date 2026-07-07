# The `pmt` command-line tool

`pmt` is a thin renderer over the toolchain's library API: library code
never prints, and every stage returns a structured report (compiler
warnings and an optimizer report; the linker's dropped-functions and
relaxation report); `pmt -v` on the relevant subcommand renders that report
as text. This is why the CLI mirrors the library shape so closely, and why
a future embedder can consume `compile`/`assemble`/`link`/`disassemble`/
`Machine` directly without going through a subprocess at all.

```
pmt — Post-machine toolchain

USAGE: pmt <SUBCOMMAND> [ARGS]

SUBCOMMANDS:
  compile      .pmc source -> .pmo object (-S for .pma, --emit-ir for CFG JSON)
  asm          .pma assembly -> .pmo object
  link         .pmo objects -> .pmx executable (+ .pmx.map sidecar)
  lint         lint .pmc sources (hygiene findings; docs/lint.md)
  fmt          format .pmc sources in place (--check to preview; -)
  dis          disassemble a .pmo or .pmx (--listing for the address view)
  run          execute a .pmx on a tape
  tape         build/show .pmt tape-block snapshots
  ir           render --emit-ir JSON (ir graph -> Mermaid)
  completions  emit a shell completion script (zsh; bash/fish follow-on)

Run `pmt <SUBCOMMAND> --help` for details. `pmt --version` prints the version.
```

`pmt --version` prints two lines: `pmt <VERSION>` (the toolchain crate's own
version) and `pmc language <VERSION>` (the `.pmc` language acceptance-contract
version — `docs/language.md`). The two numbers move on independent axes: a
crate release with no grammar change repeats the same language-version line,
while the language version only bumps when the grammar itself changes.

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
functions — `docs/language.md (visibility)`) always print to stderr as
`FILE:LINE:COL: warning: MESSAGE` (the column is new; `-Werror` semantics are
unchanged by it); `-v` additionally prints the optimizer's per-pass round
report; `-Werror` turns every warning into a compile failure. `--emit-ir`
writes `<output base>.ir.json` — see
`docs/language.md (the IR artifact)` and `docs/formats.md (IR JSON)`.
"Repeated stages resolve last-wins" refers to snapshot labels, not the
flag: a stage label captured in several optimizer rounds (e.g.
`after:inline`) resolves to the last captured snapshot, while the
`--emit-ir` flag itself may appear only once per command line —
repeating it is an unknown-flag error.

## `pmt asm`

```
USAGE: pmt asm INPUT.pma [-o OUT.pmo] [-g]
```

Assembles hand-written or disassembled `.pma` text into a `.pmo` object;
`-g` records the label/line debug section (`docs/formats.md`).

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
library unless `--nostdlib` is given (`docs/stdlib.md`); explicit `-l NAME`
resolves `NAME.pmo` against the `-L` directories, in the order given, and
errors if it isn't found on any of them — there is no on-disk library
directory to fall back to; the standard library is embedded in the
toolchain binary itself. `-v` renders which defined-but-unreachable
functions were dropped and how many call/jump sites relaxed to their short
form versus stayed far.

## `pmt lint`

```
USAGE: pmt lint PATH... [--exclude PATH]... [--allow CODE]... [--fix [--force]]

PATH is a .pmc file or a directory; directories are walked recursively
for *.pmc (sorted order, symlinks not followed, dot-entries skipped).

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
```

PATH is a `.pmc` file or a directory. Directories are walked
recursively for `*.pmc` in sorted order; symlinks are never followed
and dot-entries (`.git`, editor scratch) are skipped. A PATH that
yields no `.pmc` files is an error. `--exclude PATH` (repeatable)
skips a file or prunes a directory subtree; paths are compared as
spelled (no globs — the shell covers the include side), and exclusion
wins even over explicitly listed files.

Files lint independently: a file that fails to parse is reported on
stderr and the batch continues. Exit codes: 0 = every file clean,
1 = findings or errors anywhere (tool errors are also 1).

`--fix` applies safe fixes in place and lints the result again — the
report and exit code reflect what remains. `--fix --force` also
applies the gated fixes (deletions and rewrites whose diagnosis may
have another reading). `--force` without `--fix` is an error. A file
with a fatal error is never written. The rule catalog and per-rule fix
behavior live in `docs/lint.md`.

## `pmt fmt`

```
USAGE: pmt fmt PATH... [--exclude PATH]... [--check]
       pmt fmt - [--check]

PATH is a .pmc file or a directory; directories are walked recursively
for *.pmc (sorted order, symlinks not followed, dot-entries skipped).
`-` reads one .pmc from stdin and writes the result to stdout; it
cannot be combined with PATH arguments.

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs)
  --check         do not write; with PATH..., list files that would be
                  reformatted and exit 1 if any would change; with -,
                  exit 1 if stdin would change (CI mode)
```

PATH is a `.pmc` file or a directory, walked the same way as `pmt
lint`'s batch: directories recurse for `*.pmc` in sorted order,
symlinks are never followed, dot-entries are skipped, and `--exclude
PATH` (repeatable, no globs) skips a file or prunes a subtree. Files
format independently: a file that fails to lex or parse is reported on
stderr and the batch continues.

By default `pmt fmt` rewrites each file in place, and only when its
formatted text differs from what's already on disk — an
already-canonical file is never rewritten, so a clean tree sees no
spurious modification times. `--check` writes nothing; instead it lists
the path of every file whose formatted text would differ and exits 1 if
any did, 0 otherwise — the CI-friendly mode. `-` reads one `.pmc` from
stdin and writes the formatted text to stdout instead of running a
directory walk; it cannot be combined with `PATH` arguments. `- --check`
mirrors the same semantics against stdin: nothing is written either way,
and the exit code alone reports whether stdin would change.

Exit codes: 0 = success (every input already canonical, or rewritten in
place); 1 = under `--check`, at least one input would change, or a
lex/parse error occurred anywhere in the batch. The canonical style
itself — indentation, label/command alignment, comma-group layout,
blank lines, comment handling, and the token-spacing table — is
`docs/fmt.md`.

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

## `pmt tape`

```
USAGE: pmt tape build " * * *" [--head N] [-o OUT.pmt]
       pmt tape show FILE.pmt

build: cell characters are the PM-1 glyphs (space = blank, * = mark);
the leftmost character is cell 0. show: renders any .pmt with its own
alphabet.
```

`build` writes with PM-1's default glyphs (`docs/formats.md`); `show`
renders any `.pmt` using its own embedded alphabet, so it works for
tapes built with a different glyph set.

## `pmt run`

```
USAGE: pmt run APP.pmx [FLAGS]

TAPE (default: empty, head 0):
  --tape-block IN.pmt        load the initial tape from a snapshot
  --tape " * *" [--head N]   build the initial tape inline
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

`--tape-block` and `--tape` are mutually exclusive; with neither, the
initial tape is empty with the head at 0. `--max-steps` defaults to
10,000,000 (`--no-step-limit` removes the budget entirely — use with a
program you trust to terminate); `--max-tacts` has no default (unset =
unlimited). `--tact-profile` sets device costs as `move,read,write`
(electronic default `1,1,1`; a slower "mechanical" profile can model a
physical tape's motion cost — `docs/isa.md (timing model)`).

**`--trace` format:** streams live, one line per retired instruction, in
the same address/bytes/mnemonic shape as `dis --listing`, with a
post-execution state suffix: `; MF=<0|1> head=<n>` — reflecting the state
*after* that instruction's effect (so the head/MF shown are what the
instruction just produced, in the Delphi step-view tradition;
`docs/history.md`). `-v` is accepted for symmetry with the other
subcommands but currently has no additional effect: `run`'s outcome and
stats print unconditionally regardless of `-v`.

**Exit codes:** `0` the program stopped normally (`stp`); `2` the program
halted abnormally (`hlt`); `3` the program trapped; `1` a tool-level error
(bad arguments, unreadable file, malformed container — never a program
outcome).

## `pmt ir`

```
USAGE: pmt ir graph FILE.ir.json [--function NAME]

Renders --emit-ir output as a Mermaid flowchart (one per function).
```

Reads a `--emit-ir` JSON file (`docs/formats.md (IR JSON)`) and renders
each function's control-flow graph as a Mermaid `flowchart TD`; block
contents (labels, ops, terminal instruction) become node text, `check`
terminators become a pair of `MF`/`!MF` edges. `--function NAME` restricts
output to one function.

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
