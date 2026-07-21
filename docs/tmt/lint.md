# Linting `.tmc`/`.tma` — `tmt lint`

`tmt lint` reports hygiene findings the compiler and assembler
deliberately do not refuse. Each input's extension picks its rule table:
a `.tmc` file runs the compiler's analysis (through resolution, no code
generation) against the `.tmc` catalog below; a `.tma` file runs a full
assemble against the arch-agnostic assembly rules (`docs/core.md`) plus
the TM-1 additions further down this page. Either way a finding prints
as `FILE:LINE:COL: lint: MESSAGE`. Exit code 0 means every file is
clean, 1 means findings or errors somewhere. The command surface — the
directory walk, `--exclude`, the per-file fatal that keeps a batch
going — is `docs/tmt/cli.md`.

Lint reports lint findings only. Compile warnings stay on the compile
channel (`tmt compile`), with one deliberate exception: three rules
below (`unused-import`, `unused-routine`, `binding-product-threshold`)
**re-expose** a warning the compiler already raises, so that a `tmt lint`
run and its allow-list cover it too. The compile channel keeps its copy;
the detection is not duplicated, only the reporting.

`tmt lint` has no `--fix`: nothing it reports is applied for you on the
command line (`--fix` is an unknown flag). Several rules do attach a fix —
the per-rule entries below say which ones and what each one does — and
where a fix exists it surfaces through the editor's code actions
(`docs/lsp.md (code actions)`) rather than on the command line. Two of
them, `redundant-jump-to-next` and `leftover-debugger`, are arch-agnostic
rules shared with the PM-1 toolchain, where `pmt lint --fix` also applies
them from the command line — `redundant-jump-to-next` directly, and
`leftover-debugger`'s deletion gated behind `--fix --force`.

## Rule tiers and `--allow`

Most rules are **default-on**. Two are **opt-in**, run only when `--warn`
names them: `state-may-trap` (a deliberately partial state is idiomatic
in this language, so a totality lint by default would be noise on ordinary
programs) and `index-identity-map` (binding differently-glyphed alphabets
by index is occasionally the intent). Opt-in is explicit enablement, never
allow-removal — there is no way to reach either rule by un-suppressing it.

`--allow CODE` suppresses a rule and **allow beats warn**: a code named
by both flags stays suppressed. Naming a default-on rule with `--warn`
is accepted and does nothing; the rule was already running.

An unknown code named by either flag is a whole-tool error that aborts
the run before any file is read, so a typo cannot silently disable
linting:

```
$ tmt lint prog.tmc --allow no-such-rule
tmt: unknown lint rule `no-such-rule`
```

### One allow namespace across both languages

`--allow` and `--warn` draw from the UNION of every catalog `tmt` knows:
the `.tmc` rules, the opt-in rule, the `.tma` additions, and core's
arch-agnostic assembly rules. One allow-list therefore works for a batch
mixing both languages — a `.tma`-only code named on a `.tmc` run is
accepted and simply inert for that file, and a `.tmc`-only code on a
`.tma` run likewise. That is what lets a single project file govern a
directory holding both.

Two codes appear in both catalogs — `leftover-debugger` and, on the
`.tma` side only, the core rules' own names. `leftover-debugger` is one
code implemented twice (a `debugger` marker in `.tmc`, a `brk`
instruction in `.tma`), so allowing it suppresses both.

## Project file: `tmt.json`

A repository can carry its allow-list in a `tmt.json` file, so the
suppressions a team agreed on travel with the source rather than living
in shell aliases and CI flags. Its `lint.allow` entries draw from the
same shared namespace `--allow` does, and the two are combined as a
union.

`tmt.json` — the schema, nearest-ancestor discovery, the union with
editor settings, and which surfaces read it — is documented in full at
`docs/tmt/cli.md`; it is not restated here.

## The `.tmc` rules

### leftover-debugger (`.tmc`)

