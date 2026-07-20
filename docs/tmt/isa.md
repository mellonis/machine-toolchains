# TM-1 instruction set and processor architecture

TM-1 (`arch` byte `0x02`) is the multi-tape Turing-machine architecture
this toolchain family's second front end targets. Where PM-1 drives one
two-symbol tape and branches on a single mark bit, TM-1 drives up to
**sixteen tapes**, each with its own alphabet, reads all their heads in
one instruction, and branches through **match and dispatch tables**. On
top of that it carries an optional **frames execution profile**: a
call may run its callee under a projection that narrows and renames the
tapes the callee sees, which is what makes a routine written against one
tape alphabet reusable inside a program written against another.

This page covers what TM-1 itself contributes: its processor shape, its
opcode table, how tables and frames execute, how the three call
mechanisms relate, and how a TM-1 program terminates.

The machinery underneath is shared with every other architecture and is
documented once, in `docs/core.md`: the sans-I/O core and its buses, the
device model, loading, the match-table walk, the tact accounting rules,
the trap taxonomy, `DebugSession`, and the link-time composition engine.
The container formats and the `.tma` assembly grammar that carry TM-1
code are `docs/formats.md`; the source language that compiles to it is
`docs/tmt/language.md`; the `tmt` commands that drive it are
`docs/tmt/cli.md`.

## Processor architecture

TM-1 uses the wide corner of the core's model:

- **1..=16 tape devices**, addressed by index, one head each. The count
  is fixed by the image, not by the architecture module — `Tm1::new`
  validates the declared count and then holds no per-machine state at
  all, so every lowering below is width-agnostic.
- **Per-tape alphabets**, index 0 always the blank. Different tapes in
  one machine may have different cardinalities; a routine signature
  requires each to be at least 1 and does not otherwise bound it, but
  the compact operand family below is what limits which symbol indices
  an instruction can actually name.
- **Either execution profile**: base (no frames) or frames. Which one an
  image runs is decided at link time, not by the source.
- A register file of IP, the return stack, MR, TR, and FR.

### Registers

The core's register file (`docs/core.md (registers)`) as TM-1 uses it:

- **IP** — instruction pointer, byte offset into the code image.
- **SP** — implicit in the return stack depth; `call`/`call.m` push,
  `ret`/`retx` pop.
- **MR** — the match register. TM-1 writes it **only from `mtc`**, the
  match-table walk: `mtc` sets MR to the 1-based index of the first row
  that matched every tape, or 0 when none did. `rd`, `wr`, `mov`, and
  `wrmv` leave MR untouched. This is the sharpest departure from PM-1,
  where every tape instruction re-latches the flag: in TM-1 a `jm`/`jnm`
  tests the most recent `mtc` outcome regardless of how much tape motion
  happened in between, and no TM-1 lowering ever emits the core's
  latch-on-tape-op micro-op at all.
- **TR** — the tuple register: the per-tape symbols `rd` latches. Its
  width is how many tapes were read, and a match table's rows are
  compared against it position by position.
