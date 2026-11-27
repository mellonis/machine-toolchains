# fmt: comment placement and trailing-comment alignment

Design for [#68](https://github.com/mellonis/machine-toolchains/issues/68)
and [#69](https://github.com/mellonis/machine-toolchains/issues/69).
Both were filed on a misreading; this spec records what the code
actually does, what should change, and what should not.

## Background

`#66` added the `.tmc` fmt dogfood gate and reformatted five sources.
Reading that output raised two complaints, which became #68 (`.tma`
full-line comments relocate) and #69 (two `.tmc` outputs read as
mistakes). Investigating both showed the formatter behaving exactly as
documented in every case. Neither issue describes a defect.

What survives is a narrower and more useful question: the canonical
forms produce output no author writes by hand. The flagship's own
layout disagrees with the formatter in three places, and in each the
author's version reads better. This spec changes the canonical forms to
match, rather than changing the files to match the forms.

### Corrections to the issues as filed

- **#68 evidence 1** claims full-line comments move "in opposite
  directions per section". There are not two rules. There is one —
  `own_line_comment_col` in `crates/core/src/asm/fmt.rs` — keyed on
  whether the first `.func` has been seen. The flagship straddles that
  point, so a single rule produces both moves.
- **#68 evidence 2** claims the trailing-comment column shifts "~40 to
  ~24". Measured: all 41 trailing comments are authored at column 40;
  35 normalize to `COMMENT_COL` = 32, one to 33, five stay at 40. The
  formatter is normalizing a non-canonical file, which is its job.
- **#68 evidence 3** claims `.routine`'s operand respacing matches
  house style. There is no single house style: 21 vector operands are
  written tight, seven `.targets` lists spaced. The real rule — commas
  in a directive are spaced, commas in an operand are tight — is
  coherent and needs no change.

## Decisions

### D1 — own-line comment placement (core, both dialects)

`own_line_comment_col` collapses from three branches to two:

1. **Continuation.** An own-line comment run sitting inside a group
   whose preceding line carries a trailing comment prints at that
   group's comment column.
2. **Structural.** Everything else prints at column 0.

The `MNEMONIC_COL` arm is deleted, so no comment is ever placed at
column 8. The `.func` lookahead carve-out is deleted with it — its only
purpose was routing comments around that arm.

A run with no preceding commented line in its group has no column to
continue and falls to rule 2. This covers file headers, section
banners, and any comment opening a group.

Rationale: column 0 and the trailing-comment column are the two
positions that carry meaning — structural, and attached-to-this-line.
Rule 1 now covers *attached*, which leaves column 8 meaning neither. In
the flagship all ten own-line body comments are authored at column 0
and none is authored at column 8; the branch being deleted describes a
practice no one follows.

**This is shared, not caps-gated.** `AsmCaps` gates syntax a dialect
does not have, not whitespace taste. Consequence: a `.pma` file with an
own-line comment inside a `.func` body reformats, column 8 to column 0.
No committed `.pma` file is affected — the corpus is two scratch files
under `.superpowers/` with zero body comments — but inline test
fixtures may be, and this is a real behaviour change for PM-1 users.

### D2 — trailing-comment alignment (core, both dialects)

The fixed `COMMENT_COL` becomes a per-group column.

- **Column** = `max(32, widest code width in the group + 1)`, where a
  line's code width is its character count up to the comment with
  trailing whitespace trimmed. 32 is a floor; a group only ever widens.
- **A group ends at** a blank line, a column-0 comment, or a
  `.section` / `.func` / `.routine` directive.
- **Own-line continuation runs stay inside** the group; they are what
  D1 rule 1 aligns to it.
- **Lines with no trailing comment contribute no width**, so a long
  uncommented line does not drag its neighbours right.
- **A `.rept` block ends a group**, and its body lines contribute no
  width. `.rept` bodies print verbatim from source rather than through
  the grid, so a 37-character body line staying at its authored column
  40 must not drag an enclosing group to 38 — that would be visibly
  incoherent, with the group aligned to a member that never joins it.
