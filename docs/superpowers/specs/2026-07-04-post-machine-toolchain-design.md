# Post Machine Toolchain — Design

**Date:** 2026-07-04
**Repo:** `mellonis-workspace/machines/toolchains` (own repo under the `machines/` umbrella)
**Status:** approved design, pre-implementation

## 1. Purpose

A complete Rust toolchain for a Post machine: a C-like source language, an
optimizing compiler, an assembler/disassembler, a linker, and a bytecode
processor (VM). It finishes the work started in four Delphi generations
(2002–2012), which collectively produced a language without a code generator
(`Compiller`, 2012), a code generator without that language (`Old
Test-PostMachine`, 2007), and a machine without a compiler (`PMProcessor`).
This project closes the triangle and adds the piece none of them attempted: a
linker with separate compilation and libraries.

Self-contained: no runtime dependency on `@post-machine-js/machine` or
`@turing-machine-js/machine`. Related to them thematically, not technically.

## 2. Toolchain pipeline

```
hello.pmc ──pmt compile──▶ hello.pmo         (default; -S emits hello.pma)
lib.pma   ──pmt asm──────▶ lib.pmo
*.pmo     ──pmt link─────▶ app.pmx           (needs main; dead functions dropped)
app.pmx   ──pmt dis──────▶ text asm          (also accepts .pmo)
app.pmx   ──pmt run──────▶ execution on a tape
```

| Extension | Role |
|---|---|
| `.pmc` | C-like source |
| `.pma` | textual assembly (human hub; round-trippable through dis/asm) |
| `.pmo` | object file (binary; symbols + relocations, per-function code) |
| `.pmx` | linked executable (binary) |
| `.pmt` | tape-block snapshot (binary; VM input/output — the extension coincides with the `pmt` tool name; both spell "post-machine tape/toolchain", accepted) |

## 3. Source language (`.pmc`)

C-like surface, deliberately flat control flow: **labels, `goto`, `check`,
and function calls only** — no loops, no general `if`, no expressions.

```c
// Move right until the first blank cell.
goToEnd() {
1:  right;
    check(1, 2);      // cell marked → goto 1, blank → goto 2
2:  left;             // last command — implicit return
}

// The old explicit-successor style works too:
goToBegin() {
1:  left(2);          // left, then goto 2
2:  check(1, 3);
3:  right(!);         // right, then return
}

main() {
    @goToEnd();       // not defined here → external symbol for the linker
    right;
    check(3, 4);
3:  unmark(!);        // unmark, then return — in main: stop the machine
4:  mark;             // last command — implicit stop
}
```

### 3.1 Structure

- A file is a sequence of function definitions: `name() { statements }` —
  no `void` (the language has no types), no parameters, no return values,
  no nesting.
- `main` is the program entry point; required at link time for a `.pmx`.
- Identifiers: Unicode, JavaScript-flavored, concretely: first character
  alphabetic (Unicode `Alphabetic`) or `_`, then alphanumeric or `_` — a
  conservative subset of JS `ID_Start`/`ID_Continue`, and exactly the
  `.pma` symbol rule, so every compiled name survives the trip through
  generated assembly. Case-sensitive.
- Comments: `//` line and `/* ... */` block.

### 3.2 Statements

Every command takes an optional **successor** in parentheses: a numeric label
(jump there afterwards) or `!` (return afterwards). No successor = fall
through to the next statement. Returning from `main` stops the machine.

| Statement | Meaning |
|---|---|
| `left` `right` `mark` `unmark` | tape builtins; `left;` ≡ `left();` = fall through, `left(5);` = then goto 5, `left(!);` = then return |
| `halt` | abnormal stop (`hlt` opcode); no successor — execution ends |
| `debugger` | breakpoint (`brk` opcode) — JS semantics: pauses under an attached debugger, no-op otherwise; no successor |
| `check(A1, A2);` | the only conditional: cell marked → `A1`, blank → `A2`; each arm is a label or `!` |
| `goto N;` | unconditional jump; `N` is a numeric label only — `goto !;` is a syntax error (use `(!)` on the preceding command) |
| `@name();` `@name(5);` `@name(!);` | call a user function (`@` sigil), with the same optional successor (`@name(!)` is a tail call) |
| `N:` | numeric label, local to the enclosing function |
| `cmd, cmd, …, cmd;` | comma group: commands run in sequence under one statement (the `Sum2.pms` dialect). Only the last item may carry a successor or be a `check` or `halt`; earlier items must be bare (builtins, `debugger`, or `@calls` — `halt` mid-group is rejected like mid-group `check`, since the rest could never run). A label applies to the whole group. |

There is no `return` keyword: mid-function return is the `(!)` successor, and
the last command of a body may omit it — falling off the end is an implicit
return.

```c
1:  right, right, mark(5);      // group, then goto 5
2:  left, check(1, !);          // group ending in the conditional

// errors — non-last items must be bare:
// 3:  left(1), left(2);        // successor mid-group
// 4:  check(1, 2), left;       // check mid-group
```

### 3.3 Rules

- **Reserved words** (cannot name a function): `goto`, `check`, `left`,
  `right`, `mark`, `unmark`, `halt`, `debugger`. (`export`, `use`,
  `namespace`, `as` are CONTEXTUAL keywords, not reserved — §3.4.)
- Builtins may omit `()`. User calls are written `@name();` — `@` prefix
  and parens required. A bare identifier statement (with or without parens,
  no `@`) is an error unless it is a builtin; `@` on a builtin name is an
  error too.
- Labels are decimal numbers, unique per function, referenced only by `goto`
  and `check` in the same function. Declaration order is free (no
  strictly-increasing requirement, unlike the 2012 compiler).
