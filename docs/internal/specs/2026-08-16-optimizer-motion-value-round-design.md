# Optimizer round: motion and value passes — design

Date: 2026-08-16. Status: approved design.
Driving issues: [#76](https://github.com/mellonis/machine-toolchains/issues/76)
(the round's triaged design input), [#32](https://github.com/mellonis/machine-toolchains/issues/32)
(dispatch-vector trampolines, folded into this round by ruling).

## 1. Context and goals

v0.3.0 closed the TM-1 arc; #76 is the next engineering round. Its design
input was triaged 2026-08-08 from an external survey of both pipelines,
with every candidate fact-checked against the machine model. This spec
turns the surviving candidates into one implementable round.

Goals, in one line each:

- Recover the measured ~5% dispatch-trampoline overhead on the TM flagship
  (#32) so generated code stops paying for what hand-written assembly
  never pays.
- Add the two PM passes the survey ranked highest and the fact-check
  upheld: inverse-move elimination over MF dataflow, and cross-arm suffix
  sinking.
- Widen TM `dead_rows` from same-band cover to vector dominance.
- Replace the guessed inline thresholds with measured ones.

## 2. Scope and non-goals

**In scope (the five items, per the scope ruling):**

1. TM: dispatch-target threading inside `jump_threading` (#32).
2. PM: `move_elim` — inverse-move-pair elimination with MF reasoning.
3. TM: `dead_rows` identical-effect subsumption (scope corrected by C2 —
   see the corrections block in §3).
4. PM: `tail_sink` — identical arm suffixes sink past the `check` join.
5. Both: inline threshold tuning via a measured sweep (axes corrected by
   C3).

**Deferred to its own round:** dead-write elimination via liveness — the
survey's "real missing pass". It needs the read-set substrate (the
triggered follow-up in the volatile/async/footprint spec §10), which is a
different size class of work. This round is its trigger, not its vehicle.

**Non-goals, stated so they are not silent caps:**

- No hoisting above a `check`, ever — a write latches MF, the check reads
  MF; the reorder changes the branch. Sinking only (item 4).
- `dead_rows` does pairwise dominance only. Union cover ("rows 2+3
  together cover row 5") is set-cover and out of scope.
- No LICM, PRE, path cloning, LTO-style cross-module inlining, or
  profile-guided anything (rejected in #76 with reasons; unchanged here).
- No core changes: the whole round lives in the two arch crates.
  **Zero `crates/core` diff is a round invariant.**

## 3. Rulings

Three decisions were made in the design conversation (2026-08-16) and are
fixed for the round:

**R1 — #32 acceptance is a mechanism gate, not a parity gate.** The pass
must thread every reachable bare-goto trampoline: the flagship run
executes **zero** post-dispatch trampolines at `-O1` (operationally: zero
executed `jmp` instructions reached as the *target* of a dispatch jump —
`djmp`, or `jm` when taken; a `jmp` merely following a not-taken `jm` in
the trace is not a trampoline), and the mode-equivalence matrix stays
green. The step/instruction/image numbers against the hand-written+`wrmv`
baseline are *recorded* as measurements. The residual gap (the fixed
write-then-move rule shape) is acknowledged as out of scope — a parity
requirement would hard-couple the round to a cause no in-scope pass can
fix.

**R2 — threading lives at IR level, inside `jump_threading`.** Rejected
alternatives, for the record: a link-time patch after relaxation (puts an
optimization in the un-gated `link` path, couples to the relaxation
fixpoint, grows core's contract surface for a TM-only win) and a
codegen-level fix that never emits the stub (changes `-O0` output shape,
weakening its faithful-structure debugging value, and bakes into codegen
a transformation the per-pass equivalence discipline cannot isolate and
`--fno-` cannot switch off). At IR level the pass runs before the
composition engine, so mono/frames/hybrid inherit it uniformly, and the
equivalence matrix covers it for free. `-O0` keeps its stubs by
definition of being the unoptimized floor.

**R3 — inline tuning is a measured sweep with a mechanical decision
rule** (§9), not a constant bump and not a size-neutral-only rule.

**Corrections C1–C3, ruled 2026-08-16 after the code reading that opened
plan-writing.** Three of the spec's assumptions did not survive contact
with the implementation; each correction was approved explicitly:

- **C1** — the trampoline is a codegen artifact, not an IR edge: a rule's
  transition already names its destination state directly, and state-level
  forwarder threading with cycle protection already exists in
  `jump_threading`. §5 is amended to the hint-based mechanism (the
  `dispatch_select` precedent); `TM_IR_VERSION` bumps 2 → 3. R2's spirit
  stands: the decision lives in the pass.
- **C2** — the spec'd `dead_rows` widening is vacuous: the pass already
  does full vector dominance with wildcards, and same-band is *provably*
  the whole sound subset for pairwise dead-row cover (a coverer has ≥
  wildcards, so it is never in an earlier band; a later-band coverer
  cannot kill a row that fires first). Item 3 is replaced by
  **identical-effect subsumption** (§6).
- **C3** — the sweep's call-site policy axis does not exist: PM already
  inlines any-size callees at a single call site (`ops ≤ 6 OR single
  site`), and TM already inlines eligible leaf callees at every site
  (`leaf ∧ rules ≤ 6`). §9 is amended to a cap-only sweep.

## 4. Born-under contracts

Every pass born or extended in this round starts life under three
standing contracts (#76; `docs/pmt/optimizer.md`, `docs/tmt/optimizer.md`):

1. **The un-stripped `brk` observability barrier.** No motion crosses an
   un-stripped `brk`; each pass states its `brk` behavior explicitly.
2. **The per-band volatile barrier.** Every access to a volatile band is
   observable: no value-persistence assumptions, no access-sequence
   changes. Every pass declares its per-band volatile behavior.
3. **PM two-variant classification.** Each PM pass is classified as sound
   in the volatile column or gated to the normal column only. This is a
   mandatory line in the pass's doc entry.

Round-wide floors, unchanged and re-verified: `-O0` bit-identity on both
sides (all five items are `-O1`-only), PM behavior unchanged at `-O0`,
zero `crates/core` diff.

## 5. TM: dispatch-target threading (`jump_threading`, item 1 / #32)

**The gap.** A bare `.tmc` rule — no write, no move, no debugger, only a
transition — lowers its dispatch-vector entry to a one-instruction `jmp`
stub instead of pointing at the destination body. `jump_threading` today
rewrites only ordinary control-flow edges (block terminators), so a stub
reached through a table entry is invisible to it. On the flagship run of
`++[>+++<-]>.` that is 23 of 44 executed dispatches paying one extra
executed `jmp` each (~5% of total steps); hand-written `.tma` names
destinations directly in `.targets` and pays nothing.

**The mechanism (as corrected by C1).** In the IR a rule's transition
already names its destination state; the stub exists only in codegen's
emission. The fix is therefore a lowering hint, exactly the
`dispatch_select`/`IrDispatch` precedent:

- `IrRule` gains a serialized `direct: bool` field (serde-default
  `false`; `TM_IR_VERSION` 2 → 3). `validate_world` enforces that
  `direct` appears only on a **bare** rule: `write == None ∧ moves ==
  None ∧ !debugger ∧ transition == Goto`.
- `jump_threading` (after its existing forwarder-state resolution, which
  already collapses chains and preserves cycles) marks every bare rule
  `direct = true`, counting newly-marked rules as its changes.
- Codegen, seeing `direct`, emits the destination *state's* label as the
  dispatch-table entry (and as the `jm` target in the `Branch` arm) and
  skips emitting the rule's stub block entirely.
- A rule carrying a `debugger` is not bare — its stub keeps the `brk`.
  This is contract §4.1 restated for table entries. Trap transitions and
  synthesized rows are not `Goto` and are never marked.

**Downstream interplay.** No orphan cleanup is needed — the stub is
simply not emitted. `dispatch_select` composes: its `Branch` arm consumes
the same hint. The composition engine is label-agnostic (the hint is
resolved before `.targets`/`.exits` are emitted), and the
everything-matrix (`-O0`/`-O1` × mono/frames/hybrid, trap kinds included)
is the standing proof. The `.rept` re-detection emitter's assemble-both
self-check guards the changed `-S` text; the flagship's pinned `-S`
fixture regenerates with the change documented in the ledger. `-O0` and
`--fno-jump-threading` builds never set the hint, so their emitted text
is untouched.

**Flags.** No new pass name: `--fno-jump-threading` switches the new
behavior off with the old.

## 6. TM: `dead_rows` identical-effect subsumption (item 3, as corrected by C2)

**Why the original item dissolved.** The pass already computes full
vector dominance with wildcards; its same-band restriction is a proof,
not a limitation (see C2 in §3). What remains sound and useful is the
*other* direction: deleting a more-specific row that a later covering row
would handle **identically**.

**The rule.** Within one state, delete row R when a later-evaluated row W
exists such that:

1. W covers R cell-wise (per tape: wildcard, or the same concrete index);
2. W's effect on R's inputs is identical to R's — per tape, the
   normalized write cells are equal (`None` normalizes to all-`Keep`),
   *or* R's pattern cell is `Index(a)` and one side writes `Keep` while
   the other writes `Index(a)` (writing the matched symbol back equals
   keeping it, on exactly R's inputs); normalized moves equal (`None` =
   all-`Stay`); transitions equal;
3. neither R nor W carries `debugger` (deleting R would elide R's
   reachable pause; W carrying one would *add* a pause on R's inputs);
4. **no intermediate capture**: every row M strictly between R and W in
   the emitted evaluation order is disjoint from R's pattern (some tape
   has different concrete indices) — otherwise R's inputs would fall to
   M, not W, after the deletion.

**Evaluation order without drift.** The emitted order is codegen's
re-banding (`[exact, sorted] ++ [partial, source] ++ [catch-all,
source]`). The pass must use the *same* order codegen uses, so the
banding computation is extracted into one shared helper both call —
duplicating it would invite silent divergence.

**Effect.** Match tables shrink (fewer rows for `mtc` to walk, smaller
images) and two-row shapes surface for `dispatch_select`. Deleting R
makes W fire on R's inputs — observably identical by construction, which
is precisely what the equivalence matrix re-verifies.

**Contracts.** Condition 3 is the `brk` barrier. **Volatile refinement
(ruled at implementation, superseding this spec's earlier blanket
claim):** the plain identical-effect case (byte-equal writes/moves) is
volatile-safe — the dynamic access sequence on R's inputs is identical
under W — but the `Keep ≡ Index(a)` fold is NOT: `Keep` emits no write
instruction while `Index(a)` emits one, and on a volatile band that
write is an observable bus transaction. The fold is therefore gated
per-tape on `IrTape::volatile`; literal equality keeps folding
everywhere. Additionally, states already lowered to the
`IrDispatch::Branch` shape are skipped whole (deleting a row would
break the two-row invariant `codegen::branch()` indexes by). No new
flag; `--fno-dead-rows` covers the subsumption. The module's
union-cover "recorded trigger" note stays as is.

## 7. PM: `move_elim` (item 2)

**Target shape:** an adjacent inverse move pair — `rgt; lft` or
`lft; rgt` — with nothing between, within one block. Straightline only
this round; the fixpoint plus `jump_threading`/`branch_fold` canonicalize
enough that cross-block pairs are not worth their complexity.

**Soundness.** The pair is *not* a no-op: every tape instruction latches
MF, so the pair re-couples MF to the cell under the restored head.
Elimination needs one of two proofs, both over the existing
`Uncoupled | Coupled` lattice in `optimizer/dataflow.rs`:

1. **MF-identical:** at the pair's program point MF is provably
   `Coupled` — it already equals the cell at head. The pair re-latches
   the same cell (head returns; adjacency means no intervening write), so
   MF is unchanged and elimination is invisible even to an immediate MF
   read.
2. **MF-dead:** every path from the pair to any MF read passes another
   tape op first (a dominating re-latch), so the pair's latch cannot be
   observed.

A pair meeting neither proof stays. Refinement applies only on provably
coupled paths, per the dataflow module's standing rule.

**Contracts.** An un-stripped `brk` between the moves breaks adjacency by
construction (stated in the doc entry anyway). **Volatile column: never
fires** — a move on a volatile band is itself an observable access;
classified normal-column-only.

**Surface.** New pass name `move_elim` in `optimizer::pass_names()`,
which feeds `--fno-move-elim`, `--emit-ir=after:move_elim`, the
completions registry, and its drift guard mechanically.

## 8. PM: `tail_sink` (item 4)

**Target shape:** both arms of a `check` end in an identical instruction
suffix before the same continuation. The suffix is emitted once past the
join; the arms jump to the shared copy. The scan runs from the join
upward and stops at the first difference — or at an un-stripped `brk` on
either arm: two arm breakpoints are two distinct observation sites and
never merge; code below a `brk` still sinks.

**Why it is the safe class.** Sinking never reorders anything: the check
has already executed, and each path runs exactly the instruction sequence
it ran before. Only the static copy count changes. Consequently the pass
is **volatile-column sound** — the executed access sequence on every
band, volatile included, is unchanged — making it the round's only PM
pass that fires in the volatile column. `variant_columns.rs` pins the
classification.

**Wins.** Primary: image size (deduplication). Secondary: a shared tail
ending in a call becomes a single `tail_call` candidate — hence the slot
before `tail_call`.

**Layout interaction, stated explicitly.** Codegen's fall-through
invariant (the 2007-inherited layout rule, active even at `-O0`) never
emits a jump to the physically next instruction, so the arm laid out
directly above the shared tail loses its join jump automatically; at most
one arm pays a real jump. The size accounting is therefore "suffix saved
minus at most one jump".

**Threshold.** Sink when the shared suffix is ≥ 2 ops — a conservative
net-size bound given the accounting above. The sweep harness (§9) records
whether 1-op sinks would pay; the bound moves on evidence only.

**Surface.** New pass name `tail_sink`; same mechanical flag/registry/
guard consequences as `move_elim`.

## 9. Inline threshold sweep (item 5)

**Current policies (as corrected by C3):** PM inlines a callee when
`ops ≤ INLINE_MAX_OPS (= 6)` **or** it has a single call site
module-wide (any size); TM inlines a **leaf** routine when
`rules ≤ INLINE_MAX_RULES (= 6)`, at every call site. The caps were
chosen conservatively, not measured; the policy structures are not under
test in this round.

**Harness.** One `#[ignore]`d measurement harness per the house explicit-
run convention, in-tree so it is repeatable. It compiles the corpus at
`-O1` per configuration and prints a table; it doubles as the round's
before/after instrument for the #32 numbers (R1's trampoline count and
the baseline comparison). Corpus: the UTM flagship on the pinned
`++[>+++<-]>.` run, both TM stdlib twins, the PM stdlib, and the PM
golden programs on their committed inputs. Metrics per configuration:
total executed steps, image bytes, instruction count.

**Axes (cap-only, per C3):** size cap ∈ {6, 12, 24}, PM and TM swept
independently — six configurations total. Each side's existing policy
structure (PM's single-site escape, TM's leaf-only constraint and
every-site splicing) stays untouched; TM's `inline` remains the sound
superset of the engine collapse under every configuration.

**Decision rule (mechanical, ruled R3):** pick the configuration with the
best corpus-wide step total, subject to image growth ≤ 5% over the
current baseline; ties break toward the smaller cap and the simpler
policy; if multi-site shows no step win, single-site stays. The chosen
constants and the justifying table land in `docs/pmt/optimizer.md` /
`docs/tmt/optimizer.md` and the round ledger when the sweep task runs —
the spec commits to the rule, not to numbers it cannot know yet.

**Guardrails:** `-O0` untouched by construction; equivalence suites green
at the chosen configuration on both sides, plus a spot-check at the most
aggressive swept configuration.

## 10. Pass roster, ordering, flags

- **PM roster grows nine → eleven.** Proposed per-function order:
  `tail_sink` after `branch_fold` and before `tail_call`; `move_elim`
  immediately **before** `fuse_tape_ops` at the end, so bare inverse
  pairs are eliminated before fusion considers absorbing a move into a
  fused write. The fixpoint loop reruns to
  convergence, so slotting is a starting order, not a correctness claim.
  The one hard ordering contract remains `tail_call` before `tail_merge`;
  this round introduces no new hard constraints.
- **TM roster unchanged by name.** Items 1 and 3 land inside
  `jump_threading` and `dead_rows`; existing `--fno-` flags cover them,
  and per-pass isolation for testing comes free.
- **User-facing surface** changes only PM-side: two new entries in
  `optimizer::pass_names()` cascade into `--fno-<pass>`,
  `--emit-ir=after:<pass>`, the completions registry, and the registry
  drift-guard test (including its `EXPECTED` mirrors where present). TM's
  completions registry is untouched.

## 11. Testing and acceptance

- **R1 as a committed test:** a flagship-run assertion counts executed
  post-dispatch trampolines at `-O1` via the trace (an executed `jmp`
  reached as the target of `djmp`, or of `jm` when taken — per R1's
  operational definition) and requires zero; the `-O0` count stays as
  the honest unoptimized baseline. The everything-matrix
  (`-O0`/`-O1` × mono/frames/hybrid incl. trap kinds) remains the
  behavioral proof.
- **Per-pass targeted tests:**
  - threading: bare rule marked, debugger rule not marked, non-`Goto`
    transitions not marked, `validate_world` rejects `direct` on a
    non-bare rule; codegen consumption — `.targets` names the state
    label, the stub block is absent, the `Branch` arm's `jm` targets the
    state label, `-O0` text byte-unchanged;
  - `dead_rows` subsumption: the plain positive, the
    Keep/`Index(matched)` write-equivalence positive, the
    differing-effect negative, the intermediate-capture negative, the
    debugger-on-either-row negative;
  - `move_elim`: both proofs positive, kept-pair negative (uncoupled +
    immediate MF read);
  - `tail_sink`: suffix shapes, the `brk` scan stop, threshold boundary,
    volatile-column fires fixture.
- **Harness extensions:** `opt_equivalence.rs` gains programs exercising
  each new PM pass; `variant_columns.rs` pins both PM classifications;
  `gated_passes.rs` covers both new `--fno-` flags.
- **Mutation review, per the volatile-round precedent:** each new or
  extended pass gets a deliberately-broken variant that the suite must
  catch before the pass counts as tested.
- **Round invariants re-verified at the end:** `-O0` bit-identity both
  sides, zero `crates/core` diff, clippy `-D warnings`, `cargo fmt
  --check`.

## 12. Documentation and delivery

- `docs/pmt/optimizer.md`: entries for `move_elim` and `tail_sink` (each
  with brk, volatile, and column-classification lines), the tuned inline
  constants with the sweep table.
- `docs/tmt/optimizer.md`: the `jump_threading` dispatch-target
  extension, the `dead_rows` dominance widening with the band-order rule
  and the lint asymmetry note, the tuned inline constants.
- CHANGELOG rides the next release cut per house convention; this round
  ships on master only. The cut's version block declares **TM IR 3**
  (the `direct` rule field, C1); every other version space is unchanged
  by this round.
- #76 and #32 close when the round merges; the deferred dead-write-
  elimination pass gets its own issue at close, linking the read-set
  substrate follow-up.
