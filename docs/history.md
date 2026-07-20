# History and lineage

This repository holds two toolchains, and they do not share a past.

The Post-machine half finishes work started across four Delphi
implementations of a Post machine, built between 2002 and 2012. None of the
four, on its own, was a complete toolchain: one generation had a language
with no code generator behind it, one had a code generator with no real
source language in front of it, and one was a machine with no compiler at
all. This project closes that triangle, and adds the piece none of the four
ever attempted: a linker, with separate compilation and libraries.

The Turing-machine half has **no Delphi ancestor**. Its lineage runs
through a JavaScript library generation instead, and through one problem
the 2007 Delphi generation identified and left unsolved — see **The
Turing-machine side**, below.

## What each generation contributed

**Generation A, 2002 — `Compiller/`.** The direct ancestor of `.pmc`: a
Pascal-era `.pms` source language, compiled by the original `Compiller`
project. Two of its programs, `Sum.pms` and `Ty.pms`, are ported verbatim
(modulo syntax) as this project's golden tests — see below. The
`.pms → .pmc` line is the language lineage this toolchain continues most
directly: `check`/`goto`/labelled successors, comma-groups, and the
builtin vocabulary (`left`/`right`/`mark`/`unmark`) all descend from this
generation's surface syntax. Erratum: the frozen spec dates `Compiller` to 2012; the project is from 2002 (generation A). The freeze preserves the error; this page corrects it.

**The fall-through optimization.** The 2007 generation (`Old Test-PostMachine`'s compiler) added the optimization this toolchain inherits: a layout rule this toolchain still honors as an invariant,
active even at `-O0`: an unconditional jump to the instruction that is
already physically next is never emitted. PM-1's code generator lays out
basic blocks in an order chosen specifically so the common case falls
through instead of jumping (`docs/pmt/language.md (optimization)`,
`crates/post-machine/src/codegen.rs`) — a size-and-speed win the Delphi
lineage discovered and this toolchain keeps as a baseline, not an
opt-in pass.

**2007 — `Old Test-PostMachine`.** A code generator without a matching
source language of its own: it emitted machine code and supported calls
through a return-address stack, but that stack had no equivalent of PM-1's
mandatory `ent` check (`docs/pmt/isa.md`) — nothing verified that a call
target was actually the start of a function before jumping to it. PM-1's
`ent`-verification-always-on rule (every `call`/`call.s` traps unless the
target byte is an `ent`) is this toolchain's answer to that gap: call
safety the 2007 generation's stack never had.

**`PMProcessor`, 2012.** A machine with no compiler of its own: programs were
inspected and understood primarily through disassembly, not written
against a high-level source language — the disassembler-first mindset
this toolchain's own `pmt dis` (with its recursive-descent function
discovery, `docs/formats.md (assembly text)`) is a direct descendant of.
`PMProcessor`'s `TPostMachineProcessor`/`TBelt` split — the processor
talking to the tape only through a narrow interface, never touching it
directly — is the ancestor of PM-1's bus architecture (`docs/pmt/isa.md`),
generalized so that EVERY memory, not just the tape, sits behind a bus.
Its continuation-bit idea for encoding multi-byte opcodes is why PM-1
reserves opcodes `≥ 0x80` for exactly that future use, unused in v1. Every
Delphi generation also carried a step-cap guard against runaway
loops — the direct ancestor of `pmt run`'s `--max-steps` default of
10,000,000 (`docs/pmt/cli.md`).

**Generation D.** The lineage's most ambitious instruction design added an
`AF`/`BF`/`EF` flags trio and an `ja`/`jb`/`je` jump family: edge and
topology conditionals meant to let a program branch on whether the head
had reached an edge, boundary, or interior position of a *bounded* tape.
They were never actually wired up — the routine responsible for updating
them (`UpdateFLAGS`) was a stub that never ran. This toolchain supersedes
that whole approach rather than reviving it: PM-1 has no bounded tape by
default (`InfiniteTape`, `docs/pmt/isa.md`), an out-of-alphabet or otherwise
invalid device access is a `DeviceFault` trap rather than a flag a program
must remember to check, and — per the device-agnostic principle
(`docs/pmt/isa.md`, the processor never knows the head position or the tape's
topology) — any edge behavior a *bounded* tape does need is the device's
problem to enforce, never an instruction's.

