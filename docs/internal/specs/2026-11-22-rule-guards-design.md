# Rule guards — design (2026-11-22)

Status: design settled in conversation with the author; awaiting the
implementation plan. Driver: the tracker's "rule guards" idea — a
compile-time predicate over a rule's bindings that filters the pattern's
cartesian expansion.

## Intent and why

A rule today expands its pattern cells into the full cartesian product of
the cells' row counts, and nothing can say "the two cells hold the same
symbol" or "x is the glyph before y". The chess engine's `pd_dir` state —
1,457 rules — is exactly that product written out by a generator, because
the language could not express the thin relation the rules are about.

A **rule guard** is a predicate written after the pattern that removes
rows from that expansion:

```
['a'..'z' as x, 'a'..'z' as y] where { x == y || x == y - 1 } -> …
```

676 rows of product become 51 — the diagonal and the subdiagonal — and
those 51 are ordinary rows. Codegen, the linker and the VM never see a
guard: the feature lives in the grammar and the expander. This is a
**deliberate drop** — the precise opposite of the silent drops the
strictness work removes: the rows a program has are derivable by hand
from what is written.

## Goals

- `where { … }` after a rule's pattern vector; the guard filters the
  cartesian expansion at expansion time, per candidate row.
- A small guard language over the rule's bindings: glyph identity,
  sequence order, `in <set>`, `&&`/`||`, parentheses, constant offsets.
- Typed comparison domains: **glyph identity** vs **sequence position**,
  with one written-out licensing rule for order comparisons.
- Narrowing by `in <set>`, scoped precisely (the `&&`-cone rule below).
- Typed refusals at compile time; no silent acceptance of a guard that
  cannot mean what it looks like.
- Policies carried: `tmt fmt` keeps the guard verbatim (it is written
  text, like a cell name); generated headers print guards (they are
  semantics, like a range); digests stay stable across spellings.

## Non-goals

- No guards in `.tma` vectors or `.rept` — those are the assembly
  dialects and stay positional.
- No run-time evaluation. A guard answers "which rows exist", not "what
  is allowed"; run-time permission is the contracts mechanism
  (`enters`/`leaves`, check states).
- No write-fold variables in a guard. A guard sees exactly the pattern's
  bindings and set/literal terms; write-fold expressions are a separate,
  numeric domain and never leak across.
- No general arithmetic in guards. Offsets are `± k` with `k` an integer
  constant. `x * 2` and friends are out.
- No cyclic arithmetic: `y - 1` for the sequence's first member is a
  position off the line, and no row matches it. No wrap-around.
- No new lint rules and no quickfix in this round. A guard dropping every
  row of a rule warns as the existing `empty-expansion`.
- No optimizer or IR notion of a guard; expansion output is plain rows.

## Anti-goals

- **A comparison license that is not written in the guard.** "Two
  alphabets that embed the same set" is why a set's order is
  well-defined across tapes — it is not a license to compare without
  saying so. The set that admits the comparison appears as an `in`
  conjunct, visible to the reader (approach B below was rejected on this
  ground).
- **A guard that hides rows opaquely.** The written product minus the
  stated predicate must be hand-derivable; the acceptance pins are
  derivation-first equivalences, not recorded outputs.

## Constraints (from the house, each earned by a past bug)

- One parse path; the guard is a green-tree node with trivia, so the
  formatter and the comment audit can see where a comment sits.
- **The clean-reprint lesson (named-cells round):** a printer that
  reprints a rule must carry the guard or the reader silently loses it —
  the `name:`-dropping bug was exactly this. `fmt` prints guards
  verbatim; the header printer prints them like it prints ranges.
- The header printer prints **semantics** already (`pattern_text` prints
  ranges, set refs, bindings), and `graph_digest` hashes those printed
  lines — the guard rides both, so a named/source spelling and its twin
  cannot drift apart (the "two truths in one printer" bug is not
  repeated).
- `PRODUCT_THRESHOLD` (256) measures the expansion work; a guard is
  evaluated per candidate row and does not make expansion cheaper, so
  the threshold counts the pre-filter product. The lint twin
  (`binding-product-threshold`) speaks of the same pre-filter product.
