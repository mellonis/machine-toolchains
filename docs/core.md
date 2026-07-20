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
every fatal rendering and editor integrations match on them — and each
CLI page documents the rendering (`FILE:LINE:COL: error: MESSAGE
[CODE]`) and the catalog its dialect can produce.

### Assembly lint

The assembly lint layer is arch-agnostic in the same way: control flow
comes from `Flow`, the break instruction from `break_opcode`. Its five
rules therefore apply to every dialect, and each dialect's lint page
documents them in its own vocabulary.

| Code | Arms on |
|---|---|
| `unreachable-code` | an item with no label right after a `Stop` or `Jump` item; a label resets the arm |
| `unused-label` | a label nothing in its function references through a jump or call operand |
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
  digest-suffixed name. Mono emits no frames region.
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
