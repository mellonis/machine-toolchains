# The `.pmc` language reference

The `.pmc` language version is **0.2** (pre-1.0: the version is `0.N` and `N`
bumps on any grammar change; at a declared 1.0 the axes activate — major =
breaking acceptance change, minor = additive syntax; no patch digit —
spec-text corrections are errata, implementation-conformance fixes live in
the crate changelog). See "Grammar version history" at the end of this
page for what each version changed.

`.pmc` is the C-like source language for the Post-machine toolchain. Control
flow is deliberately flat: labels, `goto`, `check`, and function calls only —
no loops, no general `if`, no expressions. A `.pmc` file compiles to a `.pmo`
object (`pmt compile`); see `docs/cli.md` for the compiler's command-line
flags and `docs/isa.md` for what the generated code actually runs on.

```c
// Move right until the first blank cell.
goToEnd() {
 1: right;
    check(1, 2); // cell marked → goto 1, blank → goto 2
 2: left;        // last command — implicit return
}

main() {
    @goToEnd(); // not defined here → external symbol for the linker
    right;
    check(3, 4);
 3: unmark(!); // unmark, then return — in main: stop the machine
 4: mark;      // last command — implicit stop
}
```

## Program structure

A file is a sequence of function definitions: `name() { statements }` — no
`void` (the language has no types), no parameters, no return values. `main`
is the program's entry point; a `.pmx` executable requires one at link time.

Identifiers are Unicode, JavaScript-flavored: the first character must be
alphabetic (Unicode `Alphabetic`) or `_`, every following character
alphanumeric or `_`. This is a conservative subset of JS `ID_Start`/
`ID_Continue`, and it is exactly the `.pma` symbol grammar (see
`docs/formats.md (assembly text)`), so every compiled name survives the trip
through generated assembly unchanged. Identifiers are case-sensitive.
Comments are `//` to end of line and `/* ... */` blocks; block comments do
not nest — the first `*/` closes the comment, so `/* /* nested */ */` leaves
a stray `*/` behind as a lex error rather than a doubly-nested comment. A
lone `/` that starts neither `//` nor `/*` is a lex error, and so is an
unterminated `/* ...` block that never finds its closing `*/`.

### Statements

The four tape builtins and `@`-calls take an optional **successor** in
parentheses: a numeric label (jump there afterwards) or `!` (return
afterwards); omitting it falls through to the next statement. `check` takes
two mandatory arms instead — one per outcome, each a label or `!` — not a
single optional successor. `goto` takes one mandatory numeric label, never
optional and never `!`. `halt` and `debugger` take no successor at all.
Returning from `main` stops the machine.

| Statement | Meaning |
|---|---|
| `left` `right` `mark` `unmark` | tape builtins; `left;` = fall through, `left(5);` = then goto 5, `left(!);` = then return — `left();` (empty parens) is a syntax error |
| `halt` | abnormal stop; no successor — execution ends |
| `debugger` | breakpoint — pauses under an attached debugger, no-op otherwise; no successor |
| `check(A1, A2);` | the only conditional: cell marked → `A1`, blank → `A2`; each arm is a label or `!` |
| `goto N;` | unconditional jump; `N` is a numeric label only — `goto !;` is a syntax error (put `(!)` on the preceding command instead) |
| `@name();` `@name(5);` `@name(!);` | call a user function (`@` sigil), with the same optional successor (`@name(!)` is a tail call) |
| `N:` | numeric label, local to the enclosing function |
| `cmd, cmd, …, cmd;` | comma group: commands run in sequence under one statement. Only the last item may carry a successor or be a `check` or `halt`; earlier items must be bare (builtins, `debugger`, or `@calls`) — `halt` mid-group is rejected for the same reason mid-group `check` is: the rest could never run. `goto` is excluded from a comma group entirely, not merely from non-last position — it may not appear in a group at all, first, middle, or last. A label applies to the whole group. |

There is no `return` keyword: mid-function return is the `(!)` successor, and
the last command of a body may omit it — falling off the end is an implicit
return (in `main`, an implicit stop).

```c
 1: right, right, mark(5); // group, then goto 5
 2: left, check(1, !);     // group ending in the conditional

// errors — non-last items must be bare:
// 3: left(1), left(2);  // successor mid-group
// 4: check(1, 2), left; // check mid-group
// 5: goto 1, left;      // goto never groups, not even first
// 6: left, goto 1;      // ...nor last
```

### Rules

