# machine-toolchains

A Rust toolchain family for tape machines. Two architectures are built on
one arch-agnostic core, each with its own source language, optimizing
compiler, assembler/disassembler, linker, and bus-accurate processor (VM),
and each driven by its own CLI:

- the **Post machine PM-1** — a C-like source language (`.pmc`) compiled and
  linked down to a single-tape, index-based processor, driven by `pmt`;
- the **multi-tape Turing machine TM-1** — a source language (`.tmc`) with
  worlds, grafts, and link-time composition, compiled through a `.tma`
  assembly stage and linked to a multi-tape processor, driven by `tmt`.

The two share `crates/core`: the sans-I/O VM and its buses, the tape
devices, the container formats, the linker, and the assembler/disassembler
frameworks are arch-agnostic by contract — neither machine's specifics leak
into the core, which is proven against a small fake test architecture.

The two toolchains do not share a past. The Post machine finishes work
started across four Delphi implementations built between 2002 and 2012 — a
language without a code generator, a code generator without that language,
and a machine without a compiler — and adds the piece none of them
attempted: a linker, with separate compilation and libraries. It still
carries the two 2002-era programs (`Sum.pms`/`Ty.pms`) as golden tests.
TM-1 has no Delphi ancestor: its lineage runs through a JavaScript library
generation and through one problem the 2007 Delphi work identified and left
unsolved. See `docs/history.md` for both.

Each CLI also runs a Language Server Protocol server on stdio — `pmt lsp`
for `.pmc` and `.pma`, `tmt lsp` for `.tmc` and `.tma` — wired into any
LSP-capable editor, and backed by the same compiler, assembler, and linter
the CLI uses. The ready-made editor integrations live under `editors/`; see
`docs/lsp.md` for capabilities and wiring. Each CLI also runs a Debug
Adapter Protocol server on stdio — `pmt dap`, `tmt dap` — debugging a
built executable at source or instruction granularity with tapes and
registers in the variables view; see `docs/dap.md` for the launch
schema and protocol surface.

## Build

```
cargo build --release
```

Produces two binaries at `target/release/`: `pmt` (PM-1) and `tmt` (TM-1).

## Quick start

### The Post machine (`pmt`)

`sum.pmc` ports the historic `Sum.pms` program — unary addition, where a
number n is written as n+1 marks and the input is two marked sections
separated by a single blank, head on the first mark:

```
// Port of the historic Sum.pms (Delphi generation A, Compiller/):
// adds the two unary numbers on the tape. Numbers are n+1 marks; input
// "a gap b" with the head on a's first mark; output one section a+b.
use std::goToEnd, std::goToBegin;

main() {
     1: @goToEnd();
     2: right;
     3: right;
     4: @goToEnd();
     5: unmark;
     6: left;
     7: @goToBegin();
     8: left;
     9: mark;
    10: @goToEnd();
    11: unmark;
    12: left;
    13: @goToBegin(!);
}
```

The five commands below compile, link, build an input tape, run, and
disassemble it. They write `.pmo`, `.pmx`, `.pmx.map`, and `.pmt` files
into the current directory — consider running them from a scratch
directory. From the repository root, with `pmt` built as above:

```
$ target/release/pmt compile crates/post-machine/tests/golden/sum.pmc -o sum.pmo
$ target/release/pmt link sum.pmo -o sum.pmx
$ target/release/pmt tape-block build "*** **" -o sum.pmt
$ target/release/pmt run sum.pmx --tape-block sum.pmt
outcome: Stopped
steps 53, core tacts 142, stall tacts 50 (total 192)
origin 0, head 0 reads '*'
|****|
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
render their stage reports; `tape-block build`/`run --tape-block` build and
consume `.pmt` snapshots; `dis` shows the linked `.pmx` with real function
names resolved from the `.pmx.map` sidecar that `link` wrote alongside it.
Full flag reference: `docs/pmt/cli.md`.

That `dis` output is canonical assembly — valid `asm` input that re-links
to this same image — so it prints the form the assembler accepts rather
than the one the linker picked: every `call` above is a *short* call in
the image, because a symbol site's width belongs to the linker
(`docs/core.md (relaxation)`). `--listing` is the byte-exact view, one
row per instruction with its address and raw bytes:

```
$ target/release/pmt dis --listing sum.pmx | head -4
main:
  0000:  0D              ent
  0001:  1B 14           call.s  0x0017 <std::goToEnd>
  0003:  05              rgt
