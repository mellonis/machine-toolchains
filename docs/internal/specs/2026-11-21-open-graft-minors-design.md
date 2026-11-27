# Open-graft review minors — design (2026-11-21)

The four deferred findings of the `open-graft` round's final review,
settled before the 0.6.0 cut so the recorded debt does not ride into
release history. Three are text facts that disagree with the code; one
(the refusal's failure selection) is a real behavioral choice the code
never stated. Nothing here changes the cover law itself.

## 1. The counterexample texts state the wrong one (M1)

The joint-cover pin's comment and the same sentence in
`docs/tmt/language.md` (open grafts) both claim: rows `[0, *]` and
`[*, 1]`, "an opaque symbol on the first tape with the second on `'1'`
matches neither". Inverted: an opaque symbol on the first tape matches
row `[*, 1]` exactly when the second holds `'1'` (its `*` is on the
first tape). The true counterexample is the second tape on `'0'` — then
row 1 demands `'0'` where the symbol is opaque and row 2 demands `'1'`
on the second tape. This is verbatim what `ir.rs`'s own doc example
says (`rows_admit_opaque_index_over_rows`), which is the source of
truth; the two texts mistranscribed it.

**Fix:** align both texts with the `ir.rs` statement. The test's
assertion (`tape == "p"`) is correct and stays. Comment-only; no code.

## 2. An open map mints dead read holes (M2)

`build_tapemap` runs `close_unlisted(&mut rmap, host_card)` whenever
the cardinalities differ — including on an OPEN map, where `read_class`
classifies every unlisted source `Opaque` straight from `pairs` and
never consults `SymMap::holes` at all. Those holes are dead state that
contradicts `TapeMap`'s own model ("never a hole and never an alias")
and leak into the dedup key as content that means nothing. The write
half is unaffected: `wmap`'s close is load-bearing (it is what stops an
unnamed glyph landing through identity) and stays under
`open || host_card != graph_card`.

**Approaches.** (a) Skip the read-side close on an open map — chosen;
the model statement becomes literally true and the key drops
meaningless bytes. (b) Keep it and document why it is harmless —
rejected: it keeps derived state that lies about the type's invariant.

**Fix:** `if !open && host_card != graph_card` for the rmap close; the
comment block above it scopes "every unlisted source is a hole" to
closed maps. Pinned by an open-unequal `build_tapemap` unit test
asserting `rmap.holes` is empty and the unlisted symbol classifies
`Opaque` — the mutating reversal (drop `!open`) reddens it.

## 3. The refusal's failure selection is tape-major (M4)

The coverage loop iterates tapes outer, states inner, so the refusal
names the first (tape, state) pair in tape-major order — while both
the code comment ("the first state that would let the opaque index
decide is refused by name") and the docs prose ("a state whose rows …
is refused") read state-major. Pick one; the pair named is observable
in the error.

**Approaches.** (a) Reorder to states outer, tapes inner — chosen: the
prose already promises state-major, and the state is where the fix
lands ("give the state a `*` row"), so naming the first offending state
in source order is the predictable answer. (b) Keep tape-major and
document — rejected: it would rewrite two texts to promise the less
natural order. Existing pins are single-state and unaffected.

**Fix:** swap the loops; state in `docs/tmt/language.md` (open grafts)
the selection rule in one clause: the first state in source order with
an uncovered open tape, and that state's first such tape. Pinned by a
two-state discriminating test (state `one` fails only on the second
tape, state `two` only on the first: state-major names `one`/second
tape, tape-major would name `two`/first) — the loop reversal reddens it.

## 4. The `open-graft-uncovered` row sits out of order (M3)

`docs/tmt/cli.md`'s compile-error table follows `code_registry!`
declaration order; the new row was appended near `noreturn-violated`
while the registry holds it between `identity-glyph-mismatch` and
`map-not-closed`. The drift guard is a sorted set-compare and cannot
see placement.

**Approaches.** (a) Move the docs row to the registry position —
chosen, docs-only. (b) Move the enum variant and registry entry —
rejected: churn in the registry and its declaration-order witness test
to fix a docs cosmetic.

## Non-goals

- No change to the single-row-cover law, its predicate, or any
  acceptance/refusal boundary (the `[0, *]` / `[*, 1]` refusal is
  pinned and stays).
- No new codes, rules, or quickfixes; no CHANGELOG entry (the
  unreleased open-graft entry already describes this behavior; M4 only
  names a pair differently within it).
- The round's plan ledger (`docs/internal/plans/2026-11-21-open-graft.md`)
  is history; it is not amended.

## Constraints

- Diff stays `crates/turing-machine/` plus `docs/`: PM byte-identity
  and core-neutrality gates hold by construction (no `crates/core`,
  no `crates/post-machine` file is touched).
- House style: each fix's invariant is pinned by a named test carrying
  the mutation that reddens it, except comment/docs-only fixes, which
  are verified by reading against `ir.rs`'s statement and recorded as
  hold-still in the mutation checklist.
