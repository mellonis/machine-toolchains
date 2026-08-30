# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

A Rust toolchain family for tape machines. Two architectures share one arch-agnostic core: the Post machine PM-1 (C-like `.pmc` language → optimizing compiler → assembler → linker → bus-accurate VM, CLI `pmt`) and the multi-tape Turing machine TM-1 (`.tmc` language → compiler → `.tma` assembly → linker with table sections and a link-time composition engine → multi-tape VM, CLI `tmt`). GPL-3.0-or-later. It completes work spread across four Delphi implementations (2002–2012); `docs/history.md` has the lineage.

## Current state

**v0.4.0, released 2026-08-17** — "the debugging release": `pmt dap`/`tmt dap` plus both editor pairs as DAP clients, the optimizer motion/value round, and map-sidecar source provenance.

| Contract | Version |
|---|---|
| crates — `mtc-core`, `mtc-post-machine`, `mtc-turing-machine` | 0.4.0 |
| `.pmc` language / PM-1 `.pma` dialect | 0.4 / 0.3 |
| `.tmc` language / TM-1 `.tma` dialect | 0.1 / 0.3 |
| PM IR / TM IR | 4 / 3 |
| containers MO / MX / MT | 3 / 2 / 2 |
| `pmt.json` / `tmt.json` `project` schema | 0.2 / 0.2 |
| editor plugins (all four) | 0.2.0, `MIN_TESTED_PMT`/`MIN_TESTED_TMT` floors at 0.4.0 |

**Both toolchain arcs are complete.** PM-1/`pmt` and TM-1/`tmt` each ship the whole chain — compiler, assembler, disassembler, linker, VM, lint, fmt, LSP, DAP, a project manifest with a `build` driver, an embedded stdlib, and a two-plugin editor pair. The TM-1 flagship `docs/examples/brainfuck-utm/brainfuck-utm-handwritten.tma` — a hand-written universal Turing machine interpreting brainfuck — assembles, links and runs, proven by derivation-first goldens.

**Open work.** #14 zero-copy typed-view CST AST (the "C2" migration — in flight on `feat/c2-green-tree`); #6 a wasm32 build of `mtc-core` plus browser-demo integration, fed by #55 (a brainfuck-runner example); #87 a hardware PM-1 RTL core against the bus contract; #30 a tape-machine testing library (`pmt test`/`tmt test`); #9 a post-machine-js dialect front end; #94 letting a tapeblock buffer a step's per-tape commands and run them in parallel, instead of the bus serializing one device at a time.

**Where the history lives.** How the repo got here — the phase-by-phase trace, and the things that were probed and rejected — is `docs/superpowers/build-history.md`, deliberately not loaded into every session. Per-release facts: `CHANGELOG.md`. Design rationale: `docs/superpowers/specs/`. **Keep this file at standing state** — current versions, live constraints, open work. A finished round's story goes to the history file, not back into this preamble.

## Commands

```
cargo build --release                                   # produces target/release/pmt and target/release/tmt
cargo test --workspace                                  # everything: unit + integration + property tests
cargo clippy --workspace --all-targets -- -D warnings   # quality gate
cargo fmt --check                                       # quality gate
cargo build -p mtc-core --no-default-features        # no_std vm gate (docs/core.md (async session))
```

Single test file / single test:

```
cargo test -p mtc-post-machine --test cli_programs
cargo test -p mtc-post-machine --test opt_equivalence <test_name>
```

Regenerate golden files (explicit, `#[ignore]`d — writes into `crates/post-machine/tests/golden/`):

```
cargo test -p mtc-post-machine --test golden_programs regen -- --ignored
```

CI (`.github/workflows/test.yml`) runs fmt → clippy → the no_std build → `cargo nextest run --workspace` on ubuntu. **The toolchain is pinned in `rust-toolchain.toml`**, so CI and every local checkout compile with the same compiler by construction — plain `cargo clippy` here IS the gate CI runs, with no side-toolchain dance. Bumping the pin is its own deliberate commit: a newer compiler may emit lints the old one never did, and those get fixed in that same change. The file itself carries the why.

`pmt` exit codes from `run`: 0 = program stopped (`stp`), 2 = halted (`hlt`), 3 = trapped. Full flag reference: `docs/pmt/cli.md`; `tmt` shares the exit codes.

