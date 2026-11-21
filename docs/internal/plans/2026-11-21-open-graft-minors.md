# Open-graft review minors — implementation plan (2026-11-21)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** settle the four deferred findings of the `open-graft` round's
final review — two counterexample texts that state the wrong
counterexample, dead read-holes minted on open maps, the refusal's
tape-major failure selection, and one docs table row out of registry
order.

**Architecture:** one behavioral change (`build_tapemap` stops minting
read holes on open maps; the coverage refusal selects state-major), two
text corrections pinned against `ir.rs`'s own doc example as the source
of truth, and one docs-row move. All in `crates/turing-machine/` and
`docs/`; the cover law itself is untouched.

**Tech Stack:** Rust (workspace, `mtc-turing-machine`), cargo test.

**Spec:** `docs/internal/specs/2026-11-21-open-graft-minors-design.md`
(§1–§4 name the four items; the plan's tasks follow them in order 1, 2,
4, 3 of the spec's numbering — see each task's title).

## Global Constraints

- The diff touches only `crates/turing-machine/` and `docs/`; PM
  byte-identity and core-neutrality hold by construction.
- No new error codes, lint rules, or quickfixes; no CHANGELOG entry.
- Every behavioral fix's invariant is pinned by a named test that its
  recorded mutation reddens; text-only fixes are hold-still entries in
  the round's mutation checklist.
- Test comments and doc comments carry no `Task N` / `spec §N` notation.
- Commits: conventional with scope, e.g. `fix(turing-machine): …`.

## Review Focus

- An open map's WRITE half must keep closing whatever the cardinalities
  — the fix in Task 2 touches only the read map; the existing
  write-hole pins (`open_map_write_half_is_hard_closed`-style tests from
  the open-graft round) are the guard, and Task 2's step re-runs them.
- An open map on EQUAL cardinalities must be untouched by Task 2 (the
  read-side close never ran there; `check_injective_completion` stays
  skipped on open) — Task 2's pin runs both cardinalities.
- A state that is wholly covered must be passed over and the refusal
  land on a later state (Task 3 adds `a_wholly_covered_state_is_passed_over`).
- The M1 texts must stay mutually consistent with
  `ir.rs::rows_admit_opaque_index_over_rows`'s doc example; no test can
  pin prose, so the step records the exact sentence both texts must
  agree with.
- The dedup key must stay sensitive to openness and pairs after the
  derived holes leave it — the existing instance-key pins
  (`open_graft`-round `…_instance_key_…` tests) guard this; Task 2
  re-runs them.

---

### Task 1 (spec §1, M1): the counterexample texts agree with the law

**Files:**
- Modify: `crates/turing-machine/src/expand.rs` — the doc comment of
  test `two_open_tapes_need_joint_row_cover` (~line 3568)
- Modify: `docs/tmt/language.md` — the "Refused, not trapped." paragraph
  (~line 1434)

