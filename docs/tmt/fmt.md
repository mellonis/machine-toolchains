# Formatting `.tmc`/`.tma` — `tmt fmt`

`tmt fmt` reprints a source file to one canonical layout. Each input's
extension picks its formatter: a `.tmc` file goes through the language's
own printer, described on this page; a `.tma` file goes through the
canonical assembly grid shared with the rest of the toolchain
(`docs/formats.md`). The command surface — the directory walk,
`--check`, stdin with `--lang`, exit codes — is `docs/tmt/cli.md`.
Invoked with no PATH arguments at all, `tmt fmt` formats the nearest
project manifest's declared source set
(`docs/tmt/project.md (the declared source set)`) — the set is smaller
and explicitly declared, but each file in it is formatted by exactly the
rules below.

Both rewrites are whitespace-only, which is what makes `--check` a safe
CI gate.

## The four properties

The `.tmc` printer walks the lossless CST rather than the flattened
program, which buys four properties the formatter's test battery
exercises against every fixture in the repository.

**Canonical.** Output depends on the token stream and on the few layout
choices the CST deliberately records — whether a blank line was present,
whether a state was written on one line, and, for a comment written
inside a comma-separated list, whether the author put it on its own line
or trailing an entry. It never depends on the author's spacing. Two files
differing only in horizontal whitespace format identically:

```
alphabet   bit{'_','0'}
machine{tape t:bit;entry state s{[*]->stop;}}
```

and the same program written with spaces sprinkled through it both
produce:

```
alphabet bit { '_', '0' }
machine {
  tape t: bit;
  entry state s { [*] -> stop; }
}
```

The recorded choices are the exception, and they are vertical, not
horizontal: a state the author wrote across several lines stays in block
form even when it would fit on one.

**Idempotent.** `format(format(s)) == format(s)`. Every layout decision
is either derived from token content — widths, the line limit — or from
a property the printer's own output preserves.

**Whitespace-only.** No token is added, dropped, or rewritten. A number
reprints from its written spelling, a glyph reprints with only the two
escapes the lexer accepts, a substitution reprints from its own tokens —
redundant parentheses like `{(v)}` and number spellings like `{v+007}`
survive — bare-name `goto` sugar stays bare, and an omitted transition
stays omitted rather than gaining a `goto`. Renaming, reordering imports,
and normalizing spellings are lint's business or the author's, never
fmt's.

