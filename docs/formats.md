# File formats

All multi-byte integers are little-endian. This page covers the three
binary container formats — objects, executables and tape blocks — the
assembly text grammar the two dialects share, the link-time map sidecar,
and the IR JSON
artifact. It is a **wire-format** page: what the bytes and the text mean,
never what the machine does with them. Opcode and execution semantics are
`docs/pmt/isa.md` for PM-1 and `docs/tmt/isa.md` for TM-1; the parts of the
virtual machine, assembler and linker that neither architecture owns are
`docs/core.md`.

Both toolchains write the same three containers under their own
extensions, and each has its own assembly dialect:

| | objects | executables | tape blocks | assembly text |
|---|---|---|---|---|
| `pmt` (PM-1) | `.pmo` | `.pmx` (+ `.pmx.map`) | `.pmt` | `.pma` |
| `tmt` (TM-1) | `.tmo` | `.tmx` (+ `.tmx.map`) | `.tmt` | `.tma` |

The `pmt` subcommands that read and write these files are
`docs/pmt/cli.md`; the `tmt` ones are `docs/tmt/cli.md`. One section below
describes each container once, for both toolchains; where their contents
differ, the difference is called out in that section rather than split into
a second one. The two assembly dialects, being different languages, are
documented per toolchain — `docs/pmt/asm.md` for `.pma` and
`docs/tmt/asm.md` for `.tma` — over the one shared section below that
holds the grammar and the byte layouts they have in common.

## Shared conventions

Magics are toolchain-neutral: two ASCII letters plus a binary epoch byte —
`MO 0x01` object, `MX 0x01` executable, `MT 0x01` tape-block. The epoch
byte marks header-layout generations and doubles as a text-file guard; a
`u16 format version` field inside each header covers evolution within an
epoch. Each container dispatches on its own version field — MO reads
`1..=3`, MX and MT read `1..=2` today, selecting the layout from that field
and never from the extension. The containers are shared across the machine
toolchains built on this codebase: the file *extension* carries the
toolchain flavor — `.pmo`/`.pmx`/`.pmt` from `pmt`, `.tmo`/`.tmx`/`.tmt`
from `tmt` — while the magic plus an `arch` byte identify the actual
content. A `.pmo` and a `.tmo` are both `MO 0x01`; a `.pmx` and a `.tmx` are
both `MX 0x01`; a `.pmt` and a `.tmt` are both `MT 0x01`. **Neither
toolchain ever dispatches on a file extension** — only on the sniffed
magic, so a container renamed to the wrong extension still reads correctly,
and one handed to the wrong subcommand is rejected for what it *is*. That
is a claim about the three binary containers specifically: the
source-text commands (`lint`, `fmt`) route `.pmc`/`.pma`/`.tmc`/`.tma`
input by file extension on purpose, since source text carries no
sniffable magic of its own.

**CRC-32** (IEEE 802.3, reflected, polynomial `0xEDB88320`) covers the
whole file with the 4-byte CRC field itself zeroed. Writers zero the field,
compute the CRC over the whole buffer, and stamp it in last; every reader
(loader, linker, disassembler) verifies the CRC before decoding anything
else — a mismatch is a clean "corrupt file" error, never a trap mid-run.

`sniff(bytes)` identifies a container from its first 3 bytes
(`ContainerKind::Object` / `Executable` / `TapeBlock`). It is what lets
`pmt dis` and `tmt dis` accept either an object or an executable on the same
command line, and what turns a mistaken argument into a diagnosis rather
than a decode failure — `tmt dis` handed a tape block answers "that is a
tape block — use `tmt tape-block show`".

