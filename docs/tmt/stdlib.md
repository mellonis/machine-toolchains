# Standard library

`tmt link` adds the embedded standard library as an implicit last library
unless `--nostdlib` is given (`docs/tmt/cli.md (--nostdlib)`). It ships
written in `.tmc` itself — dogfooding the compiler — and its goldens double
as compiler and optimizer tests.

It offers binary-number arithmetic on a tape, ported from the binary-number
libraries of the `turing-machine-js` project, in **two namespaces**:
`std::binaryNumbers` (ten routines) and `std::binaryNumbersBare` (four).

## Two representations, not two wrapper styles

The split is the first thing to get right, because the two namespaces expose
overlapping operations under the same names and nothing checks the choice
early: calling a delimited routine over a bare tape compiles and links, and
the mismatch surfaces only at run time — here as a `NoTransition` trap, when
the routine looks for a marker the tape's alphabet does not have. The two
differ in **how a number is written on the tape** — the alphabet and the
framing — not in how the routines are packaged.

| | `std::binaryNumbers` | `std::binaryNumbersBare` |
|---|---|---|
| Alphabet | 5 symbols: `'_'`, `'^'`, `'$'`, `'0'`, `'1'` | 3 symbols: `'_'`, `'0'`, `'1'` |
| A number is | `^` digits… `$` — explicitly delimited | a bare run of digits, blanks on both sides |
| Numbers per tape | several, blank-separated (`… ^101$ _ ^10$ …`) | one per blank-delimited region |
| Navigation | safe: the markers say where a number ends | none offered — there is nothing to navigate between |
| Cost | extra states per algorithm to handle `'^'`/`'$'` | much smaller state graphs |

The markers are what the trade is about. They cost states in every algorithm
that has to step over them, and they buy the ability to hold several numbers
on one tape and move between them deliberately. The bare form gives that up
for a three-symbol alphabet and much smaller graphs — bare `plusOne` is two
states against the delimited version's four, and bare `invertNumber` is a
single state.

Each namespace exports its alphabet as `symbols`, which is the normative
statement of the representation. An exported alphabet is a **source-level**
declaration: it contributes no linkable symbol, so a caller in another
compilation unit cannot name it. Declare a local alphabet with the same
glyphs in the same order instead — see below.

## Calling a library routine

Every routine has the same signature shape: one tape parameter named `num`,
typed by its namespace's `symbols` alphabet.

```
export routine plusOne(tape num: symbols)
```

The consumption path across the link boundary is a **transparent call** — a
`call` with an empty argument list. The routine then runs on the caller's
tape, reading and writing through the caller's own alphabet by index, with
the head wherever the caller left it:

```
alphabet a { '_', '0', '1' }

machine {
  tape num: a;
  entry state s { [*] -> call std::binaryNumbersBare::plusOne() then done; }
  state done    { [*] -> stop; }
}
```

Because a transparent call binds by index, **the local alphabet must list the
same glyphs in the same order** as the namespace's `symbols`. The indices are:

```
std::binaryNumbers::symbols        '_'=0  '^'=1  '$'=2  '0'=3  '1'=4
std::binaryNumbersBare::symbols    '_'=0  '0'=1  '1'=2
```

A call that *binds* a tape (`call std::…::plusOne(num = num)`) needs the
callee's tape signature, which the compiler only has for a routine defined in
the same compilation unit; binding into a library routine reports
`external-binding-unsupported` — the compiler names this a limit it has not
lifted yet, not a property of the language. A `graft` of one of the exported graphs is
subject to the same rule for a different reason — a graft splices the graph's
source, so it needs that source in the unit and reports `undefined-graph`
otherwise. Both forms work when the library's source is compiled into the
consumer's own unit; the transparent call is what works against the linked
object.

## Roster

Each routine's contract below is its `?` doc lines in the library source —
the text an editor surfaces on hover — which is what the routines are
compiled from. Head position is part of every contract, on entry and on
exit, and is the part most easily got wrong: several routines leave the head
somewhere data-dependent.

### `std::binaryNumbers` — the delimited representation

Every routine takes `(tape num: symbols)` over the 5-symbol alphabet.

