# Standard library

`tmt link` adds the embedded standard library as an implicit last library
unless `--nostdlib` is given (`docs/tmt/cli.md (--nostdlib)`). It ships
written in `.tmc` itself — dogfooding the compiler — and its goldens double
as compiler and optimizer tests.

It offers binary-number arithmetic on a tape, ported from the binary-number
libraries of the `turing-machine-js` project, in **two representations**:
`std::binaryNumbers` (ten routines) and `std::binaryNumbersBare` (four). Each
is mirrored by a volatile twin namespace, `std::binaryNumbersVolatile` and
`std::binaryNumbersBareVolatile`, for programs whose band is a device — see
*The volatile twins* below.

## Two representations, not two wrapper styles

The split is the first thing to get right, because the two representations
expose overlapping operations under the same names and nothing checks the
choice early: calling a delimited routine over a bare tape compiles and
links, and the mismatch surfaces only at run time — here as a `NoTransition`
trap, when the routine looks for a marker the tape's alphabet does not have.
The two differ in **how a number is written on the tape** — the alphabet and
the framing — not in how the routines are packaged.

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

Five of the fourteen routines below additionally declare a machine-checked
`writes`/`preserves` clause (`docs/tmt/language.md (contract clauses)`) on
their `num` parameter, formalizing part of the same `?` doc-line contract
as grammar the compiler now enforces, rather than leaving it as prose
alone. The **Contract** column in each table below names the clause where
one exists; a routine with no entry there makes no machine-checked claim
about what it writes, though its doc-line prose still describes the
behavior, as prose always has.

Not every doc-line guarantee is expressible that way, and its absence from
the Contract column is not an oversight. `deleteNumber`, `normalizeNumber`,
`plusOne`, and `minusOneFast` in the delimited namespace, and `minusOne` in
the bare one, each document a **conditional** tape-unchanged guarantee — the
delimited four only when the head starts off a number, the bare one only on
underflow — and a `writes`/`preserves` clause is an unconditional promise
about every run of the routine, so a guarantee that holds on only one input
shape cannot be written as one. Their doc-line prose remains the only
statement of it, by design.

### `std::binaryNumbers` — the delimited representation

Every routine takes a single tape parameter, `num`, typed by the namespace's
5-symbol `symbols` alphabet — see the Contract column below for the four
that also declare a `writes` clause.

| Routine | On entry | Effect | Head on exit | Contract |
|---|---|---|---|---|
| `goToNumber()` | head on the number, any cell up to and including its `'$'` | tape unchanged | that `'$'` | `writes {}` |
| `goToNumbersStart()` | head on the number, any cell from its `'^'` rightward | tape unchanged | that `'^'` | `writes {}` |
| `goToNextNumber()` | head on the current number's `'$'`, or the blank gap after it | tape unchanged | the next number's `'$'` | `writes {}` |
| `goToPreviousNumber()` | head on the current number's `'$'` | tape unchanged | the previous number's `'$'` | `writes {}` |
| `deleteNumber()` | head on the number, any cell | every cell of `'^'`…`'$'` becomes blank | the cell where the `'$'` was | — |
| `normalizeNumber()` | head on the number | leading `'0'`s stripped; the `'^'` relocates rightward. Zero keeps its form `'^$'` | the `'$'` | — |
| `plusOne()` | head on the number | adds one; on overflow the number grows one cell left (`'^111$'` → `'^1000$'`) | the `'$'` | — |
| `minusOneFast()` | head on the number | subtracts one by direct borrow, then normalizes. Zero stays zero (`'^$'` − 1 → `'^$'`) | the `'$'` | — |
| `invertNumber()` | head on the number | flips every bit | the `'$'` | — |
| `minusOne()` | head on the number | subtracts one via `x − 1 == ~(~x + 1)`; result normalized (`'^1$'` − 1 → `'^$'`) | the `'$'` | — |

`deleteNumber`, `normalizeNumber`, `plusOne`, and `minusOneFast` treat a
head on a blank as a no-op and leave the tape untouched. `invertNumber` and
`minusOne` do not: they walk left looking for a `'^'`, so they must start
on a number.

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