The `arch` byte says which architecture the content is for, and every
subcommand that reads it refuses content it cannot handle rather than
guess: a loader refuses an image it cannot execute — `pmt run` on a
TM-1 executable reports `unknown architecture 0x02` — and `dis` refuses
it the same way, for both objects and executables, before disassembling
a single instruction: `dis` is an arch-agnostic framework driven by the
mnemonic table its own tool supplies, so reading the other toolchain's content
against that table would print something well-formed but meaningless.
Read an image or object with the `dis` of the toolchain that wrote it.

## `.pmx` / `.tmx` — executable

A `.pmx` is an **executable image**: the linker's output, a pure code
image with the tape supplied separately at run time.

An `.pmx` reader dispatches on the `u16 format version` field: **version 1**
is the code-only image, **version 2** is a sectioned image adding a table
section plus per-tape alphabet cardinalities and a processor profile. The
linker picks by **content, not by toolchain**: the sectioned shape is
emitted when the reached set carries table content or a routine signature,
and the code-only shape otherwise. In practice that means every `pmt link`
output is version 1 — PM-1's dialect has neither tables nor signatures —
and every `tmt link` output is version 2, since a TM-1 program's entry
carries a `.routine` signature naming its tape count and alphabets.

The magic and `sniff()` are identical across both versions — version
selection is the header field alone, never the extension.

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
supplied to the VM at run time (`pmt run app.pmx --tape-cells "  *  ***" --head 2`,
or a loaded `.pmt`, or via the API directly). `entry offset` is validated to
be inside the code section, and the loader additionally checks that byte is
`ent` before running (`docs/pmt/isa.md`). The linker guarantees the
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

**How the fields index each other.** The frame register `FR` is a
**composite index**: 0 is the identity context, and `1..=K` name directory
entries. A framed call carries a **site index** (the `call.m` operand,
`docs/tmt/asm.md (the mnemonic set)`), and the two tables resolve it in
one lookup each:

```
FR'         = compose[FR][site]        ; the composite active for the duration of the call
descriptor  = directory[FR' - 1]       ; its frame-descriptor offset in the table section
```

A compose entry of **0 is reserved-invalid** — a reachable framed call
never yields it (the linker enumerated every reachable `(frame, site)`
pair at link time), so reading a 0 at run time is a malformed-operand
trap, not a normal outcome. What the processor then does with the
descriptor, and what it costs, is `docs/tmt/isa.md (framed calls)`.

## `.pmo` / `.tmo` — object file

**MO** is the object container both toolchains emit: relocatable code plus
the symbol, signature, table, and binding records the linker consumes. A
`.pmo` and a `.tmo` are the same `MO` format at different arch bytes.

