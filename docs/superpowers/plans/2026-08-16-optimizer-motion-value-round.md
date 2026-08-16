# Optimizer Motion/Value Round Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the five-item optimizer round — TM dispatch-target
threading (#32), PM `move_elim`, TM `dead_rows` identical-effect
subsumption, PM `tail_sink`, and the measured inline-cap sweep — per the
approved spec.

**Architecture:** All work lives in the two arch crates. TM #32 is a
serialized per-rule hint (`IrRule.direct`) set by `jump_threading` at
`-O1` and consumed by codegen's dispatch-target emission (the
`dispatch_select` hint precedent). The two new PM passes join the
per-function `PIPELINE` and the `pass_names()`-driven flag surface.
Subsumption extends `dead_rows` behind a shared emitted-row-order helper.
The sweep parameterizes the inline cap through `OptOptions` and picks
constants by the spec's decision rule.

**Tech Stack:** Rust (cargo workspace), serde IR, the in-repo test
harnesses (`opt_equivalence`, `gated_passes`, `mode_equivalence`,
golden programs). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-16-optimizer-motion-value-round-design.md`
(read it first — the rulings R1–R3 and corrections C1–C3 are binding).

## Global Constraints

- Every change is `-O1`-only: `-O0` output stays byte-identical to plain
  codegen on both sides (standing floor; verified in Task 10).
- **Zero `crates/core` diff** across the whole round (spec §2).
- The un-stripped `brk` barrier and per-band volatile contracts bind
  every pass (spec §4); each PM pass carries an explicit volatile-column
  classification: `move-elim` gated, `tail-sink` clean.
- `TM_IR_VERSION` moves 2 → 3 exactly once (Task 1); no other version
  space moves this round.
- Gates on every commit: `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo fmt --check` clean; the touched crate's tests green.
- Code comments cite durable docs pages only (`docs/tmt/optimizer.md
  (…)` style) — never the spec, never issue numbers.
- Commit style: conventional with scope (`feat(turing-machine): …`,
  `feat(post-machine): …`, `test(…): …`, `docs(…): …`). No AI
  attribution footers.
- Each new/extended pass ends its task with a mutation check: introduce
  the stated deliberate bug, confirm the named test fails, revert.

---

### Task 1: TM IR — the `direct` rule hint

**Files:**
- Modify: `crates/turing-machine/src/ir.rs` (`IrRule` at ~line 154,
  `TM_IR_VERSION` at line 62, `validate_world` — find with
  `grep -n "fn validate_world" crates/turing-machine/src/ir.rs`)

**Interfaces:**
- Consumes: existing `IrRule`, `IrTransition::Goto`.
- Produces: `IrRule.direct: bool` (serde-default `false`, skipped when
  false), `TM_IR_VERSION == 3`, and a `validate_world` rule: `direct`
  is legal only on a bare rule (`write.is_none() && moves.is_none() &&
  !debugger && matches!(transition, IrTransition::Goto { .. })`). Tasks
  2, 3, 5 rely on the field name `direct` and this validation.

- [ ] **Step 1: Write the failing tests** in `ir.rs`'s existing
  `#[cfg(test)]` module (find its validator tests for the message
  style; mirror it):

```rust
#[test]
fn direct_is_rejected_on_a_non_bare_rule() {
    // Build a minimal valid world via the same helpers the existing
    // validate_world tests use, then set `direct` on a rule that
    // carries a write. Assert validate_world returns Err mentioning
    // "direct".
}

#[test]
fn direct_round_trips_and_defaults_false() {
    // A world serialized without the field deserializes with
    // direct == false; a world with direct == true on a bare rule
    // round-trips to_json/from_json unchanged.
}
```

Write them as real code against the file's existing test helpers (they
construct `IrWorld`s inline; copy the smallest existing constructor).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-turing-machine --lib ir`
Expected: FAIL — `direct` field does not exist.

- [ ] **Step 3: Implement** — add to `IrRule` (next to `synthesized`,
  same serde pattern):

```rust
    /// Lowering hint (the `jump_threading` pass's output — never set by
    /// lowering): this bare rule's dispatch entry names its `Goto`
    /// destination state directly and no stub block is emitted
    /// (docs/tmt/optimizer.md (dispatch-target threading)).
    #[serde(default, skip_serializing_if = "is_false")]
    pub direct: bool,
