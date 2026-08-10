# `.pma` — the PM-1 assembly dialect

The PM-1 `.pma` dialect version is **0.3** (`PM1_PMA_DIALECT_VERSION`;
pre-1.0: the version is `0.N` and `N` bumps on any grammar change, the
same acceptance-contract shape as the `.pmc` language version in
`docs/pmt/language.md`). See "Dialect version history" below for what
each version changed.

This page is the dialect's own surface: how a `.pma` file looks, what
`pmt compile -S`, `pmt asm` and `pmt dis` guarantee about the round
trip, symbol jumps, and the `.volatile` build-column directive. What
every dialect shares — the lexical shape, the canonical column grid, the
comment-column rules, the `.func` visibility and name grammar, and the
byte layouts assembly text lowers to — is
`docs/formats.md (assembly text)`; the assembler framework behind both
dialects, with the capability set a dialect opts into, is
`docs/core.md (the assembler framework)`. Opcode semantics and timing
are `docs/pmt/isa.md`.

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

The two producers of canonical `.pma` text — `pmt compile -S` and
`pmt dis` — differ on one point of the grid
(`docs/formats.md (assembly text)`), the long-label rule: `pmt dis`'s
grid (`grid_line`) keeps a short label field — 7 characters or fewer,
the name plus its `:` — inline with its instruction, and moves only a
field of 8 characters or more to its own line, so a long label never
pushes the mnemonic column out of alignment; `pmt compile -S` puts every
label on its own line unconditionally, regardless of length. `pmt fmt`
treats both shapes as already canonical, so reformatting the output of
either `pmt compile -S` or `pmt dis` is always a no-op. `pmt dis` output
is always valid assembler input — round-tripping through `asm`
reproduces the original bytes exactly, build-column tags and the
program-volatile bit included ("The `.volatile` directive" below).

`pmt dis` accepts either binary. From a `.pmo`: real names come from the
symbol table, code is shown per function, and call sites are named from
relocations. From a `.pmx`: names come from the `-g` sidecar map when one
is present (`FILE.pmx.map` beside the executable, or `--map`); otherwise
they are synthesized via **recursive-descent discovery** — a worklist walk
from the entry point following control-flow edges; every verified `call`
target is a function root (exact in v1, which has no indirect control
flow). Discovered roots are named `main` (the entry) or `func_XXXX`;
internal jump targets are named `LXXXX`; bytes never reached by the walk
print as `.byte` directives, one per byte. The `ent` byte remains the
runtime call guard, but function discovery itself comes from control flow,
not byte scanning — an operand byte that happens to equal the entry opcode
is never mistaken for a function start.

**Symbol jumps (tail calls):** `jmp @name` takes a function symbol, not a
label — in an object it assembles as a far `jmp` plus a relocation (the
same hole-and-relocation mechanism as `call`), and relaxes to `jmp.s` at
link time exactly like a `call`. `jmp.s @name` is a syntax error (width is
linker-selected, like `call.s`), and conditional `jm @name`/`jnm @name` are
errors — v1 branches take labels only. Disassemblers print a relocated jump
(from an object, via its relocation table) or a jump landing on a function
root (from an executable, via discovery) in the `jmp @name` form; a jump
into another function's middle that lands on no known root falls back to
`.byte`.

## The `.volatile` directive

`.volatile` is a presence-form directive — no operand, no value — naming
a **build column** (`docs/pmt/language.md (volatile programs)`). It has
two legal placements, meaning different things:

- **Directly after a `.func` line** it tags that block as the volatile
  column. Absence is the normal column; there is no `.normal`.
- **Before the first `.func`** it sets the object's program-volatile bit
  — the header flag the linker reads off the entry-defining object to
  pick a column for every name (`docs/formats.md (MO)`;
  `docs/core.md (linking)`).

Anywhere else is an error. "Directly after" means the next item: own-line
comments are trivia and do not close the slot, but a label, an
instruction, or a second `.volatile` does, and the complaint is that
`.volatile` must directly follow its `.func`. A second file-level
`.volatile` is a duplicate `.volatile`.

A name may be defined **once per column**, which makes a bare/`.volatile`
pair the only same-name pair one file may carry; two bare `.func f`
blocks stay `duplicate-function` exactly as before. The two members of a
pair must also agree on visibility: `.func f local` paired with a
`.func f` is refused, because the linker pairs a name's columns only
among exported ones and a half-local pair would half-vanish there.

