# Dead-write elimination via spatial liveness — design

Date: 2026-08-17. Status: approved design.
Driving issue: [#88](https://github.com/mellonis/machine-toolchains/issues/88).
Sequencing: the arc branches from master after
[#89](https://github.com/mellonis/machine-toolchains/issues/89)'s fix
merges (adjacent `move_elim.rs` surface).

## 1. Problem and honest expectations

Dead writes on these machines are composition artifacts, not authoring
waste: `inline` splices a callee whose trailing cleanup/restore writes
become redundant against the caller's continuation; TM graft stamping
creates the same shape between a stamped instance and its continuation.
`cell_state` already catches the straightline same-cell cases; this arc
adds what only liveness can prove — a write at a cell the head later
returns to and overwrites with no observation in between, including
through calls.

Expected wins are modest (smaller than the dispatch-threading round's
5%); part of the value is completeness — the one classic pass the
pipeline lacks. A **measurement task** rides the arc: corpus-wide fired-
deletion counts recorded in the optimizer docs like the sweep tables. A
near-zero count is a legitimate finding, not a failure.

## 2. Scope (ruled)

- **Both toolchains in one arc, phased** (PM substrate → PM pass → TM
  substrate → TM pass), one spec, one plan.
- **Full interprocedural** via per-function/per-world summaries — with
  the precision boundary stated as a theorem, not an apology (§3).
- **Correction recorded:** the #53 footprint engine is a *value*
  footprint (which symbol indices may land on a tape — `SymSet`), not a
  spatial one. TM's spatial dimension is built beside it
  (`optimizer/spatial.rs`), not extracted from it.
- Non-goals: fused-write halves (PM `WrLft`/`WrRgt` — fusion runs after
  this pass anyway), TM `Stop` scope-sensitivity, trap-row observation
  refinement, offset windows beyond ±32 — all pre-declared follow-ups.

## 3. The summary and liveness model

Per-function (PM) / per-world (TM) summaries, computed at `optimize()`
round start (fresh each round — `inline` reshapes bodies), by fixpoint
over the call graph, bottom-up over SCCs; recursive SCCs take the
conservative top. Dimensions:

1. **Net head displacement** per tape: `Known(k)` | `Unknown`. Any
   head-moving cycle (every walk-until loop) is `Unknown`.
   **The precision theorem:** dead-through-call requires the callee's
   displacement to be `Known` — with it `Unknown`, the caller's post-
   call offset frame is lost and the call is a full observation barrier
   *by the model*. Complex callees on tape machines have data-dependent
   movement, so summaries see through only statically-bounded callees;
   `inline` dissolves most of those boundaries anyway. Stated in the
   docs so nobody mistakes the barrier for implementation laziness.
2. **Observed offsets** (may-side, entry-relative, per tape): where the
   callee's observations land. Bounded bitmask window ±32; overflow →
   unknown-extent (top).
3. **Write offsets, split may/must**: `may_write` (over-approx) exists
   for completeness/diagnostics; **`must_write`** (under-approx —
   offsets written on *every* path; empty under any `Unknown`) is what
   the caller's liveness may use as kills. The asymmetry is load-
   bearing: uses over-approximate, kills under-approximate.
4. **`may_halt`**: the callee can end the run (`halt`; conservatively
   any `stop` TM-side, any trap-carrying state). Run-end observes the
   whole tape, so a `may_halt` callee is `observes: Top` at every call
   site.
5. **MF-at-exit** (PM only): provenance of the last latch
   (`FromCell(offset)` | `FromWrite` | `Unknown`) — the caller's first
   post-call `check` consumes it.

**The liveness ground truth:** the final tape is the program's output,
so *everything is live at every exit* (`Return` — the caller observes;
`Halt`/`Stop`/trap — the run's output). A write is dead only when every
path from it reaches an overwrite of the same offset before any use,
call-barrier, `brk` (the debugger observes the whole tape), or exit.

## 4. PM substrate — `crates/post-machine/src/optimizer/summary.rs`

An analysis, not a pass: absent from `PIPELINE` and `pass_names()`.
Call graph from `IrOp::Call` + `IrTerm::TailCall`, keyed by name.
Per-function forward worklist over blocks tracking the head offset
(join: equal merges, differing → `Unknown-from-here` — the
`block_entry_facts` shape). Ops move/write at the tracked offset; calls
apply callee summaries (Known displacement shifts the frame and imports
translated sets; Unknown poisons). **Observations are latch-provenance
driven:** the walk tracks the offset MF was last latched from; a `Check`
terminator converts that latch into an observed offset — reusing the
MF-coupling model's insight. Exit displacement: all `Return` paths must
agree, else `Unknown`.

## 5. PM pass — `dead-write` (`optimizer/dead_write.rs`)

Slot: per-function `PIPELINE`, before `move-elim` (sees plain `Wr`, not
fused forms). Backward liveness over offset bitmasks, only where the
forward frame is `Known`; exits all-live; `Brk` all-live; a move whose
latch is check-consumed (precomputed consumption map) uses its
destination offset; calls use the callee's translated `observes` (may)
and kill via `must_write` only. A `Wr` whose offset is dead after it is
deleted.

**Why deletion cannot corrupt MF, by construction:** a `Wr` latches MF
from its written value; a consumed latch means the offset was observed
→ live → never deleted; an unconsumed latch is re-latched before any
observation, so the perturbation is invisible. The module doc carries
this argument.

Contracts: volatile-GATED (dropping a write on a volatile band is the
canonical violation — joins the gated set, now five); `-O1`-only.

## 6. TM substrate — `crates/turing-machine/src/optimizer/spatial.rs`

Per-world spatial summaries beside `footprint.rs` (the name keeps the
value-vs-spatial distinction visible). Forward walk over the state
graph tracking per-tape offsets; joins poison per-tape; head-moving
cycles go `Unknown`. **Observation refinement:** a conditional state
observes tape `k` only if some row's pattern carries a concrete cell on
`k` — all-wildcard columns are unobserved; straight-line states observe
nothing. `must_write` on a tape requires every row of the state to
write it. Calls translate callee summaries through the binding record
(bindless = identity); grafts are spliced at compile time and need
nothing. Declared v1 coarsenings (measurement is the referee): any
`Stop` = may-end-run; any trap-carrying state (synthesized graft-hole
rows included) = observation point.

## 7. TM pass — `dead-write` (TM edition)

Backward liveness over per-tape offset masks on the state graph; exits
and `debugger` rows all-live; volatile bands skipped per-tape. Deletion
**rewrites the rule's write cell to `Keep`** — never deletes rules (row
shape is dispatch structure). Slot: before `dead-rows` in the TM
`PIPELINE`; `--fno-dead-write` symmetrical with PM.

## 8. Testing

- Summary units per crate: displacement Known/Unknown/SCC-top, the
  may/must split, PM latch-consumption, TM wildcard-column refinement,
  TM binding translation, `may_halt`.
- Pass units: the write–leave–return–overwrite shape; every barrier
  kind (Unknown-displacement callee, `may_halt` callee, `brk`, TM
  trap-state); a must-write-kill-through-call positive; the
  observed-write-kept negative.
- Equivalence programs both crates; per-crate mutation checks: collapse
  `must_write` into `may_write` → a named kill test fails; treat
  `Unknown` displacement as `Known(0)` → an equivalence program fails.
- The measurement task's corpus counts land in both optimizer pages.
- Floors re-verified: `-O0` bit-identity both sides, zero `crates/core`
  diff, the byte floors, gated/clean classification fixtures
  (`gated_passes.rs` PM; per-band fixture TM).

## 9. Rosters, docs, delivery

- PM `PIPELINE`: twelve per-function passes (`dead-write` before
  `move-elim`); gated set five. TM `PIPELINE`: `dead-write` before
  `dead-rows`. Registration cascades (flags, completions, drift guards)
  are mechanical on both sides.
- Both optimizer pages gain the pass section plus a shared "spatial
  summaries" subsection: the model, the may/must asymmetry, the
  precision theorem, the declared coarsenings, the measurement numbers.
- Delivery: plan → SDD on `feat/dead-write-liveness`, branched from
  master after #89 merges. #88 closes at the arc's merge; follow-ups
  filed at close: PM fused-write halves; TM `Stop` scope-sensitivity +
  trap-row refinement; window widening if measurement demands.
