# Standard library

`pmt link` always adds a prebuilt `std.pmo` as an implicit last library
unless `--nostdlib` is given (`docs/cli.md`). It ships written in `.pmc`
itself — dogfooding the compiler — and its golden tests double as compiler
tests. All eleven routines take no arguments and share one vocabulary: a
**section** is a maximal run of marked cells on the tape.

## Roster

Every routine's precondition and postcondition below is the normative
contract — it is copied verbatim from the doc comments in
`crates/post-machine/src/stdlib/std.pmc`, which is itself the source of
truth these are compiled from. Under `--strict-cells`
(`docs/isa.md (execution)`), a routine that writes assumes its stated
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
| `goToBlankRight()` | a blank exists strictly right of the head (always true off a finite tape's sections) | head on the nearest such blank. |
| `goToBlankLeft()` | a blank exists strictly left of the head | head on the nearest such blank. |
| `eraseSection()` | head on a mark of a section | section erased; head on the first cell right of where the section was. |
| `appendMark()` | head on a mark of a section | section grown by one mark on the right; head on the new (last) mark. |
| `prependMark()` | head on a mark of a section | section grown by one mark on the left; head on the new (first) mark. |
| `removeLastMark()` | head on a mark of a section | last mark removed; head one cell left of the removed mark (the new last mark, or a blank if the section had one mark). |
| `removeFirstMark()` | head on a mark of a section | first mark removed; head one cell right of the removed mark (the new first mark, or a blank if the section had one mark). |

`appendMark`/`prependMark`/`removeLastMark`/`removeFirstMark` use "first"/
"last" rather than "head"/"tail" to avoid colliding with the machine's own
head.

## Linking semantics

- **Symbol resolution:** duplicate exported symbols across user objects are a link-time error; libraries resolve first-wins and may be shadowed by user objects.
- **Implicit std:** after all user objects are collected and their symbol
  references resolved against each other, any remaining unresolved names
  are matched against `std.pmo` — familiar `libc` semantics. `--nostdlib`
  opts out entirely.
- **Lazy reachability:** linking a library never pulls in more than
  `main` transitively reaches. An unreferenced std routine costs nothing
  in the final `.pmx` — the same dead-function elimination that applies
  to user objects (`docs/formats.md (.pmo)`).
- **Overriding std:** shadowing is an OPT-IN property of exported names.
  To override a namespaced std export, declare a same-named export inside
  the same namespace in your own source:
  ```c
  namespace std {
      export goToEnd() { /* your replacement */ }
  }
  ```
  This is the same symbol, and user code beats the library in that
  arbitration — accidental collision is impossible (local symbols are
  invisible to cross-object resolution, `docs/language.md (visibility)`),
  while a deliberate override is explicit.
- **Interposition vs optimization (semantic-binding caveat):** `-O1`'s
  inline pass binds intra-module calls at compile time. That means
  overriding one of a *library's own internal* callees only affects call
  sites that are still calls after optimization — the linker guarantees
  interposition only for the relocations it actually still sees, which is
  the same semantic-binding default mainstream compilers use. Whether
  `std.pmo` itself needs to be fully interposable (built with
  `--fno-inline`) is a build-configuration decision, not a language rule;
  see `docs/language.md (optimization)` for the general interposition
  caveat.
