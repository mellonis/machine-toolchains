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
epoch. Each container dispatches on its own version field — MO reads
`1..=3`, MX and MT read `1..=2` today, selecting the layout from that field
and never from the extension. The containers are shared across present and future machine
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

An `.pmx` reader dispatches on the `u16 format version` field: **version 1**
is the code-only image PM-1 emits, **version 2** is a sectioned image that
adds a table section plus per-tape alphabet cardinalities and a processor
profile. The magic and `sniff()` are identical across both versions —
version selection is the header field alone, never the extension.

### Version 1 (code-only)

```
offset  size  field
0       3     magic "MX" 0x01
3       2     u16 format version (= 1)
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

### Version 2 (sectioned)

```
magic "MX" 0x01 (3) | u16 version = 2 | arch (1) | flags = 0 (1) | crc u32 (4)
tape_count u8 (1..=16) | profile u8 (0 = base, 1 = frames) | entry u32 | code_size u32 | table_size u32 | frames_offset u32
alphabet_cardinalities: tape_count × u32 | code bytes | table bytes
```

Version 2 carries everything version 1 does plus four additions: a
**table section** (`table_size` bytes after the code, holding the VM's
match/dispatch tables — its table ROM), one **u32 alphabet cardinality per
tape** (`tape_count` of them, `1..=16` tapes), a one-byte **processor
profile** (0 = base, 1 = frames), and a **`frames_offset` u32** naming where
the frames region begins inside the table section (0 for an image with no
frames region — see the frames region below). These fields are stored
verbatim; the format layer never interprets `arch`, `profile`, or the
cardinalities. A reader that finds a non-zero `frames_offset` checks the
whole declared region fits inside the table section before trusting it. A
version-1 reader still loads any PM-1 image, and a version-2 reader loads
both — the two shapes share magic and CRC discipline and differ only past
the version field.

### The frames region

The **frames region** is the runtime data that turns a shared framed-call
instruction into a per-context frame selection. It lives at `frames_offset`
inside the table section (never in a separate section — it references
descriptors already in the table ROM by offset). A base-profile image has
`frames_offset == 0` and no region; a frames-profile image always carries
one.

```
composite_count K   u16 LE      — directory size (distinct composites, 1..=K)
site_count      S   u16 LE      — compose-table columns (framed call sites)
directory       K × u32 LE      — descriptor offsets into the table section
compose         (K+1) × S × u16 LE
                                — row = active frame FR (0..=K), column = site index,
                                  entry = composite index (1..=K); 0 = reserved-invalid
