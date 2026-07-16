# PM-1 instruction set and processor architecture

PM-1 (`arch` byte `0x01`) is the Post-machine architecture v1 ships. This
page covers the processor model, the opcode table, timing, and execution —
the container formats that carry PM-1 code (`.pmo`/`.pmx`/`.pmt`/`.pma`) are
`docs/formats.md`; the source language that compiles to it is
`docs/language.md`; the `pmt` commands that drive it are `docs/cli.md`.

## Processor architecture

**Hardware realizability is a design requirement**: every concept here maps
to synchronous digital logic plus a physical tape transport. Architectural
state is fixed-width only (IP/SP are `u32`, FLAGS, a bounded return stack);
stack is SRAM plus an SP register; code is ROM; symbols are indices —
hardware never sees glyphs; traps are a fault-code register latched on trap
plus a HALT line (a structured fault result is the API's software
rendering of that); nothing in the core assumes an unbounded tape (physical
tapes are bounded).

The core is a Harvard machine with **every memory behind a bus**: it owns
only its registers and the fetch/decode/execute automaton. Code, the return
stack, and the tape are external components reached through narrow
interfaces (a code bus, a stack bus, and a device bus):

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

The core itself is **sans-I/O**: a pure transition function
`(coreState, busResponse) → (coreState', nextBusRequest)` that emits bus
requests and never performs I/O. A driver executes the requests; v1 ships
the synchronous driver (a tight loop over in-memory devices — full speed,
what tests and `pmt run` use). The core is unit-testable with no devices at
all: feed it responses, assert the requests it emits.

In the Rust VM: code ROM is a byte slice sized exactly to the `.pmx` code
section (operands read as `u32`/`i32`/`i8` in little-endian); the return
stack is a fixed-depth `Vec<u32>` of code offsets (default depth 1024) —
full on `call` traps overflow, empty on `ret` traps underflow; the tape is
one of the implementations below, reached through the `Tape` trait.

### Registers

- **IP** — instruction pointer: byte offset of the current instruction in
  the code image.
- **SP** — implicit in the return-stack's depth: `call` pushes, `ret` pops;
  overflow and underflow are traps.
- **FLAGS** — bit 0 is **MF** (match flag). Every tape instruction (`lft`,
  `rgt`, `wr`, `wrl`, `wrr`) latches `MF := (tape.read() == 1)` after
  acting — for the fused write+move opcodes that read happens after the
  move — and MF is also latched once before the very first instruction
  runs (from the initial tape state). `jm`/`jnm` test MF, never the tape
  directly. Other bits are reserved and read as 0.

Besides these architectural registers, the core has internal buffers no
instruction can observe: the instruction/operand latch staged between
fetch and execute. The debug API may display it; the programming model
never depends on it.

### The tape and device bus

The processor never knows the head position — it drives the tape through a
device bus, addressing only device 0 in PM-1. Devices operate on **symbol
indices, not symbols**: the processor is alphabet-agnostic. The actual
glyphs are presentation-layer metadata, supplied by tooling or a loaded
`.pmt`, never by the processor.

```rust
trait Tape {
    fn alphabet_size(&self) -> u32;   // writing an index >= size faults
    fn left(&mut self);
    fn right(&mut self);
    fn read(&self) -> u32;            // index of the symbol under the head
    fn write(&mut self, index: u32) -> Result<(), DeviceFault>;
    fn head(&self) -> i64;            // current head position
}
// PM-1's tape is the 2-symbol case: alphabet_size() == 2, 0 = blank, 1 = marked
```

For PM-1, the language's `mark`/`unmark` compile to `wr 1`/`wr 0`; the ISA
itself has no mark/unmark concept, only `wr` and the MF latch it triggers.

Shipped tape implementations:

- **InfiniteTape** (default) — unbounded in both directions, paged sparse
  storage: a hash map of fixed-size pages, each a `u64` bitmask, with the
  current page cached (the head only ever moves ±1). Reads never allocate
  — a page miss is blank; a write that zeroes its page frees it — so
  memory stays proportional to the number of pages holding a non-blank
  cell, never to how far the head has walked.
- **AnnularTape** — a ring-shaped bounded tape (wraps at both ends);
  `AnnularTape::new(size)` takes its size from the caller (2048 is a
  common example size, not a hardcoded default).
- **StrictTape** — a decorator over any tape: marking an already-marked
  cell, or unmarking an already-blank one, is a fault. Default semantics
  are idempotent (repeated marks/unmarks are no-ops) — this is required
  for the cell-state optimizer pass to be legal (`docs/language.md
  (optimization)`); `pmt run --strict-cells` / `pmt compile --strict-cells`
  disables that pass and enables this decorator.

