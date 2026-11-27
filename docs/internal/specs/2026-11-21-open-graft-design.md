# Open Graft Design — `*` in a graft's map

**Date:** 2026-11-21 · **Issue:** mellonis/machine-toolchains#137 · **Branch:** `next`
**Supersedes:** nothing; satisfies "Phase 4 of the binding arc… in the 'graphs and standard library' spec" — that named spec does not exist, and this design carries its open-graft half.

## Intent and why

A `call`'s or `bind`'s map may end with `*` (an **open map**): every caller
symbol the pairs do not name reads, in the callee, as the callee tape's opaque
index, which only the callee's `*` rows match. That is what lets a routine
written over a small alphabet run on a wider tape. A `graft`'s map refuses the
same marker — `open-graft-unsupported` (`crates/turing-machine/src/expand.rs`,
the `build_tapemap` marker check) — so a GRAPH over a small alphabet cannot be
grafted onto a wider tape without collapsing the extra symbols onto one of its
own glyphs, which changes what the body sees. The standard library does exactly
that today where it folds `'^'` and `'$'` onto the blank.

Lift the refusal, for the symmetry of the three binding constructs.

## Goals

- `*` is legal in a graft's tape map, with the same spelling and the same
  per-site rules as at a call/bind site (`open_bindings.rs` marker tests).
- Unlisted host symbols stay what the graph never had a glyph for: only the
  graph's `*` rows match them. They are never aliased onto a glyph and never
  holed into a trap row — the two wrong outcomes the issue records.
- The openness of a map participates in the splice's dedup key and survives a
  graft inside a graft.
- A graft whose graph body would decide a row on an opaque symbol is refused at
  compile time (see Decisions), naming the state.
- `enters-unmet` and friends say something true at an open graft site.

## Non-goals

- No change to `call`/`bind` open maps, binding records, or the linker's opaque
  index machinery (`crates/core` stays untouched by this round).
