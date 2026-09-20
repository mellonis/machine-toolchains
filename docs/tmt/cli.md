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
  build        compile+link driver: .tmc/.tma/.tmo inputs or manifest targets
  dis          disassemble a .tmo or .tmx (--listing for the address view)
  interface    print a unit's exported declarations as a header
  run          execute a .tmx on a multi-tape .tmt block
  tape-block   new/set/show .tmt tape-block snapshots
  ir           render --emit-ir JSON (ir graph -> Mermaid, ir footprints -> write sets)
  lint         hygiene findings over .tmc and .tma sources
  fmt          canonical formatting for .tmc and .tma sources
  lsp          run the LSP server for .tmc and .tma on stdio
  dap          run the DAP debug-adapter server on stdio
  completions  emit a shell completion script (zsh, bash, fish)
  man          emit the tmt(1) manual page (roff) to stdout

Run `tmt <SUBCOMMAND> --help` for details. `tmt --version` prints the version.
```

`tmt --version` prints three lines: `tmt <VERSION>` (the toolchain crate's
own version), `tmc language <VERSION>` (the `.tmc` language
acceptance-contract version — `docs/tmt/language.md`), and `tma dialect
(tm-1) <VERSION>` (the TM-1 `.tma` dialect version — `docs/tmt/asm.md`). The
three numbers move on independent axes: a crate release with no grammar
change repeats the same language-version and dialect-version lines, and each
grammar version only bumps when its own grammar changes.

Every usage block on this page is quoted verbatim from the binary and
checked against it by a test — this page is a reference, not a paraphrase.
For most subcommands that block is the real `--help` output; `tape` and
`ir` are the exception: every action inside one of those groups answers
`--help` too, but renders that group's own shared usage text — the same
block a bare invocation of the group prints — so the blocks quoted here
come from the bare invocation and cover every action in the group (each
of those sections repeats this).

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
  --stamped-asm      emit raw stamped .tma (skip .rept re-detection)
  --emit-ir[=STAGE]  write the world-graph IR JSON next to the output
                     (STAGE: lowered | final | after:<pass> for a registered
                      pass; default final)
  --fno-<pass>       disable one optimizer pass (repeatable)
  --foutline         enable the default-off `outline` optimizer pass
  --extern FILE      read FILE's declarations (.tmh strict, .tmc lenient;
                     repeatable, in command-line order)
  --nostdlib         do not read the embedded standard library's declarations
  -Werror            treat warnings as errors
  -v                 render the compile report (passes, rounds)
```

Consumes one `.tmc` source; produces a `.tmo` object (or, with `-S`, the
generated `.tma` assembly text). Without `-o` the output takes the input's
name with the extension replaced.

Codegen stamps range-expanded families out one block (or match-table row)
per value; before writing the `-S` text the compiler folds each such family
back into the `.rept` loop a human would have written (`docs/formats.md`).
The rewrite is verified by assembling both the stamped and the folded text
and comparing the object bytes, so it can only change how the assembly reads,
never what it assembles; on any mismatch it keeps the stamped text.
`--stamped-asm` skips the fold and emits the raw stamped assembly. `-g`
implies it — the debug line map is keyed to the stamped physical lines and
cannot survive the rewrite — so a `-g` build always carries the stamped text.

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
optimizer, and `--emit-ir=after:<pass>` reads that list to validate its
stage argument, so an unrecognized stage name is always rejected up front.
`--fno-<pass>` reads the same list too, but only to render the flag family
in the generated completion script (`tmt completions`) — the compiler
itself does not check the suffix, so `--fno-<pass>` accepts any suffix and
silently disables nothing when it names no real pass. Ask the binary for
the current set of `after:` stages by naming an unknown one:

```
$ tmt compile prog.tmc --emit-ir=after:bogus
tmt: unknown IR stage `after:bogus` (lowered | final | after:inline | after:outline | after:jump-threading | after:tail-call | after:tail-merge | after:dce | after:dead-rows | after:dispatch-select)
```

### `--fno-<pass>` and `--foutline`

`--fno-<pass>` disables one optimizer pass and is repeatable — `--fno-inline
--fno-dce` disables both. It is a flag *family*: one full flag per pass name,
not a `name=value` pair. Nothing checks the suffix against the pass
registry at compile time, though — `--fno-bogus` is accepted and silently
ignored rather than rejected; only the generated completion script renders
the family from the real pass list.

`outline` is the one pass that defaults **off**, so it has flags with both
senses and both are real. `--foutline` turns it on; `--fno-outline` — which
the family renders because `outline` is a registered pass like any other —
keeps it off. `--foutline` takes effect only at `-O1`, because that is the
only level at which the optimizer runs.

The pass names both flag families accept, what each pass does, and the
contracts the whole pipeline holds to are
`docs/tmt/optimizer.md (passes)` — including the case for and against
turning `outline` on, which is a judgement about a program's shape
rather than a rule that holds generally.

### `--emit-ir`

`--emit-ir` writes the world-graph IR as JSON next to the output, at
`<output base>.ir.json`. The IR is a documented, versioned artifact rather
than an internal detail (`docs/formats.md (IR JSON)`).

The stage argument is **equals-only**: `--emit-ir` and `--emit-ir=STAGE` are
both accepted, but `--emit-ir STAGE` as two tokens is not — the stage would
be left behind and rejected as a stray positional. `STAGE` is one of the
pipeline bookends `lowered` / `final` (the default), or `after:<pass>` for
any registered pass. An unknown stage is rejected up front, with an error
naming every stage that does resolve. A *registered* stage is not
automatically safe, though: a snapshot is only captured for a pass that
actually changed something, so a stage naming a pass that fired nothing on
this particular input still fails — just later, as a missing snapshot.
`tmt compile <program> -O1 --emit-ir=after:inline` on a program with
nothing to inline is exactly that case: it exits 1 with
``no IR snapshot labeled `after:inline` was captured`` and writes no
`.ir.json` sidecar (the object output itself is written normally), while
the same flag naming a stage whose pass did fire succeeds. A stage label
captured in several optimizer rounds resolves to the last snapshot
captured under it. The flag itself may appear only once per command line;
repeating it is an unknown-flag error.

`docs/tmt/optimizer.md (passes)` works every pass through a before/after
graph example built with this flag, and `tmt ir graph` renders the
documents it writes.

### `--extern` and `--nostdlib`

`--extern FILE` reads one other unit's declarations for the footprint/
contract check (`docs/tmt/language.md (contract clauses)`): a callee found
in one of these — or in the embedded standard library, believed by default
— contributes its DECLARED effective write set; a callee found nowhere
contributes the whole alphabet. `FILE`'s extension decides how it is read,
never a second front end: a `.tmh` is read STRICTLY, the same shape `tmt
interface` enforces on a header (a routine body or a `machine` block is an
error); anything else — a `.tmc` — is read LENIENTLY as a full program,
where any body and any `machine` block are simply unused. The flag is
repeatable, and order is meaningful: modules are consulted in command-line
order, with the embedded standard library consulted last — so a `--extern
std.tmh` of the user's own shadows the built-in `std` when both are
present. This mirrors the linker's own first-wins rule for a name declared
in more than one object.

