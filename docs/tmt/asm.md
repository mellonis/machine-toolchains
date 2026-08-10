# `.tma` — the TM-1 assembly dialect

The TM-1 `.tma` dialect version is **0.3** (`TM1_TMA_DIALECT_VERSION`;
pre-1.0: the version is `0.N` and `N` bumps on any grammar change — the
same acceptance-contract shape as the `.pma` dialect,
`docs/pmt/asm.md`). Where PM-1 drives one two-symbol tape, TM-1 drives
up to sixteen tapes, each with its own alphabet, and branches through
match/dispatch tables rather than the mark register alone. The dialect
turns on three grammar features the classic `.pma` grammar leaves off —
a **tables** section, the `.rept` macro, and `[..]` **vector** operands
— plus a per-routine signature directive. Version 0.3 adds the fused
write+move mnemonic `wrmv` (a rule's whole write+move action in one
instruction). Its full version history is at the end of this page.

This page is the dialect's own surface: how a `.tma` file looks, what
`tmt asm` and `tmt dis` guarantee about the round trip, how the twenty
mnemonics are spelled, and what the `.tmc` compiler emits. The grammar
those spellings sit in is shared with every other dialect and lives in
`docs/formats.md (assembly text)` — the lexical shape and canonical
column grid, sections and the routine signature, vector operands, match
and dispatch tables, the compact symbol family, the `.rept` macro, frame
descriptors, and bound calls, each with the bytes it lowers to. The
assembler framework behind both dialects, with the capability set a
dialect opts into, is
`docs/core.md (the assembler framework)`; opcode semantics and timing
are `docs/tmt/isa.md`.

```asm
.section tables
Tfetch: .row    [1, *, *, *]    ; match tape 0 == 1, others any
        .row    [8, *, *, *]
Dfetch: .targets L_step, L_halt ; MR = 1 → L_step, MR = 2 → L_halt

.section code
.routine main, tapes=4, alpha=(9, 127, 127, 2)
.func main
L_step: rd                      ; latch every head into its slot
        mtc     Tfetch          ; walk the table, set the match reg
        djmp    Dfetch          ; dispatch on the match reg
L_halt: stp
```

One instruction (or one table directive) per line, `;` line comments, the
same **canonical column grid** as `.pma` (labels at column 0, mnemonics at
8, operands at 16, trailing comments aligned per group at or past 32 —
`docs/formats.md (assembly text)` has the exact rule); the parser accepts any
whitespace on input, and `tmt fmt` / `tmt dis` emit the grid. `tmt dis`
output is valid assembler input — including a `--call-mech=mono` linked
image, whose stamped specialized routine copy is named with a
`.`-separated digest suffix (`bare.513e6968`, see `docs/tmt/isa.md
(call mechanisms)`) drawn from the same character set an ordinary
identifier already accepts, so the name re-lexes like any other.
Reassembling an **object's** disassembly reproduces the original bytes
exactly. Reassembling and re-linking a **linked image's** disassembly
reproduces an equivalent image — same code, same table content — but not
always the same bytes: a frame that originated from a declarative binding
always disassembles to raw `.frame`/`call.m` syntax (there is no way to
reconstruct the declarative form), and relinking that syntax does not
necessarily lay out the tables section the way the original
declarative-binding link did.

The same "equivalent image, not always the same bytes" rule covers one
more shape, hand-written-only like the frame case above: a body whose
explicit entry-prologue (`ent`) byte is the target of an unconditional
jump re-links to a byte-different but semantically identical image —
same outcome, same tape content, under every call mechanism — when the
jump opens a **genuine cut** of the disassembler's control-flow walk
(`docs/pmt/asm.md` states the same rule for `.pma`). A cut needs all
four of: nothing falls through into the boundary; the region it opens
never falls out its own end (it always stops or resolves its own final
jump); no local-label edge spans the boundary in either direction (a
call, or a jump onto an already-established root, is exempt); and every
table the boundary would split lies wholly on one side of it. These four
are what the walk checks, and together they cover every mechanism by
which the rendered text either names a code address or lets execution
cross a boundary.