```

Bump `TM_IR_VERSION` to 3 and extend its doc comment with one line for
the field. In `validate_world`, where rules are walked, add:

```rust
if r.direct
    && !(r.write.is_none()
        && r.moves.is_none()
        && !r.debugger
        && matches!(r.transition, IrTransition::Goto { .. }))
{
    return Err(format!(
        "state {} rule {}: `direct` on a non-bare rule",
        st.id, k
    ));
}
```

(match the surrounding error-message style exactly). Fix every struct
literal of `IrRule` in the crate that now misses the field
(`cargo check` lists them; add `direct: false`).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-turing-machine --lib ir`
Expected: PASS.

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy -p mtc-turing-machine --all-targets -- -D warnings && cargo fmt --check
git add crates/turing-machine/src/ir.rs
git commit -m "feat(turing-machine): IrRule.direct lowering hint - TM IR version 3"
```

---

### Task 2: TM `jump_threading` marks bare rules

**Files:**
- Modify: `crates/turing-machine/src/optimizer/jump_threading.rs`

**Interfaces:**
- Consumes: `IrRule.direct` (Task 1).
- Produces: after this pass runs at `-O1`, every bare `Goto` rule has
  `direct == true`; the pass's change count includes newly-marked rules
  (so the fixpoint driver converges: re-marking counts 0).

- [ ] **Step 1: Write the failing tests** in the module's existing
  `#[cfg(test)]` tests (mirror its world-construction helpers):

```rust
#[test]
fn a_bare_goto_rule_is_marked_direct() { /* run(); assert rule.direct */ }

#[test]
fn a_debugger_rule_is_not_marked() { /* debugger: true => !direct */ }

#[test]
fn non_goto_transitions_are_not_marked() {
    // Return / Stop / Halt / CallThen / TailCall / TrapRead rules
    // stay !direct.
}

#[test]
fn a_rule_with_a_write_or_move_is_not_marked() { }

#[test]
fn marking_is_idempotent_for_the_fixpoint() {
    // First run() returns  >0 (marks), second run() returns 0.
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-turing-machine --lib optimizer::jump_threading`
Expected: FAIL.

- [ ] **Step 3: Implement** — at the end of `run(w)`, after the existing
  forwarder retargeting:

```rust
    // Dispatch-target threading (docs/tmt/optimizer.md (dispatch-target
    // threading)): a bare rule's entry can name its destination state
    // directly, so codegen skips the one-jmp stub. Marking only — the
    // emission change lives in codegen, keyed off `direct`.
    for st in &mut w.states {
        for r in &mut st.rules {
            if !r.direct
                && r.write.is_none()
                && r.moves.is_none()
                && !r.debugger
                && matches!(r.transition, IrTransition::Goto { .. })
            {
                r.direct = true;
                changes += 1;
            }
        }
    }
```

(adapt to the function's actual change-count variable; run the marking
AFTER forwarder resolution so a retargeted rule is marked with its final
destination in the same application).

- [ ] **Step 4: Run to verify pass** — same command, PASS. Also run
  `cargo test -p mtc-turing-machine` — the driver's debug-build
  `validate_world` re-check now exercises Task 1's rule on every
  optimized test program.

- [ ] **Step 5: Mutation check** — temporarily drop the `!r.debugger`
  conjunct; `a_debugger_rule_is_not_marked` must fail; revert.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src/optimizer/jump_threading.rs
git commit -m "feat(turing-machine): jump_threading marks bare goto rules direct"
```

---

### Task 3: TM codegen consumes the hint

**Files:**
- Modify: `crates/turing-machine/src/codegen.rs` — `conditional()`
  (~line 291: `rule_labels` minting, the classify loop at ~307, the
  rule-block emission later in the fn) and the `Branch` arm builder
  (~line 229, `branch`).
- Create: `crates/turing-machine/tests/direct_dispatch.rs`

**Interfaces:**
- Consumes: `IrRule.direct` (Tasks 1–2); `compile` at
  `crates/turing-machine/src/compiler.rs:2122`
  (`compile(source, CompileOptions) -> CompileOutput`; the emitted
  assembly text field is the one
  `compile_object_equals_assembly_of_its_emitted_tma`
  (compiler.rs:4011) reads — use the same field).
- Produces: for a `direct` rule, the `.targets` entry (or the Branch
  `jm` operand) is the destination state's label (`w.states[id].name`)
  and no stub block is emitted for that rule.

- [ ] **Step 1: Write the failing tests** in the new
  `tests/direct_dispatch.rs` (helpers copied from
  `tests/opt_equivalence.rs`'s TM build pattern):

```rust
//! The dispatch-target threading emission contract
//! (docs/tmt/optimizer.md (dispatch-target threading)).

const BARE: &str = "\
alphabet a { '0', '1' }
tape t: a;
machine {
  state s0 { ['0'] -> goto s1;  ['1'] -> write ['1'] goto s2; }
  state s1 { [*] -> write ['1'] stop; }
  state s2 { [*] -> stop; }
}
";
// Adjust the surface syntax to a compiling .tmc program — crib the
// smallest fixture from tests/opt_equivalence.rs / tests/tmc_fold.rs
// and keep: one bare rule (-> goto s1) and one payload rule in s0.

#[test]
fn o1_targets_name_the_state_and_the_stub_is_gone() {
    let o1 = emitted_asm(BARE, OptLevel::O1);
    // The bare rule's .targets line names s1's own label; the minted
    // s0__0-style stub label for that rule does not appear at all.
    assert!(o1.contains("s1"), "targets must name the state");
    assert!(!o1.contains("s0__0"), "no stub block for the direct rule");
    // The payload rule keeps its stub:
    assert!(o1.contains("s0__1"));
}

