# Changelog

Release notes for the machine toolchains. Every entry opens with a
version block listing all of the project's version spaces — the
toolchain crates, the per-architecture source languages and assembly
dialects, the IR encodings, the container formats, and the
project-manifest schemas — stating `unchanged` where nothing moved, so
the blocks double as a compatibility matrix across releases.

## [0.5.0-rc.1] - 2026-09-02

The first release candidate for 0.5.0, cut so the browser demo can
integrate the bundle before the final release; the entry is retitled
`0.5.0` at that cut, with whatever lands in between folded in.

The browser release, and the release in which the front ends stop
parsing twice. `mtc-wasm` exposes both toolchains to JavaScript — compile,
lint, format, disassemble and run a program inside a page — and the
release workflow builds the bundle it compiles to, smoke-tests it and
attaches it to the tagged release.

Underneath it sits the larger half of the range: `.pmc` and `.tmc`
now each parse exactly once, into a lossless syntax tree that
reproduces its source byte for byte, and the compiler, the formatter,
the linters and the language servers all read that one tree. What a
user sees of it is that formatting never moves a comment again on
either language, that a quickfix never eats one, and that a document
broken mid-edit still answers.

| Version space | This release | Previous |
|---|---|---|
| Toolchain crates (`mtc-core`, `mtc-post-machine`, `mtc-turing-machine`) | **0.5.0-rc.1** | 0.4.0 |
| `mtc-wasm` crate / JavaScript API | **0.5.0-rc.1** — new | — |
| `.pmc` language | 0.4 — unchanged | 0.4 |
| PM-1 `.pma` dialect | 0.3 — unchanged | 0.3 |
| `.tmc` language | 0.1 — unchanged | 0.1 |
| TM-1 `.tma` dialect | 0.3 — unchanged | 0.3 |
| PM IR encoding (JSON) | 4 — unchanged | 4 |
| TM IR encoding (JSON) | 3 — unchanged | 3 |
| Container formats (MO / MX / MT) | 3 / 2 / 2 — unchanged | 3 / 2 / 2 |
| `pmt.json` project-manifest schema | 0.2 — unchanged | 0.2 |
| `tmt.json` project-manifest schema | 0.2 — unchanged | 0.2 |
| VS Code extensions (PM-1, TM-1) | 0.2.0 — unchanged | 0.2.0 |
| JetBrains plugins (PM-1, TM-1) | **0.2.1** | 0.2.0 |
| Tested-binary floors of all four plugins | 0.4.0 — unchanged | 0.4.0 |
<!-- maintainer: floors -->

No source language, assembly dialect, IR encoding, container format or
manifest schema moved this release: a program that built with 0.4.0
builds unchanged, and every artifact either toolchain wrote still
loads.

### The browser bundle

- **`mtc-wasm`**, a fourth workspace crate, is the JavaScript binding:
  three classes — `Toolchain`, `Program` and `Session` — over the same
  compiler, linker and virtual machine the command-line tools run, with
  every other type a plain JavaScript object. It needs no filesystem,
  no clock and no server: a page loads the module and compiles, lints,
  formats, disassembles and runs `.pmc` and `.tmc` sources locally.
  `docs/wasm.md` is the reference.
- **The bundle.** `scripts/build-wasm-bundle.sh` produces the module,
  the wasm-bindgen glue as an ES module (`web` target, initialised from
  a URL, a `Response` or raw bytes), a generated `.d.ts` that is the
  API reference, and a `manifest.json` carrying the toolchain and crate
  versions, the wasm-bindgen version, the commit it was built from and
  a SHA-256 per file. The manifest is what a consumer pins; verify the
  checksums before loading. The release workflow builds the tarball on
  CI with the pinned toolchain and attaches it to the tagged release.
- **Size.** Both toolchains' full chains, JavaScript boundary included,
  measure 1.12 MB raw and 450 KB gzipped — the order of a
  diagram-rendering library. A ceiling of 1 MB gzipped is enforced by
  the smoke test, so a size regression fails the build rather than
  reaching a page.
- **The compile channels mirror the CLI's.** `check` returns lint
  findings as warnings and a compile fatal as one error, with the
  library's own fix edits attached; compile *warnings* come with
  `build`, the same split the command line keeps between `lint` and
  `compile`. `format` returns the canonical whitespace-only text or
  that same fatal. All diagnostic and fix positions are half-open
  UTF-16 string offsets — the coordinate a browser editor indexes by —
  and a position past the end of the text clamps to its length.
- **`Program`** exposes the band layout with real glyphs, a listing row
  per instruction (address, bytes, mnemonic, operand, plus the
  containing function and any label), the line table in both directions,
  reassembleable disassembly text, and the executable image and map
  sidecar exactly as the CLI would write them. Freeing a program while
  one of its sessions still runs is safe — the session borrows the
  process-wide architecture registry, not the program — so a page may
  rebuild while an earlier run is in flight.
- **`Session`** is the run, pumped by the embedder: tapes live inside
  it, one band per machine tape, seeded in alphabet indices and blank
  where no seed is given. `pump` retires instructions until its budget
  runs out, a pause fires or the program ends, and reports which;
  `pause`, breakpoints, per-band snapshots, the registers, the call
  stack, statistics and `stop` complete the surface. A trap is not a
  pause: it ends the run, and its kind is spelled the way the CLI's
  exit-code-3 family spells it, with `stopped` and `halted` matching
  exit codes 0 and 2. A TM-1 program's tape count must fall in
  `1..=16`, and a seed cell outside its band's alphabet throws, naming
  the band.
- **Two argument conventions worth knowing before the first call.**
  The trailing options arguments — `check`'s and `build`'s options
  object, `session`'s seeds and limits — are typed `T | undefined`
  without TypeScript's `?`, a limitation of the wasm-bindgen version
  the bundle pins rather than a design choice: pass `undefined`
  explicitly instead of omitting them. `pump`'s budget is the one
  genuinely optional parameter, and it rejects anything below 1 — a
  zero budget would spin rather than advance.
- **Failure modes are documented, not discovered.** Expected errors are
  values or thrown errors; a Rust panic is a bug, and the module is
  built with `panic = "abort"`, so it takes the module with it. An
  embedder that sees an undocumented throw should discard the module
  and recreate its worker rather than keep calling in.