All three of those complaints — the two placement ones above and the
visibility one — render as `syntax` (`docs/core.md (error codes)`). The
directive introduces no error code of its own; the only coded diagnostic
it changes is `duplicate-function`, which it makes column-aware rather
than name-only.

What an author controls, then, is four shapes:

| The file writes | The object carries | A normal program links | A volatile program links |
|---|---|---|---|
| one bare `.func f` | one `normal` blob | it | it, counted as a fallback |
| one `.func f` + `.volatile` | one `volatile` blob | it, counted as a fallback | it |
| a pair with different bodies | a `normal` and a `volatile` blob | the bare one | the tagged one |
| a pair with identical bodies | one `both` blob | it | it |

The last row is the assembler's own dedup, mirroring the compiler's: a
legal pair whose two blocks assemble to the same bytes and the same call
sites collapses to one `both`-tagged blob. A single block is deliberately
never auto-promoted to `both` — `both` is a **statement**, made by
writing the function twice. Promoting a lone block would erase the
fallback signal the first two rows exist to carry: a normal-only or
volatile-only function would become unwritable. It would cost the text
round trip too, since a single bare `.func f` would then assemble to a
`both` blob, which disassembles as two blocks — text that no longer
comes back as the text that produced it.

`pmt dis` emits all of it — the program-bit line leads the dump, a
tagged block carries the directive under its `.func`, and a `both` blob
prints twice, bare first — so the text reassembles to the object it came
from, byte for byte:

```
$ pmt compile -O1 two-v.pmc -o two-v.pmo
$ pmt dis two-v.pmo > two-v.pma
$ pmt asm two-v.pma -o rt.pmo && cmp two-v.pmo rt.pmo && echo identical
identical
$ cat two-v.pma
.volatile
.func main
        wr      1
        stp
.func main
.volatile
        wr      1
        wr      1
        stp
```

(That is `volatile main() { mark; mark; }` at `-O1`: the normal column
drops the idempotent second write, the gated column keeps it, and the
object carries both plus the program bit.) The round trip is byte-exact
without `-g`; a `-g` object's debug lines describe `.pmc` sources the
disassembly does not have, so they do not survive the trip — the same
declared exception that applies to every other debug side table.

**The directive is selection metadata, not protection.** It says which
column a blob belongs to and which kind of program this object builds.
It says nothing about the body, which the assembler transcribes exactly
as written either way. Hand-written assembly preserves the author's
transactions on both architectures regardless of any directive: nothing
after the assembler reorders, merges, or drops tape operations — PM-1's
only post-assembly rewrite is the linker narrowing a call's width, and
TM-1's mono stamping remaps symbols, never sequences. So a `.pma` file
with no `.volatile` in it is not "an unprotected build"; it is a
normal-column build of exactly the instructions it lists.

**One footgun.** A `.volatile` block's calls bind the callee's volatile
column. For a `local` callee — bound directly within the object, never
through the linker's namespace — a missing volatile twin has nowhere to
fall back to, so the reference becomes an external and the link fails
with a bare `unresolved symbols: NAME`, which never mentions the column
that was missing. Give a local helper both columns, or export it (an
exported name falls back and is merely counted).

**PM-1 only.** `.func` is a core directive both dialects share, but
`.volatile` rides an assembler capability only the PM-1 dialect enables:
`.tma` does not recognize the word at all, since TM-1 volatility is a
property of a tape parameter rather than of a routine
(`docs/tmt/language.md (volatile tapes)`). The framework also refuses to
combine the directive with `.routine` signatures or table sections —
merging build columns renumbers blobs, and those records are indexed by
blob — a rule no shipped dialect can reach, since the one dialect with
the directive has no table surface.

## Dialect version history

- **0.1** — the v1 toolchain's dialect; the retroactive baseline the
  version scheme measures from.
- **0.2** — one tightening: label names dropped `.` and `::` from their
  accepted characters, leaving letters, digits, and underscores (Unicode
  letters still legal). Symbol names in `.func` and jump/call operands are
  unaffected — the dotted/`::`-segmented grammar above still applies to
  them.
- **0.3** — additive, two things. The fused write+move mnemonics `wrl`
  and `wrr` join the mnemonic set (each takes a one-element symbol vector
  like `wr`, `docs/pmt/isa.md`). And the `.volatile` directive joins the
  directive set, in both placements ("The `.volatile` directive" above).
  No existing program changes meaning; the accepted set only grew, and a
  file that writes neither assembles to the bytes it always did.
