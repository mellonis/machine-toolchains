# Standard library

`pmt link` always adds a prebuilt `std.pmo` as an implicit last library
unless `--nostdlib` is given (`docs/pmt/cli.md`). It ships written in `.pmc`
itself — dogfooding the compiler — and its golden tests double as compiler
tests. All eleven routines take no arguments and share one vocabulary: a
**section** is a maximal run of marked cells on the tape.

## Roster

Every routine's precondition and postcondition below mirrors the `?` doc
lines in `crates/post-machine/src/stdlib/std.pmc` — the text an editor
surfaces on hover for a `std::` call — which is itself the source of
truth these are compiled from. Under `--strict-cells`
(`docs/pmt/isa.md (execution)`), a routine that writes assumes its stated
precondition holds: `eraseSection` and the `remove*` routines only unmark
cells that are marked, and `appendMark`/`prependMark` only mark cells that
are blank, whenever their precondition is met — so strict-cells mode does
not trap on a correctly-preconditioned call. Every routine returns
normally (no `halt`, no trap) whenever its precondition holds; none of them
touch tape cells outside the section (and, for `appendMark`/`prependMark`,
its immediate new edge cell).

| Routine | Precondition | Postcondition |
|---|---|---|
| `goToEnd()` | head on a mark of a section | head on the section's LAST mark; tape unchanged. (The historic Sum.pms pair.) |
| `goToBegin()` | head on a mark of a section | head on the section's FIRST mark; tape unchanged. |
| `goToMarkRight()` | a mark exists strictly right of the head | head on the nearest such mark; tape unchanged. |
| `goToMarkLeft()` | a mark exists strictly left of the head | head on the nearest such mark; tape unchanged. |
| `goToBlankRight()` | a blank exists strictly right of the head (always true off a finite tape's sections) | head on the nearest such blank; tape unchanged. |
| `goToBlankLeft()` | a blank exists strictly left of the head | head on the nearest such blank; tape unchanged. |
| `eraseSection()` | head on a mark of a section | section erased; head on the first cell right of where the section was. |
| `appendMark()` | head on a mark of a section | section grown by one mark on the right; head on the new (last) mark. |
| `prependMark()` | head on a mark of a section | section grown by one mark on the left; head on the new (first) mark. |
| `removeLastMark()` | head on a mark of a section | last mark removed; head one cell left of the removed mark (the new last mark, or a blank if the section had one mark). |
| `removeFirstMark()` | head on a mark of a section | first mark removed; head one cell right of the removed mark (the new first mark, or a blank if the section had one mark). |

`appendMark`/`prependMark`/`removeLastMark`/`removeFirstMark` use "first"/
"last" rather than "head"/"tail" to avoid colliding with the machine's own
head.

## Linking semantics

- **Symbol resolution:** an exported name has one definition per link.
  Two user objects defining one name are a link-time error; so is a name
  a library also defines (another library, or the embedded standard
  library) once the program reaches it (`docs/core.md (linking)`). A name
  nothing calls never collides.
- **Implicit std:** after all user objects are collected and their symbol
  references resolved against each other, any remaining unresolved names
  are matched against `std.pmo` — familiar `libc` semantics. `--nostdlib`
  opts out entirely.
- **Lazy reachability:** linking a library never pulls in more than
  `main` transitively reaches. An unreferenced std routine costs nothing
  in the final `.pmx` — the same dead-function elimination that applies
  to user objects (`docs/formats.md (.pmo)`).
- **Replacing std:** a library routine cannot be replaced one at a
  time — defining a `std::` name the program calls is a duplicate-symbol
  link error. To replace the library, link without it (`--nostdlib`) and
  supply your own set. Accidental collision is impossible in either case
  (local symbols are invisible to cross-object resolution,
  `docs/pmt/language.md (visibility)`).
- **Both build columns ship:** the embedded library is one object,
  compiled once per process, and it carries both build columns
  (`docs/pmt/language.md (volatile programs)`) — so a volatile program
  and a plain one link the bodies each needs out of the same `std.pmo`,
  with no second library and nothing to select at build time. The dedup
  keeps that cheap: only `eraseSection`, `removeFirstMark`, and
  `removeLastMark` compile to two different bodies; the other eight are
  identical either way and ship as a single blob serving both. One
  consequence is worth knowing: the library is compiled at `-O1`
  regardless of the level on your command line, so a `-O0` build of a
  volatile program that reaches one of those three still links its gated
  body, and its image differs from the same program without the modifier
  (`docs/pmt/optimizer.md (volatile builds)`).
- **No interposition:** the library's own internal calls are bound inside
  the library object, and nothing can interpose on them.
