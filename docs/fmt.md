# Formatting `.pmc` — `pmt fmt`

`pmt fmt` reprints a `.pmc` file to one canonical layout — indentation,
label/command alignment, comma-group line breaks, blank lines, and
comment position. It is the fix side of `pmt lint`'s `line-too-long`
finding (`docs/lint.md`): a line that only needs rewrapping, fmt rewraps
it. fmt changes whitespace and comment placement only — it never touches
a token. Leading zeros on a label stay leading zeros, `@goToEnd` stays
`@goToEnd`; renaming, reordering `use` paths, and rewriting numbers are
lint's or the author's job, not fmt's.

## Indentation

4 spaces per block level, never tabs. A `namespace ns { … }` block's
contents sit one level deeper than the namespace header; a function
body sits one level deeper than its header, and a nested function body
one level deeper again. Input tabs and CR line endings are normalized
away by the full reprint: output is always LF, with exactly one final
newline.

## Label and command alignment

Within one function body, every command lines up in a shared **command
column**, and labels right-align into the space before it. The column is
the smallest multiple of 4 that is at least as wide as both the body's
own indent and the widest **inline** label (a label on the same line as
its command) plus 2 — the `+2` reserves a left margin of at least one
space before the label and the one space after its final `:`. An
unlabeled statement indents straight to the command column; a labeled
one right-aligns its `:` so every inline label in the body shares one
colon column, with the command sitting exactly one space after it.

This is the model behind the standard library's own layout — a
two-function excerpt (top-level, inside `namespace std { }`, so the
command column is 8):

```c
namespace std {
    export goToEnd() {
     1: right;
        check(1, 3);
     3: left;
    }

    export goToBegin() {
     1: left;
        check(1, 3);
     3: right;
    }
}
```

A label may also sit on its own line — the author writes a newline
right after its final `:`, and fmt preserves that choice; fmt never
breaks a label itself, only ever moves whitespace around one. An
own-line label that would still fit the label field (its colon lands at
or before the shared colon column) right-aligns like an inline label,
with its command on the following line; one too wide for the field
hangs at a single leading space instead. Neither shape counts toward the
widest-inline-label measurement above. Worked example (command column 8,
set by the five-digit inline label):

```c
foo() {
 11111: right;
    12:
        left;
 999999999:
        halt;
}
```

`12:` is short enough to fit the field and aligns under `11111:`;
`999999999:` overflows it and hangs at one space. Both commands land on
the command column regardless.

## Comma groups

A statement's comma group (`cmd, cmd, cmd;`) keeps the author's own line
breaks and only reflows when a line doesn't fit:

- No newline in the source, and the line fits in 80 characters: one
  line, each `,` tight to the preceding command with one space after.
- No newline in the source, but it overflows 80: greedy-fill — pack
  commands onto the line while they still fit, break after the last
  comma that fit, and continue the remainder on a new line indented to
  the command column, repeating as needed.
- The author already split the group across lines: that grouping is
  preserved verbatim, continuation lines indented to the command column;
  greedy-fill only kicks in on an individual preserved line that itself
  exceeds 80.

```c
foo() {
 1: left, right, mark;
}
```

versus an author-preserved split:

```c
foo() {
 1: left, right,
    mark;
}
```

A statement with no comma to break on — a single long command, most
often a long qualified call — cannot be wrapped and stays over 80
characters; `line-too-long` still reports it.

## Blank lines

The author's blank lines are preserved, runs of two or more collapsed
to one, none forced. fmt never inserts a blank line — not between
declarations, not between statements, not between adjacent `use` lines,
not around a standalone comment. There are exactly two places fmt
removes blank lines the author wrote, both edits to existing blanks,
never a fresh insertion elsewhere: a run of two or more consecutive
blank lines collapses to one, anywhere; and a blank line sitting
immediately after a body's opening `{` or immediately before its
closing `}` is stripped entirely (to zero, not one), so a blank right at
a body's edge never survives even as a single line. Everywhere else, a
single blank the author left stays exactly as written, and its absence
is never filled in. There is deliberately no "one blank line between
declarations" rule to enforce.

## Comments

Every comment keeps its position relative to the code around it: a
comment on its own line before a declaration or statement stays there
(and travels with it — a blank line the author placed above the comment
stays above the comment, not between the comment and what it documents);
a comment dangling before a body's closing brace prints at the body's
indent; block-comment interiors are reprinted verbatim, untouched.

A trailing comment (same line as the statement it follows) gets one
space before its `//` by default. When the author aligned a run of two
or more trailing comments in a column, fmt keeps them aligned — the
column is recomputed from the reformatted code (the longest line in the
run, plus one space), so the alignment survives even though the code
around it reflowed. If keeping a comment aligned would push its line
past 80 characters, that one line falls back to a single space instead
(and may then be reported by `line-too-long`); the rest of the run stays
aligned.

## Spacing

Canonical intra-statement spacing, independent of what the source wrote:

| Construct | Canonical |
|---|---|
| Call | `@name(...)` — `@` tight to the name, name tight to `(` |
| Builtin + successor | `left(5)`, `mark(!)` — no space before `(`, contents tight |
| `check` | `check(1, 3)` — tight `(`, one space after the arm comma, tight `)` |
| `goto` | `goto 5` — one space |
| Path | `std::x` — `::` tight |
| `,` / `;` | tight to the preceding token; one space after `,`, newline after `;` |
| `as` | one space each side: `their::name as alias` |
| `!` | tight — `(!)`, `check(!, 1)` |

A spaced form the grammar still accepts (`1 : right;`, `std :: goToEnd`)
is normalized to the tight form above; fmt never strips a token, so a
mandatory pair of call parens (`@f();`) is left exactly as written.

## `--check`, stdin, and exit codes

```
pmt fmt PATH... [--exclude PATH]... [--check]
pmt fmt -       [--check]
```

By default `pmt fmt` rewrites each file in place, and only when its
formatted text differs from what's on disk — an already-formatted file
is never rewritten, so running fmt does not churn file modification
times across a clean tree. `--check` writes nothing: it lists the path
of every file whose formatted text would differ, and exits 1 if any
did (0 if none did) — the CI-friendly mode. `-` reads one `.pmc` from
stdin and writes the formatted text to stdout, for editors without an
LSP hooked up and for shell pipelines or git filters; `-` cannot be
combined with `PATH` arguments. `- --check` mirrors the same semantics
against stdin: nothing is written either way, and the exit code alone
says whether the input would change.

Exit codes: 0 = success — every file (or stdin) is already canonical, or
was rewritten in place; 1 = under `--check`, at least one input would
change, or a lex/parse error occurred anywhere in the batch. A file that
fails to lex or parse is reported and left untouched; with a directory
walk, the rest of the batch still runs.