Every routine takes a single tape parameter, `num`, typed by the namespace's
3-symbol `symbols` alphabet, and every one of them expects the head on the
**leftmost digit** on entry — see the Contract column below for the one
that also declares a `preserves` clause.

| Routine | Effect | Head on exit | Contract |
|---|---|---|---|
| `plusOne()` | adds one; on overflow the number grows one cell left (`'111'` → `'1000'`) | data-dependent: the digit the carry settled on — the cell that flipped `'0'` → `'1'`, which on overflow is the new leading `'1'` | — |
| `minusOne()` | subtracts one; the result is **not** normalized, so a borrow that reaches the most significant digit leaves a leading zero (`'1000'` − 1 → `'0111'`) | data-dependent: the cell that flipped `'1'` → `'0'`. On underflow (an empty region) the tape is unchanged and the head sits one cell left, on a blank | — |
| `invertNumber()` | flips every bit | the trailing blank | `preserves { '_' }` |
| `normalizeNumber()` | strips leading zeros. All-zeros restores a single `'0'`, so zero keeps its representation | the first `'1'`, or that restored `'0'` | — |

The bare exit positions are the sharp edge of this namespace: only
`invertNumber` lands somewhere fixed. Chaining two bare routines generally
means repositioning the head between them.

## Anatomy: a graph and its facade

Most operations are defined **once**, as an `export graph` whose exits are
explicit `state` parameters, and then wrapped in a one-line `export routine`
facade that grafts that graph with `done = return`:

```
export graph invertNumberGraph(
  tape num: symbols preserves { '_' },
  state done
) {
  entry state sweep {
    ['0'] -> write ['1'] move [>] goto sweep;
    ['1'] -> write ['0'] move [>] goto sweep;
    ['_'] -> done;
  }
}

export routine invertNumber(tape num: symbols preserves { '_' }) {
  entry graft invertNumberGraph(num = num, done = return);
}
```

The convention is `<op>Graph` for the behaviour and `<op>` for the facade.
The two forms are the two ways to reuse a world, and they differ in what the
exit is: grafting the graph splices a private copy with **static**
continuations chosen per site, while calling the facade shares one body and
returns **dynamically** to whoever called it
(`docs/tmt/language.md (choosing between them)`).

An `entry graft` may carry an optional `as NAME` suffix; a *non-entry* graft
must be named, because nothing could otherwise `goto` its spliced instance and
it would be unreachable (`docs/tmt/language.md (graft)`). The library names
its non-entry grafts — the ones its own `goto`s enter — and leaves its entry
grafts unnamed, an entry graft's name being only a label nothing references.
Such a name is inert: dropping an unused one leaves the compiled object
byte-identical, because it is source-level and contributes nothing linkable.

Not every operation fits the shape. `std::binaryNumbers::invertNumber` and
`std::binaryNumbers::minusOne` are plain routines with no graph behind them,
because their bodies are compositions of `call`s — and a `call` inside a
graph body cannot yet be spliced, since the call's binding arguments name the
graph's own signature tapes and its `then` continuation is a graph-space
state. That check fires **at the graft site**, not at the graph's
definition: a graph whose body carries a call compiles without complaint as
long as nothing grafts it.

Only the routine facades become linkable symbols — **twenty-eight** of
them: the fourteen across the two representations above, plus fourteen
more — a volatile twin of each, under the SAME name, in its own
namespace (see *The volatile twins*, below). Graphs and alphabets are
source-level constructs and contribute none.

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
facade and the bare implementation, and drops the other twenty-six.

## The volatile twins

Each of the two representations is mirrored by a **volatile twin** namespace:

| Representation | Plain namespace | Volatile twin |
|---|---|---|
| delimited | `std::binaryNumbers` | `std::binaryNumbersVolatile` |
| bare | `std::binaryNumbersBare` | `std::binaryNumbersBareVolatile` |