The fourth condition is where `.tma` differs from `.pma`: `.tma` is the
only dialect with table sections, so it is the only one where a boundary
can actually collide with a table. A boundary that would split a match
table, a dispatch table, or a frame descriptor across two regions is
declined outright, rather than rendered as text the assembler would
refuse (`bad-table`) — the discovered corpus never hits this (no shipped
program hand-writes a mid-body `ent` at all), but the condition holds at
the dialect level regardless.

## The mnemonic set

The dialect accepts twenty mnemonics. The opcode table — each mnemonic's
byte, operand shape, and semantics — is `docs/tmt/isa.md (instruction
set)`. What belongs here is how they are *spelled*:

- **Jump and call targets are labels**; `call` additionally accepts a
  routine symbol. `call.s` exists in the mnemonic table for disassembly
  display and link-time relaxation only: the assembler always emits far
  `call` and rejects `call.s <target>` in source, because the width is
  linker-selected. The linker's relaxation fixpoint narrows a far `call`
  to `call.s` when the target is in short range, exactly as PM-1 does.
- **`mtc` and `djmp` take a table label** defined in the tables section.
- **`wr`, `mov`, and `wrmv` take bracketed vectors** — the element
  vocabulary is `docs/formats.md (vector operands)`; `wrmv` takes two,
  comma-separated.
- **`trap` and `retx` take an immediate.** `#<n>` is a single unsigned
  byte, `0`..=`255`, written with a leading `#` (`trap #0`, `retx #1`).
  It is distinct from a symbol or a label — it carries a raw number.
- **`call.m <target>, <frame>`** pairs a call target with a `.frame`
  label from the tables section. The operand's two halves are a rel
  displacement and, after link, a call-site index
  (`docs/formats.md (the frames region)`).
- **`ent` is emitted implicitly by `.func`** and is the runtime call
  guard. Unlike `call.s` above, which the assembler rejects outright in
  source, a hand-written `ent` is not a grammar violation — an extra one
  anywhere in a function body assembles cleanly as a harmless duplicate
  no-op. There is simply no reason to write it: `.func` already emits it.

## What the `.tmc` compiler emits

The `.tmc` language front end (`tmt compile`) generates this dialect and
nothing exotic: a conditional state lowers to `rd` / `mtc` / `djmp` over a
match table satisfying the row discipline (`docs/tmt/isa.md (match and
dispatch)`), a rule's write+move
action lowers to a single `wrmv` (elided when it is all-keep + all-stay),
and a cross-alphabet `call` lowers to the **binding-call operand**
(`docs/formats.md (bound calls)`) — never a hand-authored `.frame`. So a
compiled object always reaches the
link stage as ordinary code plus bound-call records, and the choice of
call mechanism stays a link-time decision independent of the source.

## Dialect version history

- **0.1** — the initial TM-1 assembly surface: the base mnemonics, the
  sectioned `.routine` / `.section` / `.row` / `.targets` / `.rept`
  directives, and the `[..]` write- and move-vector operand forms.
- **0.2** — the **frames** family: the framed call `call.m`, the
  multi-exit return `retx`, the explicit `trap`, the `#imm` immediate
  operand, the `.frame` / `.map` / `.exits` frame descriptors, and the
  declarative binding-call operand.
- **0.3** — additive: the fused write+move mnemonic `wrmv [w…], [m…]` —
  the write vector then the move vector in one instruction (all writes
  precede all moves). It is the `-O0` codegen canon for a rule's action;
  no earlier program changes meaning.

0.3 also gained the trailing-comma list continuation on `.targets`,
`.exits`, and `.map` (`docs/formats.md`, "match and dispatch tables" and
"frame descriptors") without becoming 0.4. The stated acceptance contract
is that `N` bumps on *any* grammar change once a version has shipped and
so become a contract to preserve; this addition landed during 0.3's own
development, before its first release, so it folded into 0.3 rather than
opening 0.4. The dialect version is **0.3** as released; a grammar change
proposed after that point bumps to 0.4 in the ordinary way.