- Strictness priority is preserved: set resolution and cell checks run
  before guard typing reports anything (the named-cells round's ruling —
  older errors keep their priority).
- Drift guards to move with the feature: the error-code registry ↔
  `docs/tmt/cli.md` rows; the reserved-word count (31 → 32) in
  `docs/tmt/language.md` version history and the CLAUDE.md table; the
  TextMate grammars' keyword tier (`where` is `keyword.control.tmc`
  like its 31 siblings); no CLI surface change (no completions work).

## Approaches

### What the guard is

1. **Expansion filter (chosen).** The expander evaluates the guard per
   candidate row and keeps rows where it holds. Blast radius: parser,
   expander, printers. Everything downstream is untouched by
   construction, and the equivalence "guarded rule ≡ the written-out
   rows it keeps" is trivially checkable.
2. **Source-level rewrite** (expand guards into multiple concrete rules
   before compilation): rejected — it multiplies source-form artifacts
   (headers, digests, fmt) and makes the written program diverge from
   every generated one.
3. **Run-time predicate** (checked during execution, trap or filter at
   run time): rejected — guards decide which rows *exist*; deciding what
   is *allowed* is the contracts mechanism's job. Blurring the two would
   put a second run-time checker in the language.

### Order contexts (when `<` is legal)

A binding has an **order context**: the declared sequence its positions
live in — its tape's alphabet by default, narrowed to a set by an
`in S` conjunct.

- **A. Strict contexts (chosen).** An order comparison (`<`, `<=`, `>`,
  `>=`, `± k` participation) is legal iff the two operands share one
  context: both are bindings of the **same alphabet** (unnarrowed), or
  both are narrowed in their cones to the **same set**. A literal
  participates through its position in the other operand's context.
  Otherwise: a compile error naming both contexts. `==`/`!=` are glyph
  identity — always legal, cross-alphabet included.
- **B. Embedding-aware.** Additionally, two different alphabets that both
  embed one set declaration compare over that set's order without `in`
  conjuncts. Rejected: it makes the license invisible, contradicts the
  feature's design intent (a set exists in guards to *admit* comparison,
  and saying `in qq` is what makes the admission readable), and gets
  ambiguous when two alphabets share several sets.

### Narrowing propagation (`in S` and `||`)

**The `&&`-cone rule** (settled with the author, in his words: *"B does
not [see it] — we don't know why the conjunction is not true"*):

> An `in S` conjunct narrows every occurrence of its binding within its
> **cone** — the subtree of the maximal upward chain of `&&` nodes from
> the conjunct (the whole conjunction the conjunct sits in, including
> `||` operands nested under it). It narrows nothing outside that cone.

```
where { x in S && (A || B) }   // A and B see x ∈ S: survival forces the
                               // conjunct true
where { (x in S && A) || B }   // B does not: survival via B tells us
                               // nothing about `x in S`
where { (x in S && A) || x < y }   // ERROR outside the cone: `x < y`
                                   // compares on default contexts
```

Sound and compositional: a row survives the guard only when every
conjunct holds, so within one conjunct-committed cone the narrowing is
always real.

## Decided semantics

### Syntax

`where` sits between the pattern vector and `->`; `where` becomes the
32nd reserved word (version-history entry; today's 31 are unaffected,
and no shipped text uses `where` as a name).

```
rule   := pattern 'where' '{' guard '}'? … '->' transition
guard  := or
or     := and ('||' and)*
and    := test ('&&' test)*
test   := cmp | membership
cmp    := operand ( '==' | '!=' | '<' | '<=' | '>' | '>=' ) operand
member := name 'in' setname
operand:= term ( ('+' | '-') integer )?
term   := binding | glyph-literal | number-literal
```

- A **bare** `==`/`!=` between offset-free terms compares **glyph
  identity** (labels, never quotes) — legal between any two bindings,
  any tapes, and against a literal.
- Any **order** comparison, and any comparison with an **offset** on
  either side, is **position arithmetic** in one common sequence — the
  licensing rule above applies.
- A comparison with no binding on either side is refused (nothing varies
  per row; it would be a constant the author should write as rows).
- `in S` is both a restriction and a narrowing source. It is legal only
  if `S`'s members all lie in the binding's tape alphabet (else an error
  naming the set and the alphabet). The narrowed sequence is `S`'s
  declared member order, and set name resolution is the declarations
  machinery's (exportable, `use`-importable — a guard in a library graph
  may name an exported set).

