# Comment-behaviour audit — `.tmc` formatter

Measured 2026-08-30 against `target/release/tmt` (the scratchpad harness,
61 positions × {block, line} × 3 passes = 122 runs), re-measured
2026-08-31 by the executable port `crates/turing-machine/tests/comment_positions.rs`
at the base of the never-move branch — the port's neighbour metric is
STRICTER than the scratchpad's line metric, and every number below comes
from the port's run, not from memory. Evidence base and acceptance
criterion for the never-move plan
(`docs/superpowers/plans/2026-08-30-tmc-comments-never-move.md`,
[#98](https://github.com/mellonis/machine-toolchains/issues/98)).

## The metric

A comment's position is its pair of significant-token NEIGHBOURS — the
nearest non-comment token before and after it in a `WithComments` lex.
The never-move rule holds for a position iff formatting leaves that pair
unchanged. The scratchpad harness compared source lines instead; two
positions that pass line-wise fail neighbour-wise (below).

## Facts that bound the work (0 exceptions in 122 runs)

- **No comment is ever lost.**
- **No fixture fails to parse.**
- **Exactly one shape is non-idempotent**: a BLOCK comment in an
  `alphabet` header (either slot), settling at pass 2. The LINE flavour
  of the same slots settles at pass 1.

## Already correct — the regression guard, not the work

33 of 61 positions keep their neighbours today, held green (with token
preservation and idempotence) by `already_correct_positions_stay`:

- the file level (before the first item, between items, after the last);
- `use` in every slot but one — after the keyword, between paths, before
  the `;`, trailing (the exception is `inside-path`, below);
- just inside every opener (`alphabet`/`namespace`/`machine`/`state`
  `{`, `routine`/`graft` `(`, a `with map` `{`, a pattern/write/move
  vector `[`);
- own line before a closer, and between list elements / rules;
- trailing a closer or a `;` (except `tape`'s, below);
- inside a doc run and between a doc run and its declaration.

## In scope — the four work lists, by measured destination

**The `alphabet` header** (2 positions — `kw-name`, `name-brace`): the
comment lands inside the `{`, between `LBrace` and the first glyph;
block flavour also unstable (pass 2).

**The other header families** (16 positions): `namespace`, `machine`,
`state` (incl. the `entry` slot) drop the comment to its own line inside
the body, right after the `{`; `routine` (`kw-name`, `name-paren`),
`graft` (`kw-target`), `bind` (`kw-target`) put it riding the argument
list's `(`; `routine/paren-brace` puts it after the body's `{`; `tape`
(all four header slots) and `graft/before-as` push it past the whole
statement's `;` — its neighbours become `Semi` and the next statement's
first token.

**Interior list entries** (4 positions): the comment is pushed to the
entry's end — `use a:: /* c */ b;` past `b` (line-identical, so the
scratchpad missed it), a range's comment past `..'z'`, a signature
parameter's past its alphabet, a binding argument's past its value.
NOT all list interiors move: `alphabet/between-elems`, `bind/in-map`,
and both vector interiors already stay — the audit's "all 5 list
surfaces push" was the line metric talking.

**The rule's action slot** (6 positions): a comment after the pattern,
the arrow, `move [>]`, or before the `;` is pushed to the rule's tail
past the `;`; `write /* c */ ['a']` migrates INTO the vector; a
substitution-interior comment is pushed past the `}`.

## The port's two findings beyond the scratchpad

1. `use/inside-path` moves (`a:: /* c */ b` → `a::b /* c */`) — the
   plan's "use, every position, stays" holds only line-wise. It joins
   the list-interior work list.
2. `tape/before-semi` moves past its `;` — the "trailing a closer"
   already-correct group counted only braced closers.

## Raw data

The scratchpad originals (`cases.py`, `run.py`, `group.py`, `inline.py`,
`raw.txt`, `FINDINGS.md`) measured `tmt` at `797d51c`; the executable
port carries the same 61 templates verbatim (verified by mechanical
comparison against `cases.py`) and is the surviving, re-runnable form.