- **What the binding does not carry**, deliberately: project manifests
  and user libraries, the composition of several assembly units, the
  language-server surface, and a JavaScript-implemented tape device.
  None is needed to compile, inspect and run a program in a page.

### Assembly, tape blocks, and the standard library with its lines

Three surfaces the demo's own design asked of the first candidate, all
additive: every call the first candidate had keeps its meaning.

- **Assembly is a language, not a read-only view.** `lang` takes
  `"pma"` and `"tma"` beside `"pmc"` and `"tmc"` — a language is an
  architecture crossed with a kind, and everything downstream of a build
  keys on the architecture alone. `build` on an assembly language runs
  the assembler with its line table on, so `lineOf` and
  `addressForLine` answer against the physical lines the instructions
  were written on, and links the unit against the embedded stdlib, so a
  hand-written `call std::goToEnd` resolves; the result is the same
  `Program`, and `optLevel` is accepted and ignored. An assembler
  refusal is the one error, carrying the assembler's own code. `check`
  runs the assembly lint behind the same assemble gate the CLI uses, and
  `format` prints the canonical column grid. A TM-1 image carries only
  cardinalities, so an assembled program's bands are `tape0`, `tape1`, …
  labelled with the decimal strings `0`…`card-1`, the convention the
  CLI's tape-block authoring already uses. The disassembly of a
  source-built program builds as assembly to the same code bytes — the
  text-expressibility gate, through the browser.
- **Tape blocks travel as values.** `decodeTapeBlock(bytes)` returns the
  block alphabet and, per tape, its cells with the glyph table those
  cells actually index — the tape's own if the file gave it one, the
  block's otherwise; `encodeTapeBlock(block)` is the inverse, taking the
  decoded shape with its redundancies optional, and a session's
  snapshots as they are. The container version is the format's own rule
  rather than a parameter: every tape inheriting the block alphabet is
  version 1, what `pmt` writes; any tape with its own table is version
  2, what `tmt` writes for bands that differ. Cells are validated before
  anything is written, and every shape the container cannot hold throws
  instead of aborting the module. `Program.seedsFromTapeBlock(block)`
  maps a block onto the program's bands **by glyph**, so a block that
  spells an alphabet in another order still lands on the right symbols,
  and a glyph the band does not know throws naming the tape, the glyph
  and the band — a block authored for another program never silently
  relabels.
- One change outside the crate: each toolchain's allow-list validator
  is public, so the binding validates lint `allow` codes on assembly
  through the same shared namespace the CLI does.

### One parse path

- **Both source languages parse once.** `.pmc` and `.tmc` each build a
  single lossless syntax tree, and the compiler front end, the
  formatter, the linters and the language services all read it — the
  second, hand-written parse tree each crate used to build alongside it
  is gone. The tree reproduces its source byte for byte, which is what
  makes "the formatter cannot lose what you wrote" a structural
  property rather than a promise. `docs/core.md` (syntax trees)
  describes the framework.
- **Assembly joins them.** The `.pma`/`.tma` parse emits a tree from
  the same shaping walk that builds the assembler's own item records,
  so the two cannot drift, and the tree is byte-lossless for any input
  — CRLF included. `.rept` blocks nest, and every item, comment
  included, can be located in it; both assembly language services now
  take their per-item line structure from the tree instead of zipping
  against the source's non-blank lines.
- **A broken document still parses.** Recovery wraps the broken region
  in an error node and resynchronises: a broken top-level declaration
  costs its declaration, a broken statement costs its statement inside
  an otherwise intact function, and a broken state, graft, bind or tape
  costs itself inside an otherwise intact world. Every sibling keeps
  its parse, and the tree stays lossless across the damage.
- **What that buys in the editor**: document symbols answer from the
  current text across a broken declaration, listing the declarations
  around the damage with their real positions — only a lexing failure
  now leaves an outline unanswered. Every other feature stays keyed to
  the clean parse by construction.
- **Formatting refuses broken input, deliberately.** Canonical layout
  over an error region is undefined, and silently reformatting around
  broken text on save is worse than declining to.
- **Rule-level recovery inside a `.tmc` state is out of scope** by
  ruling — the statement and world item are the recovery grain.

### Formatting and comments

- **A comment is never moved, on either language.** `pmt fmt` and
  `tmt fmt` print every comment between the same two significant
  tokens it was written between, and both formatters are now
  unconditionally idempotent — one pass settles, always. This replaces
  the previous per-position relocation rules, under which a comment in
  a declaration header, a label region or a list could be drawn
  somewhere else in the output. `docs/pmt/fmt.md` and `docs/tmt/fmt.md`
  are the references.
- **`.tmc`**: header comments stay in their header, in every
  declaration family; a comment inside a list entry prints inside
  that entry without forcing the list multi-line; and a rule's
  action-slot comment stays in its slot, with the state grid ruling on
  the rule — a block comment keeps its rule on the grid, occupying its
  column and widening it for the group, while a line comment takes that
  rule off the grid, where only its own commented vectors break.
- **Two `.tmc` relocations are recorded residuals**, both stable on the
  first pass: a `call` transition's own machinery claims comments past
  its target, and a `with map` comment on a map-bearing argument moves
  onto the map's opening brace.
- **`.pmc`**: a comment in the label region prints in the label prefix
  and the statement takes the own-line-label layout; a comment before
  the terminating semicolon prints before it; comments inside a
  `check`'s arms, a command's successor, a call's arguments or a `use`
  path print inside that item or path; and a header-interior comment
  prints on the header line in the slot it was written in. The
  stacked-label shape that used to need a second pass to settle is
  gone with the relocation machinery.
- **A consequence to expect**: a blank line inside a header, a label
  region or an item's parentheses renders compactly, since the comment
  around it now stays where it was written.

### Lint and quickfixes

- **A quickfix whose edit span contains a comment is withheld** — the
  finding still reports, only the remedy goes. Applying it would
  silently delete the comment, and since `pmt lint --fix` writes the
  result to disk this was data loss, not merely an editor nicety going
  wrong. The guard is one chokepoint per lint surface rather than a
  check inside each rule, so a fix-emitting rule added later is covered
  by construction, and it covers all four languages: both source
  linters and the architecture-neutral assembly layer. Both entry
  channels — the batch command and the editor service — run through it.
  `docs/pmt/lint.md` and `docs/tmt/lint.md` gained a quickfix
  availability section, and `docs/core.md` documents the assembly-side
  guard.