`--nostdlib` drops the embedded standard library from that lookup —
`std::…` names then behave like any other external, contributing the
whole alphabet unless an explicit `--extern` supplies their declarations
too (a user may disable the built-in library and supply their own under
the same `std` name).

Naming a name is a separate step from believing its contract: `--extern`
supplies what the footprint check believes about a callee's write set. It
does not, by itself, resolve a call target or an alphabet reached through
`use` — an unresolved `use`-imported name fails exactly as it does without
`--extern`.

A `--extern` file that fails to read or parse is a compile error naming
that file's own path, never the primary input's.

### Compile errors

A fatal compile error stops the compile and renders as
`FILE:LINE:COL: error: MESSAGE [CODE]`. The bracketed suffix is one of
this page's **error codes** — a stable kebab-case identifier for the
error kind — safe to match in scripts and editor integrations. The same
rendering carries the same codes wherever a fatal surfaces: `tmt
compile` itself, and the per-file fatal lines of `tmt lint` and
`tmt fmt`. Codes are permanent identifiers: they never change meaning.
Feature-context detail stays with its feature — the fold family's
verbatim messages live in `docs/tmt/language.md (substitution)`, the
symbol-map family (graft, call/bind, and a named map's own declaration
checks) in `docs/tmt/language.md (symbol maps)` and its "Named maps"
subsection.

| Code | Meaning |
|---|---|
| `lex-error` | The source failed to tokenize: an unexpected character, an unterminated block comment, or a malformed glyph literal. |
| `unexpected-token` | The parser needed one construct and saw another. |
| `reserved-name` | A reserved keyword used where a name is expected (a state, alphabet, or path-segment name). |
| `multiple-machines` | More than one `machine { … }` block in one file — a program has exactly one, a library has none. |
| `tape-not-in-machine` | A `tape` declaration outside a `machine` block — routines and graphs take their tapes from the signature. |
| `machine-in-declarations` | A `machine { … }` block read in a declarations-only reading — a header carries no program entry point. |
| `routine-body-in-declarations` | A routine carries a body in a declarations-only reading — a header states its signature only. |
| `naked-pattern` | A rule pattern written without its enclosing `[ … ]` — bare single-tape patterns are not supported. |
| `wildcard-binding` | `* as v` — a wildcard cannot bind; write the range explicitly so the expansion cost is visible. |
| `range-kind-mismatch` | A range whose endpoints are not the same kind (`'a'..3`) — `glyph..glyph` or `number..number` only. |
| `char-arithmetic` | Arithmetic on a glyph-bound substitution (`{c+1}`) — only numeric bindings fold. |
| `graft-needs-name` | A non-`entry` `graft` with no `as name` — an unreferenced unnamed instance would be dead. |
| `state-redirect` | The `state name;` redirect form — a state always has a `{ … }` body. |
| `dangling-doc-run` | A doc/attention run not immediately followed by a declaration that accepts documentation. |
| `doc-line-order` | A `?` doc line appears after the run has already entered its `!` block. |
| `unknown-attribute` | An attention line's leading `[ident]` names something other than the recognized attribute vocabulary (`deprecated`). |
| `duplicate-attribute` | A second `[deprecated]` attribute inside one run. |
| `contract-clause-order` | A `writes { … }` clause on a signature tape parameter written after that parameter's `preserves` clause — the fixed order is `writes` then `preserves`. |
| `duplicate-contract-clause` | A second `writes` or `preserves` clause on one signature tape parameter. |
| `empty-alphabet` | An alphabet with no elements — a world needs at least one symbol. |
| `duplicate-glyph` | The same glyph appears twice in one alphabet. |
| `alphabet-too-large` | An alphabet resolves to more than 127 symbols. |
| `range-endpoint-not-scalar` | A glyph range endpoint that is not a single Unicode scalar. |
| `range-descending` | A range whose low endpoint exceeds its high endpoint — ranges are inclusive and ascending. |
| `duplicate-name` | Two entities (alphabet, routine, graph, or namespace) share one name in one scope. |
| `duplicate-binding` | Two imports bind one bare name in one scope — qualify the target or disambiguate with `as`. |
| `too-many-tapes` | A world declares more than 16 tapes. |
| `unresolved-alphabet` | A tape (or signature tape parameter) names an alphabet no scope resolves — either nothing declares it anywhere, or it is reached through `use` or a qualified path whose declarations were not given (pass `--extern` or declare it locally). |
| `duplicate-tape` | Two tapes share one name in one world. |
| `duplicate-state` | Two states (or a state and a graft instance) share one name in one world. |
| `duplicate-param` | Two signature parameters share one name. |
| `entry-count` | A world's `entry` count is not exactly one. |
| `return-outside-routine` | A `return` transition, continuation, or `state` argument outside a routine body. |
| `goto-into-bind` | `goto` targeting a bind name — a bind is a call target, never a state. |
| `goto-not-a-state` | `goto` targeting a routine or graph — a reuse target, not a state. |
| `undefined-state` | `goto`, a continuation, or a state argument names no state (or graft instance) in the world. |
| `wrong-target-kind` | A `call`/`graft`/`bind` target resolves to the wrong entity kind. |
| `undefined-graph` | A `graft` target names no graph in scope. |
| `unknown-arg` | A binding argument names a parameter the signature does not declare. |
| `duplicate-arg` | Two binding arguments share one parameter name. |
| `missing-arg` | A signature parameter has no binding argument. |
| `wrong-arg-kind` | A binding argument is the wrong kind for its parameter. |
| `unresolved-tape-target` | A tape-parameter argument names a target that is not a tape in the enclosing world. |
| `duplicate-tape-target` | Two tape-parameter arguments of one `call`, `graft`, or `bind` name the same caller tape — one caller tape cannot back two callee tapes. |
| `bind-call-args` | A `call` on a world-local bind name carries binding arguments — a bind is already fully bound at its declaration. |
| `contract-symbol-unknown` | A `writes`/`preserves` clause names a glyph that is not a symbol of the parameter's alphabet. |
| `writes-outside-contract` | A world's inferred write footprint on one tape leaves the effective set its contract declares (`writes` minus `preserves`). |
| `graft-cycle` | A graph definition graft-depends on itself, directly or through a cycle of definitions. |
| `graft-call-unsupported` | A grafted graph's body contains a `call` — splicing a calling graph into the host is not supported. |
| `map-symbol-not-in-alphabet` | A symbol map (a graft binding's, or a named map declaration's own pairs) references a glyph that is not in the alphabet it maps. |
| `map-blank-pin` | A symbol map maps the blank off itself — blank must read as blank, and a write-back must not un-pin it. |
| `map-conflict` | A symbol map maps one symbol to two different images in one direction. |
| `map-not-injective` | A symbol map on equal-size alphabets is not injective — identity completion collides. |
| `identity-glyph-mismatch` | An omitted symbol map on tapes whose alphabets are not glyph-for-glyph equal — an omitted map means identity. |
| `map-not-closed` | A named map declaration's two alphabets differ in size and it leaves a non-blank source symbol unmapped — unlike a graft's inline map (which silently holes an unnamed source), a declaration reused at many sites must name every one explicitly. |
| `named-map-source-mismatch` | A `with map NAME` site's caller tape alphabet is not the named map's own declared source alphabet. |
| `named-map-target-mismatch` | A `with map NAME` site's callee parameter alphabet is not the named map's own declared target alphabet. |
| `undefined-map` | A `with map NAME` site names no map in scope — either nothing declares it anywhere, or it is reached through `use` or a qualified path whose declarations were not given (pass `--extern` or declare it locally). An unresolvable `use` import with no site naming it is a separate, non-fatal finding (`unused-import`), not this code. |
| `fold-out-of-alphabet` | A write substitution folds to a value with no glyph in the tape's alphabet. |
| `zero-modulus` | A `%` in a write-cell fold has a zero modulus. |
| `negative-remainder` | A `%` fold produces a negative remainder — reachable only when subtraction takes the left operand negative. |
| `fold-overflow` | A write-cell fold overflows `i64` during evaluation. |
| `exact-row-conflict` | Two rules in one state match the same concrete tuple with neither carrying a wildcard. |
| `row-width` | A rule's pattern, write, or move vector width differs from the world's tape count. |
| `too-many-state-params` | A signature declares more than 255 `state` parameters — the published exit count is one byte wide. |
| `state-args-need-declarations` | A `call` supplies `state` arguments to a routine whose declarations were not given — an exits vector is positional, so the callee's own parameter order is needed (pass `--extern`, or declare it locally). |
| `internal-error` | The compiler broke its own invariant — generated assembly failed to assemble, or a compiler-built IR world failed validation. A compiler bug, not a source error; please report it. |

