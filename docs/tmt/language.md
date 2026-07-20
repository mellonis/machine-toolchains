# The `.tmc` language reference

The `.tmc` language version is **0.1** (pre-1.0: the version is `0.N` and
`N` bumps on any grammar change; at a declared 1.0 the axes activate —
major = breaking acceptance change, minor = additive syntax; no patch
digit — spec-text corrections are errata, implementation-conformance
fixes live in the crate changelog). This is the language's first cut, so
every statement on this page describes 0.1. See "Grammar version
history" at the end for the version scheme's own record.

`.tmc` is the source language for the TM-1 toolchain. A program is a set
of **states**, each a list of **rules**; a rule matches what the heads
read across every tape and says what to write, where to move, and which
state to enter next. There are no expressions, no variables that outlive
a rule, and no control flow beyond the state graph — the language is a
notation for a multi-tape Turing machine, not a procedural language over
one.

A `.tmc` file compiles to a `.tmo` object (`tmt compile`); see
`docs/tmt/cli.md` for the compiler's flags, `docs/tmt/isa.md` for the
machine the generated code runs on, and `docs/tmt/stdlib.md` for the
routines that ship with the toolchain.

```
? Walk right; replace every 'b' with 'a'; stop at the first blank.

alphabet ab { '_', 'a', 'b' }

machine {
  tape main: ab;

  entry state scan {
    ['b'] -> write ['a'] move [>] goto scan;
    ['a'] ->            move [>] goto scan;
    ['_'] -> stop;
  }
}
```

## Program structure

A file is a sequence of top-level items in any order:

- `alphabet` declarations,
- `machine`, `routine`, and `graph` blocks — collectively **worlds**,
- `namespace` blocks, which nest and may contain any of the above,
- `use` imports.

A file with a `machine` block is a **program**; a file without one is a
**library**. A file may contain at most one `machine` block; a second is
a compile error.

A machine compiles to the symbol `main`, which is the linker's default
entry point. Two machines across one link are a duplicate-symbol error
and none at all is an unresolved-entry error (`tmt link --entry` can
name a different entry; see `docs/tmt/cli.md`). Because the machine
claims that name, a program may not also declare a top-level `main`
routine or graph.

Identifiers are Unicode: the first character must be alphabetic (Unicode
`Alphabetic`) or `_`, every following character alphanumeric or `_`.
That is exactly the `.tma` symbol grammar (`docs/formats.md (assembly
text)`), so every compiled name survives the trip through generated
assembly unchanged. Identifiers are case-sensitive, and no identifier
may be one of the reserved keywords (see "Reserved keywords").

Comments are `//` to end of line and `/* … */` blocks. A line whose first
non-whitespace character is `?` or `!` is not a comment but a doc or
attention line — real grammar, described under "Doc lines and attention
lines".

Two literal forms name symbols. A **glyph literal** is single-quoted:
`'a'`, `'_'`, `'^'`. Its content is any non-empty UTF-8 string, so one
grapheme, an emoji, or a multi-scalar sequence are each a single glyph;
`\'` and `\\` are the only escapes, and any other backslash sequence, an
empty `''`, or a literal that reaches end-of-line unclosed is a lex
error. A **numeric literal** is a bare decimal: `0`, `126`. The two forms
share one label space — see "Alphabets".

### Worlds

A world is a named state graph with a tape signature. The three kinds
differ in how they are entered and how they leave.

| Kind | Declares tapes | Entered by | May leave via |
|---|---|---|---|
| `machine` | `tape` declarations in its body | the program's entry point | `stop`, `halt` |
| `routine` | `tape` parameters in its signature | `call` | `return`, `stop`, `halt` |
| `graph` | `tape` parameters in its signature | `graft` | its `state` parameters, `stop`, `halt` |

The `machine` block is the program: it declares the physical tapes, and
its entry is where execution begins.

A `routine` is a callable subprogram. It compiles to one shared body that
every call site jumps into, and `return` hands control back to whichever
site called it. `return` is legal only inside a routine; writing it in a
`graph` or `machine` body is a compile error.

