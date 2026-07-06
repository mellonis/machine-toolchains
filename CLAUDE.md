# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

A Rust toolchain for a Post machine: C-like `.pmc` language → optimizing compiler → assembler → linker → bus-accurate VM, all driven by one CLI (`pmt`). GPL-3.0-or-later. It completes work spread across four Delphi implementations (2002–2012); `docs/history.md` has the lineage. A future Turing toolchain (`tmt`, arch TM-1) is expected to reuse the arch-agnostic core — `docs/examples/brainfuck-utm.tma` is a speculative TM-1 assembly file validating that design, not runnable code.

## Commands

```
cargo build --release                                   # produces target/release/pmt
cargo test --workspace                                  # everything: unit + integration + property tests
cargo clippy --workspace --all-targets -- -D warnings   # quality gate
cargo fmt --check                                       # quality gate
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

`pmt` exit codes from `run`: 0 = program stopped (`stp`), 2 = halted (`hlt`), 3 = trapped. Full flag reference: `docs/cli.md`.

## Documentation authority

`README.md` + `docs/` (language, isa, formats, cli, stdlib, history) are the durable references. The original design spec `docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md` is **FROZEN** — a historical record, no longer amended and no longer cited by code. Code comments cite the durable pages by page + parenthetical topic keyword, e.g. `docs/isa.md (timing model)`. Published content (README, `docs/`, code comments) is forge-agnostic: no issue/PR numbers, no hosting-provider URLs — describe substance in prose. Internal artifacts (`docs/superpowers/`, this file) are unrestricted.

## Architecture

Two-crate workspace with a hard boundary:

- **`crates/core` (`mtc-core`)** — arch-agnostic by contract: container formats (MO/MX/MT), the sans-I/O VM core + bus + driver + tape devices + `DebugSession`, the linker, and the assembler/disassembler frameworks. It carries **zero PM-1 knowledge**; its own tests run against a crate-private fake arch (`vm/arch.rs::test_arch`, arch id `0x7F`) to prove it.
- **`crates/post-machine` (`mtc-post-machine`)** — everything PM-1: the arch module, the `.pmc` compiler pipeline, the optimizer, the embedded stdlib, and the `pmt` binary.

Dependencies are deliberately minimal: `serde`/`serde_json` only, `proptest` as a dev-dep. **No clap** — CLI arg parsing is hand-rolled.

### Pipeline and key types

`.pmc` → `lexer.rs` (`Vec<Token>`) → `parser.rs` (recursive descent → `Program` AST) → `compiler.rs::compile(source, CompileOptions) -> CompileOutput` which internally runs duplicate-binding checks → flatten (name mangling + visibility) → `ir::lower` (`IrProgram`, a versioned per-function CFG) → `optimizer::optimize` (in-place) → `codegen::emit_program` (CFG → `.pma` text only) → core `asm::assemble` (`ObjectFile`). The IR is a **documented, versioned JSON artifact** (`IR_VERSION` in `ir.rs`), not an internal detail.

Then: core `linker::link(objects, libraries, LinkOptions) -> LinkOutput { executable, map, report }` → `vm::Machine::from_executable` → `run` / `DebugSession`.

### The arch contract

An architecture plugs into core through two tables, both living in the arch crate:

1. `Arch` trait (`core/src/vm/arch.rs`) — `operand_kind(opcode)` + `lower(opcode, operand) -> Vec<MicroOp>`: the VM core executes micro-ops and **knows no opcodes**.
2. `ArchSyntax` (`core/src/asm/mod.rs`) — mnemonic/relaxation tables for the assembler/disassembler. PM-1's is `pm1_syntax()` in `post-machine/src/asm/mod.rs`; short opcode = far `| 0x10`.

### VM model

`Core` (`vm/core.rs`) is a pure `BusResponse -> BusRequest` transition function — no I/O, no opcode knowledge. The synchronous `driver.rs` answers bus requests and does all tact accounting: fetch/execute cost **core tacts**; device move/read/write add **stall tacts** scaled by `TactProfile`. Traps are controlled stops (typed `Trap`), distinct from `stp`/`hlt`. Tape devices are index-based (the processor never sees glyphs): `InfiniteTape`, `AnnularTape`, and `StrictTape` (a decorator faulting on writing a cell's existing value — the historic 2006/2007 semantics).

### Optimizer (`post-machine/src/optimizer/`)

Eight passes, fixpoint-looped with a round cap: `inline` (program-level, runs first) then per-function `check_fold`, `jump_threading`, `cell_state`, `branch_fold`, `tail_call`, `tail_merge`, `dce`. Constraints that are contracts, not preferences:

- **Pass order**: `tail_call` must run before `tail_merge` (return-chaining destroys tail-call's precondition). Stated in `optimizer/mod.rs`.
- **MF-coupling soundness** (`optimizer/dataflow.rs`): after ≥1 tape op the match flag equals the cell at head; before any tape op it is the decoupled reset value. The `Uncoupled | Coupled(_)` lattice tracks this; check-edge refinement applies only on provably coupled paths.
- **-O0 bit-identity**: `-O0` output must stay byte-identical to plain codegen — no optimizer artifact may leak.
- **Equivalence contract** (enforced by `tests/opt_equivalence.rs`): passes preserve final tape, termination kind, and MF-dependent branches. Step counts and resource-limit outcomes may change — except across an un-stripped `brk`, which is an observability barrier no motion crosses.

### Formats (`core/src/formats/`)

Pure byte codecs, little-endian, no I/O. `.pmo`/MO (objects), `.pmx`/MX (pure code image — tape supplied at run time), `.pmt`/MT (tape snapshots; **glyphs live only here**). Containers are identified by `sniff()` on the magic — **never dispatch on file extensions**. Every reader verifies CRC-32 before decoding anything. Debug names live in the JSON `.pmx.map` sidecar, keeping `.pmx` a pure image.

### Linker (`core/src/linker/`)

Two-phase: `resolve` (namespace + BFS reachability from `main` — unreachable functions are dropped and may reference anything) then `layout` (relaxation: a monotone shrink-only fixpoint that narrows far calls to short; the assembler always emits far `call` — only the linker selects `call.s`). Libraries are first-wins and silently shadowed by user definitions.

### Stdlib (`post-machine/src/stdlib/`)

An embedded `.pmc` string (`include_str!("std.pmc")`, 11 exported `std::` routines), compiled once per process via `OnceLock` at `-O1` with debugger strips — embedded deliberately because a cargo-installed binary has no data directory. Linked lazily via the reachability pass; `--nostdlib` opts out.

### CLI (`post-machine/src/cli/`)

**Thin-renderer rule: library code never prints.** Every stage returns a structured report (`CompileReport`, `LinkReport`, `OptReport`, `RunResult`); every byte of terminal output originates in `cli/` (rendered under `-v`), and errors flow as typed values. `bin/pmt.rs` is a shell around `cli::execute`. Seven subcommands split across `build.rs` (compile/asm/link), `inspect.rs` (dis/tape/ir), `run.rs` (run, incl. live `--trace`).

## Testing conventions

- Integration tests live per crate under `tests/`; there is no shared test-support module — each file defines its own local helpers.
- **Goldens are derivation-first**: `golden_programs.rs` derives the expected `TapeSnapshot` in code, asserts the run matches the derivation, then asserts the committed `.pmt` is byte-identical to the derived snapshot. Never regenerate goldens from run output.
- `opt_equivalence.rs` runs each program at `-O0` and `-O1` on the same tapes and compares observables.
- Core's format round-trips and the operand codec are property-tested (`proptest`), including never-panics-on-noise cases.

## Commit style

Conventional commits with scope: `feat(cli):`, `fix(core):`, `test(post-machine):`, `docs(plan):`, `polish(post-machine):`.

## Version spaces and release notes

The repo carries several independently versioned contracts — the
toolchain crates, the `.pmc` language (`PMC_LANG_VERSION`, an acceptance
contract: pre-1.0 it is `0.N` and N bumps on ANY grammar change;
major/minor axes activate at a declared 1.0; no patch digit — errata and
implementation-conformance fixes never move it), the per-arch `.pma`
dialects (same kind of contract; PM-1's is implicitly 0.1 until its
first change introduces a constant), `IR_VERSION` (JSON encoding), and
the container formats (MO/MX/MT). The toolchain version is never the
carrier for a language version.

Release notes open with a **version block** listing ALL of these spaces
explicitly, stating `unchanged` where nothing moved — the block doubles
as a compatibility matrix across releases. Component sections follow
only where changes exist. A future `CHANGELOG.md` uses the same
structure in ref-free prose (published-docs policy); tracker links
belong in GH release notes.