Editor plugin builds live only under `editors/` (never repo root). Two independent pairs, PM-1 (`-pm`) and TM-1 (`-tm`), sharing the grammars in `editors/grammars/`: `cd editors/vscode-{pm,tm} && npm run package` (vsix); `cd editors/jetbrains-{pm,tm} && JAVA_HOME=<a JetBrains IDE's bundled JBR> ./gradlew buildPlugin` (zip) — each README has specifics. All four plugins are at 0.2.0 (VS Code: the DAP debugger contributions; JetBrains: manifest-target `build` run configurations, the LSP4IJ-DAP debug bridge, automatic manifest JSON Schemas, and LanguageFileType-based TextMate coloring), with `MIN_TESTED_PMT`/`MIN_TESTED_TMT` floors at 0.4.0. Both JetBrains plugins pin **LSP4IJ 0.20.1**, which cannot reach the DAP Disassembly view at all on IntelliJ-platform 2026.x: its frontend-split debugger UI carries no `XDebugSession` in the action's `DataContext`, so LSP4IJ hid the action. Fixed upstream by falling back to `XDebuggerManager`, verified here on a nightly. **When LSP4IJ tags a release carrying it, bump the pin in both `build.gradle.kts`, state that minimum version in both JetBrains READMEs, and re-run the debug items of both sideload checklists.** Until then a user on a released LSP4IJ has no entry point, which is why `docs/dap.md` still describes none.

## Documentation authority

`README.md` + `CHANGELOG.md` + `docs/` (the shared root pages core, formats, history, lsp; and the per-toolchain domains `docs/pmt/` and `docs/tmt/`, each with language, isa, cli, lint, fmt, stdlib, and `project.md` — the per-toolchain project-manifest reference, `pmt.json`'s and `tmt.json`'s respectively) are the durable references. The original design spec `docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md` is **FROZEN** — a historical record, no longer amended and no longer cited by code. Code comments cite the durable pages by page + parenthetical topic keyword, e.g. `docs/core.md (timing model)`. **No `docs/superpowers/` spec or plan is ever cited by NEW code — frozen or active** (pre-existing citations migrate opportunistically when the surrounding code is next touched; no retroactive sweep). A task brief may quote a driving spec as `spec §N`; that notation is internal and MUST NOT survive into a doc comment. When the durable `docs/` page for a feature doesn't exist yet, carry the substance in prose (a `spec §N` ref is not a placeholder for it) and add the `docs/<page>.md (keyword)` citation once the page lands. Published content (README, `docs/`, code comments) is forge-agnostic: no issue/PR numbers, no hosting-provider URLs — describe substance in prose. Internal artifacts (`docs/superpowers/`, this file) are unrestricted.

## Architecture

Three-crate workspace with a hard boundary. The subsections below carry per-subsystem detail; this is the ownership map.

- **`crates/core` (`mtc-core`)** — arch-agnostic **by contract**, carrying **zero PM-1/TM-1 knowledge**: container codecs (MO/MX/MT), the sans-I/O VM core + bus + driver + tape devices + `DebugSession`, the linker (including table-section emission and the composition algebra), the assembler/disassembler frameworks over a total lossless assembly CST (`asm/{lexer,cst,lower}.rs`, spanned coded `AsmError`) with capability-gated extensions behind `AsmCaps { tables, rept, vectors, volatile }` (default all-off: `.section`/`.row`/`.targets` match+dispatch tables with discipline validation, `.rept`/`{expr}` text macros, `[..]` vector operands, the `.routine` signature directive), the arch-agnostic `asm/lint/` (5 rules driven by `Flow`/`break_opcode`) and canonical-grid `asm/fmt.rs`, the language-agnostic lossless **syntax-tree framework** (`syntax/` — green/red model, `TreeBuilder` with retroactive checkpoints, the `AstNode` typed-view contract and its `ast_node!` macro, `TextLineIndex`; `docs/core.md` (syntax trees)), and the language-agnostic LSP and DAP server frameworks (`lsp/`, `dap/` — transport, JSON-RPC, protocol types, position mapping, document store, multi-service routing behind the `LanguageService` trait). It proves its neutrality by testing against a crate-private fake arch (`vm/arch.rs::test_arch`, arch id `0x7F`) and fake asm dialects.
- **`crates/post-machine` (`mtc-post-machine`)** — everything PM-1: the arch module, the `.pmc` pipeline and its optimizer, the embedded stdlib, the `.pmc`/`.pma` lint and fmt layers, `pmt.json` + the `pmt build` driver, both `LanguageService`s, the DAP adapter, and the `pmt` binary. `pm1_syntax()` opts into exactly one `AsmCaps` capability — `volatile`, for the `.volatile` build-column directive; tables/`.rept`/vectors stay off, and PM-1 byte-identity is a standing regression gate.
- **`crates/turing-machine` (`mtc-turing-machine`)** — everything TM-1 (arch id `0x02`): the arch module (`Tm1::new(tape_count)`, 20 opcodes — the base set plus `trap`, the framed `call.m`/`retx`, and the fused `wrmv`; batch `rd` over all heads, `mtc`/`djmp` table dispatch, per-tape `wr`/`mov` vectors with `-` keep and `<`/`>`/`.` moves; MR written only by `mtc`), the `.tma` dialect (`tm1_syntax()`, caps all on), the `.tmc` front end, the embedded stdlib twins, both lint layers and the `.tmc` formatter, `tmt.json` + the `tmt build` driver, the completions registry, both `LanguageService`s, the DAP adapter, and the `tmt` binary (thirteen subcommands, same exit codes as `pmt`).

