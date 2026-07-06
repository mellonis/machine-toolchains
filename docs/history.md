# History and lineage

This toolchain finishes work started across four Delphi implementations of
a Post machine, built between 2002 and 2012. None of the four, on its own,
was a complete toolchain: one generation had a language with no code
generator behind it, one had a code generator with no real source language
in front of it, and one was a machine with no compiler at all. This
project closes that triangle, and adds the piece none of the four ever
attempted: a linker, with separate compilation and libraries.

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
through instead of jumping (`docs/language.md (optimization)`,
`crates/post-machine/src/codegen.rs`) — a size-and-speed win the Delphi
lineage discovered and this toolchain keeps as a baseline, not an
opt-in pass.

**2007 — `Old Test-PostMachine`.** A code generator without a matching
source language of its own: it emitted machine code and supported calls
through a return-address stack, but that stack had no equivalent of PM-1's
mandatory `ent` check (`docs/isa.md`) — nothing verified that a call
target was actually the start of a function before jumping to it. PM-1's
`ent`-verification-always-on rule (every `call`/`call.s` traps unless the
target byte is an `ent`) is this toolchain's answer to that gap: call
safety the 2007 generation's stack never had.

**`PMProcessor`.** A machine with no compiler of its own: programs were
inspected and understood primarily through disassembly, not written
against a high-level source language — the disassembler-first mindset
this toolchain's own `pmt dis` (with its recursive-descent function
discovery, `docs/formats.md (assembly text)`) is a direct descendant of.
`PMProcessor`'s `TPostMachineProcessor`/`TBelt` split — the processor
talking to the tape only through a narrow interface, never touching it
directly — is the ancestor of PM-1's bus architecture (`docs/isa.md`),
generalized so that EVERY memory, not just the tape, sits behind a bus.
Its continuation-bit idea for encoding multi-byte opcodes is why PM-1
reserves opcodes `≥ 0x80` for exactly that future use, unused in v1. Every
Delphi generation also carried a step-cap guard against runaway
loops — the direct ancestor of `pmt run`'s `--max-steps` default of
10,000,000 (`docs/cli.md`).

**Generation D.** The lineage's most ambitious instruction design added an
`AF`/`BF`/`EF` flags trio and an `ja`/`jb`/`je` jump family: edge and
topology conditionals meant to let a program branch on whether the head
had reached an edge, boundary, or interior position of a *bounded* tape.
They were never actually wired up — the routine responsible for updating
them (`UpdateFLAGS`) was a stub that never ran. This toolchain supersedes
that whole approach rather than reviving it: PM-1 has no bounded tape by
default (`InfiniteTape`, `docs/isa.md`), an out-of-alphabet or otherwise
invalid device access is a `DeviceFault` trap rather than a flag a program
must remember to check, and — per the device-agnostic principle
(`docs/isa.md`, the processor never knows the head position or the tape's
topology) — any edge behavior a *bounded* tape does need is the device's
problem to enforce, never an instruction's.

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
requirement (`docs/isa.md`): a real machine has a fault-code register and
a HALT line, and `hlt` is a program deliberately asserting that same fault
path itself, as opposed to a trap the processor raises involuntarily. A
matching `abortState` sentinel is, as of this writing, being designed for
the `turing-machine-js` library, to give that lineage's abnormal endings
the same first-class status `hlt` gives this one.

This distinction matters more here than it would elsewhere because a
2-symbol Post machine has no in-band error channel: its entire alphabet is
`{blank, mark}`, so there is no spare symbol to encode "something went
wrong" on the tape itself. Termination kind — `stp`, `hlt`, or which trap —
is the machine's only free output channel beyond the tape. That is exactly
why the optimizer's equivalence contract (`docs/language.md
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

The full design this toolchain was built from is preserved at
`docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md`,
frozen as of this documentation set landing. It is no longer the
authority code cites or docs are derived from day to day — that role now
belongs to this page and its siblings (`docs/language.md`, `docs/isa.md`,
`docs/formats.md`, `docs/cli.md`, `docs/stdlib.md`, and `README.md`) — but
it remains the historical record of the reasoning behind v1's design
decisions, including the future architecture sketched in its appendix.
