# Plan 6a — Optimizer Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `-O1` becomes real: a pass driver over the CFG IR running check-fold, jump-threading, dce, cell-state (the historic redundant mark/unmark elimination, generalized), and branch-fold — under the spec §8 equivalence contract, with per-pass opt-outs, per-stage IR snapshots, and an equivalence test harness that runs `-O0` vs `-O1` builds on the same tapes.

**Architecture:** `crates/post-machine/src/optimizer/` — one module per pass (spec §10), a driver that loops a fixed pipeline to a change-fixpoint (round-capped), and a shared dataflow module feeding cell-state and branch-fold. Every pass is `fn(&mut IrFunction) -> u32` (change count). The Plan 5 acceptance item is enforced structurally: `ir::validate_function` checks the closed-terminator-targets invariant after every pass in debug builds.

**Scope split (controller decision, Plan 2a/2b precedent):** this is 6a. Plan 6b delivers the remaining §8 passes — inline, tail-merge, tail-call — which need core changes (symbol-operand jumps in assembler/linker/disassembler). `-O1` here = the 6a passes; 6b extends the same pipeline.

**Tech Stack:** Rust edition 2024, no new dependencies.

## The MF-coupling soundness argument (binding design, novel in this plan)

PM-1 semantics: **every tape instruction latches MF from the cell at the (new) head position** (`lft`/`rgt` latch the destination cell, `wr` the written value); nothing else latches MF or moves the head; at machine reset MF = 0 regardless of the tape.

**Coupling invariant:** after at least one tape instruction has executed, `MF == (cell_at_head == 1)` — and it stays true across jumps, `call`/`ret` (a callee either executes tape ops, re-establishing it at its final head position, or executes none and disturbs neither MF nor head), `ent`, and `brk`.

**The trap:** before ANY tape instruction has executed, MF is the reset value 0, decoupled from the tape. A `check` on that path branches on 0, not on the cell. Any analysis that treats a `check` edge as evidence about the cell is UNSOUND on such paths.

Therefore the dataflow lattice must track coupledness explicitly:

```
Fact ::= Uncoupled            -- coupling not provable on some path here
       | Coupled(Option<u32>) -- invariant holds; symbol under head if known
```

- Function entry: `Uncoupled` (even `main` — reset MF; and callees can't know their caller's history).
- `lft`/`rgt` → `Coupled(None)`; `wr i` → `Coupled(Some(i))`.
- `call` → `Coupled(None)` if already coupled (see invariant), else `Uncoupled`.
- `brk` → degrade value knowledge only: `Coupled(_)` → `Coupled(None)`, `Uncoupled` stays (brk is an observability barrier — no elimination across it — but does not disturb machine state).
- `check` edge refinement (marked edge → `Coupled(Some(1))`, blank edge → `Coupled(Some(0))` — sound because the PM-1 alphabet has exactly two symbols) applies **only when the fact at the check is `Coupled(_)`**; from `Uncoupled`, edges get `Uncoupled`.
- Merge: `Uncoupled ∨ x = Uncoupled`; `Coupled(a) ∨ Coupled(b) = Coupled(if a == b { a } else { None })`.

cell-state drops `wr i` only under `Coupled(Some(i))` (cell unchanged AND the MF the drop skips equals the MF already latched — both are `(i == 1)` by the invariant). branch-fold folds a `check` only under `Coupled(Some(sym))`. An equivalence test pins the trap case: a program whose first instruction is `check` must behave identically at `-O0` and `-O1` (both take the reset-MF branch).

## Global Constraints