## `tmt asm`

```
USAGE: tmt asm INPUT.tma [-o OUT.tmo] [-g]
```

Assembles hand-written or disassembled `.tma` text into a `.tmo` object;
`-g` records the label/line debug section. The TM-1 `.tma` dialect enables
every capability the assembler has that carries a diagnostic of its own —
sections, match and dispatch tables, `.rept` macros, vector operands,
`.routine` signatures, and frame descriptors
(`docs/formats.md (assembly text)`, `docs/tmt/isa.md`). It opts out of just
one, `volatile`: build columns are a PM-1 notion, since TM-1 volatility is
a property of a tape parameter rather than of a routine
(`docs/tmt/language.md (volatile tapes)`), and that capability adds a
directive rather than an error code. A fatal assembly error renders in the
same `FILE:LINE:COL: error: MESSAGE [CODE]` shape as a compile error, with
the assembler's own stable codes — the shared catalog in
`docs/core.md (error codes)`; every row of that catalog can fire from
`tmt asm`.

## `tmt link`

```
USAGE: tmt link INPUT.tmo... [-o OUT.tmx] [FLAGS]

FLAGS:
  --no-relax        keep every call site in far form
  --entry NAME      link NAME as the program entry (default: main)
  --call-mech MECH  bound-call lowering: mono | frames | hybrid (default: hybrid)
  --nostdlib        do not auto-link the embedded standard library
  --allow CODE      suppress a link warning code (repeatable)
  -Werror           treat link warnings as errors
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
rows — so a frameless link keeps the single-line report. One further line
follows per hybrid exit-bearing fold decision — the callee, its site count,
body and descriptor bytes, and whether the group was shared under frames or
spliced as mono seeds — so a link with no such site prints none.

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

### Link warnings

A link warning names a site the linker can see is suspect but will not
refuse — a callee whose alphabet is narrower than the caller's band, or
one whose glyphs differ at the same width. It prints always, in the same
format a compile warning does, and carries a bracketed code:

```
main+0x0001: warning: `sub` reads a 3-symbol alphabet where `main`'s tape 0 is 5 wide [narrow-alphabet]
```

The codes share the one allow namespace `tmt lint` uses, so `--allow CODE`
suppresses one here and `lint.allow` in `tmt.json` suppresses it for
`tmt build`. `-Werror` promotes every unsuppressed warning to an error.
Errors are outside the namespace and cannot be suppressed — a callee
wider than the caller or declaring exits the site does not supply, a
binding naming a parameter or a glyph the callee does not declare, an
open binding into a tape the callee does not declare opaque, a graft or
an imported alphabet whose digest drifted, and the copy path's own
refusals (`docs/core.md (call mechanisms)`).

In manifest mode `-Werror`'s promotion is per TARGET, not per build: a
strict refusal stops the build where it stands, and the targets already
linked keep the artifacts they wrote — the same way a plain link error
on a later target behaves.

| Code | Meaning |
|---|---|
| `glyph-mismatch` | A call site binds by index into a callee whose alphabet is the same size but spells different glyphs, so the callee reads the caller's symbols as other symbols. |
| `narrow-alphabet` | A call site binds by index into a callee whose alphabet is narrower, so the caller's high symbols have no image in it. |

## `tmt build`

```
USAGE: tmt build [INPUT.tmc|.tma|.tmo ...] [-o OUT.tmx] [FLAGS]   (argv mode)
       tmt build [TARGET ...] [FLAGS]                             (manifest mode)

Argv mode compiles/assembles/loads every input in memory, links with
the stdlib, and writes OUT.tmx (+ .tmx.map). Manifest mode discovers
the nearest tmt.json with a `project` section from the current
directory and builds its targets (all of them when none is named).

COMPILE FLAGS (argv mode; manifest mode: override the profile):
  --debug | --release   presets (manifest mode: profile selection)
  -O0 | -O1             optimization level
  -g                    record debug info
  --strip-debugger      drop `brk` at codegen
  --fno-<pass>          disable one optimizer pass (repeatable)
  --foutline            enable the default-off `outline` pass
  -Werror               treat (post-refinement) warnings as errors

LINK FLAGS (argv mode only; the manifest declares these):
  --nostdlib            do not link the built-in std
  -L DIR / -l NAME      library search dir / library (repeatable)
  --entry NAME          link NAME as the program entry (default: main)
  -o OUT.tmx            output path

COMMON:
  --allow CODE          suppress a link warning code (repeatable)
  --no-relax            keep every symbol site in far form
  --call-mech MECH      bound-call lowering: mono | frames | hybrid
  --keep-objects        write each intermediate .tmo next to its source
  --run [TARGET]        manifest mode: build, then run the target's run block
  --list-targets        manifest mode: print `NAME[\trun]` per target
  -v                    render the build report