- **`.tmc` quickfix spans are now node-range queries** over the syntax
  tree — the declaration or statement node's own range, or a token pair
  inside it — instead of scans outward from an anchor token. A comment
  anywhere in a declaration therefore lands *inside* the span, where
  the guard withholds the fix, and no span helper can be truncated or
  voided by one.

### The standard libraries

- **Every exported tape of the TM-1 standard library declares an exact
  `writes` clause.** Fifteen of the forty carried one before; the other
  twenty-five made no machine-checked claim at all. Each clause names
  precisely the symbols some run of the routine writes, and the
  compiler's contract check holds the body to it. The compiled object
  is byte-identical before and after — a clause changes what the
  compiler checks, never what it emits.
- **The write-footprint inference believes an external callee's
  declared contract.** A call whose target lay outside the compilation
  unit used to contribute the caller tape's whole alphabet, so hovering
  any routine that touched the standard library reported it as writing
  every symbol of the tape — including routines that write nothing at
  all. An external callee whose resolved signature is visible now
  contributes its declared effective set, projected through the binding
  exactly as a local callee's inferred set is; a callee nobody vouches
  for still contributes everything, and objects at the link boundary
  carry no contracts and stay conservative.

### Language servers and editors

- **Every `as NAME` is a declaration, and what it stands for is one hop
  away.** A reference to an aliased name lands on the `as NAME` that
  introduced it — a `goto` on its graft statement's alias, a call
  written through a `use` alias on the alias itself — and one more
  jump from that statement reaches the graph or the imported
  declaration. Two instances of one graph stay distinguishable, and the
  rule matches what an editor means by "definition" everywhere else.
  `docs/lsp.md` states it for both languages.
- **The services are measurably lighter per keystroke**: the line index
  is built once per document version instead of per request at eight
  call sites, formatting prints from the tree the document state
  already holds rather than re-lexing and re-parsing, and the tree's
  sibling and last-token queries are addressed rather than walked, so a
  top-level traversal is no longer quadratic in allocations.
- **JetBrains plugins 0.2.1 — every language feature works again.**
  Under 0.2.0 both plugins started the language server for an opened
  file and then went silent: no navigation, no hover, nothing, on every
  LSP4IJ version. The file types were language file types with no
  parser definition, and the platform then builds a plain-text syntax
  model whose language disagrees with the file's — a document the LSP
  bridge never opens. Each language now owns a minimal parser
  definition of its own, and both plugins additionally register, for
  their languages, the per-language extensions the bridge otherwise
  provides only for plain text: the semantic-token view behind quick
  documentation, the structure view, folding, code blocks and parameter
  info.
- **The Ctrl/Cmd-hover underline is identifier-precise** in both
  JetBrains plugins, a direct consequence of owning the file's syntax
  model. The whole-file underline previously recorded as an upstream
  limitation was ours.
- **The editor READMEs are user pages now** — requirements, install,
  settings, tasks or run configurations, debugging, manifest
  validation — with the build-and-sideload instructions and manual test
  checklists moved to a maintainers' file beside each and kept out of
  the packaged extension.
- **The DAP disassembly view is still unreachable from JetBrains**: the
  IDE's split debugger front end leaves LSP4IJ's action without a
  session to act on, so it hides itself. Fixed upstream and verified on
  a nightly; the plugins pin a released version that does not carry the
  fix yet.
- The VS Code extensions are unchanged at 0.2.0, and all four plugins
  keep their tested-binary floor at 0.4.0.

### Disassembly and debugging

- **Wide instructions read again in the listing.** A sixteen-tape
  `wrmv` is twenty-five bytes and a ninety-nine-character operand, and
  folding that into one lane interleaved hex with mnemonics until
  neither column could be read down. Bytes now wrap after five per
  line and the operand wraps in its own column, breaking between whole
  vectors first and inside one only when a single vector cannot fit.
  `run --trace` is unaffected and stays one row per retired
  instruction.
- **The debug adapters send `instructionBytes` separately.** A
  disassembly row's `instruction` field now holds mnemonic and operand
  alone, so a client with its own address and bytes columns no longer
  renders them twice. Both fields come from the same listing the CLI
  composes its grid from.
- **`dis` prints the far mnemonic for a linker-narrowed site**, which
  is why `pmt dis` shows `call` where the image holds `call.s`. That is
  deliberate — far is the only form the assembler accepts, and
  re-linking the recovered text re-derives the same narrowing, which is
  what keeps disassemble → assemble → link byte-exact. Previously
  recorded only in a source comment, it is now stated in `docs/core.md`
  and cited from both `dis` references; `--listing` is the byte-exact
  view beside it.

### Documentation and examples

- **Five worked examples join the flagship**, with one runner over all
  six. Four are the same RPN calculator under four representations —
  variable-length binary through the standard library, four hex digits
  packed into one tape, those digits split into a register over four
  tapes, and four tapes that *are* the stack — so the trade-off between
  tape count and vector width is visible side by side rather than
  asserted: on one addition the standard-library form takes 788,375
  steps where the packed, register and wide forms take 279, 195 and 164.
  The fifth is a flat unary machine using no reuse constructs at all.
  Each example carries three differentials: `-O0` against `-O1`, the
  three bound-call lowerings against each other, and a
  disassemble/reassemble round trip, each comparing whole final tapes
  rather than the one value a case checks.
- Both `run` command references now carry the exit-code table (PM-1's
  was prose), the `tmt.json` lint section moved to the lint page where
  `pmt.json`'s already lived, and both README quick starts show a
  `--listing` block beside the canonical disassembly.
- `docs/wasm.md` is the new reference page for the browser bundle.

### Tooling and CI

- **The Rust toolchain is pinned.** `channel = "stable"` pinned
  nothing — rustup re-resolved it per machine, so a fresh CI runner and
  a laptop months behind compiled the same commit with different
  compilers and different lints. The pin is an exact version now — Rust
  1.98.0 — which makes plain `cargo clippy` locally the same gate CI
  runs and retires the side-toolchain check that used to compensate.
  The `wasm32-unknown-unknown` target is pinned beside it, so rustup
  installs it wherever the toolchain is installed.