- **The column is unbounded.** It is never capped by `LINE_WIDTH`; a
  group whose widest member is far right pushes its comments past 80
  and `line-too-long` reports the result. This matches D4, which
  removes the same guard on the `.tmc` side. Width is the linter's
  concern in both printers.

Rationale: file-wide alignment was rejected because one long line would
reflow every comment in the file. Group-wide alignment keeps a one-line
edit local, which is the property gofmt protects. The group-boundary
clause naming `.section` / `.routine` is caps-gated syntax and never
fires in `.pma`.

An earlier draft of this paragraph argued that the floor is what keeps
PM-1 output byte-identical, "since PM operands are bare addresses, so
every `.pma` group resolves to 32". **That argument is false and is
retracted** — see the correction below. A `.pma` call with a
sixteen-character target reaches width 37 and widens its group. What
actually protects PM-1 is stronger and structural: `format_asm` is not
on the compile path at all (its only callers are `pmt fmt` and the LSP
formatting provider), tool-emitted assembly carries no comments, and
the repository contains no committed `.pma` file.

### D3 — operand lists: no change

`.routine`'s `alpha=(9, 127, 127, 2)` stays spaced. Directive commas
are spaced, operand commas are tight; that split is by syntactic
position and is worth keeping.

### D4 — `.tmc` run alignment always wins (turing crate)

`trailing_spacing` in `crates/turing-machine/src/fmt.rs` drops its
`LINE_WIDTH` guard. Every member of an aligned run aligns, including
one whose comment then carries the line past 80 columns.

Rationale: the per-member fallback produces a run where three members
align and one does not, which reads as a typo rather than as a rule.
The author's own layout of that block sits at column 23 with an
85-character line, so alignment already beat the limit by hand.

An earlier draft of this rationale said "width remains the linter's
concern: `line-too-long` continues to report the result." **That is
false for `.tmc`, and was verified false during implementation:** a
110-character `.tmc` line produces no `line-too-long` diagnostic.
That rule is arch-agnostic assembly lint — it fires on `.pma` and
`.tma`, never on `.tmc`. So an over-80 `.tmc` line is simply
unreported, under this decision and under every alternative to it. The
pre-existing claim at `docs/tmt/fmt.md` (the paragraph on a long
`with map` binding) is wrong for the same reason and is corrected in
this round.

The decision stands on its own ground: a run where three members align
and one does not reads as a typo. Since no option produces a lint
report, lint behaviour is not a discriminator between them.

D2 and D4 make the same call for the same reason — neither printer caps
its computed column at `LINE_WIDTH`. Stated in both places rather than
left to be inherited by silence, because the two are independent code
paths and an implementer reading one should not have to infer the
other.

This is new behaviour in one respect worth stating plainly — fmt will
now lengthen a line past 80 that would otherwise have fit. The existing
documentation only says fmt declines to *rewrap* an already-long line.
`docs/tmt/fmt.md` must say the stronger thing.

### D5 — the orphaned semicolon: no change

When a tail own-line comment forces a break, the closing `;` stays at
the statement's own indent — column 0 for a top-level `use`.

Moving it up to the last path's line was ruled out on losslessness
grounds, not taste: it would place the comment after the `;` and
reorder the token stream, which `fmt_tmc`'s signature check compares in
order.

## Round 2 — decisions taken after the whole-branch review

The final review found a blocking regression: `tmt fmt` over `tmt dis`
output inflated six ~80-character lines to 825, because the
disassembler emits ~400 lines with no blank line, no own-line comment
and no `.func` inside `.section tables`, making the whole region one
group under D2, whose widest member is a 753-character `.targets`.

Investigating it surfaced a second, older defect: **`tmt dis` output
has never assembled**, on this branch or before it. So the property at
stake for TM is the stability and readability of that output, not a
round trip. Tracked as its own issue.

### D6 — the disassembler emits human-shaped assembly