| Routine | On entry | Effect | Head on exit |
|---|---|---|---|
| `goToNumber()` | head on the number, any cell up to and including its `'$'` | tape unchanged | that `'$'` |
| `goToNumbersStart()` | head on the number, any cell from its `'^'` rightward | tape unchanged | that `'^'` |
| `goToNextNumber()` | head on the current number's `'$'`, or the blank gap after it | tape unchanged | the next number's `'$'` |
| `goToPreviousNumber()` | head on the current number's `'$'` | tape unchanged | the previous number's `'$'` |
| `deleteNumber()` | head on the number, any cell | every cell of `'^'`…`'$'` becomes blank | the cell where the `'$'` was |
| `normalizeNumber()` | head on the number | leading `'0'`s stripped; the `'^'` relocates rightward. Zero keeps its form `'^$'` | the `'$'` |
| `plusOne()` | head on the number | adds one; on overflow the number grows one cell left (`'^111$'` → `'^1000$'`) | the `'$'` |
| `minusOneFast()` | head on the number | subtracts one by direct borrow, then normalizes. Zero stays zero (`'^$'` − 1 → `'^$'`) | the `'$'` |
| `invertNumber()` | head on the number | flips every bit | the `'$'` |
| `minusOne()` | head on the number | subtracts one via `x − 1 == ~(~x + 1)`; result normalized (`'^1$'` − 1 → `'^$'`) | the `'$'` |

`deleteNumber`, `normalizeNumber` and `plusOne` treat a head on a blank as a
no-op and leave the tape untouched. `invertNumber` and `minusOne` do not:
they walk left looking for a `'^'`, so they must start on a number.

The two navigators are not symmetric about the gap between numbers.
`goToNextNumber` accepts a head on the blank after a number and reaches the
next one. `goToPreviousNumber` does not: from that blank it steps left, reads
the `'$'` it just left, and stops there — landing on the number it started
after rather than the one before. Enter it from the `'$'` itself.

`minusOneFast` and `minusOne` compute the same function. `minusOneFast` is
the direct borrow subtractor; `minusOne` is the deliberately heavy one,
composed from `invertNumber`, `plusOne`, `invertNumber`, `normalizeNumber`
run in sequence on the same tape — it exists because the composition is worth
showing, not because it is the one to reach for.

### `std::binaryNumbersBare` — the bare representation

Every routine takes `(tape num: symbols)` over the 3-symbol alphabet, and
every one of them expects the head on the **leftmost digit** on entry.

| Routine | Effect | Head on exit |
|---|---|---|
| `plusOne()` | adds one; on overflow the number grows one cell left (`'111'` → `'1000'`) | data-dependent: the digit the carry settled on — the cell that flipped `'0'` → `'1'`, which on overflow is the new leading `'1'` |
| `minusOne()` | subtracts one; the result is **not** normalized, so a borrow that reaches the most significant digit leaves a leading zero (`'1000'` − 1 → `'0111'`) | data-dependent: the cell that flipped `'1'` → `'0'`. On underflow (an empty region) the tape is unchanged and the head sits one cell left, on a blank |
| `invertNumber()` | flips every bit | the trailing blank |
| `normalizeNumber()` | strips leading zeros. All-zeros restores a single `'0'`, so zero keeps its representation | the first `'1'`, or that restored `'0'` |

The bare exit positions are the sharp edge of this namespace: only
`invertNumber` lands somewhere fixed. Chaining two bare routines generally
means repositioning the head between them.

## Anatomy: a graph and its facade

Most operations are defined **once**, as an `export graph` whose exits are
explicit `state` parameters, and then wrapped in a one-line `export routine`
facade that grafts that graph with `done = return`:

```
export graph invertNumberGraph(tape num: symbols, state done) {
  entry state sweep {
    ['0'] -> write ['1'] move [>] goto sweep;
    ['1'] -> write ['0'] move [>] goto sweep;
    ['_'] -> done;
  }
}

export routine invertNumber(tape num: symbols) {
  entry graft invertNumberGraph(num = num, done = return);
}
```

The convention is `<op>Graph` for the behaviour and `<op>` for the facade.
The two forms are the two ways to reuse a world, and they differ in what the
exit is: grafting the graph splices a private copy with **static**
continuations chosen per site, while calling the facade shares one body and
returns **dynamically** to whoever called it
(`docs/tmt/language.md (choosing between them)`).

An `entry graft` may carry an `as NAME` suffix, and the library source writes
one throughout. That name is **optional**: only a *non-entry* graft must be
named, because an unnamed non-entry instance would be unreachable
(`docs/tmt/language.md (graft)`). None of the library's names are referenced,
and dropping them all leaves the compiled object byte-identical. Read the
name as incidental — the pattern is the graft, not its label.