- **All four library crates are gated on `wasm32-unknown-unknown`.**
  The sans-I/O core was designed for the browser and the compilers and
  linker touch no filesystem or clock, so the property was already
  true; CI now enforces it. The gate is compile-only: the standard
  library on that target is a stub, so a filesystem or clock call
  leaking into a library would still build and only misbehave in a
  page.
- **The bundle is built and smoke-tested on every push to the default
  branch and every pull request**, loading the real glue and running
  both toolchains end to end — check, format, build, a pumped session,
  the line table and a trap — verifying the manifest checksums and
  holding the gzipped module under its ceiling. The release workflow
  runs the same smoke test before attaching the tarball to a tag.
  wasm-opt runs against an explicitly named feature set and binaryen is
  installed from a pinned upstream release, so a distribution's
  years-old wasm-opt can no longer reject the module.
- **The worked examples' case tables run in the test suite**, reading
  the same tables the shell runner reads, so the two can never assert
  different things. A case expecting a trap must trap on a missing
  transition specifically, so an exhausted step budget can no longer
  pass for a diagnosis.

### Fixes

- Both command-line tools exit quietly when their standard output is
  closed early — `tmt dis --listing … | head` and its `pmt` twin used to
  end in a panic about a broken pipe rather than the usual silence a
  reader that stopped listening deserves.
- `pmt fmt` no longer duplicates a comment after a function's opening
  brace on every pass — an empty body, a body of only comments, or a
  blank line before the first statement grew one more copy per run, and
  `--check` could never go green on such a file.
- Two same-line comments between `use` and the first path printed glued
  together; each is spaced now.
- A `.tmc` block comment whose text spans lines no longer pads its
  neighbours' columns against its full width: it takes its rule off the
  grid, as a line comment always did.
- The `.pmc` non-camel-case lint stops suggesting a rename to the name
  already written, and offers a rename only when the derived name lands
  inside the convention's alphabet — a non-ASCII identifier could
  previously be advised to become another non-conforming one, or one
  the lexer rejects outright.

## [0.4.0] - 2026-08-17

The debugging release. Both toolchains gain a Debug Adapter Protocol
server — `pmt dap` and `tmt dap` — turning the engine's existing
`DebugSession` into something an editor drives: breakpoints in the
gutter, stepping at line or instruction granularity, machine state in
the variables view, a disassembly view for the code between the lines.
Both editor plugin pairs ship as its clients, and the optimizer on each
side picks up a round of motion and value passes.

| Version space | This release | Previous |
|---|---|---|
| Toolchain crates (`mtc-core`, `mtc-post-machine`, `mtc-turing-machine`) | **0.4.0** | 0.3.0 |
| `.pmc` language | 0.4 — unchanged | 0.4 |
| PM-1 `.pma` dialect | 0.3 — unchanged | 0.3 |
| `.tmc` language | 0.1 — unchanged | 0.1 |
| TM-1 `.tma` dialect | 0.3 — unchanged | 0.3 |
| PM IR encoding (JSON) | 4 — unchanged | 4 |
| TM IR encoding (JSON) | **3** — the `direct` lowering hint on rules | 2 |
| Container formats (MO / MX / MT) | 3 / 2 / 2 — unchanged | 3 / 2 / 2 |
| `pmt.json` project-manifest schema | 0.2 — unchanged | 0.2 |
| `tmt.json` project-manifest schema | 0.2 — unchanged | 0.2 |

The map sidecar (a JSON companion, not a numbered container) gains one
optional per-function field, `source` — additive and
backward-compatible; pre-provenance sidecars keep parsing and keep
their old behavior.

### Debugging over DAP

- **`pmt dap` / `tmt dap`**: a stdio Debug Adapter Protocol server on
  each toolchain, editor-agnostic, built on a new architecture-neutral
  server loop in the core (the same framing the LSP servers use, plus a
  run/drain alternation so a running machine keeps advancing between
  client requests). `docs/dap.md` is the reference.
- **Two launch modes.** Target mode names a project-manifest target and
  builds it in process through the same driver `build` uses, with debug
  info forced on; program mode takes a prebuilt executable and (for
  TM-1, required; for PM-1, optional) a tape snapshot. `stopOnEntry`
  and a per-instruction `trace` stream are common to both.
- **Breakpoints**: source breakpoints resolve through the map's line
  table with the same snapping rule breakpoints have always had, answer
  verified/unverified honestly, and — when the map carries source
  provenance — are filtered per file, so two translation units sharing
  a line number can no longer capture each other's requests. DAP's
  per-source REPLACE semantics are honored per file and independently
  per breakpoint kind; instruction breakpoints need no map at all.
- **Stepping**: line granularity walks instructions until the resolved
  (function, line) position changes, function identity included;
  instruction granularity steps exactly one; step-in/over/out follow
  call depth, mirroring the engine debugger's own controls.
- **State**: registers and per-tape scopes in the variables view, tape
  cells and writable flags editable through `setVariable` (instruction
  pointers stay read-only, as does TM-1's frame register), and the
  debuggee's stopped/halted/trapped outcome reported as the same exit
  codes `run` uses.
- **The disassembly view**: `disassemble` serves strictly positional
  windows around any address — the head of the address space pads with
  placeholders rather than sliding the window, which is what keeps a
  client's address arithmetic honest.
- **Source provenance.** Builds record, per function, the source file
  it came from; the adapters attach a DAP `source` object to every
  frame whose provenance resolves to a file that exists, which is what
  lets an editor focus the file and highlight the line on every stop.
  Frames at an address before the function's first line-mapped
  instruction render at the function's opening line (the
  native-debugger prologue convention); a function with provenance but
  no line table at all stays sourceless — never a sourced line 0,
  which the protocol forbids and a real client punishes.
- **Lifecycle honesty**: a client that vanishes without `disconnect`
  mid-run ends the session instead of leaving the server ticking
  unobserved at full CPU with a dead transport.

### The optimizer motion/value round

- **PM-1, two new passes.** `move-elim` cancels inverse move pairs
  proven equivalent under the match-flag dataflow, converging within a
  single optimization run; `tail-sink` deduplicates arm suffixes past a
  check join. Both obey the standing equivalence contract and the
  `brk` observability barrier.