### Loading

`main` is located at **link time**: the linker resolves the `main` symbol
and writes its byte offset into the `.pmx` header's entry field (see
`docs/formats.md`). A `.pmx` carries no symbol table — at run time the
entry point is just that number. Loading:

1. validate magic, CRC-32, format version, and arch byte; select the
   architecture module for that arch byte;
2. copy the code section into read-only code memory;
3. attach the tape (device 0) supplied by the caller (default
   `InfiniteTape`);
4. initialize `IP := entry offset`, the return stack empty, and latch
   `MF := (tape.read() == 1)` (this latch is tact-free — it is loading,
   not execution);
5. verify `code[IP]` is `ent` (a corrupt entry point is rejected before a
   `Machine` exists — `LoadError::EntryNotEntryMarker`, or
   `FormatError::Malformed` if the entry offset itself is out of bounds —
   distinct from the runtime `Trap` taxonomy below, since no instruction
   ever executes);
6. run.

## Instruction set

Byte-addressed, variable-length: 1-byte opcode plus an optional immediate.
**Control flow** — `jmp`/`jmp.s` (unconditional), `jm`/`jm.s`/`jnm`/`jnm.s`
(conditional on MF), and `call`/`call.s`/`ret` — is the family of opcodes
that can move IP anywhere other than the next instruction; everything else
always falls through. Jump and call operands are **IP-relative to the end
of the instruction** — position-independent code, which keeps the linker
to pure layout plus patching.

| Opcode | Mnemonic | Operand | Meaning |
|---|---|---|---|
| `0x00` | — | | invalid → trap |
| `0x01` | `nop` | | no operation |
| `0x02` | `stp` | | stop, normal termination |
| `0x03` | `hlt` | | halt, abnormal termination |
| `0x04` | `lft` | | head left (latches MF) |
| `0x05` | `rgt` | | head right (latches MF) |
| `0x06` | `wr` | symbol vector | write symbol index to the cell (latches MF). In PM-1 always one element: `wr 1` = mark, `wr 0` = blank |
| `0x07` | `wrl` | symbol vector | write symbol index, then head left (latches MF after the move) — a fused `wr`+`lft` |
| `0x08` | `jmp` | rel i32 | unconditional jump |
| `0x09` | `jm` | rel i32 | jump if match (MF = 1) |
| `0x0A` | `jnm` | rel i32 | jump if no match (MF = 0) |
| `0x0B` | `call` | rel i32 | verify target is `ent`, push return address, jump |
| `0x0C` | `ret` | | pop return address, jump |
| `0x0D` | `ent` | | function landing pad; executes as no-op |
| `0x0E` | `brk` | | breakpoint (`debugger` builtin) |
| `0x0F` | `wrr` | symbol vector | write symbol index, then head right (latches MF after the move) — a fused `wr`+`rgt` |
| `0x18` | `jmp.s` | rel i8 | short form of `0x08` |
| `0x19` | `jm.s` | rel i8 | short form of `0x09` |
| `0x1A` | `jnm.s` | rel i8 | short form of `0x0A` |
| `0x1B` | `call.s` | rel i8 | short form of `0x0B` |

This table matches `pm1_syntax()` in `crates/post-machine/src/asm/mod.rs`
entry-for-entry (19 real entries; `0x00` and opcodes `≥ 0x80` are not
table rows — they decode to "invalid" or "reserved").

- **Short-form rule:** `short = far | 0x10`.
- **Additive ISA revision:** `wrl` (`0x07`) and `wrr` (`0x0F`) are the
  first opcodes added after v1 — a fused write-then-move that writes,
  moves the head, and latches MF once after the move, behaving exactly
  like the unfused `wr`; `lft` / `wr`; `rgt` pair it stands in for. Adding
  opcodes is a **minor ISA revision**: they occupy previously-unassigned
  bytes, so a processor built before the revision traps them as invalid
  opcodes, and code that uses them requires a VM that recognizes the
  revision.
- **`ent` verification is always on:** `call`/`call.s` trap
  (`CallTargetNotEntry`) unless the target byte is `0x0D`. Every function
  begins with `ent` — the compiler emits it, and the assembler's `.func`
  directive inserts it. Jumping onto an `ent` is legal (it executes as a
  no-op); only `call` checks.