A `graph` is a reusable *pattern* of behaviour rather than a callable
body. It has no return: it names its exits as `state` parameters, and
each graft site says which of its own states each exit leads to. A graph
is spliced into its host at compile time — one private copy per graft
site — so its continuations are static.

```
alphabet ab { '_', 'a', 'b' }

routine touch(tape t: ab) {
  entry state s { [*] -> write ['a'] return; }
}

graph findA(tape t: ab, state hit, state miss) {
  entry state s {
    ['a'] -> hit;
    ['_'] -> miss;
    [*]   -> move [>] goto s;
  }
}

machine {
  tape main: ab;
  entry graft findA(t = main, hit = shout, miss = giveUp) as seek;
  state shout  { [*] -> call touch(t = main) then done; }
  state giveUp { [*] -> halt; }
  state done   { [*] -> stop; }
}
```

### Entry

Every world marks exactly one entry, written `entry state NAME { … }` or
`entry graft …`. A world with no entry, or with more than one, is a
compile error. `entry` attaches only to `state` and `graft`; `entry
bind` is a parse error.

## Alphabets

An `alphabet` declaration names an ordered set of symbols:

```
alphabet ab     { '_', 'a', 'b' }        // three glyphs
alphabet bytes  { 0..126 }               // 127 numeric symbols
alphabet chars  { '_', 'a'..'e' }        // blank plus a glyph range
alphabet mixed  { '_', 'x', 0..3 }       // the two literal forms may mix
```

Elements are listed in **position order**, and a symbol's position is its
index on the tape. **Index 0 is always the blank** — whatever glyph is
written first is the blank, and nothing else about it is special. The
blank need not be spelled `'_'`; that is only a convention this
toolchain's own sources follow.

```
alphabet fancy { 'blank', 'ok', '🙂' }   // 'blank' is the blank
```

A range element `lo..hi` expands in place, inclusive and ascending.
Numeric ranges mint one symbol per value; glyph ranges walk Unicode
scalar succession and therefore require single-scalar endpoints
(`'a'..'e'` is fine, `'ab'..'az'` is not). Descending or mixed-kind
endpoints are rejected.

Every symbol carries a **glyph label**, and labels must be unique within
an alphabet. A numeric literal's label is its value's decimal string, so
`5` and `05` are the same symbol, and the quoted `'0'` and the bare `0`
are the same symbol too:

```
alphabet bad1 { 0, 5, 05 }      // error: duplicate glyph `5`
alphabet bad2 { '_', '0', 0 }   // error: duplicate glyph `0`
```

An alphabet must have at least one element, and may resolve to at most
**127** symbols. The ceiling is the instruction encoding's: TM-1 names
symbol indices `0`..`126` and reserves `0x7F` as the transparent marker,
so a wider alphabet has symbols no instruction could mention
(`docs/tmt/isa.md (tapes and alphabets)`). `alphabet big { 0..127 }` is
128 symbols and is rejected.

Alphabets may be declared inside namespaces and `export`ed like any other
item.

## Tapes and heads

Each tape is unbounded in both directions and carries one head. A tape is
bound to exactly one alphabet, and that binding is what gives its cells
meaning: the machine stores symbol *indices*, and glyphs exist only for
presentation (`docs/tmt/isa.md (tapes and alphabets)`).

A `machine` declares its tapes directly; the declaration order is the
**vector position order** every rule in that world uses:

```
machine {
  tape src: bits;    // vector position 0
  tape dst: bits;    // vector position 1
  …
}
```

A `routine` or `graph` takes its tapes as signature parameters instead,
in the same positional sense:

```
routine plusOne(tape num: bits) { … }
graph findX(tape t: marks, state found, state missing) { … }
```

A `tape` declaration is grammatical only in a `machine` body — a routine
or graph that wants a tape declares a parameter. A world may have at most
sixteen tapes, matching the architecture's width
(`docs/tmt/isa.md (processor architecture)`), and tape names must be
unique within their world.

Signature parameters come in two kinds, `tape NAME: ALPHABET` and `state
NAME`; the latter are exit parameters, covered under "`graft`". Parameter
names must be unique within a signature.