- Falling off the end of a function body = implicit return (the last
  command's `(!)` may always be omitted).
- Calling an undefined function is not an error at compile time: it becomes an
  external symbol resolved by the linker (no `extern` boilerplate) — but the
  compiler warns unless the name is declared with `use` or called fully
  qualified (§3.4).
- Duplicate function definitions in one file are an error; across objects,
  a link-time error.

### 3.4 Visibility, nesting, namespaces, imports

- **Hidden by default:** top-level functions are module-local unless
  prefixed `export`; the un-namespaced top-level `main` always exports.
  Local functions become `local` symbols (§6.2): bound directly within
  their object, invisible to cross-object resolution — they can neither
  shadow nor be shadowed.
- **Nested definitions** (`outer() { inner() { … } … @inner(); }`):
  flat code, scoped callability — an inner function is callable from its
  parent's body and deeper only; always local; hoisted (visible anywhere
  in the body); resolution is innermost-scope-outward. Flattened with
  dot-mangled names (`outer.inner`) — unnameable from source (`.pmc`
  identifiers cannot contain `.` or `:`).
- **Namespaces:** `namespace ns { … }` blocks — a naming/scope construct
  only: multiple per file, nestable, OPEN (reopening merges — scopes key
  by path; any object may define `ns::*` symbols; no sealing in v1).
  Exports inside become `ns::path::name` symbols (namespaces join with
  `::`, nesting keeps `.` — symbols self-decompose at the last `::`).
  Namespace names share the name pool with functions per scope. Only the
  un-namespaced top-level `main` is the entry.
- **Imports:** `use PATH [as alias][, PATH…];` declares an external
  symbol by its full name and binds ONE bare name (alias, else path
  tail) into the declaring scope — legal at file level and inside
  namespace blocks (scoped; inner shadows outer). Binding collisions
  within one scope (keyed on the post-alias name) are errors; definitions
  outrank bindings. Bare `use name;` is the path-length-1 case.
- **Qualified calls:** `@ns::path::name()` — absolute (scope chain
  skipped), `::` segments only (nested functions stay unnameable),
  self-declaring (exempt from the undeclared-external warning).
- **Warnings** (report-carried, never printed; CLI strictness later):
  bare calls to undeclared externals (once per name); unused imports;
  unused functions (unexported + unreached from `main`/exports — sound
  because locals are invisible outside the module).

## 4. Processor architecture

**Hardware realizability is a design requirement:** every v1 concept must
map to synchronous digital logic plus a physical tape transport — the VM
is the reference implementation of a machine that could be built (FPGA,
discrete logic, a mechanical tape). Concretely: fixed-width architectural
state only (IP/SP u32, FLAGS, MR bounded by table size); stack = SRAM +
SP register; code = ROM; symbols = indices (hardware never sees glyphs);
traps = a fault-code register latched on trap + a HALT line (the API's
structured fault is its software rendering); CRC is the flasher's job, not
the device's; and nothing in the core assumes an unbounded tape — physical
tapes are bounded/annular, which the tape profiles already model. Future
features must pass the same test.

Harvard core with **every memory behind a bus** — the 2007
`TPostMachineProcessor`/`TBelt` split, applied to all memories. The core owns
only its registers and the fetch/decode/execute automaton; code, stack, and
tape are external components reached through narrow interfaces:

```
┌─ processor core ────────────┐
│  IP, SP, FLAGS(MF)          │
│  fetch / decode / execute   │
└──┬──────────┬──────────┬────┘
   │ code bus │ stack bus│ device bus
   ▼          ▼          ▼
 code ROM   stack RAM   tape (device 0)
 fetch(a)   push/pop,   left/right/
 → byte     depth       read/write
```

The core is implemented **sans-I/O**: a pure transition function
`(coreState, busResponse) → (coreState', nextBusRequest)` that emits bus
requests and never performs I/O itself. **Drivers** execute the requests:
v1 ships the synchronous driver (tight loop over in-memory devices — full
speed, what tests and `pmt run` use); an asynchronous driver (awaiting
Promise-returning devices — real hardware, wait states become real waits)
is a thin future shell requiring no change to the core, ISA semantics, or
timing model. The core alone is testable with no devices at all: feed
responses, assert requests.

A real-world build (FPGA, relay logic, a physical tape) implements the same
buses in hardware; the Rust VM wires the same core to in-memory
implementations: code ROM is a `Box<[u8]>` sized exactly to the `.pmx`
code section (operands read via `u32::from_le_bytes` etc.); the return
stack is a fixed `Vec<u32>` of code offsets with configurable depth
(default 1024), `SP` counting entries — full on `call` traps overflow,
empty on `ret` traps underflow; the tape is the paged structure of §4.2,
not a flat array. v1 buses are synchronous; an asynchronous bus variant (real
hardware takes time to move a tape) is a designed-for additive extension —
the same pattern as `turing-machine-js` #219's hardware tape block. TM-1
adds a fourth bus for the read-only table section (Appendix A).

### 4.1 Registers

- **IP** — instruction pointer: byte offset of the current instruction in the
  code image.
- **SP** — stack pointer: top index of the dedicated return stack (a separate
  memory of return addresses; size is a VM parameter). `call` pushes /
  `ret` pops; overflow and underflow are traps.
- **FLAGS** — bit 0 is **MF** (match flag). In PM-1 every tape instruction
  (`lft`, `rgt`, `wr`) performs an implicit match against the mark
  index: `MF := (tape.read() === 1)`, also latched once before the first
  instruction. `jm`/`jnm` (jump if match / no match) test MF, never the tape
  directly. MF is formally the 1-bit view of the **match register MR**
  (`MF = MR ≠ 0`) — wider architectures give MR more values (index of the
  matched rule) while `jm`/`jnm` keep their meaning (Appendix A). Other
  bits reserved, read as 0.

Besides the architectural registers above, the core has **internal buffers**
that no instruction can observe: the instruction register / operand latch
(IR — fetched opcode + operand bytes, staged between fetch and execute; near
notional over synchronous buses, load-bearing over async ones), and in TM-1
a head-read latch (batched reads strobe all heads once into a symbol-vector
latch; matches compare against the latch, making the read tuple an atomic
snapshot). The debug API may display them; the programming model never
depends on them.