- Opcodes `≥ 0x80` are reserved for future multi-byte encodings.
- **Width selection:** intra-function jumps are relaxed by the
  assembler/compiler back end (iterate until sizes stabilize). `call`
  width is decided by **linker relaxation**: lay out with far calls, then
  iteratively shrink calls whose targets fit a signed byte (-128..127) to
  `call.s`, re-patching until stable (`pmt link --no-relax` disables this).

## Timing model (tacts)

Deterministic cycle accounting over the buses: each code-bus byte fetched
costs 1 tact; the execute base costs 1 tact per instruction; each stack
word pushed or popped costs 1 tact; device commands cost what the **tape
profile** says — the electronic default is `move/read/write = 1`, and a
`pmt run --tact-profile M,R,W` lets a mechanical profile model a physical
tape's slower motion. The MF latch is honest: every tape instruction pays
its trailing `read()`. A fused write+move (`wrl`/`wrr`) is one instruction,
not two: it pays a single fetch and one trailing MF latch (the `read()`
after the move), skipping the intermediate latch read that the unfused
`wr`; `lft` / `wr`; `rgt` pair pays right after its write.

Examples at the electronic default: `rgt` costs 4 tacts (fetch 1 + execute
1 + move 1 + latch-read 1); `jm` costs 6 vs `jm.s` costs 3 (relaxation is a
real speed win, not just a size win); `call` costs 8 vs `call.s` costs 5
(fetch 5 + the `ent`-verification read 1 + the stack push 1 + execute 1 —
the `ent` check is a real code-bus read at the target address).

**Wait states:** during a device transaction the core stalls — nothing
executes, and the tact counter runs for the device's full price (no
pipeline hides the latency). Accounting splits into *core tacts*
(fetch/execute/stack) and *stall tacts* (waiting on the device); both are
reported in run stats (`RunStats`, via `DebugSession::stats()`).

## Execution

The program starts at the `.pmx` entry point (`main`'s `ent`). Normal
termination is `stp`; abnormal termination is `hlt` (`halt` in the source
language — the first program-initiated abnormal stop this toolchain
lineage has ever had; see `docs/history.md`). A **trap** is the
processor's controlled stop on an execution error: it halts on the
faulting instruction and reports the fault plus a full state snapshot.

Trap causes:

| Trap | Cause |
|---|---|
| `InvalidOpcode` | opcode `0x00` or any undefined byte |
| `CodeOutOfBounds` | a jump, call target, or fetch landed outside the code image |
| `BadOperand` | a malformed operand for the decoded opcode |
| `CallTargetNotEntry` | `call`/`call.s` targeted a byte that is not `ent` |
| `StackOverflow` | `call` on a full return stack |
| `StackUnderflow` | `ret` on an empty return stack |
| `StepLimit` | the configurable step budget (`pmt run --max-steps`, default 10,000,000) was exceeded |
| `TactLimit` | the configurable tact budget (`--max-tacts`) was exceeded |
| `Device` | a device fault — under `--strict-cells`, marking an already-marked cell or unmarking an already-blank one; or a symbol index outside the tape's alphabet |

A non-interactive run reports a trap as a structured `Outcome::Trapped`
result; under the debug API it instead pauses on the faulting instruction.

**Run results** (`RunResult`, returned by `Machine::run`): `outcome`
(`Stopped` / `Halted` / `Trapped(trap)`), `stats` (`steps`, `core_tacts`,
`stall_tacts`, and their sum via `total_tacts()`), `ip` (the address of the
last instruction worked on — the faulting instruction for traps, the
terminating `stp`/`hlt` otherwise), and `stack` (the return stack's
contents at termination, deepest frame first — non-empty on a trap that
occurred inside a call).

### DebugSession

`Machine::debug` opens an interactive `DebugSession` over the same code
image. Depth is just the return stack's depth, so stepping commands are
depth-based: `step_in` executes exactly one instruction; `step_over` runs
until depth returns to at or below where it started (stepping over a
call); `step_out` runs until depth drops below where it started (finishing
the current call); `continue_`/`run_steps` run further, either to
completion/pause or for a fixed instruction budget. Breakpoints are
addresses; a session paused at one is not re-paused by resuming past it.

Pause causes (`PauseCause`): `Step` (a stepping command completed),
`Breakpoint(addr)` (about to execute the instruction at `addr`), `Brk` (a
`debugger`/`brk` instruction just retired), `Manual` (an instruction budget
ran out, the sync analogue of an external pause), and `Trap(trap)` (paused
on the fault, with state still inspectable — any further stepping then
reports the session as finished). `pmt run --trace` drives a `DebugSession`
under the hood and streams one listing line per retired instruction; see
`docs/cli.md`.