A `debugger` marker left on a rule. It lowers to a `brk` (`docs/core.md`),
and an un-stripped `brk` is an optimizer observability barrier that no
pass may move code across — so shipping one does not merely leave a
debugging aid in the binary, it pessimizes `-O1` output around it.

### unused-import

A `use` binding nothing references. Re-exposed from the compile channel
so the shared allow-list covers it: an import that resolves to nothing
used is dead weight in the module's namespace and a common leftover
after a refactor.

### unused-routine

A non-exported `routine` no `call` or `bind` names anywhere in the
module. Exported routines are library API and are never flagged. A
routine counts as referenced by any `bind` target even when that bind is
itself never called — a deliberate over-approximation, so the rule can
miss a dead routine but never invent one (the dead bind is
`unused-binding`'s finding in its own right).

### unused-graph

A non-exported `graph` no `graft` names anywhere in the module.
Exported graphs are library API and are never flagged. A graph that
nothing grafts contributes no states to any world — it is source that
compiles to nothing.

### unused-binding

A `bind … as N` whose name no `call` in the same world targets. A bind
is world-local, so only a `call N(…)` inside its own world could reach
it; if none does, the binding's whole point — giving a routine a
call-able name under a symbol map — has no consumer.

### unused-graft-instance

A named, non-entry `graft … as N` nothing in the world jumps to — a
spliced-in copy of a graph that no `goto`, no `call … then N`, and no
binding argument reaches. Dead splices are worth catching because a
graft is not free: it stamps a private copy of the graph's states.

An entry graft is the world's entry and is always live. The reference
scan over-approximates (every bare binding-argument target counts as a
potential reference), so the rule can let a genuinely dead instance
through rather than flag a live one.

```
b.tmc:13:3: lint: graft instance `deadSplice` is never used
```

### unused-graft-name

An **entry** graft's `as NAME` that nothing references. An entry graft is
reachable as the world's entry and its splice runs whether or not it is
named, so the name matters only when some `goto`, `call … then`, or
binding argument routes back to the instance; if none does, the name is
dead surface an entry graft may legally omit. This is the
reachable-but-unreferenced gap `unused-graft-instance` structurally skips
(that rule flags only non-entry grafts), so the two partition the grafts
by entry-ness and never double-report.

```
b.tmc:7:3: lint: entry graft instance name `seek` is never used
```

The fix removes exactly the ` as NAME` clause, leaving a valid unnamed
entry graft.

### unused-alphabet

An `alphabet` declaration no tape draws on — neither a machine `tape`
declaration nor a routine/graph signature tape parameter names it. Unlike
`unused-routine`/`unused-graph`, an **exported** alphabet is flagged too:
a tape may draw only on a locally-defined alphabet, so an alphabet has no
cross-object references in this language version to protect — an
exported-but-undrawn-on alphabet is as dead as a private one.

```
b.tmc:2:10: lint: alphabet `dead` is never used by any tape
```

The fix deletes the whole declaration, including any leading doc/attention
run — an orphaned `?`/`!` run is a parse error, so the doc goes with the
alphabet it documents.

### unused-tape

A machine `tape` no rule ever reads, writes, or moves, and no reuse ever
binds. A tape is untouched when, across every rule of the machine world,
its pattern cell is a wildcard (or omitted), its write cell keeps (`-`, or
omitted), and its move cell stays (`.`, or omitted) — and it is never
passed as a binding argument to a `call`/`graft`/`bind`, where a spliced
or called subgraph could touch it out of the machine's own view.

```
b.tmc:4:3: lint: tape `scratch` is never read, written, or moved
```

`fix: None` — a tape is a vector position, so deleting one narrows the
arity of every pattern/write/move vector in the world at once, not a safe
single-span textual edit. The finding is worth surfacing regardless: an
untouched tape still costs a cell in every emitted row.

### unused-exit

A `graph` `state` exit parameter its own body never targets — no `goto`,
no bare-name goto, no `call … then`, and no binding argument hands it on.
A graph's `state` parameters are its exits (the continuations a graft
wires up), and a declared-but-unreached one is dead surface every graft
site is still obliged to bind. It fires regardless of `export`: an exit no
body rule targets cannot fire for any caller, exported or not.