#[test]
fn o0_and_fno_jump_threading_are_byte_unchanged() {
    let o0 = emitted_asm(BARE, OptLevel::O0);
    assert!(o0.contains("s0__0"), "-O0 keeps the stub");
    let fno = emitted_asm_disabled(BARE, &["jump-threading"]);
    assert!(fno.contains("s0__0"), "--fno keeps the stub");
}

#[test]
fn a_branch_state_jm_targets_the_state_directly() {
    // Two-row selective-then-catch-all state whose selective rule is
    // bare: after dispatch_select + threading, the jm operand is the
    // destination state's label.
}

#[test]
fn behavior_is_unchanged_o0_vs_o1() {
    // Compile BARE at -O0 and -O1, assemble+link+run on the same
    // seeded tape (crib the run helper from tests/opt_equivalence.rs),
    // compare (outcome kind, snapshots, heads).
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-turing-machine --test direct_dispatch`
Expected: FAIL (stub still emitted).

- [ ] **Step 3: Implement** in `conditional()`: where the classify loop
  builds `let tgt = rule_labels[k].clone();`, select instead:

```rust
        let tgt = if r.direct {
            let IrTransition::Goto { state } = r.transition else {
                unreachable!("validate_world: direct implies Goto");
            };
            w.states[state as usize].name.clone()
        } else {
            rule_labels[k].clone()
        };
```

and skip the rule-block emission for `direct` rules (where the per-rule
blocks are built, `continue` on `r.direct`). In the `branch` builder,
apply the same selection to the `jm` operand when the selective rule is
`direct` (the catch-all's fall-through block is NOT a jump target —
leave it as is, with a comment saying why).

- [ ] **Step 4: Run to verify pass** — the new test file, then the whole
  crate: `cargo test -p mtc-turing-machine`. The everything-matrix and
  golden tests must stay green. The flagship `-S`/`.rept` pinned
  fixture will change shape — regenerate it by the fixture's documented
  regen path (see `tests/rept_emit.rs`), review the diff (only stub
  lines and `.targets` operands may move), and note the regen in the
  ledger.

- [ ] **Step 5: Mutation check** — make the `tgt` selection ignore
  `direct` (always stub); `o1_targets_name_the_state…` must fail;
  revert.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src/codegen.rs crates/turing-machine/tests/direct_dispatch.rs <regenerated fixtures>
git commit -m "feat(turing-machine): codegen emits direct dispatch targets, skipping bare-rule stubs"
```

---

### Task 4: the R1 trampoline gate on the flagship

**Files:**
- Create: `crates/turing-machine/tests/trampoline_gate.rs`

**Interfaces:**
- Consumes: the compiled flagship (`docs/examples/` — the `.tmc` twin
  used by `tests/golden_programs.rs`; reuse its build+tape helpers),
  `machine.debug_tapes(options)` / `session.ip()` /
  `session.step_in_tapes(devices)` and
  `mtc_core::asm::listing_line(&tm1_syntax(), &exe.code, ip, &resolve)`
  exactly as `cli/run.rs::drive_traced` (line 277) uses them.
- Produces: the round's acceptance gate as a committed test.

- [ ] **Step 1: Write the test** (it will FAIL before Tasks 2–3 land if
  written first; this task is sequenced after them, so write it now and
  expect PASS at `-O1` — the interesting assertion is the count):

```rust
//! Spec R1: at -O1 the flagship executes ZERO post-dispatch
//! trampolines — an executed `jmp` reached as the target of `djmp`,
//! or of `jm` when taken (docs/tmt/optimizer.md (dispatch-target
//! threading)).

fn trampolines(exe: &Executable, program: &str) -> (u64, u64) {
    // Build tapes exactly as tests/golden_programs.rs does for the
    // UTM; then step:
    //   let mut session = machine.debug_tapes(options);
    //   loop {
    //       let ip = session.ip();
    //       let (line, len) = listing_line(&syntax, &exe.code, ip, &|_| None);
    //       let mnemonic = /* parse from `line` after the addr column */;
    //       if mnemonic == "jmp"
    //           && prev.map_or(false, |(pm, pip, plen)| {
    //               pm == "djmp" || (pm == "jm" && ip != pip + plen)
    //           })
    //       { count += 1; }
    //       prev = Some((mnemonic, ip, len as u32));
    //       match session.step_in_tapes(&mut devices) { /* break on finish */ }
    //   }
    // Returns (trampoline count, total steps).
}

#[test]
fn o1_flagship_executes_zero_trampolines() {
    let (count, _steps) = trampolines(&build_flagship(OptLevel::O1), "++[>+++<-]>.");
    assert_eq!(count, 0);
}

#[test]
fn o0_flagship_still_pays_them() {
    // The honest unoptimized baseline: strictly more than zero.
    let (count, _steps) = trampolines(&build_flagship(OptLevel::O0), "++[>+++<-]>.");
    assert!(count > 0);
}
```

Fill the helper bodies with real code cribbed from
`tests/golden_programs.rs` (flagship build + tape seeding) and
`cli/run.rs:277` (the session loop). Check `listing_line`'s actual
return shape at its definition before parsing.

- [ ] **Step 2: Run**

Run: `cargo test -p mtc-turing-machine --test trampoline_gate`
Expected: PASS (zero at `-O1`, >0 at `-O0`). If the `-O1` count is not
zero, Tasks 2–3 missed a shape — diagnose before proceeding (the count
tells you which dispatches still trampoline; print their `ip`s).

- [ ] **Step 3: Record the measurements** — extend the test with an
  `#[ignore]`d `measurements` fn printing the #32 table (executed
  `jmp`/`djmp`/`jm`, trampolines, steps, image bytes, instruction count
  vs the hand-written twin built the way `tests/golden_programs.rs`
  builds it). Run it once
  (`cargo test -p mtc-turing-machine --test trampoline_gate measurements -- --ignored --nocapture`)
  and paste the table into the round ledger
  (`.superpowers/` progress notes) — the docs get it in Task 9.

