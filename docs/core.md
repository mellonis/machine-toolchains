# The arch-agnostic core

`mtc-core` is the half of this toolchain family that knows no
architecture. It owns the processor VM (the sans-I/O core, the bus
protocol, the driver, the tape devices, the debugger session), the
assembler and disassembler frameworks, the assembly lint layer and
formatter, the linker, the container codecs, and the language-server
framework. Everything instruction-specific arrives from outside through
two tables.

This page documents what the core provides and the contracts it holds
every architecture to. The per-architecture pages document the opcodes
themselves (`docs/pmt/isa.md` for PM-1); the container wire formats and
the assembly text grammar are `docs/formats.md`.

The boundary is a real one, not a convention: the core's own tests run
against a crate-private fake architecture, so anything the core can do
is by construction expressible without naming a real one.

## The architecture contract

An architecture plugs in through two tables.

**`Arch`** supplies execution knowledge:

```rust
trait Arch {
    fn arch_id(&self) -> u8;
    fn operand_kind(&self, opcode: u8) -> Option<OperandKind>;   // None = not ours
    fn lower(&self, opcode: u8, operand: &Operand) -> Result<Vec<MicroOp>, Trap>;
    fn is_entry_marker(&self, byte: u8) -> bool;
}
```

The core fetches a byte, asks the architecture what operand shape
follows it, fetches that, and then executes the **micro-ops** the
architecture lowers the pair into. The core therefore knows no opcodes
at all: it knows `MoveLeft`/`MoveRight`/`Write`/`Read`/`ReadAll`,
`LatchMatch`, `JumpRel`/`JumpRelIf`, `Call`/`Ret`, `MatchTable`/
`DispatchJump`, `CallFrame`/`RetX`, `Raise`, `Stop`/`Halt`/`Brk`/`Nop`.
An unrecognized opcode (`operand_kind` returning `None`) traps.

Operand wire shapes are likewise a fixed vocabulary the architecture
selects from rather than defines: no operand, a signed 8- or 32-bit
code-relative displacement, a self-delimiting symbol or move vector, a
fused write-then-move vector pair, an absolute table-section offset, a
raw 8-bit immediate, or a framed-call pair (displacement plus frame
descriptor offset). Their byte layouts are `docs/formats.md (assembly
text)`.