```

The five commands above are what `pmt build` collapses into one step once
a project has more than a one-off source file. A `pmt.json` at the
repository root (alongside the `sum.pmt`/`sum.pmx` the walkthrough above
wrote there), declaring one target:

```json
{
  "project": {
    "targets": {
      "sum": {
        "sources": ["crates/post-machine/tests/golden/sum.pmc"],
        "run": { "tape-block": "sum.pmt" }
      }
    }
  }
}
```

turns `pmt build --run sum` into that same compile-link-run sequence
against the `sum.pmt` snapshot built above. Manifest reference (schema,
profiles, targets, run settings): `docs/pmt/project.md`.

### The Turing machine (`tmt`)

`a1_replace_b.tmc` mirrors that pipeline for a `.tmc` program: a single-tape
machine that walks right, rewrites every `b` to `a`, and stops at the first
blank.

```
? Walk right; replace every 'b' with 'a'; stop at the first blank.

alphabet ab { '_', 'a', 'b' }

machine {
  tape main: ab;

  entry state scan {
    ['b'] -> write ['a'] move [>] goto scan;
    ['a'] ->             move [>] goto scan;
    ['_'] -> stop;
  }
}
```

One difference from `pmt` shows up in the tape. TM-1's processor is
index-based — a cell holds a symbol *index*, never a glyph — and a linked
image records only how many symbols each tape has, not their source names.
Glyphs live on the tape side alone, so where a tape block gets its labels
depends on what you build it from. Point `tmt tape-block new` at the
**source** and it reads the real glyphs, and the tape names, straight out of
the `machine` block — so the program below is fed `'a','b','b','a'` and
prints `aaaa` back. Point it at the **image** instead and all it can know is
each tape's cardinality, so bands are labelled `0` through `card-1`; `tmt
tape-block set --alphabet` repins them afterwards, relabelling without moving
a cell.

```
$ target/release/tmt compile crates/turing-machine/tests/golden/a1_replace_b.tmc -o replace.tmo
$ target/release/tmt link replace.tmo -o replace.tmx
$ target/release/tmt tape-block new --from crates/turing-machine/tests/golden/a1_replace_b.tmc \
    --cells "0='a','b','b','a'" -o replace.tmt
$ target/release/tmt run replace.tmx --tape-block replace.tmt
outcome: Stopped
steps 24, core tacts 113, stall tacts 67 (total 180)
tape 0: origin 0, head 4 reads '_'
|aaaa_|
$ target/release/tmt dis replace.tmx
.routine main, tapes=1, alpha=(3)
.section tables
T0:     .row    [0]
        .row    [1]
        .row    [2]

T1:     .targets L001C, L0014, L000C
.section code
.func main
L0001:  rd
        mtc     T0
        djmp    T1
L000C:  wrmv    [1], [>]
        jmp     L0001
L0014:  wrmv    [-], [>]
        jmp     L0001
L001C:  stp
```

The run rewrites `1221` (`abba`) to `1111` (`aaaa`) and halts on the
trailing blank at head 4; `dis` then shows the linked `.tmx` — the TM-1
instruction stream (`rd` reads every head, `mtc`/`djmp` match and dispatch,
`wrmv` writes and moves) and the match/dispatch tables that drive it — with
a `.tmx.map` sidecar written alongside, exactly as `pmt` does. Beyond this
single-tape sketch, `tmt` adds what TM-1 needs over the same containers:
multi-tape snapshots, the `.tma` assembly stage, and a linker with a
link-time composition engine selected by `--call-mech`. Both CLIs share
their exit codes (`0` stopped, `2` halted, `3` trapped). Full flag
reference: `docs/tmt/cli.md`.

`--listing` is the byte-exact view here too — one row per instruction,
its address and raw bytes, with the canonical view's table labels shown
as the raw addresses they encode:

```
$ target/release/tmt dis --listing replace.tmx | head -6
main:
  0000:  0D              ent
  0001:  04              rd
  0002:  05 00 00 00 00  mtc     0x0000
  0007:  06 06 00 00 00  djmp    0x0006
  000c:  12 81 82        wrmv    [1], [>]