- No shared-grafts / call lowering of a graft site (#118 is parked). When it
  lands, an open graft lowered as a call IS the open binding that already
  exists; that future shape is a note, not this work.
- No equality semantics for repeated pattern names (the other half of #127).
- Writes stay closed, *harder* under openness: the graph writes back only
  what the pairs map — `close_unlisted` runs on the write map unconditionally
  under an open map (equal cards included), the mirror of the call-side rule
  that an open binding closes its write half (`* as v` stays
  `wildcard-binding`). The wmap side of `build_tapemap` changes exactly that
  one gate and nothing else.
- Formats (MO/MX/MT) do not move: a spliced graph leaves plain rows
  downstream; graft provenance digests cover the BODY, not the site's map.

## Anti-goals

- A silent runtime trap standing in for a coverage check — no "uncovered state
  traps later" default.
- A temporary alias of an unlisted symbol onto a same-numbered glyph under any
  excuse.
- Touching `crates/core` from a TM-only round (standing gate: core neutrality).

## Constraints (from the project)

- PM-1 byte-identity: PM output must not move a byte. This round is
  `crates/turing-machine` (+ docs) only; core untouched.
- The 127-symbol ceiling and `AlphabetTooWide` behavior stay as they are.
- Comment never-move + fmt idempotence: the syntax (`*` in a map) already
  parses and prints at call/bind sites; graft maps are the same map syntax, so
  `tmt fmt` is expected to already hold. Verify, do not re-engineer.
- Published docs stay forge-agnostic and ref-free (issue number lives here,
  internal).

## Established facts this design builds on

- `map_rule` already maps a graph `Cell::Wild` to a host `Cell::Wild`. The
  graph's `*` rows would already catch unlisted host symbols if they were not
  shadowed by `trap_read_rows`, which prepends `hole → trap #0` rows built from
  `SymMap::holes()`, those holes being minted by `close_unlisted`.
- `close_unlisted(map, domain_card)` holes every unlisted non-blank symbol of
  the domain; `build_tapemap` calls it on BOTH sides only when
  `host_card != graph_card` ("closed-on-unequal"; equal cards get identity
  completion / `check_injective_completion`).
- `SymMap::write_key()` feeds the splice dedup key and already encodes the
  holes; openness must be distinguishable there (`expand.rs`, TapeMap keying).
- Opacity is already inferred per routine tape for open call/bind bindings
  (`open_bindings.rs`: `a_tape_read_through_a_wildcard_row_is_opaque`,
  `one_reading_state_without_a_wildcard_closes_the_tape`,
  `a_routine_no_state_reads_is_vacuously_opaque`). The graft check reuses that
  predicate over the graph body at the splice, where the body is in hand.

## Approaches considered

1. **Opaque set in `SymMap` (chosen).** An open read map mints no holes;
   instead the unlisted symbols become the map's **opaque set**, excluded from
   `preimage()` (they are not there today anyway — they name no pair) and from
   `holes()`; `*` rows reach them because `Wild` maps to `Wild`. The set is
   materialized in the map so it can enter the dedup key and compose across a
   graft inside a graft. Estimated at the issue's 60–100 lines in `expand.rs`
   plus the coverage check.
   *Trade-offs:* keeps every change compile-time and TM-side; the dedup-key
   rule is the one subtle part.
2. **A synthetic opaque index in the graph's space** (mechanical symmetry with
   the callee's opaque index). *Rejected:* at splice time there is no callee
   alphabet — the issue's own ruling — and minting an extra in-range symbol
   collides with the 127 ceiling and nested splices.
3. **Keep refusing; add nothing.** *Rejected:* that is the status quo #137
   exists to end; the standard library's `'^'`/`'$'` fold is the motivating
   wrongness.

## Decisions (the two the issue left open — these are the designer's calls)

- **A state that would decide a row on an opaque symbol is a compile error
  (`open-graft-uncovered`), not a runtime trap.** Mirrors the routine-side rule
  (a routine takes an open binding only when it is opaque); the splice has the
  body in hand and can name the offending state. Rationale: the whole point of
  the feature is "the extra symbols keep their meaning as *other*", and a trap
  dictating row selection is exactly that meaning being lost at run time. The
  check is the SAME opacity predicate the routine binding already infers — one
  predicate, two consumers. Per-state granularity: the error names the first
  reading state whose rows lack the covering `*` row.
- **`.tmc` language version moves 0.2 → 0.3.** The grammar does not move (the
  marker parses today), but the acceptance contract does: a text that errored
  at 0.2 compiles at 0.3. Pre-1.0 `0.N` is exactly the axis that carries this.
  Nothing else in the version block moves.

## What is left to the builder (plan-level)

- Where exactly the opaque set lives in `SymMap`/`TapeMap` vs. a flag + derived
  recompute — the materialized set is the intent; the key law is: two splices
  differing in openness or in opaque membership cannot share a dedup key.
- Compositional law to pin by test: a graft inside a graft — an opaque symbol
  at any layer reads as opaque at every deeper instance and matches only `*`
  cells all the way down.
- Whether `empty-expansion`'s wording needs an open-map clause (a rule whose
  every cell preimage dies is still the same warning).
- `enters-unmet` message shape at an open graft site (call form reports "a
  symbol this open map sends to the callee's opaque index"; the graft form
  should say the graft's own thing).
- `tmt interface`/header printing of a graph body containing an open graft —
  expected free (the printer ends in `tmt fmt` over printed text); verify by a
  pin, not by assumption.
- Treatment of the blank (index 0): identity-pinned as today; `*` covers it as
  it does at call sites.

## Gates for the round

- The three standing gates this round can touch: `-O0` bit-identity and the
  `brk` barrier (compiler-side expansion changes rows — rerun
  `opt_equivalence`), the everything-matrix (unchanged mechanisms, but the
  spliced rows flow through all three), and the compiled-stdlib byte-identity
  gate as a NEGATIVE control only if any formatter text changes (expected:
  none).
- New pins: closed-graft behavior byte-identical before/after whenever the map
  has no `*` (the splice for closed maps MUST NOT move); the nested law above;
  the coverage refusal naming the state; dedup-key discrimination.