- **TM-1.** `dead_rows` learns identical-effect subsumption across
  bands; `jump_threading` marks bare-goto rules *direct* and codegen
  emits their dispatch targets without the intermediate stub states —
  the lowering hint that moves the TM IR encoding to version 3; the
  default-off `outline` clears its direct hint on escape rewrites.
- **Tuned inlining.** The inline cap is now an explicit option on the
  compile surface, and the shipped defaults were chosen by a sweep
  harness over the example corpus rather than by feel.

### Editor integration

- **VS Code**: both extensions contribute a debugger type launching the
  same binary the language client resolves, with launch configuration
  schemas, initial configurations and snippets for both launch modes,
  breakpoint support in all four languages, and the workspace folder as
  the adapter's working directory.
- **JetBrains**, plugins at 0.2.0: run configurations gain the `build`
  subcommand with a manifest-target picker fed by `build
  --list-targets` and a run-after-build toggle; the DAP servers
  register with LSP4IJ's debugger bridge (gutter breakpoints, stepping,
  variables, editable launch templates); the bundled project-manifest
  JSON Schemas attach to `pmt.json`/`tmt.json` automatically; and
  TextMate syntax coloring in the IDE actually works now — the file
  types became language file types colored through the plugin's own
  highlighter providers, replacing a borrowed registration that could
  never resolve a foreign file type and rendered plain text.
- All four plugins move to 0.2.0 with their tested-binary floors at
  0.4.0.

### Fixes

- PM-1's move-elimination converges within one optimizer invocation
  instead of leaving work for a hypothetical second run.
- The core builds again without the standard library (the `no_std` VM
  gate) after the source-path helper landed std-only.
- The Delphi-era example programs the project descends from join the
  golden corpus as derivation-first tests.

## [0.3.0] - 2026-08-11

The release that turns a Post-machine toolchain into a toolchain
*family*. `tmt` — the toolchain for the multi-tape Turing machine TM-1,
from a C-like source language through an assembler, a composing linker
and a multi-tape virtual machine — ships whole, on the same
architecture-agnostic core `pmt` runs on. Alongside it, PM-1 learns to
compile for tapes it does not own, and the core VM grows an
asynchronous execution surface for driving real hardware.

| Version space | This release | Previous |
|---|---|---|
| Toolchain crates (`mtc-core`, `mtc-post-machine`, `mtc-turing-machine`) | **0.3.0** — `mtc-turing-machine` is new | 0.2.0 (no Turing crate) |
| `.pmc` language | **0.4** | 0.3 |
| PM-1 `.pma` dialect | **0.3** | 0.2 |
| `.tmc` language | **0.1** — first release | — |
| TM-1 `.tma` dialect | **0.3** — first release | — |
| PM IR encoding (JSON) | **4** | 3 |
| TM IR encoding (JSON) | **2** — first release | — |
| Container formats (MO / MX / MT) | **3 / 2 / 2** — readers accept every earlier version | 2 / 1 / 1 |
| `pmt.json` project-manifest schema | **0.2** | 0.1 (the lint-only shape, numbered retroactively) |
| `tmt.json` project-manifest schema | **0.2** — first release | — |

### The TM-1 toolchain

- **A second architecture on the shared core.** TM-1 is a multi-tape
  Turing machine: several tapes, each with its own head and its own
  glyph alphabet of up to 256 symbols, driven by one instruction
  stream. The new `mtc-turing-machine` crate plugs into `mtc-core`
  through the same two tables PM-1 uses — the micro-op lowering table
  and the assembler syntax table — so the core still carries no
  architecture knowledge of either machine. The syntax table itself grew
  for this release: an architecture now declares which optional
  assembler capabilities its dialect wants, which is how the whole
  table/macro/vector surface below can be on for TM-1 and off for PM-1.
- **`tmt`**, the driver: twelve subcommands — `compile`, `asm`, `link`,
  `build`, `dis`, `run`, `tape-block`, `ir`, `lint`, `fmt`, `lsp`,
  `completions` — sharing `pmt`'s exit-code convention (0 stopped, 2
  halted, 3 trapped).
- **The `.tmc` language, version 0.1.** Programs are built from
  *worlds* — the `machine` block and its routines — over declared
  alphabets and tapes. A rule is a triple in fixed order — a bracketed
  pattern with one cell per tape, an action of optional `write` and
  `move` vectors, and a transition — and the transition may be omitted,
  in which case the machine stays in the current state. Three reuse
  mechanisms with different semantics: `call` (a runtime call through
  the machine's call mechanism), `graft` (compile-time splicing of a
  graph into the caller), and `bind` (a named binding of a routine to
  a tape tuple). Symbol maps translate between a caller's alphabet and
  a callee's, with the blank pinned, identity completion for equal
  alphabets, and closed maps with holes for unequal ones. Pattern
  ranges and `{…}` substitutions expand at compile time over the
  assembler's own expression grammar. Namespaces, visibility and
  imports work as in `.pmc`, as do doc lines, attention lines and the
  `[deprecated]` attribute.
- **The `.tma` assembly dialect, version 0.3**, and the capabilities
  behind it. The core assembler gained a capability gate (all
  capabilities off by default, which is what keeps PM-1's dialect
  untouched); TM-1 turns all of them on: `.section`/`.row`/`.targets`
  match-and-dispatch tables with discipline validation, `.rept` macros
  with `{expr}` substitution, `[…]` vector operands, `.routine`
  signature declarations, and the `.frame`/`.map`/`.exits` frame
  descriptors.
- **The instruction set** is the familiar base set plus what a
  multi-tape machine needs: a batch `rd` across every head, `wr` and
  `mov` as per-tape vectors (with a keep element and a stay move), the
  fused `wrmv`, `mtc`/`djmp` table dispatch through a match register,
  an explicit `trap`, and the framed `call.m`/`retx` pair.
- **The frames execution profile.** A frame register and a frame cache
  in the core VM turn a call into an indexed frame selection: `call.m`
  takes a call-site index, `retx` returns through one of a
  descriptor's declared exits, and `trap` stops the machine on a
  deliberately impossible transition.
- **Link-time composition** decides how a bound call is lowered:
  `tmt link --call-mech mono | frames | hybrid` (default `hybrid`).
  `frames` composes at run time, through a composite directory in the
  executable's frames region; `mono` stamps a specialized copy of the
  callee per call site, deduplicated by content digest; `hybrid` picks
  per site. The three mechanisms are proven to agree — same outcome,
  same final tapes — across the example corpus at both optimization
  levels, and the composition algebra is property-tested against a
  brute-force oracle.
