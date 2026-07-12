# File formats

All multi-byte integers are little-endian. This page covers the four
binary/text containers (`.pmo`, `.pmx`, `.pmt`, `.pma`), the `.pmx.map`
sidecar, and the IR JSON artifact. PM-1's opcode semantics are
`docs/isa.md`; the `pmt` subcommands that read and write these files are
`docs/cli.md`.

## Shared conventions

Magics are toolchain-neutral: two ASCII letters plus a binary epoch byte —
`MO 0x01` object, `MX 0x01` executable, `MT 0x01` tape-block. The epoch
byte marks header-layout generations and doubles as a text-file guard; a
`u16 format version` field inside each header covers evolution within an
epoch. The containers are shared across present and future machine
toolchains built on this codebase: the file *extension* carries the
toolchain flavor (`.pmo`/`.pmx`/`.pmt` from `pmt`), while the magic plus an
`arch` byte identify the actual content. Tools never dispatch on file
extensions — only on the sniffed magic.

**CRC-32** (IEEE 802.3, reflected, polynomial `0xEDB88320`) covers the
whole file with the 4-byte CRC field itself zeroed. Writers zero the field,
compute the CRC over the whole buffer, and stamp it in last; every reader
(loader, linker, disassembler) verifies the CRC before decoding anything
else — a mismatch is a clean "corrupt file" error, never a trap mid-run.

`sniff(bytes)` identifies a container from its first 3 bytes
(`ContainerKind::Object` / `Executable` / `TapeBlock`), used by `pmt dis` to
accept either a `.pmo` or a `.pmx` on the same command line.

## `.pmx` — executable

```
offset  size  field
0       3     magic "MX" 0x01
3       2     u16 format version (FORMAT_VERSION = 1)
5       1     u8 arch (0x01 = PM-1)
6       1     u8 flags (0; reserved)
7       4     u32 crc32
11      4     u32 entry offset
15      4     u32 code size
19      —     code bytes
```

The initial tape contents are **not** embedded in a `.pmx` — they are
supplied to the VM at run time (`pmt run app.pmx --tape "..*..***" --head 2`,
or a loaded `.pmt`, or via the API directly). `entry offset` is validated to
be inside the code section, and the loader additionally checks that byte is
`ent` before running (`docs/isa.md`). The linker guarantees the
**`.pmx entry`** symbol is literally `main`, which is what lets a bare
executable's disassembly name the entry root `main`.

## `.pmo` — object file

```
magic "MO" 0x01
u16 format version (OBJECT_FORMAT_VERSION = 2; readers accept 1..=2)
u8 arch
u8 flags (bit 0 = has debug section)
u32 crc32
string table:   u32 count, then per string: u16 length, UTF-8 bytes
symbol table:   u32 count, then per symbol: u32 name (string index),
                u8 kind (0 = external, 1 = defined, 2 = local),
                u32 blob index (defined/local) or 0xFFFFFFFF (external)
code blobs:     u32 count, then per blob: u32 length, code bytes
                (one blob per defined/local function; intra-function jumps
                already resolved; every blob starts with ent)
relocations:    u32 count, then per relocation: u32 blob, u32 offset,
                u32 symbol (one relocation per call site; each hole is a
                4-byte placeholder, the operand of a far call instruction
                at offset - 1)
debug section (present iff flags bit 0 is set), once per blob:
                u32 label count, then per label: u32 name (string index),
                u32 code offset
                u32 line count, then per line: u32 code offset, u32 source line
```

Symbol kind 2 (**Local**) was added in object format version 2: a local
symbol is defined but not exported — bound directly within its own object,
invisible to cross-object resolution, so it can neither shadow nor be
shadowed (`docs/language.md (visibility)`, `docs/stdlib.md`). Version-1
object bytes (no locals) still decode under a version-2 reader.

Per-function granularity is what gives the linker dead-function
elimination and leaves link-time inlining open as a future extension. A
"library" is simply a `.pmo` with many functions — only what `main`
transitively reaches gets linked in (`docs/stdlib.md`).

## `.pmt` — tape-block snapshot

Binary tape-block state — one or more tapes with their heads, usable as
`pmt run` input and output; golden tests diff final blocks as files.

```
offset  size  field
0       3     magic "MT" 0x01
3       2     u16 format version (FORMAT_VERSION = 1)
5       1     u8 flags (0; reserved)
6       4     u32 crc32
10      1     u8 alphabet count (non-zero)
—       —     per glyph: u16 length, UTF-8 bytes
—       1     u8 tape count (non-zero)
—       —     per tape: i64 origin, u32 length, u8 indices[length], i64 head
```