```

`tmt build` is the compile+link driver, dispatching between two modes by
looking at the shape of its own positional arguments — the manifest is
consulted only when it needs to be. Any positional ending `.tmc`, `.tma`,
or `.tmo` selects **argv mode**: every input is compiled, assembled, or
loaded from disk as needed, held in memory, linked against the standard
library (or an explicit `-L`/`-l` set), and written to `OUT.tmx`; no
`tmt.json` is read at all in this mode. Otherwise every positional is
read as a **target name**, selecting **manifest mode**: `tmt build`
discovers the nearest `tmt.json` carrying a `project` section by walking
up from the current directory, and builds the named targets (or every
declared target when none is named). Mixing the two positional shapes on
one command line — a source path alongside a target name — is an error;
a build is either fully argv-driven or fully manifest-driven.

**Flag table**, split by which mode reads which flag:

- **Compile-side** (`--debug`/`--release`, `-O0`/`-O1`, `-g`,
  `--strip-debugger`, `--fno-<pass>`, `--foutline`, `-Werror`) apply in
  argv mode directly; in manifest mode they **override** the
  corresponding key of the selected profile for this invocation only —
  the manifest itself is never rewritten. Two of them have no such key
  to override: `--fno-<pass>` and `--foutline` are **flag-only axes**.
  A profile carries `opt`, `debug-info`, `strip-debugger`, and `werror`
  and nothing else, so pass selection cannot be committed to the
  manifest at all — those two always come from the command line,
  layered on top of whichever profile is in force. `-S`, `--emit-ir`,
  and `--stamped-asm` are deliberately absent from `tmt build`:
  per-file inspection of generated `.tma` or world-graph IR JSON stays
  `tmt compile`'s job, not the multi-file driver's.
- **Link-side, argv mode only** (`--nostdlib`, `-L`, `-l`, `--entry`,
  `-o`): the manifest already declares the equivalent information itself
  (linked libraries, standard-library opt-out, per-target entry symbol,
  output path), so manifest mode **rejects** `-o`, `-L`, `-l`,
  `--nostdlib`, and `--entry` outright rather than silently ignoring
  them — five flags, one more than the compile-side/link-side split
  alone would suggest, because a target's entry symbol is as much a
  manifest-declared fact as its output path or its libraries. In argv
  mode, `--nostdlib` reaches the compile step too, not only the link
  step: the declarations base the footprint/contract check believes
  (`tmt compile`'s own `--nostdlib`, above) drops the embedded standard
  library as well — argv mode has no `--extern` of its own, so this is
  its one opt-out.
- **Common to both modes** (`--allow`, `--no-relax`, `--call-mech`,
  `--keep-objects`, `-v`). `--call-mech` is the one link-side flag
  manifest mode does *not* reject: it is accepted there as a
  per-invocation override of the target's declared lowering, resolved
  flag first, then the target's own `call-mech` key, then the project's
  default `call-mech` key, then the linker's own default when none of
  those set it. The manifest records the *committed* lowering for a
  target; the flag exists to experiment against that commitment for one
  build without editing `tmt.json`. `--allow` is likewise never
  rejected: argv mode reads it alone, and manifest mode unions it with
  the manifest's own `lint.allow` — `--allow` on the command line can
  only suppress more, never fewer, of what the manifest already
  suppresses. `-Werror` covers both stages in both modes: compile
  warnings (refined against the declared name set first) and, since the
  link stage now diagnoses too, link warnings — see
  "### Link warnings" under `tmt link`.
- **Manifest mode only** (`--run`, `--list-targets`): argv mode has no
  notion of a target or a declared run block for either flag to act on.

**Profile selection (manifest mode):** there is no per-target profile
name in the manifest schema — selection happens once per invocation, the
same base for every target that invocation builds. `--release` selects
the `release` base; omitting both `--release` and `--debug` selects
`debug`. The individual compile-side flags above then layer on top of
that base's keys for this invocation only — an individual flag always
wins over whatever the resolved profile declares (see
`docs/tmt/lint.md (project file)` and `docs/tmt/project.md` for the
profile schema itself).

**`--run [TARGET]`:** builds first, then runs the target's declared run
block, reached only after a successful build; naming more than one
target (or none, when the manifest declares more than one) alongside
`--run` is an error, since exactly one target must be selected to run
afterward. Unlike `pmt run`, `tmt run` has **no empty-tape default** — it
always drives a whole multi-tape band loaded from a `.tmt` snapshot — so
a target's run block must declare a `tape`; a target with no run block
at all, or one whose run block declares no `tape`, cannot be `--run` and
names the target in a pointed error instead of inventing one. Exit codes
mirror `tmt run`: `0` the program stopped (`stp`), `2` the program halted
abnormally (`hlt`), `3` the program trapped; a build failure
short-circuits before any of these apply.

**`--list-targets`:** manifest mode only; prints one line per declared
target — `NAME`, a tab, then `run` when that target carries a run block
(omitted otherwise) — machine-readable, one target per line. This is
also what the generated zsh completion script's dynamic target
completion shells out to at completion time, so the candidate list
always tracks the manifest with zero drift.

**`--keep-objects`:** in both modes, writes each intermediate `.tmo`
object next to its source file instead of discarding it once linked in
memory.

**Undeclared-external refinement:** the ordinary "undeclared external"
compile warning fires per file, on a bare call whose name that file
never imports. `tmt build` sees the whole declared set for the build —
every input in argv mode, every target's declared sources in manifest
mode — so it drops that warning wherever the name turns out to be
defined somewhere else in the same build. `tmt compile`, working one
file at a time, has no such visibility and stays per-file honest,
warning on every bare undeclared call regardless of what a sibling file
happens to define.

See `docs/tmt/project.md` for the manifest's `project` section itself —
the schema, target and profile shapes, and the discovery rule.

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
tape block it says so and points at `tmt tape-block show`.

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

**Mnemonic width follows from who picks it.** TM-1's one short form is
the call, and a call the linker narrowed prints in the canonical view
as plain `call` — far is the only form the assembler accepts, and
re-linking that text re-derives the same narrowing
(`docs/core.md (relaxation)`). `--listing` is where the `call.s`
encoding shows.

**Wide instructions wrap in two columns.** A per-tape `wr`/`mov` vector
grows with the tape count, so a listing row is not always one line. The
byte column wraps after five bytes and the operand column wraps
independently, which keeps hex and mnemonics from interleaving on a
continuation line — the eye can still read either column straight down.
The operand breaks only when it must, and then at the widest seam
available: between whole `[..]` vectors first, and inside a vector on
element boundaries only when one vector alone still will not fit. The
lane is wide enough that a sixteen-tape vector never breaks internally.
Continuation lines carry neither the address nor the mnemonic, so the
address column remains an exact index of where each instruction starts.

## `tmt interface`

```
USAGE: tmt interface INPUT [-o OUT.tmh]