Not every operation fits the shape. `std::binaryNumbers::invertNumber` and
`std::binaryNumbers::minusOne` are plain routines with no graph behind them,
because their bodies are compositions of `call`s — and a `call` inside a
graph body cannot yet be spliced, since the call's binding arguments name the
graph's own signature tapes and its `then` continuation is a graph-space
state. That check fires **at the graft site**, not at the graph's
definition: a graph whose body carries a call compiles without complaint as
long as nothing grafts it.

Only the routine facades become linkable symbols — fourteen of them. Graphs
and alphabets are source-level constructs and contribute none.

## Cross-representation reuse: `invertNumber`

The delimited `invertNumber` does not implement bit-flipping. It calls the
bare one, across the representation boundary, through a symbol map:

```
export routine invertNumber(tape num: symbols) {
  entry state toStart {
    ['^'] -> move [>] goto atFirstDigit;
    [*]   -> move [<] goto toStart;
  }
  state atFirstDigit {
    [*] -> call std::binaryNumbersBare::invertNumber(
             num = num with map { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }
           ) then return;
  }
}
```

It walks left to the `'^'`, steps right onto the first digit — which is the
bare routine's entry contract — and hands the tape over.

The map is where the interest is. The two alphabets have different
cardinalities, so the map is **closed**: every non-blank source symbol it does
not name would be a hole that traps when read
(`docs/tmt/language.md (unequal alphabets)`). All four non-blank delimited
symbols are named, so there are no holes. The digits pair two-way with `->`.
The markers collapse **one-way** onto the callee's blank with `=>`, which is
the legal spelling for many-to-one: two source symbols reading as one image
could not be written back unambiguously, so `=>` declares a read collapse
with no write-back path at all (`docs/tmt/language.md (the two arrows)`).

That collapse is what makes the composition work, and it does two jobs at
once. The bare routine sweeps right flipping bits and stops when it reads a
blank — reading the delimited `'$'` as blank is exactly the stop condition it
needs, so it halts on the end marker and the head lands on the `'$'`, which
is the delimited contract. And the markers themselves survive: the bare
routine never writes a blank, and the one-way arrow gives it no way to write
through those pairs even in principle. The delimited number comes back with
its framing intact.

Linking shows the dependency: a program calling
`std::binaryNumbers::invertNumber` keeps exactly two routines, the delimited
facade and the bare implementation, and drops the other twelve.

## Linking and embedding

The library source is embedded in the toolchain binary as a string rather
than installed as a data file, because a `cargo install`ed binary has no data
directory. There is no on-disk library directory to fall back to.

It is compiled **once per process**, behind a `OnceLock`, at `-O1` with `brk`
stripped — the release preset — which also makes it the optimizer's first
live workload. That build is the one every link uses, whatever level the
consumer's own code was compiled at: linking an `-O0` object still links the
`-O1` library. The compiled object has no `machine` world of its own, being a
library, so nothing is dropped when it is compiled; selection happens
entirely at link.

- **Lazy reachability.** The linker keeps only what the program transitively
  reaches, so an unreferenced routine costs nothing in the final `.tmx`. A
  program calling one bare routine links that one and drops the other
  thirteen; `tmt link -v` reports exactly which
  (`docs/core.md (the linker)`). Reachability follows calls across the
  representation boundary too — the delimited `minusOne` pulls in its three
  delimited callees and the bare `invertNumber` under them.
- **Symbol resolution.** The stdlib is appended *last*, and libraries resolve
  first-wins, so command-line objects and explicit `-l` libraries shadow it.
  Exporting a routine under the same qualified name in your own source
  therefore overrides the library's, silently and by design — it is the same
  symbol, and user code wins that arbitration
  (`docs/tmt/language.md (namespaces)`).
- **`--nostdlib`.** Opts out entirely. A program that still references a
  `std::` name then fails at link with an unresolved symbol
  (`docs/tmt/cli.md (--nostdlib)`).
- **Call lowering is a link-time choice.** How a call to a library routine
  becomes machine behaviour — a stamped specialized copy, a framed call
  through the frame register, or a mix — is selected by `tmt link
  --call-mech` and is orthogonal to the library
  (`docs/tmt/isa.md (call mechanisms)`). The library's goldens run the full
  matrix of both optimization levels against all three lowerings and assert
  every combination reproduces the same hand-derived tape.