```
magic "MO" 0x01
u16 format version (readers accept 1..=3; writers emit
                OBJECT_FORMAT_VERSION_V2 = 2 unless v3 records are present,
                then OBJECT_FORMAT_VERSION_V3 = 3)
u8 arch
u8 flags (bit 0 = has debug section, bit 1 = has signatures,
                bit 2 = has table blobs, bit 3 = has variant tags,
                bit 4 = the program is a volatile build)
u32 crc32
string table:   u32 count, then per string: u16 length, UTF-8 bytes
symbol table:   u32 count, then per symbol: u32 name (string index),
                u8 kind (0 = external, 1 = defined, 2 = local),
                u32 blob index (defined/local) or 0xFFFFFFFF (external)
code blobs:     u32 count, then per blob: u32 length, code bytes
                (one blob per defined/local function — two where a
                function ships both build columns, see variant tags
                below; intra-function jumps already resolved; every blob
                starts with ent)
relocations:    u32 count, then per relocation: u32 blob, u32 offset,
                u32 symbol (one relocation per call site; each hole is a
                4-byte placeholder, the operand of a far call instruction
                at offset - 1)
debug section (present iff flags bit 0 is set), once per blob:
                u32 label count, then per label: u32 name (string index),
                u32 code offset
                u32 line count, then per line: u32 code offset, u32 source line
── version 3 appends five trailing sections, in this order ──
signatures (present iff flags bit 1 is set), once per blob:
                u8 arity (1..=16), then arity × u32 alphabet cardinality
                (each >= 1)
table blobs (present iff flags bit 2 is set), once per blob:
                u32 length, table bytes
variant tags (present iff flags bit 3 is set):
                u32 count (= the blob count), then per blob: u8 tag
                (0 = normal, 1 = volatile, 2 = both)
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
shadowed (`docs/pmt/language.md (visibility)`, `docs/pmt/stdlib.md`). Version-1
object bytes (no locals) still decode under a later reader.

Object format version 3 was added for generic-routine composition, with
four record kinds — routine signatures, per-routine table blobs, table
fixups, and bound calls. Build columns added a fifth kind later, the
per-blob variant tags, alongside a program-volatile header bit; being
later in time says nothing about where it sits on the wire, and the
layout above places it third of the five. An
object carrying any of them serializes as version 3; an object with none
present still serializes byte-for-byte as version 2. In practice
`tmt compile` emits version 3, since every routine it generates carries a
signature, and so does `pmt compile`, since every `.pmc` compilation
records build columns — even a program whose two columns are identical
throughout tags each blob `both`. The version-2 shape is what an
assembler still produces from text that asks for none of the version-3
records: a `.pma` file with no `.volatile` directive, or a `.tma` file
with no `.routine` signature, table section, or bound call. A reader
accepts 1..=3 and rejects a pre-version-3 object that sets any
version-3 flag bit. The signature, table-blob, and variant-tag
sections are gated by flags bits 1, 2, and 3; the table-fixup and
bound-call sections are unconditional — a version-3 object always writes
both counts, zero when the respective list is empty.

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
  it and the symbol map between the two alphabets. Placement is
  **injective**: two callee tapes may never name the same caller tape, so
  a callee can never declare more tapes than its caller has to bind them
  to. A map pair flagged **one-way** is read-only: collapse is allowed
  and it is excluded from write-back.
- **Variant tags** name each blob's **build column** — one `u8` per blob,
  parallel to the blobs like the debug section, and carrying its own
  explicit count so a length mismatch is a decode-time rejection rather
  than a debug-build assertion. A PM-1 compilation builds every function
  both ways (`docs/pmt/language.md (volatile programs)`), so a name whose
  two builds differ contributes two adjacent blobs tagged `normal` and
  `volatile`, and a name whose builds came out identical contributes one
  blob tagged `both`, serving either program kind.

**The program-volatile bit** (flags bit 4) is not a section: it records
that this object's program is a volatile build, which the linker reads
off the object defining the entry symbol to choose a column for every
name (`docs/core.md (linking)`). The bit is deliberately **independent**
of the tag section. An object may set it while carrying no tags at all —
that is exactly what a hand-assembled volatile program looks like
(`.volatile` before the first `.func`, no per-function directive) — and
such a link is legal: every reached name then offers only the normal
column and every one of them is counted as a fallback, which is the
intended signal rather than a degenerate case.

The **legacy rule is typed, not inferred**: an object with no variant
section at all — one written before variant tags existed, one produced
by `pmt asm` from directive-free text, one from an architecture with no
build columns — reads as all-`normal`. It offers exactly one column, so
a volatile program linking it takes that counted fallback instead of an
error. The absence is stored as an absence (no stand-in vector of
`normal` tags), which is what lets an object of the older shape keep its
version-2 bytes.

Compatibility is worth stating plainly: an object carrying variant
records is MO version 3, and every `.pmo` `pmt compile` writes now
carries them. Readers check the version field before decoding anything,
so one that predates version 3 rejects such an object rather than
misreading it — but it does reject it.

The format layer validates **structure** only. It bounds-checks every
field — arity in `1..=16`, cardinality non-zero, `caller_tape` below 16,
every blob and symbol index in range, each hole's `offset..offset + 4`
inside its blob, each table offset inside its table blob — and rejects
reserved map-pair flag bits. Whether a binding's maps form the legal
bijection the composition demands — completion, hole rules, write-back
consistency, and injective placement (the `caller_tape` fields of one
binding must be pairwise distinct) — is **mapping legality**, checked by
the linker, not the format.

Per-function granularity is what gives the linker dead-function
elimination and leaves link-time inlining open as a future extension. A
"library" is simply an object with many functions — only what the entry
transitively reaches gets linked in (`docs/pmt/stdlib.md`,
`docs/tmt/stdlib.md`).

## `.pmt` / `.tmt` — tape-block snapshot

Binary tape-block state — one or more tapes with their heads, usable as
`pmt run` / `tmt run` input and output; golden tests diff final blocks as
files.

A reader dispatches on the `u16 format version` field: **version 1** carries
a single shared block alphabet (what `pmt` emits, PM-1 being single-tape and
single-alphabet), **version 2** lets each tape carry its own glyph table
(what `tmt` emits, a TM-1 program's tapes commonly differing in alphabet). The magic and `sniff()` are identical
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
no tape block at hand, tooling falls back to whatever default the
architecture can supply. PM-1 has a fixed pair — `" "` for blank, `"*"` for
mark — because its alphabet is fixed at two symbols. TM-1 has no fixed
alphabet and therefore no fixed glyphs, so what a band is labelled with
depends on where the block came from. From an **executable**,
`tmt tape-block new --from app.tmx` has only the per-tape cardinalities in
the image header to go on, and labels each band's symbols with the decimal
strings `0`…`card-1`. From **source**, `tmt tape-block new --from app.tmc`
reads the real glyphs — and the tape names — out of the `machine` block's
tape declarations. Either way `tmt tape-block set --alphabet` repins a band's
glyph table afterwards, relabelling without moving a cell.

Code-side artifacts — objects, executables, and the map sidecar — carry
symbol indices only, never glyphs, matching the hardware-realizability rule
that the processor never sees glyphs (`docs/pmt/isa.md`). Tape **names**
follow the same rule and are never stored: the bus addresses bands by
number, so a name given on the command line is resolved to an index at parse
time and discarded.

CLI: `pmt tape-block build " * * *" --head 3 -o in.pmt`,
`pmt tape-block show in.pmt`,
`pmt run app.pmx --tape-block in.pmt [--save-tape-block out.pmt]`
(`docs/pmt/cli.md`). The TM-1 side starts from the program rather than from a
literal, since the tape count and each tape's cardinality are properties of
it — and one invocation authors the whole band:
`tmt tape-block new --from app.tmc --cells "main='s','b','1'" -o in.tmt`,
then `tmt tape-block show in.tmt`, and
`tmt run app.tmx --tape-block in.tmt [--save-tape-block out.tmt]`
(`docs/tmt/cli.md`).

## Assembly text

Both toolchains assemble from a line-oriented text dialect: PM-1's `.pma`
and TM-1's `.tma`. Each dialect's own surface — its sample shape, its
mnemonic spellings, the directives only it has, and its version
history — is its toolchain's page: `docs/pmt/asm.md` and
`docs/tmt/asm.md`. This section is what the two share: the lexical shape
and canonical layout every dialect prints in, the grammar extensions the
assembler framework offers behind capabilities a dialect opts into
(`docs/core.md (the assembler framework)`), and the bytes that text
lowers to. A dialect that leaves a capability off does not accept the
directives riding it: the classic `.pma` grammar enables none of the
sectioned, vector, and macro surface below, and `.tma` is today the only
dialect that enables all of it.

One instruction — or one table directive — per line, `;` line comments.
The **canonical column grid** — labels at column 0, mnemonics at column
8, operands at column 16, trailing spaces trimmed — is what `pmt fmt`
and `tmt fmt` (`docs/pmt/cli.md`, `docs/tmt/cli.md`) enforce on
hand-written source, and what `compile -S` and `dis` emit directly; the
assembler's parser itself accepts any whitespace on input.

A trailing comment's column is not fixed at 32; it aligns per **group**
at `max(32, widest code width in the group + 1)`, where a line's code
width is its character count up to the comment, trailing whitespace
trimmed. 32 is a floor, so a group only ever widens past it, never below
it — which is what keeps output unchanged for a group whose members all
fit under it already. A group is the maximal run of lines that share one
comment column; it ends at a blank line, an own-line comment printed at
column 0 (below), a `.rept` block, or any structural directive — the
formatter treats `.section`, `.func`, `.routine`, and `.volatile` alike
there, as block structure rather than as grid lines. A line with no
trailing comment still belongs to its
group but contributes no width to it, and a `.rept` block's body prints
verbatim rather than through this grid at all, so it contributes no
width either. The column is not capped by the 80-column limit: a group
can widen a member past it, and the result is reported like any other
overlong line (`line-too-long`, `docs/core.md (assembly lint)`).

An own-line comment — one with no code on its own physical line — prints
in one of two columns. If it continues a run started by a trailing
comment on the line directly above it, with no blank line between, it
prints at that group's comment column, staying visually part of the same
comment block. Everything else — a preamble comment, one between
functions, one opening a body, or any comment that does not continue a
trailing one — prints at column 0. Column 8, the mnemonic column, is
never a comment position: it is where statements live, and a comment is
not a statement.

**Visibility and names:** `.func name local` declares an unexported
(local) function; plain `.func name` exports. Symbol names — in `.func`
lines and in jump/call operands — accept `::`-separated segments of
dotted identifiers (`std::api.helper`: the namespace part is everything
before the LAST `::`, the function-nesting part is everything after;
`docs/pmt/language.md (symbol grammar)`). **Labels are letters, digits, and
underscores only** — Unicode letters are legal (matching identifiers
elsewhere in the toolchain), but the label grammar does not accept `::`
or `.`, which is what lets the parser tell a label (`L1:`) apart from a
namespaced/nested symbol reference without ambiguity.

### Sections and the routine signature

A file in a dialect with the tables capability is split into two
sections. `.section tables` holds the match tables, the dispatch tables,
and the frame descriptors; `.section code` holds the functions. The
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

Disassembling a linked image recovers a non-entry callee's signature
only when it is reached through a `.frame` descriptor: `tapes` comes
from the descriptor's own virtual tape count, but the routine's true
per-tape alphabet is consumed by the composition engine at link time
and does not survive into the image, so `alpha` there is the
**physical** tape each virtual one projects onto instead — a
`; derived` trailing comment marks the line to say so.

### Vector operands

Under the vectors capability, the `.row` directive and any instruction
whose operand kind asks for one take a bracketed vector with **one
element per tape**, left to right. Which mnemonics those are is each
dialect's own business — TM-1 spells them `wr`, `mov`, and `wrmv`
(`docs/tmt/asm.md (the mnemonic set)`), and the examples below use that
spelling. The element vocabulary depends on where the vector appears:

- **match rows** (`.row [..]`): a symbol index is an **exact** match on
  that tape's head; `*` is the wildcard ("any symbol").
- **write vectors** (`wr [..]`): a symbol index is written to that tape's
  cell; `-` **keeps** the cell untouched (no write on that tape).
- **move vectors** (`mov [..]`): `>` steps that head right, `<` left, `.`
  stays put.

A fused write+move operand (`wrmv [w…], [m…]`) takes **two** vectors — a
write vector then a move vector, comma-separated — fusing a rule's whole
write+move action into one instruction. Its execution order is the
architecture's to define; TM-1's is `docs/tmt/isa.md (reading, writing
and moving)`. A hand-written write/move pair remains equally valid; the
fused form is a spelling, not a new capability.

### Match and dispatch tables

A **match table** is a labeled run of `.row` directives. Each row is one
vector; a run of rows under one label forms the table the architecture's
match instruction walks (TM-1's `mtc`).

A **dispatch table** is a labeled run of `.targets`/`.target` directives:
`.targets L1, …, Lk` lists the targets indexed by MR (MR = 1 selects
`L1`, and so on), and `.target L` contributes a single target.
Consecutive directives under the **same label** accrue into one table, so
a wide dispatch table can be built one entry at a time — the idiom a
`.rept` uses to emit a value-indexed table. That is a *directive*-level
continuation (several `.targets`/`.target` lines under one label); a
single directive's own operand list has a separate, *list*-level one,
described next.

`.targets`, `.exits` (below), and `.map` (below) are the grammar's three
unbounded lists: a `.targets`/`.exits`/`.map` line ending in a bare
trailing comma — nothing after it but whitespace, no comment — continues
that directive's list onto the next physical line, so a wide table can be
authored (or emitted) across several lines instead of one long one. A
comma followed by a comment does not continue (only the list's *last*
physical line may carry a trailing comment); a trailing comma on any
other directive stays the syntax error it has always been. The
canonical-grid printer wraps the other direction — a list whose
single-line form would cross the 80-column line limit is broken after a
comma, with continuation lines aligned under the list's first element
(`docs/tmt/fmt.md` shows it on `.tma`, the one shipped dialect that has
these lists) — and the two meet: a wrapped line always ends in the
trailing comma the continuation grammar reads back into one logical
directive, so reformatting a wide table is idempotent. Every other
list-shaped operand — a `.row`/`wr`/`mov`/`wrmv` vector, a `.frame
tapes=(…)` list — is bounded by the tape count (`1..=16`) and never
continues or wraps; only the three lists above can grow past one line.

Match tables carry a **row discipline** the assembler checks, reporting a
violation as a fatal error under the code `table-discipline`. The rules
and what they buy are `docs/core.md (match tables)`; the TM-1 error
spellings are `docs/tmt/isa.md (match and dispatch)`.

### The compact symbol family (the `0x7F` rule)

Table rows and vector operands use the **compact** symbol family: one
byte per element, holding a 7-bit symbol index in `0`..=`126`. The value
`0x7F` is **reserved as the transparent marker** — a match-row byte of
`0x7F` matches any latched symbol (this is what `*` compiles to), and a
write element of `0x7F` keeps the cell (what `-` compiles to). Reserving
`0x7F` is why a compact operand can **name** only indices `0`..=`126`:
every payload index must stay at or below `0x7E`, and a write or match
element outside that range is a fatal `bad-vector` error.

This is a limit on what an instruction can *mention*, not on how wide a
tape's alphabet may be. A `.routine` may declare a cardinality above 127
— such an image assembles, links, mints a tape and runs — the symbols
past index 126 simply have no compact spelling, so no `wr` or `.row` can
name them. (The `.tmc` front end is stricter: it rejects an alphabet
resolving to more than 127 symbols, since a compiled program must be able
to name every symbol it declares.)

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

### Frame descriptors

`.frame`/`.map`/`.exits` author a **frame descriptor**: the table-section
record a `call.m` activates. A descriptor projects the caller's tapes onto
a narrower callee and, per tape, remaps symbols in each direction — what
the processor does with it while the callee runs is `docs/tmt/isa.md (the
frames execution profile)`.

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

`.map`'s wrapping is coarser than `.targets`/`.exits`: the break falls
**between** a group's `.map` clauses (`<k>`, `rmap=(…)`, `wmap=(…)`), not
inside a clause's own `->`/`=>` pair list. Unlike a tape-count-bounded
list, a single `rmap=(…)` or `wmap=(…)` clause scales with its tape's
alphabet cardinality (up to 127 compact symbols) rather than with the
tape count — so a clause wider than the line limit on its own is an
unsplittable atom and still prints as one over-80 line; wrapping gets the
group under budget only when the width comes from having many clauses,
not from one very wide one.

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
time** by the composition engine (`docs/core.md (the composition
engine)`; what the three mechanisms produce for a TM-1 image is
`docs/tmt/isa.md (call mechanisms)`). Alongside the hand-authored
`.frame` form, it is the source-level way to run a framed call.

**Completing a binding.** How a binding's symbol maps complete depends on
the two tapes' sizes.
**Equal-size** alphabets identity-complete: a source symbol the binding does
not name maps to itself (the completed bijection). Across
**differently-sized** alphabets there is no identity completion — the map is
**closed**: every non-blank source symbol the binding does not name is a
hole. Read holes are caller symbols with no read pair; write holes are callee
symbols with no bidirectional pair writing back. So an unequal binding must
list every pair it wants mapped: an unlisted symbol traps even when its index
falls inside the other alphabet, and an *empty* unequal binding holes
everything but the blank (blank↔blank is always implicit). A one-way `=>`
pair, being read-only, establishes no write-back, so on an unequal tape the
symbol it collapses onto is a write hole unless a two-way pair also names it.

## `.pma` — assembly text

The PM-1 dialect's own surface — its sample shape, the `pmt dis` round
trip, symbol jumps, and the `.volatile` build-column directive — is
`docs/pmt/asm.md`. It enables none of the capability-gated subsections of
"Assembly text", above — its one capability adds the `.volatile`
directive, documented on that same page — so what it draws from that
section is the opening tier: the lexical shape, the canonical column
grid, the comment-column rules, and the `.func` visibility and name
grammar.

## `.tma` — assembly text (TM-1)

The TM-1 dialect's own surface — its sample shape, the `tmt dis` round
trip, how its twenty mnemonics are spelled, and what the `.tmc` compiler
emits — is `docs/tmt/asm.md`. The grammar it sits in is "Assembly text",
above — the opening tier every dialect draws on, plus every
capability-gated subsection there, since `.tma` is currently the one
dialect enabling them all.

## `.pmx.map` / `.tmx.map` — link-time sidecar

Written next to the executable by the linker — `<output>.pmx.map` from
`pmt link`, `<output>.tmx.map` from `tmt link` — as a JSON document with
the architecture byte and, per linked function, its absolute code range,
label offsets, and source line map (the label/line data is empty unless the
linked objects carried `-g` debug info):

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
correlation lives in this sidecar (see `docs/pmt/cli.md` for sidecar discovery
rules: an explicit `--map` wins over the `FILE.pmx.map` beside the
executable, and a missing or unparsable sidecar is silently ignored by
plain `dis`/`run`, but an unparsable *explicit* `--map` is an error).

### Source provenance

A function record may additionally carry a `source` key naming the file
its defining input was built from:

```json
{ "name": "main", "start": 0, "end": 18,
  "labels": [], "lines": [], "source": "../src/main.pmc" }