INPUT is told apart by its container magic, never by its extension: a
.tmc source or a compiled .tmo object. A .tmh extension (case-insensitive)
additionally selects declarations-only reading of a text INPUT: a
`machine` block or a routine body is rejected, and a bodiless routine
signature is required instead. Prints the unit's exported
declarations — alphabets and routine signatures with their EFFECTIVE
write contracts either way; neither arm ever prints `volatile` (it
leaves no trace past source and is never checked at a call site). From
source the header is complete: it also carries exported graph bodies in
full and every `?` doc line. From an object it carries signatures and
alphabets only — no graph body, no map, no doc line, since none of those
exist on the wire. Without -o the header goes to stdout.
```

Renders a unit's exported declarations as one canonical, deterministic
text — the same shape a header file carries (docs/tmt/language.md
(headers)). Like `dis`, `INPUT` is told apart by its container magic
rather than its extension (`docs/formats.md`): a `.tmc` renamed to
`.tmo`, or the reverse, still runs the arm its bytes actually are.

**Two arms, one printer.** From a `.tmc` source the header is complete:
every exported alphabet, every exported routine's signature with its `?`
doc lines, and every exported graph's body in full. From a compiled
`.tmo` object the header carries routine signatures and exported
alphabets only — an object's interface section has no graph body, no
symbol map, and no doc line to read back (docs/formats.md (routine
interfaces)), so those never appear on that arm. Both arms render the
IDENTICAL signature line for the same routine: a contract clause always
prints the tape's PUBLISHED write set (`writes { … }`) — the declared
EFFECTIVE set (`writes` minus `preserves`) when the tape declares either
clause, or the compiler's own INFERRED write set for that tape when
neither clause was written — rather than the author's own spelling, and
never the whole alphabet as a stand-in for "no restriction declared" (the
wire has no way to spell that). `preserves` itself never appears on
either arm: it has no representation on the wire and an object-arm render
could not reproduce it. `volatile` is dropped from both arms
for the same reason: the modifier is compile-time-only and leaves no
trace in the generated assembly (docs/tmt/language.md (volatile tapes)),
and it is never checked at a call site either, so it is not part of what
a caller may rely on. **Both arms print, ahead of each namespace's own
declarations, the `use` lines that namespace's printed content needs** —
though the two decide "needs" from different data. The source arm keeps
a `use` when its bound name is referenced AND EITHER the header prints
the import's target itself OR the target lives in ANOTHER unit, reached
through the compile's declarations table rather than through this unit's
own declarations (a program that compiled at all could only have
resolved such a name that way). The object arm reaches the equivalent
case through its own tape-alphabet matching order, described next.

**A routine over a non-exported alphabet is legal, and both arms render
it.** On the source arm, every alphabet an exported routine or graph
references prints — as `export alphabet` when it is itself exported, as
a plain `alphabet` (no `export`) when it is only referenced. On the
object arm, a tape's glyph list is matched by content, trying four
sources in order: (1) an exported alphabet reachable UNQUALIFIED from the
routine's own namespace (its own, or any enclosing one) — printed by
short name, no `use` needed; (2) an exported alphabet in any OTHER
namespace of the same object — a `use <qualified name>;` line in the
routine's own namespace, printed by short name; (3) an alphabet the
object IMPORTED from another unit — likewise a `use` line and the short
name, with no local `alphabet` declaration for it; (4) otherwise a
synthesized, deterministic `alphabet` declaration — `<routine>__<param>`,
the routine's own mangled name with `::` replaced by `_`, joined to the
parameter name — declared at the top level, before the namespace block
that uses it. Within (1)/(2)/(3) the first content match wins, in wire
order; a short-name collision inside one namespace — two matched
alphabets that would both want the same short name there — falls back to
(4) for whichever one loses the race, rather than printing an ambiguous
`use`. The object arm never fails to render a routine for want of an
alphabet name. This is also why the two-arm identity is a property of
routines over EXPORTED (or importable) alphabets — every tape in the
standard library draws from one, so (1) or (2) always matches there: a
routine over a genuinely PRIVATE, non-exported, non-imported alphabet
still renders on both arms, but the object arm's synthesized name is not
expected to match the source arm's own local spelling.

**The object arm skips the entry world.** A `machine` block always
compiles to the symbol name `main`, but it is never a callee — nothing
binds against it or reads its own interface entry — so it publishes no
write set and prints no declaration on the object arm; the source arm
never renders it either, since a `machine` block has no `export` keyword.

**A `.tmh` extension selects declarations-only reading of a text INPUT**
(docs/tmt/language.md (headers)): the identical `.tmc` grammar, read in a
mode that rejects a `machine` block and a routine WITH a body, and
requires a graph to carry one — a graph's only form is its source, so a
header cannot omit it. This is a mode on the one reader, not a second
grammar: `tmt interface` run back over a header it just wrote reproduces
that header's text unchanged.

Without `-o` the header goes to stdout; with it, to the named file.

## `tmt run`

```
USAGE: tmt run APP.tmx --tape-block TAPES.tmt [FLAGS]

TAPE:
  --tape-block TAPES.tmt     load the initial tape band from an MT snapshot
                             (one band per image tape; alphabets sized per band)
  --save-tape-block OUT.tmt  write the final band as an MT snapshot

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

`--tape-block` is **required** — unlike `pmt run`, which defaults to an empty tape,
a TM-1 image runs a whole band of tapes and there is no inline glyph-pattern
form to build one from. Mint a template with `tmt tape-block new --from` and fill
it in with `tmt tape-block set`. The block's band count must equal the image's tape
count; a mismatch is a tool error naming both numbers:

```
$ tmt run two-tape.tmx --tape-block one-tape.tmt
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
base-profile image's line is byte-identical without it. A trace line never
wraps, however many tapes the image drives: the two-column wrapping
`dis --listing` applies to a wide instruction would break the one
line per retired instruction that makes the stream greppable.

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
$ tmt run prog.tmx --tape-block t.tmt --max-steps 1
outcome: Trapped(StepLimit)
```

Because budget exhaustion is a trap, exit `3` does not by itself mean the
program is wrong — it may only mean it needed a longer leash. Read the
outcome line before concluding. `docs/tmt/isa.md` covers trap kinds at the
machine level.

## `tmt tape-block`