### Typing table

| Form | Domain | License |
|---|---|---|
| `x == y`, `x != y`, `x == 'a'` | glyph identity | none needed (any tapes) |
| `x < y`, `x ≤ y`, `x > y`, `x ≥ y` | positions | one common context (same alphabet, or both narrowed to one set) |
| `x == y - 1`, `x + 1 == y`, `x == 'c'` with offsets | positions | one common context |
| `x < 'a'`, `'a' - 1 == y` | positions | `a` must lie in the other side's context sequence |
| `x in S` | restriction | `S ⊆ x`'s tape alphabet; narrows x's context to `S` in the cone |

### Evaluation

Per candidate row the bindings hold concrete labels; identity folds
directly, positions are indices in the context sequence, offset
arithmetic is integer (no wrap). The predicate decides the row's
survival. A guard that keeps no rows warns with the existing
`empty-expansion` kind.

### Printing

- **fmt** prints a guard verbatim — it is written text between `]` and
  `->`, and the never-move rule's audit grows slots for it (`]`-`where`,
  `where`-`{`, `}`-`->` boundaries, operator/binding slots inside).
- **Headers** (`tmt interface`, `.tmh` bodies) print guards in `rule_text`
  — a spliced graph body carries its guard into the host, whose expander
  then filters in its own world. `graph_digest` covers it (CRC over the
  printed lines), so twin spellings digest identically and `GraftDrift`
  stays quiet.
- **Disassembly** never sees a guard — rows only.

### What this does NOT disturb

Named sparse cells (guards sit after `]`, orthogonal); row-level lints
(`shadowed-rule`, `exact-row-conflict`, `unreachable-rule`) which read
expanded rows and so see the post-filter truth; `binding-product-threshold`
(pre-filter product); the everything-matrix and the `brk` barrier (no
optimizer involvement); `check_rules` numbering (post-fold indices).

## Acceptance shape (for the plan)

- Derivation-first equivalence: every guard fixture's rows equal the
  hand-written product its predicate implies (`x == y` → the diagonal),
  over machine/routine/graft bodies and through the `--extern`/`.tmh`
  path.
- fmt pins: guard survival verbatim, idempotence, comment-audit slots
  green in both flavours, an adversarial `.tmc`/`.fmt` pair.
- Refusal pins with spans: order-context error naming both contexts;
  `in S` with `S` off-alphabet; unknown binding name; a non-varying
  comparison; narrowing misuse inside `||` as above.
- Header twin: guarded named source and its positional twin print one
  header; a guarded library graph grafts identically with its digest.
- Docs: `docs/tmt/language.md` gains "Rule guards" (with the two-domain
  contrast against write-folds), `docs/tmt/fmt.md` slots, the error rows,
  version history (32 reserved words), CHANGELOG block (`.tmc` 0.3, no
  movement — the acceptance addition folds into the standing per-cycle
  bump).

## Left to the builder

- Error-code names and registry placement per house convention
  (candidates: `guard-order-context`, `guard-unknown-binding`,
  `guard-set-outside-alphabet`, `guard-constant-comparison` — final
  naming follows the registry's own ordering and the docs table).
- The expander's evaluation mechanics and where the per-row binding
  environment already exists (range expansion builds one); keeping the
  guard evaluator as a pure function of that environment.
- Parser/green-tree node shape for the guard sub-AST and the fmt trivia
  slot wiring; the comment-audit template count (87 → the round's new
  total) and where the slot list grows.
- Span precise: a refusal names the offending operand/comparison, not
  the whole guard (mirror of the named-cells rule that a refusal names
  the name).