- **Reserved words** (cannot name a function): `goto`, `check`, `left`,
  `right`, `mark`, `unmark`, `halt`, `debugger`. `export`, `use`,
  `namespace`, and `as` are CONTEXTUAL keywords, not reserved — see
  Visibility below. The guard applies to every `::` path segment, not just
  a lone head name: the same bar that stops a reserved word naming a
  function also stops it naming any segment of a qualified call or import
  path — `@std::goto();` and `use std::goto;` are both syntax errors,
  because such a symbol could never be defined from `.pmc` source in the
  first place.
- **Sigil adjacency:** `@` must be immediately followed by the callee
  name — no whitespace, digit, punctuation, comment, or end of input
  between them; `@ qq();` is a syntax error, because the sigil is part of
  the name's spelling. This is specific to `@`: spaced label colons
  (`1 : right;`) and spaced paths (`std :: goToEnd`) are unaffected and
  remain legal.
- **Empty builtin parens:** builtins may omit `()` entirely (`left;`); if
  parens ARE written on a builtin, they must contain a successor
  (`left(5)` / `left(!)`) — empty `()` is a syntax error (`left();`), not
  fall-through sugar. This bar is builtins-only: user calls keep
  mandatory, emptyable parens — `@name();` stays legal and still means
  fall-through. A bare identifier statement (with or without parens, no
  `@`) is an error unless it names a builtin; putting `@` on a builtin
  name is an error too.
- Labels are decimal numbers, unique per function, referenced only by `goto`
  and `check` in the same function. Declaration order is free. Labels may
  stack — `1: 2: left;` names one statement with both labels; either one
  reaches it from a `goto` or `check`.
- Falling off the end of a function body is an implicit return — the last
  command's `(!)` may always be omitted. A function body may also be empty
  (`f() { }`), which compiles to an immediate return (in `main`, an
  immediate stop).
- Calling an undefined function is not a compile error: it becomes an
  external symbol resolved by the linker (no `extern` boilerplate needed) —
  but the compiler warns unless the name is declared with `use` or called
  fully qualified (see Visibility).
- Duplicate function definitions in one file are a compile error; across
  objects, a link-time error (see `docs/stdlib.md`).

## Visibility, nesting, namespaces, imports

- **Hidden by default:** top-level functions are module-local unless
  prefixed `export`; the un-namespaced top-level `main` always exports
  regardless — writing `export main() { … }` is therefore a redundant
  no-op, not an error. Local functions are bound directly within their own
  object, invisible to cross-object resolution — they can neither shadow
  nor be shadowed by another object's symbols of the same name.
- **Nested definitions** (`outer() { inner() { … } … @inner(); }`): flat
  code, scoped callability — an inner function is callable from its
  parent's body and deeper only. It is always local, hoisted (visible
  anywhere in the enclosing body), and resolved innermost-scope-outward.
  Nested functions flatten to dot-mangled names (`outer.inner`) —
  unnameable from source, since `.pmc` identifiers cannot contain `.` or
  `:`.
- **Namespaces:** `namespace ns { … }` blocks are a naming/scope construct
  only — multiple per file, nestable, and OPEN: reopening the same
  namespace merges into it (scopes key by path), and any object may define
  `ns::*` symbols; there is no sealing in v1. Exports inside a namespace
  become `ns::path::name` symbols — namespaces join with `::`, nesting
  keeps `.` (symbols self-decompose at the last `::`; see below).
  Namespace names share the name pool with functions at the same scope.
  Only the un-namespaced top-level `main` is ever the program entry: a
  `main` defined inside a `namespace` block is an ordinary function like
  any other — it is not auto-exported (it needs its own `export` to be
  visible outside its module) and it is never the program's entry point.