`tmt dis` emits a blank line between tables, as a person writing
assembly by hand does — and as the hand-written flagship `.tma` already
does, which is exactly why it never hit this.

Verified before adoption: inserting a blank line before each `T<n>:`
table makes `tmt fmt` a **complete no-op** on disassembler output, 0
lines changed. This closes the blocking regression on its own, with no
threshold on the group column and no grammar change.

The alternative considered and rejected was capping the group column at
some width. It would have worked, but it treats the symptom: a 400-line
run with no structural break is not a "group" in any meaningful sense,
and the honest fix is to stop emitting one.

### D7 — list continuation in `.tma`

A trailing comma continues a list onto the next line:

```
D0:     .targets L_plus, L_minus, L_left, L_right,
                 L_dot, L_lbr, L_rbr, L_halt        ; comment
```

No new syntax, and a trailing comma is currently always an error, so
nothing that assembles today changes meaning.

**That example illustrates the SYNTAX only — it is not real formatter
output.** Measured during implementation: unwrapped, its code is 78
characters, under the 80-column budget, so `fmt` would leave it on one
line. Any documentation page showing wrapped output must paste a real
transcript of a list that genuinely exceeds the budget, not this.

**Applies to the three unbounded lists: `.targets`, `.exits`, `.map`.**
Measured rather than assumed — `.row`, `alpha=(…)` and `.frame
tapes=(…)` are bounded by tape count (16 maximum) and cannot reach the
width where wrapping matters; `.target` and `.section` take one
operand. `.byte` was initially included on the assumption it took a
list and was **removed after testing**: `.byte 1, 2, 3` is rejected
with `bad-operand`, it takes exactly one value.

All three are gated behind `AsmCaps::tables`, so **PM-1 `.pma` is not
touched at all.**

The formatter wraps such a list when the line would exceed 80 columns,
continuation lines aligned under the first element. The disassembler
emits the wrapped form directly, so its output does not depend on a
later formatting pass.

Implementation constraint: `parse_asm_cst_with` splits the source with
`source.lines()`, so a directive cannot cross a line by construction.
The join belongs in a pre-pass ahead of that loop — the CST stays
line-oriented, and only what counts as a line changes. Spans and line
numbers need care, since diagnostics, the `-g` debug line map and both
LSP services all read them.

### D8 — no dialect version bump

`TM1_TMA_DIALECT_VERSION` stays at `0.3`. That version has never
appeared in a published release, so no acceptance contract exists yet
to break — the grammar is still maturing inside 0.3 until the release
cut. Doing this now is why the scope was widened to all three lists
rather than only the one that hurt: after the cut, adding the other two
would cost a version bump.

### D9 — the disassembler's output must assemble

Two naming mechanisms currently fail to meet. Without a debug map,
`.targets` emits raw offsets the grammar does not accept. With one, it
emits debug-map names that nothing in the file defines, because code
positions get their own independently synthesized `L0001`-style labels
drawn from jump targets only.

One mechanism replaces both: `.targets` always emits label names, and
every name it emits is defined at its address. Prefer a debug-map name
where one exists, synthesize otherwise, and emit the label either way.

### D10 — mono stamp names become lexer-safe

The linker named each mono-stamped specialization
`<routine>$<digest8>`, but the assembly lexer admits only
alphanumerics, `_` and `.` in a word. So `tmt dis` on a mono image
emitted a `call` line its own assembler rejected as raw text. Two of
the 42 round-trip combinations failed there.

The separator becomes `.`, which the lexer already accepts for symbol
names. The relevant check is `is_symbol_name`, not `is_label_name` —
the rule against dots constrains labels, not routine names — and
`outline` already mints `<name>.outline<N>` the same way.

**Widening the lexer to accept `$` was considered and rejected.** It
would make the character legal in every identifier forever, including
hand-written labels, to accommodate one internal naming convention.
The grammar is not the right seam for a generator that can simply emit
names the language already accepts.