**`ArchSyntax`** supplies assembly knowledge — see
[The assembler framework](#the-assembler-framework).

## Processor architecture

**Hardware realizability is a design requirement**: every concept here
maps to synchronous digital logic plus a physical tape transport.
Architectural state is fixed-width only (IP is a `u32`, a match register,
a bounded return stack); the stack is SRAM plus a depth counter; code is
ROM; symbols are indices — hardware never sees glyphs; traps are a
fault-code register latched on trap plus a HALT line (the structured
fault value the API returns is that register's software rendering);
nothing in the core assumes an unbounded tape (physical tapes are
bounded).

The core is a Harvard machine with **every memory behind a bus**: it owns
only its registers and the fetch/decode/execute automaton. Code, the
return stack, and the tapes are external components reached through
narrow interfaces (a code bus, a stack bus, and a device bus):

```
┌─ processor core ────────────┐
│  IP, MR, TR, FR             │
│  fetch / decode / execute   │
└──┬──────────┬──────────┬────┘
   │ code bus │ stack bus│ device bus
   ▼          ▼          ▼
 code ROM   stack RAM   tape devices 0..n
 + table    push/pop,   left/right/
   ROM      depth       read/write
 fetch(a)
 → byte
```

The core itself is **sans-I/O**: a pure transition function
`(coreState, busResponse) → (coreState', nextBusRequest)` that emits bus
requests and never performs I/O. A driver executes the requests; v1
ships the synchronous driver (a tight loop over in-memory devices — full
speed, what the tests and both CLIs' `run` use). The core is
unit-testable with no devices at all: feed it responses, assert the
requests it emits.

In the Rust VM: code ROM is a byte slice sized exactly to the image's
code section (operands read as `u32`/`i32`/`i8` in little-endian), with
the table ROM carried alongside it as a second read-only region; the
return stack is a fixed-depth `Vec<u32>` of code offsets (default depth
1024) — full on a call traps overflow, empty on a return traps
underflow; each tape is one of the devices below, reached through the
`Tape` trait.

### Registers

- **IP** — instruction pointer: byte offset of the current instruction
  in the code image. A separate internal latch remembers where the
  instruction being executed *started*, which is what traps and the
  debugger report.
- **SP** — implicit in the return stack's depth: a call pushes, a return
  pops; overflow and underflow are traps.
- **MR** — the match register: `0` means no row matched. The **match
  flag MF** is formally `MR != 0`; a one-bit-flag architecture only ever
  writes 0 or 1 here, while a table-dispatching architecture writes the
  index of the row that matched. Conditional-branch micro-ops test MF,
  never a device directly.
- **TR** — the tuple register: the symbols latched by read micro-ops
  during the current instruction. A match-table walk compares its rows
  against this prefix, and its width is how many tapes were read.
- **FR** — the frame register, under the frames execution profile: `0`
  is the identity composite, a non-zero value is the active composite
  index. A framed call computes `FR' = compose[FR][site]` and activates
  the resolved descriptor; the caller's frame is restored on return. The
  descriptor and directory wire layouts are `docs/formats.md (frame
  descriptors)`.

Besides these, the core has internal buffers no instruction can observe:
the instruction/operand latch staged between fetch and execute. The
debug API may display it; the programming model never depends on it.

### The tape and device bus

The processor never knows a head position — it drives each tape through
the device bus, addressing devices by index. Devices operate on **symbol
indices, not symbols**: the processor is alphabet-agnostic. The actual
glyphs are presentation-layer metadata, supplied by tooling or a loaded
tape block, never by the processor.

```rust
trait Tape {
    fn alphabet_size(&self) -> u32;   // writing an index >= size faults
    fn left(&mut self);
    fn right(&mut self);
    fn read(&self) -> u32;            // index of the symbol under the head
    fn write(&mut self, index: u32) -> Result<(), DeviceFault>;
    fn head(&self) -> i64;            // current head position
}
```

A single-tape architecture is just the one-device case, and a two-symbol
tape just the `alphabet_size() == 2` case.

The async device surface is a poll-shaped mirror of the same contract,
for a caller that cannot afford to block a thread on a slow or
physically remote tape — an embedder drives it as described under
[AsyncSession](#asyncsession):

```rust
trait AsyncTapeDevice {
    fn alphabet_size(&self) -> u32;
    fn head(&self) -> i64;
    fn issue(&mut self, cmd: DeviceCmd);
    fn poll(&mut self) -> DevicePoll;
}
```

`issue` puts one command on the bus — move left, move right, read, or
write, the same four operations `BusRequest`'s device variants already
carry — and each following `poll` samples the READY line on a clock
edge: `Pending` while the device is still working, `Ready` once it
settles, carrying the reply and the transaction's cost. The contract is
**one command in flight per device**: issuing a second command before
the first settles is a caller bug, and polling a device with nothing in
flight simply reports `Pending`. `head()` and `alphabet_size()` report
the device's *settled* state — as of the last completed transaction,
never an in-flight one. One further obligation falls on the caller
rather than the device: **device-set stability** — the same device set
is supplied across every `pump` call, and a device holding an in-flight
command must not be swapped out or dropped mid-transaction, because the
session re-polls the *same* device on resume, not whatever now occupies
that slot.

Two implementations ship. **`SyncAsAsync`** wraps any `Tape` and is
`Ready` on the very next poll, priced at the model cost (`cost: None`)
— the adapter that makes a pumped run of an in-memory program
bit-identical to a synchronous one. **`LatencyTape`** is the shipped
waiting device: it wraps a `Tape` behind a `LatencyProfile` naming, per
operation kind, how many polls to hold READY low and what cost to
report once the operation lands. It is the reference implementation of
the async contract, the test vehicle the pumped session's own suite
runs against, and the transport counterpart of a mechanical
`TactProfile` — where the tact profile prices a device's operations for
the timing model below, `LatencyProfile` prices how long the transport
itself takes to answer.

Shipped tape implementations:

- **InfiniteTape** — unbounded in both directions, two symbols, paged
  sparse storage: a hash map of fixed-size pages, each a `u64` bitmask,
  with the current page cached (the head only ever moves ±1). Reads
  never allocate — a page miss is blank; a write that zeroes its page
  frees it — so memory stays proportional to the number of pages holding
  a non-blank cell, never to how far the head has walked.
- **WideTape** — the same unbounded paged sparse storage for an alphabet
  of up to 256 symbols. A two-symbol band is just a `WideTape` of width
  2, so an architecture with wide alphabets uses this device throughout.
- **AnnularTape** — a ring-shaped bounded tape (wraps at both ends);
  `AnnularTape::new(size)` takes its size from the caller (2048 is a
  common example size, not a hardcoded default).
- **StrictTape** — a decorator over any tape: writing a cell the value it
  already holds is a fault. The default semantics are idempotent
  (repeated identical writes are no-ops), which is what makes a
  cell-state optimizer pass legal; a toolchain that offers strict cells
  disables that pass and wraps the device in this decorator.

A device fault is one of: an index outside the tape's alphabet, a strict
cell violation, or a reference to a device the machine does not have.

### Loading

The entry point is located at **link time**: the linker resolves the
entry symbol and writes its byte offset into the image header (see
`docs/formats.md`). An executable image carries no symbol table — at run
time the entry point is just that number. Loading:

1. validate magic, CRC-32, format version, and arch byte; select the
   architecture module for that arch byte (`LoadError::UnknownArch`);
2. reject an execution profile this VM does not implement
   (`LoadError::UnsupportedProfile`) — the precedence is arch, then
   profile, then entry marker;
3. copy the code section into read-only code memory, and the table
   section, when the image has one, into table ROM;
4. attach the devices supplied by the caller; a multi-tape image
   validates their count and per-tape alphabet cardinalities against its
   own header before running;
5. initialize IP to the entry offset and the return stack empty. MR
   starts at 0; a single-tape image additionally latches the initial
   match from the head symbol (this latch is tact-free — it is loading,
   not execution), while a multi-tape image latches nothing and lets
   head symbols enter through explicit read micro-ops;
6. verify that the byte at the entry offset is the architecture's entry
   marker (`LoadError::EntryNotEntryMarker`, or a malformed-format error
   if the entry offset itself is out of bounds) — a corrupt entry point
   is rejected before a machine exists, distinct from the runtime trap
   taxonomy below, since no instruction ever executes;
7. run.

## Match tables

An image may carry a **table ROM** beside its code: a read-only region
addressed absolutely (not IP-relative), holding match tables and
dispatch tables. The core walks them; what an architecture uses them for
is its own business.

A match table in the compact family is one byte per row position:

```text
offset 0:  width      u8   — positions per row (1..=16)
offset 1:  row_count  u16  LE
offset 3:  rows       row_count × width bytes; each byte is a 7-bit symbol
                      payload; 0x7F = wildcard ("transparent")
```

The walk feeds bytes from the table ROM one at a time and compares each
row against TR, setting MR to the 1-based index of the first row that
matched and 0 when none did. A malformed header (zero or over-wide
width, or a width wider than the latched TR) is a trap, not a panic. A
dispatch table is the companion jump vector: `DispatchJump` indexes it by
MR, which is why an MR of 0 traps rather than dispatching.

**Row discipline** — one width for every row; exact rows (no wildcard)
first, sorted and pairwise disjoint; wildcard rows after in source
order; an all-wildcard catch-all, if present, only last — is enforced by
the assembler, not by the walk. It is a property of first-match
semantics rather than of any one architecture, so the check lives in the
core assembler and every table-carrying dialect inherits it.

Sorted, pairwise-disjoint exact rows mean first-match can never be the
tiebreak between two exact rows, so a table's meaning does not depend on
the order the author happened to write them in; catch-all-last means a
catch-all never shadows a row behind it. How a dialect spells the
resulting error is its own business.

The discipline governs **authored** tables. Tables the linker emits —
mono lowering rewrites rows through a symbol-map preimage and prepends
trap rows — preserve first-match *meaning* rather than source
sortedness.

## Timing model (tacts)

Deterministic cycle accounting over the buses: each code-bus byte
fetched costs 1 tact; the execute base costs 1 tact per instruction;
each stack word pushed or popped costs 1 tact; device commands, table
reads, and frame-descriptor loads cost what the **tact profile** says.
The electronic default prices every one of them at 1
(`TactProfile::ELECTRONIC`: move, read, write, table read, frame load);
a mechanical profile models a physical tape's slower motion. A match
latch is honest: an instruction that reads a device pays for that read.

**Wait states:** during a device transaction the core stalls — nothing
executes, and the tact counter runs for the device's full price (no
pipeline hides the latency). Accounting splits into *core tacts*
(fetch/execute/stack) and *stall tacts* (waiting on a device or the
table ROM); both are reported in run stats (`RunStats`, also available
mid-run via `DebugSession::stats()`), which sum through
`total_tacts()`.

Under the async device surface the same accounting holds with one
refinement: a device may *report* its transaction's cost in tacts — a
real device's wait is real machine time, and in hardware the counter
ticks through a WAIT stall — while a device that reports nothing is
priced at the profile's model cost instead. READY sampling itself is
free: a pending poll never ticks the counter, so stats stay
deterministic no matter how often the embedder pumps.

Each architecture's page works the model through its own opcodes, where
relaxation showing up as a real speed win and not just a size win
becomes visible.

## Execution

The program starts at the image's entry point. Normal termination is the
architecture's stop micro-op; abnormal termination is its halt micro-op.
A **trap** is the processor's controlled stop on an execution error: it
halts on the faulting instruction and reports the fault plus a full
state snapshot.

Trap causes:

| Trap | Cause |
|---|---|
| `InvalidOpcode` | a byte the architecture does not recognize as an opcode |
| `CodeOutOfBounds` | a jump, call target, or fetch landed outside the code image |
| `BadOperand` | a malformed operand for the decoded opcode |
| `CallTargetNotEntry` | a call targeted a byte that is not the entry marker |
| `StackOverflow` | a call on a full return stack |
| `StackUnderflow` | a return on an empty return stack |
| `StepLimit` | the configurable step budget was exceeded |
| `TactLimit` | the configurable tact budget was exceeded |
| `Device` | a device fault: an index outside the tape's alphabet, a strict-cell violation, or no such device |
| `NoTransition` | a dispatch with MR = 0 — no match-table row fired |
| `TableOutOfBounds` | a table walk ran past the table ROM, or its header is malformed |
| `DispatchOutOfRange` | MR indexed past the dispatch table's entries |
| `UnmappedRead` | an explicit trap for a symbol the active frame's map does not carry inward |
| `UnmappedWrite` | the same, outward |
| `ExitOutOfRange` | a multi-exit return named an exit the active frame lacks, or fired with no frame active |
| `ProfileViolation` | an instruction requiring the frames profile ran on a base-profile core |

A non-interactive run reports a trap as a structured trapped outcome;
under the debug API it instead pauses on the faulting instruction.

**Run results** (`RunResult`): `outcome` (`Stopped` / `Halted` /
`Trapped(trap)`), `stats` (`steps`, `core_tacts`, `stall_tacts`), `ip`
(the address of the last instruction worked on — the faulting
instruction for traps, the terminating stop or halt otherwise), and
`stack` (the return stack's contents at termination, deepest frame
first — non-empty on a trap that occurred inside a call).

### DebugSession

`Machine::debug` opens an interactive `DebugSession` over the same code
image (`debug_tapes` for the multi-tape shape, which carries the table
ROM). Depth is just the return stack's depth, so stepping commands are
depth-based: `step_in` executes exactly one instruction (`step_in_tapes`
for a multi-device machine); `step_over` runs until depth returns to at
or below where it started (stepping over a call); `step_out` runs until
depth drops below where it started (finishing the current call);
`continue_`/`run_steps` run further, either to completion/pause or for a
fixed instruction budget. Breakpoints are addresses; a session paused at
one is not re-paused by resuming past it. The session also exposes IP,
MF, FR, depth, the stack, and stats between commands.

Pause causes (`PauseCause`): `Step` (a stepping command completed),
`Breakpoint(addr)` (about to execute the instruction at `addr`), `Brk`
(a **debug break** instruction just retired — see below), `Manual` (an
instruction budget ran out, the sync analogue of an external pause), and
`Trap(trap)` (paused on the fault, with state still inspectable — any
further stepping then reports the session as finished).

**Debug break.** An architecture may declare one opcode as its debugger
break (`ArchSyntax::break_opcode`). It retires like a no-op and pauses a
debug session with cause `Brk`; outside a session it costs a fetch and
an execute base and does nothing else. Because it is a real instruction,
an un-stripped break is an observability barrier no optimizer motion may
cross, and the `leftover-debugger` lint below flags one left in shipped
source. An architecture that declares no break opcode simply never
raises this cause and never fires that rule.

### AsyncSession

`Machine::async_session` opens a pumped `AsyncSession` over the same
code image (`async_session_tapes` for the multi-tape shape, which
carries the table ROM). Where `DebugSession`'s stepping commands block
the calling thread until a device transaction resolves, `AsyncSession`
never blocks: the embedder owns the loop, driving execution by calling
`pump(devices, budget)` repeatedly, and a device that is not yet READY
simply suspends the session between calls instead of the thread.

One `pump` call retires instructions until one of four things happens,
reported as a `PumpEvent`: a device holds READY low (`DeviceWait` —
nothing advanced past it; call `pump` again once it might have
settled); the per-call instruction budget runs out (`BudgetSpent`); a
pause condition fires at an instruction boundary (`Paused(cause)`); or
the program stops, halts, or traps (`Finished(result)`, carrying the
same `RunResult` shape `Machine::run`/`run_tapes` return — a pumped run
over `SyncAsAsync`-wrapped devices reaches the identical result, tact
for tact, as a synchronous run over the same devices). A `budget` of
`None` runs to the next pause or termination in a single call; `Some(0)`
spends immediately and reports `BudgetSpent` without retiring
anything — even when the call would otherwise resume a transaction
that has just turned READY. A device suspension never touches the
budget: `DeviceWait` reports before the budget is even consulted.

This is the session's clock correspondence: in hardware a clock
generator pumps the processor one edge at a time, and a `pump` call is
that edge. The sans-I/O core underneath is a per-tact
`BusRequest`/`BusResponse` state machine no matter which driver serves
it, so the same Rust core doubles as a golden model for a hardware
implementation — clock it the same way and its request/response trace
is what real silicon should produce.

At an instruction boundary the pause checks run in a fixed
priority — **break, then pause, then breakpoint, then budget**. A
retired debug-break instruction reports `Paused(Brk)` before anything
else is even checked; a pending `pause()` request reports
`Paused(Manual)` next and does not re-fire on a later boundary; an
instruction pointer landing on a registered breakpoint address reports
`Paused(Breakpoint(addr))` — resuming past it does not re-pause, since
by the next check the address has already moved on; only once none of
the three apply does the budget get decremented, so an instruction that
exhausts a budget while also matching a higher-priority cause reports
that cause instead, and the budget is left untouched for the next call.
Unlike `DebugSession`, where a `run_steps` budget running out itself
reports as `Paused(Manual)`, `AsyncSession` keeps the two separate — a
spent budget is always its own `BudgetSpent` event, and
`Paused(Manual)` fires only for a genuine external `pause()` call.

`add_breakpoint`/`remove_breakpoint` register and clear breakpoint
addresses; `pause()` requests a pause at the next boundary without
stopping anything itself; `stop()` consumes the session and returns its
final `RunStats`. Between calls the session exposes the same window
`DebugSession` does — `ip()`, `mf()`, `fr()`, `depth()`, `stack()`,
`stats()` — plus `finished()`, which, once a result exists, holds the
full terminal `RunResult` (not just the `Outcome` `DebugSession`'s own
`finished()` reports) and repeats it on every further `pump` call. A
trap is never a pause cause here: it folds straight into a terminal
`Finished`, with the faulting instruction's address in `RunResult.ip`
exactly as the run-results conventions above describe.

The loading step (see [Loading](#loading)) is itself a transaction on
the async path rather than the sync path's direct blocking read: a
single-tape session's first `pump` call issues a real, waitable read on
device 0 and matches it against the mark index before starting —
priced at nothing, since it is loading rather than execution, but
genuinely subject to WAIT, so a slow device 0 delays the first
instruction instead of blocking the embedder's thread. A reply that is
not a symbol — a fault, or a plain acknowledgement — is swallowed and
MF keeps its default; a missing device 0 is likewise treated as
unmarked and execution simply proceeds, the same panic-free choice
`Machine::run` makes for a mismatched device set. The multi-tape shape
(`async_session_tapes`) never latches, mirroring `debug_tapes`: MR
starts at 0 and head symbols enter only through explicit reads.

Two things this surface deliberately does not do. It does not extend
the bus protocol: the device leg gains a poll shape, but the requests
and responses crossing it are the same device commands the synchronous
driver already serves. And a multi-tape read still serializes — an
architecture's `ReadAll` micro-op expands into one `Read` per device in
device order, each issued and settled before the next is issued, never
concurrently, on the async path exactly as on the sync one. Nor does
the session enforce a device timeout: a device that never reports READY
leaves `pump` returning `DeviceWait` forever, and judging that as
"never coming back" — and choosing to stop pumping and drop the
session — is the embedder's call, not the core's.

The `vm` module — `Machine`, `Core`, `DebugSession`, `AsyncSession`, the
devices, the bus types — builds without the standard library: the
crate's `std` feature is default-on but optional, and everything that
needs it (the container codecs, the linker, the assembler and
disassembler frameworks, the language-server framework) stays behind
it. What remains without `std` is exactly the processor VM this section
describes, including the raw-code `Machine::with_arch` constructor that
needs no container parsing — the shape a firmware target embeds
against.

## The assembler framework

The assembler and disassembler are arch-generic: all instruction
knowledge arrives via `ArchSyntax`, and the text grammar they accept is
`docs/formats.md (assembly text)`.

- **Mnemonic table** — one `SyntaxEntry` per opcode: the byte, the
  mnemonic, its operand kind, and its control-flow class. Lookups run
  both ways (by mnemonic when assembling, by opcode when disassembling).
- **Relaxation pairs** — the far and short encodings of one logical
  instruction. The assembler always emits the far form of a call; only
  the linker picks the short one (see [Relaxation](#relaxation)).
- **Entry opcode** — the function landing pad the `.func` directive
  inserts.
- **Break opcode** and **trap opcode** — optional; a dialect that has
  neither simply loses the features that depend on them.
- **Capabilities** (`AsmCaps`) — opt-in grammar extensions, all off by
  default so a classic dialect's acceptance is byte-for-byte unchanged:
  `tables` (`.section` regions with `.row`/`.targets`/`.target`),
  `rept` (`.rept v, lo, hi` … `.endr` with `{expr}` substitution), and
  `vectors` (`[a, *, -, <, >, .]` operand tokens).

### Control flow

Every syntax entry declares a `Flow` class, and every arch-agnostic
consumer arms on that class rather than on a mnemonic:

| Class | Meaning |
|---|---|
| `FallThrough` | control continues at the next instruction |
| `Stop` | execution ends here; there is no successor |
| `Jump` | control transfers unconditionally |
| `Branch` | control transfers or falls through, with no other effect |
| `Call` | control transfers with side effects and comes back |

`Branch` carries a real premise: that the only thing a branch decides is
its successor. An opcode whose branch has effects beyond selecting a
successor must not be classified `Branch` — `Call` is the carve-out for
side-effecting transfer. Recursive-descent disassembly follows these
edges, and the assembly lint rules below arm on them.

### Error codes

Assembly diagnostics are spanned and coded: a span pointing at the exact
offending text, and a stable kebab-case code identifying the kind. The
codes are permanent user-visible identifiers — a CLI brackets them into
every fatal rendering (`FILE:LINE:COL: error: MESSAGE [CODE]`) and
editor integrations match on them. The catalog below is the assembler
framework's full namespace, shared by every dialect; the capability
column names the assembler capability that must be enabled for the code
to be reachable (`—` = reachable in every dialect), and each CLI page
states which capabilities its dialect enables. The `.routine` signature
and `.frame`/`.map`/`.exits` frame-descriptor directive families ride
the tables capability.

| Code | Capability | Trigger |
|---|---|---|
| `syntax` | — | the line does not parse as an instruction, directive, or label — a malformed function header, junk after a modifier, a malformed directive |
| `unknown-mnemonic` | — | the instruction word is not in the dialect's mnemonic table |
| `outside-function` | — | code or a label appears before any `.func` line |
| `duplicate-function` | — | the same function name is declared twice |
| `duplicate-label` | — | the same label is declared twice in one function |
| `unknown-label` | — | a branch or jump names a label the function never declares |
| `bad-operand` | — | an operand does not fit its instruction's shape — wrong count, wrong kind, or a malformed `@name` |
| `short-offset-out-of-range` | — | a short-form target is too far away to encode; use the far form or let the linker relax it |
| `encode-error` | — | an operand encodes to a value the container format cannot represent |
| `raw-line` | — | the line is not assembly-shaped at all — a disassembly listing row or similar text |
| `bad-rept` | rept | a `.rept v, lo, hi` whose bounds describe an empty range (`lo > hi`) |
| `bad-substitution` | rept | a `{expr}` marker in a `.rept` body failed to evaluate — bad grammar, an unknown variable, an unbalanced brace |
| `bad-vector` | vectors | a `[..]` vector operand that does not parse, or carries an element illegal in its context |
| `bad-table` | tables | a section/table structural violation — a table directive outside `.section tables`, a function inside it, an unreferenced or multiply-referenced table |
| `table-discipline` | tables | a match-table discipline violation — exact rows first, sorted and pairwise disjoint; wildcard rows after; an all-wildcard catch-all only last; all rows one width |
| `unknown-table-label` | tables | a table-space label that does not resolve — an operand naming no table, or dispatch targets outside the one owning function |
| `bad-signature` | tables | a `.routine` signature problem — a duplicate directive, no `.func` of its name, tapes outside 1..=16, an alpha list length mismatch, or a function left unsigned in a file that signs any |
| `bad-frame` | tables | a `.frame`/`.map`/`.exits` violation — a duplicate `.map k`, an index at or past the frame arity, an orphan directive with no open `.frame`, a map pair that unpins blank |

The linker and the VM do not participate in this namespace: a link
error renders as a prose message with no bracketed code, and a trap is
a structured outcome, not an error line (the trap table above).

### Assembly lint

The assembly lint layer is arch-agnostic in the same way: control flow
comes from `Flow`, the break instruction from `break_opcode`. Its five
rules therefore apply to every dialect, and each dialect's lint page
documents them in its own vocabulary.

| Code | Arms on |
|---|---|
| `unreachable-code` | an item with no label right after a `Stop` or `Jump` item; a label resets the arm |
| `unused-label` | a label nothing references — no in-function jump or call operand, and, on a dialect with table sections, no dispatch (`.targets`/`.target`) or frame-exit (`.exits`) entry |
| `redundant-jump-to-next` | a `Jump` or `Branch` whose target labels the immediately following item |
| `line-too-long` | a source line over 80 characters (character count, not bytes) |
| `leftover-debugger` | an instruction using the arch's declared break opcode; silent when it declares none |

**Channel discipline.** Lint is a hygiene channel, never an error
channel: a duplicate label, an unknown label, an unknown mnemonic, or a
line that is not assembly-shaped stays a fatal and is reported as one.
The gate is a full assembly of the input, not a partial lowering, which
is what lets fatals that only surface at layout time (label resolution)
reach the caller instead of being silently linted around. Findings are
filtered against an allow-list of codes supplied by the caller.

## The linker

Objects in, one executable image out, in two phases.

### Linking

**Resolution** builds the namespace and walks reachability:

- Duplicate exported symbols across user objects are a link-time error.
- Libraries resolve **first-wins** and may be shadowed by a user object
  defining the same exported symbol — shadowing is an opt-in property of
  exported names.
- A **local** symbol binds directly within its own object and never goes
  through the namespace, so it can neither shadow nor be shadowed. This
  is the linking-visibility rule every source language's private-by-
  default visibility rests on.
- Reachability is a BFS from the entry symbol — `main` by default, or
  whatever a caller's entry override names; a missing entry symbol is an
  error carrying the name that was looked up. Functions the walk never
  reaches are **dropped**, and a dropped function may reference anything
  at all: unresolved references only matter for what survives.
- Under `mono`/`hybrid` this promise is re-checked after the composition
  engine (below) runs: stamping retargets every lowered site to its
  specialized copy, so a generic routine reached before lowering but left
  with no remaining caller afterward is dropped too, exactly as if the
  first BFS had never reached it.

**Name resolution** is also exposed on its own, without layout: a query
answers which symbols the reachability walk reaches — in BFS order,
each paired with which input supplied its winning definition (a user
object or a library, and which one) — and which winning definitions
never got reached. It runs the exact namespace-building and BFS code
path linking does, stopped short of the composition engine, layout,
and relaxation — so its answer is resolution order and reachability
**as of the resolve phase**, before any call-mechanism lowering. Under
`mono`/`hybrid`, a later prune (above) can remove a function this
query reports as reached: it takes no `call_mech` and never runs
stamping, by design — it backs an editor overlay reasoning about
cross-file, exported-symbol visibility, a question the resolve phase
alone answers, not final image membership.

### Relaxation

**Layout** places the surviving functions and patches their call sites.
Call width is decided here, not by the assembler: lay the image out with
far calls, then iteratively shrink every call whose target fits the
short form's signed byte displacement, re-patching until the sizes stop
changing. The fixpoint is monotone and shrink-only, so it terminates;
disabling it (`--no-relax` on both CLIs) leaves every call far. Table
sections are emitted alongside code, with per-function table bases and
dispatch entries rebased through the same offset map, so a relaxation
that moves code keeps table references correct.

### The link report

Every link returns a structured account of what it did, which the CLIs
render under `-v` and libraries never print: the dropped functions, how
many call sites relaxed and how many stayed far, and — where a
composition engine ran — the stamps emitted, the composite count and
compose-matrix size, the stamps and descriptors avoided by interning,
and the trap rows and expanded rows synthesized. The counters are
image-level aggregates:

| field | meaning |
|---|---|
| `dropped` | defined-but-unreachable functions, dropped from the image |
| `relaxed_calls` / `far_calls` | call sites narrowed to the short form, or left far |
| `instantiations` | stamps emitted — one per distinct `(routine, composite)` |
| `composites` | the directory size `K` — distinct composites in the frames region |
| `compose_table_bytes` | the compose matrix size, `(K+1) × S × 2` for `S` sites |
| `dedup_savings` | stamps and descriptors avoided by interning an already-built copy |
| `synthesized_trap_rows` | unmapped-read trap rows prepended to stamped match tables |
| `expanded_rows` | extra match rows from one-way collapse expansion |

Debug names travel out of band in the map sidecar, keeping the image
itself a pure binary.

## The composition engine

An architecture with the frames profile may let a call carry a
**binding** — a declarative caller↔callee tape and symbol
correspondence, recorded on the object rather than resolved by the
assembler (`docs/formats.md (bound calls)` has the operand and the rules
for completing a binding into a pair of symbol maps). The composition
engine is the link-time pass that turns those records into concrete
frames.

It enumerates the finite set of `(routine, composite)` pairs reachable
from the entry — the same breadth-first walk as reachability, now
carrying an active composite that each binding call composes onto — in a
deterministic order, so builds are reproducible. The algebra it composes
with holds three laws the implementation is property-tested against:

- composition is **associative**;
- an **identity composite collapses away** (`E ∘ identity = E`), so a
  binding resolving to a full pass-through lowers to a plain call — the
  callee simply inherits the caller's frame — and rejoins ordinary call
  relaxation;
- **hole sets compose**: the outer holes union the preimages of the
  inner holes. A one-way pair participates in the read direction only,
  and is excluded from the bidirectional bijectivity check.

### Call mechanisms

`LinkOptions::call_mech` selects how a lowered site runs. The three
produce different images from the same objects:

- **mono** compiles for the base profile: it stamps a specialized copy
  of the callee per distinct composite, folding the projection and
  symbol maps into that copy's vectors and match tables. A statically
  known hole keeps the trap taxonomy — an unmapped-read symbol becomes a
  first-match trap row prepended to every match table, and a write with
  no physical image becomes a trap stub. Identical stamps dedup behind a
  digest-suffixed name, `<routine>.<digest8>` — a period, so the name
  re-lexes as ordinary assembly text. A period is legal in a hand-written
  routine name too, so the linker checks every freshly minted stamp name
  against every routine and stamp name already in play for this link and
  refuses with a typed error on a collision, rather than relying on the
  character choice alone to rule one out. A generic routine left with no
  remaining caller once every site retargets to its stamp does not ship
  (the reachability promise above applies after lowering too). Mono
  emits no frames region.
- **frames** compiles for the frames profile: one generic copy of each
  routine, every binding site a framed call, composites resolved through
  the frames region's directory and compose table at run time. A crossed
  hole traps through the descriptor's hole sentinel.
- **hybrid** classifies per site: a completed bijection stamps like
  mono, anything holey or one-way frames. An image with at least one
  framed site carries a frames region; an all-stamped one has none.

All three are **observably equivalent** on the same program and inputs —
same outcome, same final device state, and the **same trap kind** on a
crossed hole or an unmatched read. The fault offset and the tact cost
may differ; the kind never does.

Two restrictions bind the **mono lowering path**. A raw hand-authored
framed call cannot be lowered onto the base profile, which has no
compose machinery to activate a descriptor with. And a holey binding
whose synthesized trap rows would be consumed by a conditional branch
rather than a dispatch jump is refused, since the prepended row could
misroute the branch. Both errors name the offending routine.

`hybrid` inherits those restrictions only where it actually stamps.
Because an identity binding collapses to a plain call, it never seeds a
stamp — so what matters is whether any site is a **non-collapsing**
bijection. Hybrid delegates to the mono path wholesale, restrictions and
all, exactly when at least one bound site is a non-collapsing bijection
and none is holey or one-way. With no such site it is pure frames, and a
raw framed call elsewhere in the image links fine. With both kinds
present it takes the mixed path, where the restrictions bind only the
stamped closure reached from the bijection seeds, not the image at
large.

## The thin-renderer rule

**Library code never prints.** Every stage returns a structured value —
a compile report, an optimizer report, a link report, a run result, a
list of coded diagnostics — and every byte of terminal output is
rendered by a CLI from one of those values. Errors flow as typed values
too, never as text written to a stream from inside a library.

The rule is what keeps the core embeddable: a consumer can drive
`assemble` / `link` / `Machine` / `DebugSession` directly, in-process,
and get exactly what the command-line tools get. It is also why the
language-server framework can share every analysis with the CLI — the
server writes protocol frames on stdio, but nothing beneath it writes
anything at all.