```
b.tmc:2:38: lint: graph `g` declares exit `miss`, which its body never targets
```

`fix: None` — the exit is part of the graph's signature, so removing it is
an API change at every graft site that must currently bind it, not a safe
local textual edit.

### deprecated-call

A `call`, `graft`, or `bind` whose target carries a `! [deprecated]`
attention line (`docs/tmt/language.md`). The finding names the verb and
appends the attribute's own message when it carries one:

```
b.tmc:15:14: lint: call to deprecated `oldHelper`: use newHelper instead
```

Only locally-defined targets are checked — an imported target's doc map
is not this module's, so its deprecation cannot be seen from here.

### dead-rule

Within one state, a rule an earlier rule in the **same dispatch band**
already covers cell-wise: at every tape position the earlier rule's
glyph set is a superset of this one's, so every input reaching this rule
already matched the earlier one. It can never fire.

The band qualifier is what makes this sound rather than merely
plausible. Codegen does not dispatch rows in source order — it re-bands
a state into exact rows, then partial, then catch-all, and takes the
first match in THAT order (`docs/tmt/isa.md`). Source order equals
runtime order only within a band, so cover reasoning is confined to one.
The exact band is excluded outright: two wildcard-free rules that
overlap are a conflict the compiler rejects, not a silent shadow.

```
c.tmc:7:5: lint: this rule is unreachable — an earlier rule in `s` already covers it
```

`dead-rule` is lint's richer relative of two warnings the compiler raises
on its own channel (`docs/tmt/language.md`): `unreachable-rule` (a second
all-wildcard rule — and only that exact shape) and `empty-expansion` (a
rule whose range/glyph expansion drops to zero rows). Those two live on
the compile channel because compilation must be total and honest even when
lint never runs; `dead-rule` is the fuller same-band-cover analysis, done
only at lint time.

### redundant-identity-pairs

A `with map { x -> x }` bidirectional pair that identity completion
would have supplied anyway (`docs/tmt/language.md`) — the pair is
ceremony, and writing it out invites the reader to look for a meaning it
does not carry.

The rule fires only when the caller tape and the bound callee tape draw
from an identical alphabet — same glyphs, same order — because identity
completion is index-based and applies only across equal-size alphabets.
Anywhere subtler the rule stays quiet: `x -> x` across unequal
alphabets is load-bearing, not redundant, and a false positive there
would be advice to break a working program.

```
e.tmc:9:41: lint: identity pair `0 -> 0` is redundant — an identity mapping already supplies it
```

### binding-product-threshold

A rule whose range cells expand to a large cartesian product of match
rows. Despite the name, this has nothing to do with a `call`'s or a
`bind`'s bindings — what it measures is one rule's own pattern. Each
cell contributes one row per in-alphabet member of its range; a wildcard
or a concrete single contributes one. Past the expander's own
cutoff (256 rows) the rule is reported, because a single source line
quietly becoming hundreds of emitted rows is worth knowing about before
it shows up as image size.

Re-exposed from the compile channel, computed source-level rather than
by running expansion, and sharing the expander's cutoff so the two
always agree.

```
d.tmc:7:5: lint: rule expands to 343 match rows (over 256) — the binding product is large
```

### writes-through-collapse

A `call`/`graft`/`bind` whose one-way (`=>`) symbol map collapses onto a
callee glyph the callee then writes. A one-way pair maps the caller
glyph to the callee glyph on READ only and is deliberately excluded from
write-back (`docs/tmt/language.md`), so a write to that glyph never
travels back through the collapse — which is usually a surprise, since
the author reached for `=>` precisely to say "read-collapse, do not
write here".

What actually happens to the lost write depends on the two alphabets,
and the message says which: across equal-size alphabets identity
completion sends it back as identity, so the program runs but does
something unintended; across unequal alphabets the maps complete closed,
the glyph is a write hole, and crossing it traps.