```
USAGE: tmt tape-block new [--from APP.tmx | --from APP.tmc] [-o OUT.tmt] [EDITS]
       tmt tape-block set IN.tmt (-o OUT.tmt | --in-place)
                    [--from APP.tmc] [SHAPE] [EDITS]
       tmt tape-block show FILE.tmt [--dense | --separated]

SHAPE (set only; applied remove -> add -> reorder, before EDITS; flag
order never matters — remove keys name the INPUT block, add positions
count after removals, --reorder and EDITS address the result):
  --add-tape [KEY=]ALPHABET   insert a band at position KEY, or append
  --remove-tape KEY           drop a band
  --reorder K1,K2,...         permute bands (every band exactly once)

EDITS (repeatable; KEY is a tape index, or a tape name with --from a .tmc):
  --alphabet KEY=GLYPHS   repin tape KEY's glyphs (relabels; same cardinality)
  --cells    KEY=GLYPHS   set tape KEY's cells
  --head     KEY=N        set tape KEY's head
  --origin   KEY=N        set tape KEY's origin

GLYPHS is alphabet notation: ' ','s','1' or '0'..'9'. --alphabet applies
before --cells, so cells resolve against the glyphs just pinned.
```

Three subcommands author and inspect `.tmt` tape-block snapshots without
hand-editing bytes. The unit is the **block** — the whole multi-tape band —
so one invocation authors all of it. There is no `tape-block build`: PM-1's
is glyph-pattern sugar tied to a fixed two-symbol alphabet, while TM-1 tapes
carry per-tape alphabets. Every action answers `--help` with the same usage
shown above — bare `tmt tape-block` prints the identical text.

### Edit flags

The four edit flags are **keyed and repeatable**, so a whole block is set up
in a single call:

```
--alphabet KEY=GLYPHS   repin tape KEY's glyphs
--cells    KEY=GLYPHS   set tape KEY's cells
--head     KEY=N        set tape KEY's head
--origin   KEY=N        set tape KEY's origin
```

`KEY` is a tape index. It may also be a declared **tape name** when the
invocation passes `--from` a `.tmc` source, which is the only thing that can
supply names; a name without one is an error saying so. Names are resolved to
indices at parse time and never stored — the container addresses bands by
number, exactly as the bus does.

Repeating the same flag for the same tape is an error rather than last-wins:
silently dropping an edit is worse than making the author look at it.

`GLYPHS` is the same notation a program's `alphabet { … }` body uses, so it
copy-pastes out of the source — quoted symbols, comma-separated, with
inclusive `lo..hi` ranges expanded. `--alphabet` applies **before** `--cells`
for a given tape, so cells resolve against the glyphs just pinned. One
difference between the two: an alphabet is a set (glyphs unique, at most 127)
while cells are a sequence, so `--cells "0='1','1','1'"` is an ordinary run.

### Shape flags

`set` (only — `new` has no input block to reshape) accepts three shape flags
that change the block's **band set and their order**, ahead of the content
edits above:

```
--add-tape [KEY=]ALPHABET   insert a band at position KEY, or append
--remove-tape KEY           drop a band
--reorder K1,K2,...         permute bands (every band exactly once)
```

Shape edits run in a **fixed phase order — remove, then add, then
reorder** — regardless of where the flags sit on the command line; flag
position never matters. Each phase addresses a different shape: a
`--remove-tape` key names a band in the **input** block, exactly as it
was before this invocation touched it. An `--add-tape` position (when
given; a bare `ALPHABET` with no `KEY=` appends) counts against the block
**after** removals, so a removed band never makes a later position
ambiguous. `--reorder` is a complete permutation — every surviving band
exactly once — of the block **after** removals and adds. The content
edit flags (`--alphabet`, `--cells`, `--head`, `--origin`) always address
the final shape, after all three phases have run.

A worked example: a three-band block `[A B C]`, each band with its own
alphabet and cells so identity survives the reshape:

```
$ tmt tape-block new --alphabet "0=' ','a'" --alphabet "1=' ','b','c'" --alphabet "2=' ','d'" --cells "0='a'" --cells "1='b','c'" --cells "2='d','d'" -o base.tmt
$ tmt tape-block set base.tmt --remove-tape 1 --add-tape "0=' ','x'" --reorder 2,0,1 --cells "0='d'" -o out.tmt
$ tmt tape-block show out.tmt
tape 0: origin 0, head 0 reads 'd', alphabet [" ", "d"]
|d|
tape 1: origin 0, head 0 reads ' ', alphabet [" ", "x"]
||
tape 2: origin 0, head 0 reads 'a', alphabet [" ", "a"]
|a|
```

Reading the phases: remove 1 drops `B`, leaving `[A C]`; add at 0 inserts
a fresh band `N`, giving `[N A C]`; reorder `2,0,1` picks index 2 then
index 0 then index 1, landing `[C N A]`; `--cells 0='d'` then addresses
that final index 0, which is `C` — overwriting its old `"dd"` with `"d"`.
`N` stays empty and unlabeled except for the alphabet `--add-tape` gave it;
`A` is untouched.

When the invocation also passes `--from` a `.tmc` source, its declared
tape names ride along through every shape phase rather than staying
pinned to their original index: a name whose band `--remove-tape` drops
is retired from this invocation, and a later flag naming it gets a
dedicated error explaining it was removed by `--remove-tape` in this same
invocation, instead of the generic "no such tape"; a band `--add-tape`
inserts is always unnamed, since the flag supplies a position and an
alphabet, never a name; and `--reorder` carries each surviving name along
with its band to wherever the permutation puts it, so a `--cells
NAME=...` edit later in the same call still resolves against the right
band. None of this persists in the `.tmt` container — names are resolved
to indices at parse time only, exactly like the un-shaped edit flags.

Reordering has a foot-gun: which band sits at index 0 (and 1, and so on)
is exactly what a compiled program's tape 0 (tape 1, …) binds to at load
time, so a reorder that moves a different band into a slot changes what
the program reads and writes — silently, from the program's point of
view. `tmt run` catches the failure modes it can see: it refuses a block
whose band count, or any individual band's cardinality, disagrees with
the executable's header. It cannot catch a reorder among bands that all
happen to share a cardinality — a block shaped that way loads cleanly and
is, by construction, indistinguishable from intent.

### `tape-block new`

`new` mints a block and applies this invocation's edits to it. It has two
provenance paths plus a freehand one; `--from` dispatches on the container
magic, never on the file extension.

**`--from APP.tmx`** takes the band count and each band's cardinality from
the image header. An image carries symbol indices and no glyphs, so the bands
are labelled with the decimal strings `0..card-1`, which `--alphabet` then
repins.

**`--from APP.tmc`** additionally reads the real **glyphs and tape names**
out of the `machine` block's tape declarations, so the common case needs no
`--alphabet` at all:

```
$ tmt tape-block new --from pow2.tmc --cells "main='s','b','1','1','1','k'" -o in.tmt
$ tmt tape-block show in.tmt
tape 0: origin 0, head 0 reads 's', alphabet [" ", "s", "b", "k", "1"]
|sb111k|
tape 1: origin 0, head 0 reads ' ', alphabet [" ", "1"]
||
tape 2: origin 0, head 0 reads ' ', alphabet [" ", "1"]
||
```