- **Equivalence contract (spec §8), binding on every pass:** preserve final tape contents, termination kind (`stp`/`hlt`/which trap — except step/tact-limit traps, since step counts are explicitly unobservable), and every MF-dependent branch decision. Un-stripped `brk` is an observability barrier: no motion or elimination of tape effects across it (the optimizer runs BEFORE codegen's `--strip-debugger`, so barriers hold even in stripped builds — a documented v1 conservatism).
- **Closed-terminator-targets invariant (Plan 5 acceptance item):** after every pass, every terminator target names an existing block and `blocks[0]` is the entry. Enforced by `ir::validate_function` + a debug-build check in the driver after each pass application.
- `@call` is an opaque barrier for value facts (head, cells clobbered); the coupling rule above is the only knowledge that survives it.
- Pass names (for `--fno-<name>` and reports), exactly: `check-fold`, `jump-threading`, `cell-state`, `branch-fold`, `dce`. Pipeline order per round: that same order. Fixpoint: loop rounds until a round makes zero changes, hard cap `MAX_ROUNDS = 10`.
- `-O0` = optimizer entirely skipped (bit-identical output to Plan 5). Lowering warnings (unreachable code) are still reported even when dce deletes the blocks.
- Library code never prints; optimizer results surface as `OptReport` inside `CompileReport` (the LinkReport pattern).
- Gates per task: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. Commits per task, path-scoped, never push, no attribution footers.
- If plan code contradicts the spec or does not compile as written: report BLOCKED with details; do not improvise.

## File Structure

- Modify: `crates/post-machine/src/ir.rs` (add `validate_function`), `crates/post-machine/src/compiler.rs` (options/report/output/pipeline), `crates/post-machine/src/lib.rs`, existing test literals in `compiler.rs` tests + `tests/compile_programs.rs` + `examples/compile_and_run.rs`; plus the user-approved spec amendments in Task 1: `crates/core/src/linker/mod.rs` (MapFile loses `alphabet`), `crates/post-machine/src/arch/mod.rs` (`DEFAULT_GLYPHS`), `crates/post-machine/src/asm/mod.rs` (link wrapper pass-through), `tests/link_programs.rs`, `tests/pm1_programs.rs` (`.pmb` → `.pmt`).
- Create: `crates/post-machine/src/optimizer/mod.rs` (driver), `check_fold.rs`, `jump_threading.rs`, `dce.rs`, `dataflow.rs`, `cell_state.rs`, `branch_fold.rs`; `crates/post-machine/tests/opt_equivalence.rs`.

---

### Task 1: Driver plumbing, validate_function, options surface + glyph ownership & `.pmt` rename

**Files:**
- Modify: `crates/post-machine/src/ir.rs`, `crates/post-machine/src/compiler.rs`, `crates/post-machine/src/lib.rs`, `crates/post-machine/tests/compile_programs.rs`, `crates/post-machine/examples/compile_and_run.rs`
- Modify (user-approved spec amendments, spec text already updated): `crates/core/src/linker/mod.rs`, `crates/post-machine/src/arch/mod.rs`, `crates/post-machine/src/asm/mod.rs`, `crates/post-machine/tests/link_programs.rs`, `crates/post-machine/tests/pm1_programs.rs`
- Create: `crates/post-machine/src/optimizer/mod.rs`

**Interfaces:**
- Produces: `ir::validate_function(&IrFunction) -> Result<(), String>`; `optimizer::{OptLevel, OptOptions, PassChange, OptReport, optimize, register-style PIPELINE const}`; extended `CompileOptions { opt_level, disabled_passes, capture_ir }`, `CompileReport { warnings, opt }`, `CompileOutput { …, ir_snapshots }`. Tasks 2–5 each append one `(name, fn)` entry to `PIPELINE`.
- Consumes: Plan 5's `IrProgram`/`IrFunction`.

- [ ] **Step 1: Add `validate_function` to `crates/post-machine/src/ir.rs`** (below `lower`, above tests):

```rust
/// Structural invariants every optimizer pass must preserve (the Plan 5
/// final-review acceptance item): non-empty function, unique block ids,
/// every terminator target resolvable. `blocks[0]` remains the entry by
/// position; passes may delete or retarget but never leave a dangling
/// terminator.
pub fn validate_function(f: &IrFunction) -> Result<(), String> {
    if f.blocks.is_empty() {
        return Err(format!("{}: function has no blocks", f.name));
    }
    let mut ids = HashSet::new();
    for b in &f.blocks {
        if !ids.insert(b.id) {
            return Err(format!("{}: duplicate block id {}", f.name, b.id));
        }
    }
    for b in &f.blocks {
        let check = |t: u32| -> Result<(), String> {
            if ids.contains(&t) {
                Ok(())
            } else {
                Err(format!(
                    "{}: block {} terminator targets missing block {}",
                    f.name, b.id, t
                ))
            }
        };
        match b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => check(to)?,
            IrTerm::Check { marked, blank } => {
                check(marked)?;
                check(blank)?;
            }
            IrTerm::Return | IrTerm::Halt => {}
        }
    }
    Ok(())
}
```

(`HashSet` is already imported in ir.rs.)

- [ ] **Step 2: Create `crates/post-machine/src/optimizer/mod.rs`:**

```rust
//! `-O1` pass driver (spec §8). One module per pass; each pass is
//! `fn(&mut IrFunction) -> u32` returning its change count and MUST
//! preserve the equivalence contract and the closed-terminator-targets
//! invariant (checked in debug builds after every application).

use std::collections::HashSet;

use crate::ir::{IrFunction, IrProgram};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    #[default]
    O0,
    O1,
}

#[derive(Debug, Clone, Default)]
pub struct OptOptions {
    pub level: OptLevel,
    /// Pass names to skip (`--fno-<pass>`).
    pub disabled: HashSet<String>,
    /// Capture an IR snapshot after each pass that changed something.
    pub capture: bool,
}

/// One pass's effect on one function in one round (`pmt -v` material).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassChange {
    pub pass: &'static str,
    pub function: String,
    pub changes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OptReport {
    pub rounds: u32,
    pub changes: Vec<PassChange>,
}

type PassFn = fn(&mut IrFunction) -> u32;

/// Fixed pipeline, in per-round application order. Tasks 2-5 extend it.
const PIPELINE: &[(&str, PassFn)] = &[];

const MAX_ROUNDS: u32 = 10;

/// Run the enabled pipeline to a change-fixpoint (round-capped). `-O0`
/// returns immediately: Plan 5 output stays bit-identical.
pub fn optimize(
    ir: &mut IrProgram,
    options: &OptOptions,
    snapshots: &mut Vec<(String, IrProgram)>,
) -> OptReport {
    let mut report = OptReport::default();
    if options.level == OptLevel::O0 {
        return report;
    }
    loop {
        report.rounds += 1;
        let mut round_changes = 0u32;
        for (name, pass) in PIPELINE {
            if options.disabled.contains(*name) {
                continue;
            }
            let mut pass_total = 0u32;
            for f in &mut ir.functions {
                let n = pass(f);
                #[cfg(debug_assertions)]
                if let Err(e) = crate::ir::validate_function(f) {
                    panic!("pass `{name}` broke IR invariants: {e}");
                }
                if n > 0 {
                    report.changes.push(PassChange {
                        pass: name,
                        function: f.name.clone(),
                        changes: n,
                    });
                }
                pass_total += n;
            }
            if options.capture && pass_total > 0 {
                snapshots.push((format!("after:{name}"), ir.clone()));
            }
            round_changes += pass_total;
        }
        if round_changes == 0 || report.rounds >= MAX_ROUNDS {
            return report;
        }
    }
}
```

- [ ] **Step 3: Extend the compiler surface.** In `crates/post-machine/src/compiler.rs`:

(a) Add to the imports: `use crate::optimizer::{OptLevel, OptOptions, OptReport, optimize};`

(b) Replace the `CompileOptions` definition (note: no longer `Copy` — it now holds a `Vec`):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompileOptions {
    /// `-g`: record label/line debug info in the object, with lines
    /// remapped to `.pmc` sources.
    pub debug_info: bool,
    /// `--strip-debugger`: drop `brk` at codegen (spec §10). The
    /// optimizer runs BEFORE stripping, so `brk` barriers always hold.
    pub strip_debugger: bool,
    /// `-O0` (default) or `-O1` (spec §8 passes, 6a subset).
    pub opt_level: OptLevel,
    /// Pass names to disable (`--fno-<pass>`), e.g. `"cell-state"`.
    pub disabled_passes: Vec<String>,
    /// Capture per-stage IR snapshots (`--emit-ir=<stage>` backing):
    /// `"lowered"`, `"after:<pass>"` per changing pass, `"final"`.
    pub capture_ir: bool,
}
```

(c) `CompileReport` gains the optimizer report:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileReport {
    pub warnings: Vec<Warning>,
    pub opt: OptReport,
}
```

(d) `CompileOutput` gains snapshots (field appended after `ir`):

```rust
    /// Per-stage IR snapshots when `capture_ir` was set; empty otherwise.
    pub ir_snapshots: Vec<(String, IrProgram)>,
```

Update the `ir` field's doc comment to: `/// The FINAL CFG (post-optimizer at -O1; the lowered CFG at -O0).`

(e) In `compile()`, between `lower` and `emit_program`:

```rust
    let (mut ir, warnings) = crate::ir::lower(&program)?;
    let mut ir_snapshots = Vec::new();
    if options.capture_ir {
        ir_snapshots.push(("lowered".to_string(), ir.clone()));
    }
    let opt = optimize(
        &mut ir,
        &OptOptions {
            level: options.opt_level,
            disabled: options.disabled_passes.iter().cloned().collect(),
            capture: options.capture_ir,
        },
        &mut ir_snapshots,
    );
    if options.capture_ir {
        ir_snapshots.push(("final".to_string(), ir.clone()));
    }
```

and thread the new fields into the return value: `report: CompileReport { warnings, opt }`, plus `ir_snapshots` in `CompileOutput`.

- [ ] **Step 4: Fix all `CompileOptions` struct literals** (the two new fields break exhaustive literals). Change every `CompileOptions { debug_info: X, strip_debugger: Y }` to `CompileOptions { debug_info: X, strip_debugger: Y, ..Default::default() }` in:
  - `crates/post-machine/src/compiler.rs` tests (`debug_lines_speak_pmc_not_pma`, `strip_debugger_reaches_the_bytes` — 3 literals),
  - `crates/post-machine/tests/compile_programs.rs` (`debug_build_maps_executable_offsets_to_pmc_lines` — 1 literal),
  - `crates/post-machine/examples/compile_and_run.rs` (1 literal; the file is untracked-but-present — edit it, commit it with the rest).
  Also fix any `CompileReport { warnings }` literal in tests to include `opt` (compare `report.warnings` instead of whole-report equality where simpler).

- [ ] **Step 5: Register + re-export.** `lib.rs`: add `pub mod optimizer;` and extend the root re-export with `OptLevel`.

- [ ] **Step 6: Remove glyphs from the map** (spec §6.3/§10 amendment: glyphs live only on the tape side; the map is pure code metadata).

(a) `crates/core/src/linker/mod.rs`: delete `pub alphabet: Vec<String>` from `MapFile`; drop the `alphabet:` field from every construction site in `link()` and in the module's tests; the map-JSON test that asserted `json.contains("\"alphabet\"")` now asserts `!json.contains("\"alphabet\"")`.

(b) `crates/post-machine/src/asm/mod.rs`: the PM link wrapper no longer stamps glyphs — its body becomes a direct pass-through:

```rust
pub fn link(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    options: mtc_core::linker::LinkOptions,
) -> Result<mtc_core::linker::LinkOutput, mtc_core::linker::LinkError> {
    mtc_core::linker::link(&pm1_syntax(), objects, libraries, options)
}
```

(c) `crates/post-machine/tests/link_programs.rs`: delete the `out.map.alphabet` assertion in `map_names_the_functions`.

(d) The default rendering convention moves where it belongs — the arch module. In `crates/post-machine/src/arch/mod.rs` add (with a one-line test asserting the values):

```rust
/// Default rendering glyphs (index 0 = blank, 1 = mark) for tooling with
/// no tape at hand; a loaded `.pmt`'s own alphabet always wins (spec §6.3).
pub const DEFAULT_GLYPHS: [&str; 2] = [" ", "*"];
```

- [ ] **Step 7: `.pmb` → `.pmt` rename** (belt→tape consistency; spec already amended by the controller; the container magic stays `MT 0x01` — zero binary change, this is comments and test names only). In `crates/post-machine/tests/pm1_programs.rs`: rename `fn pmb_in_run_pmb_out` → `fn pmt_in_run_pmt_out` and fix its `.pmb` comment. Then verify: `grep -rn "pmb" crates/` returns nothing.

- [ ] **Step 8: Task-1 tests** (append to `compiler.rs` tests):

```rust
    #[test]
    fn o1_with_empty_pipeline_is_identity_and_reports_one_round() {
        let src = "main() { right; mark; }";
        let o0 = compile(src, CompileOptions::default()).unwrap();
        let o1 = compile(
            src,
            CompileOptions {
                opt_level: crate::optimizer::OptLevel::O1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(o0.object, o1.object);
        assert_eq!(o1.report.opt.rounds, 1);
        assert!(o1.report.opt.changes.is_empty());
    }

    #[test]
    fn capture_ir_yields_lowered_and_final() {
        let out = compile(
            "main() { mark; }",
            CompileOptions {
                capture_ir: true,
                ..Default::default()
            },
        )
        .unwrap();
        let stages: Vec<&str> = out.ir_snapshots.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(stages, vec!["lowered", "final"]);
        assert_eq!(out.ir_snapshots[0].1, out.ir_snapshots[1].1); // -O0: identical
    }
```

and to `ir.rs` tests:

```rust
    #[test]
    fn validate_function_accepts_lowered_ir_and_rejects_dangling_targets() {
        let (ir, _) = ir_of("f() { 1: right; check(1, !); }");
        for f in &ir.functions {
            validate_function(f).unwrap();
        }
        let mut broken = ir.functions[0].clone();
        broken.blocks[0].term = IrTerm::Goto { to: 99 };
        assert!(validate_function(&broken).is_err());
    }
```

- [ ] **Step 9: Gates, then commit.**

```bash
git add crates/post-machine/src/ir.rs crates/post-machine/src/compiler.rs crates/post-machine/src/lib.rs crates/post-machine/src/optimizer crates/post-machine/tests/compile_programs.rs crates/post-machine/examples/compile_and_run.rs crates/core/src/linker/mod.rs crates/post-machine/src/arch/mod.rs crates/post-machine/src/asm/mod.rs crates/post-machine/tests/link_programs.rs crates/post-machine/tests/pm1_programs.rs
git commit -m "feat: optimizer driver plumbing; map sheds glyphs (tape-side only); .pmb -> .pmt"
```

---

### Task 2: Structural passes — check-fold, jump-threading, dce

**Files:**
- Create: `crates/post-machine/src/optimizer/check_fold.rs`, `jump_threading.rs`, `dce.rs`; `crates/post-machine/tests/opt_equivalence.rs`
- Modify: `crates/post-machine/src/optimizer/mod.rs` (declare modules, fill `PIPELINE`)

**Interfaces:**
- Produces: `check_fold::run`, `jump_threading::run`, `dce::run` (all `fn(&mut IrFunction) -> u32`); the shared equivalence-harness helpers in `tests/opt_equivalence.rs` that Tasks 4-6 extend.
- `PIPELINE` becomes (cell-state/branch-fold slots arrive in Tasks 4-5):

```rust
const PIPELINE: &[(&str, PassFn)] = &[
    ("check-fold", check_fold::run),
    ("jump-threading", jump_threading::run),
    ("dce", dce::run),
];
```

- [ ] **Step 1: `check_fold.rs`** — spec §8 pass 1. (The one-arm-fall-through specialization is already structural in codegen's adjacency selection; the IR-level fold is the `check(N, N)` case.)

```rust
//! check-fold (spec §8 pass 1): a check with identical arms decides
//! nothing — replace with an unconditional goto. The single-arm jm/jnm
//! specialization is codegen's adjacency selection, not an IR rewrite.

use crate::ir::{IrFunction, IrTerm};

pub fn run(f: &mut IrFunction) -> u32 {
    let mut changes = 0;
    for b in &mut f.blocks {
        if let IrTerm::Check { marked, blank } = b.term
            && marked == blank
        {
            b.term = IrTerm::Goto { to: marked };
            changes += 1;
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrTerm, lower};
    use crate::lexer::lex;
    use crate::parser::parse;

    #[test]
    fn identical_arms_fold_to_goto() {
        let (mut ir, _) =
            lower(&parse(&lex("f() { 1: check(1, 1); }").unwrap()).unwrap()).unwrap();
        assert_eq!(run(&mut ir.functions[0]), 1);
        assert_eq!(ir.functions[0].blocks[0].term, IrTerm::Goto { to: 0 });
    }

    #[test]
    fn distinct_arms_untouched() {
        let (mut ir, _) =
            lower(&parse(&lex("f() { 1: check(1, !); }").unwrap()).unwrap()).unwrap();
        assert_eq!(run(&mut ir.functions[0]), 0);
    }
}
```

- [ ] **Step 2: `jump_threading.rs`** — spec §8 pass 2.

```rust
//! jump-threading (spec §8 pass 2): a jump to an EMPTY block that only
//! jumps onward retargets to the final destination. Chains collapse in
//! one application; a cycle of empty forwarders is a deliberate infinite
//! loop (`1: goto 1;`) and is preserved untouched.

use std::collections::{HashMap, HashSet};

use crate::ir::{IrBlock, IrFunction, IrTerm};

fn forwards_to(b: &IrBlock) -> Option<u32> {
    if b.ops.is_empty()
        && let IrTerm::Goto { to } | IrTerm::FallThrough { to } = b.term
    {
        Some(to)
    } else {
        None
    }
}

pub fn run(f: &mut IrFunction) -> u32 {
    let forward: HashMap<u32, u32> = f
        .blocks
        .iter()
        .filter_map(|b| forwards_to(b).map(|t| (b.id, t)))
        .collect();
    let resolve = |start: u32| -> u32 {
        let mut seen = HashSet::new();
        let mut cur = start;
        while let Some(&next) = forward.get(&cur) {
            if !seen.insert(cur) {
                return start; // cycle: preserve the loop as written
            }
            cur = next;
        }
        cur
    };

    let mut changes = 0;
    for b in &mut f.blocks {
        let mut retarget = |t: &mut u32| {
            let new = resolve(*t);
            if new != *t {
                *t = new;
                changes += 1;
            }
        };
        match &mut b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => retarget(to),
            IrTerm::Check { marked, blank } => {
                retarget(marked);
                retarget(blank);
            }
            IrTerm::Return | IrTerm::Halt => {}
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn ir_of(src: &str) -> crate::ir::IrProgram {
        lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0
    }

    #[test]
    fn goto_chain_collapses_to_final_target() {
        // 1 -> 2 -> 3 -> mark: blocks 0(goto 1), 1(goto 2), 2(mark).
        let mut ir = ir_of("f() { goto 1; 1: goto 2; 2: goto 3; 3: mark; }");
        let f = &mut ir.functions[0];
        assert!(run(f) > 0);
        // Entry now targets the mark block directly.
        assert_eq!(f.blocks[0].term, IrTerm::Goto { to: 3 });
    }

    #[test]
    fn empty_self_loop_is_preserved() {
        let mut ir = ir_of("f() { 1: goto 1; }");
        assert_eq!(run(&mut ir.functions[0]), 0);
        assert_eq!(ir.functions[0].blocks[0].term, IrTerm::Goto { to: 0 });
    }

    #[test]
    fn blocks_with_ops_are_not_threaded_through() {
        let mut ir = ir_of("f() { goto 1; 1: mark(2); 2: left; }");
        assert_eq!(run(&mut ir.functions[0]), 0);
    }
}
```

- [ ] **Step 3: `dce.rs`** — spec §8 pass 3 (2012 warned; this deletes — the lowering warning still fires, compile keeps reporting it).

```rust
//! dce (spec §8 pass 3): delete blocks unreachable from the entry.
//! Reachability-only deletion cannot dangle a reachable terminator, so
//! the closed-targets invariant is preserved by construction.

use std::collections::HashSet;

use crate::ir::{IrFunction, IrTerm};

pub fn run(f: &mut IrFunction) -> u32 {
    let index: std::collections::HashMap<u32, usize> =
        f.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let mut seen = HashSet::new();
    let mut work = vec![f.blocks[0].id];
    while let Some(id) = work.pop() {
        if !seen.insert(id) {
            continue;
        }
        match f.blocks[index[&id]].term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => work.push(to),
            IrTerm::Check { marked, blank } => {
                work.push(marked);
                work.push(blank);
            }
            IrTerm::Return | IrTerm::Halt => {}
        }
    }
    let before = f.blocks.len();
    f.blocks.retain(|b| seen.contains(&b.id));
    (before - f.blocks.len()) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    #[test]
    fn unreachable_block_is_deleted_and_entry_survives() {
        let (mut ir, warnings) =
            lower(&parse(&lex("f() { goto 1; right; 1: left; }").unwrap()).unwrap()).unwrap();
        assert_eq!(warnings.len(), 1); // lowering still warns
        let f = &mut ir.functions[0];
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(run(f), 1);
        assert_eq!(f.blocks.len(), 2);
        crate::ir::validate_function(f).unwrap();
    }
}
```

- [ ] **Step 4: `tests/opt_equivalence.rs`** — the spec §11 harness all later tasks extend:

```rust
//! Equivalence harness (spec §11): every optimizer pass is tested by
//! running -O0 and -O1 builds of the same program on the same tapes and
//! comparing observables — outcome kind, final tape, final head.

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, RunLimits, RunOptions};
use mtc_post_machine::asm::link;
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;

fn build(src: &str, level: OptLevel) -> mtc_core::formats::executable::Executable {
    let out = compile(
        src,
        CompileOptions {
            opt_level: level,
            ..Default::default()
        },
    )
    .expect("compiles");
    link(&[out.object], &[], LinkOptions::default())
        .expect("links")
        .executable
}

fn run_tape(
    exe: &mtc_core::formats::executable::Executable,
    cells: &[bool],
    head: i64,
) -> (mtc_core::vm::Outcome, Vec<i64>, i64) {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells(cells.iter().copied(), 0, head);
    let options = RunOptions {
        limits: RunLimits {
            max_steps: Some(10_000),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = machine.run(&mut tape, options);
    (result.outcome, tape.marked_cells(), tape.head())
}

/// Assert -O0 and -O1 agree on every tape; return (o0_len, o1_len).
pub fn assert_equivalent(src: &str, tapes: &[(&[bool], i64)]) -> (usize, usize) {
    let o0 = build(src, OptLevel::O0);
    let o1 = build(src, OptLevel::O1);
    for (cells, head) in tapes {
        let r0 = run_tape(&o0, cells, *head);
        let r1 = run_tape(&o1, cells, *head);
        assert_eq!(r0, r1, "observables diverged on tape {cells:?}/{head}: {src}");
    }
    (o0.code.len(), o1.code.len())
}

const TAPES: &[(&[bool], i64)] = &[
    (&[false], 0),
    (&[true], 0),
    (&[true, true, true], 0),
    (&[false, true, true], 0),
    (&[true, false, true], 1),
];

#[test]
fn check_fold_shrinks_and_preserves() {
    let src = "main() { right; 1: check(1, 1); 1000: mark; }";
    // check(1,1) -> goto 1 -> infinite loop? No: marked or blank both go
    // to label 1 = the check block itself... use a forward target:
    let src = "main() { right; check(5, 5); 5: mark; }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(o1 < o0, "fold must shrink: {o0} -> {o1}");
}

#[test]
fn jump_threading_shrinks_and_preserves() {
    let src = "main() { goto 1; 1: goto 2; 2: goto 3; 3: mark; }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(o1 < o0);
}

#[test]
fn dce_removes_dead_code_bytes() {
    let src = "main() { goto 9; right; left; right; left; 9: mark; }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(o1 < o0);
}

#[test]
fn empty_infinite_loop_still_loops_at_o1() {
    let src = "main() { 1: goto 1; }";
    let o1 = build(src, OptLevel::O1);
    let (outcome, _, _) = run_tape(&o1, &[true], 0);
    assert!(
        matches!(outcome, mtc_core::vm::Outcome::Trapped(mtc_core::vm::Trap::StepLimit)),
        "the loop must survive optimization: {outcome:?}"
    );
}
```

Note for the implementer on `check_fold_shrinks_and_preserves`: the first `src` line is shadowed dead code left as a documented temptation — DELETE the first binding and keep only the `check(5, 5)` version (the plan shows both to explain why: folding `check(1,1)` whose label is its own block would produce a self-loop `goto`, a legitimate but non-shrinking program). Final test body has ONE `src`.

- [ ] **Step 5: Wire `PIPELINE`** in `optimizer/mod.rs`: add `pub mod check_fold; pub mod dce; pub mod jump_threading;` and replace the empty `PIPELINE` with the three-entry version shown in Interfaces. The Task-1 test `o1_with_empty_pipeline_is_identity_and_reports_one_round` still passes (that program has nothing foldable) — rename it to `o1_on_unoptimizable_program_is_identity`.

- [ ] **Step 6: Gates, then commit.**

```bash
git add crates/post-machine/src/optimizer crates/post-machine/src/compiler.rs crates/post-machine/tests/opt_equivalence.rs
git commit -m "feat(post-machine): structural optimizer passes — check-fold, jump-threading, dce + equivalence harness"
```

---

### Task 3: The coupling-aware dataflow analysis

**Files:**
- Create: `crates/post-machine/src/optimizer/dataflow.rs`
- Modify: `crates/post-machine/src/optimizer/mod.rs` (declare module only — no PIPELINE change; this task is analysis, not transformation)

**Interfaces:**
- Produces: `dataflow::{Fact, transfer_op, block_entry_facts}` exactly as below; Tasks 4 and 5 consume them.

- [ ] **Step 1: `dataflow.rs`:**

```rust
//! Shared forward dataflow for cell-state and branch-fold (spec §8).
//!
//! # The MF-coupling invariant (soundness backbone — read before editing)
//!
//! Every PM-1 tape instruction latches MF from the cell at the resulting
//! head position (`lft`/`rgt` the destination cell, `wr` the written
//! value); nothing else latches MF or moves the head. Hence AFTER at
//! least one tape instruction, `MF == (cell_at_head == 1)` — and this
//! survives jumps, `ent`, `brk`, and whole `call`s (a callee either
//! re-establishes it with its own tape ops or disturbs neither MF nor
//! head). BEFORE any tape instruction executes, MF is the reset value 0,
//! DECOUPLED from the tape: a `check` on such a path branches on 0, not
//! on the cell. The lattice therefore tracks coupledness explicitly, and
//! check-edge refinement applies only on provably coupled paths.

use std::collections::HashMap;

use crate::ir::{IrFunction, IrOp, IrTerm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact {
    /// Some path may reach here with no tape instruction executed yet:
    /// MF may still be the reset value. No cell knowledge, no folding.
    Uncoupled,
    /// The coupling invariant holds; the symbol under the head, if known.
    Coupled(Option<u32>),
}

impl Fact {
    pub fn merge(self, other: Fact) -> Fact {
        match (self, other) {
            (Fact::Coupled(a), Fact::Coupled(b)) => {
                Fact::Coupled(if a == b { a } else { None })
            }
            _ => Fact::Uncoupled,
        }
    }

    /// The symbol under the head, when provable.
    pub fn cell(self) -> Option<u32> {
        match self {
            Fact::Coupled(c) => c,
            Fact::Uncoupled => None,
        }
    }
}

pub fn transfer_op(fact: Fact, op: &IrOp) -> Fact {
    match op {
        // Moves couple MF to the (unknown) destination cell.
        IrOp::Lft { .. } | IrOp::Rgt { .. } => Fact::Coupled(None),
        IrOp::Wr { index, .. } => Fact::Coupled(Some(*index)),
        // Opaque: callee clobbers head/cells, but preserves coupledness
        // (see module docs) — value knowledge only is lost.
        IrOp::Call { .. } => match fact {
            Fact::Coupled(_) => Fact::Coupled(None),
            Fact::Uncoupled => Fact::Uncoupled,
        },
        // Observability barrier: no fact-based elimination may reach
        // across it, so knowledge degrades; machine state is untouched,
        // so coupledness survives.
        IrOp::Brk { .. } => match fact {
            Fact::Coupled(_) => Fact::Coupled(None),
            Fact::Uncoupled => Fact::Uncoupled,
        },
    }
}

/// Entry fact for every reachable block: worklist to fixpoint. The
/// function entry is `Uncoupled` (reset MF / unknown caller history).
/// Check edges refine (marked → cell 1, blank → cell 0 — sound because
/// the PM-1 alphabet has exactly two symbols) ONLY from coupled paths.
pub fn block_entry_facts(f: &IrFunction) -> HashMap<u32, Fact> {
    let index: HashMap<u32, usize> =
        f.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let mut entry: HashMap<u32, Fact> = HashMap::new();
    let entry_id = f.blocks[0].id;
    entry.insert(entry_id, Fact::Uncoupled);
    let mut work = vec![entry_id];

    while let Some(id) = work.pop() {
        let b = &f.blocks[index[&id]];
        let mut fact = entry[&id];
        for op in &b.ops {
            fact = transfer_op(fact, op);
        }
        let mut push = |target: u32, edge_fact: Fact, work: &mut Vec<u32>| {
            let merged = match entry.get(&target) {
                Some(&old) => old.merge(edge_fact),
                None => edge_fact,
            };
            if entry.get(&target) != Some(&merged) {
                entry.insert(target, merged);
                work.push(target);
            }
        };
        match b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => push(to, fact, &mut work),
            IrTerm::Check { marked, blank } => {
                let (m, bl) = match fact {
                    Fact::Coupled(_) => {
                        (Fact::Coupled(Some(1)), Fact::Coupled(Some(0)))
                    }
                    Fact::Uncoupled => (Fact::Uncoupled, Fact::Uncoupled),
                };
                push(marked, m, &mut work);
                push(blank, bl, &mut work);
            }
            IrTerm::Return | IrTerm::Halt => {}
        }
    }
    entry
}
```

(If the borrow checker rejects the `push` closure capturing `entry` while `work` is passed in, restructure as a plain fn `fn join(entry: &mut HashMap<u32, Fact>, work: &mut Vec<u32>, target: u32, edge_fact: Fact)` — same logic; that is the expected shape if the closure fights.)

- [ ] **Step 2: Tests** (in `dataflow.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn facts_of(src: &str) -> (crate::ir::IrProgram, HashMap<u32, Fact>) {
        let ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        let facts = block_entry_facts(&ir.functions[0]);
        (ir, facts)
    }

    #[test]
    fn entry_is_uncoupled_and_first_check_refines_nothing() {
        // check BEFORE any tape op: reset-MF trap — edges stay Uncoupled.
        let (_, facts) = facts_of("f() { check(1, 2); 1: mark(!); 2: unmark; }");
        assert_eq!(facts[&1], Fact::Uncoupled);
        assert_eq!(facts[&2], Fact::Uncoupled);
    }

    #[test]
    fn tape_op_couples_and_check_edges_refine() {
        // rgt couples; marked edge knows cell 1, blank edge cell 0.
        let (_, facts) = facts_of("f() { right; check(1, 2); 1: mark(!); 2: unmark; }");
        assert_eq!(facts[&1], Fact::Coupled(Some(1)));
        assert_eq!(facts[&2], Fact::Coupled(Some(0)));
    }

    #[test]
    fn wr_yields_exact_knowledge_and_moves_erase_it() {
        let f = Fact::Uncoupled;
        let f = transfer_op(f, &crate::ir::IrOp::Wr { index: 1, line: 1 });
        assert_eq!(f, Fact::Coupled(Some(1)));
        let f = transfer_op(f, &crate::ir::IrOp::Rgt { line: 1 });
        assert_eq!(f, Fact::Coupled(None));
    }

    #[test]
    fn call_and_brk_degrade_but_do_not_uncouple() {
        let coupled = Fact::Coupled(Some(1));
        assert_eq!(
            transfer_op(coupled, &crate::ir::IrOp::Call { name: "g".into(), line: 1 }),
            Fact::Coupled(None)
        );
        assert_eq!(
            transfer_op(coupled, &crate::ir::IrOp::Brk { line: 1 }),
            Fact::Coupled(None)
        );
        assert_eq!(
            transfer_op(Fact::Uncoupled, &crate::ir::IrOp::Call { name: "g".into(), line: 1 }),
            Fact::Uncoupled
        );
    }

    #[test]
    fn merge_disagreement_degrades_to_unknown_value() {
        assert_eq!(
            Fact::Coupled(Some(1)).merge(Fact::Coupled(Some(0))),
            Fact::Coupled(None)
        );
        assert_eq!(Fact::Coupled(Some(1)).merge(Fact::Uncoupled), Fact::Uncoupled);
    }

    #[test]
    fn loop_facts_reach_fixpoint() {
        // goToEnd shape: 1: right; check(1, 2); 2: left;
        let (_, facts) = facts_of("f() { 1: right; check(1, 2); 2: left; }");
        // Block 0 is re-entered from its own marked edge: entry merges
        // Uncoupled (function entry) with Coupled(Some(1)) -> Uncoupled.
        assert_eq!(facts[&0], Fact::Uncoupled);
        // The blank edge is only reachable AFTER rgt -> refined.
        assert_eq!(facts[&1], Fact::Coupled(Some(0)));
    }
}
```

- [ ] **Step 3: Gates, then commit.**

```bash
git add crates/post-machine/src/optimizer
git commit -m "feat(post-machine): coupling-aware dataflow analysis for cell-state and branch-fold"
```

---

### Task 4: cell-state — redundant-write elimination

**Files:**
- Create: `crates/post-machine/src/optimizer/cell_state.rs`
- Modify: `crates/post-machine/src/optimizer/mod.rs` (declare module; `PIPELINE` gains `("cell-state", cell_state::run)` between `jump-threading` and `dce`)
- Modify: `crates/post-machine/tests/opt_equivalence.rs` (new tests)

**Interfaces:** consumes `dataflow::{Fact, transfer_op, block_entry_facts}`; produces `cell_state::run`.

- [ ] **Step 1: `cell_state.rs`:**

```rust
//! cell-state (spec §8 pass 4): the historic redundant mark/unmark
//! elimination, generalized to `wr`. Two rules, both MF-safe by the
//! coupling invariant (see dataflow module docs):
//!
//! 1. Idempotent write: `wr i` when the cell provably holds `i` on a
//!    COUPLED path — the write changes neither the tape nor MF (both
//!    the skipped latch and the current MF equal `i == 1`).
//! 2. Block-local dead store: a `wr` overwritten by a later `wr` in the
//!    same block with nothing in between that could observe the value —
//!    moves make it tape-visible, `call` may read it, `brk` is an
//!    observability barrier, and MF observation only happens at the
//!    terminator (after the last write re-latches).

use crate::ir::{IrFunction, IrOp};
use crate::optimizer::dataflow;

pub fn run(f: &mut IrFunction) -> u32 {
    let entries = dataflow::block_entry_facts(f);
    let mut changes = 0u32;
    for b in &mut f.blocks {
        // Unreachable blocks have no entry fact; they are dce's job.
        let Some(&entry_fact) = entries.get(&b.id) else {
            continue;
        };

        // Rule 1: idempotent writes.
        let mut fact = entry_fact;
        let mut kept: Vec<IrOp> = Vec::with_capacity(b.ops.len());
        for op in std::mem::take(&mut b.ops) {
            if let IrOp::Wr { index, .. } = &op
                && fact.cell() == Some(*index)
            {
                changes += 1;
                continue;
            }
            fact = dataflow::transfer_op(fact, &op);
            kept.push(op);
        }

        // Rule 2: dead stores.
        let mut dead: Vec<usize> = Vec::new();
        let mut pending: Option<usize> = None;
        for (i, op) in kept.iter().enumerate() {
            match op {
                IrOp::Wr { .. } => {
                    if let Some(p) = pending {
                        dead.push(p);
                    }
                    pending = Some(i);
                }
                IrOp::Lft { .. } | IrOp::Rgt { .. } | IrOp::Call { .. } | IrOp::Brk { .. } => {
                    pending = None;
                }
            }
        }
        if !dead.is_empty() {
            changes += dead.len() as u32;
            let mut i = 0usize;
            kept.retain(|_| {
                let drop = dead.contains(&i);
                i += 1;
                !drop
            });
        }
        b.ops = kept;
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrOp, lower};
    use crate::lexer::lex;
    use crate::parser::parse;

    fn opt_fn(src: &str) -> crate::ir::IrFunction {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        while run(&mut ir.functions[0]) > 0 {}
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        ir.functions.remove(0)
    }

    #[test]
    fn double_mark_keeps_one_write() {
        let f = opt_fn("f() { mark; mark; }");
        assert_eq!(f.blocks[0].ops.len(), 1);
    }

    #[test]
    fn overwritten_write_is_a_dead_store() {
        let f = opt_fn("f() { mark, unmark; }");
        assert_eq!(f.blocks[0].ops, vec![IrOp::Wr { index: 0, line: 1 }]);
    }

    #[test]
    fn check_arm_knowledge_kills_the_confirming_write() {
        // On the marked edge the cell provably holds 1 — `mark` is a no-op.
        let f = opt_fn("f() { right; check(1, 2); 1: mark(!); 2: unmark; }");
        let marked_block = f.blocks.iter().find(|b| b.labels == vec![1]).unwrap();
        assert!(marked_block.ops.is_empty());
    }

    #[test]
    fn moves_calls_and_brk_protect_writes() {
        let f = opt_fn("f() { mark; right; }");
        assert_eq!(f.blocks[0].ops.len(), 2); // move makes the value visible
        let f = opt_fn("f() { mark; debugger; mark; }");
        assert_eq!(f.blocks[0].ops.len(), 3); // barrier: nothing dropped
        let f = opt_fn("f() { mark; @g(); mark; }");
        assert_eq!(f.blocks[0].ops.len(), 3); // call may observe/clobber
    }

    #[test]
    fn uncoupled_entry_never_licenses_a_drop() {
        // No tape op before `mark`: cell unknown, write must stay.
        let f = opt_fn("f() { mark; }");
        assert_eq!(f.blocks[0].ops.len(), 1);
    }
}
```

- [ ] **Step 2: Equivalence tests** (append to `tests/opt_equivalence.rs`):

```rust
#[test]
fn cell_state_shrinks_and_preserves() {
    let (o0, o1) = assert_equivalent("main() { mark; mark; right; mark, unmark; }", TAPES);
    assert!(o1 < o0);
}

#[test]
fn brk_barrier_blocks_elimination() {
    let (o0, o1) = assert_equivalent("main() { mark; debugger; mark; }", TAPES);
    assert_eq!(o0, o1, "no elimination across an observability barrier");
}
```

- [ ] **Step 3: Gates, then commit.**

```bash
git add crates/post-machine/src/optimizer crates/post-machine/tests/opt_equivalence.rs
git commit -m "feat(post-machine): cell-state pass — idempotent-write and dead-store elimination under the coupling invariant"
```

---

### Task 5: branch-fold

**Files:**
- Create: `crates/post-machine/src/optimizer/branch_fold.rs`
- Modify: `crates/post-machine/src/optimizer/mod.rs` (declare; `PIPELINE` final order: `check-fold`, `jump-threading`, `cell-state`, `branch-fold`, `dce`)
- Modify: `crates/post-machine/tests/opt_equivalence.rs`

**Interfaces:** consumes the dataflow module; produces `branch_fold::run`.

- [ ] **Step 1: `branch_fold.rs`:**

```rust
//! branch-fold (spec §8 pass 6): a `check` whose MF is statically known
//! goes unconditional. Sound only on coupled paths where the cell value
//! is proven (then MF == (cell == 1) by the coupling invariant); the
//! reset-MF trap (a check before any tape instruction) stays untouched
//! because such paths are `Uncoupled`.

use crate::ir::{IrFunction, IrTerm};
use crate::optimizer::dataflow;

pub fn run(f: &mut IrFunction) -> u32 {
    let entries = dataflow::block_entry_facts(f);
    let mut changes = 0;
    for b in &mut f.blocks {
        let Some(&entry_fact) = entries.get(&b.id) else {
            continue;
        };
        let mut fact = entry_fact;
        for op in &b.ops {
            fact = dataflow::transfer_op(fact, op);
        }
        if let IrTerm::Check { marked, blank } = b.term
            && let Some(sym) = fact.cell()
        {
            b.term = IrTerm::Goto {
                to: if sym == 1 { marked } else { blank },
            };
            changes += 1;
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrTerm, lower};
    use crate::lexer::lex;
    use crate::parser::parse;

    fn fold_fn(src: &str) -> crate::ir::IrFunction {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        ir.functions.remove(0)
    }

    #[test]
    fn known_written_value_decides_the_branch() {
        // wr 1 then check: marked arm (label 1) is statically taken.
        let f = fold_fn("f() { mark; check(1, 2); 1: left(!); 2: right; }");
        assert_eq!(f.blocks[0].term, IrTerm::Goto { to: 1 });
    }

    #[test]
    fn reset_mf_check_is_never_folded() {
        let f = fold_fn("f() { check(1, 2); 1: left(!); 2: right; }");
        assert!(matches!(f.blocks[0].term, IrTerm::Check { .. }));
    }

    #[test]
    fn moves_defeat_folding() {
        let f = fold_fn("f() { mark; right; check(1, 2); 1: left(!); 2: right; }");
        assert!(matches!(f.blocks[0].term, IrTerm::Check { .. }));
    }
}
```

- [ ] **Step 2: Equivalence tests** (append to `tests/opt_equivalence.rs`):

```rust
#[test]
fn branch_fold_cascades_into_dce_and_preserves() {
    let src = "main() { mark; check(1, 2); 1: unmark(!); 2: right; }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(o1 < o0, "folded branch + dead arm must shrink: {o0} -> {o1}");
}

#[test]
fn reset_mf_semantics_survive_o1() {
    // First instruction is a check: MF is the reset 0 on EVERY tape,
    // including marked ones. -O1 must not "know better".
    let src = "main() { check(1, 2); 1: mark(!); 2: unmark(!); }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert_eq!(o0, o1, "an unfoldable program must be byte-stable");
}
```

- [ ] **Step 3: Gates, then commit.**

```bash
git add crates/post-machine/src/optimizer crates/post-machine/tests/opt_equivalence.rs
git commit -m "feat(post-machine): branch-fold pass gated on the coupling invariant"
```

---

### Task 6: `-O1` goldens — the flagship program, opt-outs, stage snapshots

**Files:**
- Modify: `crates/post-machine/tests/opt_equivalence.rs`

**Interfaces:** consumes everything; produces the golden contract for `-O1`. Byte expectations are hand-derived; on mismatch, re-derive by hand first and BLOCK with your derivation if the plan's number is wrong (established protocol).

- [ ] **Step 1: Append the goldens:**

```rust
/// The program the whole optimizer story was started for in 2002:
/// redundant marks, a decided branch, a dead arm, a confirming write.
const FLAGSHIP: &str = "\
main() {
    mark;
    mark;
    right;
    mark, mark, unmark;
    check(1, 2);
1:  mark(!);
2:  unmark;
}
";

#[test]
fn flagship_optimizes_to_exact_bytes() {
    use mtc_post_machine::arch::opcodes::*;
    // Derivation: cell-state r1: [wr1,wr1,rgt,wr1,wr1,wr0] ->
    // idempotent-drop 2nd+4th wr1, dead-store the wr1 before wr0 ->
    // [wr1, rgt, wr0]; branch-fold r1: fact Coupled(Some(0)) at the
    // check -> goto blank arm (label 2); dce r1: block `1:` dies.
    // r2: block `2:`'s wr0 is idempotent (entry fact Coupled(Some(0)))
    // -> dropped, leaving an empty Return block. r3: no changes.
    // Codegen: ent, wr 1, rgt, wr 0, stp = 7 bytes.
    let out = compile(
        FLAGSHIP,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(
        linked.executable.code,
        vec![ENT, WR, 0x81, RGT, WR, 0x80, STP]
    );
    assert_eq!(out.report.opt.rounds, 3);

    // -O0 reference: 20 bytes (ent + 11 op bytes + jnm.s 2 + wr/stp 3 + wr/stp 3).
    let o0 = compile(FLAGSHIP, CompileOptions::default()).unwrap();
    let l0 = link(&[o0.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(l0.executable.code.len(), 20);
}

#[test]
fn flagship_is_equivalent_on_all_tapes() {
    let (o0, o1) = assert_equivalent(FLAGSHIP, TAPES);
    assert_eq!((o0, o1), (20, 7));
}

#[test]
fn fno_disables_a_single_pass() {
    let out = compile(
        FLAGSHIP,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["cell-state".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !out.report.opt.changes.iter().any(|c| c.pass == "cell-state"),
        "{:?}",
        out.report.opt.changes
    );
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    assert!(linked.executable.code.len() > 7);
}

#[test]
fn capture_ir_records_the_pass_stages() {
    let out = compile(
        FLAGSHIP,
        CompileOptions {
            opt_level: OptLevel::O1,
            capture_ir: true,
            ..Default::default()
        },
    )
    .unwrap();
    let stages: Vec<&str> = out.ir_snapshots.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(stages.first().copied(), Some("lowered"));
    assert_eq!(stages.last().copied(), Some("final"));
    assert!(stages.contains(&"after:cell-state"), "{stages:?}");
    assert!(stages.contains(&"after:branch-fold"), "{stages:?}");
    assert!(stages.contains(&"after:dce"), "{stages:?}");
    assert_ne!(out.ir_snapshots.first(), out.ir_snapshots.last());
    assert_eq!(out.ir, out.ir_snapshots.last().unwrap().1);
}

#[test]
fn spec_sample_is_already_optimal() {
    // goToEnd / goToBegin / main from spec §3: nothing for 6a passes to
    // do (loops re-enter Uncoupled; calls clobber facts) — -O1 must be
    // byte-identical to -O0, proving the optimizer's do-no-harm floor.
    let src = "\
goToEnd() {
1:  right;
    check(1, 2);
2:  left;
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
";
    let o0 = compile(src, CompileOptions::default()).unwrap();
    let o1 = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(o0.object, o1.object);
}
```

(Adjust `use` lines at the top of the file as needed: `OptLevel` is already imported from Task 2's harness.)

- [ ] **Step 2: Full gates, then commit.**

```bash
git add crates/post-machine/tests/opt_equivalence.rs
git commit -m "test(post-machine): -O1 goldens — flagship 20->7 bytes, fno opt-outs, IR stage snapshots"
```

---

## Plan Self-Review Notes

- **Spec coverage:** §8 passes 1-4 and 6 land here with the §8 equivalence contract and §11's optimized-vs-unoptimized testing regime; §7.1 gains real multi-stage `--emit-ir` backing (`capture_ir`). Passes 5/7/8 (inline, tail-merge, tail-call) are Plan 6b by declared scope split. `--strict-cells` (spec §8 pass 4 note) is deliberately deferred to the plan that makes cell semantics configurable — at PM-1 v1 all writes are idempotent-by-default, so the opt-out has nothing to toggle yet.
- **Soundness:** the coupling invariant and its reset-MF exception are stated once (plan header + dataflow module docs) and enforced three ways: `Uncoupled` entry facts, gated check-edge refinement, and the `reset_mf_semantics_survive_o1` / `entry_is_uncoupled…` tests. cell-state's MF-safety argument rides the same invariant (both the skipped and current MF equal `i == 1`).
- **Plan 5 acceptance item:** discharged via `validate_function` + the driver's debug-build check after every pass application; dce preserves closure by construction (deletes only unreachable blocks); jump-threading and branch-fold only retarget to existing ids.
- **Derived numbers double-checked:** flagship -O0 = 20 bytes (1 ent + 11 op bytes + 2 jnm.s + 3 + 3), -O1 = 7, rounds = 3 (traced in the test comment). Loop-entry merge in `loop_facts_reach_fixpoint` re-derived: entry ∨ marked-edge = Uncoupled.
- **Type consistency:** every pass is `fn(&mut IrFunction) -> u32`; `PIPELINE` grows monotonically across Tasks 2/4/5 to the exact final order named in Global Constraints; `CompileOptions` literals updated everywhere including the (to-be-committed) example.