## The Turing-machine side

No Delphi generation implemented a Turing machine, so TM-1 and `tmt` have
no ancestor of the kind the four Post-machine generations gave `pmt`. The
record for this half is thinner, and the honest statement of it is short:
one unsolved problem carried over from the Delphi era, one library
generation in JavaScript, and one program written before the architecture
existed.

**The 2007 open problem.** The design record preserved with this project
(below) lists, among its notes for a future Turing architecture, one
problem attributed to the 2007 generation and left unsolved there: given
the tuple of symbols read from several tapes at once, find the matching
transition efficiently — with wildcard patterns in the mix — and hold the
transition table in a structure that makes that search cheap. It is the
one question a multi-tape machine cannot avoid and a single-tape Post
machine never has to ask.

TM-1's answer moves the work off the runtime entirely. A conditional state
compiles to a **match table** of symbol-tuple rows and a **dispatch table**
of jump targets; `mtc` walks the rows and leaves the 1-based index of the
first matching row in `MR`, and `djmp` jumps through the dispatch table
indexed by it (`docs/tmt/isa.md`). Both tables are link-time data, laid out
by the linker into a read-only table ROM carried beside the code
(`docs/core.md`, `docs/formats.md (executable image)`) — not a structure the
machine builds or searches at run time. The cost of organizing the
transitions is paid once, at link time, and the dispatch half of the 2007
question reduces to an indexed load.

**The JavaScript library generation.** The direct ancestry of this half is
the `turing-machine-js` project — a sibling library, related thematically,
not technically: nothing here depends on it at run time. Three of its ideas
survive, each changed by the move from a library to a machine:

- *Symbols as indices.* Its alphabet abstraction encoded symbols as indices
  and kept glyphs at the presentation layer. TM-1 applies the same
  separation at the hardware boundary instead: the processor addresses tape
  cells by index and never sees a glyph at all, which is why glyphs live
  only in tape blocks (`docs/formats.md (tape-block snapshot)`).
- *Composition by halt-state override.* That library composed machines by
  wrapping a sub-machine so that reaching its halt state meant returning to
  the caller. TM-1 splits the same idea in two, because a toolchain can
  afford what a library could not: `graft` matches it by continuation,
  splicing a copy with the caller's exits wired in, and `call` matches it by
  body, sharing one copy and letting a real return stack do what chains of
  wrappers did (`docs/tmt/language.md`). Neither form needs the wrapper
  memoization and chain-collapsing the library approach required.
- *Failing on an unexpected symbol.* Where the library threw, TM-1 traps: a
  `djmp` on `MR = 0` raises `NoTransition`. The consequence is that a match
  table without a catch-all row buys invalid-symbol detection for free,
  with no code emitted to check for it.

The standard library carries the same descent: `std::binaryNumbers` and
`std::binaryNumbersBare` are ports of that project's two binary-number
libraries, keeping the delimited and bare representations side by side so
the trade-off between them stays visible (`docs/tmt/stdlib.md`).

**The program that came before the machine.** `docs/examples/brainfuck-utm.tma`
is a four-tape universal Turing machine that interprets brainfuck from its
program tape. It was written against a *speculative* TM-1 — hand-authored
assembly for an architecture that did not exist yet — as a stress test of
the design, and it forced four decisions that shipped:

1. **The wildcard costs an alphabet slot.** Reserving one byte value as
   "any symbol" caps the compact symbol encoding's alphabet, so the UTM's
   byte-wide tapes had to fit under that cap (`docs/formats.md (the compact
   symbol family)`).
2. **A write vector needs a keep marker.** Writing all heads in one
   instruction is useless without a per-position "leave this one alone"
   value — the write vector's `-`, sibling of the pattern wildcard.