- **Binding composition rejects tape aliasing.** One caller tape may
  not be bound to two parameters of the same `call`, `graft` or `bind`.
  The compiler reports `duplicate-tape-target` with the span on the
  offending argument, and the linker refuses the same shape in
  assembly-authored binding records. This is a deliberate narrowing:
  the three call mechanisms disagreed about what such a program does,
  and a grafted aliased call silently dropped one parameter's demands,
  so accepting it made the equivalence guarantee false. A derived
  consequence: a call, graft or bind can no longer name more callee
  tapes than the caller has.
- **Disassembly of a composed image is valid assembler input.** Rows
  stamped by `mono` (and by `hybrid` where it stamps) are re-sorted
  into the assembler's canonical order after specialization, so
  `tmt dis` output of a linked image reassembles.
- **The embedded standard library**, four namespaces of binary-number
  routines over a shared set of graphs: `std::binaryNumbers` (ten
  routines, delimited representation) and `std::binaryNumbersBare`
  (four, bare representation), each with a volatile-signature twin —
  `std::binaryNumbersVolatile` and `std::binaryNumbersBareVolatile` —
  that grafts the same graphs behind volatile tape parameters. Linked
  lazily by reachability; `tmt link --nostdlib` opts out.
- **The optimizer** runs eight passes. Five carry over from PM-1 —
  `inline`, `jump-threading`, `tail-call` before `tail-merge`, `dce` —
  and two are shapes the Post machine cannot even express: `dead-rows`
  removes a row shadowed by another, and `dispatch-select` replaces a
  small enough table with a single conditional jump. The eighth,
  `outline`, is `inline`'s inverse and is off unless `--foutline` asks
  for it. `-O0` output stays bit-identical to plain codegen, a `brk`
  remains a barrier no pass crosses, and the equivalence matrix covers
  `-O0`/`-O1` against all three call mechanisms.
- **A worked example ships, twice.** `docs/examples/brainfuck-utm/brainfuck-utm-handwritten.tma`
  is a hand-written universal Turing machine that interprets brainfuck;
  `brainfuck-utm.tmc` is the same machine in the source language. Both
  build and run, and they are held equivalent by one set of golden tape
  snapshots — same final tapes, same outcome, from two independent
  implementations.

### PM-1: volatile programs

- **`.pmc` language 0.4** adds the `volatile` modifier on the
  un-namespaced top-level `main`. A volatile program is one whose tape
  is not exclusively its own — a device may change a cell between two
  accesses — so the compiler may no longer assume a cell reads back
  what was written. The change is additive except for one reservation:
  `volatile` is now a reserved definition name.
- **Three optimizer passes are disabled in a volatile build**:
  `cell-state` (which deletes idempotent and dead writes),
  `branch-fold` (which decides a check from a value the program merely
  wrote), and the new `fuse-tape-ops`. The other six only rewire
  control flow between accesses they leave untouched, or delete code
  that never runs, so they keep running.
- **Objects carry two build columns.** Every `pmt compile` emits a
  plain and a volatile variant of each function; where the two come out
  identical the body is stored once, and which functions must split is
  decided by a call-graph fixpoint rather than per function. The linker
  picks a column per name, honouring the entry program's volatility
  through libraries as well, and reports it. Compiled at `-O1`, the
  embedded standard library's object grows from 456 to 547 bytes:
  three of its eleven routines need a distinct volatile body, the other
  eight are stored once.
- **Objects compiled by this release are not readable by 0.2.0 tools.**
  Because every compiled object now carries build-column records,
  `pmt compile` writes the MO version 3 shape unconditionally, and a
  0.2.0 `pmt` refuses it with `unsupported format version 3`. The break
  is forward-only — this release reads every earlier object version —
  and an object assembled from a `.pma` that tags no build column still
  serializes in the older shape.
- **`.pma` dialect 0.3**, additive in two ways. The fused write+move
  mnemonics `wrl` and `wrr` join the mnemonic set, and the `.volatile`
  directive tags a `.func`'s build column — or, written ahead of the
  first `.func`, sets the object's program bit. No existing program
  changes meaning.
- **A ninth `-O1` pass, `fuse-tape-ops`**, folds a write followed by a
  move into `wrl`/`wrr`, turning two tape transactions into one. It is
  the reason `-O1` code for a non-volatile program can now be smaller
  than 0.2.0 produced for the same source.
- **PM IR encoding 4** carries the build column, and `pmt ir graph`
  gained `--variant normal|volatile` (plus `-O0`/`-O1`) so either
  column's control-flow graph can be rendered straight from a `.pmc`
  source.
- **Disassembly round-trips again.** `pmt dis` now reads a jump that
  lands on an entry prologue as a tail call rather than as a label —
  but only where that jump opens a genuine cut of the disassembler's
  control-flow walk — so a discovered function boundary no longer
  swallows the code that follows it. The defect was reachable from
  `pmt compile -O1`. One narrow exception remains, and the assembly
  reference documents it: a hand-written body containing an explicit
  `ent` byte that is both an unconditional-jump target and a genuine
  cut re-links to a byte-different but semantically identical image.
- **Tape-block tooling, and two renames that come with it.** The
  `pmt tape` subcommand is now `pmt tape-block`, and `pmt run`'s inline
  tape flag is now `--tape-cells` (it was `--tape`); the old spellings
  are no longer accepted. In exchange, `tape-block` grew `new` and `set`
  alongside `build` and `show`: a block can be created from an
  executable's declared shape and then edited — cells, head, origin, and
  the glyph alphabet — with `--in-place` or into a new file.

### Core: the asynchronous execution surface

- **A poll-shaped device protocol.** Tape devices can now answer
  asynchronously: a device is asked for a command's result and may
  reply "not yet", so a tape that models real latency — or real
  hardware — never blocks the machine. `SyncAsAsync` adapts any
  existing synchronous tape to the protocol, and `LatencyTape` models
  a configurable per-operation delay.
