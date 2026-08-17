# Changelog

Release notes for the machine toolchains. Every entry opens with a
version block listing all of the project's version spaces — the
toolchain crates, the per-architecture source languages and assembly
dialects, the IR encodings, the container formats, and the
project-manifest schemas — stating `unchanged` where nothing moved, so
the blocks double as a compatibility matrix across releases.

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
- **A worked example ships, twice.** `docs/examples/brainfuck-utm.tma`
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