**Interfaces:**
- Consumes: `ir.rs`'s doc example for `rows_admit_opaque_index_over_rows`
  ("rows `[0, *]` and `[*, 1]` are NOT opaque on either tape — an opaque
  symbol on tape 0 with tape 1 on `'0'` matches neither").
- Produces: nothing; the test's assertion (`state == "mix" && tape == "p"`)
  is correct and unchanged.

- [ ] **Step 1: Fix the test comment**

In `two_open_tapes_need_joint_row_cover`, replace the clause "but one
opaque symbol at `p` matches NEITHER row when `q` sits on `'1'`" with
"but one opaque symbol at `p` matches NEITHER row when `q` sits on
`'0'`" — `row ['0', *]` then demands `'0'` where the symbol is opaque
and row `[*, '1']` demands `'1'`. Leave the rest of the comment and the
assertion untouched.

- [ ] **Step 2: Fix the docs sentence**

In `docs/tmt/language.md` (open grafts), replace "an opaque symbol on
the first tape with the second on `'1'` matches neither" with "an
opaque symbol on the first tape with the second on `'0'` matches
neither".

- [ ] **Step 3: Verify hold-still**

Run: `cargo test -p mtc-turing-machine two_open_tapes_need_joint_row_cover`
Expected: PASS (the assertion never moved; both texts now match
`ir.rs`'s statement — this is a hold-still entry, verified by reading).

- [ ] **Step 4: Commit**

```bash
git add crates/turing-machine/src/expand.rs docs/tmt/language.md
git commit -m "docs(turing-machine): the joint-cover counterexample names the run that actually fails"
```

### Task 2 (spec §2, M2): an open map mints no read holes

**Files:**
- Modify: `crates/turing-machine/src/expand.rs:1160-1175` — the read-side
  `close_unlisted` call and its comment block in `build_tapemap`
- Test: `crates/turing-machine/src/expand.rs` (tests module, beside
  `open_map_makes_unlisted_symbols_opaque_not_holes`)

**Interfaces:**
- Consumes: `build_tapemap(map: Option<&parser::SymMap>, phys: usize,
  host_glyphs: &[String], graph_glyphs: &[String], span: Span)
  -> Result<TapeMap, CompileError>` (private, same file), `SymMap`/
  `MapPair`/`SymLit::Glyph` from `crate::parser`.
- Produces: unchanged `TapeMap` shape; `rmap.holes` now empty on an open
  map by construction.

- [ ] **Step 1: Write the failing test**

`open_unequal_map_mints_no_read_holes`: build `parser::SymMap` with
`open: Some(dummy_span)` and one pair `'a' -> '0'` (`SymLit::Glyph`
values, dummy spans), call `build_tapemap` with
`host_glyphs = ["_", "a", "b"]` (card 3) and
`graph_glyphs = ["_", "0"]` (card 2) — the unequal case. Assert:

- `t.open`
- `t.rmap.holes.is_empty()` and `t.holes().is_empty()`
- `t.read_class(1) == ReadClass::Image(1)` (the named pair, after the
  identity-retain it stays: `open` skips the retain)
- `t.read_class(2) == ReadClass::Opaque` (unlisted ≠ hole)

Also run the same map on EQUAL cardinalities
(`graph_glyphs = ["_", "0", "x"]`): `rmap.holes` keeps being empty and
read classification unchanged (the read-side close never ran there).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-turing-machine open_unequal_map_mints_no_read_holes`
Expected: FAIL — `rmap.holes` equals `{2}` today.

- [ ] **Step 3: Skip the read-side close on open maps**

In `build_tapemap`, change the read-side guard to `if !open &&
host_card != graph_card`, and update the comment block: "every non-blank
source the map does not name is a hole" applies to a CLOSED map; on an
open map the unlisted class is `Opaque` by `read_class` and never a
hole, so a hole entry would be dead state in the dedup key. The write
half's `if open || host_card != graph_card` block stays exactly as is.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --lib` and
`cargo test -p mtc-turing-machine --test open_bindings`
Expected: all PASS — including the write-half hard-close pins and the
instance-key pins (Review Focus rows 1 and 5).

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/expand.rs
git commit -m "fix(turing-machine): an open map's unlisted symbols stay opaque, never holes"
```

### Task 3 (spec §3, M4): the refusal names the first state, then its first tape

**Files:**
- Modify: `crates/turing-machine/src/expand.rs` (~1775-1815) — the
  open-graft coverage loop in `expand_grafts_into`, and its comment
- Modify: `docs/tmt/language.md` — "Refused, not trapped." paragraph
  (append the selection clause)
- Test: `crates/turing-machine/src/expand.rs` (tests module, beside
  `two_open_tapes_need_joint_row_cover`)

**Interfaces:**
- Consumes: `rows_admit_opaque_index_over_rows` (unchanged),
  `CompileErrorKind::OpenGraftUncovered { state, tape }` (unchanged).
- Produces: same error shape; only the pair CHOSEN when several qualify
  changes: the first state in source order with an uncovered open tape,
  and that state's first such tape in parameter order.

- [ ] **Step 1: Write the failing tests**

`a_refusal_names_the_first_state_then_its_first_uncovered_tape`: a graph
over two tapes `p`, `q` with two reading states — `one { [*, '1'] -> done; }`
(fails cover only on `q`) and `two { ['0', *] -> done; }` (fails only on
`p`) — grafted onto two open maps (reuse the joint-cover test's host
alphabets and maps). Assert `OpenGraftUncovered { state: "one", tape:
"q" }` — today's tape-major loop names `two`/`p`, so this discriminates.

`a_wholly_covered_state_is_passed_over`: same shape but state `one` is
`{ [*, *] -> done; }` (covered on both) and `two` fails on `p` only;
assert `state == "two" && tape == "p"`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-turing-machine a_refusal_names_the_first_state`
Expected: the first FAILs (named `two`/`p` today), the second passes
already (both orders agree there) — it is the Review Focus regression
guard.

- [ ] **Step 3: Reorder the loop and state the selection rule**

Swap the coverage loop to states outer (in `gx.states` order), open
tapes inner (tape-index order), keeping the two skips (non-open tape;
empty opaque class) and the preimage row filter exactly as they are.
Adjust the comment to end "…is refused by name — the first state in
source order that has an uncovered open tape, and within it the first
such tape." In `docs/tmt/language.md`'s "Refused, not trapped."
paragraph, append: "Where several bindings would decide, the refusal
names the first state in source order that leaves an open tape
uncovered, and that state's first uncovered tape."

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --lib expand::` and
`cargo test -p mtc-turing-machine --lib coverage`
Expected: both new tests PASS; `two_open_tapes_need_joint_row_cover`
still names `mix`/`p` (single state, both orders agree).

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/expand.rs docs/tmt/language.md
git commit -m "fix(turing-machine): the coverage refusal names the first state, then its tape"
```

### Task 4 (spec §4, M3): the `open-graft-uncovered` row back in registry order

**Files:**
- Modify: `docs/tmt/cli.md` — the `### Compile errors` table

**Interfaces:**
- Consumes: `code_registry!` order (`identity-glyph-mismatch` →
  `open-graft-uncovered` → `map-not-closed`, `compiler.rs:680-682`).
- Produces: nothing.

- [ ] **Step 1: Move the row and extend its last sentence**

Cut the `| \`open-graft-uncovered\` | …` row from beside
`noreturn-violated` and paste it between `identity-glyph-mismatch` and
`map-not-closed`. In its last sentence, after "The state and the tape
parameter are named.", add " (the first state in source order that
leaves an open tape uncovered, and that state's first uncovered tape)."

- [ ] **Step 2: Verify the drift guard**

Run: `cargo test -p mtc-turing-machine --test error_code_docs`
Expected: PASS (the guard is a sorted set-compare; placement is
presentation and is checked by reading against `compiler.rs:680-682` —
hold-still entry in the mutation checklist).

- [ ] **Step 3: Commit**

```bash
git add docs/tmt/cli.md
git commit -m "docs(turing-machine): the open-graft coverage row sits in registry order"
```

### Task 5: the round's gates

- [ ] **Step 1: Full suite and quality gates**

Run: `cargo test --workspace`, then `cargo clippy --workspace
--all-targets -- -D warnings`, then `cargo fmt --check`
Expected: ALL GREEN (the merge-time run is the recorded one).

- [ ] **Step 2: Diff audit**

Run: `git diff --stat next...HEAD` (or the branch base)
Expected: only `crates/turing-machine/` and `docs/` files — the PM
byte-identity and core-neutrality standing gates are then untouched by
construction; record the file list in the run's report.

## Mutation checklist (recorded at execution)

| Mutation | Repaired by | Verification |
|---|---|---|
| Touch a counterexample text | — | hold-still (text-only); asserted sentence checked against `ir.rs`'s doc example |
| Revert `!open &&` in the read-side close (Task 2) | `open_unequal_map_mints_no_read_holes` | red — `rmap.holes == {2}` |
| Drop `t.open`/`read_class` assertions (Task 2) | same test's remaining holes assertions | red on the revert |
| Revert to tape-major loop (Task 3) | `a_refusal_names_the_first_state_then_its_first_uncovered_tape` | red — named `two`/`p` |
| Move the docs row back (Task 4) | — | hold-still (docs layout); read against `code_registry!` |

## Review outcome (recorded at execution)

Fresh whole-branch review: **Ready to merge** — 0 Critical, 0 Important,
3 Minor. One Minor was re-graded Important by effect and fixed in the
review fix pass (three texts said a graft's inline map "silently holes an
unnamed source" without the closed-map scope — the very law this round
establishes). The deferred set below was discharged the same day on the
partner's request, so nothing remains outstanding:

- the `docs/tmt/cli.md` selection parenthetical now names the skip (the
  refusal names the first uncovered tape whose map is open and leaves an
  opaque symbol — a closed map, or a map naming every symbol, is passed
  over);
- the `open_single_pair_src_map` (ex-`open_unequal_src_map`) doc no longer
  claims its caller's alphabets, and the vacuous `t.holes()` assert carries
  the note that only `rmap.holes.is_empty()` guards the recorded mutation;
- the reviewer's optional gap is closed by
  `open_unequal_map_still_closes_its_write_half` — an open map on unequal
  alphabets hard-closes its write half; the pin was mutation-verified
  (copying the read-side `!open &&` guard onto the write close redden it).