## Rules

### The rule triple

Every rule is three parts in fixed order, terminated by `;`:

```
[pattern] -> action transition;
```

- The **pattern** is a bracketed vector with one cell per tape. It says
  what the heads must read for this rule to fire.
- The **action** is an optional `write [vector]`, an optional `move
  [vector]`, and an optional leading `debugger`. Either or both vectors
  may be omitted; a rule with neither reads and transitions without
  disturbing the tapes.
- The **transition** says which state runs next, or that the machine
  stops.

Pattern, write, and move vectors must each have exactly as many cells as
the world has tapes. A width mismatch is a compile error naming the
world's arity.

```
entry state s {
  ['a', *] -> write [-, 'b'] move [>, .] goto s;
  ['b', *] ->                move [<, <] goto s;
  [*, *]   -> write ['a', -] stop;
}
```

### Pattern cells

Each cell position matches the symbol under that tape's head. A cell is
one of:

| Cell | Matches |
|---|---|
| `'a'` or `7` | exactly that symbol |
| `'a'..'d'` or `1..125` | every symbol in that inclusive range |
| `*` | every symbol on that tape — a wildcard |

A cell may bind what it matched with `as NAME`, making the matched symbol
available to the write vector as a substitution (see "Range expansion and
substitution"). A binding on a wildcard is rejected:

```
[* as v] -> …    // error: bind an explicit range so the expansion cost is visible
```

A rule whose every cell is `*` is the state's **catch-all**.

### Write and move vectors

A write cell is a literal symbol, a substitution `{name}` / `{name±k}`,
or `-` meaning **keep** the cell's current symbol. A move cell is `<`
(left), `>` (right), or `.` (stay). Omitting the whole `write` vector
keeps every cell; omitting the whole `move` vector leaves every head
where it is.

A written symbol must exist in that tape's alphabet.

### Transitions

| Transition | Effect |
|---|---|
| `goto NAME` | enter state `NAME` in this world |
| `NAME` | the same thing — the bare-name sugar |
| `call TARGET(args) then CONT` | run a routine, then continue at `CONT` |
| `return` | leave this routine, back to its caller — routines only |
| `stop` | normal termination |
| `halt` | abnormal termination |

`stop` and `halt` are the machine's two terminations; `tmt run` exits 0
on `stop` and 2 on `halt` (`docs/tmt/isa.md (execution)`). A `call`'s
continuation `CONT` may be a state name or any of `return`, `stop`,
`halt`, under the same rules.

A leading `debugger` in the action emits a breakpoint the debugger
surfaces; `tmt compile --strip-debugger` drops them
(`docs/tmt/cli.md`).

### Which rule fires

Rules within one state are **not** tried in source order. Ranges and
bindings expand first (see "Range expansion and substitution"), and the
code generator then sorts the resulting rows into three bands, taking
the first match in that order (`docs/tmt/isa.md (match and dispatch)`):

1. **Exact rows** — every cell a concrete symbol. A wildcard-free rule
   lands here, including one that reached concreteness by expanding a
   range. Dispatched through a match table.
2. **Partial rows** — some cells concrete, some wildcard. Within this
   band, source order decides.
3. **The catch-all row** — every cell `*`. Always last.

The consequence worth internalising: a wildcard-carrying rule written
*before* a more specific one does not shadow it. In this state the
second rule fires whenever the cell holds `'a'`, even though the
catch-all is written first:

```
entry state s {
  [*]   -> stop;    // fires on '_' and 'b'
  ['a'] -> halt;    // fires on 'a' — exact band beats catch-all
}
```

Rows in the exact band may never overlap: two wildcard-free rules that
match the same input are an **exact-row conflict**, rejected at compile
time rather than silently resolved by order. The check is on the
expanded rows, so two ranges that share a single symbol collide just as
two identical literals do. Because the exact band is disjoint, sorting
it is behaviour-preserving.

```
['a'] -> stop;
['a'] -> halt;        // error: two rules match the same input

['a'..'b'] -> stop;
['b'..'c'] -> halt;   // error: both expand a row matching 'b'
```

Overlap that *does* involve a wildcard is legal, and band order plus
source order resolve it. A rule the bands can prove unreachable is a
lint finding rather than an error (`docs/tmt/lint.md`).

A state need not be total. When no rule matches, no catch-all is
synthesized: the dispatch finds nothing and the machine takes the
`NoTransition` trap (`docs/tmt/isa.md (execution)`), which `tmt run`
reports as exit 3. Falling off a state is therefore a diagnosable
runtime event, not undefined behaviour.

## Reuse: `call`, `graft`, and `bind`

Three constructs reuse a world elsewhere. They differ in what is shared
at run time and in when continuations are decided.

### `call`

`call` invokes a `routine`. One body is shared by every call site, and
`return` goes back to whichever site is on the stack — a dynamic return.

```
entry state s { [*] -> call plusOne(num = data) then done; }
```

The argument list binds the callee's tape parameters to the caller's
tapes by parameter name, optionally through a symbol map (see "Symbol
maps"). When the callee is defined in this compilation unit its
signature is known, and **every** parameter must be bound: a missing,
duplicate, or unrecognized argument name is a compile error naming the
parameter.

A call whose target lives in *another* compilation unit is the opposite
case — it must not bind tapes at all. Projecting them would need the
callee's signature, which this unit does not have, so the empty list is
the only legal form and the linker resolves the symbol.

```
use hidden;
…
[*] -> call hidden() then done;          // fine — resolved at link
[*] -> call hidden(t = main) then done;  // error: needs hidden's tape signature
```

### `graft`

`graft` splices a `graph` into the host world at compile time. Each graft
site gets its own private copy of the graph's states, and the graph's
exit parameters are wired to host states at the site — a static
continuation, no return stack involved.

```
entry graft findX(t = work, found = celebrate, missing = giveUp) as seek;
```

Each `state` parameter in the graph's signature must be bound at the
site, to a state of the host world or to a terminator (`stop`, `halt`,
or — inside a routine — `return`). That last form is what turns a
behaviour graph into a callable routine:

```
export routine goToNumber(tape num: symbols) {
  entry graft goToNumberGraph(num = num, done = return) as body;
}
```

A graft instance is named with `as NAME`, and the name is what other
rules `goto` to enter the spliced copy. Only an `entry graft` may omit
the name, since an unnamed non-entry instance would be unreachable.

Grafts nest: a graph may graft another graph, and splicing recurses.
0.1 rejects a graft whose graph body contains a `call`, reporting it at
the graft site.

### `bind`

`bind` declares a named, pre-bound call target: the argument list is
fixed once at the declaration, and call sites then invoke it by name with
an empty argument list.

```
machine {
  tape main: ab;
  bind helper(t = main) as h;
  entry state s { [*] -> call h() then done; }
  state done   { [*] -> stop; }
}
```

A bind is a call target, not a state — `goto h` is a compile error.

### Choosing between them

- `call` when the body should be shared and the continuation should
  depend on the caller. It costs a return-stack frame and, across
  differing tape shapes, a frame projection.
- `graft` when each use wants its own continuations, or when the reused
  behaviour has several distinct exits rather than one return. It costs
  code size — one copy per site.
- `bind` when several call sites share one argument list and repeating it
  would be noise.

The stdlib ships both forms of each operation — a behaviour `graph` with
explicit exits and a one-line `routine` facade that grafts it with `done
= return` — so a consumer picks per use site (`docs/tmt/stdlib.md`). How
the three lower onto the machine, and what `tmt link --call-mech`
chooses between, is `docs/tmt/isa.md (call mechanisms)`.

## Symbol maps

A tape argument may carry a symbol map, letting a routine or graph
written against one alphabet run over a tape that uses another:

```
call flip(n = num with map { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' })
```

The map is written source-first: the left side names a symbol of the
**caller's** tape, the right side a symbol of the **callee's** alphabet.

### The two arrows

`->` declares a **two-way** correspondence: the callee reads the source
symbol as its image, and a write of that image lands back as the source
symbol.

`=>` declares a **one-way** read collapse: the callee reads the source
symbol as its image, and nothing is written back through that pair. This
is the legal spelling for many-to-one — several caller symbols may `=>`
the same callee symbol, where the same set of `->` pairs would be a
write-back collision.

```
'a' -> 'x'                 // two-way
'^' => '_', '$' => '_'     // both collapse onto the callee's blank
```

### The blank is pinned

Index 0 must read as index 0. Mapping the blank off itself (`'_' -> 'x'`)
is rejected, and so is a two-way pair whose *image* is the blank
(`'y' -> '_'`), because its write-back would un-pin the blank. A
read-only collapse onto the blank (`'y' => '_'`) is the legal form.

### Equal alphabets: identity completion

When the two tapes' alphabets have the same cardinality, unlisted symbols
**identity-complete** — a symbol the map does not name maps to its own
index. The completed map must then be injective, since a shared body
reading two distinct symbols as one could not write either back
unambiguously:

```
// ab {'_','a','b'} → ab2 {'_','a','b'}
with map { 'a' -> 'b' }
// error: identity completion collides on `b` — 'a' and 'b' would both read as 'b'
```

Omitting the map entirely means identity across the board, which requires
the two alphabets to be **glyph-for-glyph equal** — not merely the same
size. Two three-symbol alphabets with different glyphs still need an
explicit map.

### Unequal alphabets: closed maps and holes

When the cardinalities differ, there is no identity to complete — index
`k` on one tape has no reason to mean index `k` on the other. The map is
therefore **closed**: every non-blank source symbol the map does not name
becomes a **hole**. The blank stays pinned as always.

A hole is not a silent identity and not a compile error. It is a
diagnosable runtime event: reading a held-out symbol through the map
takes the `UnmappedRead` trap, and writing one that has no host image
takes `UnmappedWrite` (`docs/tmt/isa.md (explicit traps)`). Under a
graft the compiler synthesizes the trap rows at splice time; under a
frames call the projection raises them.

```
// wide {'_','^','$','0','1'} → bare {'_','0','1'}
with map { '0' -> '0', '1' -> '1' }
// '^' and '$' are holes: reading either traps UnmappedRead
```

Naming them explicitly is what makes the cross-representation call in the
stdlib work — `'^' => '_'` and `'$' => '_'` let the markers read as the
callee's blank, and they survive the call because the callee never writes
a blank.

An explicitly written identity pair is not a hole: `'0' -> '0'` keeps `0`
mapped even under the closed rule, which is why the example above lists
the digits rather than relying on their indices lining up.

## Range expansion and substitution

Ranges and bindings are source-level notation. The compiler expands each
rule into concrete rows before any code is generated, so nothing about
them survives into the machine.

### Pattern ranges

A ranged or bound cell expands to one row per symbol it matches. Across
several such cells the expansion is cartesian, with the leftmost tape
varying slowest. A range value with no glyph on that tape simply drops
that alternative rather than failing.

Expansion is a product, and a large one is a lint finding
(`docs/tmt/lint.md`) rather than an error.

### Substitution

A bound cell's symbol can be written back through `{name}`:

```
entry state copy {
  ['0'..'1' as c, *] -> write [-, {c}] move [>, >] goto copy;
  ['_', *]           -> stop;
}
```

A substitution may carry an integer offset, `{v+k}` or `{v-k}`. 0.1 folds
the offset per expanded row, against the numeric value the cell bound in
*that* row — the substitution is table-expansion sugar, resolved at
compile time, not a runtime computation. The fold is bounds-checked: a
row whose result names no symbol in that tape's alphabet is a compile
error at the substitution's own span.

```
alphabet bytes { 0..126 }

entry state inc {
  [1..125 as v] -> write [{v+1}] stop;   // 125 rows, each folded
  [126]         -> halt;
  [0]           -> write [1] stop;
}
```

Offsets fold numeric bindings. A binding that took a glyph carries no
numeric value, and 0.1 rejects arithmetic on one (`{c+1}`) at parse time.
`{c}` — the bare pass-through — applies to a glyph binding as it does to
a numeric one.

## Namespaces, visibility, and imports

`namespace NAME { … }` nests declarations and prefixes their names.
Namespaces nest arbitrarily, and a namespace may be reopened — each
`namespace` block is its own node, and declarations accumulate under the
same path.

```
namespace std {
  namespace binaryNumbers {
    export alphabet symbols { '_', '^', '$', '0', '1' }
    export routine plusOne(tape num: symbols) { … }
  }
}
```

Within one compilation unit every declaration is reachable by its
qualified name — `std::binaryNumbers::plusOne` — whether or not it is
exported. **`export` controls link-time visibility**: an exported world
becomes a symbol other objects may resolve against, and a non-exported
one is emitted as a local the linker will not hand out. Calling a
non-exported routine from another `.tmo` fails at link with an
unresolved symbol.

`use` imports a qualified name into the current scope so it can be
written bare, optionally under an alias:

```
use mylib::plusOne;
use outer::inner::touch as poke;
```

A single `use` may list several paths: `use a, mylib::b as c;`. An alias
rebinds only the local name; the declared symbol is unchanged. `use` also
declares a name defined in another compilation unit — that is how an
unbound cross-unit call names its callee. An import nothing references is
a lint finding.

## Doc lines and attention lines

A line whose first non-whitespace character is `?` or `!` lexes as one
token — a **doc line** or an **attention line** — consuming the rest of
the line as raw text. The rule is purely positional: it keys on the
line's first non-whitespace column, independent of where in the grammar
that line falls. This is the same rule the `.pmc` language uses
(`docs/pmt/language.md (doc lines and attention lines)`).

```
? Walk right to the current number's end marker. On entry the head is on
? the number; on exit it rests on that '$'. The tape is unchanged.
! [deprecated] use goToNumberFast instead
export routine goToNumber(tape num: symbols) { … }
```

### Run shape and attachment

A **run** is at most two contiguous blocks in a fixed order: an optional
`?` block, then an optional `!` block. A `?` line after the run has
entered its `!` block — interleaved, or the whole run written backwards —
is a compile error.

A run binds to the next declaration that accepts documentation. Those
are: `alphabet`, `routine`, `graph`, `machine`, `namespace`, `state`,
`graft`, and `bind`. Blank lines and ordinary comments between the run
and the declaration do not break the attachment.

`tape` declarations, `use` imports, and individual rules do **not** accept
documentation. A run before one of those, or before nothing at all, is a
dangling-doc-run error reported at the run's own first line; a `?` line
inside a state body, where a rule is expected, is a parse error.

```
machine {
  ? documents the state
  entry state s { [*] -> stop; }
}
```

Consecutive `?` lines join into one paragraph in order. One leading space
directly after the sigil is canonical and stripped, so `? foo` and `?foo`
store identical text. The text is plain prose — 0.1 interprets no markup
inside it.

### The `[deprecated]` attribute

An attention line may open with a bracketed identifier, `! [ident] rest
of the line`; without one the whole line is free prose. `deprecated` is
the only attribute 0.1 recognizes — any other bracketed identifier is a
compile error at the identifier's own span. Everything after the closing
`]` is the attribute's message, trimmed. At most one `[deprecated]` may
appear in a run; a second is an error at the second occurrence.

A deprecated entity's callers are a lint finding
(`docs/tmt/lint.md`), and the message surfaces on hover in the editor
(`docs/lsp.md`).

## Reserved keywords

Twenty-four words are fully reserved and may not be used as any name — a
tape, state, world, namespace, alias, binding, or graft-instance name:

```
alphabet  machine  tape    state   entry   routine  graph   namespace
export    use      graft   bind    as      map      with    write
move      goto     call    then    return  stop     halt    debugger
```

Reservation is enforced wherever a name is expected: `tape state: ab;` is
rejected, naming the offending word.

`deprecated` is **not** in this set — it is a contextual attribute word,
meaningful only directly after `[` at the start of an attention line's
text, and it remains available as an ordinary identifier.

## Grammar version history

- **0.1** — the language's first cut, and the baseline the version
  scheme measures from. Everything on this page is 0.1.