```
e.tmc:12:41: lint: one-way map collapses onto `0`, which `writer` writes — the write bypasses the collapse
```

The rule fires only on a literal write the local callee provably makes
at the bound tape's position; a computed write, or an external callee
whose body is unseen, is skipped.

### state-may-trap (opt-in)

A state whose rules leave some input unmatched and that has no
catch-all, so the match engine traps on that input. **Off by default** —
enable it with `--warn state-may-trap`.

```
$ tmt lint b.tmc --warn state-may-trap
b.tmc:18:9: lint: state `partial` may trap — its rules do not cover every input and there is no catch-all
```

The rule proves a gap before firing: it builds each rule's per-cell
match set over the tape alphabets, enumerates the input product, and
reports only when some concrete tuple matches no rule. A state with a
catch-all is never flagged; a state carrying an unresolvable range, or
whose product is too large to enumerate cheaply, is skipped rather than
guessed at. Every path errs toward silence. It is opt-in not because it
is unreliable but because partial states are a normal way to write this
language, and on a real program the rule has a great deal to say.

### index-identity-map (opt-in)

A `call` or `bind` with an **omitted** symbol map binding a caller tape to
a callee tape whose alphabets are not glyph-for-glyph equal. With no `with
map { … }` the binding maps by index (`docs/tmt/language.md`), so a glyph
the caller reads as one thing the callee reads as another — occasionally a
deliberate re-labelling by position, so the rule is **off by default**;
enable it with `--warn index-identity-map`. It mirrors
`redundant-identity-pairs` inverted: that rule fires when the two
alphabets are identical, this one when they differ at some shared index.
The message names the first differing index and both glyphs, caller side
first.

```
$ tmt lint b.tmc --warn index-identity-map
b.tmc:8:34: lint: call maps by index across differently-glyphed alphabets ('a' vs 'x' at index 1); glyphs change meaning here
```

Only `call` and `bind` — a graft's omitted map means glyph identity and
either matches or errors at compile time, so it never reaches this rule.
Silent when a map is written (the author is explicit), when the two
alphabets are glyph-for-glyph equal over their shared indices, or when the
callee's alphabet is not visible in this compilation (an external routine
resolved at link). `fix: None`: writing the intended map needs the
author's intent — which glyph should become which — that the tool cannot
guess.

## The `.tma` additions

TM-1's assembly dialect carries defects the arch-agnostic rules cannot
see, because those rules know nothing of sections, match tables, frame
descriptors, or `.rept` macros (`docs/formats.md`). These four rules
cover them. All are default-on — there is no `--warn` tier on the `.tma`
side — and they run alongside core's rules, both streams merged into one
source-ordered report.

### shadowed-wildcard-rows

A match-table row covered by an earlier row in the same dispatch band —
it can never match, so it is dead. This is the assembly-level twin of
`dead-rule` above: the same same-band cover model applied to a different
cell vocabulary (raw wildcard-or-index cells instead of `.tmc` glyph
sets). Row `W` covers row `R` when at every position `W`'s cell is a
wildcard or exactly the index `R` has there.

```
f.tma:5:5: lint: this row can never match — the earlier row at line 4 in the same match table already covers it
```

Consecutive `.row` directives form one table (a labeled row opens a new
one), and `.rept` bodies are scanned as tables of their own. A cell that
is a `.rept` substitution template is opaque: it never covers and is
never reported.

### retx-exit-bounds

A `retx #k` whose `k` is at or past the exit count of the frame active
when it runs — the return always traps (`docs/tmt/isa.md`). This is a
defect the assembler cannot refuse on its own, because the governing
exit count belongs to the frame descriptor a `call.m` installs, not to
the returning function.

```
f.tma:23:9: lint: retx #3 is out of range — the governing frame declares 1 exit(s) (valid #0..#0), so this return always traps
```

Resolution is in-file only. A routine reached solely from another
translation unit has no visible descriptor here and its returns are
skipped silently; a routine that in-file `call.m`s bind to more than one
distinct descriptor has a context-dependent exit count and is likewise
left alone. The common hand-authored shape — one descriptor per
callee — resolves exactly.