**The trade this makes explicit.** `$` was collision-proof for free,
being illegal in a hand-written name. A legal separator gives that up,
so the guarantee moves into the code: a `used_names` set seeded from
the entire resolved order before the first mint, and a new
`LinkError::StampNameCollision`. That is strictly better than the
property it replaces — it is checkable, and it is checked, rather than
resting on a side effect of the grammar. Removing the guard turns a
dedicated integration test red.

Do not revert this to `$` without also restoring a collision guarantee,
and do not drop the guard on the grounds that collisions "cannot
happen" — with a legal separator they can.

### Corrections from the whole-branch review

- **I1** — `.rept` header trailing comments print at a fixed column 32
  while every other comment goes through the group column, contradicting
  the rule `docs/formats.md` states in this same round.
- **I2** — `.section` / `.func` / `.routine` pieces are padded *to* the
  group column but excluded *from* its width, producing exactly the
  raggedness D2's rationale claims to remove. Reachable in ordinary
  source: a `.routine` line is already 46 characters.
- **M1** — the test asserting D2's column is uncapped is vacuous; it
  would pass unchanged before this branch. It must assert on the narrow
  line's column, not merely that some line exceeds 80.
- **M2** — the floor argument in this spec was false as written. What
  actually protects `.pma` is stronger: tool-emitted assembly contains
  no comments at all, and the repository contains no committed `.pma`
  file.

## Scope

In:

- `crates/core/src/asm/fmt.rs` — D1 and D2, plus the group scan D2
  needs.
- `crates/turing-machine/src/fmt.rs` — D4.
- `docs/formats.md` (assembly text) — the own-line comment rule and the
  alignment model.
- `docs/tmt/fmt.md` — D4's stronger statement about line length.
- Worked examples in `docs/pmt/fmt.md` and `docs/tmt/fmt.md`. Both
  carry real tool transcripts; each must be re-run and re-pasted rather
  than eyeballed, per the repo's docs-audit practice. `docs/formats.md`
  is the only page that states the grid columns themselves.
- `docs/examples/brainfuck-utm.tma` — reformat under the new rules.
- A `.tma` fmt dogfood gate, mirroring `fmt_tmc.rs`'s
  `every_tmc_source_is_already_fmt_clean`. This was #68's original ask
  and is the reason the rules had to be settled first: the gate pins
  whatever canonical form is in force.

Round 2 added, and this section originally listed only round 1:

- `crates/core/src/asm/cst.rs` — the trailing-comma continuation
  pre-pass (D7).
- `crates/core/src/asm/assembler.rs` — the continuation's assembly-side
  acceptance.
- `crates/core/src/asm/disassembler.rs` — the unified label mechanism,
  blank lines between tables, wrapped list emission, and the shared
  `render_tables` that replaced a second copy of the table loop (D6, D9).
- `crates/core/src/linker/stamp.rs` and `linker/mod.rs` — the lexer-safe
  stamp names and the collision guard (D10).
- `crates/turing-machine/src/lint/tma/rules/duplicate_map_source.rs` —
  the rule assumed a `.map` group sat on one line, which a wrapped group
  broke.
- `docs/core.md`, `docs/tmt/isa.md`, `docs/tmt/cli.md`, `README.md` —
  the round-trip claims and the stamp-name examples.

Out:

- `#66`'s reformatted `.tmc` goldens, except where D4 moves them.
- Any `.pmc` formatter change. D1 and D2 are assembly-side.
- Comment re-attachment inside comma lists — separate, already closed.

## Expected output

- The flagship's file-header comment block (lines 1–33, prose plus its
  own top and bottom `====` borders — distinct from the section
  banners) and all ten `; ==== … ====` section banners stay at column
  0. An earlier draft called the header "20-line"; it is 33.
- Its `; no catch-all` block aligns under `; 'H'` at that group's
  column.