```

`tmt build` is the same compile-link driver `pmt build` is, over its own
`tmt.json`. A manifest declaring one target — its sources, its `.tmt`
tape, and optionally the `call-mech` it commits to — turns the sequence
above into `tmt build --run <target>`. Manifest reference:
`docs/tmt/project.md`.

## Documentation

The two toolchains are documented per domain, over a set of shared pages
that cover what they hold in common.

**The Post machine** — `docs/pmt/`:

- `docs/pmt/language.md` — the `.pmc` source language: structure,
  statements, visibility/namespaces/imports, doc lines and attention lines,
  optimization, the IR artifact, and the grammar-version history.
- `docs/pmt/isa.md` — the PM-1 processor: registers, the opcode table,
  timing, and execution.
- `docs/pmt/asm.md` — the `.pma` assembly dialect: the sample shape, the
  `pmt dis` round trip, symbol jumps, the `.volatile` build-column
  directive, and the dialect-version history.
- `docs/pmt/cli.md` — every `pmt` subcommand and flag.
- `docs/pmt/optimizer.md` — the `-O1` pipeline: the nine passes, the
  contracts binding them, and a worked before/after IR example per pass.
- `docs/pmt/project.md` — the `pmt.json` project manifest: schema,
  per-section discovery, targets, profiles, and run settings, and how
  `pmt build` consumes it.
- `docs/pmt/lint.md` — hygiene findings over `.pmc` and `.pma` sources via
  `pmt lint`, with `--fix`.
- `docs/pmt/fmt.md` — the canonical `.pmc`/`.pma` layout via `pmt fmt`, with
  `--check` and stdin.
- `docs/pmt/stdlib.md` — the standard library's routine roster and linking
  semantics.

**The Turing machine** — `docs/tmt/`:

- `docs/tmt/language.md` — the `.tmc` source language: worlds, rules,
  alphabets and tapes, `call`/`graft`/`bind`, symbol maps, range expansion,
  doc and attention lines, and the grammar-version contract.
- `docs/tmt/isa.md` — the TM-1 processor: the opcode set, multi-tape
  vectors, match/dispatch tables, the frames execution profile, framed
  calls, traps, and the three call mechanisms.
- `docs/tmt/asm.md` — the `.tma` assembly dialect: the sample shape, the
  `tmt dis` round trip, how the twenty mnemonics are spelled, what the
  `.tmc` compiler emits, and the dialect-version history.
- `docs/tmt/cli.md` — every `tmt` subcommand and flag.
- `docs/tmt/optimizer.md` — the `-O1` pipeline: the eight passes, the
  contracts binding them, and a worked before/after graph example per
  pass.
- `docs/tmt/project.md` — the `tmt.json` project manifest: schema,
  per-section discovery, targets, profiles, the bound-call lowering, and
  run settings, and how `tmt build` consumes it.
- `docs/tmt/lint.md` — hygiene findings over `.tmc` and `.tma` sources via
  `tmt lint`.
- `docs/tmt/fmt.md` — the canonical `.tmc`/`.tma` layout via `tmt fmt`.
- `docs/tmt/stdlib.md` — the binary-number standard library twins and how
  the delimited representation is composed over the bare one.

**Shared:**

- `docs/core.md` — the arch-agnostic core: the sans-I/O VM and its buses,
  tape devices, loading, the trap taxonomy, `DebugSession`, the composition
  engine, and the assembler, lint, and linker frameworks.
- `docs/formats.md` — the container formats — `.pmo`/`.tmo`, `.pmx`/`.tmx`,
  `.pmt`/`.tmt`, the assembly text grammar the two dialects share, the
  `.map` sidecar, and IR JSON — with the sniff-not-extension rule shared by
  both toolchains.
- `docs/lsp.md` — the language-server framework and its per-language
  services: capabilities, editor wiring, and configuration.
- `docs/dap.md` — the debug-adapter framework and its two adapters:
  launch config, the protocol surface, breakpoints and stepping, the
  writable-state contract, and degradation without `-g`.
- `docs/history.md` — where both designs come from.

The full design behind the Post-machine half was written up as a spec, now
frozen as a historical record and no longer the authority code cites or docs
are derived from day to day; `docs/history.md` explains the handover.

## Workspace layout

A three-crate Cargo workspace:

- `crates/core` (library) — the VM core and buses, tape devices, the
  `MO`/`MX`/`MT` container formats, the linker (including the link-time
  composition engine), the assembler/disassembler frameworks, and the
  language-agnostic LSP server framework. Arch-agnostic by contract: it
  carries no PM-1 or TM-1 knowledge, and its own tests run against a small
  fake test architecture to prove that.
- `crates/post-machine` (library + the `pmt` binary) — the PM-1
  architecture module, the `.pmc` compiler and optimizer, the standard
  library, the lint/fmt/completions/language-server surface, and the `pmt`
  CLI.
- `crates/turing-machine` (library + the `tmt` binary) — the TM-1
  architecture module, the `.tmc` compiler and optimizer, the `.tma`
  assembly dialect, the standard library, the lint/fmt/completions/
  language-server surface, and the `tmt` CLI.
- `editors/` — ready-made editor integrations built on the language
  servers: a VS Code extension and a JetBrains/LSP4IJ plugin for each
  toolchain, with their shared TextMate grammars in `editors/grammars/`.
  All are sideload-only, with their own README-documented install and build
  steps.

Both CLIs are thin renderers: library code never prints, so every stage in
the two library crates is usable directly from Rust (or, one day, from
another front end) without going through a subprocess.

## Tests

```
cargo test --workspace
```

runs everything: unit tests co-located in all three crates, integration
tests under each crate's `tests/` directory (format/relaxation round-trips,
compiler/assembler/linker end-to-end programs, golden ports of the historic
`Sum.pms`/`Ty.pms` and the TM-1 language's worked examples, optimizer
equivalence checks, lint and fmt rule coverage, and the editor-facing
surfaces — completions and grammar), and property tests for container
round-trips. `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo fmt --check` are the other two quality gates this workspace holds
itself to.