### 4.2 Tape interface and device bus

The processor never knows the head position. It drives peripherals through a
**device bus**: an array of tape devices, of which architecture v1 (PM-1)
addresses only device 0 — the tape. Future architectures address more
devices (see Appendix A). Devices operate on **symbol indices, not
symbols** — the processor is alphabet-agnostic (the `Alphabet`
index-encoding idea from `@turing-machine-js`, applied at the hardware
boundary). The actual glyphs are presentation-layer metadata (debug info /
run-time supplied); tools show bare indices when no alphabet is attached.

```rust
trait Tape {
    fn alphabet_size(&self) -> u32;   // writing index >= size is a trap
    fn left(&mut self);
    fn right(&mut self);
    fn read(&self) -> u32;            // index of the symbol under the head
    fn write(&mut self, index: u32);
}
// PM-1's tape is the 2-symbol case: alphabet_size() == 2, 0 = blank, 1 = marked
```

For PM-1: the language's `mark`/`unmark` compile to `wr 1`/`wr 0`;
`MF := read() === 1`. The ISA itself has no mark/unmark concept.

Shipped implementations:

- **InfiniteTape** (default) — unbounded in both directions; head coordinate
  exposed for inspection/UI. **Paged sparse storage** (the `TBelt` packed
  bit array, generalized): a `HashMap` of fixed-size pages, each page a
  `u64` bitmask (wider cells per page for bigger alphabets), with the
  current page cached since the head moves ±1 — most ops are bit-ops with
  no map lookup.
  Guarantees: reads never allocate (page miss = blank; walking any distance
  over blank tape costs zero memory); a blank write that zeroes its page
  frees the page — memory stays `O(pages containing non-blank cells)`,
  never `O(touched cells)`. (Deliberately unlike growable-array tapes such
  as `turing-machine-js`'s, which allocate the walked distance.)
- **AnnularTape(size = 2048)** — ring-shaped bounded tape, the historical
  `TBelt` (−1024…1023, wrap-around).
- **StrictTape decorator** — wraps any tape; `mark` on a marked cell or
  `unmark` on a blank one throws (2006/2007 semantics). Default semantics are
  idempotent — required for the mark/unmark optimizations to be legal;
  compiling with `--strict-cells` disables those optimizations.

### 4.3 Loading

`main` is located at **link time**: the linker resolves the `main` symbol and
writes its byte offset into the `.pmx` header's entry-offset field. A `.pmx`
carries no symbol table — at run time "main" is just that number (like ELF's
`e_entry`). The loader (`pmt run` / API):

1. validates magic, crc32, format version, and arch byte; selects the
   architecture module;
2. copies the code section into read-only code memory;
3. attaches the tape (device 0) supplied by the caller (default
   `InfiniteTape`);
4. initializes `IP := entry offset`, `SP := 0`, latches
   `MF := (tape.read() === 1)`;
5. verifies `code[IP]` is `ent` (corrupt entry point traps before the first
   step);
6. runs.

### 4.4 Timing model (tacts)

Deterministic cycle accounting over the buses: each code-bus byte fetched =
1 tact; execute base = 1 tact per instruction; each stack word pushed/popped
= 1 tact; device commands cost what the **tape profile** says — electronic
default `move/read/write = 1`, a configurable `mechanical` profile (e.g.
`move 50, write 10, read 5`) models a physical tape. The MF latch is honest:
every tape instruction pays its trailing `read()`. Examples (electronic):
`rgt` = 4 tacts; `jm` = 6 vs `jm.s` = 3 (relaxation is a speed win, not
just size); `call` = 8 vs `call.s` = 5 (the `ent` verification is a real
code-bus read at the target — 1 tact). The tact counter is meta-state like
the step counter — on VM state, `DebugSession`, and run results, usable as
a budget (`--max-tacts`), never observable by programs, so the §8
equivalence contract is unaffected and optimizer tests can assert "fewer
tacts, same tape".

**Wait states:** during a device transaction the core stalls — IR held,
nothing executes, the tact counter runs for the device's full price (no
pipeline hides latency; that's the honest model). Accounting splits into
*core tacts* (fetch/execute/stack) and *stall tacts* (waiting on devices) —
both reported on stats and `DebugSession`, which also exposes core state
as `running` / `stalled-on-device`. In sync v1 the stall is arithmetic;
the async-bus extension replaces the counted constant with a real
device-ready wait (a WAIT line), observably identical. Finer realism
(pipelining) stays out of scope.

### 4.5 Execution and traps

Program starts at the `.pmx` entry point (main's `ent`). Normal termination:
`stp`. Abnormal: `hlt`. A **trap** is the processor's controlled stop on an
execution error (a CPU-exception analogue; the Delphi generations' `raise
EPMException`): the machine halts on the faulting instruction and reports the
fault kind plus a full state snapshot (IP, SP, FLAGS, stack, tape). Trap
causes in PM-1: invalid opcode (`0x00` or undefined), jump/IP outside the
code image, `call` to a byte that is not `ent`, return-stack
overflow/underflow, step-limit exceeded (configurable runaway guard, the
descendant of every Delphi generation's step cap), and — under strict-cells
semantics — double-mark/double-unmark. In the API a trap surfaces as a
structured fault result; under the debug API it pauses on the faulting
instruction instead.

VM API: `run()`, generator-based `step()`, full state inspection (IP, SP,
FLAGS, stack contents, tape), address breakpoints, and the `brk` opcode which
pauses under the debug API and is a no-op otherwise.

The interactive debugger is a **`DebugSession`** with the same surface shape
as `turing-machine-js` v7's (session owns the run; `pause`/`step` events
with a `cause`; `continue`/`stepIn`/`stepOver`/`stepOut`; external
`pause()`/`stop()`; run-interval throttle) — familiar API, new internals:
depth is just SP, so `stepIn` = one instruction, `stepOver` = run until
`SP ≤ SP₀`, `stepOut` = until `SP < SP₀`. Pause causes: `breakpoint`
(address), `brk` (opcode), `step`, `manual`, and `trap` (paused on the
faulting instruction with the fault attached). `.pmo` line maps later add
source-level stepping above instruction stepping.

## 5. Instruction set

Byte-addressed, variable-length: 1-byte opcode + optional immediate. Jump and
call operands are **IP-relative** (to the end of the instruction) —
position-independent code, which keeps the linker to pure layout + patching.

| Opcode | Mnemonic | Operand | Meaning |
|---|---|---|---|
| `0x00` | — | | invalid → trap |
| `0x01` | `nop` | | no operation |
| `0x02` | `stp` | | stop, normal termination |
| `0x03` | `hlt` | | halt, abnormal termination |
| `0x04` | `lft` | | head left (latches MF) |
| `0x05` | `rgt` | | head right (latches MF) |
| `0x06` | `wr` | symbol vector | write symbol index to the cell (latches MF). Operand is the self-delimiting vector from Appendix A; in PM-1 always one element: `wr 1` = mark, `wr 0` = blank |
| `0x07` | — | | reserved |
| `0x08` | `jmp` | rel i32 | unconditional jump |
| `0x09` | `jm` | rel i32 | jump if match (MF = 1) |
| `0x0A` | `jnm` | rel i32 | jump if no match (MF = 0) |
| `0x0B` | `call` | rel i32 | verify target is `ent`, push return address, jump |
| `0x0C` | `ret` | | pop return address, jump |
| `0x0D` | `ent` | | function landing pad; executes as no-op |
| `0x0E` | `brk` | | breakpoint (`debugger` builtin) |
| `0x18` | `jmp.s` | rel i8 | short form of `0x08` |
| `0x19` | `jm.s` | rel i8 | short form of `0x09` |
| `0x1A` | `jnm.s` | rel i8 | short form of `0x0A` |
| `0x1B` | `call.s` | rel i8 | short form of `0x0B` |

- Short form rule: `short = far | 0x10`.
- **`ent` verification is always on**: `call`/`call.s` trap unless the target
  byte is `0x0D`. Every function begins with `ent` (the compiler emits it; the
  assembler's `.func` directive inserts it). Jumping onto an `ent` is legal
  (it is a no-op) — only `call` checks.
- Opcodes `≥ 0x80` are reserved for future multi-byte encodings (the
  PMProcessor continuation-bit idea).
- **Width selection:** intra-function jumps are relaxed by the
  assembler/compiler back end (iterate until sizes stabilize). `call` width is
  decided by **linker relaxation**: layout with far calls, then iteratively
  shrink calls whose targets fit ±127, re-patching until stable
  (`--no-relax` disables).

## 6. File formats

All multi-byte integers little-endian.

Magics are toolchain-neutral — two ASCII letters + a binary epoch byte:
`MO 0x01` object, `MX 0x01` executable, `MT 0x01` tape-block (the epoch
byte marks header-layout generations and doubles as a text-file guard;
the `u16 format version` field covers evolution within an epoch) —
because the containers are shared across machine toolchains
(§10): the file *extension* carries the family flavor (`.pmo`/`.pmx`/`.pmt`
from `pmt`; `.tmo`/`.tmx`/`.tmt` from `tmt` later), the magic + arch byte
identify the actual content. Tools never dispatch on extensions.

### 6.1 `.pmx` — executable

```
magic "MX" + u8 epoch (0x01) | u16 format version | u8 arch (0x01 = PM-1) | u8 flags (0)
u32 crc32 | u32 entry offset | u32 code size
code bytes
```

`crc32` covers the whole file with the field itself zeroed; writers stamp it
last, every consumer (loader, linker, disassembler) verifies it before
decoding anything — mismatch is a clean "corrupt file" error, not a trap
mid-run.

The `arch` byte identifies the instruction set. The VM is split into a
generic core (fetch loop, return stack, traps, debug API) and a pluggable
architecture module (opcode table + semantics); `arch` selects the module.
v1 defines only `0x01` (PM-1); a future Turing-oriented `0x02` (TM-1,
Appendix A) reuses the core, formats, assembler framework, linker, and CLI.

The initial tape contents are **not** embedded — they are supplied to the VM
at run time (`pmt run app.pmx --tape "..*..***" --head 2` or via API),
matching the Delphi processors' separate tape/code loading. `--tape` maps
its first character to cell 0 rightward (`.`/space = blank, `*` = mark);
`--head <int>` (default 0) sets the initial head coordinate — negative is
legal on an infinite tape. The API form constructs the tape from symbols +
head position directly.

### 6.2 `.pmo` — object file

```
magic "MO" + u8 epoch (0x01) | u16 format version | u8 arch (0x01 = PM-1) | u8 flags (bit0 = has debug section; other bits 0)
u32 crc32 (same scheme as .pmx)
string table
symbol table:  name → { defined: blob index | local: blob index | external }
               (wire kinds: external 0, defined 1, local 2 — local =
               defined-but-not-exported; object format version 2, readers
               accept 1–2; MX/MT keep version 1)
code blobs:    one per defined function (intra-function jumps resolved,
               starts with ent)
relocations:   { blob, offset, symbol } for each call site (4-byte hole)
debug section (optional): per-blob label map + source line map
```

Per-function granularity is what gives the linker dead-function elimination
and leaves the door open for link-time inlining. A "library" is simply a
`.pmo` with many functions — only what `main` reaches is linked in.

### 6.3 `.pmt` — tape-block snapshot

Binary tape-block state (the `TapeBlock` concept: N tapes with their heads;
PM-1 blocks always hold one tape), usable as VM input and output — golden
tests diff final blocks as files:

```
magic "MT" + u8 epoch (0x01) | u16 format version | u8 flags (0) | u32 crc32
alphabet: u8 count, then one length-prefixed UTF-8 glyph per index
u8 tape count
per tape: i64 origin | u32 length | u8 indices[length] | i64 head position
```

The alphabet travels with the data — a `.pmt` renders with its own glyphs
(index 0 = blank by convention); one dense span per tape, cells outside it
blank. **Glyphs live ONLY on the tape side**: the tape block's alphabet is
the authoritative rendering source; with no tape at hand, tools fall back
to the arch module's default glyphs (PM-1: `" "`, `"*"`). Code-side
artifacts — `.pmo`, `.pmx`, and the `.pmx.map` sidecar — carry indices
only, never glyphs (§4's realizability rule: hardware never sees glyphs).
CLI: `pmt tape build " * * *" --head 3 -o in.pmt`,
`pmt tape show in.pmt`,
`pmt run app.pmx --tape-block in.pmt [--save-tape-block out.pmt]`.

### 6.4 `.pma` — assembly text

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

Symbolic labels (`L1:`), one instruction per line, `;` comments. Canonical
column grid emitted by `pmt compile -S` and `pmt dis`: labels at column 0,
mnemonics at 8, operands at 16, comments at 32; the assembler itself accepts
any whitespace. `pmt dis` output is valid assembler input (round-trip
property, tested against the canonical form).

`pmt dis` accepts both binaries. From `.pmo`: real names via the symbol
table, per-function code, call sites named from relocations. From `.pmx`:
names from the `-g` sidecar map when present; otherwise synthesized via
**recursive-descent discovery** (worklist from the entry point following
control-flow edges; every verified call target is a function root — exact
in v1, which has no indirect control flow; bytes never reached print as
`.byte`): `func_XXXX` at roots, `LXXXX` at jump targets. The `ent` byte
remains the runtime call guard, but function discovery comes from control
flow, not byte scanning.

**Symbol jumps (tail calls):** `jmp @name` takes a function symbol, not a
label — it assembles as the far `jmp` plus a relocation (the same
hole-and-debt mechanism as `call`) and relaxes to `jmp.s` at link time.
`jmp.s @name` is an error (width is linker-selected, like `call.s`), and
conditional `jm`/`jnm @name` are errors — v1 branches take labels only.
Disassemblers print relocated jumps (from objects, via the relocation
table) and jumps landing on function roots (from executables, via
discovery) in the `jmp @name` form; a jump into another function's middle
that targets no root still falls back to `.byte`.

**Visibility and names:** `.func name local` declares a local (unexported)
function; plain `.func name` exports. Symbol names in `.func` lines and
call/jump operands accept `::`-separated segments of dotted identifiers
(`std::api.helper` — namespace part before the last `::`, function-nesting
part after; every symbol self-decomposes). LABELS remain colon-free — the
label grammar depends on it.

## 7. Compiler

Modules, all pure and individually testable:

1. **Lexer** — tokens with line:col (the 2007 compiler's diagnostic quality
   as the baseline).
2. **Parser** — recursive descent → AST (one parameterized function-body
   parser; no copy-paste per context like the 2012 state machine).
3. **Lowering** — AST → per-function CFG IR: basic blocks of
   `{lft, rgt, wr(i), brk, call}` with terminators
   `{fallthrough, goto, check(t,f), return, halt, tailcall(name)}` —
   `halt` is a terminator, not a block op (a block after `halt` can never
   execute; a false fall-through edge would poison the optimizer's
   dataflow); `tailcall(name)` is produced only by the optimizer (§8 pass
   8), never by lowering (IR JSON v2); v3 adds per-function `local`
   visibility flags and pre-mangled nested/namespaced names (§3.4). `!`
   check arms target a shared synthetic return block per function.
   Statement successors (`(5)`, `(!)`, fall-through, end-of-body) all
   lower to these block edges — the old IR's `-1` stop / `-2` auto-link
   semantics, made explicit.
4. **Semantic checks** — undefined labels, duplicate labels/functions;
   warnings for unreachable code (which `-O1` then deletes).
5. **Optimizer** — see §8.
6. **Back end** — CFG → `.pmo` (default) or `.pma` (`-S`): block layout,
   fall-through selection (an unconditional jump to the physically next
   instruction is never emitted — the 2007 compiler's optimization as a
   layout invariant, active even at `-O0`), intra-function jump relaxation,
   `ent` prologue; the return terminator emits `ret` (in `main`: `stp`).

### 7.1 IR as an artifact

The IR is a **versioned, documented JSON artifact**, not an internal detail:
`pmt compile --emit-ir[=stage]` dumps per-function CFGs at any pass boundary
(`lowered`, `after:<pass>`, `final`) for optimizer debugging, exact-effect
pass tests, and analysis; `pmt ir graph` renders a CFG as Mermaid (the
`toMermaid` tradition). Codegen consumes IR regardless of origin — the
`.pmc` parser is just the first front end. Designed extensions (not v1):
IR files as compile input, and a `@post-machine-js`-dialect front end
(JSON program form; `erase`/`mark` → `wr`, `check` → check terminator,
subroutines → functions, groups → straight-line blocks) so historical
programs compile to `.pmx`.

## 8. Optimizer

**Equivalence contract** — every pass must preserve: final tape contents,
termination kind (`stp` / `hlt` / which trap), and every MF-dependent branch
decision. Resource-limit traps (step/tact limits, stack overflow/underflow)
are quality-of-implementation outcomes, not semantic observables: passes may
change resource consumption — inline and tail-call change stack depth, and a
self-recursive tail call becomes an in-place loop (StackOverflow at `-O0` →
StepLimit at `-O1` is legal). Explicitly *not* observable: step count and
intermediate states —
except at un-stripped `brk` instructions, which are observability barriers
(no motion or elimination across them; the debugger sees honest state).
The **effect model** each pass reasons over: cell writes, head moves
(never removable; they invalidate cell knowledge), and the **MF latch** —
every tape instruction latches MF and jumps don't, so removing a
tape-redundant write still requires proving MF is unaffected at all its
`jm`/`jnm` uses (the cell-state pass tracks cells *and* MF). `@call` is an
opaque barrier (head, cells, MF all clobbered) until inlining dissolves it.
Comma groups lower to plain sequences before optimization and confer no
atomicity.

Per-function CFG passes, each a named module, individually toggleable
(`-O0` none, `-O1` all; `--fno-<pass>` opt-outs):

1. **check-fold** — `check(N, N)` → `goto N`; `check` with one arm falling
   through → single `jm`/`jnm` (generalizes the 2007 `if1`/`if0`
   specialization).
2. **jump-threading** — jump-to-jump chains collapse to their final target.
3. **dce** — unreachable-block elimination (2012 warned; this deletes).
4. **cell-state** — redundant-write elimination (the mark/unmark
   optimization, generalized to `wr`): track the known cell value between
   head moves — consecutive writes to the same cell keep only the last
   (`wr i; wr j` → `wr j`); a write of a value the cell provably holds
   (from a prior write or a `check` arm) is dropped. Legal because default
   cell semantics are idempotent; disabled under `--strict-cells`.
5. **inline** — call inlining: leaf/small functions and single-call-site
   functions, intra-module. Link-time (cross-module) inlining is a designed
   extension point, not in v1.
6. **branch-fold** — conditional jumps with statically-known MF vanish or
   go unconditional: `wr 1; jnm X` → jump deleted; `wr 1; jm X` → `jmp X`
   (which layout may then absorb). Fed by the same cell/MF dataflow as
   cell-state.
7. **tail-merge** — identical trailing sequences are shared and branches
   retargeted (`1: check(!, 2); 2: mark(!);` emits `jm Lstp; wr 1;
   Lstp: stp` — one `stp` serves both paths, not two).
8. **tail-call** — a call whose successor is return (`@f(!)`, or a call
   falling into end-of-body) emits `jmp` to the callee instead of
   `call` + `ret`: saves a stack slot and two IPs of travel. The jump
   target is the callee's `ent`, which is legal for jumps. Not applied in
   `main` (whose return is `stp`, not `ret` — the callee's `ret` would
   underflow).

The list is open: further candidates from the old notes slot in as passes.

## 9. Linker

`pmt link a.pmo b.pmo -o app.pmx`:

1. Collect symbols; error on duplicates and on unresolved references; require
   `main`.
2. Reachability from `main` → drop dead functions.
3. Layout blobs; patch call relocations (IP-relative).
4. Relaxation: shrink in-range calls to `call.s`, iterate to fixpoint
   (`--no-relax` to skip).
5. Emit `.pmx` (entry = `main`'s `ent`).

**Standard library:** a prebuilt `std.pmo` ships with the toolchain,
written in `.pmc` (dogfooding; its golden tests double as compiler tests).
After user objects are collected, remaining unresolved symbols are matched
against it — `libc` semantics: only reachable routines link in (free via
dead-function elimination), `--nostdlib` opts out. Local symbols never
enter the resolution namespace — bound directly within their object; a
local name may repeat across objects freely; calling another object's
local is an unresolved-symbol error; unreached locals are silently
omitted (not reported as dropped). Shadowing/interposition is an OPT-IN
property of exported names: overriding a namespaced export means
declaring inside the same namespace (`namespace std { export goToEnd()
{…} }` — same symbol, user-beats-library arbitration). Accidental
collision impossible, deliberate override explicit; this supersedes the
earlier "user definitions shadow stdlib naturally" rule. Additional libraries via the `cc` convention: `-l <name>` resolves
`<name>.pmo` on the library search path — `-L <dir>` entries in order, then
the toolchain's own `lib/` directory (where `std.pmo` lives; std stays
implicit rather than requiring `-l std`).

**Interposition vs optimization:** `-O1`'s inline binds intra-module calls
at compile time, so shadowing a library's *internal* callees affects only
the symbolic call sites that survive optimization — the linker guarantees
interposition only for relocations it actually sees (the semantic-binding
default of mainstream compilers). A library that must stay fully
interposable is built with `--fno-inline`; whether `std.pmo` is built that
way is settled with the Plan 7 build.

Starter roster, each with a documented head pre/postcondition:
`goToEnd` / `goToBegin` (the historic pair: from inside a marked section to
the first blank after/before it), `goToMarkRight` / `goToMarkLeft`,
`goToBlankRight` / `goToBlankLeft`, `eraseSection`, and the section-edge
quartet `appendMark` / `prependMark` / `removeLastMark` / `removeFirstMark`
(grow/shrink a section from either end — "first/last" rather than
"head/tail" to avoid colliding with the machine head). Final roster settled
against the old notes during implementation.

## 10. Project shape

- **Location:** `~/Developer/mellonis-workspace/machines/toolchains/`,
  own git repo under the `machines/` umbrella (gitignored sibling; add an
  entry to `machines/CLAUDE.md`).
- **Stack:** Rust (stable toolchain, edition 2024), a **cargo workspace**.
  Quality gates: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt
  --check`; coverage via `cargo llvm-cov` once tests exist. Dependencies
  kept minimal: `std` for everything feasible; `serde`/`serde_json` only
  for the JSON artifacts (IR dumps, `.pmx.map` sidecar); CRC32 implemented
  in-repo (it's ~20 lines). A `wasm32` build of `core` (for a future
  browser demo) is a designed-for target, not a v1 deliverable.
- **Crates:** the shared/arch boundary is enforced by the crate split:
  - `crates/core` (lib) — VM core + buses, tape devices (tapes, `.pmt`),
    `MO`/`MX`/`MT` container formats, linker, assembler/disassembler
    frameworks, `DebugSession`. **Arch-agnostic by contract**: core
    contains no opcode; arch modules supply decode tables, instruction
    semantics, mnemonics, and relaxation widths through the arch trait.
    Core's own tests run against a tiny fake test arch, so PM-specifics
    can't leak in unnoticed;
  - `crates/post-machine` (lib + `pmt` bin) — PM-1 arch module, `.pmc`
    compiler + optimizer, stdlib, the `pmt` CLI;
  - `crates/turing-machine` — future (`tmt`, TM-1; Appendix A).
  v1 ships `core` + `post-machine`. crates.io names/publication are later
  decisions.
- **Build modes:** orthogonal switches, `cc`-style — `-g` (debug info:
  label/line maps in `.pmo`; at link, a JSON sidecar `app.pmx.map` carrying
  function ranges and line maps — the `.pmx` itself stays a pure code
  image, and glyphs stay on the tape side, §6.3), `-O0`/`-O1`, `--strip-debugger` (drop
  `brk` at codegen). Presets: `--debug` ≡ `-g -O0`; `--release` ≡
  `-O1 --strip-debugger`. Default: `-O0`, no `-g`. `-g -O1` is legal with
  the usual caveat: optimized code maps to source approximately.
- **CLI:** one binary `pmt` with subcommands `compile`, `asm`, `link`, `dis`,
  `run`, `tape` (§6.3), `ir` (§7.1). **Verbosity:** library code never
  prints — each stage returns a structured report (e.g. the linker's
  `LinkReport`: dropped functions, relax decisions) that `pmt -v` renders;
  `pmt run --trace` streams per-instruction listing-form disassembly via
  the step API. Library-first: the bin is a thin
  wrapper over public library functions (`compile(source)`,
  `assemble(asm_text)`, `link(objects)`, `disassemble(bytes)`, `Processor`,
  tapes), so tests — and a future WASM browser demo — consume the API
  directly.
- **Source layout:** `crates/core/src/` — `vm/`, `devices/`, `formats/`,
  `linker/`, `asm/`, `disasm/`; `crates/post-machine/src/` — `lexer/`,
  `parser/`, `ir/`, `optimizer/` (one module per pass), `codegen/`, `arch/`
  (the PM-1 module), `stdlib/`, `cli/` (+ `src/bin/pmt.rs`).

## 11. Testing

- Unit tests per module (`#[cfg(test)]` co-located; `tests/` for
  cross-module integration), plus property tests (`proptest`, dev-dep)
  for format round-trips and relaxation fixpoints.
- **Golden end-to-end:** ports of the historical programs (`Sum.pms`,
  `Ty.pms` → `.pmc`) through compile → link → run, asserting final tape
  state (diffed as `.pmt` snapshots, §6.3).
- **Round-trip:** `asm(dis(x)) == x` for `.pmo` and `.pmx`.
- **Equivalence:** every optimizer pass is tested by running optimized vs
  unoptimized builds on the same tapes and comparing final state (plus
  asserting the size actually shrank where expected).
- Relaxation edge cases: jumps at exactly ±127/±128, chains that only
  stabilize after several iterations.

## 12. Documentation

`docs/` in-repo: language reference (`.pmc`), ISA reference, file-format
spec (`.pmo`/`.pmx`/`.pmt`/`.pma`), CLI guide, and a history page mapping this
design back to the four Delphi generations (what was inherited: the
language lineage, fall-through optimization, `ent`-style safety the 2007
call stack lacked, the PMProcessor's disassembler-first mindset).


**Section-reference stability:** code comments cite this spec by section
number plus a topic keyword ("spec §8 (equivalence contract)"). Section
numbers are APPEND-ONLY: new material lands as new subsections or at
section ends; renumbering existing sections is a breaking change that
requires a `grep -rn "spec §" crates/` sweep in the same commit. This
spec is the build-time design authority ONLY UNTIL the Plan 7 docs land
(user ruling): Plan 7's documentation task migrates every code-comment
reference from this spec to the durable references (README + the `docs/`
language/ISA/format/CLI pages), after which THIS document freezes as a
historical design record — linked from the history page, no longer cited
by code, no longer amended. Implementation-internal invariants (e.g. the
MF-coupling argument, closed-terminator-targets) do not migrate to user
docs — they stay self-contained in module docs, being contracts between
passes rather than with users.

**Forge agnosticism:** the rule binds PUBLISHED content — `README.md`,
the `docs/` reference pages, and code comments: no provider URLs, and no
forge-issue references at all; cross-project work is described in prose
(the library and the feature by name, e.g. "the abortState sentinel
being designed for turing-machine-js"). The canonical repository URL
lives ONLY in `Cargo.toml`'s `repository` metadata field (one line to
update on migration). Internal dev artifacts (progress ledger, plan
documents) are unrestricted — issue links and URLs are fine there.
(As refined by user rulings of 2026-07-06; the original rule was
repo-wide plain-text references.)

## 13. Out of scope (v1)

- Cross-module (link-time) inlining — extension point only.
- Multi-byte opcode encodings (`≥ 0x80` reserved).
- Embedding initial tape data in `.pmx`.
- npm publication, browser demo integration — later decisions.
- Converters from the legacy Delphi formats (`.pme`, `.tpme`, 32-bit-word) —
  possible future utilities, not v1.
- The async bus driver — the sans-I/O core and interfaces ship in v1 (§4);
  the Promise-driven driver is the future thin shell.
- Assembler repetition macros (`.rept` etc.) — a TM-1-era assembler
  feature (Appendix A, UTM finding 3); PM-1 programs don't need them.
- IR files as compile input and the `@post-machine-js`-dialect front end
  (§7.1) — designed extensions only; v1 ships `--emit-ir` output.
- The TM-1 architecture itself — v1 only carries its seams (arch byte,
  core/arch-module VM split, device bus, the index-based `Tape` interface)
  and the design seed in Appendix A.

## Appendix A — TM-1 architecture seed (future, not v1)

Recorded now so the ideas aren't lost; to be designed properly together with
the Turing-machine source language that will target it.

- **Purpose:** a multi-tape, wide-alphabet architecture (`arch = 0x02`) for a
  C-like Turing-machine language with its own toolchain front end (`tmt`,
  Turing Machine Toolchain).
- **Sharing contract:** `tmt` = new language front end + TM-1 arch module +
  thin CLI; everything else is imported from this project: the VM core
  (fetch shell, buses, stack, traps, debug API), the `MO`/`MX`/`MT`
  container formats (written as `.tmo`/`.tmx`/`.tmb` — §6's
  extension-vs-magic rule),
  the linker (relaxation queries the arch module for short forms), the
  assembler/disassembler frameworks (mnemonic tables come from the arch
  module), and the tape/tape devices — i.e., everything in
  `packages/core`. Decided: `tmt` becomes `packages/turing-machine` in the
  same monorepo (§10), importing `core` exactly as `post-machine` does.
- **Batched I/O:** reads and writes are whole-tuple operations — one
  instruction reads all heads at once (symbol vector), one writes all heads,
  one moves all heads. This matches the formal multi-tape TM step
  (transition on the read tuple).
- **Self-delimiting symbol vectors (UTF-8-like):** in instruction operands,
  each symbol byte carries 7 bits of payload; the high bit means "this is the
  last tape's symbol" (0 = more tapes follow, 1 = last). No count prefix
  needed; the encoding is independent of the tape count. Payloads are
  **symbol indices** (see §4.2) — the encoding never carries glyphs.
- **Two instruction families:** compact ops for single-byte (7-bit) symbols,
  and n-byte-symbol ops for wider alphabets (multi-byte symbols, same
  continuation idea within a symbol).
- **Tape cap:** a fixed maximum tape count per machine (constant N, exact
  value to decide; the encoding above doesn't limit it).
- **Match register (MR):** the general form of PM-1's MF. MR holds the
  *index of the matched transition rule* (0 = no match); MF is formally
  `MR ≠ 0`, so `jm`/`jnm` are arch-invariant. Two candidate dispatch
  strategies for TM-1, to be decided with the language:
  1. *compile-time* — the compiler lowers a state's rule set to a
     compare-and-branch decision DAG over `match vec; jm` pairs (boolean
     view of MR suffices; the 2007 matching problem becomes a compiler
     pass);
  2. *hardware* — a table-match instruction matches all heads against an
     encoded pattern table, sets MR to the winning rule's index, and an
     indexed-dispatch jump jumps through a target table by MR (the
     matching problem moves into the VM).
  Both fit the same MR model; PM-1's implicit match-against-mark is the
  1-bit case either way.
- **Match-table encoding (hardware strategy):** rows = candidate rules,
  **first match wins**, `MR :=` 1-based index of the winning row (0 = no
  match). Each row is one symbol-index vector in the self-delimiting
  encoding above — N bytes per row over N tapes in the compact family. A
  reserved payload (`0x7F` compact) means *any symbol* at that tape
  position; wildcard and concrete positions mix freely within a row —
  `[other, a, b, other]` is one row. Combined with ordered matching,
  a wildcard position has exactly `ifOtherSymbol` semantics: it fires
  only for combinations no earlier row claimed. A final all-any row is
  the catch-all (tables ending in one can never miss). The
  format can also enumerate every symbol combination explicitly, so any
  complete transition function is expressible; wildcards + the otherwise
  row are the compression, not a limitation.
- **Table layout & search:** three sections — (1) exact rows (no
  wildcards), sorted by symbol vector → binary search; (2) wildcard rows,
  scanned in order; (3) optional all-any catch-all. Exact rows therefore
  beat wildcard rows (as `TapeBlock` patterns beat `ifOtherSymbol`). MR
  numbering follows encoded order. The assembler verifies sortedness,
  disjointness of exact rows, and section discipline at build time — the
  VM trusts the layout.
- **Table location:** match tables and dispatch target-tables live in a
  dedicated **read-only table section** of the executable — a third
  Harvard memory beside code ROM (the modern descendant of PMProcessor's
  AS segment). Match/dispatch instructions carry a small table index, not
  the table itself; the linker deduplicates identical tables across the
  whole program and patches dispatch-table entries (code offsets) like
  any relocation. Requires a sectioned `.pmx` layout — a format-version
  bump over v1's code-only image, which the v1 header's version field
  already anticipates.
- **No-match semantics:** the match instruction never traps — it sets
  `MR := 0`, observable via `jnm` for explicit handling. The indexed
  dispatch instruction **traps on `MR = 0`** (classical TM "no applicable
  transition", `@turing-machine-js`'s throw-on-unexpected-symbol).
  Catch-all-terminated tables can never produce it.
- **UTM stress test** (`docs/examples/brainfuck-utm.tma` — the 4-tape
  brainfuck universal TM from machines-demo#64, hand-written in speculative
  TM-1 asm). Findings it forced:
  1. *Wildcard vs alphabet size:* reserving `0x7F` as "any" caps
     compact-family alphabets at 127 symbols — the UTM's byte alphabet had
     to drop to 0..126, or use the n-byte family.
  2. *Write vectors need a "keep" marker* (`-`): batched `wr` must be able
     to leave chosen tapes untouched — a per-position no-write value,
     sibling of the match wildcard.
  3. *The assembler needs repetition macros* (`.rept v, 0, 126 … {v} …
     .endr`): table-driven states like brainfuck `+`/`-` are inherently
     one-rule-per-value; macros keep the source compact while the table
     stays what a TM must pay.
  4. *Unconditional states are free:* a TM state with only an
     `ifOtherSymbol` rule needs no `rd`/`mtc`/`djmp` — it compiles to
     straight-line `wr`/`mov`/`jmp`. And a catch-all-less fetch table gives
     the interpreted program an "invalid opcode" trap via `MR = 0` for
     free. TapeCommand's write-then-move maps naturally onto the separate
     `wr` + `mov` instructions.
- **Known open problem (from 2007):** an efficient multi-tape
  transition-matching algorithm and in-memory structure — given the read
  tuple, find the matching transition (with wildcard/any-symbol patterns).
  Candidate directions when the time comes: study `@turing-machine-js`'s
  `TapeBlock` pattern matching; compile the transition table to a decision
  DAG over tape symbols; hash exact tuples with wildcard fallback chains.