3. **The assembler needs repetition macros.** Table-driven states are
   inherently one row per symbol value; `.rept` keeps the source compact
   while the table stays the size a Turing machine must pay for.
4. **Unconditional states are free.** A state whose only rule matches every
   tape needs no match table and no dispatch: it lowers to straight-line
   write/move/jump, and a chain of them collapses further under the
   fall-through rule the Post-machine side contributed.

It is no longer speculative. The example assembles, links, and runs, and it
is exercised as a golden program by the test suite — which is what turned
the four findings above from design intentions into shipped behaviour.

**The architecture was planned before it was built.** The frozen design
record for the Post-machine toolchain already reserved the architecture
byte `0x02` for TM-1, named `tmt` as its front end, and fixed the sharing
contract that TM-1 would supply only a language front end, an architecture
module, and a thin CLI — importing the VM core, the container formats, the
linker, and the assembler frameworks unchanged. That contract held. What
TM-1 imports rather than reimplements is documented on its own page
(`docs/core.md`), and the arch-agnostic core still carries no knowledge of
either architecture: it is tested against a fake one.

## Abnormal-stop lineage

Every prior generation in this family — the 2007 and 2012 Delphi
implementations, and both JavaScript library generations
(`@post-machine-js/machine` and `@turing-machine-js/machine`) — had only
one kind of stop: normal termination. In `turing-machine-js` specifically,
reaching its halt state from inside a subroutine just means RETURN from
that subroutine, not an abnormal end; genuinely abnormal endings (a
runaway step count, an invalid transition) surfaced as host JavaScript
exceptions, not as a distinct machine-level outcome.

PM-1's `hlt` is the first program-initiated abnormal stop anywhere in this
lineage. It exists because of this project's hardware-realizability
requirement (`docs/pmt/isa.md`): a real machine has a fault-code register and
a HALT line, and `hlt` is a program deliberately asserting that same fault
path itself, as opposed to a trap the processor raises involuntarily. A
matching `abortState` sentinel has since shipped in the
`turing-machine-js` library (v7.1.0), prompted directly by this
toolchain's `stp`/`hlt` distinction, giving that lineage's abnormal
endings the same first-class status `hlt` gives this one.

This distinction matters more here than it would elsewhere because a
2-symbol Post machine has no in-band error channel: its entire alphabet is
`{blank, mark}`, so there is no spare symbol to encode "something went
wrong" on the tape itself. Termination kind — `stp`, `hlt`, or which trap —
is the machine's only free output channel beyond the tape. That is exactly
why the optimizer's equivalence contract (`docs/pmt/language.md
(optimization)`) treats termination kind as an observable on the same
footing as final tape contents and match-flag-dependent branch decisions:
it is not incidental metadata, it is one of only two places a program's
result can live at all.

## The historic programs

`Sum.pms` and `Ty.pms`, both from Generation A's `Compiller/` tree, are
ported to `.pmc` as this project's golden end-to-end tests
(`crates/post-machine/tests/golden/sum.pmc` and `ty.pmc`, diffed against
committed `.pmt` snapshots). One period-faithful detail survived the trip
unedited: the real 2002 `Sum.pms` still opens with the declaration
`Program Ty;` — a copy-paste artifact left over from `Ty.pms`, whose body
was reused as the starting point and never fully renamed. The golden
ports preserve each original program's statement sequence faithfully
(historic fidelity over minimality); `tests/golden/` is these two
programs' modern home.

## The design record

The full v1 design this toolchain was built from was written up as a
spec, kept as a frozen record in the repository's internal design
documents. It is no longer the authority code cites or docs are derived
from day to day — that role now
belongs to this page and its siblings (`docs/pmt/language.md`, `docs/pmt/isa.md`,
`docs/formats.md`, `docs/pmt/cli.md`, `docs/pmt/stdlib.md`, and `README.md`) — but
it remains the historical record of the reasoning behind v1's design
decisions, including the future architecture sketched in its appendix.