- **FR** — the frame register, meaningful under the frames profile: 0 is
  the identity context (the machine's own tapes, no translation) and a
  non-zero value is the active composite index. See
  [The frames execution profile](#the-frames-execution-profile).

A trace line from `tmt run --trace` shows the observable ones —
`MF=<0|1>` (the match flag, formally `MR != 0`), the head positions, and
on a frames-profile image `FR=<n>`.

### Tapes and alphabets

Each TM-1 tape is the core's wide device: unbounded in both directions,
paged sparse storage, symbol **indices** rather than glyphs. Glyphs live
only in a tape-block snapshot and are presentation metadata; the
processor never sees them.

A function's tape count and per-tape alphabet cardinalities come from
its `.routine` signature (`docs/formats.md (assembly text)`). The
**entry** routine's signature is what fixes the executable image's tape
band: the header records the tape count and one cardinality per tape,
and a run validates the supplied tape block against them before
executing anything. A routine body may be authored at a *narrower* arity
than the machine's width — that is exactly what makes it callable under
a frame.

Symbol indices in tables and vectors come from the compact family: one
byte per element, payload `0`..=`0x7E`, with `0x7F` reserved as the
transparent marker (`*` in a match row, `-` in a write vector). That
reservation is the practical alphabet ceiling — an instruction can name
symbol indices 0 through 126 and no others, so a tape whose alphabet runs
wider than that has symbols no `wr` or `.row` can mention. The byte-level
rule is `docs/formats.md (the compact symbol family)`.

### Loading

The general sequence is `docs/core.md (loading)`. TM-1 contributes:

- its entry marker is `ent` (`0x0D`), so an image whose entry offset
  does not point at an `ent` byte is rejected before a machine exists —
  and `call`, `call.s`, and `call.m` all verify the same byte at their
  target;
- a TM-1 image is always the sectioned executable shape, carrying a
  table section (its table ROM) alongside the code;
- **nothing is latched at load.** A multi-tape image starts with MR = 0
  and an empty TR; head symbols enter only through an explicit `rd`.
  PM-1's tact-free initial match latch has no TM-1 counterpart, which is
  why a TM-1 state always begins by reading.

## Instruction set

Byte-addressed and variable-length: a one-byte opcode plus an optional
operand. Jump and call displacements are IP-relative to the end of the
instruction, so code is position-independent and the linker stays pure
layout plus patching. Twenty mnemonics:

| Opcode | Mnemonic | Operand | Meaning |
|---|---|---|---|
| `0x01` | `nop` | — | no operation |
| `0x02` | `stp` | — | stop, normal termination |
| `0x03` | `hlt` | — | halt, abnormal termination |
| `0x04` | `rd` | — | latch every visible head into its TR slot, in one instruction |
| `0x05` | `mtc` | table | walk a match table against TR; set MR |
| `0x06` | `djmp` | table | dispatch: jump through the table indexed by MR |
| `0x07` | `wr` | symbol vector | write one symbol per tape (`-` keeps a cell) |
| `0x08` | `jmp` | rel i32 | unconditional jump |
| `0x09` | `jm` | rel i32 | jump if the last `mtc` matched a row (MR ≠ 0) |
| `0x0A` | `jnm` | rel i32 | jump if the last `mtc` matched no row (MR = 0) |
| `0x0B` | `call` | rel i32 | verify target is `ent`, push return address, jump |
| `0x0C` | `ret` | — | pop return address, jump |
| `0x0D` | `ent` | — | function landing pad; executes as a no-op |
| `0x0E` | `brk` | — | debugger break |
| `0x0F` | `mov` | move vector | move one step per tape (`<` left, `>` right, `.` stay) |
| `0x11` | `trap` | `#kind` | raise a typed trap: `#0` unmapped-read, `#1` unmapped-write |
| `0x12` | `wrmv` | write + move vectors | fused write-then-move: all writes, then all moves |
| `0x13` | `call.m` | framed call | call a routine and activate a frame for it |
| `0x14` | `retx` | `#k` | multi-exit return — leave the active frame through exit `k` |
| `0x1B` | `call.s` | rel i8 | short form of `0x0B` |

This table matches `tm1_syntax()` in
`crates/turing-machine/src/asm/mod.rs` entry for entry. Notes:

- **Short-form rule:** `short = far | 0x10`. Only `call` has a short
  form. `call.s` exists for disassembly and link-time relaxation only —
  the assembler always emits far `call` and rejects `call.s <target>` in
  source, because call width is a linker decision (`docs/core.md
  (relaxation)`).
- **`0x10` is an unused gap**, and any byte the table does not name
  traps as an invalid opcode.
- **`brk` is TM-1's declared debug break** (`docs/core.md (debug
  break)`): it retires as a no-op, pauses a debug session, and is what
  the `leftover-debugger` lint looks for.
- **`ent` verification is always on** for every call form, including
  `call.m`.

### Reading, writing and moving: the vectors

TM-1's tape instructions are **batched across tapes**. This is not an
optimization but the machine model: a Turing machine's step reads all
heads, writes all cells, and moves all heads as one transition, and the
instruction set says so directly.

- **`rd`** takes no operand and latches *every visible head* into TR in
  one instruction. "Visible" is the machine's own width under the
  identity frame, or the active frame's arity under a framed call — the
  lowering is a single width-agnostic micro-op that expands at execution
  time, so the same routine body reads correctly at either width.
- **`wr [..]`** takes one element per tape, left to right: a symbol
  index writes that tape's cell, and `-` **keeps** it untouched. An
  all-keep vector does no work at all.
- **`mov [..]`** takes one element per tape: `>` steps that head right,
  `<` left, `.` stays. An all-stay vector does no work.
- **`wrmv [w…], [m…]`** fuses the pair. The two vectors share one
  arity, and **all writes precede all moves** — so a tape's write lands
  on the cell the head is standing on before that head moves off it.
  It is behaviourally exactly `wr [w…]` followed by `mov [m…]`, in one
  instruction and one fetch.

`wrmv` is the shape a compiled rule's action takes: the `.tmc` front end
emits one `wrmv` per conditional rule, and elides it entirely when the
action is all-keep and all-stay. A hand-written `wr`/`mov` pair remains
equally valid — `wrmv` is a fused spelling, not a new capability.

The element vocabularies and their byte encodings are `docs/formats.md
(vector operands)`.

### Match and dispatch

A TM-1 state is `rd` / `mtc` / `djmp`: read every head, find which rule
fires, jump to that rule's body.

**`mtc <table>`** walks a match table — a labelled run of `.row`
directives, one vector per row — comparing each row against TR position
by position, where a symbol index is an exact match on that tape's head
and `*` matches anything. It sets MR to the **1-based index of the first
row that matched every tape**, or 0 when no row did. The walk itself is
the core's (`docs/core.md (match tables)`); what TM-1 adds is that this
is the *only* thing that writes MR.

**`djmp <table>`** indexes a dispatch table by MR: MR = 1 selects the
first target, MR = 2 the second, and so on.

**Table discipline** — the ordering and width rules a match table's rows
must satisfy — is enforced by the assembler, not by the walk; the rules
and what they buy are `docs/core.md (match tables)`. In TM-1 a violation
is a fatal assembly error under the code `table-discipline`, not a lint
finding:

```
error: exact rows must be sorted and pairwise disjoint [table-discipline]
error: the all-wildcard row must be last [table-discipline]
```

**When nothing matches**, MR stays 0, and what happens next is the
program's choice:

- a following `djmp` on MR = 0 traps `NoTransition` — so **omitting the
  catch-all buys an invalid-symbol fault for free**, with no code to
  write;
- a following `jnm` branches instead, which is how a program handles the
  unmatched case itself.

Both are load-bearing idioms. A dispatch table shorter than the MR it is
handed traps `DispatchOutOfRange` — a table with rows the dispatch has no
targets for is caught at run time, not silently.

One caveat worth carrying into the next section: the discipline governs
**authored** tables. The linker's mono lowering (below) emits rows that
preserve first-match *meaning* rather than source sortedness.

## The frames execution profile

A **frame** is a projection: it narrows the machine's tapes to the
callee's and, per tape, renames the symbols the callee reads and writes.
It is what lets a routine written against a three-symbol bare alphabet
run unchanged inside a program whose tape carries five symbols.

Under the frames profile the processor holds two extra pieces of state:
**FR**, the active composite index, and a **frame cache** holding the
decoded descriptor FR names. Everything a framed callee does goes
through that cache:

- a **tape index** is virtual — virtual tape `k` addresses the physical
  tape the descriptor's entry `k` names, and an index past the
  descriptor's arity is a malformed operand;
- a **read** maps the physical symbol inward through that tape's read
  map;
- a **write** maps the virtual symbol outward through its write map;
- an **empty map is the identity**, so an unmapped-but-present tape
  costs nothing;
- a **map hole** — an entry the map does not carry, or one past its end
  — traps: `UnmappedRead` inward, `UnmappedWrite` outward.

Under FR = 0 every one of those is a pass-through, which is why a
base-profile image and a frames-profile image running an unframed call
behave identically.

The cache is filled **once per call**, from a fixed-size descriptor
read. Nothing is walked per tape access and nothing is re-derived per
nesting level, so translation is **O(1) at any call depth**. The frame
stack exists only to restore FR on return, never to translate.

### Framed calls

`call.m <target>, <frame>` calls `<target>` and activates a frame for
the duration of the call. Its operand pairs the call displacement with a
**site index** — a dense per-image number for this call site. The site
index is deliberately *not* a descriptor address: at run time the
processor performs one compose lookup and one directory read,

```
FR'        = compose[FR][site]     ; the composite active for this call
descriptor = directory[FR' - 1]    ; its offset in the table section
```

then loads that descriptor into the frame cache. The indirection is the
whole point: **the same instruction resolves to a different frame
depending on the context it is reached in**, which is what lets one
generic copy of a routine serve every caller. A hand-authored `call.m`
names one fixed descriptor, so its compose column is constant across
every row; a call the toolchain derived from a declarative binding may
resolve differently per calling context through the identical
instruction.

On return, `ret` and `retx` pop the caller's FR from the frame stack and
reload its descriptor through the directory. A plain `ret` skips that
reload when the call did not change the frame — the cache already holds
the right descriptor; `retx` always reloads a non-identity caller frame,
and restoring FR = 0 simply empties the cache. The region holding the
directory and the compose matrix is `docs/formats.md (frames region)`;
the descriptor's own byte layout is `docs/formats.md (frame
descriptors)`.

Both instructions are profile-gated: `call.m` or `retx` on a
base-profile image traps `ProfileViolation` rather than misbehaving.

### Multi-exit returns

`retx #k` leaves the active frame through **exit `k`** of its exit
vector. Unlike `ret`, the pushed return address is discarded and control
resumes at the caller-side label recorded as exit `k` — so one callee
can report *which* of several outcomes it reached without a shared
convention for encoding the answer on a tape.

The exit vector belongs to the frame being left, and is read from the
current cache before anything pops. A `k` past the vector, or a `retx`
with no frame active, traps `ExitOutOfRange`.

### Explicit traps

`trap #kind` raises a typed trap directly: `#0` is unmapped-read, `#1`
is unmapped-write. Numeric kinds leave room for named kinds later
without a grammar break; any other kind is a malformed operand.

The instruction exists so a *statically* known map hole costs nothing at
run time. Where the frames profile discovers a hole dynamically by
crossing a sentinel in the descriptor, a stamped copy of the same
routine knows at link time that a symbol has no image and can branch
straight to a `trap` stub — and both raise the same kind, which is what
keeps the mechanisms below interchangeable.

## Call mechanisms

A TM-1 program does not normally hand-author frames. It writes a
**binding call** — `call <target> [<binding>]` in `.tma`, or a
cross-alphabet `call` in `.tmc` — which names the caller↔callee tape and
symbol correspondence inline and leaves the toolchain to derive the
frame. The operand's grammar and the rules for completing a binding into
a map (identity completion on equal-size alphabets, closed on unequal
ones) are `docs/formats.md (bound calls)`.

Binding calls are **lowered at link time** by the core's composition
engine, and `tmt link --call-mech mono | frames | hybrid` (default
`hybrid`) selects the lowering. The three mechanisms, the algebra behind
them, the equivalence contract they hold to, and the restrictions that
bind the mono path are all `docs/core.md (the composition engine)`. What
matters at the TM-1 level is that they are **link-time choices over one
program**, not three languages: nothing in a `.tmc` or `.tma` source
selects a mechanism, and the same objects link under all three.

What the choice looks like on a TM-1 image. Linking one cross-alphabet
program — a five-symbol delimited caller running a three-symbol bare
callee through a binding that collapses two boundary markers onto the
callee's blank — three ways:

```
mono     1 stamp,   0 composites,  0 B compose table,  2 expanded rows
frames   0 stamps,  1 composite,   4 B compose table
hybrid   0 stamps,  1 composite,   4 B compose table
```

All three stop with the identical final tape and head position, in the
same number of steps but not the same number of tacts — frames pays for
descriptor loads that mono folded away at link time. Made holey by
dropping one pair from the binding, all three trap `UnmappedWrite` — at
different addresses, since the faulting instruction is a different
instruction in a different body.

Both shapes are legible in a disassembly. The **frames** image shows the
binding call rewritten to `call.m bare, F0` against a derived descriptor,

```asm
F0:     .frame  tapes=(0)
        .map    0, rmap=(1->1, 2->2, 3->0, 4->0), wmap=(1->1, 2->2)
; frames: 1 composite(s), 1 site(s)
;   C1: bare@[0{1->1,2->2,3=>0,4=>0}]
```

— note that the one-way pairs appear in the read map and *not* in the
write map, which is the one-way rule made concrete. The **mono** image
instead shows a digest-named copy (`bare$513e6968`) whose match-table
rows have been rewritten through the read map's preimage: the callee's
single blank row expands into one row per caller symbol that reads as
blank, which is where the `expanded_rows` counter comes from. Where a
caller symbol has no image at all, a synthesized trap row is prepended
ahead of everything else — which is the concrete case behind the caveat
above that the row discipline governs authored tables only.

## Timing model (tacts)

The accounting rules are `docs/core.md (timing model)`: a fetched code
byte costs 1 tact, the execute base 1, each stack word 1, and device
commands, table reads, and frame-descriptor loads whatever the tact
profile prices them at. `tmt run` always runs the electronic default,
where every one of those is 1 — TM-1 exposes no mechanical-profile flag.

What TM-1 adds is which of those knobs its instructions actually reach:

- **`rd` pays one device read per visible tape**, so a state's read cost
  scales with the machine's width — or, inside a frame, with the frame's
  arity rather than the machine's. A narrow callee under a frame reads
  fewer tapes than its caller does.
- **`mtc` and `djmp` pay table reads**, byte at a time, and a match walk
  is priced for the bytes it actually touches — a row that fails on tape
  0 costs less than one that fails on the last tape.
- **`call.m` pays a frame load**: the compose entry, the directory
  entry, and the descriptor's bytes. This is the frames profile's whole
  price, and it is paid per call rather than per tape access. `ret` and
  `retx` pay it again to restore a non-identity caller frame.
- **`wrmv` is one instruction, not two**: it pays a single fetch and a
  single execute base where the `wr`; `mov` pair pays two of each, with
  identical device costs.

The stall side of the accounting is where a frames-profile image differs
most visibly from a stamped one: in the run compared above, mono spent
46 stall tacts against frames' 70, on identical device work — the
difference is entirely descriptor loading.

## Execution

A program starts at the image's entry point (`main`'s `ent` by default,
or whatever `tmt link --entry` named). Normal termination is `stp`;
abnormal termination is `hlt`. A **trap** is the processor's controlled
stop on an execution error, reported as a structured trapped outcome by
a plain run and as a pause on the faulting instruction under the debug
API. `tmt run` exits **0** on `stp`, **2** on `hlt`, **3** on a trap.

TM-1 can raise these of the core's traps (`docs/core.md (execution)` has
the full taxonomy):