Dependencies are deliberately minimal: `serde`/`serde_json` only, `proptest` as a dev-dep. **No clap** — CLI arg parsing is hand-rolled.

### Pipeline and key types

`.pmc` → `lexer.rs` (`Vec<Token>`; grammar 0.3 incl. positional `?`/`!` doc-line tokens) → `parser.rs` (recursive descent; the green tree is the one path every consumer runs — `parse_green_from_tokens` feeds the compiler and the `.pmc` language service through `syntax::extract_program`'s flattening, and feeds `fmt` directly over the raw tree and its trivia, since extraction flattens the tree and drops trivia; `parse` = `lower_cst ∘ parse_cst` over the C1 CST survives only as the differential oracle and inside `#[cfg(test)]` modules) → `compiler.rs::compile(source, CompileOptions) -> CompileOutput` which internally runs duplicate-binding checks → flatten (name mangling + visibility; also builds `Analysis.docs`, the qualified doc/deprecation map consumed by the `deprecated-call` lint, hover, and completion tags) → `ir::lower` (`IrProgram`, a versioned per-function CFG) → `optimizer::optimize` (in-place) → `codegen::emit_program` (CFG → `.pma` text only) → core `asm::assemble` (`ObjectFile`). The IR is a **documented, versioned JSON artifact** (`IR_VERSION` in `ir.rs`), not an internal detail.

Then: core `linker::link(objects, libraries, LinkOptions) -> LinkOutput { executable, map, report }` → `vm::Machine::from_executable` → `run` / `DebugSession`.

**The compiler front end, the `.pmc` language service, and `fmt` all run
the green tree.** `analyze`/`analyze_staged` run `lex_with(WithComments)`
→ `parse_green_from_tokens` → `syntax::extract_program`, and so do the
embedded stdlib's roster and the build driver's source scan. Lint follows
for free — `LintContext` carries tokens and the flattened AST, never a
CST. `fmt` runs the same `parse_green_from_tokens` but skips extraction,
printing straight from the raw tree and its trivia (extraction flattens
the tree and drops trivia). `parse` (`lower_cst ∘ parse_cst`, over the
C1 CST) has zero production callers left — it survives only as the
differential oracle (`text() == source`, and `extract_program`
struct-equal to `lower_cst(parse_cst(...))` across the corpus) and as
the parse behind the optimizer's, IR's, and codegen's own
`#[cfg(test)]` modules. Removing the C1 CST itself — in this crate and
in the TM one — is later work.

### The `.tmc` front end (`turing-machine/src/`)

`lexer` → `parser` → `compiler` (flatten + checks) → `expand` (graft splicing and range expansion, compiler-side stamping, oracle-property-tested) → per-world state-graph `ir` → `optimizer` → `codegen` → core asm. `parser::parse_green`/`parse_green_from_tokens` seed the same parser walk with a green sink, yielding a lossless green tree held to `text() == source` over the shipped corpus and over generated programs; typed views over it (`syntax::views`) and `syntax::extract_program` reconstruct a `Program` struct-equal to `lower_cst(parse_cst(...))`, held to that same equality over the corpus and over generated programs. **The compiler front runs that path**: `compiler::analyze` is `lex_with(WithComments)` → `parse_green_from_tokens` → `extract_program`, with acceptance and errors pinned identical to the old `lex` → `parse` over the corpus and a deliberately-broken set; `analyze_staged` (the language-service entry) now builds `program` by the same route and is pinned to agree with `analyze` field-for-field on a clean source (`tokens` compared directly, not each against a third party) and, at every broken stage, on the WHOLE fatal — kind and span, not just its code. Its `Analysis.tokens` therefore carries comments, so `lint()` filters through `parser::significant_tokens` before the rules walk it — FIVE quickfix helpers index off a neighbouring token instead of searching — `decl_span`, `braced_world_decl_span`, `reuse_statement_span`, `as_clause_span`, `marker_span` — and a comment landing in the indexed position either voids the fix or, between a doc run and its keyword, truncates it into an orphaned `?`/`!` run that no longer parses. Those five are the closed set: they plus `arrow_span` are every reader of `LintContext.tokens`, and `arrow_span` alone is immune because it locates its arrow by range containment rather than by index. **`fmt` runs the green tree too** — `crate::fmt::format` is `lex_with(WithComments)` → `parse_green_from_tokens` → `fmt/print.rs`, which walks the raw tree and asks `fmt/trivia.rs` for the derived layout facts extraction drops. So `parse_cst` has zero production callers in either crate; what survives is test-only, in TWO roles — half of the differential oracle (`parser::parse` = `lower_cst ∘ parse_cst`, the CST side `tests/syntax_parity.rs`/`tests/tmc_property.rs` check `extract_program` against), and the parse behind in-crate `#[cfg(test)]` helpers that are not oracles (`expand.rs`, `compiler.rs`, `syntax/extract.rs`), which the C1-removal plan has to rehome too. The C1 *types* are not dead with the parse path — `fmt/print.rs` still imports `cst::{DocRunItem, DocRunKind}`, which is what `syntax::extract::extract_doc_items` returns. Removing the C1 CSTs themselves is later work. Language rules that are design, not implementation detail (`docs/tmt/language.md`):

- **Substitution passthrough is decided BY TREE SHAPE, not text** — a single bare name is passthrough, and `{(c)}` stays passthrough. Otherwise write-folds take the assembler's exact expression grammar (`+ - * %`, parens, i64, folded per expanded row); `negative-remainder` is raised at *both* ends, mirroring core's `subst.rs`.
- **An omitted transition means "stay in the current state"** (needs ≥1 of write/move/debugger; `call … then` stays mandatory). `Transition::Stay` is resolved in the compiler and never reaches IR — type-level, not a runtime check.
- **`rept_emit.rs` can never change what assembles**: it self-checks by assembling both forms and byte-comparing, falling back to stamped output. `-g` is always stamped, because the debug line map is stamped-line-indexed.
- **Zero-row states are valid** and trap on entry; `NoTransition` parity holds to 126-wide alphabets, and at the 127-symbol ceiling `zero_row` falls back to a bare `trap`.
- **Oversize alphabets (>255) are a typed `FormatError::AlphabetTooWide`**, never a panic.

### The arch contract

An architecture plugs into core through two tables, both living in the arch crate:

1. `Arch` trait (`core/src/vm/arch.rs`) — `operand_kind(opcode)` + `lower(opcode, operand) -> Vec<MicroOp>`: the VM core executes micro-ops and **knows no opcodes**.
2. `ArchSyntax` (`core/src/asm/mod.rs`) — mnemonic/relaxation tables for the assembler/disassembler, plus `break_opcode` (drives the arch-agnostic `leftover-debugger` lint). PM-1's is `pm1_syntax()` in `post-machine/src/asm/mod.rs`; short opcode = far `| 0x10`.

### VM model

`Core` (`vm/core.rs`) is a pure `BusResponse -> BusRequest` transition function — no I/O, no opcode knowledge. The synchronous `driver.rs` answers bus requests and does all tact accounting: fetch/execute cost **core tacts**; device move/read/write add **stall tacts** scaled by `TactProfile`. Traps are controlled stops (typed `Trap`), distinct from `stp`/`hlt`. Tape devices are index-based (the processor never sees glyphs): `InfiniteTape`, `AnnularTape`, and `StrictTape` (a decorator faulting on writing a cell's existing value — the historic 2006/2007 semantics).

### Optimizers (`post-machine/src/optimizer/`, `turing-machine/src/optimizer/`)

**PM-1** — program-level `inline` at round start, then eleven per-function passes fixpoint-looped with a round cap: `check-fold`, `jump-threading`, `cell-state`, `branch-fold`, `tail-sink`, `tail-call`, `tail-merge`, `dce`, `move-elim`, `fuse-tape-ops`. **TM-1** — program-level `inline` (a sound superset of the linker engine's collapse) and the default-off `outline` (`--foutline`), then six per-function: `jump-threading`, `tail-call`, `tail-merge`, `dce`, plus TM-native `dead-rows` (same-band cover) and `dispatch-select` (selective-then-catch-all flagged for the compact `jm` lowering, machine world only). `pass_names()` in each crate is the authority — the completions registry and `--fno-<pass>` are drift-guarded against it.

Constraints that are contracts, not preferences:

- **Pass order**: `tail-call` before `tail-merge` on both toolchains (tail-merge's whole-state dedup destroys the shape tail-call keys on); TM additionally orders `dead-rows` before `dispatch-select`. Stated at each definition site.
- **MF-coupling soundness** (PM, `optimizer/dataflow.rs`): after ≥1 tape op the match flag equals the cell at head; before any tape op it is the decoupled reset value. `Uncoupled | Coupled(_)` lattice; check-edge refinement only on provably coupled paths.
- **Volatile builds disable a strict subset** along one line: treating the match flag as a *register* is sound on any tape; *predicting* it, or a cell, from what the program merely wrote is not. `docs/pmt/optimizer.md` (volatile builds).
- **`-O0` bit-identity**: `-O0` stays byte-identical to plain codegen on both toolchains.
- **Equivalence contract** (`tests/opt_equivalence.rs`, both crates): passes preserve final tape, termination kind, MF-dependent branches. Step counts and resource-limit outcomes may change — except across an un-stripped `brk`, an observability barrier no motion crosses.

### Formats (`core/src/formats/`)

Pure byte codecs, little-endian, no I/O. `.pmo`/MO (objects), `.pmx`/MX (pure code image — tape supplied at run time), `.pmt`/MT (tape snapshots; **glyphs live only here** — an image can only label bands `0..card-1` from its cardinalities, so `tmt tape-block new --from *.tmc` reads real glyphs and tape names from source, and `--alphabet` repins a band afterwards; tape NAMES are never stored, the bus addresses bands by number). Containers are identified by `sniff()` on the magic — **never dispatch on file extensions**. Every reader verifies CRC-32 before decoding anything. Debug names live in the JSON `.pmx.map` sidecar, keeping `.pmx` a pure image.

### Linker (`core/src/linker/`)

Two-phase: `resolve` (namespace + BFS reachability from `main` — unreachable functions are dropped and may reference anything) then `layout` (relaxation: a monotone shrink-only fixpoint that narrows far calls to short; the assembler always emits far `call` — only the linker selects `call.s`). Libraries are first-wins and silently shadowed by user definitions.

### Call lowering and composition (TM-1)

`tmt link --call-mech = mono | frames | hybrid` selects how declarative binding calls lower. The compose model is runtime `FR' = compose[FR][site]` through a composite directory in the MX v2 frames region: `call.m` operands are call-site indices, FR is a composite index, and raw hand-authored sites are constant compose columns. `mono` stamps specialized copies instead (rmap-preimage row rewriting, synthesized trap rows, one-way row expansion, digest-named dedup). The composition algebra (`core/src/linker/compose.rs`) is law-property-tested against a brute-force oracle, and the three modes are proven equivalent by the everything-matrix.

**Both mono stamping refusals — `MonoRawFrame` and `MonoHoleyMatchBranch` — advise `--call-mech=frames`, never hybrid.** Hybrid's classifier inspects top-level sites only and delegates wholesale to mono through its `!any_frames` fast path, so advising hybrid would be circular. Probed, and pinned by link tests in both worlds.

### Stdlib (`post-machine/src/stdlib/`)

An embedded `.pmc` string (`include_str!("std.pmc")`, 11 exported `std::` routines), compiled once per process via `OnceLock` at `-O1` with debugger strips — embedded deliberately because a cargo-installed binary has no data directory. Linked lazily via the reachability pass; `--nostdlib` opts out.

### CLI (`post-machine/src/cli/`)

**Thin-renderer rule: library code never prints.** Every stage returns a structured report (`CompileReport`, `LinkReport`, `OptReport`, `RunResult`); every byte of terminal output originates in `cli/` (rendered under `-v`), and errors flow as typed values. `bin/pmt.rs` is a shell around `cli::execute`. Thirteen subcommands split across `build.rs` (compile/asm/link), `driver.rs` (build — the manifest-aware compile+link driver, argv mode or `pmt.json` manifest mode), `inspect.rs` (dis/tape-block/ir), `run.rs` (run, incl. live `--trace`), `completions.rs` (completions), `lint.rs` (lint — both languages by extension, shared allow namespace), `fmt.rs` (fmt — both languages, stdin via `-` with `--lang`), `lsp.rs` (lsp — the dual-language LSP server on stdio; the only place real stdio is handed to the core server loop), and `dap.rs` (dap — the debug adapter, likewise on stdio). `tmt`'s `cli/` mirrors the split.

`pmt dis`/`tmt dis` **refuse foreign architectures** the way `run` does, for executables and objects alike (`LoadError::UnknownArch` on an `arch` mismatch against `ARCH_PM1`/`ARCH_TM1`).

### Shell completion (`post-machine/src/completions/`, `turing-machine/src/completions/`)

`pmt completions <shell>` / `tmt completions <shell>` render a script to stdout. Because both CLIs are hand-rolled with no clap, no framework can generate them and they would drift: `completions::registry` is the **single in-crate description of the CLI surface** (every subcommand, each flag's value shape, exclusive groups, positional extension filters) and `completions::zsh` renders an `_arguments -C` script from it. The drift guard cross-checks `--fno-<pass>`/`--emit-ir=after:<pass>` against `optimizer::pass_names()` exactly and probes the real parser with every registry entry.

**The one direction the guard cannot check is a real flag the registry is MISSING** — the hand-rolled parser has no reflection over its match arms. `EXPECTED_TOP_LEVEL`, a hand-maintained mirror of the subcommand set, partly compensates; keep it updated. A separate test shells out to a real `zsh` to confirm the script parses and loads under `compinit` (skipped with a note when `zsh` is absent); interactive candidate correctness needs a pty and was checked by hand. bash and fish are recognized names that fail with a clear not-yet-implemented error. Design doc: `docs/superpowers/specs/2026-07-06-pmt-shell-completion-design.md`.

### Lint, fmt, and the shared allow namespace

Four lint surfaces — `.pmc`, `.pma`, `.tmc`, `.tma` — share **one allow namespace**. Core owns a closed arch-agnostic `asm::lint` (5 rules driven by `Flow`/`break_opcode`); each arch merges its own over it. `lint.allow` in `pmt.json`/`tmt.json` is **nearest-ancestor, union — never a cascade**. Rulings that are design, not oversight:

- **`unreachable-rule` is deliberately narrow** — only a second all-wildcard rule. Band dispatch keeps exact and partial rules reachable after a catch-all, so a broader rule would be wrong.
- **`index-identity-map` is a warn-tier audit, not an error** — omitted-map call/bind index identity is intended semantics (`docs/tmt/language.md`).
- **`unused-alphabet`/`unused-tape` and `unused-graft-name`/`unused-exit` are export-independent by design.**
- **Two quickfixes were withheld after probing**: `redundant-identity-pairs` (not byte-identical) and `dead-rule` (silent-miscompile risk); ten more rules carry documented `None` reasons. Re-derive the proof before adding either.
- **Core's `unused-label` sees labels reached through `Dispatch.targets`/`Frame.exits`.** The old `.tma` force-suppression (400 false findings on the flagship) was **deleted** — do not reintroduce it.

`fmt` is canonical and **whitespace-only** on both source languages: `.pmc` and `.tmc` both print from their green syntax trees (`syntax/`); `.pma`/`.tma` use core's canonical-grid `asm/fmt.rs`. **Idempotent with one exception per source language, and the two are ONE mechanism, not two coincidences** — where the printer cannot reprint a comment where it was written it moves it, and the position it moves it to is itself one the printer reads as a layout choice on the next parse, so pass 2 lays the file out differently and pass 3 agrees with pass 2 (`fmt --check` therefore reports one change on a file carrying such a shape; running fmt twice settles it). The two shapes are instances of that. `.pmc` — a comment between two STACKED labels is drawn onto the label line, pushing the command down, and that break re-reads as an author-written break after the labels, so pass 2 drops the comment to its own line. `.tmc` — a comment between a declaration's keyword and its name has to move, and WHERE depends on the declaration: a `state`/`namespace` block body takes it own-line ahead of the first item, a `routine`/`graph` signature or a `graft`'s binding list takes it riding the `(`, a `tape` takes it trailing the `;`, and an `alphabet` body takes it riding the `{`. Only the `alphabet` destination is unstable, and the reason is the shape pass 1 leaves rather than anything about the source: pass 1 emits `alphabet ab { /* a */ '_' }`, which this printer never produces from a source already written that way (a comment riding the `{` breaks the body one element per line), so pass 2 breaks it. The author's own line breaks decide nothing — the same declaration written across three lines collapses to that identical pass 1 and takes the same two passes. Both behaviors predate the green printers and were verified byte-identical against them; TM's fixture pins passes 1, 2 and 3 by value, PM's pins 1 and 2 only. **The compiled-stdlib byte-identity gate is a NEGATIVE CONTROL for a formatter change, never proof that fmt's output is unchanged** — it runs the compiler over the embedded source (`compile(stdlib::SOURCE, …)`), the formatter is nowhere in that path, and unless the change itself reformats `std.pmc`/`std.tmc` that source is not even touched — so the gate passes with a completely broken printer. Run it anyway, at BOTH opt levels, byte-comparing the object before and after: it proves the change did not leak into the compiler. Take actual byte-identity from the printers' pinned fixtures, each crate's fmt-clean dogfood corpus, and — TM-side — the committed `.fmt` sidecars over the adversarial set. Interior list comments print **in place** (own-line keeps its line, trailing rides the preceding entry, a LINE comment forces multi-line). One documented gap, deliberately unasserted so a future fix does not fail the fixtures: the five `paren_list`/`with-map` surfaces break to multi-line on ANY interior comment — a rendering gap, not data loss.

### Project manifests and build drivers

`pmt.json`/`tmt.json` are each **one file with two independently discovered sections**: `lint` walks to the nearest ancestor, `project` to the nearest ancestor that HAS a `project` key — so a lint-only file between a source and its project root is transparent to the project walk. `project.rs` is the **one loader**, validating the whole file for every consumer (CLI and LSP alike), so a typo in either section surfaces for both; `config.rs` is only a lint-only view over it.

`pmt build`/`tmt build` (`cli/driver.rs`) dispatch on whether the positionals look like sources or target names; mixing them is an error. Argv mode is an in-memory cc driver — objects never touch disk without `--keep-objects`. Manifest mode **rejects the flags the manifest already declares** (`-o`/`-L`/`-l`/`--nostdlib`, plus TM's `--entry`); `--call-mech` is the one link-side flag it accepts, as a per-invocation override. Three TM divergences are contract: `call-mech` as both project default and per-target key; a `tape`-only run block (`tmt run` drives a band from a `.tmt` snapshot and has no empty-tape default, so a target without one cannot be `--run`); and a declared source set that keeps `.tma`, dropping only the object extension. A bare `lint`/`fmt` runs over that declared set, never a directory scan; `--no-config` is rejected on a bare `lint`, because there the manifest *is* the input. See `docs/pmt/project.md`, `docs/tmt/project.md`.

### Editor integration (`*/src/lsp/`, `editors/`)

Each arch crate holds both its `LanguageService`s — PM's `.pmc` and `.pma`, TM's `.tmc` and `.tma` — served by one `pmt lsp` / `tmt lsp` process through core's multi-service routing. CLI≡editor parity is structural: one `lint_*` call feeds both. Full feature matrix in `docs/lsp.md`.

- **Extension routing is case-insensitive** (`core/src/lsp/server.rs::bind_service`, compare-time ASCII fold) — `.TMA`/`.PMA` bind like their lowercase forms.
- **The cross-file overlay** resolves a document's names against its project target's declared siblings and libraries in the linker's own order (exports only, first-source-wins, then libraries, then the embedded stdlib), unioned across every target it belongs to. PM-1 gets the full surface; **TM-1 is narrower BY LANGUAGE RULE** — no semantic tokens, and only a transparent argless call/bind or a `use` path may name something outside the compilation unit, never a graft target, binding name/value, `with map` pair, or vector-cell glyph. Both crates carry a faithfulness fixture comparing the overlay against the real linker **by provenance, not merely by name**. `docs/lsp.md` records the caveats (lexical-only path identity, cross-document staleness, multi-target diagnostic divergence, UTF-16 spans into unopened non-ASCII files) — recorded limits, not bugs to rediscover.
- **All four TextMate grammars scope their keyword tier `keyword.control.<lang>`** — not preference: IntelliJ's built-in TextMate engine maps only a small scope-prefix table, and `storage.type`/`keyword.operator.*` rendered plain. Grammars live once in `editors/grammars/` with bidirectional drift guards.
- Both JetBrains plugins pin `jvmToolchain(17)`. Upstream limitation: JetBrains Cmd+hover may underline the whole file (LSP4IJ ignores `originSelectionRange` on TextMate-backed types).
- **Live editor verification is the maintainer's post-merge step** — sideload checklists ship unticked, and plugin READMEs state only verified facts.

### DAP (`core/src/dap/`, `*/src/dap/`)

`pmt dap` / `tmt dap` speak the Debug Adapter Protocol on stdio; transport, protocol types and the server loop are arch-agnostic in `core/src/dap/`, each arch crate supplying its adapter. Both VS Code extensions contribute the debugger type; both JetBrains plugins bridge through LSP4IJ's DAP module.

**Two independent axes decide what a session can show, and conflating them is the easy mistake.** *Source provenance* — a per-function record of the file its defining input was built from, written into the `.map` sidecar by `build` — decides whether a stack frame is **openable** (a frame without it still carries name/line/instruction pointer and works in the Disassembly view). *`-g`* decides only whether the per-function **line table** is populated: a linked executable names every function's address range either way, so a `-g`-less program still stops at function boundaries and `granularity: "instruction"` stays exact — clients stepping one should request that granularity explicitly. An executable with no sidecar at all degrades one step further: no address resolves, so even the function-boundary stop never fires. Full reference: `docs/dap.md`. The Disassembly view is unreachable in JetBrains on the LSP4IJ version the plugins pin — fixed upstream, pending a release; the plugin paragraph under Commands carries the detail and what to do when one ships.

## Testing conventions

- Integration tests live per crate under `tests/`; there is no shared test-support module — each file defines its own local helpers.
- **Goldens are derivation-first**: `golden_programs.rs` derives the expected `TapeSnapshot` in code, asserts the run matches the derivation, then asserts the committed `.pmt` is byte-identical to the derived snapshot. Never regenerate goldens from run output.
- `opt_equivalence.rs` runs each program at `-O0` and `-O1` on the same tapes and compares observables.
- Core's format round-trips and the operand codec are property-tested (`proptest`), including never-panics-on-noise cases.


**Standing regression gates.** These are contracts across rounds, not per-round checks:

- **PM-1 byte-identity.** TM-side work must not change a byte of PM-1 output. Enforced structurally rather than by one named test: PM's derivation-first goldens byte-compare the committed `.pmt`, and `asm_volatile.rs` pins directive-free files assembling byte-identically. Re-check deliberately on any change to `crates/core`.
- **`crates/core` neutrality.** The core crate carries zero PM-1/TM-1 knowledge and proves it by testing against a crate-private fake arch (`vm/arch.rs::test_arch`, arch id `0x7F`) and fake asm dialects. A TM-only round showing a `crates/core` diff is a design smell, not a convenience.
- **`-O0` bit-identity** and **the `brk` barrier**, per pass, on both toolchains (`opt_equivalence.rs::brk_barrier_blocks_elimination`, `volatile_equivalence.rs`).
- **The everything-matrix** (`turing-machine/tests/opt_equivalence.rs::everything_matrix_is_green`): `-O0`/`-O1` × `mono`/`frames`/`hybrid`, trap kinds included.
- **Compiled-stdlib byte-identity** — compile the embedded stdlib before and after at BOTH opt levels and byte-compare the object. It is the gate for a text-only change; for a FORMATTER change it is only a negative control, since the formatter is not in that path (see Lint, fmt, and the shared allow namespace).
- **Drift guards are set-compares in BOTH directions**: error-code registries against the docs inventory tables, `recognized_directives(caps)` against the docs, the completions registry against the real parser and `optimizer::pass_names()`, the TextMate grammars against the parsers, and `cli_docs` quoting `pmt --help`/`tmt --help` verbatim. Adding a rule, opcode or flag without updating its mirror fails the set-compare — that is the intent, not friction.
- **The text-expressibility gate**: everything the compiler can put in an object is expressible in hand-written assembly, proven by dis→asm byte-identity. One declared exception: `-g` debug side-tables. A feature that can only be emitted, never written by hand, breaks it.
- Temp paths in tests are named by PID plus a per-call atomic counter; keep new disk-writing tests collision-free under parallel runs.

## Commit style

Conventional commits with scope: `feat(cli):`, `fix(core):`, `test(post-machine):`, `docs(plan):`, `polish(post-machine):`.

## Version spaces and release notes

The repo carries several independently versioned contracts — the
toolchain crates, the `.pmc` language (`PMC_LANG_VERSION`, an acceptance
contract: pre-1.0 it is `0.N` and N bumps on ANY grammar change;
major/minor axes activate at a declared 1.0; no patch digit — errata and
implementation-conformance fixes never move it), the per-arch `.pma`
dialects (same kind of contract; PM-1's is `PM1_PMA_DIALECT_VERSION`,
born at 0.2 when labels tightened to dot-free), `IR_VERSION` (JSON
encoding), the container formats (MO/MX/MT), and each toolchain's
project-manifest schema — PM-1's `pmt.json` `project` section
(`docs/pmt/project.md`) and TM-1's `tmt.json` one
(`docs/tmt/project.md`) are each at **0.2**, with the lint-only shape
that predates them retroactively **0.1** (unenforced by any literal
version field in the files themselves; tracked here and in release
notes). The two manifest schemas are independent contracts that happen
to move together at 0.2 and already diverge there — `call-mech` and the
`.tmt`-tape-only run block exist only on the TM side. The toolchain
version is never the carrier for a language version.

Release notes open with a **version block** listing ALL of these spaces
explicitly, stating `unchanged` where nothing moved — the block doubles
as a compatibility matrix across releases. Component sections follow
only where changes exist. `CHANGELOG.md` (first entry: v0.2.0) uses this
structure in ref-free prose (published-docs policy); tracker links
belong in GH release notes.

Realized release flow (v0.2.0 precedent): docs audit first (per-page
claim verification + citation-keyword resolution); bump both crates,
both editor plugins, and their `MIN_TESTED_PMT` floors in one commit
with the CHANGELOG entry; merge, tag `vX.Y.Z`, `gh release create` with
the freshly built plugin artifacts attached.