- **Imports:** `use path [as alias][, path…];` declares an external symbol
  by its full name and binds ONE bare name (the alias if given, else the
  path's last segment) into the declaring scope. A path is
  `ident (:: ident)*` — `use std::goToEnd;` imports the symbol
  `std::goToEnd` and binds the bare name `goToEnd`. `use` is legal at file
  level and inside `namespace` blocks; the binding is scoped there and
  below (inner scopes shadow outer ones). Two imports binding the same
  bare name in the same scope are an error (keyed on the name AFTER
  aliasing) — UNLESS the two `use` lines are exactly identical (same path
  and alias), in which case the duplicate is tolerated: the first `use`
  wins the binding, so the second is never counted as used and surfaces as
  an unused-import warning instead (see Warnings below). A function
  definition always outranks an import binding of the same name.
- **Qualified calls:** `@ns::path::name()` is absolute — it skips the scope
  chain entirely, uses `::`-separated segments only (nested functions stay
  unnameable this way), and is self-declaring: it never triggers the
  undeclared-external warning, whether or not the symbol resolves inside
  this module.
- **Warnings** (carried on the compile report, never printed by library
  code — `pmt -v`/`pmt compile -Werror` render or escalate them; see
  `docs/cli.md`): a bare call to an undeclared external (once per name);
  an unused import; an unused function (unexported and unreached from
  `main` or any export — sound, because local symbols are invisible
  outside the module by construction). `-Werror` turns every warning on
  the report into a compile failure. Hygiene findings beyond these
  warnings — unused labels, shadowed imports, naming style — are the
  lint layer's job: `pmt lint` (docs/lint.md), a separate channel that
  never runs during compilation.

### Symbol grammar

Every compiled symbol name self-decomposes: the namespace part is
everything before the LAST `::`, and the function-nesting part (`.`-joined)
is everything after it. `std::api.helper` is namespace `std`, function
`api`, nested function `helper` — no side-table is needed to tell namespace
segments from nesting segments; the separators alone are enough. This
grammar is shared with `.pmo` symbol names and `.pma` `.func`/call operands
(`docs/formats.md (assembly text)`).

## Optimization

**Fall-through layout is a baseline, not a pass:** even at `-O0`, the code
generator never emits an unconditional jump to the instruction that is
already physically next — basic blocks are laid out in an order chosen so
the common case falls through instead. This is a layout invariant of
codegen itself, active regardless of optimization level (`docs/history.md`
has its lineage).

`pmt compile` accepts `-O0` (default, no optimization) or `-O1` (the full
pass pipeline: check-fold, jump-threading, cell-state, branch-fold,
tail-call, tail-merge, dce, plus the program-level inline pass — see
`docs/isa.md` for none of these; they are compiler internals with no ISA
surface). Individual passes can be turned off with `--fno-<pass>` (e.g.
`--fno-inline`), repeatable.

**The observable-equivalence guarantee:** whatever `-O1` does, a program's
observable behavior is unchanged from `-O0`: the final tape contents, the
termination kind (`stp` / `hlt` / which trap), and every branch decision
that depends on the match flag are identical between builds. Two things are
explicitly *not* observable and may differ: resource-limit outcomes (a
stack overflow at `-O0` may become a step-limit trap at `-O1`, because
tail-call optimization turns a growing call stack into an in-place loop —
this is a quality-of-implementation change, not a semantic one), and
intermediate step counts/states. The one exception: an un-stripped
`debugger` statement (`brk` in the ISA) is an observability barrier — no
motion or elimination happens across it, so a debugger attached at `-O1`
still sees honest state at every breakpoint.

**Interposition:** `-O1`'s inline pass binds intra-module calls at compile
time, so if you shadow one of a library's *internal* callees (see
`docs/stdlib.md`), the override only affects call sites that survive
optimization — the linker only guarantees interposition for call sites it
actually still sees as calls. A library that must stay fully interposable
should be built with `--fno-inline`.

## The IR artifact

The compiler's intermediate representation — a per-function control-flow
graph — is a versioned, documented JSON artifact, not an internal
implementation detail. `pmt compile --emit-ir[=STAGE]` writes it to
`<output>.ir.json`:

- `STAGE` is `lowered` (right after AST → CFG lowering, before any
  optimizer pass runs), `after:<pass-name>` (state right after a named
  pass last changed something), or `final` (the default — CFG after the
  whole pipeline, i.e. what codegen consumed).
- Stage labels can repeat across snapshots: a pass that fires in several optimizer rounds captures several `after:<pass>` snapshots. `--emit-ir=after:<pass>` selects the LAST captured snapshot with that label (last-wins). The flag itself appears at most once per command line — repeating it is an unknown-flag error.
- `pmt ir graph FILE.ir.json [--function NAME]` renders the IR as a
  Mermaid flowchart, one per function (or a single named one).

See `docs/formats.md (IR JSON)` for the JSON shape and version number.

## Grammar version history

- **0.1** — the v1 toolchain's grammar; the retroactive baseline the
  version scheme measures from.
- **0.2** — three tightenings, all under Statements/Rules above: sigil
  adjacency (`@` must be immediately followed by the callee name, with
  nothing — whitespace, digit, punctuation, comment, or end of input — in
  between); the reserved-word guard extended to every `::` path segment,
  not just a lone head name; and empty parens on a tape builtin
  (`left();`) becoming a syntax error — parens on a builtin, if present,
  must carry a successor. Call parens (`@f();`) are unaffected by the
  last one.