A twin exports the same routine names as its counterpart, under the same
contracts — including the same `writes`/`preserves` clause, verbatim,
wherever the counterpart declares one (the Contract column above names
which five) — computing the same thing. It differs in exactly one way:
its tape parameter is declared `volatile`
(`docs/tmt/language.md (volatile tapes)`).

```
export routine plusOne(volatile tape num: symbols)
```

The naming rule is the whole convention, and it is a rule about namespaces:
append `Volatile` to the namespace, never to the routine. It is
`std::binaryNumbersVolatile::plusOne`, not
`std::binaryNumbers::plusOneVolatile`. Choosing the namespace once at the top
of a program is also the point — the mark is a property of the band, so it
should not be a decision taken again at each call site.

### Why a second namespace rather than something said at the call site

A volatile band is one whose contents the program does not own: a device
other machinery may read or write between two steps. Nothing may be assumed
about a value written there earlier, and no access to it may be moved or
dropped. Writing `volatile tape` on a machine's own tape declaration says
that about the machine's own code.

It says nothing whatsoever about a library routine that code calls. The
standard library reaches a program at link time already compiled, from a
source the program never sees; whatever the caller declared, the routine on
the other side of that call was compiled from its OWN signature. Nor does
inlining rescue it — a call into a linked object has no body on the caller's
side to splice. So for the mark to reach the callee at all, it has to be in
the callee's own source, in force when the library itself was compiled. That
is what a twin is: the same library, compiled with the band declared
volatile.

Nothing in the library forces the choice. A program whose tape is ordinary
memory calls the plain namespace and gets an ordinary routine; a program
whose band is a device calls the twin.

### The same graphs, and a chain that keeps the mark

The twins duplicate no behaviour. Twelve of the fourteen twin routines are
graph-backed facades, and each grafts the **same** exported graph its plain
counterpart grafts — there is no `…Volatile` copy of any graph, and every
behaviour is still defined exactly once, in one `?`-documented place.

That works because a graft is governed by its host. The spliced rows land on
whatever tape the host binds into the graph's parameter, so a graph written
with no `volatile` keyword anywhere in it becomes volatile rows the moment a
twin facade splices it — and the same graph spliced into a plain facade
stays ordinary. A twin importing its counterpart's `symbols` alphabet rather
than restating it is the same idea one level down: the representation, like
the behaviour, is stated once.

The other two routines — the delimited `invertNumber` and `minusOne` — are
compositions of calls rather than graph facades, and they mirror their chains
with every call **retargeted to its callee's twin**. The delimited
`invertNumber` calls `std::binaryNumbersBareVolatile::invertNumber`, not the
bare namespace's plain one; `minusOne`'s four legs are its own namespace's
routines. A twin that called a plain routine would drop the mark at that one
boundary, and the rest of the chain would run as if the band were ordinary
memory — the mistake the namespace split exists to keep out of the library.

### Byte-identical, for now

Today a twin compiles to exactly the same bytes as its counterpart. Every
pass in the pipeline already preserves per-band access sequences, so nothing
currently gates on the volatile mark and there is nothing for it to change
(`docs/tmt/optimizer.md (volatile barrier)`).

That is a fact about the current optimizer, not a property of the twins, and
the library's tests say so explicitly: one test asserts the byte identity and
carries a standing obligation to be retired by the first pass that assumes
values on a non-volatile band, while a separate test asserts the functional
equivalence — same outcome, same final tape, across both optimization levels
and all three call lowerings — that must hold whatever the optimizer learns
to do. When the twins do start costing more than their counterparts, that is
the mark working.

In every other respect a twin is an ordinary exported routine: it links, it
is reachable or dropped, and it is shadowed by a same-named definition of
your own on exactly the same terms as the routine it mirrors.

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
  twenty-seven; `tmt link -v` reports exactly which
  (`docs/core.md (the linker)`). Reachability follows calls across the
  representation boundary too — the delimited `minusOne` pulls in its three
  delimited callees and the bare `invertNumber` under them, and the twin of
  that same `minusOne` pulls in the twins of all four.
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
