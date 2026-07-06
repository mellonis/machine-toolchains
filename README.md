# machine-toolchains

A Rust toolchain for a Post machine: a C-like source language (`.pmc`), an
optimizing compiler, an assembler/disassembler, a linker, and a
bus-accurate bytecode processor (VM), all driven by one CLI, `pmt`. It
finishes work started across four Delphi implementations of a Post machine
built between 2002 and 2012 — a language without a code generator, a code
generator without that language, and a machine without a compiler — and
adds the piece none of them attempted: a linker, with separate compilation
and libraries. See `docs/history.md` for the full lineage, including the
two 2002-era programs (`Sum.pms`/`Ty.pms`) this project still carries as
golden tests.

## Build

```
cargo build --release
```

Produces the `pmt` binary at `target/release/pmt`.

## Quick start

The five commands below compile, link, build an input tape, run, and
disassemble a port of the historic `Sum.pms` program (unary addition: two
marked sections separated by one blank cell, each of length n+1 for the
number n it represents). The commands write `.pmo`, `.pmx`, `.pmx.map`,
and `.pmt` files into the current directory — consider running them from a
scratch directory. From the repository root, with `pmt` built as above:

```
$ target/release/pmt compile crates/post-machine/tests/golden/sum.pmc -o sum.pmo
$ target/release/pmt link sum.pmo -o sum.pmx
$ target/release/pmt tape build "*** **" -o sum.pmt
$ target/release/pmt run sum.pmx --tape-block sum.pmt
outcome: Stopped
steps 53, core tacts 142, stall tacts 50 (total 192)
origin 0, head 0
|****|
 ^
$ target/release/pmt dis sum.pmx
.func main
        call    std::goToEnd
        rgt
        rgt
        call    std::goToEnd
        wr      0
        lft
        call    std::goToBegin
        lft
        wr      1
        call    std::goToEnd
        wr      0
        lft
        call    std::goToBegin
        stp
.func std::goToEnd
L0018:  rgt
        jm.s    L0018
        lft
        ret
.func std::goToBegin
L001E:  lft
        jm.s    L001E
        rgt
        ret
```

The input tape reads `*** **` — three marks (representing 2), a blank, two
marks (representing 1) — and `run` reports the final tape `****`: four
marks, representing 3 = 2 + 1. `compile`/`link` (`-o`) both accept `-v` to
render their stage reports; `tape build`/`run --tape-block` build and
consume `.pmt` snapshots; `dis` shows the linked `.pmx` with real function
names resolved from the `.pmx.map` sidecar that `link` wrote alongside it.
Full flag reference: `docs/cli.md`.

## Documentation

- `docs/language.md` — the `.pmc` source language: structure, statements,
  visibility/namespaces/imports, optimization, and the IR artifact.
- `docs/isa.md` — the PM-1 processor: registers, buses, the opcode table,
  timing, execution, and the debug API.
- `docs/formats.md` — the binary/text container formats: `.pmo`, `.pmx`,
  `.pmt`, `.pma`, the `.pmx.map` sidecar, and IR JSON.
- `docs/cli.md` — every `pmt` subcommand and flag.
- `docs/lint.md` — hygiene findings over `.pmc` sources via `pmt lint`,
  with `--fix`.
- `docs/stdlib.md` — the standard library's routine roster and linking
  semantics.
- `docs/history.md` — where this design comes from.

The original design document,
`docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md`, is
frozen as a historical record — see its banner and `docs/history.md` for
why.

## Workspace layout

A two-crate Cargo workspace:

- `crates/core` (library) — the VM core and buses, tape devices, the
  `MO`/`MX`/`MT` container formats, the linker, and the
  assembler/disassembler frameworks. Arch-agnostic by contract: it carries
  no PM-1-specific knowledge, and its own tests run against a small fake
  test architecture to prove that.
- `crates/post-machine` (library + the `pmt` binary) — the PM-1
  architecture module, the `.pmc` compiler and optimizer, the standard
  library, and the `pmt` CLI itself. The CLI is a thin renderer: library
  code never prints, so every stage above is usable directly from Rust
  (or, one day, from another front end) without going through a
  subprocess.

## Tests

```
cargo test --workspace
```

runs everything: unit tests co-located in both crates, integration tests
under each crate's `tests/` directory (format/relaxation round-trips,
compiler and linker end-to-end programs, golden ports of the historic
`Sum.pms`/`Ty.pms`, optimizer equivalence checks), and property tests for
container round-trips. `cargo clippy --workspace --all-targets -- -D
warnings` and `cargo fmt --check` are the other two quality gates this
workspace holds itself to.