- [ ] **Step 4: Commit**

```bash
git add crates/turing-machine/tests/trampoline_gate.rs
git commit -m "test(turing-machine): the R1 zero-trampoline gate on the flagship"
```

---

### Task 5: TM `dead_rows` identical-effect subsumption

**Files:**
- Modify: `crates/turing-machine/src/codegen.rs` (extract the banding
  into a shared helper), `crates/turing-machine/src/optimizer/dead_rows.rs`

**Interfaces:**
- Consumes: the classify loop in `codegen.rs::conditional` (~line 307).
- Produces: `pub(crate) fn emitted_row_order(st: &IrState) -> Vec<usize>`
  (row indices in emitted order: exact sorted lexicographically by the
  SAME key codegen sorts by, then partial in source order, then
  catch-all in source order) — used by BOTH `conditional` and the new
  subsumption; and the subsumption itself inside `dead_rows::run`.

- [ ] **Step 1: Extract the helper first (refactor, no behavior
  change).** Move the classification out of `conditional` into
  `emitted_row_order` (place it in `codegen.rs`, `pub(crate)`), rewrite
  `conditional` to consume it, and run the whole crate:
  `cargo test -p mtc-turing-machine` must stay green with zero fixture
  changes (byte-identical emission is the refactor's proof).

- [ ] **Step 2: Commit the refactor**

```bash
git add crates/turing-machine/src/codegen.rs
git commit -m "polish(turing-machine): extract emitted_row_order from conditional()"
```

- [ ] **Step 3: Write the failing subsumption tests** in
  `dead_rows.rs`'s test module (mirror its existing world builders).
  The spec's rule, spelled as tests:

```rust
#[test]
fn a_specific_row_with_identical_effect_is_subsumed_by_the_catch_all() {
    // s: ['0'] -> write ['1'] goto t;  [*] -> write ['1'] goto t;
    // The exact row dies; the catch-all serves its inputs identically.
}

#[test]
fn keep_vs_writing_the_matched_symbol_back_counts_as_identical() {
    // ['0'] -> write ['0'] goto t;  [*] -> goto t;   (write None = keep)
    // R writes its own matched symbol; W keeps. Identical on R's inputs.
}

#[test]
fn differing_effect_survives() {
    // ['0'] -> write ['1'] goto t;  [*] -> goto t;   — R stays.
}

#[test]
fn an_intermediate_overlapping_row_blocks_subsumption() {
    // Emitted order R(exact), M(partial overlapping R), W(catch-all):
    // deleting R would hand R's inputs to M, not W — R stays.
}

#[test]
fn debugger_on_either_row_blocks_subsumption() { }

#[test]
fn dead_rows_same_band_cover_still_works() {
    // The pre-existing behavior, pinned unchanged.
}
```

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p mtc-turing-machine --lib optimizer::dead_rows`
Expected: the new positives FAIL.

- [ ] **Step 5: Implement** in `dead_rows::run`, after the existing
  same-band cover, iterating `emitted_row_order(st)`:

```rust
// Identical-effect subsumption (docs/tmt/optimizer.md (row
// subsumption)): delete emitted-earlier row R when a later row W
// covers it with identical effect and no row between them can
// capture R's inputs.
```

Implementation outline (write it as real code): for each pair (R at
emitted position i, W at position j > i): check (1) cover — per tape,
`W wildcard ∨ W == R` with R concrete on that tape or both wildcard;
(2) normalized effect equality — normalize `write`/`moves` `None` to
all-`Keep`/all-`Stay`, then per tape `wr_eq(rw, ww, r.pattern[k])`
where `wr_eq` treats `Keep` ≡ `Index(a)` when `r.pattern[k] ==
Index(a)`; transitions compared with `==` (note `direct` from Task 1 is
a lowering hint, not effect — compare transitions, not the flag);
(3) `!r.debugger && !w.debugger && !r.synthesized && !w.synthesized`;
(4) every row M with i < pos(M) < j is pattern-disjoint from R (some
tape where both are concrete and different). Delete matching Rs
(collect indices, remove, count).

- [ ] **Step 6: Run to verify pass** — module tests, then the whole
  crate (equivalence matrix + goldens green).

- [ ] **Step 7: Mutation check** — disable condition (4); the
  intermediate-capture negative must fail; revert.

- [ ] **Step 8: Commit**

```bash
git add crates/turing-machine/src/optimizer/dead_rows.rs
git commit -m "feat(turing-machine): dead_rows identical-effect subsumption across bands"
```

---

### Task 6: PM `move_elim`

**Files:**
- Create: `crates/post-machine/src/optimizer/move_elim.rs`
- Modify: `crates/post-machine/src/optimizer/mod.rs` (module decl,
  `PIPELINE` entry `("move-elim", move_elim::run)` inserted BEFORE
  `("fuse-tape-ops", …)`; `gated_pass_names()` gains `"move-elim"`),
  `crates/post-machine/tests/gated_passes.rs` (the gated verdict),
  `crates/post-machine/tests/opt_equivalence.rs` (an exercising program)

**Interfaces:**
- Consumes: `dataflow::{Fact, block_entry_facts, transfer_op}`
  (`crates/post-machine/src/optimizer/dataflow.rs`), `IrOp`, `IrTerm`.
- Produces: `pub fn run(f: &mut IrFunction) -> u32` deleting provable
  adjacent inverse move pairs; pass name `"move-elim"`.

- [ ] **Step 1: Write the failing unit tests** in the new module (crib
  the block-builder helpers from `tail_merge.rs`'s tests):

```rust
#[test]
fn coupled_pair_is_eliminated_even_before_a_check() {
    // wr 1; rgt; lft; term Check — fact at the pair is Coupled(None)
    // (the wr), so the pair goes; the check reads identical MF.
}

#[test]
fn uncoupled_pair_before_a_check_is_kept() {
    // Entry block: rgt; lft; term Check — entry fact Uncoupled, no
    // later latch: kept.
}

#[test]
fn uncoupled_pair_is_eliminated_when_a_latch_dominates_the_next_read() {
    // rgt; lft; wr 0; term Check — MF-dead: the wr re-latches.
}

#[test]
fn a_call_between_pair_and_check_blocks_mf_dead() {
    // rgt; lft; call f; term Check — callee may read MF at entry: kept
    // (unless coupled — build it uncoupled).
}

#[test]
fn a_brk_after_the_pair_blocks_mf_dead() { /* Brk = observation */ }

#[test]
fn cross_successor_mf_dead_walks_goto_chains() {
    // Pair in block A (term Goto B); B starts with a tape op: eliminated.
}

#[test]
fn lft_rgt_order_also_matches() { }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --lib optimizer::move_elim`
Expected: FAIL (module doesn't exist → add the empty module +
registration first so it compiles, tests fail on behavior).

- [ ] **Step 3: Implement.** Module doc: state the target shape, the
  two proofs, the gated-volatile classification (cite
  `docs/pmt/optimizer.md (move elimination)`). Core:

```rust
use std::collections::{HashMap, HashSet};

use super::dataflow::{Fact, block_entry_facts, transfer_op};
use crate::ir::{IrFunction, IrOp, IrTerm};

fn is_inverse_pair(a: &IrOp, b: &IrOp) -> bool {
    matches!(
        (a, b),
        (IrOp::Rgt { .. }, IrOp::Lft { .. }) | (IrOp::Lft { .. }, IrOp::Rgt { .. })
    )
}

/// MF-dead: from `block`'s ops after position `i` (the pair removed),
/// every path re-latches MF (a tape op) before any MF read (a Check
/// terminator), observation (Brk) or opaque reader (Call, TailCall).
fn mf_dead_after(f: &IrFunction, index: &HashMap<u32, usize>, block: usize, i: usize, seen: &mut HashSet<u32>) -> bool {
    for op in &f.blocks[block].ops[i..] {
        match op {
            IrOp::Lft { .. } | IrOp::Rgt { .. } | IrOp::Wr { .. }
            | IrOp::WrLft { .. } | IrOp::WrRgt { .. } => return true,
            IrOp::Brk { .. } | IrOp::Call { .. } => return false,
        }
    }
    match &f.blocks[block].term {
        IrTerm::Check { .. } => false,
        IrTerm::Return | IrTerm::Halt => true,
        IrTerm::TailCall { .. } => false,
        IrTerm::FallThrough { to } | IrTerm::Goto { to } => {
            if !seen.insert(*to) {
                return true; // latch-free, read-free cycle: no read on this path
            }
            index.get(to).is_some_and(|&b| mf_dead_after(f, index, b, 0, seen))
        }
    }
}

pub fn run(f: &mut IrFunction) -> u32 {
    let entry_facts = block_entry_facts(f);
    let index: HashMap<u32, usize> = f.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let mut deletions: Vec<(usize, usize)> = Vec::new(); // (block idx, op idx of pair start)
    for (bi, b) in f.blocks.iter().enumerate() {
        let Some(&entry) = entry_facts.get(&b.id) else { continue }; // unreachable block
        let mut fact = entry;
        let mut i = 0;
        while i + 1 < b.ops.len() {
            if is_inverse_pair(&b.ops[i], &b.ops[i + 1]) {
                let sound = matches!(fact, Fact::Coupled(_))
                    || mf_dead_after(f, &index, bi, i + 2, &mut HashSet::new());
                if sound {
                    deletions.push((bi, i));
                    // Skip past the pair; facts for later ops are
                    // recomputed conservatively by continuing the walk
                    // over the ops we KEEP.
                    i += 2;
                    continue;
                }
            }
            fact = transfer_op(fact, &b.ops[i]);
            i += 1;
        }
    }
    // apply deletions back-to-front per block; return count
    let n = deletions.len() as u32;
    for (bi, i) in deletions.into_iter().rev() {
        f.blocks[bi].ops.drain(i..=i + 1);
    }
    n
}
```

Care in the real implementation: after skipping an eliminated pair the
walk continues with the PRE-pair fact (correct: the pair is gone). One
pair per scan position; the fixpoint driver reruns for cascades.

- [ ] **Step 4: Registration** — in `mod.rs`: `pub mod move_elim;`,
  PIPELINE entry before `fuse-tape-ops`, `gated_pass_names()` returns
  `&["cell-state", "branch-fold", "fuse-tape-ops", "move-elim"]`, and
  extend the gated-set doc comment with the move-elim paragraph (a move
  on a volatile band is an observable access; eliminating the pair
  drops two accesses — gated).

- [ ] **Step 5: Run to verify pass** — module tests, then
  `cargo test -p mtc-post-machine`. `tests/completions_registry.rs`
  must pass UNCHANGED (it reads `pass_names()` dynamically — if it
  fails, read its message; only hand-maintained mirrors like
  `EXPECTED_TOP_LEVEL` cover subcommands, not passes).

- [ ] **Step 6: The gated verdict test** — in `tests/gated_passes.rs`,
  mirror an existing gated pin: a program whose normal column loses the
  pair and whose volatile column keeps it:

```rust
/// move-elim is GATED: `right; left;` after a write re-couples MF on a
/// real tape but is two observable transactions on a volatile one.
const MOVE_PAIR: &str = "main() {\n    mark;\n    right;\n    left;\n    check(1, !);\n1:  unmark;\n}\n";

#[test]
fn move_elim_is_gated() {
    assert!(count_moves(&normal(MOVE_PAIR)) < count_moves(&volatile(MOVE_PAIR)));
}
```

(write `count_moves` against the listing text — count `lft`/`rgt`
mnemonics; crib the file's existing text-scan helpers).

- [ ] **Step 7: Equivalence program** — add to
  `tests/opt_equivalence.rs` a program built around the pair shapes
  (both proofs + the kept case) run through `assert_equivalent` on
  several tapes.

- [ ] **Step 8: Mutation check** — make `run` eliminate on
  `Fact::Uncoupled` too; the kept-pair unit test AND the equivalence
  program must fail; revert.

- [ ] **Step 9: Commit**

```bash
git add crates/post-machine/src/optimizer/ crates/post-machine/tests/
git commit -m "feat(post-machine): move-elim pass - inverse move pairs over MF dataflow"
```

---

### Task 7: PM `tail_sink`

**Files:**
- Create: `crates/post-machine/src/optimizer/tail_sink.rs`
- Modify: `crates/post-machine/src/optimizer/mod.rs` (module decl,
  PIPELINE entry `("tail-sink", tail_sink::run)` AFTER
  `("branch-fold", …)` and BEFORE `("tail-call", …)`; NOT in
  `gated_pass_names` — extend that doc comment's clean list),
  `crates/post-machine/tests/gated_passes.rs` (the CLEAN verdict),
  `crates/post-machine/tests/opt_equivalence.rs`

**Interfaces:**
- Consumes: `IrOp`, `IrTerm`; the op-identity comparison mirrors
  `tail_merge.rs::same_op` (copy it locally or make `tail_merge`'s
  `pub(super)` and reuse — prefer reuse, one identity definition).
- Produces: `pub fn run(f: &mut IrFunction) -> u32`; pass name
  `"tail-sink"`.

- [ ] **Step 1: Write the failing unit tests**:

```rust
#[test]
fn identical_two_op_suffixes_sink_past_the_join() {
    // A: [Wr 1, Rgt, Rgt] -> Goto J;  B: [Lft, Rgt, Rgt] -> Goto J;
    // J has exactly these two preds. After: A [Wr 1], B [Lft],
    // J.ops starts [Rgt, Rgt].
}

#[test]
fn a_one_op_suffix_is_below_threshold() { }

#[test]
fn a_brk_stops_the_upward_scan() {
    // Suffix [Brk, Rgt, Rgt] on both arms: only [Rgt, Rgt] sinks.
}

#[test]
fn a_third_predecessor_blocks_sinking() {
    // J also reachable from a Check edge: nothing moves.
}

#[test]
fn the_entry_block_never_gains_a_prefix() { /* J == blocks[0]: skip */ }

#[test]
fn self_loop_join_is_skipped() { /* A == J */ }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --lib optimizer::tail_sink`

- [ ] **Step 3: Implement.** Module doc: sinking-only rationale, the
  dynamic-identity argument, the volatile-clean classification, the
  fall-through layout note (`docs/pmt/optimizer.md (tail sinking)`).
  Algorithm:

```rust
pub fn run(f: &mut IrFunction) -> u32 {
    // Predecessor census: block id -> (jump-pred block indices, other-edge count).
    // A jump pred reaches the join via Goto/FallThrough; Check edges and
    // entry count as "other" and disqualify the join.
    // For each join J (not blocks[0]) with exactly two jump preds A != B
    // != J and zero other edges:
    //   let n = common_suffix_len(A.ops, B.ops)  // same_op pairs, stop at Brk
    //   if n >= 2 {
    //       let suffix: Vec<IrOp> = A.ops[A.ops.len()-n..].to_vec();
    //       A.ops.truncate(...); B.ops.truncate(...);
    //       J.ops.splice(0..0, suffix);
    //       return n as u32; // one join per application; driver reruns
    //   }
    // 0
}
```

Write it out fully with the census built in one walk over terminators
(entry block's id counts one "other" edge for itself). `common_suffix_len`
compares with `same_op` and returns the run length up to (not
including) any `Brk` on either side.

- [ ] **Step 4: Registration + run to verify pass** — module tests,
  crate tests.

- [ ] **Step 5: The clean verdict test** in `tests/gated_passes.rs`,
  mirroring an existing clean pin (pass FIRES in the volatile column,
  proven against a `--fno-tail-sink` control):

```rust
const SINKABLE: &str = "main() {\n    check(1, 2);\n1:  mark;\n    right;\n    right;\n    goto 3;\n2:  left;\n    right;\n    right;\n3:  unmark;\n}\n";

#[test]
fn tail_sink_is_clean() {
    // volatile() output is SHORTER than volatile-with-tail-sink-off:
    let with = volatile(SINKABLE);
    let without = listing(SINKABLE, &[gated_pass_names(), &["tail-sink"]].concat());
    assert!(with.len() < without.len());
}
```

(adapt to the file's actual helper names; the existing clean verdicts
show the exact pattern — mirror one).

- [ ] **Step 6: Equivalence program** — add the sink shapes to
  `tests/opt_equivalence.rs` through `assert_equivalent`.

- [ ] **Step 7: Mutation check** — allow sinking when a third edge
  reaches the join; `a_third_predecessor_blocks_sinking` must fail plus
  the equivalence program; revert.

- [ ] **Step 8: Commit**

```bash
git add crates/post-machine/src/optimizer/ crates/post-machine/tests/
git commit -m "feat(post-machine): tail-sink pass - arm suffixes dedup past the check join"
```

---

### Task 8: inline-cap plumbing (both crates)

**Files:**
- Modify: `crates/post-machine/src/optimizer/mod.rs` (add
  `pub inline_cap: Option<usize>` to `OptOptions`; change
  `ProgramPassFn` to `fn(&mut IrProgram, &OptOptions) -> u32` and pass
  options through the driver), `crates/post-machine/src/optimizer/inline.rs`
  (`let cap = options.inline_cap.unwrap_or(INLINE_MAX_OPS);`),
  `crates/post-machine/src/compiler.rs` (plumb a matching
  `CompileOptions.inline_cap: Option<usize>` into `OptOptions`)
- Same shape in `crates/turing-machine/src/optimizer/mod.rs`,
  `…/optimizer/inline.rs` (cap over `INLINE_MAX_RULES`),
  `…/optimizer/outline.rs` (signature only — reads no cap),
  `…/src/compiler.rs`

**Interfaces:**
- Produces: `CompileOptions.inline_cap: Option<usize>` on both sides
  (default `None` = the shipped constant; a measurement knob, not CLI
  surface — no flag, no completions entry). Task 9 consumes it.

- [ ] **Step 1: Failing test per side** (in `inline.rs` test modules):

```rust
#[test]
fn the_cap_override_admits_a_larger_callee() {
    // A 8-op callee with two call sites: default cap keeps the call;
    // inline_cap: Some(12) inlines it.
}
```

- [ ] **Step 2: Run to verify failure**, then implement the plumbing on
  both sides. Keep `-O0` short-circuit untouched. `cargo check`
  surfaces every `ProgramPassFn` call site to update.

- [ ] **Step 3: Run both crates' full tests** — green, no fixture
  changes (default `None` must be behavior-identical; that IS the
  test).

- [ ] **Step 4: Commit**

```bash
git add crates/post-machine/src crates/turing-machine/src
git commit -m "feat: inline_cap override on OptOptions/CompileOptions for the sweep"
```

---

### Task 9: the sweep — measure, choose, bake

**Files:**
- Create: `crates/post-machine/tests/sweep.rs`,
  `crates/turing-machine/tests/sweep.rs` (both `#[ignore]`d)
- Modify (after measuring): `INLINE_MAX_OPS` in
  `crates/post-machine/src/optimizer/inline.rs`, `INLINE_MAX_RULES` in
  `crates/turing-machine/src/optimizer/inline.rs` (only if the rule
  picks a new value), `docs/pmt/optimizer.md`, `docs/tmt/optimizer.md`
  (the table + chosen constants — folded into Task 10's doc pass if
  values are unchanged)

**Interfaces:**
- Consumes: `CompileOptions.inline_cap` (Task 8); PM corpus = stdlib
  (`mtc_post_machine::stdlib`) + `tests/golden/*.pmc` on their
  committed inputs (crib `tests/golden_programs.rs`); TM corpus = the
  flagship `.tmc` + both stdlib twins (crib `tests/golden_programs.rs`
  TM-side and `tests/stdlib_twins.rs`).

- [ ] **Step 1: Write the harnesses.** Each prints one table row per
  cap ∈ {6, 12, 24}: total steps across the corpus runs, total image
  bytes, total instruction count. Non-terminating corpus members
  (`test1.pmc`) run under the same fixed `max_steps` at every cap so
  their step contribution is constant.

Run: `cargo test -p mtc-post-machine --test sweep -- --ignored --nocapture`
and the TM twin.

- [ ] **Step 2: Apply the decision rule** (spec §9, ruled R3): best
  corpus-wide step total, image growth ≤ 5% over the cap-6 baseline,
  ties toward the smaller cap. Record the tables and the choice in the
  round ledger. If a new cap wins, change the constant(s) and re-run
  BOTH full crate suites plus the spot-check at cap 24
  (`inline_cap: Some(24)` through the equivalence harness's build fn on
  a handful of its programs — add that as a `#[test]` if a constant
  changed, or note "unchanged" in the ledger).

- [ ] **Step 3: Commit**

```bash
git add crates/*/tests/sweep.rs <constant files if changed>
git commit -m "feat: inline-cap sweep harnesses; caps chosen by the ruled decision rule"
```

---

### Task 10: docs, floors, and the round close

**Files:**
- Modify: `docs/pmt/optimizer.md` (move-elim + tail-sink entries, each
  with brk/volatile/column lines; the sweep table + constants),
  `docs/tmt/optimizer.md` (dispatch-target threading, row subsumption,
  sweep table), `docs/pmt/language.md` / `docs/tmt/language.md` ONLY if
  they enumerate pass names (grep first; follow what exists)
- Verify-only: everything else.

- [ ] **Step 1: Write the doc entries.** Match each page's existing
  per-pass entry format exactly (read two neighboring entries first).
  Content per pass comes from the spec §§5–9 — substance in prose, no
  issue refs, no spec refs.

- [ ] **Step 2: Round floors.**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --stat master@{u} -- crates/core   # MUST be empty (zero-core-diff invariant)
```

Also confirm the `-O0`/`--fno-everything` byte floors: both crates'
existing floor tests ran green in the suites above — name-check that
they exist and ran (`grep -rn "byte-identical" crates/*/tests/ | head`)
and record the confirmation in the ledger.

- [ ] **Step 3: Ledger + tracker.** Update the round ledger with the
  measurement tables, fixture regens, and mutation-check log. On the
  merge commit (maintainer step): close #32 with the measured table and
  #76 with a summary comment; file the deferred dead-write-elimination
  issue (liveness/read-set substrate) linking the volatile/footprint
  spec's §10 trigger — per spec §12.

- [ ] **Step 4: Final commit**

```bash
git add docs/
git commit -m "docs: optimizer pages - the motion/value round's passes and tuned caps"
```

---

## Self-Review Notes

- Spec coverage: §5 → Tasks 1–4; §6 → Task 5; §7 → Task 6; §8 → Task 7;
  §9 → Tasks 8–9; §§10–12 → registrations inside Tasks 6–7 + Task 10.
  R1 gate → Task 4. Mutation review → per-task steps. Zero-core-diff →
  Task 10 step 2.
- Type consistency: `direct` (Tasks 1/2/3/5), `emitted_row_order`
  (Task 5), `inline_cap` (Tasks 8/9), pass names `"move-elim"` /
  `"tail-sink"` (Tasks 6/7) — single spellings throughout.
- Known executor look-ups (anchored, not placeholders): the TM
  `CompileOutput` asm-text field name (compiler.rs:4011 shows it), the
  `.tmc` fixture surface syntax (crib from `tests/opt_equivalence.rs`),
  `listing_line`'s return shape (its definition), each test file's
  existing helper names.
