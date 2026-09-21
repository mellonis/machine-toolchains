# The `.tmc` language reference

The `.tmc` language version is **0.2** (pre-1.0: the version is `0.N` and
`N` bumps on any grammar change; at a declared 1.0 the axes activate —
major = breaking acceptance change, minor = additive syntax; no patch
digit — spec-text corrections are errata, implementation-conformance
fixes live in the crate changelog). Every statement on this page
describes 0.2 unless it says otherwise. See "Grammar version history" at
the end for what each version added.

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
name a different entry; see `docs/tmt/cli.md`). **The name `main` is
reserved for the entry world in every unit**, whether or not that unit
has a `machine` block: a top-level `routine main` or `graph main` is the
`duplicate-name` error even in a library, because a library that
declared one could never be linked beside any program. The reservation
is top-level only — `ns::main`, inside a namespace, is an ordinary name.

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

### Glyph literals and numeric literals

Two literal forms name symbols. A **glyph literal** is single-quoted:
`'a'`, `'_'`, `'^'`. Its content is any non-empty UTF-8 string, so one
grapheme, an emoji, or a multi-scalar sequence are each a single glyph;
`\'` and `\\` are the only escapes, and any other backslash sequence, an
empty `''`, or a literal that reaches end-of-line unclosed is a lex
error. A **numeric literal** is a bare decimal: `0`, `126`. The two forms
share one label space — see "Alphabets". The generated assembly's own
glyph literal follows this identical rule, so every glyph a `.tmc`
alphabet can hold survives the trip through assembly text
(`docs/formats.md (glyph literals and glyph lists)`); only a `..` range
endpoint is narrower there, needing a single character or a bare number.

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

#### Routines

A `routine` is a callable subprogram. It compiles to one shared body that
every call site jumps into, and `return` hands control back to whichever
site called it. `return` is legal only inside a routine; writing it in a
`graph` or `machine` body is a compile error.

A routine may also declare **`state` parameters** — named exits its
caller wires to states of its own, the same shape a graph's exits have.
They are covered under "`state` parameters" below; leaving through one is
not returning, which is what the next paragraph's inference turns on.

