# Named, sparse vector cells in `.tmc` — design (2026-11-21)

Tracked by issue #131 (the chess-work writeup carries the motivating bug:
a generator passed CELL indices where TAPE indices were expected, and on a
16-tape machine the mistake is invisible because a cell is only ever "the
seventh comma"). This is the design for the round that implements it.

## Intent

A rule should say which tapes it touches, not at which positions they
sit. A named, sparse spelling of a rule's pattern / write / move vector —

```
[king: '♔', file: 0..7 as f] -> write [king: '-']
```

— where every tape not named is `*` (pattern), `-` (write) or `.` (move),
and names are the tape names the enclosing world already declares. This
kills the "wrong position" bug class, drops the wildcard noise on wide
worlds, and makes a rule robust to adding a tape to the world.

**Not a new semantics: pure spelling.** Named cells resolve at compile
time into the positional form; the emitted rows are byte-identical to
what the positional spelling produces. No runtime, IR, container or `.tma`
change.

## Goals

- The named-sparse spelling `name: CELL` in all three rule vectors
  (pattern, write, move), where `CELL` is anything a cell can already be —
  including a range with a binding (`file: 0..7 as f`) and a substitution
  fold.
- Every unnamed tape gets its default (`*` / `-` / `.`), so a rule that
  touches three of sixteen tapes writes three cells.
- Refusals, at compile time and before codegen: an unknown or wrong-kind
  name, a name repeated within one vector, and mixing named with
  positional cells in one vector.
- `tmt fmt` prints whichever form was written (a spelling choice is
  authorship; whitespace-only stays true).

## Non-goals

- `.tma` vector operands (and the `.rept` question): the assembly dialect
  is a separate surface (`AsmCaps::vectors`), and stays positional.
- A quickfix converting positional vectors to named form. Fixes attach to
  findings; there is no finding here, and a style lint to carry one would
  be wrong.
- Any lint rule changes: none are needed (see below), and the `unused-*`
  family keeps its exact meaning.

## The version rule (ruled by the partner)

The language version is an acceptance contract of the RELEASE, not of
each change: it bumps **once per `next` cycle relative to `master`, the
minimal bump**. `.tmc` on `next` is already at 0.3 (the open-graft map
marker took 0.2 → 0.3), so this round **moves no version** — its
acceptance change folds into the standing 0.3, and the cut's version
block describes both spellings' additions under 0.3. (The standing
`CLAUDE.md` "N bumps on ANY grammar change" sentence is refined to say
this; the refinement is committed with this round.)

## Design

### Storage and resolution

The PARSER stores each cell as written: a cell may carry a name
(`CellName` — the identifier and its span — before its ordinary
content), and a named vector's entry list is sparse. `Program` values
keep that source form — which is what makes fmt's "prints whichever
form was written" honest, and what makes the header's positional
printing an explicit step rather than an accident.

Resolution happens in the **resolution stage** (`resolve_world` /
`resolve_machine_world`), where the enclosing world's tape list and its
`state` parameters are already in hand: `ResolvedWorld.states` carries
the ordinary full-width positional rules — each named cell bound to its
tape's index, each unnamed position filled with its default (`*` / `-` /
`.`), names dropped. Everything downstream of resolution — lint rules,
`expand`, IR, codegen — sees the form it has always seen and changes not
at all. (Not in the parser: the world's tape set and the wrong-kind
answer — tape vs `state` parameter — are resolution's facts, not
grammar's.)

Misuse is refused at that same boundary:

| Defect | Code | Span |
|---|---|---|
| a name that is not a tape of the enclosing world (unknown, a `state` parameter, anything else) | `unresolved-tape-target` (existing; its definition already says "names a target that is not a tape in the enclosing world", and its message answers both cases) | the name |
| one vector naming the same tape twice | `duplicate-named-cell` (new) | the second name |
| named and positional cells in one vector | `mixed-cell-forms` (new) | the first offending cell |

`unresolved-tape-target`'s `docs/tmt/cli.md` row is widened to name both
carrier surfaces (a binding argument, a named cell). Both new codes join
`code_registry!` and the error-table drift guard in the usual way. The
checks are compile errors, not lint findings: a wrong name blocks the
compile, and the editor sees the same diagnostic through the structural
CLI≡editor parity.

### Printing

- `tmt fmt` prints the form the author wrote; named cells keep their
  names, sparse stays sparse. The formatter's never-move comment rule
  gains the new positions (name, colon, cell interior), audited like the
  other 81+.
- **Generated headers (`tmt interface`, `.tmh`) print the canonical
  POSITIONAL form, from both arms** (source and object). Approaches
  weighed: (a) positional always — both arms produce identical text by
  construction, one canonical form across every generated artifact
  (`.tmh`, `.tma` disassembly, objects), and "a name is a spelling, not a
  semantics" (the named-maps precedent) says generated artifacts speak
  the semantics; (b) named always (canonicalizing) — better to READ on a
  16-tape body, since the object does know tape names from the signature
  and could synthesize the named form; rejected because two generated
  surfaces of one unit would then speak different forms (`.tma` stays
  positional), the object arm would grow a name-synthesis step needing
  its own source/object parity pins, and header readability is already
  served by the signature's name list above the body. Chosen: (a).
  Mechanism: the header printer applies the same resolution helper to
  the body it prints (the source arm; the object arm already builds
  positional rules from rows), so both arms print one identical
  canonical form and `graph_digest`'s CRC stays arm-independent.
  Switching to (b) later is a printing policy change that breaks nothing.

### Lints

No lint rule changes and no new rules. Because resolution fills the
unnamed positions with the default cells before `Resolved` exists,
`unused-tape` / `unused-tape-parameter`'s untouched test, the binding
product, and every fix span (green-tree range queries) keep working
unchanged. One honest side effect: on a wide machine written named, the
untouched tapes are untouched in truth, so `unused-tape` speaks up more
— the same signal the feature makes visible.

## Constraints and pins (house gates)

- **Named ≡ positional byte-identity**: for every fixture, the named and
  positional spellings of one rule compile to the same rows, and run
  identically. This is the round's central pin.
- The comment audit: new grammatical positions join the never-move
  audit (name/colon slots, sparse gap semantics for comments written
  where an unnamed tape is skipped).
- Corpus silence and cleanliness: `tests/golden`, `docs/examples`,
  `std.tmc` lint/fmt/build exactly as before (they stay positional;
  nothing reformats them).
- Published-docs surfaces (`docs/tmt/language.md`, `docs/tmt/cli.md`,
  lint.md untouched) updated with the new spellings and codes; drift
  guards (error table, completions, fmt-clean headers) stay green.
- PM-1 and `crates/core` untouched → PM byte-identity and core-neutrality
  hold by construction.

## Left to the builder

Implementation shape (parser token/embed, the extraction reshape, error
message texts, fixture layout) is the plan's to specify; nothing in this
document is a placeholder for it. The three refusals' exact message
wording and the two new codes' docs rows follow in the round's docs task.