- **`AsyncSession`**, a pump-driven runner: the caller repeatedly pumps
  the session, which either reports that it is waiting on a device or
  that it made progress, and a waitable latch lets an embedder park
  instead of spinning. Cost accounting is reported the same way the
  synchronous driver reports it. Running a program through the pump
  produces bit-identical results to running it synchronously — proven
  over both toolchains' program corpora, not only unit tests.
- **`mtc-core` builds without the standard library.** The crate's
  `std` feature is on by default; with it off, the VM core, the
  containers' in-memory types and the tape devices compile as `no_std`
  against `alloc` — serde becomes optional and tape pages move to a
  `BTreeMap`. The `no_std` build is a continuous-integration gate, not
  an aspiration.

### Project manifests and the `build` drivers

- **`pmt.json` and `tmt.json` each grew a `project` section** (schema
  0.2; the lint-only shape that predates it is numbered 0.1
  retroactively): shared and per-target sources, libraries, named
  profiles, and named targets with their own entry, output and run
  settings. One loader validates the whole file for every consumer, and
  the two sections are discovered independently, so a lint-only file
  sitting between a source and its project root is transparent to the
  project walk.
- **`pmt build` and `tmt build`** are compile-and-link drivers with two
  modes. In argv mode they behave like a C compiler driver — sources in,
  executable out, intermediate objects never touching disk unless
  `--keep-objects` is given. In manifest mode they build named targets
  from the project file, one profile per invocation with individual
  flags overriding it, with `--list-targets` and `--run`. Manifest mode
  rejects the flags the manifest already declares; `--call-mech` is the
  one link-side flag it accepts, as a per-invocation override.
- **The two manifest schemas are independent contracts** and already
  differ: only `tmt.json` carries a call-mechanism default (project-wide
  and per target), and only its run block requires a tape-block
  snapshot, because `tmt run` has no empty-tape default.
- **A bare `lint` or `fmt`** — no paths — now runs over the manifest's
  declared source set instead of scanning a directory.

### Language server: the cross-file overlay

- **Names resolve across a project target.** For a document that
  belongs to a target, `pmt lsp` and `tmt lsp` resolve names against
  the target's declared sibling sources and libraries — exported
  symbols only, first source wins, then declared libraries, then the
  embedded standard library, which is the linker's own order — unioned
  across every target the document belongs to. PM-1 gets the full
  surface (completion, go-to-definition, hover, semantic tokens, and a
  refined `undeclared-external` diagnostic); TM-1 gets the same minus
  semantic tokens, over the narrower set of places its language lets a
  name cross a compilation unit.
- **The `.tmc` standard-library bridge**: `std::` routines now hover
  with their real documentation, navigate into a materialized
  per-version copy of the library source, and complete in `use` paths
  and call and bind targets.
- Both crates carry a fixture comparing the overlay's resolution
  against the real linker's by provenance rather than by name, and the
  language-server reference documents the contract together with its
  known limits — lexical path identity, cross-document staleness, and
  the difference between multi-target completion and single-target
  diagnostics.

### TM-1 footprints and declared contracts

- **Inferred write footprints.** The compiler infers, per world, which
  symbol indices each tape's body may ever write, both from the IR and
  from a resolved program, mirroring the linker's composite semantics.
  `tmt ir footprints` renders them, and hover shows a `writes <tape>`
  line.
- **Declared contracts.** A routine signature may state `writes {…}`
  and `preserves {…}` clauses; the effective permission is the declared
  (or full) write set minus the preserved symbols, checked against the
  inferred footprint. Ten standard-library routines declare contracts.
- **Two new default-on lint rules** come out of the same analysis:
  `dead-map-pair`, which flags a symbol-map pair the callee's footprint
  proves can never be exercised, and `contract-clause-overlap`, which
  flags a symbol named in both clauses of one contract.

### Lint, formatting, and completions

- **`tmt lint`** covers both TM languages: seventeen `.tmc` rules with
  two more available opt-in via `--warn`, and four TM-1 additions on
  top of the five architecture-agnostic assembly rules for `.tma`. One
  allow namespace spans both TM languages, the way the PM pair already
  shares one. `tmt lint` deliberately has no `--fix`; where a rule
  attaches a machine-applicable fix, it surfaces as an editor code
  action.
- **`tmt fmt`** formats both TM languages: a CST-driven canonical
  formatter for `.tmc` and the shared canonical grid for `.tma`. Like
  the PM formatter it changes only whitespace and is idempotent —
  proven by a byte-identical compiled standard library across a
  reformat. One documented exception survives: a comment written
  inside a comma-separated list body moves to the next item.
- **Assembly text stays reproducible.** `tmt compile -S` re-detects
  repeated row families and re-emits them as `.rept` blocks (the
  brainfuck example drops from 1,665 to 160 lines), self-checked by
  assembling both forms and comparing bytes, with `--stamped-asm` to
  opt out; `-g` output is always stamped, because the debug line map
  indexes stamped lines.
- **`tmt completions`** emits a zsh completion script from the same
  registry that describes the argument parser, with drift guards in
  both directions — the same construction `pmt completions` uses.
- Core's `unused-label` rule now sees labels reached only through a
  table section's targets or a frame descriptor's exits. That is what
  let the TM assembly path stop suppressing the rule wholesale — every
  table-driven label had looked unused, four hundred of them on the
  brainfuck example alone.

### CLI

- **Every action answers `--help`, and `-h` aliases it everywhere.**
  Nested group actions (`tape-block …`, `ir …`) used to reject `--help`
  as an unknown flag, and `-h` worked only at the top level.
- **Build diagnostics are never silently dropped.** Both `build`
  drivers used to lose already-rendered compile warnings whenever a
  later stage of the same target failed — a link failure, or any of the
  early exits on the run leg. Every path now flushes what it has
  accumulated.
- A tape-block alphabet with more glyphs than the container format can
  count is a typed error instead of a panic, and `pmt dis` / `tmt dis`
  refuse an image or object built for the other architecture, the way
  `run` already did.
- `tape-block` and `ir` now reject an unrecognized flag the same way a
  leaf subcommand does, on both tools, instead of falling through to
  the bare usage text with a success exit.

### Editors, CI, and docs

- **Four sideloadable editor integrations**, one VS Code extension and
  one JetBrains/LSP4IJ plugin per toolchain, sharing single-source
  TextMate grammars that are drift-guarded against the parsers. The
  Post-machine pair is at 0.1.3; the Turing pair is new at 0.1.0. All
  four now declare a tested-toolchain floor of 0.3.0 and warn when
  pointed at an older binary.