Whether a routine can return at all is a fact of its body: any `return`
transition, any `then return` on a call it makes, a graph exit bound to
`return` at a graft site, or `return` handed to a callee as a `state`
argument (that callee's own exit then returns from THIS routine) all
count, counted conservatively over the WHOLE body — dead states included
— so the fact never depends on the optimization level. **Leaving through
a `state` parameter is not returning**: control goes to a caller-supplied
state, not back to the instruction after the call. A routine with no way
to return is `noreturn`, and its signature may say so explicitly:

```
routine loop(tape t: ab) noreturn {
  entry state s { [*] -> goto s; }
}
```

The clause is an optional ASSERTION: the compiler checks it against the
inferred fact and refuses a mismatch (`noreturn-violated`), but the
inferred fact — carried on every compiled object and printed by `tmt
interface` either way — never depends on whether the author wrote it. A
routine read from a header has no body to infer from, so there its
declared clause *is* the fact.

`then` becomes OPTIONAL at a `call`/`bind` site whose callee is KNOWN to
be `noreturn`. "Known" means this unit can see the fact: for a routine
defined here, the inference above, written clause or not; for one defined
elsewhere, the `noreturn` clause of a declaration this compile was given
(a sibling source, a header, or the embedded standard library). That
second case is why the clause is worth writing even when the compiler
would infer it: a declarations reading never walks a body, so a routine
in another unit that does not SAY `noreturn` is read as able to return,
and its callers there must keep writing `then`. Against
an unknown callee, or one that can return, `then` stays mandatory
(`then-required`), since the linker never checks a continuation either
way. A `then` written anyway against a known `noreturn` callee is the
`unreachable-continuation` lint finding (`docs/tmt/lint.md`).

A call written without `then` compiles to the call followed by an
explicit trap (`docs/tmt/isa.md (explicit traps)`). An honest program
never reaches it — the callee never returns. A declaration that *lied*
turns what would otherwise be a silent fall-through into whatever the
linker placed next into a controlled stop, and the linker reports the lie
where it becomes observable: the `tail-call-no-continuation` warning
(`docs/tmt/cli.md (link warnings)`), raised when the linked callee can
return after all.

#### Graphs

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
NAME`; the latter are exit parameters — on a graph they are what a graft
site wires ("`graft`"), on a routine what a call site wires ("`state`
parameters"). Parameter names must be unique within a signature.

### Volatile tapes

Either declaration position accepts a `volatile` modifier immediately
before `tape`:

```
machine {
  volatile tape sensor: bits;
  tape buffer: bits;
  …
}

routine poll(volatile tape sensor: bits) { … }
```

A volatile tape is a **device band**: every access to it is externally
observable, and the external world may change its cells between
accesses. The toolchain preserves the band's exact access sequence — no
access is ever dropped, reordered, or fused away — and each read is a
fresh observation, never a value assumed to persist from an earlier one
(`docs/tmt/optimizer.md (volatile barrier)`). The modifier itself is a
compile-time-only property: it lives on the intermediate representation
and is dropped at codegen, so the generated assembly carries no trace of
it at all — only the source, or an `--emit-ir` snapshot, says which
bands are volatile.

`volatile tape T: ALPHABET` on a `routine` signature parameter fixes how
that routine's own body is compiled — every access inside it to the
bound parameter gets the volatile guarantee. That holds for any call
that keeps its own frame: one carrying a symbol map, a permuted
binding, or one crossing a compilation-unit boundary. At `-O1`, though,
`inline`'s splice-eligible forms — a bindless call, an equal-arity
full-pass-through bound call, or an arity-reducing identity
projection, always in-unit with identity tape placement and no map
pairs — dissolve the call itself: the callee's rows are spliced onto
the *caller's* own band and inherit the caller's volatility instead,
exactly like a graft (`docs/tmt/optimizer.md (volatile barrier)`).

Two asymmetries follow from how the three reuse constructs work (see
"Reuse: `call`, `graft`, and `bind`" below), and both are worth knowing
before relying on the modifier. A `graft` splices its graph's rows onto
the host's own tapes before the optimizer ever runs, so a spliced row
always lives on whichever band the *host* declared — a graph's own
`volatile` parameter is accepted and inert, which is correct, since the
host's declaration is what the spliced code actually runs against. A
`call` or a `bind` that inline does not dissolve, by contrast, targets a
routine whose body was compiled once, independently of any call site —
in this compilation unit or another, it makes no difference — and
nothing revisits that compiled form afterward: binding a volatile
machine tape into a routine that was compiled without `volatile` on the
matching parameter is not diagnosed. The author is responsible for
calling the routine variant compiled for the right kind of band, and —
for an inline-eligible call — for remembering that an optimized build
may fold it into a graft-like splice regardless.

### Contract clauses

A signature tape parameter — `tape` or `volatile tape` alike, on a
`routine` or a `graph` — may declare what it is allowed to write, in
canonical form:

```
routine mark(tape t: bits writes { '0', '1' } preserves { '1' }) { … }
```

`writes { … }` and `preserves { … }` each take an alphabet-body element
list — single glyphs and ascending ranges, the same grammar an
`alphabet` body uses (see "Alphabets"), except that a clause's list may
be empty where a bare `alphabet` body may not. Both clauses are
optional, and the order between them is fixed when both appear: `writes`
first, `preserves` second. Both are parse errors: writing `preserves`
before `writes` is the `contract-clause-order` error; a second `writes`
or a second `preserves` on one parameter is `duplicate-contract-clause`.
The fixed order is a grammar rule rather than a style preference, and it
has to be: `tmt fmt`
is a token-preserving printer (`docs/tmt/fmt.md`) that never reorders
what an author wrote, so canonical output requires the grammar itself to
settle the question once, at parse time — there is no later pass that
could sort the two clauses back into place.

`writes {}` is a real, meaningful clause, distinct from no clause at
all: it declares that the parameter is written **nowhere**, a stronger
promise than a missing clause makes (a missing clause promises nothing).
`preserves {}` is legal too, though it has no effect a missing
`preserves` does not already have.

A tape parameter's **effective set** — what its world's body, and
everything it calls or grafts, is allowed to write there — is `writes`
(or, when `writes` is absent, the parameter's whole alphabet) MINUS
`preserves`. A glyph named by both clauses is not a contradiction:
`preserves` wins, and the `writes` entry naming it is simply inert — the
`contract-clause-overlap` lint reports exactly that (`docs/tmt/lint.md`).

The effective set is also what a callee **promises its callers**. When a
world calls or binds a routine outside its own compilation unit whose
resolved signature is visible — the standard library's routines are — the
caller's inferred footprint takes the callee's effective set, projected
through the binding, in place of the callee's body it cannot walk: a
library routine declaring `writes {}` adds nothing to its caller's
footprint, one declaring `writes { '0', '1' }` adds those two glyphs, and
one declaring no clause adds the whole alphabet, exactly as an opaque
callee does. That is why a caller reaching the standard library can carry
a contract of its own that is narrower than the alphabet. A callee whose
signature is not visible at all — a library object at the link boundary,
whose signature section carries tapes and cardinalities but no clauses —
still adds the whole alphabet.

Declaring either clause is checked in two independent steps, at two
different spans. First, while a clause resolves, each glyph it names
must be a symbol of the parameter's own alphabet — a glyph that is not
is `contract-symbol-unknown`, reported at that glyph's own span inside
the clause. Once every declared clause resolves cleanly, one further
check runs once per compile, after the whole module resolves: the
world's own INFERRED write footprint on that tape must be a subset of
the effective set, or the parameter that declared the contract is named
in a fatal `writes-outside-contract`, reported at the parameter itself
rather than at any one glyph. A tape with neither clause skips both
steps and carries no contract at all. The inference the second check
compares against is a deliberate over-approximation — a symbol it
excludes provably never lands on the tape, while one it includes merely
*may* (`docs/tmt/lint.md (dead-map-pair)` explains the same inference
from the lint side) — and the error's own wording says so honestly: it
reports what a world *may* write, phrased as a possibility, never as an
observed fact.

That over-approximation has one sharp edge worth knowing before reaching
for either clause on a routine whose body computes what it writes: ANY
write cell that is a substitution (`{…}` — a bare passthrough and an
arithmetic fold alike, see "Substitution") is answered by the check's
own walk as writing the tape's *whole* alphabet, regardless of what the
substitution can actually produce, because that walk works from source
form and does not evaluate a substitution's possible outputs, only
whether one appears at all. The passthrough gets no special case here,
even though it looks like it should merely echo whatever the pattern
already matched — the walk cannot tell "echoes the input" from "computes
something new" without evaluating the row, which it does not do. Once a
body writes through a substitution on a given tape, that tape's inferred
footprint is the full alphabet no matter how narrow the substitution's
real range is — so the effective set has to be the full alphabet too for
the check on THAT tape to pass, which rules out a `writes` clause naming
anything less than every symbol and rules out `preserves` naming
anything at all; a substitution write on one tape says nothing about a
plain clause on another tape of the same world. `preserves` is the
clause this bites hardest, since reaching for it is usually trying to
say "this glyph is never touched," and that is exactly the claim a
substitution write makes unprovable to the checker:
`writes-outside-contract` fires at the parameter on every such body,
honestly — the message still says a world *may* write the glyph, never
that it does — but not usefully. There is no narrower spelling that
escapes this today; the honest remedy is to drop the clause from a
parameter whose body writes through a substitution rather than declare a
promise the checker can never confirm.

A machine's own `tape` declaration carries no contract grammar at all —
`writes`/`preserves` are legal only on a signature tape parameter, which
is what distinguishes a machine's tapes from a routine's or a graph's
(see "Tapes and heads", above); there is nowhere else in the grammar a
clause can appear.

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
  stops. It may be omitted — see "Transitions".

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

A write cell is a literal symbol, a substitution `{…}` (a passthrough or
a fold expression — see "Range expansion and substitution"), or `-`
meaning **keep** the cell's current symbol. A move cell is `<`
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
| *(omitted)* | stay in the current state — a self-loop; legal only when the rule carries an action |

`stop` and `halt` are the machine's two terminations
(`docs/tmt/isa.md (execution)`). A `call`'s continuation `CONT` may be a
state name or any of `return`, `stop`, `halt`, under the same rules.

The transition may be **omitted** entirely, which means *stay in the
current state*: the head re-runs this same state's rules on the next step.
Omission is legal only when the rule carries at least one of `write`,
`move`, or a leading `debugger` — a rule that would do nothing at all
(`['a'] -> ;`) is a parse error, not a silent spin. A `call … then` never
omits its continuation; the `then` is mandatory. Inside a grafted graph an
omitted transition self-loops to that rule's own spliced instance, not to
the graph's source state.

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

The one arrangement that genuinely dies is a **second all-wildcard rule**:
once one catch-all matches every input, a later catch-all can never fire.
The compiler warns (`unreachable-rule`) and drops it. That narrowness is
the point — *only* a second catch-all qualifies. An exact or partial rule
written after a catch-all is not dead; it sorts into an earlier band and
stays reachable, which is exactly what the example above relies on. The
broader "an earlier rule already covers this one" reasoning is lint's
richer `dead-rule` analysis (`docs/tmt/lint.md`), run at lint time over
the same bands.

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
`NoTransition` trap (`docs/tmt/isa.md (execution)`). Falling off a state
is therefore a diagnosable runtime event, not undefined behaviour.

## Reuse: `call`, `graft`, and `bind`

Three constructs reuse a world elsewhere. They differ in what is shared
at run time and in when continuations are decided.

### `call`

`call` invokes a `routine`. One body is shared by every call site, and
`return` goes back to whichever site is on the stack — a dynamic return.

```
entry state s { [*] -> call plusOne(num = data) then done; }
```

`then` may be omitted when the callee is KNOWN (its declarations are
visible to this unit) to be `noreturn` ("Worlds", above) — the call is
then in tail position, and nothing after it ever runs:

```
entry state s { [*] -> call loop(t = data); }
```

The argument list binds the callee's tape parameters to the caller's
tapes by parameter name, optionally through a symbol map (see "Symbol
maps"). When the callee is defined in this compilation unit its
signature is known, and **every** parameter must be bound: a missing,
duplicate, or unrecognized argument name is a compile error naming the
parameter. An argument list is all-or-nothing in that sense — there is no
partial binding, no leaving one parameter for a later site to supply.
Either a call names no arguments at all (the transparent form, "Calls
across units" below) or it names them all.

Within one argument list, two tape parameters may not bind the same
caller tape — a binding places callee tapes **injectively**, one caller
tape backing at most one callee tape (`duplicate-tape-target`). The same
caller tape may back different callee tapes across different calls; only
aliasing it within one call is rejected. A direct consequence: a callee
can never declare more tapes than the caller has to bind them to
(`callee_arity ≤ caller_arity`) — a routine wider than its caller is
unrepresentable, not merely unwritten.

### Calls across units

A call whose target lives in *another* compilation unit comes in two
shapes, and they differ in what the caller has to know.

**The transparent call** names no arguments at all. The callee then runs
on the caller's own tapes, in the caller's own index space, with the
heads wherever the caller left them:

```
alphabet a { '_', '0', '1' }

machine {
  tape num: a;
  entry state s { [*] -> call std::binaryNumbersBare::plusOne() then done; }
  state done    { [*] -> stop; }
}
```

Because there is no binding, the correspondence is **by index**: the
caller's symbol at index *k* is whatever the callee's own alphabet spells
at index *k*. A transparent call is therefore correct only when the
caller's alphabet lists **the same glyphs in the same order** as the
callee's. The linker checks exactly that and says so when it does not
hold: a same-width alphabet spelling different glyphs is the
`glyph-mismatch` warning, naming the first position that differs; a
narrower callee alphabet is `narrow-alphabet`; a wider one is an error
(`docs/tmt/cli.md (link warnings)`). The two ways to stop re-declaring an
alphabet by position are to **import** the callee's own alphabet
("Alphabets, maps and graphs across units") or to **bind and map
explicitly** — the form below.

**The bound call** names arguments, exactly as an in-unit call does. The
compiler emits a SYMBOLIC binding — the callee's parameter name in place
of a caller-tape position, and a bound map's destination as a glyph in
place of an index — because the callee's own tape order and index space
belong to the LINKER to resolve, not to this unit (`docs/formats.md
(bound calls)`). What gets checked, and when, depends on whether this
compile was given the callee's declarations:

- **With declarations** (a sibling source, a `--extern` file, a library,
  or the embedded standard library), the argument list is checked here,
  exactly as a local signature's is: a missing, duplicate, or
  unrecognized argument name is a compile error naming the parameter, and
  the emitted entries follow the callee's own tape order.
- **Without them**, the call still compiles: every entry is written by
  name, in source order, and the same checks run at LINK time instead,
  against the callee's real object — where a parameter the callee does
  not declare, or one the site never named, is a link error.

```
use hidden;
…
[*] -> call hidden() then done;          // transparent — resolved at link
[*] -> call hidden(t = main) then done;  // a symbolic binding, checked
                                          // here when `hidden`'s
                                          // declarations are known,
                                          // otherwise at link
```

A bound call with an OMITTED map emits no pairs, which leaves the
linker's glyph check standing guard over that tape. Writing an empty map
instead — `with map { }` — is the way to say "bind by index, and I mean
it"; see "The written empty map".

### `state` parameters

A routine's signature may declare `state` parameters after its tapes.
Each one is an **exit**: a way out of the routine other than `return`,
wired by the call site to a state of the caller's own world.

```
alphabet ab { '_', '0', '1' }

routine pick(tape t: ab, state hit, state miss) {
  entry state s {
    ['_'] -> goto hit;
    [*]   -> goto miss;
  }
}

machine {
  tape d: ab;
  entry state go { [*] -> call pick(t = d, hit = won, miss = lost); }
  state won  { [*] -> stop; }
  state lost { [*] -> halt; }
}
```

The rules:

- Inside the routine, `goto <state parameter>` leaves through that exit.
  The exit's identity is its **position** among the signature's `state`
  parameters, and both ends read that one list — which is why a call site
  must know the callee's parameter order.
- At the site, each `state` argument is given **by name**, in any order,
  like a tape argument. Its value is a state of the calling world, or a
  terminator — `stop`, `halt`, or, inside a routine, `return`. A
  terminator argument means the callee's exit ends the program (or
  returns from the *caller*) directly.
- A routine's own `state` parameter may be **forwarded** to an inner
  call, which is how a facade delegates its exits without knowing what
  they lead to. It may equally be a `then` continuation — `then <state
  parameter>` leaves the enclosing routine through that exit once the
  callee returns.
- Every exit must be bound; the argument list is complete, as any other
  is.
- A routine that only ever leaves through its exits never returns, so it
  is `noreturn` ("Routines") and `then` is optional at its call sites.
- A signature may declare at most **255** `state` parameters; the 256th
  is `too-many-state-params`, since the published exit count is one byte
  wide.
- An exit-bearing call into a routine defined in ANOTHER unit needs that
  unit's declarations, because an exits vector is positional and only the
  callee's own parameter order says which exit is which. Without them the
  site is `state-args-need-declarations`, not a deferred link check.
- Under the copy-based call mechanisms a RECURSIVE exit-bearing call
  chain cannot be stamped out and the link refuses it; the same program
  links under `tmt link --call-mech=frames` (`docs/tmt/isa.md (call
  mechanisms)`).

Two consequences of how exits lower are worth knowing before leaning on
them. A site that passes a terminator, or that forwards an exit onward,
resolves through one shared resume state per resume point per world — one
extra step on that path, and under `-g` that shared state carries the
first site that asked for it, so a debugger may show a neighbouring call
site's line there. And an exit-bearing site is never tail-called, nor is
an exit-bearing callee inlined: the optimizer leaves both shapes alone
(`docs/tmt/optimizer.md`).

### `graft`

`graft` splices a `graph` into the host world at compile time. Each graft
site gets its own private copy of the graph's states, and the graph's
exit parameters are wired to host states at the site — a static
continuation, no return stack involved. Its tape argument list is
checked the same way `call`'s is (above): two of the graph's tape
parameters may not bind the same host tape.

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

Two graft sites of the same world that splice the same graph with the
same tape arguments and the same exit wiring are ONE splice, not two: the
identity of a graft is its target, its tape composite and its
continuation, and the second site aliases the first rather than
duplicating the graph's states. The `as` name plays no part in that
identity, which is why a second site that differs only in what it is
called still earns nothing but its own name — the
`duplicate-graft-instance` lint finding (`docs/tmt/lint.md`).

Grafts nest: a graph may graft another graph, and splicing recurses. A
graph that graft-depends on itself, directly or around a cycle of
definitions, is `graft-cycle` — across units as within one.

A graft whose graph body contains a `call` is rejected at the graft site
(`graft-call-unsupported`): the call's binding arguments name the
graph's own signature tapes and its `then` continuation is a graph-space
state, neither of which the splice rewrites into host space. Write such
behaviour as a routine instead, with `state` parameters if it needs
several exits. The check fires at the SITE, not at the definition: a
graph whose body carries a call compiles happily as long as nothing
grafts it.

A graph defined in another unit grafts exactly like a local one, since a
graft needs the graph's source rather than its signature — see "Grafting
a graph from another unit" for what that means for name resolution, and
for the digest the linker checks. A graft target the declarations do not
supply is `undefined-graph`, the same two-case split an unresolved
alphabet reference has.

One consequence of the digest deserves its own line, for anyone editing a
header by hand: a graph's digest covers the body **as the header printer
renders it**, so a reference spelled differently but equivalently — a
qualified path where the printer would print a bare name — digests
differently and trips the drift check at link. Doc lines, comments,
whitespace and unrelated declarations do not move it.

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

A bind is a call target, not a state — `goto h` is a compile error. Its
declared argument list is checked the same way an ordinary `call`'s is:
one caller tape may not back two of the callee's tape parameters.

### Choosing between them

- `call` when the body should be shared and the continuation should
  depend on the caller. It costs a return-stack frame and, across
  differing tape shapes, a frame projection.
- `graft` when each use wants its own continuations, or when the reused
  behaviour has several distinct exits rather than one return. It costs
  code size — one copy per site.
- `bind` when several call sites share one argument list and repeating it
  would be noise.

Most stdlib operations ship both forms — a behaviour `graph` with
explicit exits and a one-line `routine` facade that grafts it with `done
= return` — so where both exist a consumer picks per use site
(`docs/tmt/stdlib.md`); a few ship only the routine facade, composed over
another routine rather than backed by a graph of their own. How the three
lower onto the machine, and what `tmt link --call-mech` chooses between,
is `docs/tmt/isa.md (call mechanisms)`.

## Symbol maps

A tape argument may carry a symbol map, letting a routine or graph
written against one alphabet run over a tape that uses another:

```
call flip(n = num with map { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' })
```

The map is written source-first: the left side names a symbol of the
**caller's** tape, the right side a symbol of the **callee's** alphabet.
A map is per caller tape, which is why one caller tape can never back
two callee tapes at once (see "`call`", above): two maps layered onto
one physical head is not a representable projection.

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

For a **graft** both the blank pin and this injectivity requirement are
enforced immediately, at the site's splice during compilation. A
**call**'s or a **bind**'s binding carries the same two checks, but only
the linker enforces them, once it resolves the binding — `tmt compile`
accepts a source that violates either one, and `tmt link` is where the
violation surfaces.

Omitting the map on a **graft** means identity across the board, which
requires the two alphabets to be **glyph-for-glyph equal** — not merely
the same size; the compiler rejects an omitted map between two
three-symbol alphabets that use different glyphs.

A **call** or a **bind** with an omitted map skips this check: the
binding maps by **index** instead of by glyph. Two same-size,
differently-glyphed alphabets then bind silently — the caller's symbol at
index *k* reads to the callee as whatever glyph sits at index *k* there,
and a write back lands on the caller's glyph at that same index.

The split is deliberate, and it follows the level each construct lives at.
A `graft` is a **source-level splice**, performed at compile time where
glyphs are the author's mental model — so its identity means *glyph*
identity, and mismatched glyphs are the `identity-glyph-mismatch` error
above. A `call` (and a `bind`) is a **machine-level boundary** resolved at
link time, and the machine stores only indices — it never sees a glyph —
so identity there can only mean *index* identity. Binding two same-size,
differently-glyphed alphabets by index is therefore intended semantics,
not a gap. When that index re-labelling is a surprise rather than the
intent — the same glyphs listed in a different order, say — the opt-in
`index-identity-map` lint (`tmt lint --warn index-identity-map`;
`docs/tmt/lint.md`) is the audit tool that flags it within one unit, and
the linker's own `glyph-mismatch` warning is what catches it across the
link boundary, where the callee's alphabet is not visible to the
compiler at all.

### The written empty map

`with map { }` — a map written with no pairs — is not the same thing as
no map at all, even though both bind by index and both complete to the
identity on equal-cardinality alphabets. The difference is what it tells
the linker. An OMITTED map leaves the glyph checks standing: binding into
a callee whose alphabet spells its glyphs differently is
`glyph-mismatch`. A WRITTEN map — empty included — is the author saying
what they meant, so it is never graded, and the warning goes quiet:

```
// the caller's tape is `{ '_', '1', '0' }`, the callee's `{ '_', '0', '1' }`
call mark(t = d)                // the LINK warns: glyph-mismatch at position 1
call mark(t = d with map { })   // the link is silent: bind by index, deliberately
```

Both compile without a word; the difference shows at `tmt link`.

Use it when the index re-labelling is the intent — a tape whose glyph
names differ from the callee's by design, where the positions are what
carry the meaning. Everywhere else, prefer naming the pairs: an explicit
map says which glyph becomes which, and survives a later reordering of
either alphabet.

### Unequal alphabets: closed maps and holes

When the cardinalities differ, there is no identity to complete — index
`k` on one tape has no reason to mean index `k` on the other. The map is
therefore **closed**: every non-blank source symbol the map does not name
becomes a **hole**. The blank stays pinned as always.

A hole is not a silent identity and not a compile error. It is a
diagnosable runtime event: reading a held-out symbol through the map
takes the `UnmappedRead` trap, and writing one that has no host image
takes `UnmappedWrite` (`docs/tmt/isa.md (explicit traps)`).

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

### Named maps

A map used at several sites can be declared once and referenced by name,
alongside the inline form, which stays legal everywhere it always was:

```
map wideToBits: wide -> bits { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }

bind plusOne(num = data with map wideToBits) as inc;
[*] -> call invert(num = data with map wideToBits) then t;
```

`export map` alongside `export alphabet` makes a declaration importable
(`use lib::wideToBits;`), and it travels through a `.tmh` header the same
way an exported alphabet does. A named map **expands to its declared
pairs** before anything past name resolution sees it — the linker never
receives a name, and a site written `with map NAME` compiles to exactly
the binding the same site would carry written `with map { … }` inline: a
name is a spelling, not a semantics.

The declaration is checked **once**, at the `map` statement itself, over
its own two named alphabets (SOURCE, then DST): every pair's glyphs
resolve in their own alphabet (`map-symbol-not-in-alphabet`), the blank
stays pinned (`map-blank-pin`), no symbol gets two images in one
direction (`map-conflict`), and — on equal-cardinality alphabets — the
map is injective (`map-not-injective`), exactly the graft-time checks
above. On UNEQUAL cardinalities the declaration additionally requires
every non-blank source symbol to be named explicitly (`map-not-closed`):
unlike a graft's own inline map (one splice, one visible use, so an
unnamed source quietly becomes a hole), a named declaration is meant to
be reused at every site that names it, so a gap left implicit there would
be a silent runtime trap wherever it is next used. `wideToBits` above
satisfies this:
`wide`'s four non-blank symbols (`^`, `$`, `0`, `1`) are all named, even
though two of them collapse onto the same target glyph — closed, not
injective, which unequal cardinalities never require.

At each SITE only two further facts are checked: the caller tape's
alphabet must be the map's own declared SOURCE
(`named-map-source-mismatch`), and the callee parameter's alphabet must
be its own declared TARGET (`named-map-target-mismatch`) — a named map
resolved once at its declaration cannot silently drift onto a
differently-alphabeted pair of tapes at a use site. A `with map NAME`
naming no map in scope is `undefined-map`, with the same two cases an
unresolved alphabet reference has ("Alphabets, maps and graphs across
units").

`with map NAME` and the inline `with map { … }` are the two ways to write
a map at a site. The third spelling, `with map { }` with no pairs at all,
is not an empty map in the same sense — it is how an author says "bind by
index, deliberately"; see "The written empty map".

## Range expansion and substitution

Ranges and bindings are source-level notation. The compiler expands each
rule into concrete rows before any code is generated, so nothing about
them survives into the machine.

### Pattern ranges

A ranged or bound cell expands to one row per symbol it matches. Across
several such cells the expansion is cartesian, with the leftmost tape
varying slowest. A range value with no glyph on that tape simply drops
that alternative rather than failing.

When *every* alternative drops — an all-off-alphabet range, or a single
glyph the tape's alphabet lacks — the rule expands to no rows at all. That
is the `empty-expansion` compile warning, not an error: the rule
contributes nothing and vanishes, and a state left with zero rows is
still valid — it traps on entry.

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

A substitution's body is an arithmetic **fold expression** over the row's
bindings:

```text
expr := mul (('+' | '-') mul)*
mul  := atom (('*' | '%') atom)*
atom := var | integer | '(' expr ')'
```

`+` and `-` are left-associative; `*` and `%` bind tighter. Several
distinct bound names may appear in one expression (`{a+b}`). The fold is
evaluated per expanded row over `i64`, against the numeric values the
cells bound in *that* row — the substitution is table-expansion sugar
resolved at compile time, a constant baked into the emitted row, never a
runtime computation.

Whether a substitution is arithmetic is decided by its **tree shape**, not
by whether it carries punctuation. A body that is a single bare name is
the **passthrough**: it writes the bound symbol as read, and applies to a
glyph binding as readily as to a numeric one. `{c}` is the passthrough,
and so is `{(c)}` — redundant parentheses around a lone name do not make
it arithmetic. A body that **applies an operator** is a fold, and a fold
is numeric-only: an operator on a glyph binding is the `char-arithmetic`
error (`{c+1}` where `c` bound a glyph), because a glyph carries no
numeric value to fold.

Because `%` binds tighter than `+`, a modular increment needs explicit
parentheses. `{(v+1)%127}` folds as `(v+1) mod 127`; `{v+1%127}` would
fold as `v + (1 mod 127)`. Those parentheses are load-bearing — they are
what lets the top of the alphabet wrap back to the blank:

```
alphabet bytes { 0..126 }

entry state inc {
  [0..126 as v] -> write [{(v+1)%127}] stop;   // 127 rows; 126 wraps to the blank
}
```

`%` is truncating remainder, and the fold rejects any result a tape cannot
carry, each reported at the substitution's own span:

| Code | When the fold fails |
|---|---|
| `zero-modulus` | the modulus is zero (`% 0`) |
| `negative-remainder` | the remainder is below zero — reachable only through subtraction |
| `fold-overflow` | an intermediate result leaves the `i64` range |
| `fold-out-of-alphabet` | the folded value names no symbol on that tape |

The messages, verbatim — a positive-integer-literal modulus adds the
wrapping-idiom hint to `negative-remainder`, any other modulus gets the
bare form:

```
error: zero modulus in fold (`% 0`) [zero-modulus]
error: negative remainder in fold; for a wrapping decrement write {(v+2)%3} [negative-remainder]
error: fold arithmetic overflows i64 [fold-overflow]
error: `150` is not a symbol in this tape's alphabet [fold-out-of-alphabet]
```

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

### Qualified names

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
declares a name defined in another compilation unit — that is how a
transparent cross-unit call names its callee. An import nothing
references is a lint finding.

### Alphabets, maps and graphs across units

An alphabet, a named map and a graph are **source-level** declarations:
none of them is a linkable symbol, so naming one that lives in another
unit means having that unit's declarations at hand ("Declarations and
headers", below). Given them, all three are named exactly like a routine
— by a qualified path, or by a `use` that binds the short name:

```
use lib::bits;          // an alphabet, by import
use lib::wideToBits;    // a named map, the same way
…
tape d: bits;           // …and the qualified form
tape w: lib::wide;
[*, *] -> call lib::mark(t = w with map lib::wideToBits) then done;
graft lib::seek(t = d, found = done) as walk;
```

A graph named this way is spliced, body and all, exactly as a local one
is — see "Grafting a graph from another unit" for what that means for
the names inside it.

An exported alphabet imported this way is the honest alternative to
re-declaring the callee's glyphs by position for a transparent call
("Calls across units"): there is then one declaration, and the caller's
indices are the callee's by construction rather than by agreement. The
compiled object records which alphabets it imported and the glyph lists
it compiled against, and the linker compares each against the exporting
object's own declaration, so an alphabet that changed under a consumer
stops the link instead of silently re-labelling its symbols
(`docs/core.md (graft drift)`).

`unresolved-alphabet` therefore covers two different situations, and its
message says which one it found:

- **nothing declares that name anywhere** — a typo, or a declaration
  never written;
- **something does, in another unit, but this compile was not given that
  unit's declarations** — reached through a `use` or a qualified path,
  with no sibling source, header, library or standard library supplying
  it. The remedy is to declare the alphabet locally or to give the
  compile those declarations.

`undefined-graph` and `undefined-map` split the same two ways, for the
same reason.

## Declarations and headers

A compilation unit can be read for its **declarations** alone — what it
exports and what those exports promise — without compiling it. That
reading is what lets one unit check a call into another at compile time
instead of leaving every such check to the linker, and what lets a graft
reach a graph defined elsewhere.

### Declarations

A declarations reading yields: exported alphabets, exported named maps,
exported graphs *with their bodies*, and exported routine signatures —
each routine's tapes with their glyph lists and published write set, its
`state` parameter count, and whether it can return. Routine bodies and
the `machine` block contribute nothing and are not needed.

What a compile is given to read is a matter for the tools rather than the
language: `tmt compile --extern FILE`, the sibling sources and libraries
`tmt build` derives from a manifest, and the embedded standard library
which is read unless switched off (`docs/tmt/cli.md (--extern and
--nostdlib)`, `docs/tmt/project.md (Declaration derivation)`).

**Strictness is decided by the file's extension**, not by guesswork:

- a **`.tmh`** is read STRICTLY, as declarations and nothing else. A
  `machine` block is `machine-in-declarations`; a routine carrying a body
  is `routine-body-in-declarations`. A graph, by contrast, MUST carry its
  body — a graph's only form is its source.
- a **`.tmc`** used as a declarations source is read LENIENTLY: it is an
  ordinary program, and the bodies and the `machine` block it happens to
  carry are simply not used. A graph's body is kept, so a sibling's
  exported graph is graftable exactly as a header's is.

Both readings run the one `.tmc` grammar. There is no separate header
language, and no second front end to drift from the first.

### Headers

A **header** is a `.tmh` file: declarations in `.tmc` syntax, written out
by `tmt interface` (`docs/tmt/cli.md (interface)`). It is the form a
library ships beside its compiled object so that consumers can check
their calls, import its alphabets and graft its graphs.

```
namespace lib {
  export alphabet bits { '_', '0', '1' }
  ? Leave through `hit` on a blank, through `miss` otherwise.
  export routine pick(tape t: bits writes {}, state hit, state miss) noreturn;
  export routine mark(tape t: bits writes { '1' });
  ? Walk right to the first blank.
  export graph seek(tape t: bits writes {}, state found) {
    entry state s {
      ['_'] -> goto found;
      [*] -> move [>] goto s;
    }
  }
}
```

`pick` is `noreturn` because it only ever leaves through its exits, and
`mark`'s `writes { '1' }` is the clause its source declared; `pick`'s and
`seek`'s `writes {}` were inferred, neither having been declared.

A header is **generated, not hand-maintained**: it is regenerated from
the source whenever that source changes, and editing one by hand is how a
consumer ends up compiled against a promise the object does not keep.
What it carries follows from that:

- **A routine appears as a signature terminated by `;`** — no body. Its
  tapes carry their glyph lists and one contract clause, `writes { … }`,
  which is the tape's PUBLISHED write set: the declared effective set
  (`writes` minus `preserves`) when the source declared either clause,
  and the compiler's own inferred write set when it declared neither.
  **Every tape carries one**, `writes {}` included: a header states what
  a tape writes, and there is no spelling for "nothing declared". A
  `preserves` clause never appears; it has no independent meaning once
  the effective set is published, and could not be reconstructed from a
  compiled object anyway.
- **`volatile` never appears.** The modifier shapes how a routine's own
  body is compiled and is never checked at a call site, so it is not part
  of what a caller may rely on ("Volatile tapes").
- **`noreturn` appears when the routine cannot return**, and on a bodiless
  declaration that clause is the whole of the fact.
- **A graph appears in full**, body included, because a graft splices
  source.
- **`?` doc lines ride along.** `!` attention lines and ordinary comments
  do not, and neither does any declaration a printed one does not reach.
  One consequence is worth knowing: `[deprecated]` is written on an
  attention line, so a library's deprecation does not travel through its
  header and a consumer's call site is not flagged for it
  (`docs/tmt/lint.md`). A deprecation consumers must see belongs in the
  `?` prose too.
- **`use` lines are printed where the declarations need them** — a header
  is a self-contained unit, and a name it references either is declared
  in the header itself or is imported by a `use` line the header carries.
- **Private alphabets are still named.** A routine over an alphabet that
  is not itself exported is legal; the header prints that alphabet as a
  plain `alphabet` declaration (no `export`) so the signature can name
  it.

A header printed from a compiled OBJECT rather than from source is
narrower, because an object carries less: routine signatures and
alphabets, but no graph body, no named map and no doc line, since none of
those exist on the wire. Two further consequences of reading an object:
its `state` parameters have no names on the wire, so they print
positionally as `exit0`, `exit1`, …; and the entry world is skipped
entirely, since a `machine` is never a callee.

### What a header is trusted for

A header is a **declaration**; the object beside it is the truth. The
link stage re-checks what it can:

- A grafted graph's body is digested on both sides — the exporting unit
  records the digest of the body it published, the consuming unit the
  digest of the body it spliced — and a mismatch stops the link
  (`docs/core.md (graft drift)`). An imported alphabet is verified the
  same way.
- A routine's declared `noreturn` is re-read from the linked body, so a
  header that claims it falsely is reported where it matters — at a call
  site written without a continuation ("Routines").

Two things are NOT re-checked, and are worth knowing:

- **A header-only library** — declarations with no compiled object in the
  link — is trusted outright. There is nothing to compare against.
- **A write contract** that a rebuilt object no longer keeps is not
  caught. Regenerate a library's header whenever you rebuild its object.

### Grafting a graph from another unit

A graph declared by another unit is grafted exactly like a local one, and
the splice is identical. Names inside the spliced body resolve in the
**declaring** unit, never in the consumer's: a consumer alphabet that
happens to share a name with one the graph uses is a different alphabet,
and an omitted map between the two is `identity-glyph-mismatch` rather
than a silent identity. A named map given at such a site is checked
against the library graph's own parameter alphabets, for the same reason.

Two limits are current, not permanent. A library graph's body may reach
only its own unit's declarations and the standard library's — a graph
whose body names a THIRD unit's alphabet, map or graph cannot be printed
into a header or consumed from one. And a diagnostic about a declaration
read from a header is rendered against the primary input's path, carrying
the header's line and column; the position is the header's, the filename
is not.

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
store identical text. The text is plain prose — 0.2 interprets no markup
inside it.

### The `[deprecated]` attribute

An attention line may open with a bracketed identifier, `! [ident] rest
of the line`; without one the whole line is free prose. `deprecated` is
the only attribute 0.2 recognizes — any other bracketed identifier is a
compile error at the identifier's own span. Everything after the closing
`]` is the attribute's message, trimmed. At most one `[deprecated]` may
appear in a run; a second is an error at the second occurrence.

A deprecated entity's callers are a lint finding
(`docs/tmt/lint.md`), and the message surfaces on hover in the editor
(`docs/lsp.md`).

## Reserved keywords

Twenty-eight words are fully reserved and may not be used as any name — a
tape, state, world, namespace, alias, binding, or graft-instance name:

```
alphabet  machine  tape    state   entry   routine  graph   namespace
export    use      graft   bind    as      map      with    write
move      goto     call    then    return  stop     halt    debugger
volatile  writes   preserves       noreturn
```

Reservation is enforced wherever a name is expected: `tape state: ab;` is
rejected, naming the offending word.

`deprecated` is **not** in this set — it is a contextual attribute word,
meaningful only directly after `[` at the start of an attention line's
text, and it remains available as an ordinary identifier.

## Grammar version history

- **0.1** — the language's first cut, and the baseline the version
  scheme measures from.
- **0.2** — declarations and headers. A unit can be read for its
  declarations alone, and `.tmh` is the file that carries them
  ("Declarations and headers"). With them in hand: a `call`/`bind` may
  bind tapes into a routine defined in another unit ("Calls across
  units"), an alphabet, a named map or a graph may be named across a unit
  boundary ("Alphabets, maps and graphs across units"), and a graph from
  another unit may be grafted. New grammar: `state` parameters on a
  `routine` signature
  ("`state` parameters"), the `noreturn` clause and the optional `then`
  it licenses ("Routines"), and `map NAME: SRC -> DST { … }` declarations
  with `with map NAME` sites ("Named maps"). Two acceptance changes go
  the other way: `noreturn` joins the reserved words as the
  twenty-eighth, so a 0.1 program using it as a name no longer compiles;
  and `main` is reserved for the entry world in a library as well as in a
  program ("Program structure"). Everything else 0.1 accepted, 0.2
  accepts.
