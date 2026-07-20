# PM-1 instruction set and processor architecture

PM-1 (`arch` byte `0x01`) is the Post-machine architecture v1 ships. This
page covers what PM-1 itself contributes: its processor shape, its
opcode table, what its instructions cost, and how a PM-1 program
terminates.

The machinery underneath is shared with every other architecture and is
documented once, in `docs/core.md`: the sans-I/O core and its buses, the
device model, loading, the tact accounting rules, the trap taxonomy, and
`DebugSession`. The container formats that carry PM-1 code
(`.pmo`/`.pmx`/`.pmt`/`.pma`) are `docs/formats.md`; the source language
that compiles to it is `docs/pmt/language.md`; the `pmt` commands that
drive it are `docs/pmt/cli.md`.

## Processor architecture

PM-1 is the small corner of the core's model: **one tape device**
(device 0), **two symbols** (0 = blank, 1 = marked), the base execution
profile — no table ROM, no frames — and a register file of IP, the
return stack, and a one-bit match flag.

### Registers

- **IP** — instruction pointer: byte offset of the current instruction
  in the code image.
- **SP** — implicit in the return stack's depth: `call` pushes, `ret`
  pops; overflow and underflow are traps.
- **FLAGS** — bit 0 is **MF** (match flag). Every tape instruction
  (`lft`, `rgt`, `wr`, `wrl`, `wrr`) latches `MF := (tape.read() == 1)`
  after acting — for the fused write+move opcodes that read happens
  after the move — and MF is also latched once before the very first
  instruction runs, from the initial tape state. `jm`/`jnm` test MF,
  never the tape directly. Other bits are reserved and read as 0.

PM-1 is the one-bit-flag case of the core's match register: it only ever
writes 0 or 1 there, and matches against the mark index.

### The tape

PM-1's tape is the two-symbol case of the core's device model:
`alphabet_size() == 2`, index 0 blank, index 1 marked. The language's
`mark`/`unmark` compile to `wr 1`/`wr 0`; the ISA itself has no
mark/unmark concept, only `wr` and the MF latch it triggers.

The default device is `InfiniteTape` — unbounded in both directions,
paged sparse storage. `pmt run --strict-cells` (and `pmt compile
--strict-cells`) instead wraps the tape in `StrictTape`, making it a
fault to mark an already-marked cell or unmark an already-blank one.
Default semantics are idempotent, which is what the cell-state optimizer
pass depends on (`docs/pmt/language.md (optimization)`), so the strict
flag disables that pass in the same breath.

### Loading

The general loading sequence is `docs/core.md (loading)`. PM-1
contributes two specifics: its entry marker is `ent` (`0x0D`), so a
`.pmx` whose entry offset does not point at an `ent` byte is rejected
before a machine exists; and its single-tape image latches the initial
MF from device 0's head symbol as a tact-free loading step.

## Instruction set

Byte-addressed, variable-length: 1-byte opcode plus an optional
immediate. **Control flow** — `jmp`/`jmp.s` (unconditional),
`jm`/`jm.s`/`jnm`/`jnm.s` (conditional on MF), and `call`/`call.s`/`ret`
— is the family of opcodes that can move IP anywhere other than the next
instruction; everything else always falls through. Jump and call
operands are **IP-relative to the end of the instruction** —
position-independent code, which keeps the linker to pure layout plus
patching.

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
- **`brk` is PM-1's declared debug break** (`docs/core.md (debug
  break)`): it retires as a no-op, pauses a debug session, and is what
  the `leftover-debugger` lint looks for.
- Opcodes `≥ 0x80` are reserved for future multi-byte encodings.
- **Width selection:** intra-function jumps are relaxed by the
  assembler/compiler back end (iterate until sizes stabilize). `call`
  width is decided by **linker relaxation**: lay out with far calls, then
  iteratively shrink calls whose targets fit a signed byte (-128..127) to
  `call.s`, re-patching until stable (`pmt link --no-relax` disables
  this; `docs/core.md (relaxation)`).

## Timing model (tacts)

The accounting rules are `docs/core.md (timing model)`: a fetched code
byte costs 1 tact, the execute base 1, each stack word 1, and device
commands whatever the tact profile prices them at — the electronic
default is `move/read/write = 1`, and `pmt run --tact-profile M,R,W`
lets a mechanical profile model a physical tape's slower motion.

What PM-1 adds is what its own instructions cost. The MF latch is
honest: every tape instruction pays its trailing `read()`. A fused
write+move (`wrl`/`wrr`) is one instruction, not two: it pays a single
fetch and one trailing MF latch (the `read()` after the move), skipping
the intermediate latch read that the unfused `wr`; `lft` / `wr`; `rgt`
pair pays right after its write.

Examples at the electronic default: `rgt` costs 4 tacts (fetch 1 +
execute 1 + move 1 + latch-read 1); `jm` costs 6 vs `jm.s` costs 3
(relaxation is a real speed win, not just a size win); `call` costs 8 vs
`call.s` costs 5 (fetch 5 + the `ent`-verification read 1 + the stack
push 1 + execute 1 — the `ent` check is a real code-bus read at the
target address).

## Execution

The program starts at the `.pmx` entry point (`main`'s `ent`). Normal
termination is `stp`; abnormal termination is `hlt` (`halt` in the source
language — the first program-initiated abnormal stop this toolchain
lineage has ever had; see `docs/history.md`). A **trap** is the
processor's controlled stop on an execution error, reported as a
structured trapped outcome by a plain run and as a pause on the faulting
instruction under the debug API.

PM-1 can raise these of the core's traps (`docs/core.md (execution)` has
the full taxonomy):

| Trap | Cause in PM-1 |
|---|---|
| `InvalidOpcode` | opcode `0x00` or any undefined byte |
| `CodeOutOfBounds` | a jump, call target, or fetch landed outside the code image |
| `BadOperand` | a malformed operand for the decoded opcode |
| `CallTargetNotEntry` | `call`/`call.s` targeted a byte that is not `ent` |
| `StackOverflow` | `call` on a full return stack |
| `StackUnderflow` | `ret` on an empty return stack |
| `StepLimit` | the step budget (`pmt run --max-steps`, default 10,000,000) was exceeded |
| `TactLimit` | the tact budget (`--max-tacts`) was exceeded |
| `Device` | under `--strict-cells`, marking an already-marked cell or unmarking an already-blank one; or a symbol index outside the tape's alphabet |

The table-, frame-, and profile-related traps cannot arise: PM-1 images
carry no table ROM and run the base profile.

Stepping, breakpoints, and pause causes are `docs/core.md
(DebugSession)`. `pmt run --trace` drives one under the hood and streams
one listing line per retired instruction; see `docs/pmt/cli.md`.