- Its tables stay at column 32. This is **not** the authored column 40,
  and D2 does not recover it. Measured code widths in the flagship:
  every `.row` line is 25, so a `.row` group resolves to
  `max(32, 26)` = 32 — the floor, which is where the fixed column
  already puts them. Of the two `.targets` lines, only `Dsbp` at code
  width 32 widens its group, to 33, dragging its group-mates with it;
  `Dsfp` at 30 resolves to `max(32, 31)` = 32 and stays at the floor.
  One widened group is the whole visible effect on this file.
  (Measured during implementation; an earlier draft of this paragraph
  claimed both `.targets` lines widened.)

  The five comments sitting at column 40 in current formatted output
  are not the grid: they are inside `.rept` bodies, which print
  verbatim by design and are untouched under either model.

  D2 is adopted knowing this. What it buys is not the size of the diff
  but the shape of the result: under a fixed column, the moment any
  line's code passes 32 the file goes ragged — `Dsbp` at 33 while its
  neighbours sit at 32 — and each such line strands itself
  independently. Group alignment makes that raggedness local and
  coherent: a group is aligned or it is not, and a wide member widens
  its own group rather than dropping out of it. That property is
  present in this corpus today. The authored column 40 is reachable
  only by changing the constant, which is deliberately out of scope.
- `.pma` output is unchanged wherever fields fit before column 32,
  which for bare-address operands is always. The exception is D1's
  body-comment case, which no committed file exercises.
- `docs/examples/brainfuck-utm.tmc`'s tape block aligns at column 21
  with an 82-character first line. That is its ONLY change, and it is
  the only `.tmc` file in the corpus that moves — the goldens and
  `std.tmc` are already canonical under D4.
- One change in the `.tma` flagship comes from D3 rather than from D1
  or D2, and this section originally failed to predict it:
  `.routine main, tapes=4, alpha=(9,127,127,2)` becomes
  `alpha=(9, 127, 127, 2)`. `render_routine` has always joined that
  list with `", "` — the code is untouched by this round, verified
  against the branch point. The flagship was simply authored tight and
  has never been run through `fmt` before. Expected, not scope creep.

## Verification

- `cargo test --workspace`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo fmt --check`.
- The `.tmc` goldens must still pass **without regeneration** — they
  are derivation-first, so a passing run after a whitespace-only
  reformat is the evidence that no token moved.
- `.tma` and `.tmc` fmt idempotence and token-signature guards must
  hold on the reformatted corpus.
- PM-1 byte-identity: `crates/post-machine` compiled output unchanged.
- The measurement that D4 does not silently reformat much of the `.tmc`
  corpus: count the runs whose alignment changes, and name them.

## Risks

- D2 replaces core's alignment model days before a release cut, for a
  measured benefit of two lines on the only corpus available. That
  trade was put explicitly and accepted: the model is right even where
  the diff is small.

  **RETIRED during implementation.** This risk originally continued
  "the floor bounds the blast radius; if any `.pma` group turns out to
  exceed 32 in practice, that assumption needs revisiting before
  merge." A `.pma` group *can* exceed 32 — a call with a
  sixteen-character target reaches 37. The floor was never what
  protected PM-1; `format_asm` is not on the compile path, tool-emitted
  assembly carries no comments, and no `.pma` file is committed. Not a
  merge blocker.

- D4's reach across the `.tmc` corpus. **RETIRED — measured.** One
  file, two lines: `docs/examples/brainfuck-utm.tmc`, where
  `tape prog: ops;` gains three spaces so its comment joins the three
  already aligned at column 21. The goldens and the stdlib do not move.
- D1 is a documented behaviour change for PM-1 users. No committed
  `.pma` *file* exercises it, but three inline fixtures in core's fmt
  test module do and will fail until updated — `crates/core/src/asm/
  fmt.rs` lines 633, 784, and 836, each pinning a `; note` at column 8
  inside a `.func` body. Three neighbours stay green because they are
  already structural: 632 (preamble / between-functions / trailing),
  816 (top-level indented, already normalized to 0), and 842 (a run
  leading into the next `.func`). Post-machine's own fmt test file has
  no such fixture. Updating those three is expected work, not a
  surprise — but they are the change's only automated witness, so they
  should be updated deliberately rather than mechanically.