| Trap | Cause in TM-1 |
|---|---|
| `InvalidOpcode` | `0x00`, the `0x10` gap, or any byte the mnemonic table does not name |
| `CodeOutOfBounds` | a jump, call target, or fetch landed outside the code image |
| `BadOperand` | a malformed operand: a `wr`/`mov`/`wrmv` vector that is empty, over 16 wide, or mismatched between the two `wrmv` groups; a write payload above `0x7F` or a move code above `>`; a `trap` kind other than 0 or 1; a virtual tape index past the active frame's arity; a `call.m` site past the compose table's columns |
| `CallTargetNotEntry` | `call`, `call.s`, or `call.m` targeted a byte that is not `ent` |
| `StackOverflow` / `StackUnderflow` | a call on a full return stack; a return on an empty one |
| `StepLimit` / `TactLimit` | the `tmt run --max-steps` / `--max-tacts` budget was exceeded |
| `Device` | a symbol index outside that tape's alphabet, or a reference to a device the machine does not have |
| `NoTransition` | a `djmp` with MR = 0 — no match-table row fired |
| `TableOutOfBounds` | a match walk or descriptor load ran past the table ROM, or a table header is malformed |
| `DispatchOutOfRange` | MR indexed past the dispatch table's entries |
| `UnmappedRead` / `UnmappedWrite` | a crossed map hole under a frame, or an explicit `trap #0` / `trap #1` |
| `ExitOutOfRange` | `retx #k` named an exit the active frame lacks, or fired with no frame active |
| `ProfileViolation` | `call.m` or `retx` ran on a base-profile image |

Unlike PM-1, TM-1 can raise every trap in the taxonomy: it carries a
table ROM, and it may run either profile.

Stepping, breakpoints, and pause causes are `docs/core.md
(DebugSession)`. `tmt run --trace` drives one under the hood and streams
one listing line per retired instruction, each carrying the post-state
match flag, head positions, and — on a frames-profile image — FR; see
`docs/tmt/cli.md`.