- **Per-target build tasks**: both VS Code extensions offer
  `build <target>` and `build --run <target>` tasks sourced from the
  toolchain's own `--list-targets`, with a problem matcher for compiler
  diagnostics; the JetBrains side ships the same flow as a documented
  run-configuration recipe. A JSON Schema for the project files rides
  along.
- **Continuous integration runs the full quality gate** on every push
  and pull request — formatting, clippy, the `no_std` build of the core
  crate, then the workspace test suite under `cargo-nextest`. Before
  this release, CI only audited dependencies.
- **The documentation set is split by toolchain**: `docs/pmt/` and
  `docs/tmt/` each carry language, ISA, assembly, CLI, optimizer,
  project, standard-library, lint and formatting references, with
  `docs/core.md` for the architecture-agnostic core and
  `docs/formats.md`, `docs/history.md` and `docs/lsp.md` shared between
  the two. A pre-release audit re-ran every transcript in every page
  against the shipped binaries and set-compared every roster — flag
  lists, lint codes, opcodes, standard-library rosters, version
  constants — in both directions.

## [0.2.0] - 2026-07-12

| Version space | This release | Previous |
|---|---|---|
| Toolchain crates (`mtc-core`, `mtc-post-machine`) | **0.2.0** | 0.1.0 |
| `.pmc` language | **0.3** | 0.1 (0.2 and 0.3 both land in this release) |
| PM-1 `.pma` dialect | **0.2** | 0.1 (implicit) |
| IR encoding (JSON) | 3 — unchanged | 3 |
| Container formats (MO / MX / MT) | unchanged | — |

### `.pmc` language

- **Doc lines and attention lines** (language 0.3): a `?` line documents
  the following function; a `!` line carries attention prose or a
  machine-readable attribute — `[deprecated]`, with the rest of the
  line as its message. Runs are docs-then-attention, bind to the next
  function declaration (nested included, at its own indent), and
  dangling runs, out-of-order blocks, unknown attributes, and duplicate
  attributes are compile errors with stable codes. One acceptance
  change rides along: a successor `!` may no longer start a line.
- **Grammar tightenings** (language 0.2): sigil adjacency (`@ name` is
  a syntax error), reserved words barred in all `::` path segments, and
  a pack of clearer parse errors.
- The language version is surfaced as a constant, in the language
  reference's header, and in `pmt --version`.

### `.pma` assembly

- **Dialect 0.2**: labels are letters, digits, and underscores only —
  dots and `::` are rejected (Unicode letters remain valid), which is
  what lets labels and namespaced symbol references coexist without
  ambiguity. The dialect version is surfaced alongside the language
  version.
- **Spanned, coded assembler errors**: `line:col`-precise spans out of
  a total, lossless assembly CST; every error carries a stable
  kebab-case code. Listing output and other non-assembly text is
  refused with a dedicated `raw-line` error instead of being
  misparsed.

### Lint

- `pmt lint` covers both languages: eleven `.pmc` rules (including
  `deprecated-call`, which flags calls to `[deprecated]` functions)
  and five `.pma` rules (unreachable code, unused labels, redundant
  jumps to the next instruction, overlong lines, leftover debugger
  breaks). One allow namespace spans both languages — a single
  `lint.allow` entry in `pmt.json`, on the command line, or in IDE
  settings suppresses a code everywhere. Machine-applicable fixes
  apply with `--fix`; deletion-shaped fixes gate behind `--force`.

### Formatting

- `pmt fmt` formats both languages: the `.pmc` formatter (4-space
  indent, 80-column comma-group wrapping, comment placement, doc-run
  printing) and the `.pma` canonical grid (labels at column 0,
  mnemonics at 8, operands at 16, trailing comments at 32, long labels
  on their own line). Both obey a zero-token-changes contract — only
  whitespace moves; number spellings such as leading zeros are
  preserved exactly (this release fixes a violation where leading-zero
  numbers were rewritten). Disassembler and `-S` output are already
  canonical; formatting them is the identity.

### Language server and editors

- **`pmt lsp`**: one server process serves `.pmc` and `.pma` over
  stdio — diagnostics with stable codes, completions (with operand
  hints for assembly mnemonics and qualified names across namespaces),
  go-to-definition (into a materialized copy of the standard library
  for `std::` calls), document symbols, semantic tokens, formatting,
  lint quickfixes, and — new in this release — **hover documentation**
  sourced from doc lines, with deprecation callouts and strikethrough
  tags on deprecated calls and completions.
- The embedded standard library documents itself: all eleven routines
  carry doc lines, so hover works out of the box on `std::` calls.
- Sideloadable editor integrations for **VS Code** and **JetBrains
  IDEs** (via LSP4IJ), each with a shared TextMate grammar, per-editor
  settings for the binary path and lint allow-list, run/task
  integration, and a manual acceptance checklist. Both plugins are at
  0.1.2, tested against this release.

### CLI

- New subcommands since 0.1.0: `lint`, `fmt` (in-place by default,
  `--check` for CI, stdin via `-` with `--lang`), `lsp`, and
  `completions` (zsh; generated from the same registry that drives the
  argument parser, so completions cannot drift from the flags).
- `pmt --version` reports all three moving version spaces.

### Tooling and docs

- Dependency vulnerability auditing: a `cargo audit` CI gate on
  lockfile changes plus a weekly schedule; the current lockfile is
  clean against the RustSec advisory database.
- A pre-release documentation audit verified every published page's
  claims against the shipped code — the reference pages, the README,
  and both editor guides describe this release accurately.

## [0.1.0] - 2026-07-06

The baseline release: the complete PM-1 pipeline — the C-like `.pmc`
language with namespaces and imports, an eight-pass `-O1` optimizing
compiler with a documented soundness model, `.pmo` objects, a
relaxing linker with lazy standard-library resolution, pure `.pmx`
executables with debug sidecars, `.pmt` tape snapshots, and a
bus-accurate sans-I/O virtual machine with typed traps and a stepping
debug session — driven by `pmt` (compile, asm, link, dis, run, tape,
ir), with the embedded standard library and the durable documentation
set (language, ISA, formats, CLI, stdlib, history).