```

The field is optional both ways: a record without it parses (so every
pre-provenance sidecar stays readable), and a function whose file is
unknown simply omits it. Only the build drivers can know it — `pmt
build`/`tmt build` compile and assemble their inputs in-process, so each
compiled (`.pmc`/`.tmc`) and assembled (`.pma`/`.tma`) unit stamps its
functions with its own path, while a prebuilt object input, a linked
library, and the embedded stdlib carry none. Plain `pmt link`/`tmt link`
over object files never writes the field: an object records no source
path, and a `.pmo`/`.tmo` is not a file a debugger could open. A
specialized mono-stamp copy inherits the provenance of the routine it
specializes.

The stored path is relative to the sidecar's **own directory** whenever
the two share a root (falling back to absolute otherwise), so a build
tree can be moved or archived wholesale and the correlation survives.
Both the emission and a consumer's resolution back to an absolute file
are purely lexical — paths are joined and `.`/`..`-folded as strings,
never resolved through the filesystem — the same identity policy the LSP
cross-file overlay documents in `docs/lsp.md`, with the same caveat: a
symlinked tree can present one file under two spellings. The debug
adapters are the primary consumer (`docs/dap.md`).

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

Each compiler can write its intermediate representation as a **versioned,
documented JSON document** rather than keep it an internal detail. The two
are different artifacts with independent version counters, because the two
languages lower to different shapes: `.pmc` is imperative and lowers to a
per-function control-flow graph of basic blocks; `.tmc` is a set of
transition rules and lowers to a per-world state graph. Neither reader
accepts the other's document.

### The `.pmc` CFG IR

`pmt compile --emit-ir` (`docs/pmt/language.md (the IR artifact)`) writes a
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

### The `.tmc` state-graph IR

`tmt compile --emit-ir` writes the state-graph IR: `TM_IR_VERSION = 3`. The
form follows the model — a Turing world is a set of states, each a
priority-ordered list of classical match rows, so the document is a graph of
states rather than a CFG of basic blocks. `tmt ir graph` renders one of its
worlds as a diagram (`docs/tmt/cli.md`).

```json
{
  "version": 3,
  "worlds": [
    {
      "name": "main",
      "kind": "machine",
      "arity": 1,
      "tapes": [{ "name": "main", "alphabet": "ab", "cardinality": 3 }],
      "entry": 0,
      "states": [
        {
          "id": 0,
          "name": "scan",
          "line": 8,
          "rules": [
            {
              "pattern": [{ "kind": "index", "index": 2 }],
              "write": [{ "kind": "index", "index": 1 }],
              "moves": ["right"],
              "transition": { "kind": "goto", "state": 0 },
              "line": 9
            }
          ],
          "dispatch": "table"
        }
      ],
      "local": false,
      "line": 5
    }
  ],
  "entry_world": 0
}
```

The document is **index-only**: patterns and write vectors carry symbol
indices, never glyphs — the processor never sees glyphs, so neither does the
IR. Each tape's alphabet *name* and cardinality ride along for readability
and for index-bound validation; the glyph tables stay in the presentation
layers (the map sidecar, tape blocks).

- `kind` per world is `machine` or `routine`. Graphs do not survive to the
  IR — they have been spliced into their hosts by then.
- State ids are dense (`0..states.len()`) in emission order: a world's own
  states in source order, then its spliced graft instances. The entry state
  is *named* by `entry`, not moved to position zero, so every rule's `line`
  keeps pointing at the source it came from.
- Per-cell tags (`kind` field): a pattern cell is `wildcard` or `index`
  (carrying `index`); a write cell is `keep` or `index`. Moves are `left`,
  `right`, `stay`. `write` and `moves` are omitted entirely when the whole
  vector is the identity — all-keep, all-stay — which is the same condition
  under which codegen elides the action instruction.
- Per-transition tags (`kind` field, snake_case): `goto` (`state`),
  `call_then` (`target`, an optional `binding`, and a `then` resume point
  that is itself a `goto`/`return`/`stop`/`halt`), `return`, `stop`, `halt`,
  `tail_call` (`target`), and the two synthesized trap terminals `trap_read`
  and `trap_write`. A `binding` entry carries the same per-callee-tape data
  the `.tma` binding-call operand does: `caller_tape`, plus each authored
  `src`/`dst` pair resolved to caller and callee alphabet indices, with
  one-way pairs flagged. No blank pin or closure is applied here — the
  composition engine does that at link time.
- `dispatch` is a codegen hint, `table` (the canonical form: a match table
  plus an indexed jump) or `branch` (the two-row form the optimizer's
  dispatch-selection pass picks). `tail_call`, `branch`, and the `debugger`
  and `synthesized` row flags are the shapes only the optimizer or the
  compiler's own splicing produce; a `-O0` document carries none of them but
  `synthesized`.