### rept-var-unused

A `.rept v, lo, hi` … `.endr` block whose loop variable is never
substituted in the body, so every iteration expands identically — a
copy-paste count wearing a macro's clothes.

```
f.tma:19:9: lint: the `.rept` loop variable `v` is never used in the body — every iteration expands identically
```

Substitution only touches `{…}` markers, so a bare mention of the
variable in a comment or a mnemonic is not a use. The scan is
conservative in the safe direction: it flags only when no `{…}` anywhere
in the block mentions the variable as a whole-word identifier.

### duplicate-map-source

A `.map` directive whose `rmap=(…)` or `wmap=(…)` clause lists the same
source symbol twice (`rmap=(1->2, 1->3)`). The assembler accepts it
silently and the **last** mapping wins — the emitted object is
byte-identical to the one the winning pair alone produces — so the earlier
pair is dead. The defect is **clause-generic**: the same last-wins
shadowing in the read map (`rmap`, physical → virtual) or the write map
(`wmap`, virtual → physical). The two are separate namespaces, so a symbol
appearing once in each is not a repeat, while a `.map` duplicating in both
yields one finding per clause.

```
f.tma:5:28: lint: source symbol 1 mapped twice; the last mapping wins
```

The finding spans the later (winning) pair; the fix removes the earlier
(shadowed) pair together with its trailing comma, so the remaining list
still parses. Top-level `.map` directives only — a `.map` inside a `.rept`
body is not scanned (a completeness limit, never a wrong finding).

## The arch-agnostic rules on `.tma`

A `.tma` file also runs core's assembly rules, read against the TM-1
syntax. They are documented at `docs/core.md`; all five apply here:

| Code | Fires on |
|---|---|
| `unreachable-code` | An unlabeled item after an unconditional jump or stop. |
| `unused-label` | A label nothing references — no operand and no table entry. Dispatch (`.targets`/`.target`) and frame-exit (`.exits`) entries count as references, so a label reached only through a table is not flagged (see below). |
| `redundant-jump-to-next` | A jump or branch whose target labels the next item. |
| `line-too-long` | A source line over 80 characters. |
| `leftover-debugger` (`.tma`) | An instruction using the architecture's declared debugger-break opcode. TM-1 declares one (`brk`), so this rule is live here. |

```
g.tma:4:9: lint: jump/branch to `nxt` targets the next instruction — fall-through is identical
g.tma:5:1: lint: leftover debugger break left in source
g.tma:7:9: lint: unreachable code: no label between here and the preceding unconditional jump/stop
g.tma:8:81: lint: line is 110 characters long (limit 80)
```

### `unused-label` and table references

The fifth core rule, `unused-label`, runs on `.tma` like the other four.
It once could not: the rule counted a label as referenced only when an
in-function jump or call operand named it, and on TM-1 that undercounts
badly. A code label reached through a `.targets` / `.target` dispatch
entry, or listed in a `.exits` frame descriptor, is referenced from the
table section, not by any operand — so on a program that dispatches
through a table nearly every branch target looked unused. The brainfuck
interpreter shipped under `docs/examples/` tripped 400 such findings, all
naming reachable code, which is why the rule was held off the `.tma` path
entirely.

Core now feeds its lint rules the lowered tables, so `unused-label`
counts a dispatch or exit target as a reference and flags only genuinely
dead labels. Match-table rows and frame `.map` clauses name symbols
rather than code labels and never count. A label a table reaches counts
as used everywhere in the file — the conservative direction for a
hygiene rule, since it can only silence a finding, never invent one. On
the brainfuck example the rule now reports nothing.

A label stamped out of a `.rept` block (`Linc{v}` → `Linc0`…`Linc126`)
has no single source line of its own, so if one were genuinely
unreferenced the finding is anchored at the enclosing `.rept` header and
carries no delete fix — there is no one line to remove.