A source with no `machine` block is a library — it takes its tapes from each
routine's signature and has no single band to describe — so `new` refuses it
by name.

**Without `--from`**, the `--alphabet` flags define the block: one per tape,
keyed contiguously from `0`. TM-1 has no fixed alphabet, so every band needs
one. The contiguity rule is what makes a mistyped key an error rather than a
silently oversized block.

### `tape-block set`

`set` has clone semantics: it reads the input, applies the edits, and writes
the result out. Exactly one output destination is required — `-o OUT.tmt` or
`--in-place` — and the two are mutually exclusive; supplying neither is an
error, which is what keeps `set` from silently clobbering its input. Any
subset of the shape and edit flags may be given, and with none of either
`set` is a plain copy.

`--from APP.tmc` on `set` supplies tape **names only** — the flag itself
never reshapes the block; `--add-tape`/`--remove-tape`/`--reorder` do that.
Once supplied, though, the names are not just labels for content edits: the
shape phases consume them too, checking the declared count against the
block up front and carrying each name along as its band moves through
remove, add, and reorder (the names paragraph above).

`--alphabet` **relabels, it never re-maps**. Cell indices are untouched; only
the glyph table they are read through is replaced. That is what makes an
already-authored block repinnable in place:

```
$ tmt tape-block show t.tmt
tape 0: origin 0, head 0 reads '1', alphabet ["0", "1", "2", "3", "4"]
|124|
$ tmt tape-block set t.tmt --in-place --alphabet "0=' ','s','b','k','1'"
$ tmt tape-block show t.tmt
tape 0: origin 0, head 0 reads 's', alphabet [" ", "s", "b", "k", "1"]
|sb1|
```

A repin must keep the tape's **effective cardinality** exactly. Cells are
validated against that alphabet on read, so a narrowing repin would strand a
cell holding a now-out-of-range index and make the block unloadable:

```
$ tmt tape-block set t.tmt --in-place --alphabet "0=' ','x'"
tmt: --alphabet `0`: tape 0 has cardinality 5, the given alphabet has 2 glyphs
```

A `--cells` glyph outside the band's effective alphabet is an error naming
what it was checked against:

```
$ tmt tape-block set t.tmt --in-place --cells "0='Z'"
tmt: --cells `0`: glyph `Z` is not in [" ", "s", "b", "k", "1"]
```

### `tape-block show`

`show` renders each band through **its own** effective alphabet — its glyph
table if it has one, otherwise the block's fallback — and prints that
alphabet per band. A single header line could not: on a block minted from a
multi-alphabet program the bands genuinely differ, and the block-level table
is only a default for bands carrying none.

Cells are delimited **adaptively**: dense when every glyph in the band's
alphabet is a single character, since nothing can be misread, and separated
when any glyph is longer, since `|011|` would otherwise be ambiguous between
three cells and two. `--dense` and `--separated` force either form for stable
output; passing both is an error.

The head line **names the glyph under the head** — `head 4 reads '1'` — rather
than marking it with a caret line beneath the span. A caret must be padded
from column zero out to the head, so a head resting far from the origin costs
a line as long as the span itself: on a megacell tape that one line doubles
the output while carrying a single character of information. The cell's index
is already on the same line, and its offset within the span is `head - origin`.
A head outside the stored span reads blank — the span is a window on an
unbounded tape, not the whole of it.

## `tmt ir`

```
USAGE: tmt ir graph FILE.ir.json [--function NAME]
       tmt ir footprints FILE.ir.json [--function NAME]

`graph` renders --emit-ir output as a Mermaid flowchart (one per world).
`footprints` renders each world's inferred write footprint: per tape, the
symbol indices its body may ever write, out of the tape's cardinality. Both
share the `--function` flag (pmt's flag name, for cross-tool muscle memory);
a TM world IS the unit here (the `machine` block or a routine), so NAME is a
world name.
```

Reads a `--emit-ir` JSON file and renders each world's state graph as a
Mermaid `flowchart TD`. `--function NAME` restricts output to one world;
naming a world the file does not contain is an error. As with `tape`,
`graph --help` renders the same usage block shown above, same as bare
`tmt ir`.

### `tmt ir footprints`

Reads the same `--emit-ir` JSON and renders each world's **inferred write
footprint**: for every signature tape, the set of symbol indices the
world's body — and everything it calls, grafts, or binds within the file
the IR was compiled from — may ever write to it. One block per world:

```
$ tmt ir footprints tally.ir.json
world helper
  tape 0 (v): writes {2} of 3

world main
  tape 0 (t): writes {2} of 3
  tape 1 (scratch): writes {} of 3
```

`tape K (NAME): writes {…} of CARD` names the tape by its signature
position and its own name, lists the write set as ascending indices, and
states the tape's cardinality alongside it — `of 3` is the tape's own
alphabet size, not a count of anything in the set, which can be smaller
(`writes {2} of 3`) or empty (`writes {} of 3`, never omitted: an empty set
is real information, not a missing line). Worlds render in the IR's own
program order — the same order `--emit-ir` wrote them in — never a
sorted or hashed order, so a report is byte-stable across runs on the same
input. `--function NAME` restricts output to one world, and an unknown
name is an error in the same shape as `ir graph`'s:

```
$ tmt ir footprints tally.ir.json --function nope
tmt: no world `nope` in tally.ir.json
```

**The report is index-only, like the IR itself.** An `.ir.json` carries a
tape's cardinality but never its glyphs — they live only in the `.tmc`
source the IR was compiled from — so `writes {2}` names a *position*, not
a character; recovering what glyph sits at that position means reading the
tape's alphabet declaration back in the source, where a symbol's index is
its position in the declared order (`docs/tmt/language.md (alphabets)`).
`ir graph`'s Mermaid output carries the same limitation — its row labels
are indices too — so neither `ir` child is a glyph-space view; the source
is.

A hand-edited or corrupted `.ir.json` can name two worlds identically,
which a compiler-emitted file never does (`--emit-ir` names are mangled
unique); `ir footprints` rejects that file outright rather than rendering
one world's write sets under the other's tape list:

```
$ tmt ir footprints dup.ir.json
tmt: duplicate world `main` in dup.ir.json
```

## `tmt lint`

```
USAGE: tmt lint PATH... [--exclude PATH]... [--allow CODE]... [--warn CODE]... [--no-config]

PATH is a .tmc or .tma file, or a directory; directories are walked
recursively for *.tmc and *.tma (sorted order, symlinks not followed,
dot-entries skipped). Omitting PATH uses the nearest manifest's declared
source set (docs/tmt/project.md (the declared source set)); requires a
`tmt.json` project and is incompatible with --no-config. .tmc sources
lint through the .tmc rule table; .tma sources through the five
arch-agnostic asm rules plus the TM-1 additions (shadowed rows, retx
exit bounds, unused rept vars, duplicate map source).

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
prog.tmc:4:15: lint: state `s` may trap — its rules do not cover every input and there is no catch-all
```