The alphabet travels WITH the tape data — a `.pmt` renders using its own
glyphs (index 0 is blank by convention). **Glyphs live ONLY on the tape
side.** A tape block's alphabet is the authoritative rendering source; with
no tape block at hand, tooling falls back to the architecture module's
default glyphs (PM-1: `" "` for blank, `"*"` for mark — the PM-1 arch
module's `DEFAULT_GLYPHS` constant). Code-side artifacts — `.pmo`, `.pmx`, and the
`.pmx.map` sidecar — carry symbol indices only, never glyphs, matching the
hardware-realizability rule that the processor never sees glyphs
(`docs/isa.md`).

CLI: `pmt tape build " * * *" --head 3 -o in.pmt`, `pmt tape show in.pmt`,
`pmt run app.pmx --tape-block in.pmt [--save-tape-block out.pmt]`
(`docs/cli.md`).

## `.pma` — assembly text

The PM-1 `.pma` dialect version is **0.2** (pre-1.0: the version is `0.N`
and `N` bumps on any grammar change, the same acceptance-contract shape as
the `.pmc` language version in `docs/language.md`). See "Dialect version
history" below for what each version changed.

```asm
.func goToEnd                   ; emits ent, defines symbol
L1:     rgt
        jm      L1              ; assembler picks jm.s automatically
        lft
        ret

.func main
        call    goToEnd         ; width decided at link time
        rgt
        wr      1               ; mark
        stp
```

One instruction per line, `;` line comments. The **canonical column grid**
emitted by `pmt compile -S` and `pmt dis` (and produced by `grid_line`):
labels at column 0, mnemonics at column 8, operands at column 16, trailing
comments at column 32, trailing spaces trimmed; the assembler's parser
itself accepts any whitespace on input. A label field of 8 characters or
more (the name plus its `:`) moves to its own line rather than sharing
the instruction's line, so a long label never pushes the mnemonic column
out of alignment; a field of 7 characters or fewer stays inline. `pmt dis`
output is always valid assembler input — round-tripping through `asm`
reproduces the original bytes exactly. `pmt fmt` (`docs/cli.md`) is the
tool that enforces this grid on hand-written `.pma` source — `pmt compile
-S` and `pmt dis` already emit it directly, so formatting their output is
always a no-op.

`pmt dis` accepts either binary. From a `.pmo`: real names come from the
symbol table, code is shown per function, and call sites are named from
relocations. From a `.pmx`: names come from the `-g` sidecar map when one
is present (`FILE.pmx.map` beside the executable, or `--map`); otherwise
they are synthesized via **recursive-descent discovery** — a worklist walk
from the entry point following control-flow edges; every verified `call`
target is a function root (exact in v1, which has no indirect control
flow). Discovered roots are named `main` (the entry) or `func_XXXX`;
internal jump targets are named `LXXXX`; bytes never reached by the walk
print as `.byte` directives, one per byte. The `ent` byte remains the
runtime call guard, but function discovery itself comes from control flow,
not byte scanning — an operand byte that happens to equal the entry opcode
is never mistaken for a function start.

**Symbol jumps (tail calls):** `jmp @name` takes a function symbol, not a
label — in an object it assembles as a far `jmp` plus a relocation (the
same hole-and-relocation mechanism as `call`), and relaxes to `jmp.s` at
link time exactly like a `call`. `jmp.s @name` is a syntax error (width is
linker-selected, like `call.s`), and conditional `jm @name`/`jnm @name` are
errors — v1 branches take labels only. Disassemblers print a relocated jump
(from an object, via its relocation table) or a jump landing on a function
root (from an executable, via discovery) in the `jmp @name` form; a jump
into another function's middle that lands on no known root falls back to
`.byte`.

**Visibility and names:** `.func name local` declares an unexported
(local) function; plain `.func name` exports. Symbol names — in `.func`
lines and in jump/call operands — accept `::`-separated segments of
dotted identifiers (`std::api.helper`: the namespace part is everything
before the LAST `::`, the function-nesting part is everything after;
`docs/language.md (symbol grammar)`). **Labels are letters, digits, and
underscores only** — Unicode letters are legal (matching identifiers
elsewhere in the toolchain), but the label grammar does not accept `::`
or `.`, which is what lets the parser tell a label (`L1:`) apart from a
namespaced/nested symbol reference without ambiguity.

### Dialect version history

- **0.1** — the v1 toolchain's dialect; the retroactive baseline the
  version scheme measures from.
- **0.2** — one tightening: label names dropped `.` and `::` from their
  accepted characters, leaving letters, digits, and underscores (Unicode
  letters still legal). Symbol names in `.func` and jump/call operands are
  unaffected — the dotted/`::`-segmented grammar above still applies to
  them.

## `.pmx.map` — link-time sidecar

Written next to a `.pmx` by `pmt link` as `<output>.pmx.map`: a JSON
document with the architecture byte and, per linked function, its absolute
code range, label offsets, and source line map (the label/line data is
empty unless the linked objects carried `-g` debug info):

```json
{
  "arch": 1,
  "functions": [
    { "name": "main", "start": 0, "end": 18,
      "labels": [], "lines": [] }
  ]
}
```

The `.pmx` itself stays a pure code image — all naming and debug
correlation lives in this sidecar (see `docs/cli.md` for sidecar discovery
rules: an explicit `--map` wins over the `FILE.pmx.map` beside the
executable, and a missing or unparsable sidecar is silently ignored by
plain `dis`/`run`, but an unparsable *explicit* `--map` is an error).

## IR JSON

`pmt compile --emit-ir` (`docs/language.md (the IR artifact)`) writes a
versioned JSON document: `IR_VERSION = 3`.

```json
{
  "version": 3,
  "functions": [
    {
      "name": "goToEnd",
      "line": 1,
      "blocks": [
        {
          "id": 0,
          "labels": [1],
          "line": 1,
          "ops": [{ "op": "rgt", "line": 1 }],
          "term": { "kind": "check", "marked": 0, "blank": 1 },
          "term_line": 1
        }
      ],
      "local": false
    }
  ]
}
```

Per-op tags (`op` field, snake_case): `lft`, `rgt`, `wr` (carries `index`),
`brk`, `call` (carries `name`) — each also carries its source `line`.
Per-terminator tags (`kind` field, snake_case): `fall_through` (`to`),
`goto` (`to`), `check` (`marked`, `blank`), `return`, `halt`, and
`tail_call` (`name`) — the last is optimizer-produced only (never emitted
by lowering) and replaces a trailing `call` + `return` with a direct jump
to the callee.