**Trivia-preserving.** Every comment reprints somewhere — see
[Comments](#comments) below for the placement rules, including the one
list kind (a rule's pattern/`write`/`move` vector) whose layout depends
on the comment's kind rather than only on its position.

## Indentation

Two spaces per block level, never tabs. (PM-1's `.pmc` printer uses
four. A `.tmc` rule commonly sits five levels deep — namespace,
namespace, routine, state, rule — where four-space steps would push the
transition table off the right margin.) Output is always LF with exactly
one final newline; an empty file reprints as a single newline.

## The state-block grid

Within a grid group, a state's rules are laid out as a table. The
pattern is padded to the group's widest pattern so every `->` lands in
one column; then the optional action segments — `debugger`, `write
[...]`, `move [...]` — each occupy a column sized to the group's widest
instance.

A rule pads a column it does not use only when it has content in a LATER
column; trailing columns collapse. That is what keeps a bare-transition
row tight against its arrow, which is how these tables are written by
hand:

```
['b'] -> write ['a'] move [>] goto scan;
['a'] ->             move [>] goto scan;
['_'] -> stop;
```

The transition itself is not column-aligned. It is the row's tail, and
padding it would leave a ragged gap in every table whose rules mix
`write`-only and `write`-plus-`move` actions.

A grid group is either one multi-line state's whole rule list — own-line
comments and blank lines inside it do not split the grid, since a state
is one table — or a run of adjacent single-line states.

### Single-line states

`state done { [*] -> stop; }` stays on one line when the author wrote it
that way and it carries no interior comment. A maximal run of adjacent
single-line states, with no blank line or doc run between them, is one
unit: their headers pad to a common width so the `{` column lines up,
and their rules share one grid.

```
entry state go   { ['0'] -> move [>] goto go; [*]   -> goto d; }
state d          { [*]   -> stop; }
state longerName { ['1'] -> stop; [*]   -> stop; }
```

If any member of the run would cross the line limit, the whole run
expands to block form. Expansion is stable, because an expanded state is
no longer written on one line.

## Argument lists and the width threshold

The threshold is the **80-column line limit** — the same width
`line-too-long` enforces on the two assembly dialects (`docs/tmt/lint.md`);
`.tmc` has no line-length lint of its own, so fmt's active wrapping below
is what keeps most `.tmc` lines under it. A parenthesized list — a
`call`'s bindings, a `graft`/`bind`'s bindings, a `routine`/`graph`
signature, an `alphabet` body — renders on one line while the resulting
line fits. Past that it breaks one entry per line, indented two columns
past the construct's first token, with the closing `)` or `}` returning
to that token's column:

```
[*] -> call aRatherLongRoutineNameHere(
         someTapeName = someTapeName,
         anotherTapeName = scratch
       ) then fin;
```

A signature, a `call`/`graft`/`bind` argument list, and a `with map`
pair list also break on a second, width-independent trigger: an interior
comment written inside them, however short, forces the break regardless
of whether the resulting line would have fit
([Comments inside a list](#comments-inside-a-list), below). An
`alphabet` body only escapes that trigger for a SAME-LINE `/* … */`
comment, which stays inline there; a `//` comment, or any own-line
comment, block or line, still forces a break in an `alphabet` body
exactly as it does in every other list — not just because of width.

A single binding argument is never broken further on width alone — a
`with map { … }` with no interior comment of its own stays inline, so one
very long binding may still exceed the limit. That is deliberate:
breaking a map across lines buys little and costs the map its
at-a-glance readability. As noted above, no lint catches the result — a
line this long goes unreported, unlike an unwrappable `.pmc` line, which
`line-too-long` does flag. An interior comment inside that same map does
break it, since the map is itself one of the bracketed lists above.

## Blank lines

The author's choice is preserved, any run of two or more blank lines
collapses to one, and a blank is never forced. A list's first item never
takes a leading blank, which is also what suppresses a blank immediately
after `{`.

## Comments

An own-line comment prints at its block's indent, with each of its
lines' trailing whitespace stripped; a block comment's interior
indentation is content and is left verbatim. Doc (`?`) and attention
(`!`) runs, `[deprecated]` included, stay directly above the declaration
they document, in source order.

### Trailing comments

A trailing comment sits one space after the code by default. In a run of
two or more adjacent single-line entries that all carry one, the
comments align one column past the run's widest code line — every member
of the run aligns, even one whose own code was short enough that its
comment now lands past column 80. Nothing reports that: `.tmc` has no
line-length lint, unlike `.pmc` (`docs/pmt/lint.md`).

Alignment does not consult the author's source columns. A run either
aligns or it does not, decided purely from the reformatted widths — so
these ragged inputs:

```
['0'] -> write ['1'] move [>] goto go; // ragged source column
['1'] -> move [>] goto go; // not aligned at all in source
[*] -> stop; // third
```

come out aligned regardless:

```
['0'] -> write ['1'] move [>] goto go; // ragged source column
['1'] ->             move [>] goto go; // not aligned at all in source
[*]   -> stop;                         // third
```

**This differs from the `.pmc` formatter**, and someone moving between
the two languages will notice. `pmt fmt` aligns a run only when the
author had already aligned it in source, so its output is not a pure
function of the token stream: two `.pmc` files with identical tokens
differing only in where the `//` sat format differently
(`docs/pmt/fmt.md`). `tmt fmt` reads no source columns at all — no
horizontal position in the input can change its output — which is both
simpler to predict and one less way for a second pass to disagree with
the first.

### Comments inside a list

A comment written inside a comma-separated list — an `alphabet` body, a
`routine`/`graph` signature, a `call`/`graft`/`bind` argument list, a
`with map` pair list, a `use` path list, or a rule's pattern / `write` /
`move` vector — prints where it was written, attached to the entry it
sits against.

Placement follows what the author did. A comment that trails an entry
stays on that entry's line; a `//` comment always forces the list onto
multiple lines, because nothing can follow it on its physical line:

```
alphabet bit {
  '_', // the blank
  '0',
  '1'
}
```

An own-line comment keeps its own line, above the entry it precedes:

```
alphabet bit {
  '_',
  // the blank
  '0',
  '1'
}
```

A comment after the last entry prints before the closing delimiter,
still inside the list:

```
alphabet bit {
  '_',
  '0',
  '1' // trailing last
}
```

A SAME-LINE `/* … */` comment (trailing an entry, no `//` beside it) can
stay inline instead of forcing a break, in the list kinds that already
have an inline form for their entries: an `alphabet` body, a `use` path
list, and a rule's pattern / `write` / `move` vector (whose own grid
consequences are described below). The bracketed lists — a signature, a
`call`/`graft`/`bind` argument list, a `with map` pair list — have no
inline-with-comments form, so any interior comment breaks them:

```
alphabet bit { '_', /* the blank */ '0', '1' }
```

An OWN-LINE block comment still forces the break even in those two list
kinds — inlining it would lose the distinction between "trails this
entry" and "precedes the next one".

The bracketed lists — a `routine`/`graph` signature, a
`call`/`graft`/`bind` argument list, and a `with map` pair list — have no
inline-with-comments form: any interior comment there, block or line,
breaks the list across lines, still riding the entry it was written
against:

```
alphabet bits { '_', '0', '1' }

routine w(
  tape t: bits, /* x */
  state d
) {
  entry state s { [*] -> stop; }
}
```

**A pattern, `write`, or `move` vector is different**, because these
three vectors double as the state-block grid's columns (above): a
comment inside one of them decides not just how the vector prints, but
whether its rule stays part of the grid at all.

A same-line `/* … */` comment stays inline inside the vector. The rule
stays a grid row — its width still counts toward the group's shared
columns, so it keeps aligning against its neighbours exactly like an
uncommented rule would.

Anything else — a `//` comment, or any own-line comment, block or line —
cannot sit inline. The rule carrying one comes off the grid and renders
across several lines instead, excluded from the group's width
computation in both directions: it neither pads to match the columns its
neighbours share, nor widens those columns for them.

```
['0', /* lo */ '1'] -> stop;
[*, *]              -> move [>, .] goto s;
[
  '0', // note
  '1'
] -> stop;
[
  /* first */
  '0',
  '1'
] -> stop;
```

The first two rules share one grid: the same-line block comment sets the
pattern column's width, and the plain rule beside it pads to match. The
third and fourth rules both left the grid — one for its `//` comment,
the other for an own-line block comment with no `//` in sight — and
neither one moved the first two rules' shared column.

## `.tma` formatting

A `.tma` file formats through the canonical assembly grid — labels,
mnemonics, and operands in fixed columns — shared with the toolchain's
other assembly dialect and documented with the format itself
(`docs/formats.md`):

```
T0:     .row    [*, 1]
        .row    [*, *]
D0:     .targets hit, miss
F0:     .frame  tapes=(1, 0)
```

The grid is whitespace-only and idempotent on the same terms as the
`.tmc` printer. Rewrapping an overlong line is not part of it, so a line
already over 80 characters stays that way after formatting. Alignment can
also *create* an overlong line that was not one before: a trailing
comment's column is not fixed — it aligns per group at whichever is
wider, 32 or one past the group's widest code (`docs/formats.md`,
"assembly text") — so a short line sharing a group with a much wider one
can have its comment pushed well past column 80 by the alignment alone.
Either way, `line-too-long` reports the result after formatting; the grid
is never capped by the line limit to keep a group's column from moving.