```

Its three parts, in order:

| part | size | meaning |
|---|---|---|
| composite_count `K` | `u16 LE` | number of distinct composites — the **directory** length |
| site_count `S` | `u16 LE` | number of framed call sites — the **compose** table's column count |
| directory | `K × u32 LE` | one descriptor offset per composite (index `i` ⇒ `directory[i-1]`), pointing into the table section |
| compose | `(K+1) × S × u16 LE` | a matrix: `compose[FR][site]`, rows are active frames `0..=K`, columns are sites |

**How a framed call resolves.** The frame register `FR` is a **composite
index**: 0 is the identity context (the machine's own tapes, no
translation), and `1..=K` name directory entries. A framed call carries a
**site index** `S` (see the framed-call operand below), and at run time the
processor performs exactly one compose lookup and one directory read:

```
FR'         = compose[FR][site]        ; the composite active for the duration of the call
descriptor  = directory[FR' - 1]       ; its frame-descriptor offset in the table section
```

then loads that descriptor into the frame cache. On return, `ret`/`retx`
reload the caller's descriptor the same way (via the directory) when the
restored `FR` is non-zero. A compose entry of **0 is reserved-invalid** — a
reachable framed call never yields it (the linker enumerated every
reachable `(frame, site)` pair at link time), so reading a 0 at run time is
a malformed-operand trap, not a normal outcome. Every read is a fixed-size
frame-cache fill priced at call time; nothing is walked per tape access, so
the per-call cost is **O(1) at any call depth** — the frame stack exists
only to restore `FR` on return, never to translate.

## `.pmo` — object file

```
magic "MO" 0x01
u16 format version (readers accept 1..=3; writers emit
                OBJECT_FORMAT_VERSION_V2 = 2 unless v3 records are present,
                then OBJECT_FORMAT_VERSION_V3 = 3)
u8 arch
u8 flags (bit 0 = has debug section, bit 1 = has signatures,
                bit 2 = has table blobs)
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
── version 3 appends four trailing sections, in this order ──
signatures (present iff flags bit 1 is set), once per blob:
                u8 arity (1..=16), then arity × u32 alphabet cardinality
                (each >= 1)
table blobs (present iff flags bit 2 is set), once per blob:
                u32 length, table bytes
table fixups:   u32 count, then per fixup: u32 blob, u32 offset,
                u32 table offset (into that blob's own table blob)
bound calls:    u32 count, then per bound call: u32 blob, u32 offset,
                u32 symbol, u8 tape count, then per tape binding:
                u8 caller tape (< 16), u16 pair count, then per pair:
                u32 src, u32 dst, u8 flags (bit 0 = one-way)
```

Symbol kind 2 (**Local**) was added in object format version 2: a local
symbol is defined but not exported — bound directly within its own object,
invisible to cross-object resolution, so it can neither shadow nor be
shadowed (`docs/language.md (visibility)`, `docs/stdlib.md`). Version-1
object bytes (no locals) still decode under a later reader.

Object format version 3 was added for generic-routine composition: it
appends four record kinds — routine signatures, per-routine table blobs,
table fixups, and bound calls. An object carrying any of them serializes as
version 3; a plain PM-1 object, with none present, still serializes
byte-for-byte as version 2, and that is what the compiler and assembler
emit. A reader accepts 1..=3 and rejects a pre-version-3 object that sets
either version-3 flag bit. The signature and table-blob sections are gated
by flags bits 1 and 2; the table-fixup and bound-call sections are
unconditional — a version-3 object always writes both counts, zero when the
respective list is empty.

- **Routine signatures** state a generic routine's contract: the virtual
  tape arity — how many tapes the routine operates on, `1..=16` — and, per
  tape, the alphabet cardinality (how many glyphs that tape distinguishes,
  each `>= 1`). One signature per code blob, parallel to the blobs like the
  debug section.
- **Table blobs** hold a routine's own match/dispatch tables — the
  per-routine counterpart of the executable's table section — one blob per
  code blob.
- **Table fixups** are operand holes in a blob's `mtc`/`djmp` instructions:
  the u32 operand is an offset into that blob's own table blob, which the
  linker rebases into the final image's table section. The 4-byte hole obeys
  the same `offset..offset + 4` in-blob invariant as a call relocation.
- **Bound calls** are the declarative call sites of composed routines
  (`call name [binding]`): each marks a call operand hole, like a
  relocation, then binds every callee virtual tape — which caller tape feeds
  it and the symbol map between the two alphabets. A map pair flagged
  **one-way** is read-only: collapse is allowed and it is excluded from
  write-back.

The format layer validates **structure** only. It bounds-checks every
field — arity in `1..=16`, cardinality non-zero, `caller_tape` below 16,
every blob and symbol index in range, each hole's `offset..offset + 4`
inside its blob, each table offset inside its table blob — and rejects
reserved map-pair flag bits. Whether a binding's maps form the legal
bijection the composition demands — completion, hole rules, write-back
consistency — is **mapping legality**, checked by the linker, not the
format.

Per-function granularity is what gives the linker dead-function
elimination and leaves link-time inlining open as a future extension. A
"library" is simply a `.pmo` with many functions — only what `main`
transitively reaches gets linked in (`docs/stdlib.md`).

## `.pmt` — tape-block snapshot

Binary tape-block state — one or more tapes with their heads, usable as
`pmt run` input and output; golden tests diff final blocks as files.

An `.pmt` reader dispatches on the `u16 format version` field: **version 1**
carries a single shared block alphabet (what PM-1 emits), **version 2** lets
each tape carry its own glyph table. The magic and `sniff()` are identical
across both versions — version selection is the header field alone.

### Version 1 (shared alphabet)

```
offset  size  field
0       3     magic "MT" 0x01
3       2     u16 format version (= 1)
5       1     u8 flags (0; reserved)
6       4     u32 crc32
10      1     u8 alphabet count (non-zero)
—       —     per glyph: u16 length, UTF-8 bytes
—       1     u8 tape count (non-zero)
—       —     per tape: i64 origin, u32 length, u8 indices[length], i64 head
```

### Version 2 (per-tape glyph tables)

```
magic "MT" 0x01 (3) | u16 version = 2 | flags = 0 (1) | crc u32 (4)
block_alphabet: u8 count + per-glyph (u16 len + utf8)
tape_count u8
per tape: origin i64 | cells_len u32 | cells | head i64 | own_alphabet_count u8 | own_alphabet (u16 len + utf8) ×
```

Version 2 keeps the block alphabet as a shared fallback and appends an
optional glyph table to each tape. An `own_alphabet_count` of 0 means the
tape **inherits** the block alphabet — an empty per-tape override is treated
as inherit, not as a distinct empty alphabet. Cells are validated against
each tape's *effective* alphabet (its own table if present, otherwise the
block). A version-1 reader loads any PM-1 tape block, and a version-2 reader
loads both shapes.

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

The PM-1 `.pma` dialect version is **0.3** (pre-1.0: the version is `0.N`
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
- **0.3** — additive: the fused write+move mnemonics `wrl` and `wrr` join
  the mnemonic set (each takes a one-element symbol vector like `wr`). No
  existing program changes meaning; the accepted set only grew (`docs/isa.md`).

## `.tma` — assembly text (TM-1)

The TM-1 `.tma` dialect version is **0.2** (pre-1.0: the version is `0.N`
and `N` bumps on any grammar change — the same acceptance-contract shape
as the `.pma` dialect above). Where PM-1 drives one two-symbol tape, TM-1
drives up to sixteen tapes, each with its own alphabet, and branches
through match/dispatch tables rather than the mark register alone. The
dialect turns on three grammar features the classic `.pma` grammar leaves
off — a **tables** section, the `.rept` macro, and `[..]` **vector**
operands — plus a per-routine signature directive. Version 0.2 adds the
**frames** family: the framed call `call.m`, the multi-exit return
`retx`, the explicit `trap`, the `#imm` immediate operand, and the
`.frame`/`.map`/`.exits` frame-descriptor directives (and the declarative
binding-call operand). See *Frame descriptors* below.

```asm
.section tables
Tfetch: .row    [1, *, *, *]            ; match tape 0 == 1, others any
        .row    [8, *, *, *]
Dfetch: .targets L_step, L_halt         ; MR = 1 → L_step, MR = 2 → L_halt

.section code
.routine main, tapes=4, alpha=(9, 127, 127, 2)
.func main
L_step: rd                              ; latch every head into its slot
        mtc     Tfetch                  ; walk the table, set the match reg
        djmp    Dfetch                  ; dispatch on the match reg
L_halt: stp
```

One instruction (or one table directive) per line, `;` line comments, the
same **canonical column grid** as `.pma` (labels at column 0, mnemonics at
8, operands at 16, comments at 32); the parser accepts any whitespace on
input, and `pmt fmt` / `tmt dis` emit the grid. `tmt dis` output is always
valid assembler input and round-trips to the original bytes.

### Sections and the routine signature

A `.tma` file is split into two sections. `.section tables` holds the
match and dispatch tables; `.section code` holds the functions. The
default section is `code`, so a file may omit `.section code`. Only table
directives are legal in the tables section, and only functions/code in the
code section.

`.routine <name>, tapes=<N>, alpha=(<c1>, …, <cN>)` declares a function's
generic-routine signature: `tapes` is the tape count (1..=16), and `alpha`
lists one alphabet cardinality per tape (each at least 1; the list length
equals `tapes`). The directive must **precede** the `.func` it names, any
distance in the same file; it attaches when the function is defined. The
entry routine's signature fixes the executable image's tape count and
per-tape alphabets, which a run validates its tape band against.

### The mnemonic set

Nineteen mnemonics. The match register (**MR**) is written only by `mtc`;
the vector `rd`/`wr`/`mov` instructions leave it untouched, so `jm`/`jnm`
test the most recent `mtc` outcome regardless of intervening tape motion.

| opcode | mnemonic | operand | meaning |
|---|---|---|---|
| `0x01` | `nop` | — | no-op |
| `0x02` | `stp` | — | stop — clean termination (`tmt run` exits 0) |
| `0x03` | `hlt` | — | halt (`tmt run` exits 2) |
| `0x04` | `rd` | — | latch every tape head into its match slot in one fetch |
| `0x05` | `mtc` | table | walk a match table against the latched heads, setting MR |
| `0x06` | `djmp` | table | dispatch: jump through the table indexed by MR |
| `0x07` | `wr` | symbol vector | write one symbol per tape (`-` keeps a cell) |
| `0x08` | `jmp` | label | unconditional jump |
| `0x09` | `jm` | label | jump if the last `mtc` matched a row (MR ≠ 0) |
| `0x0A` | `jnm` | label | jump if the last `mtc` matched no row (MR = 0) |
| `0x0B` | `call` | symbol | call a routine; the width is chosen at link time |
| `0x0C` | `ret` | — | return from a call |
| `0x0D` | `ent` | — | entry marker — emitted implicitly by `.func`, the runtime call guard |
| `0x0E` | `brk` | — | debugger break |
| `0x0F` | `mov` | move vector | move one step per tape (`<` left, `>` right, `.` stay) |
| `0x11` | `trap` | `#kind` | raise a typed trap: `#0` unmapped-read, `#1` unmapped-write |
| `0x13` | `call.m` | frame call | framed call — call a routine and activate a frame for it |
| `0x14` | `retx` | `#k` | multi-exit return — leave the active frame through exit `k` |
| `0x1B` | `call.s` | label | the **short** form of `call` — see below |

The three frames instructions (`trap`, `call.m`, `retx`) and their
`#imm` / frame-call operands are detailed under *Framed calls, traps, and
multi-exit returns* and *Frame descriptors* below.

`call.s` exists in the mnemonic set for disassembly display and link-time
relaxation only: the assembler always emits far `call` and rejects
`call.s <target>` in source (the width is linker-selected). The linker's
relaxation fixpoint narrows a far `call` to `call.s` when the target is in
short range, exactly as PM-1 does.

### Vector operands

Three instructions and the `.row` directive take a bracketed vector with
**one element per tape**, left to right. The element vocabulary depends on
where the vector appears:

- **match rows** (`.row [..]`): a symbol index is an **exact** match on
  that tape's head; `*` is the wildcard ("any symbol").
- **write vectors** (`wr [..]`): a symbol index is written to that tape's
  cell; `-` **keeps** the cell untouched (no write on that tape).
- **move vectors** (`mov [..]`): `>` steps that head right, `<` left, `.`
  stays put.

### Match and dispatch tables

A **match table** is a labeled run of `.row` directives. Each row is one
vector; a run of rows under one label forms the table `mtc` walks. The
walk returns the 1-based index of the first row every tape matches — this
number is the match register (MR) — or 0 if no row matches. Table
discipline: all rows share one width (1..=16); **exact** rows (no
wildcard) come first, sorted and pairwise disjoint; **wildcard** rows
follow in source order; an **all-wildcard** catch-all, if present, is
last. A table with no catch-all leaves MR = 0 on an unmatched head, and a
following `djmp` on MR = 0 traps — a machine can get an invalid-symbol
fault for free by omitting the catch-all.

A **dispatch table** is a labeled run of `.targets`/`.target` directives:
`.targets L1, …, Lk` lists the targets indexed by MR (MR = 1 selects
`L1`, and so on), and `.target L` contributes a single target.
Consecutive directives under the **same label** accrue into one table, so
a wide dispatch table can be built one entry at a time — the idiom a
`.rept` uses to emit a value-indexed table.

### The compact symbol family (the `0x7F` rule)

TM-1 tables and vectors use the **compact** symbol family: one byte per
element, holding a 7-bit symbol index in `0`..=`126`. The value `0x7F` is
**reserved as the transparent marker** — a match-row byte of `0x7F`
matches any latched symbol (this is what `*` compiles to), and a write
element of `0x7F` keeps the cell (what `-` compiles to). Reserving `0x7F`
is why the compact family caps alphabets at **127 symbols**: every payload
index must stay at or below `0x7E`. An alphabet wider than 127 symbols
needs a wider symbol family, which the compact grammar does not yet reach.

### The `.rept` macro

`.rept <var>, <lo>, <hi>` … `.endr` expands its body textually, once per
integer `value` in `lo..=hi` (the GNU-as model): each body line is emitted
with its `{expr}` markers replaced by the evaluated integer. The
expression grammar is `+`, `-`, `*`, and `%` over the loop variable,
integer literals, and parentheses; arithmetic is signed, and overflow, a
zero modulus, or a negative remainder are errors rather than silent
wrap-around. Because expansion is textual, a `{expr}` may appear anywhere
on a line — inside a `[..]` vector element, in a label name, or in a
dispatch target — so `.rept` naturally emits value-indexed rows and
targets. A labeled directive inside a `.rept` emits the **same** label
every iteration; combined with same-label continuation, that is how a
`.rept` builds one wide match or dispatch table across its expansion.

### Framed calls, traps, and multi-exit returns

A **framed call** runs a routine under a **frame** — a projection that
narrows the machine's tapes to the callee's, remapping tape indices and,
per tape, the symbols read and written. Three mnemonics and one operand
form make up the surface:

- `call.m <target>, <frame>` — call `<target>` and activate `<frame>`
  (a `.frame` label in the tables section) for the duration of the call.
  Inside the callee, tape and symbol references are **virtual**: the frame
  translates them to physical tapes and symbols. The caller's frame is
  restored when the callee returns. The operand pairs a rel displacement
  (the call target) with a **site index** — a dense per-image number for
  this call site. The site index is *not* a descriptor offset: at run time
  the processor reads `compose[FR][site]` to pick the composite active in
  the current context, then `directory[…]` for its descriptor (the frames
  region above). A hand-authored `call.m` names one fixed descriptor, so its
  compose column is constant across every frame; a declarative binding call
  (below) may resolve to a different composite per calling context through
  the same instruction.
- `retx #k` — return through **exit `k`** of the active frame's exit
  vector. Unlike `ret`, the pushed return address is discarded; control
  resumes at the caller-side label recorded as exit `k`. A `k` past the
  exit vector traps (exit-out-of-range).
- `trap #kind` — raise a typed trap explicitly: `#0` is unmapped-read,
  `#1` is unmapped-write. Numeric kinds leave room for named kinds later
  without a grammar break.

`#<n>` is the **immediate** operand: a single unsigned byte, `0`..=`255`,
written with a leading `#` (`trap #0`, `retx #1`). It is distinct from a
symbol or a label — it carries a raw number.

### Frame descriptors

`.frame`/`.map`/`.exits` author a **frame descriptor**: the table-section
record a `call.m` activates. A descriptor projects the caller's tapes onto
a narrower callee and, per tape, remaps symbols in each direction.

```asm
.section tables
Fh: .frame  tapes=(2, 0)                 ; arity = list length; virtual k → physical tapes[k]
    .map    0, rmap=(1->1, 3=>0)         ; per virtual tape; at most once per k; omitted ⇒ identity
    .map    1, wmap=(2->1)
    .exits  done, alt                     ; optional, once; labels in the owning function
```

- `.frame <name> tapes=(<p0>, …, <pk>)` opens a labeled group. The list
  length is the **arity** (the callee's tape count, 1..=16); virtual tape
  `k` projects onto physical tape `<pk>`.
- `.map <k>[, rmap=(…)][, wmap=(…)]` continues the group, giving virtual
  tape `k`'s symbol maps (at most one `.map` per `k`; an omitted map is
  identity). `rmap` pairs are read maps written **physical→virtual**;
  `wmap` pairs are write maps written **virtual→physical**.
- `.exits <label>, …` (at most once) lists the exit vector — the
  caller-side labels `retx #k` returns to, in the function that names the
  frame via `call.m`.

**Arrows.** `->` is an ordinary map entry; `=>` marks the pair **one-way**
— read-direction only. `=>` is legal in `rmap` (the read side) and
**rejected in `wmap`** (the write side is never one-way). The one-way
spelling does not change the descriptor bytes — the wire form carries no
one-way flag — it only constrains where a pair may appear.

**Blank pinning.** Index 0 (the blank symbol) always maps to 0 and cannot
be re-pointed: a `0->X` pair with `X ≠ 0` is an error, in either map. A
non-blank symbol **may** fold onto blank, though: `Y->0` in `rmap` reads a
foreign boundary marker *as* the callee's blank (the canonical marker
collapse), and `Y->0` in `wmap` erases on write. Only index 0 itself is a
fixed point; whether a given fold is sound is the composition engine's
concern, not the raw authoring surface.

**Wire layout.** The descriptor is little-endian and self-describing:

| field | bytes | meaning |
|---|---|---|
| arity | `u8` | number of virtual tapes (`1`..=`16`) |
| exit_count | `u16` | number of exit-vector entries |
| *per virtual tape (× arity):* | | |
|  phys | `u8` | physical tape this virtual tape projects onto |
|  rmap_len | `u16` | read-map length (`0` = identity) |
|  rmap | rmap_len × `u16` | indexed by **physical** symbol → virtual symbol |
|  wmap_len | `u16` | write-map length (`0` = identity) |
|  wmap | wmap_len × `u16` | indexed by **virtual** symbol → physical symbol |
| exits | exit_count × `u32` | code offsets |

A map entry of `0xFFFF` is a **hole**: crossing it (reading through an
`rmap` hole, writing through a `wmap` hole) traps; a hole is never a
symbol. A `*_len` of `0` is the identity map. A dense map always pins
index 0 to 0. The **exits** are blob-relative code offsets in an object
file and absolute code addresses after link — the linker rebases them
through the owning function exactly as it rebases dispatch-table entries.

### Bound calls — the binding call operand

A **binding call** is the declarative, source-level way to spell a framed
call: instead of naming a hand-authored `.frame`, it lists the caller↔
callee tape binding inline, and the toolchain derives the frame. It
assembles to a **bound-call** record on the object.

```asm
        call    plusOne [2{1->3, 2=>0}, 0]
```

`call <target> [<entry>, …]` where `entry = <physIdx>` or
`<physIdx>{<src>-><dst>, <src>=><dst>, …}` — the list **position** is the
callee's virtual tape, `<physIdx>` is the caller's physical tape it binds
to, and each brace pair binds symbols. As in `.map`, `->` is a two-way
pair and `=>` a one-way (read-direction) pair; the one-way bit is recorded
as real data. In the example, callee virtual tape 0 binds caller physical
tape 2 (with a two-way `1↔3` and a one-way marker collapse `2⇒0`), and
callee virtual tape 1 binds physical tape 0 unchanged.

A binding call **assembles** — it is stored as a bound-call record on the
object, carrying the target and the binding — and is **lowered at link
time** by the composition engine (below). Alongside the hand-authored
`.frame` form, it is the source-level way to run a framed call.

### The frames profile and the composition engine

At link time a **composition engine** turns declarative binding calls into
concrete frames. It enumerates the finite set of `(routine, composite)`
pairs reachable from the entry — the same breadth-first walk as
reachability, now carrying an active composite that binding calls compose
onto — in a deterministic order, so builds are reproducible. Bindings
compose **associatively**; an identity composite collapses away
(`E ∘ identity = E`), so a binding that resolves to a full pass-through
lowers to a plain `call` (the callee inherits the caller's frame) and joins
the ordinary `call → call.s` relaxation. Hole sets compose too: the outer
holes union the preimages of the inner holes. A one-way `=>` pair
participates only in the read direction and is excluded from the
bidirectional bijectivity check.

How a lowered site runs depends on the **call mechanism**, chosen at link
time (`tmt link --call-mech mono | frames | hybrid`, default hybrid):

- **mono** compiles for the **base profile**: it stamps a specialized copy
  of the callee per distinct composite, folding the projection and symbol
  maps into the copy's vectors and match tables. A holey read keeps the
  trap taxonomy statically — an unmapped-read caller symbol becomes a
  first-match trap row prepended to every match table, and a write with no
  physical image becomes a `trap` stub outright. Identical stamps dedup.
  Mono emits **no frames region**.
- **frames** compiles for the **frames profile**: one generic copy of each
  routine, every binding site a `call.m`, and the composites resolved
  through the frames region's directory and compose table at run time. A
  crossed hole traps through the descriptor's `0xFFFF` sentinel.
- **hybrid** (the default) classifies **per site**: a completed bijection
  (equal-size, no holes, no one-way) stamps like mono; anything holey or
  one-way frames. An image with at least one framed site carries a frames
  region; an all-stamped hybrid image has none.

All three mechanisms are **observably equivalent** on the same program and
tapes — same outcome, same final tapes and heads, and the **same trap
kind** on a crossed hole or an unmatched read (the offset may differ; the
kind never does). Two restrictions apply to mono only: a raw
`.frame`/`call.m` cannot be lowered onto the base profile (it needs the
frames machinery), and a holey binding whose synthesized trap rows would be
read by a conditional branch rather than a dispatch jump is refused — the
prepended trap row could misroute. Both name the offending routine and
point at `--call-mech=frames` or `hybrid`.

**Link report.** `tmt link -v` renders the composition engine's counters
(the linker itself never prints — the CLI does):

| field | meaning |
|---|---|
| `dropped` | defined-but-unreachable functions, dropped from the image |
| `relaxed_calls` / `far_calls` | symbol sites narrowed to the short call form, or left far |
| `instantiations` | mono stamps emitted — one per distinct `(routine, composite)` (0 in frames mode) |
| `composites` | the directory size `K` — distinct composites in the frames region |
| `compose_table_bytes` | the compose matrix size, `(K+1) × S × 2` |
| `dedup_savings` | stamps and descriptors avoided by interning an already-built copy |
| `synthesized_trap_rows` | unmapped-read trap rows prepended to stamped match tables |
| `expanded_rows` | extra match rows from one-way collapse expansion (mono) |

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

### Sidecar bindings

A frames-profile image adds a `bindings` array — one record per composite in
the frames region's directory, in directory order — so a debugger or
disassembler can name a frame without decoding descriptor bytes. A frameless
link omits the key entirely (a pre-bindings sidecar still parses; the field
defaults empty):

```json
{
  "arch": 2,
  "functions": [ { "name": "main", "start": 0, "end": 10, "labels": [], "lines": [] } ],
  "bindings": [
    {
      "index": 1,
      "routine": "helper",
      "label": "helper@[2{1->3},0]",
      "tapes": [
        { "phys": 2, "pairs": [[1, 3, false]], "read_holes": [], "write_holes": [2] },
        { "phys": 0, "pairs": [], "read_holes": [], "write_holes": [] }
      ]
    }
  ]
}
```

Each record carries the runtime composite `index` (1-based, its directory
slot), the callee `routine`, the derived `label` (below), and the per-tape
**structured truth**: the physical tape `phys` this virtual tape projects
onto, the non-identity read `pairs` (each `[src, dst, one_way]` — identity
pairs are implicit), and the explicit `read_holes` / `write_holes`. Both
engine-synthesized composites and hand-authored `.frame` descriptors get a
record — the latter decoded from its bytes, the same shape either way.
Structure is truth; the label is derived from it.

### Binding labels

The human-readable label for a composite follows one canonical grammar,
shared by the sidecar's `label` field and the `tmt dis` frames legend so a
composite reads the same everywhere:

```
label = name "@[" entry ("," entry)* "]"       ; entries join with a bare comma
entry = physIdx [ "{" pairs "}" ]              ; list position = virtual tape
pairs = pair ("," pair)*                       ; decimal, sorted by src
pair  = src "->" dst | src "=>" dst            ; => marks a one-way (read-only) pair
```

- an equal-size (completed bijection) tape omits identity pairs and the
  empty `{}`;
- a holey (unequal-size) tape lists **all** mapped pairs — identity
  completion does not exist across differently-sized alphabets, so an absent
  src is a hole (no collision with the identity-omission rule);
- the blank pair `0->0` is never written;
- an entry with more than 8 displayed pairs collapses to a **digest**
  `{#xxxxxxxx}` — the CRC-32 (the container checksum) of the tape's completed
  dense maps, a content address matched like a short hash, never decoded;
- when two composites render the same label in one image, the second and
  later get a deterministic `.2`, `.3`, … suffix. The suffix is display-only;
  semantics always come from the structured record.

### Image-inspectability principle

A single contract governs the split between the image and its sidecar,
generalizing the `.pmx` code-image rule:

> Everything the machine executes is inspectable from the image alone; the
> sidecar adds names and provenance, never semantics.

Without a map, a frames-profile image still shows full index-level mappings
(descriptors are operational data in the table ROM) and even the label
digests (they are computable from descriptor content); a mono image is
concrete, self-contained code — a stripped binary with only names missing.
Glyph rendering is orthogonal: it comes from a supplied tape's glyph tables,
not from the map. The sidecar exists to attach names, never to carry
behavior the image lacks.

## IR JSON

`pmt compile --emit-ir` (`docs/language.md (the IR artifact)`) writes a
versioned JSON document: `IR_VERSION = 4`.

```json
{
  "version": 4,
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
`wr_lft` and `wr_rgt` (each carries `index`), `brk`, `call` (carries
`name`) — each also carries its source `line`. `wr_lft` / `wr_rgt` are the
fused write+move ops: a write to the pre-move cell, a head move, and an MF
latch in one instruction. They are optimizer-produced only (the fuse
tape-ops pass at `-O1`); lowering and `-O0` never emit them.
Per-terminator tags (`kind` field, snake_case): `fall_through` (`to`),
`goto` (`to`), `check` (`marked`, `blank`), `return`, `halt`, and
`tail_call` (`name`) — the last is optimizer-produced only (never emitted
by lowering) and replaces a trailing `call` + `return` with a direct jump
to the callee.