### Project configuration

For each input file, `tmt lint` discovers a `tmt.json` by walking up from
that file's directory and unions its `lint.allow` with any `--allow` flags.
`--no-config` skips that discovery for every file, leaving the run governed
by the flags alone. See `docs/tmt/lint.md (project file)`.

`--no-config` is rejected outright on a *bare* `tmt lint` — one with no PATH
arguments — because there the manifest's declared source set is the input
itself, so skipping discovery would leave nothing to lint. Alongside
explicit paths it behaves exactly as described above.

There is no `--fix` on `tmt lint` (`--fix` is an unknown flag): nothing it
reports is applied for you on the command line. Several rules do attach a
fix — `docs/tmt/lint.md` says which ones and what each one does — and
where a fix exists it surfaces through the editor's code actions
(`docs/lsp.md (code actions)`) instead.

## `tmt fmt`

```
USAGE: tmt fmt PATH... [--exclude PATH]... [--check]
       tmt fmt - [--check] [--lang tmc|tma]

PATH is a .tmc or .tma file, or a directory; directories are walked
recursively for *.tmc and *.tma (sorted order, symlinks not followed,
dot-entries skipped). Omitting PATH uses the nearest manifest's declared
source set (docs/tmt/project.md (the declared source set)); requires a
`tmt.json` project. `-` reads one source from stdin and writes the
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
extension picks its formatter. Both rewrites are whitespace-only, which
is what makes `--check` a safe CI gate for either language, and both are
idempotent — a `.tmc` comment prints between the same two tokens it was
written between, so nothing needs a second pass to settle. The canonical
styles themselves are `docs/tmt/fmt.md`.

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

Unlike `tmt lint`, `tmt fmt` takes no **configuration** from `tmt.json` —
formatting has no configurable surface for a project file to set, and there
is correspondingly no `--no-config` flag on it. It does read the file in one
case, and only to learn *what* to format: a bare `tmt fmt` with no PATH
arguments formats the nearest manifest's declared source set
(`docs/tmt/project.md (the declared source set)`). Nothing in the file
changes *how* any of those files are formatted.

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

## `tmt dap`

```
USAGE: tmt dap

Run the DAP debug-adapter server for a .tmx program on stdio until the client disconnects.
Exit code: 0 after a clean disconnect, 1 on transport EOF before one.
```

Runs the Debug Adapter Protocol server on stdio, mirroring `tmt lsp`'s role:
the other subcommand that hands real stdio to library code, every protocol
frame going over stdin/stdout. Two `launch` shapes are recognized, named by
which of `"program"`/`"target"` the request's arguments carry (giving
exactly one is required):

- **Program mode** names a prebuilt `.tmx` executable (`"program"`) and a
  `.tmt` tape snapshot (`"tape"`) — unlike `pmt dap`, TM-1 has no
  empty-tape default (the same rule `tmt run` itself enforces), so `"tape"`
  is mandatory in this mode.
- **Target mode** names a `tmt.json` manifest target (`"target"`, an
  optional `"project"` path override) and builds it in process through the
  same path `tmt build TARGET` runs, always with debug info forced on. The
  tape comes from the target's own `run` block; a target with no `run`
  block, or one whose `run` block declares no `tape`, cannot be launched —
  the same rule `tmt build --run` itself enforces.

Both modes cover the full v1 session lifecycle (`initialize`/`disconnect`),
`stopOnEntry`, `configurationDone`, run control (`continue`/`pause`),
source and instruction breakpoints, stepping at line or instruction
granularity, the stack/scopes/variables surface over TM-1's registers and
every tape, `setVariable`, `disassemble`, and the opt-in `"trace"` output
stream. Termination renders a summary `output` event (the same steps/tacts
numbers `tmt run` prints) followed by `terminated` and `exited`, with the
same 0/2/3 exit-code mapping as `tmt run`'s stopped/halted/trapped outcomes.
See `docs/dap.md` for the full launch-config schema, the closed
output-events list, the writable-state contract, and the degradation
rules.

## `tmt completions`

```
USAGE: tmt completions <SHELL>

Emits a shell completion script to stdout for the given SHELL: zsh, bash,
or fish.

  tmt completions zsh  > ~/.zfunc/_tmt
  tmt completions bash > ~/.local/share/bash-completion/completions/tmt
  tmt completions fish > ~/.config/fish/completions/tmt.fish
```

`tmt` is hand-rolled with no argument-parsing framework, so a completion
script cannot be generated by one. Instead the whole CLI surface — every
subcommand, each one's flags with their value shapes, exclusive groups, and
each positional's file-extension filter — is described once in an in-crate
registry, and each shell renderer reads that. A drift-guard test probes the
real parser with every registry entry and cross-checks the `--fno-<pass>`
and `--emit-ir=after:<pass>` choices against the optimizer's own pass list,
so no generated script can quietly fall out of step with the flags the
parser accepts.

All three shells complete subcommand names (including the nested `tape-block
new` / `set` / `show` and `ir graph` / `footprints`), each subcommand's
flags, `-O0` / `-O1` as an either/or pair, `--call-mech`'s three values,
`--lang`'s two, the known `--emit-ir` stages (joined with `=`, the one
spelling the parser reads), one `--fno-<pass>` per optimizer pass, file
arguments filtered to the extension the subcommand actually reads, and
`build`'s target names, read from the nearest manifest at completion time.
A flag already on the line is not offered again unless it is repeatable.

The bash script runs on bash 3.2 and later and needs no `bash-completion`
package: it carries its own helpers, and it re-reads the command line itself
so that `--emit-ir=after:inline` completes as one word even though bash
breaks words at `=` and `:` (the one cost: a path with a space or a quote in
it is not completed). The fish script is a plain list of `complete`
statements, so fish evaluates the conditions itself; file arguments go
through fish's own suffix helper, which ranks the files with the expected
extension first and still lists the rest after them, as fish completions
conventionally do, where bash offers only the matching ones. Directories are
always offered where a file is, in both shells, which is why `lint` and
`fmt` need no special case there.

## `tmt man`

```
USAGE: tmt man

Emits the tmt(1) manual page as man(7) roff to stdout.

  tmt man > tmt.1
  man ./tmt.1
```

The page is built from the same in-crate registry the completion scripts
read — the subcommand list with its one-line glosses — and from every
subcommand's `--help` text, reproduced verbatim in a section of its own, so
the page cannot say anything `--help` does not. It opens with NAME, SYNOPSIS
and DESCRIPTION, lists the subcommands, quotes each one's usage, and closes
with EXIT STATUS and a SEE ALSO naming `pmt(1)` and the reference pages under
`docs/tmt/`. Where the page is installed is the packager's choice; `man
./tmt.1` reads it in place, and `mandoc -T lint` accepts it.
