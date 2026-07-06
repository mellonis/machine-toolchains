# Plan 7: `pmt` CLI, stdlib, DebugSession, goldens, docs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the final v1 layer: the `pmt` binary (all seven spec subcommands), the standard library written in `.pmc`, a `DebugSession` over a step-granular VM driver, golden end-to-end tests of the historic programs with `.pmt` diffs, and the durable documentation set (with code-reference migration and design-spec freeze).

**Architecture:** Everything sits on the existing library API — the bin is a thin renderer over `compile`/`assemble`/`link`/`disassemble*`/`Machine` (library-first, spec-mandated). The one core change is refactoring the run loop into a reusable one-instruction stepper (zero behavior change for `run()`), on which `DebugSession` and `pmt run --trace` are built. The stdlib is `.pmc` source embedded in the crate and compiled once on demand.

**Tech Stack:** Rust stable, edition 2024, cargo workspace (`mtc-core` + `mtc-post-machine`). No new dependencies: the CLI arg parser is hand-rolled std; serde only for the existing JSON artifacts.

## Global Constraints

- Quality gates per task: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. Gates are declared on a CLEAN tree (`git status` empty) — the commit itself must pass.
- All 318 existing tests stay green. Existing `-O0`/`-O1` byte goldens must not change (this plan adds no compiler/optimizer behavior).
- Libraries never print — only the `cli` module renders text. Structured reports (`CompileReport`, `OptReport`, `LinkReport`) are the only source `-v` renders from.
- No new dependencies in any `Cargo.toml`. CLI is std-only.
- Forge agnosticism (user ruling 2026-07-06): applies to PUBLISHED content. `README.md`, the `docs/` reference pages, and code comments carry no provider URLs and reference no forge issues at all — cross-project work is described in prose (name the library and the feature, never an issue number); the canonical repository URL lives only in `Cargo.toml` metadata. Internal dev artifacts (progress ledger, plan documents) are unrestricted — issue links/URLs are fine there.
- Commit per task (pre-approved for this repo), never push, no Claude attribution anywhere.
- `.pmc`/`.pma`/format semantics are FROZEN — this plan may not alter language, ISA, formats, optimizer, or linker behavior except the additive `CoreEvent::Break` (Task 1), which `run()` must treat exactly like `Step`.
- Grid rendering (listing, dis) uses the established canonical columns; listing form is explicitly NOT reassembleable and must never emit `.func`.

## Rulings ratified by this plan (review anchors)

- **R1 — std ships embedded:** "prebuilt `std.pmo`" is realized as `.pmc` source embedded via `include_str!`, compiled once per process (`OnceLock`) with the release preset (`-O1`, `--strip-debugger`). A cargo-installed binary has no data directory; observable behavior is identical. `-L`/`-l` load real `.pmo` files for user libraries; `--nostdlib` skips the embedded std.
- **R2 — std is NOT built `--fno-inline`:** semantic binding (spec §9 interposition ruling) applies: overriding a std routine rebinds direct user calls, not std-internal uses. Documented in `docs/stdlib.md`.
- **R3 — roster postconditions settle per the old notes** (delegated by spec §9 "settled against the old notes"): `goToEnd`/`goToBegin` land ON the last/first MARK of the current section (the historic `Sum.pms`/`Ty.pms` semantics: `right; if(1,3); left`), not on the adjacent blank. The §9 parenthetical sketch is superseded by this settlement; `docs/stdlib.md` is the normative statement.
- **R4 — snapshot labels stay un-round-qualified:** `--emit-ir=<stage>` picks the LAST matching snapshot (documented last-wins). `PassChange` gains no round field (v2 parking lot).
- **R5 — warning strictness:** `pmt compile -Werror` turns warnings into a failing exit. Warnings render to stderr always.
- **R6 — `pmt run` defaults:** `--max-steps` defaults to 10,000,000 (the Delphi step-cap descendant); `--no-step-limit` opts out. Exit codes: 0 = stopped, 1 = CLI/toolchain error, 2 = halted (`hlt`), 3 = trapped.
- **R7 — `brk` surfacing:** core emits a new `CoreEvent::Break` when an instruction containing `MicroOp::Brk` retires; `run()` handles it identically to `Step` (brk stays a no-op without a debugger); `DebugSession` pauses on it.
- **R8 — no `pmt debug` subcommand:** v1 CLI is exactly the seven spec subcommands. `DebugSession` is a library feature and powers `pmt run --trace`.
- **R10 — `--trace` is live and stateful (user ruling 2026-07-06):** trace lines stream to stderr AS the program runs (not buffered into `CliOutput`), and every line carries a post-execution state suffix `  ; MF=<0|1> head=<n>` — the Delphi step-view lineage. Requires `Tape::head()` on the device trait (Task 1) and a writer seam through the CLI (`execute_with`).
- **R9 — trap-state deferral closed:** `RunResult` gains `ip: u32` (address of the last instruction the core worked on) and `stack: Vec<u32>` (return stack at termination); it loses `Copy` (keeps `Clone`).

## File Structure

| File | Responsibility |
|---|---|
| `crates/core/src/vm/bus.rs` (modify) | `CoreEvent::Break` variant |
| `crates/core/src/vm/core.rs` (modify) | `brk_pending` flag, `instr_start()` accessor, Break emission |
| `crates/core/src/vm/driver.rs` (modify) | `step_instruction` refactor, `run()` on top, `RunResult` new fields |
| `crates/core/src/vm/debug.rs` (create) | `DebugSession`, `DebugEvent`, `PauseCause` |
| `crates/core/src/vm/machine.rs` (modify) | `Machine::debug`, `run()` fills new `RunResult` fields |
| `crates/core/src/asm/disassembler.rs` (modify) | `listing_line`, `listing_executable`, map-aware `disassemble_executable` |
| `crates/post-machine/src/asm/mod.rs` (modify) | wrapper updates (`disassemble_executable_with_map`) |
| `crates/post-machine/examples/compile_and_run.rs` (modify) | use library listing, drop hand-rolled copy |
| `crates/post-machine/src/stdlib/mod.rs` + `std.pmc` (create) | embedded stdlib |
| `crates/post-machine/tests/golden_programs.rs` + `tests/golden/*` (create) | golden e2e |
| `crates/post-machine/src/ir.rs` (modify) | `IrFunction::to_mermaid` |
| `crates/post-machine/src/cli/mod.rs` (create) | dispatch, arg helper, shared render helpers |
| `crates/post-machine/src/cli/build.rs` (create) | `compile`, `asm`, `link` subcommands |
| `crates/post-machine/src/cli/inspect.rs` (create) | `dis`, `tape`, `ir` subcommands |
| `crates/post-machine/src/cli/run.rs` (create) | `run` subcommand (+ `--trace`) |
| `crates/post-machine/src/bin/pmt.rs` (create) | thin main |
| `crates/post-machine/tests/cli_programs.rs` (create) | CLI integration tests |
| `docs/*.md`, `README.md` (create/rewrite) | durable docs, reference migration, spec freeze |
| all `crates/**/*.rs` (comments only) | Task 8: full comment audit — staleness, citation hygiene, doc back-fill |

---

### Task 1: Step-granular driver, `CoreEvent::Break`, `DebugSession`, `RunResult` state

**Files:**
- Modify: `crates/core/src/vm/bus.rs`, `crates/core/src/vm/core.rs`, `crates/core/src/vm/driver.rs`, `crates/core/src/vm/machine.rs`, `crates/core/src/vm/mod.rs`
- Create: `crates/core/src/vm/debug.rs`

**Interfaces:**
- Consumes: existing `Core`, `ReturnStack`, `Tape`, `TactProfile`, `RunLimits`, `RunStats`, `Outcome`, `Trap`.
- Produces (later tasks rely on these exact names):
  - `RunResult { outcome: Outcome, stats: RunStats, ip: u32, stack: Vec<u32> }` — loses `Copy`, keeps `Debug, Clone, PartialEq, Eq`.
  - `pub(crate) enum StepEvent { Retired, Break, Finished(Outcome) }` and `pub(crate) fn step_instruction(...)` in `driver.rs`.
  - `pub struct DebugSession<'a>` with `step_in`, `step_over`, `step_out`, `continue_`, `run_steps`, `add_breakpoint`, `remove_breakpoint`, `ip()`, `mf()`, `depth()`, `stack()`, `stats()`, `finished()`; `pub enum DebugEvent { Paused(PauseCause), Finished(Outcome) }`; `pub enum PauseCause { Step, Breakpoint(u32), Brk, Manual, Trap(Trap) }`.
  - `Machine::debug(&self, opts: RunOptions) -> DebugSession<'_>`.
  - `Core::instr_start(&self) -> u32`.
  - `Tape::head(&self) -> i64` — new required method on the device trait (R10).
  - Re-exports from `vm/mod.rs`: `DebugSession`, `DebugEvent`, `PauseCause`.

- [ ] **Step 1: `CoreEvent::Break` in the bus vocabulary**

In `crates/core/src/vm/bus.rs` add the variant:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreEvent {
    Request(BusRequest),
    Step,
    /// An instruction containing `MicroOp::Brk` retired. Drivers without
    /// a debugger treat this exactly like `Step` (brk is a no-op); a
    /// debug session pauses on it (spec-lineage: §4.5 brk semantics).
    Break,
    Stopped,
    Halted,
    Trapped(Trap),
}
```

- [ ] **Step 2: core emits Break; expose `instr_start`**

In `crates/core/src/vm/core.rs`:

1. Add field `brk_pending: bool` to `Core` (init `false` in `new()`).
2. Add accessor next to `ip()`:

```rust
    /// Address of the instruction the core is executing (or last worked
    /// on) — the faulting address on traps, unlike `ip()` which has
    /// advanced past fetched bytes.
    pub fn instr_start(&self) -> u32 {
        self.instr_start
    }
```

3. In `step_execute`'s micro-op loop, split the `Nop | Brk` arm:

```rust
                MicroOp::Nop => continue,
                MicroOp::Brk => {
                    self.brk_pending = true;
                    continue;
                }
```

(`brk_pending` must live on `Core`, not a local: a hypothetical arch may lower `[Brk, MoveLeft]`, where retirement happens in a later `step_execute` call than the one that saw `Brk`.)

4. At retirement (currently `self.phase = Phase::StepAck; CoreEvent::Step`):

```rust
        // 3. Instruction retired.
        self.phase = Phase::StepAck;
        if std::mem::take(&mut self.brk_pending) {
            CoreEvent::Break
        } else {
            CoreEvent::Step
        }
```

5. Update the core test driver helper (the scripted-image function feeding `CodeRead`s) so it acks `CoreEvent::Break` the same way it acks `Step`. The existing `halt_and_brk_nop` test keeps passing (Stopped outcome unchanged); extend it (or add a sibling test) to assert the brk instruction yields `Ev::Break` (not `Ev::Step`) and that resuming after `Break` continues normally.

- [ ] **Step 3: refactor `driver::run` onto `step_instruction`**

In `crates/core/src/vm/driver.rs`, extract the loop body. The bus-servicing and limit logic must be MOVED verbatim, not rewritten — behavior is pinned by the existing tact tests, including the mid-instruction `TactLimit` check after every serviced request:

```rust
/// One instruction boundary of the sync driver (spec-lineage §4.4
/// accounting). `started` is the fresh/resume flag: `false` before the
/// first call. Callers must not call again after `Finished` (the core is
/// in its terminal phase).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepEvent {
    Retired,
    Break,
    Finished(Outcome),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn step_instruction(
    core: &mut Core,
    code: &[u8],
    stack: &mut ReturnStack,
    device: &mut dyn Tape,
    profile: TactProfile,
    limits: RunLimits,
    stats: &mut RunStats,
    started: &mut bool,
) -> StepEvent {
    let over_tacts =
        |stats: &RunStats| limits.max_tacts.is_some_and(|max| stats.total_tacts() >= max);

    let mut event = if *started {
        core.resume(BusResponse::Ok) // ack the StepAck phase
    } else {
        *started = true;
        core.start()
    };
    loop {
        match event {
            CoreEvent::Request(request) => {
                let response = match request {
                    // ... the entire request-servicing match from today's
                    // run(), verbatim, mutating `stats` ...
                };
                if over_tacts(stats) {
                    return StepEvent::Finished(Outcome::Trapped(Trap::TactLimit));
                }
                event = core.resume(response);
            }
            CoreEvent::Step | CoreEvent::Break => {
                stats.steps += 1;
                stats.core_tacts += 1; // execute base (spec-lineage §4.4)
                if limits.max_steps.is_some_and(|max| stats.steps >= max) {
                    return StepEvent::Finished(Outcome::Trapped(Trap::StepLimit));
                }
                if over_tacts(stats) {
                    return StepEvent::Finished(Outcome::Trapped(Trap::TactLimit));
                }
                return if matches!(event, CoreEvent::Break) {
                    StepEvent::Break
                } else {
                    StepEvent::Retired
                };
            }
            CoreEvent::Stopped => return StepEvent::Finished(Outcome::Stopped),
            CoreEvent::Halted => return StepEvent::Finished(Outcome::Halted),
            CoreEvent::Trapped(trap) => return StepEvent::Finished(Outcome::Trapped(trap)),
        }
    }
}
```

No semantic shift is permitted: the refactor keeps limit checks at the exact same points as today's `run()` (after each serviced request; at the Step/Break boundary before returning `Retired`), so every `RunResult` value is bit-identical. Trace the ordering against the existing tact tests before declaring green.

`run()` becomes:

```rust
pub fn run(
    core: &mut Core,
    code: &[u8],
    stack: &mut ReturnStack,
    device: &mut dyn Tape,
    profile: TactProfile,
    limits: RunLimits,
) -> RunResult {
    let mut stats = RunStats::default();
    let mut started = false;
    loop {
        match step_instruction(
            core, code, stack, device, profile, limits, &mut stats, &mut started,
        ) {
            StepEvent::Retired | StepEvent::Break => {}
            StepEvent::Finished(outcome) => {
                return RunResult {
                    outcome,
                    stats,
                    ip: core.instr_start(),
                    stack: stack.entries().to_vec(),
                };
            }
        }
    }
}
```

And `RunResult` (R9):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub outcome: Outcome,
    pub stats: RunStats,
    /// Address of the last instruction the core worked on: the faulting
    /// instruction for traps, the terminating `stp`/`hlt` otherwise.
    pub ip: u32,
    /// Return stack at termination (deepest frame first).
    pub stack: Vec<u32>,
}
```

Fix all compile fallout in existing tests by cloning/borrowing — assertions must not be weakened.

- [ ] **Step 4: write failing tests for `DebugSession` (TestArch)**

Create `crates/core/src/vm/debug.rs` with a `#[cfg(test)] mod tests` first. TestArch opcodes: `0x01` nop, `0x02` stop, `0x03` halt, `0x04` brk, `0x05` left+latch, `0x06` right+latch, `0x07` wr(vec)+latch, `0x08` jmp rel8, `0x0A` call rel32, `0x0B` ret, `0x0E` ent. Tests (write them against the API in Step 5, run to see them fail to compile):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::devices::InfiniteTape;
    use crate::vm::driver::{ReturnStack, RunLimits, TactProfile};
    use crate::vm::trap::Trap;
    use crate::vm::{Core, Outcome};

    fn session(code: &[u8]) -> DebugSession<'static> {
        static ARCH: TestArch = TestArch;
        DebugSession::new(
            Core::new(&ARCH, 0),
            code.to_vec(),
            ReturnStack::new(4),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
    }

    #[test]
    fn step_in_pauses_each_instruction() {
        // nop; nop; stop
        let mut s = session(&[0x01, 0x01, 0x02]);
        let mut tape = InfiniteTape::new();
        assert_eq!(s.step_in(&mut tape), DebugEvent::Paused(PauseCause::Step));
        assert_eq!(s.ip(), 1);
        assert_eq!(s.step_in(&mut tape), DebugEvent::Paused(PauseCause::Step));
        assert_eq!(s.step_in(&mut tape), DebugEvent::Finished(Outcome::Stopped));
        assert_eq!(s.finished(), Some(Outcome::Stopped));
        assert_eq!(s.stats().steps, 2); // stop retires no Step event
    }

    #[test]
    fn continue_pauses_on_brk_then_finishes() {
        // nop; brk; nop; stop
        let mut s = session(&[0x01, 0x04, 0x01, 0x02]);
        let mut tape = InfiniteTape::new();
        assert_eq!(s.continue_(&mut tape), DebugEvent::Paused(PauseCause::Brk));
        assert_eq!(s.ip(), 2); // paused AFTER the brk instruction retired
        assert_eq!(s.continue_(&mut tape), DebugEvent::Finished(Outcome::Stopped));
    }

    #[test]
    fn breakpoint_pauses_before_the_instruction_and_resumes_past_it() {
        // nop; nop; nop; stop
        let mut s = session(&[0x01, 0x01, 0x01, 0x02]);
        let mut tape = InfiniteTape::new();
        s.add_breakpoint(2);
        assert_eq!(
            s.continue_(&mut tape),
            DebugEvent::Paused(PauseCause::Breakpoint(2))
        );
        assert_eq!(s.ip(), 2); // instruction at 2 NOT yet executed
        assert_eq!(s.stats().steps, 2);
        // continuing from the breakpoint must move past it, not re-pause
        assert_eq!(s.continue_(&mut tape), DebugEvent::Finished(Outcome::Stopped));
    }

    #[test]
    fn breakpoint_at_entry_pauses_only_after_leaving_it() {
        // bp at the entry instruction: first continue_ executes it (a
        // fresh session is "paused at" entry already, debugger convention)
        let mut s = session(&[0x01, 0x02]);
        let mut tape = InfiniteTape::new();
        s.add_breakpoint(0);
        assert_eq!(s.continue_(&mut tape), DebugEvent::Finished(Outcome::Stopped));
    }

    #[test]
    fn step_over_runs_the_call_to_completion() {
        // 0: call +2 -> 7; 5: nop; 6: stop; 7: ent; 8: nop; 9: ret
        let code = &[0x0A, 0x02, 0x00, 0x00, 0x00, 0x01, 0x02, 0x0E, 0x01, 0x0B];
        let mut s = session(code);
        let mut tape = InfiniteTape::new();
        assert_eq!(s.step_over(&mut tape), DebugEvent::Paused(PauseCause::Step));
        assert_eq!(s.ip(), 5); // back on the nop after the call
        assert_eq!(s.depth(), 0);
    }

    #[test]
    fn step_out_returns_to_the_caller() {
        // 0: call +2 -> 7; 5: nop; 6: stop; 7: ent; 8: nop; 9: ret
        let code = &[0x0A, 0x02, 0x00, 0x00, 0x00, 0x01, 0x02, 0x0E, 0x01, 0x0B];
        let mut s = session(code);
        let mut tape = InfiniteTape::new();
        assert_eq!(s.step_in(&mut tape), DebugEvent::Paused(PauseCause::Step));
        assert_eq!(s.ip(), 7); // paused at the callee's ent
        assert_eq!(s.depth(), 1);
        assert_eq!(s.step_out(&mut tape), DebugEvent::Paused(PauseCause::Step));
        assert_eq!(s.depth(), 0);
        assert_eq!(s.ip(), 5);
    }

    #[test]
    fn trap_pauses_on_the_faulting_instruction_then_finishes() {
        // ret with empty stack → StackUnderflow
        let mut s = session(&[0x0B]);
        let mut tape = InfiniteTape::new();
        assert_eq!(
            s.continue_(&mut tape),
            DebugEvent::Paused(PauseCause::Trap(Trap::StackUnderflow))
        );
        assert_eq!(s.finished(), Some(Outcome::Trapped(Trap::StackUnderflow)));
        // any further motion reports the terminal outcome
        assert_eq!(
            s.step_in(&mut tape),
            DebugEvent::Finished(Outcome::Trapped(Trap::StackUnderflow))
        );
    }

    #[test]
    fn run_steps_budget_pauses_manual() {
        let mut s = session(&[0x01, 0x01, 0x01, 0x02]);
        let mut tape = InfiniteTape::new();
        assert_eq!(s.run_steps(&mut tape, 2), DebugEvent::Paused(PauseCause::Manual));
        assert_eq!(s.stats().steps, 2);
        assert_eq!(s.run_steps(&mut tape, 10), DebugEvent::Finished(Outcome::Stopped));
    }
}
```

Note on `step_over`/`step_in` expectations: after `step_in` executes the call instruction, the pause point is the callee's `ent` (ip = 7, depth 1); verify against the trace and pin the true values — derive, never adjust to observed without re-derivation. For `step_over`: executing the call from depth 0 leaves depth 1, so the session keeps stepping until depth ≤ 0 again; the pause lands after the `ret` executes, ip = 5 (the return address, i.e. the `nop` at 5). Trace: call pushes 5; ret pops → ip 5.

- [ ] **Step 5: implement `DebugSession`**

```rust
//! Interactive debugger session (spec-lineage §4.5): the same surface
//! shape as turing-machine-js v7 sessions — the session owns the run;
//! step/pause with a cause; depth-based stepIn/stepOver/stepOut (depth
//! is just SP). Sync v1: external pause/run-interval throttle is
//! modelled by `run_steps` chunking.

use std::collections::BTreeSet;

use super::core::Core;
use super::devices::Tape;
use super::driver::{
    ReturnStack, RunLimits, RunStats, StepEvent, TactProfile, step_instruction,
};
use super::trap::Trap;
use super::Outcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseCause {
    /// A stepping command completed.
    Step,
    /// About to execute the instruction at this address.
    Breakpoint(u32),
    /// A `brk` instruction retired.
    Brk,
    /// A `run_steps` budget was exhausted (the sync analog of external
    /// `pause()` / the run-interval throttle).
    Manual,
    /// Trapped — paused ON the fault with state inspectable; any further
    /// stepping reports `Finished(Trapped)`.
    Trap(Trap),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugEvent {
    Paused(PauseCause),
    Finished(Outcome),
}

pub struct DebugSession<'a> {
    core: Core<'a>,
    code: Vec<u8>,
    stack: ReturnStack,
    stats: RunStats,
    profile: TactProfile,
    limits: RunLimits,
    breakpoints: BTreeSet<u32>,
    started: bool,
    finished: Option<Outcome>,
    trap_reported: bool,
}

impl<'a> DebugSession<'a> {
    pub fn new(
        core: Core<'a>,
        code: Vec<u8>,
        stack: ReturnStack,
        profile: TactProfile,
        limits: RunLimits,
    ) -> Self {
        Self {
            core,
            code,
            stack,
            stats: RunStats::default(),
            profile,
            limits,
            breakpoints: BTreeSet::new(),
            started: false,
            finished: None,
            trap_reported: false,
        }
    }

    pub fn add_breakpoint(&mut self, addr: u32) {
        self.breakpoints.insert(addr);
    }

    pub fn remove_breakpoint(&mut self, addr: u32) {
        self.breakpoints.remove(&addr);
    }

    /// Next instruction's address while paused; the faulting
    /// instruction's address after a trap pause.
    pub fn ip(&self) -> u32 {
        match self.finished {
            Some(Outcome::Trapped(_)) => self.core.instr_start(),
            _ => self.core.ip(),
        }
    }

    pub fn mf(&self) -> bool {
        self.core.mf()
    }

    pub fn depth(&self) -> usize {
        self.stack.depth()
    }

    pub fn stack(&self) -> &[u32] {
        self.stack.entries()
    }

    pub fn stats(&self) -> RunStats {
        self.stats
    }

    pub fn finished(&self) -> Option<Outcome> {
        self.finished
    }

    /// One raw instruction; the shared bottom of every public motion.
    fn advance(&mut self, device: &mut dyn Tape) -> StepEvent {
        if !self.started {
            // Initial MF latch, tact-free (loading, not execution) —
            // mirrors Machine::run; PM-1 matches against mark index 1.
            self.core.set_mf(device.read() == 1);
        }
        let event = step_instruction(
            &mut self.core,
            &self.code,
            &mut self.stack,
            device,
            self.profile,
            self.limits,
            &mut self.stats,
            &mut self.started,
        );
        if let StepEvent::Finished(outcome) = event {
            self.finished = Some(outcome);
        }
        event
    }

    /// Terminal-state gate: once finished, every motion reports the
    /// outcome (a trap is reported as a pause exactly once).
    fn gate(&mut self) -> Option<DebugEvent> {
        let outcome = self.finished?;
        Some(DebugEvent::Finished(outcome))
    }

    fn settle(&mut self, event: StepEvent, cause_on_retire: PauseCause) -> DebugEvent {
        match event {
            StepEvent::Retired => DebugEvent::Paused(cause_on_retire),
            StepEvent::Break => DebugEvent::Paused(PauseCause::Brk),
            StepEvent::Finished(Outcome::Trapped(trap)) if !self.trap_reported => {
                self.trap_reported = true;
                DebugEvent::Paused(PauseCause::Trap(trap))
            }
            StepEvent::Finished(outcome) => DebugEvent::Finished(outcome),
        }
    }

    pub fn step_in(&mut self, device: &mut dyn Tape) -> DebugEvent {
        if let Some(done) = self.gate() {
            return done;
        }
        let event = self.advance(device);
        self.settle(event, PauseCause::Step)
    }

    pub fn continue_(&mut self, device: &mut dyn Tape) -> DebugEvent {
        self.run_until(device, None, None)
    }

    pub fn run_steps(&mut self, device: &mut dyn Tape, budget: u64) -> DebugEvent {
        self.run_until(device, None, Some(budget))
    }

    pub fn step_over(&mut self, device: &mut dyn Tape) -> DebugEvent {
        let depth0 = self.stack.depth();
        if let Some(done) = self.gate() {
            return done;
        }
        let event = self.advance(device);
        if self.stack.depth() > depth0 {
            if let StepEvent::Retired = event {
                return self.run_until(device, Some(depth0), None);
            }
        }
        self.settle(event, PauseCause::Step)
    }

    pub fn step_out(&mut self, device: &mut dyn Tape) -> DebugEvent {
        let target = self.stack.depth().checked_sub(1);
        let Some(target) = target else {
            // already at the outermost frame: stepping out = run to end
            return self.continue_(device);
        };
        self.run_until(device, Some(target), None)
    }

    /// Shared engine: run until (a) depth ≤ `until_depth`, (b) a
    /// breakpoint is about to execute, (c) a brk retires, (d) the budget
    /// runs dry, or (e) the program finishes/traps. Breakpoints are not
    /// checked before the first instruction of a resume (a paused-at-
    /// breakpoint session must move past it).
    fn run_until(
        &mut self,
        device: &mut dyn Tape,
        until_depth: Option<usize>,
        mut budget: Option<u64>,
    ) -> DebugEvent {
        if let Some(done) = self.gate() {
            return done;
        }
        // A zero budget pauses immediately without stepping (and guards
        // the decrement below against u64 underflow) — ratified fix,
        // 2026-07-06, after the Task-1 implementer flagged the panic.
        if budget == Some(0) {
            return DebugEvent::Paused(PauseCause::Manual);
        }
        loop {
            let event = self.advance(device);
            match event {
                StepEvent::Retired => {
                    if let Some(target) = until_depth {
                        if self.stack.depth() <= target {
                            return DebugEvent::Paused(PauseCause::Step);
                        }
                    }
                    if let Some(b) = budget.as_mut() {
                        *b -= 1;
                        if *b == 0 {
                            return DebugEvent::Paused(PauseCause::Manual);
                        }
                    }
                    if self.breakpoints.contains(&self.core.ip()) {
                        return DebugEvent::Paused(PauseCause::Breakpoint(self.core.ip()));
                    }
                }
                other => return self.settle(other, PauseCause::Step),
            }
        }
    }
}
```

Wire up `vm/mod.rs`: `pub mod debug;` + `pub use debug::{DebugEvent, DebugSession, PauseCause};`. Note `step_over` returning early when depth ≤ depth0 for the plain (non-call) instruction: single `advance` + `settle` covers it.

- [ ] **Step 6: `Machine::debug` and `Machine::run` fallout**

In `machine.rs`:

```rust
    /// A debug session over this machine's image (spec-lineage §4.5).
    /// The session owns its core/stack; the device arrives per call.
    pub fn debug(&self, opts: RunOptions) -> DebugSession<'a> {
        DebugSession::new(
            Core::new(self.arch, self.entry),
            self.code.clone(),
            ReturnStack::new(opts.stack_depth),
            opts.profile,
            opts.limits,
        )
    }
```

(The initial MF latch happens inside the session's first `advance`.) `Machine::run` keeps its own pre-latch — no change there beyond `run()`'s new `RunResult` fields flowing through. Add a machine-level test: a program that traps (`jmp` out of the image via TestArch `0x08` with a large offset) yields `result.ip == 0` (the jmp's address) and `result.stack` empty; a `call`-then-trap program yields `result.stack == vec![return_addr]`.

- [ ] **Step 6b: `Tape::head()` on the device trait (R10)**

Trace rendering (Task 6) and state inspection need the device's head position through `&dyn Tape`. Add to the `Tape` trait in `devices/mod.rs`:

```rust
    /// Current head position (annular tapes: the current index).
    fn head(&self) -> i64;
```

`InfiniteTape` already has an inherent `head()` — implement the trait method by delegating to it (or fold the inherent method into the trait impl). `StrictTape` delegates to its inner tape. `AnnularTape` returns its current index as `i64`. Update every test double implementing `Tape` across the workspace (return the tracked position; `0` only where the double genuinely never moves). NO default body — a silent `0` default would lie about real tapes.

- [ ] **Step 7: run gates and commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check` on a clean tree.
Expected: all green (318 existing + new).

```bash
git add -A crates/core
git commit -m "feat(core): step-granular driver, CoreEvent::Break, DebugSession, trap-state on RunResult"
```

---

### Task 2: Listing renderer and map-aware disassembly

**Files:**
- Modify: `crates/core/src/asm/disassembler.rs`, `crates/post-machine/src/asm/mod.rs`, `crates/post-machine/examples/compile_and_run.rs`
- Tests: co-located in `disassembler.rs` + adjust pm1 wrapper callers

**Interfaces:**
- Consumes: `ArchSyntax`, `Executable`, `MapFile` (`crate::linker::MapFile`), existing private `decode_one`/`Decoded`.
- Produces:
  - `pub fn listing_line(syntax: &ArchSyntax, code: &[u8], addr: u32, resolve: &dyn Fn(u32) -> Option<String>) -> (String, u32)` — one formatted listing line (no trailing newline) + instruction byte length (unknown byte → `.byte`, length 1; truncated operand → `.byte`, length 1).
  - `pub fn listing_executable(syntax: &ArchSyntax, exe: &Executable, map: Option<&MapFile>) -> String` — debugger code view (user ruling, ledger P2b): addresses + raw bytes + mnemonics, function headers from the map, NOT reassembleable.
  - `disassemble_executable(syntax, exe, map: Option<&MapFile>)` — SIGNATURE CHANGE: canonical exe dis names roots from the map when given (`f.start == root → f.name`), falling back to the existing `main`/`func_XXXX` synthesis.
  - pm1 wrappers: `disassemble_executable(exe)` (unchanged behavior, passes `None`) and new `disassemble_executable_with_map(exe: &Executable, map: &MapFile) -> String`; new `listing_executable(exe, map: Option<&MapFile>)` and re-export `listing_line` usage via `pm1_syntax()`.

- [ ] **Step 1: write the failing derived-golden test**

In `disassembler.rs` tests (this image is hand-authored PM-1 bytes, spec §5 opcodes — NOT compiled output):

```rust
    #[test]
    fn listing_renders_the_derived_golden() {
        use crate::linker::{MapFile, MapFunction};
        // 0: ent | 1: rgt | 2-3: wr 1 (0x06 0x81) | 4-5: jm.s -5 → 1 | 6: stp
        let exe = Executable {
            arch: 0x01,
            entry: 0,
            code: vec![0x0D, 0x05, 0x06, 0x81, 0x19, 0xFB, 0x02],
        };
        let map = MapFile {
            arch: 0x01,
            functions: vec![MapFunction {
                name: "main".into(),
                start: 0,
                end: 7,
                labels: vec![("L1".into(), 1)],
                lines: vec![],
            }],
        };
        let listing = listing_executable(&pm1_like_syntax(), &exe, Some(&map));
        let expected = "\
main:
  0000:  0D              ent
  0001:  05              rgt
  0002:  06 81           wr      1
  0004:  19 FB           jm.s    0x0001 <main.L1>
  0006:  02              stp
";
        assert_eq!(listing, expected);
    }
```

`pm1_like_syntax()`: the core crate cannot depend on PM-1 — build a minimal local `ArchSyntax` in the test with exactly the entries used (`ent` 0x0D None, `rgt` 0x05 None, `wr` 0x06 SymbolVec, `jm.s` 0x19 RelI8 Branch, `stp` 0x02 Stop), mirroring how other core tests build fake syntaxes. Column grid: address at 2, `{addr:04x}:`, two spaces, bytes hex `{:<15}`, mnemonic `{:<8}`, operand; lines `trim_end`ed; function header `name:` at column 0 when the map has `start == addr`.

Also add:
- `listing_line` length test on a SymbolVec operand (`[0x06, 0x01, 0x82]` → wr `1, 2`, len 3).
- coverage test: for the golden exe, sum of `listing_line` lengths from 0 == code len.
- map-aware canonical dis: `disassemble_executable(&syntax, &exe2, Some(&map))` where exe2 has a call — the callee root prints under its map name instead of `func_XXXX`; with `None` the output is byte-identical to today's (pin with an existing case).

Run: `cargo test -p mtc-core listing` — expect FAIL (functions missing).

- [ ] **Step 2: implement**

Port the example's `print_listing` logic into `disassembler.rs` (it is the approved preview — ledger P5 note), restructured as `listing_line` + `listing_executable`:

- `listing_line`: decode at `addr` exactly as the example does (None → `.byte`; RelI8/RelI32 with bounds checks → target via `resolve` rendered `"{target:#06x} <{name}>"` or bare `"{target:#06x}"`; SymbolVec self-delimiting 7-bit list; truncated → `.byte`). Format `format!("  {addr:04x}:  {bytes_hex:<15} {mnemonic:<8}{operand}")`, `trim_end`, return `(line, len as u32)`.
- `listing_executable`: walk `0..code.len()`; before each line, if the map names a function starting here emit `"{name}:\n"`; resolver = map lookup (function starts → name; labels → `"{fn}.{label}"`); `map: None` → no headers, no names.
- `disassemble_executable` gains the `map: Option<&MapFile>` parameter; only the `func_name` closure changes:

```rust
    let func_name = |addr: u32| {
        if let Some(m) = map {
            if let Some(f) = m.functions.iter().find(|f| f.start == addr) {
                return f.name.clone();
            }
        }
        if addr == exe.entry {
            "main".to_string()
        } else {
            format!("func_{addr:04X}")
        }
    };
```

Update pm1 `asm/mod.rs`:

```rust
pub fn disassemble_executable(exe: &Executable) -> String {
    mtc_core::asm::disassemble_executable(&pm1_syntax(), exe, None)
}

pub fn disassemble_executable_with_map(exe: &Executable, map: &MapFile) -> String {
    mtc_core::asm::disassemble_executable(&pm1_syntax(), exe, Some(map))
}

pub fn listing_executable(exe: &Executable, map: Option<&MapFile>) -> String {
    mtc_core::asm::listing_executable(&pm1_syntax(), exe, map)
}
```

Fix all callers of the core signature (core tests, pm1 tests). Round-trip tests keep asserting on the `None` path.

- [ ] **Step 3: slim the example**

In `compile_and_run.rs`, delete the hand-rolled `print_listing` and call the library:

```rust
    println!("== listing (addresses) ==");
    print!(
        "{}",
        mtc_post_machine::asm::listing_executable(&linked.executable, Some(&linked.map))
    );
```

Run `cargo run -p mtc-post-machine --example compile_and_run` — output section must show the same listing shape as before.

- [ ] **Step 4: gates and commit**

```bash
git add -A crates docs
git commit -m "feat(asm): listing renderer (debugger code view) + map-aware executable disassembly"
```

---

### Task 3: The standard library

**Files:**
- Create: `crates/post-machine/src/stdlib/mod.rs`, `crates/post-machine/src/stdlib/std.pmc`
- Modify: `crates/post-machine/src/lib.rs` (add `pub mod stdlib;`)
- Test: `crates/post-machine/tests/stdlib_programs.rs`

**Interfaces:**
- Consumes: `compile`, `CompileOptions`, `OptLevel`, pm1 `link`, `Machine`, `InfiniteTape`.
- Produces: `stdlib::SOURCE: &str`, `stdlib::object() -> &'static ObjectFile`. Symbol names all `std::`-prefixed. Later tasks (4, 5) link against `stdlib::object()`.

- [ ] **Step 1: write `std.pmc`**

`crates/post-machine/src/stdlib/std.pmc` — the roster of 11 (spec §9), postconditions per R3. Head pre/postconditions are stated per routine here and become `docs/stdlib.md` verbatim in Task 7:

```c
// The pmt standard library (dogfooding: written in .pmc, spec-lineage §9).
// Section = maximal run of marked cells. Pre/postconditions are the
// normative contract; docs/stdlib.md mirrors them.
//
// Under --strict-cells note: routines that write assume the stated
// precondition; eraseSection/remove* unmark only marked cells and
// append/prepend mark only blank cells when preconditions hold.

namespace std {
    // pre: head on a mark of a section. post: head on the section's
    // LAST mark; tape unchanged. (The historic Sum.pms pair.)
    export goToEnd() {
        1: right;
        2: check(1, 3);
        3: left;
    }

    // pre: head on a mark of a section. post: head on the section's
    // FIRST mark; tape unchanged.
    export goToBegin() {
        1: left;
        2: check(1, 3);
        3: right;
    }

    // pre: a mark exists strictly right of the head. post: head on the
    // nearest such mark; tape unchanged.
    export goToMarkRight() {
        1: right;
        2: check(!, 1);
    }

    // pre: a mark exists strictly left of the head. post: head on the
    // nearest such mark; tape unchanged.
    export goToMarkLeft() {
        1: left;
        2: check(!, 1);
    }

    // pre: a blank exists strictly right of the head (always true off a
    // finite tape's sections). post: head on the nearest such blank.
    export goToBlankRight() {
        1: right;
        2: check(1, !);
    }

    // pre: a blank exists strictly left of the head. post: head on the
    // nearest such blank.
    export goToBlankLeft() {
        1: left;
        2: check(1, !);
    }

    // pre: head on a mark of a section. post: section erased; head on
    // the first cell right of where the section was.
    export eraseSection() {
        1: @goToBegin();
        2: unmark;
        3: right;
        4: check(2, !);
    }

    // pre: head on a mark of a section. post: section grown by one mark
    // on the right; head on the new (last) mark.
    export appendMark() {
        1: @goToEnd();
        2: right;
        3: mark(!);
    }

    // pre: head on a mark of a section. post: section grown by one mark
    // on the left; head on the new (first) mark.
    export prependMark() {
        1: @goToBegin();
        2: left;
        3: mark(!);
    }

    // pre: head on a mark of a section. post: last mark removed; head
    // one cell left of the removed mark (the new last mark, or a blank
    // if the section had one mark).
    export removeLastMark() {
        1: @goToEnd();
        2: unmark;
        3: left(!);
    }

    // pre: head on a mark of a section. post: first mark removed; head
    // one cell right of the removed mark (the new first mark, or a
    // blank if the section had one mark).
    export removeFirstMark() {
        1: @goToBegin();
        2: unmark;
        3: right(!);
    }
}
```

- [ ] **Step 2: the module**

`crates/post-machine/src/stdlib/mod.rs`:

```rust
//! The standard library: `.pmc` source embedded in the toolchain and
//! compiled once per process (ruling R1: "prebuilt std.pmo ships with
//! the toolchain" realized as an embedded object — a cargo-installed
//! binary has no data directory). Built with the release preset; -O1
//! may inline std-internal calls, so overriding a std routine rebinds
//! direct user calls, not std's internal uses (semantic binding).

use std::sync::OnceLock;

use mtc_core::formats::object::ObjectFile;

use crate::compiler::{CompileOptions, compile};
use crate::optimizer::OptLevel;

pub const SOURCE: &str = include_str!("std.pmc");

pub fn object() -> &'static ObjectFile {
    static OBJECT: OnceLock<ObjectFile> = OnceLock::new();
    OBJECT.get_or_init(|| {
        compile(
            SOURCE,
            CompileOptions {
                opt_level: OptLevel::O1,
                strip_debugger: true,
                ..Default::default()
            },
        )
        .expect("the embedded stdlib compiles")
        .object
    })
}
```

Add `pub mod stdlib;` to `lib.rs`.

- [ ] **Step 3: write the tests (fail first), then make green**

`crates/post-machine/tests/stdlib_programs.rs`. Harness:

```rust
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunLimits, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::link;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::stdlib;

/// Compile `source`, link against std, run from `cells`/`head`; return
/// (marked cells, final head). Cells index from origin 0.
fn run_std(source: &str, cells: &[bool], head: i64) -> (Vec<i64>, i64) {
    let out = compile(source, CompileOptions::default()).expect("compiles");
    let linked = link(
        &[out.object],
        std::slice::from_ref(stdlib::object()),
        LinkOptions::default(),
    )
    .expect("links");
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(&linked.executable, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells(cells.iter().copied(), 0, head);
    let result = machine.run(
        &mut tape,
        RunOptions {
            limits: RunLimits { max_steps: Some(100_000), ..Default::default() },
            ..Default::default()
        },
    );
    assert_eq!(result.outcome, Outcome::Stopped, "program must stop normally");
    (tape.marked_cells(), tape.head())
}

const M: bool = true;
const B: bool = false;
```

Per-routine tests, each with the derived expectation (traces in comments; derive from the source, never from observation):

```rust
#[test]
fn go_to_end_lands_on_the_last_mark() {
    // {0,1,2} h0: right→1,2,3(blank)→left→2
    let (marks, head) = run_std(
        "use std::goToEnd; main() { @goToEnd(!); }",
        &[M, M, M],
        0,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], 2));
}

#[test]
fn go_to_begin_lands_on_the_first_mark() {
    let (marks, head) = run_std(
        "use std::goToBegin; main() { @goToBegin(!); }",
        &[M, M, M],
        2,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], 0));
}

#[test]
fn go_to_mark_right_finds_a_distant_mark() {
    // {4} h0: rights through 1..3, stops on 4
    let (marks, head) = run_std(
        "use std::goToMarkRight; main() { @goToMarkRight(!); }",
        &[B, B, B, B, M],
        0,
    );
    assert_eq!((marks, head), (vec![4], 4));
}

#[test]
fn go_to_mark_left_finds_a_distant_mark() {
    let (marks, head) = run_std(
        "use std::goToMarkLeft; main() { @goToMarkLeft(!); }",
        &[M, B, B, B],
        3,
    );
    assert_eq!((marks, head), (vec![0], 0));
}

#[test]
fn go_to_blank_right_exits_the_section() {
    let (marks, head) = run_std(
        "use std::goToBlankRight; main() { @goToBlankRight(!); }",
        &[M, M, M],
        0,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], 3));
}

#[test]
fn go_to_blank_left_exits_the_section() {
    let (marks, head) = run_std(
        "use std::goToBlankLeft; main() { @goToBlankLeft(!); }",
        &[M, M, M],
        2,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], -1));
}

#[test]
fn erase_section_clears_it_from_the_middle() {
    // {0..=3} h2: goToBegin→0; unmark,right ×4 → stops at 4
    let (marks, head) = run_std(
        "use std::eraseSection; main() { @eraseSection(!); }",
        &[M, M, M, M],
        2,
    );
    assert_eq!((marks, head), (vec![], 4));
}

#[test]
fn append_mark_grows_right() {
    // {0,1} h0: goToEnd→1; right→2; mark
    let (marks, head) = run_std(
        "use std::appendMark; main() { @appendMark(!); }",
        &[M, M],
        0,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], 2));
}

#[test]
fn prepend_mark_grows_left() {
    let (marks, head) = run_std(
        "use std::prependMark; main() { @prependMark(!); }",
        &[M, M],
        1,
    );
    assert_eq!((marks, head), (vec![-1, 0, 1], -1));
}

#[test]
fn remove_last_mark_shrinks_right() {
    // {0,1,2} h1: goToEnd→2; unmark; left→1
    let (marks, head) = run_std(
        "use std::removeLastMark; main() { @removeLastMark(!); }",
        &[M, M, M],
        1,
    );
    assert_eq!((marks, head), (vec![0, 1], 1));
}

#[test]
fn remove_first_mark_shrinks_left() {
    let (marks, head) = run_std(
        "use std::removeFirstMark; main() { @removeFirstMark(!); }",
        &[M, M, M],
        1,
    );
    assert_eq!((marks, head), (vec![1, 2], 1));
}

#[test]
fn remove_last_mark_on_a_single_mark_empties_the_tape() {
    // {0} h0: goToEnd stays (right→1 blank→left→0); unmark; left→-1
    let (marks, head) = run_std(
        "use std::removeLastMark; main() { @removeLastMark(!); }",
        &[M],
        0,
    );
    assert_eq!((marks, head), (vec![], -1));
}

#[test]
fn stdlib_compiles_clean_and_exports_exactly_the_roster() {
    use mtc_core::formats::object::SymbolDef;
    let out = compile(stdlib::SOURCE, CompileOptions::default()).expect("compiles");
    assert!(out.report.warnings.is_empty(), "{:?}", out.report.warnings);
    let mut names: Vec<&str> = stdlib::object()
        .symbols
        .iter()
        .filter(|s| matches!(s.def, SymbolDef::Defined { .. }))
        .map(|s| s.name.as_str())
        .collect();
    names.sort_unstable();
    let mut expected = vec![
        "std::appendMark", "std::eraseSection", "std::goToBegin",
        "std::goToBlankLeft", "std::goToBlankRight", "std::goToEnd",
        "std::goToMarkLeft", "std::goToMarkRight", "std::prependMark",
        "std::removeFirstMark", "std::removeLastMark",
    ];
    expected.sort_unstable();
    assert_eq!(names, expected);
}

#[test]
fn user_namespace_injection_overrides_a_std_routine() {
    // Spec §9 interposition: same-namespace export, user beats library.
    let (marks, head) = run_std(
        "namespace std { export goToEnd() { 1: left(!); } }\n\
         use std::goToEnd; main() { @goToEnd(!); }",
        &[M, M, M],
        1,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], 0)); // the override: one left
}
```

Adjust the exact `Symbol`/`SymbolDef` field access to `formats::object`'s real shape (`Symbol { name, def }` per Plan 6c) — if the shape differs, fix the TEST to read the real API; do not touch the format. `goToBlankLeft` trace check: from h2 `{0,1,2}`: left→1 marked→left→0 marked→left→-1 blank → return. Head −1 ✓.

- [ ] **Step 4: gates and commit**

```bash
git add -A crates/post-machine
git commit -m "feat(post-machine): the standard library — 11 std:: routines, embedded and release-built"
```

---

### Task 4: Golden end-to-end — the historic programs, diffed as `.pmt`

**Files:**
- Create: `crates/post-machine/tests/golden_programs.rs`, `crates/post-machine/tests/golden/sum.pmc`, `crates/post-machine/tests/golden/ty.pmc`, `crates/post-machine/tests/golden/sum.expected.pmt`, `crates/post-machine/tests/golden/ty.expected.pmt`, `crates/post-machine/tests/golden/ty_empty.expected.pmt`

**Interfaces:**
- Consumes: Task 3 `stdlib::object()`, `TapeBlockFile`/`TapeSnapshot`, `InfiniteTape::to_snapshot`, `arch::DEFAULT_GLYPHS`.
- Produces: the golden `.pmt` files Task 6's CLI e2e test reuses (`sum.expected.pmt`).

- [ ] **Step 1: port the historic sources**

`tests/golden/sum.pmc` — port of `Compiller/Sum.pms` (2002-era dialect: `if(a,b)` → `check(a,b)`, `add` → `mark`, `delete` → `unmark`, `@Algorithm` → `use std::…` + `@call`); unary addition of two sections separated by one blank (n is encoded as n+1 marks):

```c
// Port of the historic Sum.pms (Delphi generation A, Compiller/):
// adds the two unary numbers on the tape. Numbers are n+1 marks; input
// "a gap b" with the head on a's first mark; output one section a+b.
use std::goToEnd, std::goToBegin;

main() {
    1: @goToEnd();
    2: right;
    3: right;
    4: @goToEnd();
    5: unmark;
    6: left;
    7: @goToBegin();
    8: left;
    9: mark;
    10: @goToEnd();
    11: unmark;
    12: left;
    13: @goToBegin(!);
}
```

`tests/golden/ty.pmc` — port of `Compiller/Ty.pms` (explicit successors `right(2)` → line-numbered fall-throughs preserved as labels; `(!)` → return successor). Decrements a unary number; empty input passes through:

```c
// Port of the historic Ty.pms (Delphi generation A, Compiller/):
// removes one mark from the section under the head (unary decrement).
// The mark/goToEnd/unmark dance at 4-6 is preserved verbatim from the
// original — historic fidelity over minimality.
use std::goToEnd;
use std::goToBegin;

main() {
    1: check(2, !);
    2: @goToEnd();
    3: right;
    4: mark;
    5: @goToEnd();
    6: unmark;
    7: left;
    8: unmark;
    9: left;
    10: @goToBegin(!);
}
```

(One file uses the comma-list `use`, the other single-path statements — both forms exercised.)

- [ ] **Step 2: write the test with fully derived expectations**

Derivations (normative; the traces are the authority — if a run disagrees, BLOCK and re-derive, never adjust):

- **sum**, input marks {0,1,2} ∪ {4,5} (2+1 in n+1 encoding), head 0:
  goToEnd→2; right→3; right→4; goToEnd→5; unmark 5; left→4; goToBegin→4; left→3; mark 3; goToEnd→4; unmark 4; left→3; goToBegin→0; stop.
  **Final: marks {0,1,2,3}, head 0** (= 3 in n+1 encoding: 2+1 ✓). Snapshot: `origin 0, cells [1,1,1,1], head 0`.
- **ty**, input marks {0,1,2}, head 0:
  check→2; goToEnd→2; right→3; mark 3; goToEnd→3 (right→4 blank, left→3); unmark 3; left→2; unmark 2; left→1; goToBegin→0; stop.
  **Final: marks {0,1}, head 0.** Snapshot: `origin 0, cells [1,1], head 0`.
- **ty on empty tape**, head 0: check falls to `!` → stop immediately.
  **Final: no marks, head 0.** Snapshot (dense span = head): `origin 0, cells [0], head 0`.

`tests/golden_programs.rs`:

```rust
use std::fs;
use std::path::Path;

use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunLimits, RunOptions};
use mtc_post_machine::arch::{DEFAULT_GLYPHS, Pm1};
use mtc_post_machine::asm::link;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;
use mtc_post_machine::stdlib;

fn golden_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"))
}

fn build(pmc: &str, opt: OptLevel) -> mtc_core::formats::executable::Executable {
    let source = fs::read_to_string(golden_dir().join(pmc)).expect("golden source");
    let out = compile(
        &source,
        CompileOptions { opt_level: opt, ..Default::default() },
    )
    .expect("compiles");
    assert!(out.report.warnings.is_empty(), "{:?}", out.report.warnings);
    link(
        &[out.object],
        std::slice::from_ref(stdlib::object()),
        LinkOptions::default(),
    )
    .expect("links")
    .executable
}

fn run(exe: &mtc_core::formats::executable::Executable, cells: &[bool], head: i64) -> InfiniteTape {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells(cells.iter().copied(), 0, head);
    let result = machine.run(
        &mut tape,
        RunOptions {
            limits: RunLimits { max_steps: Some(1_000_000), ..Default::default() },
            ..Default::default()
        },
    );
    assert_eq!(result.outcome, Outcome::Stopped);
    tape
}

fn block(snapshot: TapeSnapshot) -> TapeBlockFile {
    TapeBlockFile {
        alphabet: DEFAULT_GLYPHS.iter().map(|g| g.to_string()).collect(),
        tapes: vec![snapshot],
    }
}

/// (source file, golden file, input cells, head, DERIVED final snapshot)
fn cases() -> Vec<(&'static str, &'static str, Vec<bool>, i64, TapeSnapshot)> {
    vec![
        (
            "sum.pmc",
            "sum.expected.pmt",
            vec![true, true, true, false, true, true],
            0,
            TapeSnapshot { origin: 0, cells: vec![1, 1, 1, 1], head: 0 },
        ),
        (
            "ty.pmc",
            "ty.expected.pmt",
            vec![true, true, true],
            0,
            TapeSnapshot { origin: 0, cells: vec![1, 1], head: 0 },
        ),
        (
            "ty.pmc",
            "ty_empty.expected.pmt",
            vec![],
            0,
            TapeSnapshot { origin: 0, cells: vec![0], head: 0 },
        ),
    ]
}

#[test]
fn goldens_match_the_derived_snapshots_and_files() {
    for (pmc, golden, cells, head, expected) in cases() {
        for opt in [OptLevel::O0, OptLevel::O1] {
            let tape = run(&build(pmc, opt), &cells, head);
            assert_eq!(tape.to_snapshot(), expected, "{pmc} at {opt:?}");
        }
        // the committed .pmt is byte-for-byte the derived block
        let bytes = fs::read(golden_dir().join(golden)).expect("golden .pmt present");
        assert_eq!(bytes, block(expected).to_bytes(), "{golden} drifted");
    }
}

// NOTE: no O1-shrinks assertion here — sum/ty's only optimizable code is
// `main`, where tail-call is exempt (tail_call.rs) and std is always built
// -O1; O0 and O1 user objects may be byte-identical. Shrink assertions
// live in opt_equivalence.rs where shrinkage is derived.

/// Regenerates the golden .pmt files FROM THE DERIVED SNAPSHOTS above
/// (never from run output — derivation-first).
/// cargo test -p mtc-post-machine --test golden_programs regen -- --ignored
#[test]
#[ignore = "writes the golden files; run explicitly"]
fn regen_goldens() {
    for (_, golden, _, _, expected) in cases() {
        fs::write(golden_dir().join(golden), block(expected).to_bytes()).unwrap();
    }
}
```

- [ ] **Step 3: generate the goldens, verify, commit**

Run `cargo test -p mtc-post-machine --test golden_programs regen_goldens -- --ignored`, then the full suite. `goldens_match…` must pass with the freshly written files. Sanity-render one: the bytes of `sum.expected.pmt` decode via `TapeBlockFile::from_bytes` to the derived struct (the round-trip property from Plan 1 guarantees it).

```bash
git add -A crates/post-machine/tests
git commit -m "test(post-machine): golden e2e — historic Sum/Ty ports through std, diffed as .pmt"
```

---

### Task 5: CLI part 1 — `pmt` binary, `compile` / `asm` / `link`

**Files:**
- Create: `crates/post-machine/src/cli/mod.rs`, `crates/post-machine/src/cli/build.rs`, `crates/post-machine/src/bin/pmt.rs`
- Modify: `crates/post-machine/src/lib.rs` (add `pub mod cli;`)
- Test: `crates/post-machine/tests/cli_programs.rs` (started here, extended in Task 6)

**Interfaces:**
- Consumes: everything public from earlier plans + `stdlib::object()`.
- Produces (Task 6 extends the same module):
  - `pub struct CliOutput { pub stdout: String, pub stderr: String, pub code: u8 }`
  - `pub fn execute(args: &[String]) -> Result<CliOutput, String>` — `Err` renders as `pmt: {msg}` with exit 1.
  - `pub(crate) struct Args` helper: `flag(&mut self, name: &str) -> bool`, `value(&mut self, name: &str) -> Result<Option<String>, String>`, `positionals(self) -> Result<Vec<String>, String>` (rejects leftover unknown `-`/`--` tokens by name in the error).

- [ ] **Step 1: bin + dispatch skeleton + failing tests**

`src/bin/pmt.rs`:

```rust
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match mtc_post_machine::cli::execute(&args) {
        Ok(out) => {
            print!("{}", out.stdout);
            eprint!("{}", out.stderr);
            ExitCode::from(out.code)
        }
        Err(message) => {
            eprintln!("pmt: {message}");
            ExitCode::FAILURE
        }
    }
}
```

`src/cli/mod.rs` — dispatch, `Args`, shared helpers (usage text is the CLI reference's skeleton; keep it in sync with `docs/cli.md` in Task 7):

```rust
//! The `pmt` command-line tool: a thin renderer over the library API.
//! Libraries never print; every byte of terminal output originates here.

mod build;

pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: u8,
}

impl CliOutput {
    pub(crate) fn ok(stdout: String, stderr: String) -> Self {
        Self { stdout, stderr, code: 0 }
    }
}

const USAGE: &str = "\
pmt — Post-machine toolchain

USAGE: pmt <SUBCOMMAND> [ARGS]

SUBCOMMANDS:
  compile  .pmc source -> .pmo object (-S for .pma, --emit-ir for CFG JSON)
  asm      .pma assembly -> .pmo object
  link     .pmo objects -> .pmx executable (+ .pmx.map sidecar)
  dis      disassemble a .pmo or .pmx (--listing for the address view)
  run      execute a .pmx on a tape
  tape     build/show .pmt tape-block snapshots
  ir       render --emit-ir JSON (ir graph -> Mermaid)

Run `pmt <SUBCOMMAND> --help` for details. `pmt --version` prints the version.
";

pub fn execute(args: &[String]) -> Result<CliOutput, String> {
    execute_with(args, &mut std::io::stderr().lock())
}

/// Writer seam (R10): `--trace` streams into `trace_out` live. The bin
/// path passes stderr; tests pass a `Vec<u8>` and assert on it.
pub fn execute_with(
    args: &[String],
    trace_out: &mut dyn std::io::Write,
) -> Result<CliOutput, String> {
    match args.first().map(String::as_str) {
        None | Some("--help") | Some("-h") => Ok(CliOutput::ok(USAGE.into(), String::new())),
        Some("--version") => Ok(CliOutput::ok(
            format!("pmt {}\n", env!("CARGO_PKG_VERSION")),
            String::new(),
        )),
        Some("compile") => build::compile(&args[1..]),
        Some("asm") => build::asm(&args[1..]),
        Some("link") => build::link(&args[1..]),
        Some("dis") => inspect::dis(&args[1..]),   // Task 6
        Some("tape") => inspect::tape(&args[1..]), // Task 6
        Some("ir") => inspect::ir(&args[1..]),     // Task 6
        Some("run") => run::run(&args[1..], trace_out), // Task 6
        Some(other) => Err(format!("unknown subcommand `{other}`\n\n{USAGE}")),
    }
}
```

(For this task, stub `dis`/`tape`/`ir`/`run` arms with `Err("implemented in Task 6".into())`? NO — leave the arms out entirely until Task 6; the match covers only Task 5's subcommands plus the catch-all.)

`Args` helper in `mod.rs`:

```rust
/// Minimal flag scanner: flags may appear anywhere; `--name value` and
/// `--name=value` are both accepted; remaining tokens are positionals.
pub(crate) struct Args {
    tokens: Vec<Option<String>>,
}

impl Args {
    pub(crate) fn new(args: &[String]) -> Self {
        Self { tokens: args.iter().cloned().map(Some).collect() }
    }

    /// Consume a boolean flag; true if present (first occurrence).
    pub(crate) fn flag(&mut self, name: &str) -> bool {
        for slot in &mut self.tokens {
            if slot.as_deref() == Some(name) {
                *slot = None;
                return true;
            }
        }
        false
    }

    /// Consume `name value` or `name=value`.
    pub(crate) fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        for i in 0..self.tokens.len() {
            let Some(tok) = self.tokens[i].as_deref() else { continue };
            if tok == name {
                self.tokens[i] = None;
                let next = self.tokens.get_mut(i + 1).and_then(Option::take);
                return next.ok_or_else(|| format!("{name} needs a value")).map(Some);
            }
            if let Some(rest) = tok.strip_prefix(&format!("{name}=")) {
                let value = rest.to_string();
                self.tokens[i] = None;
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    /// Consume every occurrence of a repeatable `name value` flag.
    pub(crate) fn values(&mut self, name: &str) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        while let Some(v) = self.value(name)? {
            out.push(v);
        }
        Ok(out)
    }

    /// Everything left must be positional (no dashed tokens).
    pub(crate) fn positionals(self) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for tok in self.tokens.into_iter().flatten() {
            if tok.starts_with('-') && tok != "-" {
                return Err(format!("unknown flag `{tok}`"));
            }
            out.push(tok);
        }
        Ok(out)
    }
}
```

Start `tests/cli_programs.rs` with the scaffold tests:

```rust
use std::fs;
use std::path::PathBuf;

use mtc_post_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn no_args_prints_usage() {
    let out = execute(&[]).unwrap();
    assert!(out.stdout.contains("USAGE: pmt"));
    assert_eq!(out.code, 0);
}

#[test]
fn unknown_subcommand_errors() {
    assert!(execute(&args(&["bogus"])).is_err());
}
```

- [ ] **Step 2: `compile` / `asm` / `link` in `cli/build.rs`**

```rust
//! Build-side subcommands: compile, asm, link.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::LinkOptions;

use crate::compiler::{CompileOptions, CompileReport, compile as compile_source};
use crate::optimizer::OptLevel;
use crate::stdlib;

use super::{Args, CliOutput};

const COMPILE_USAGE: &str = "\
USAGE: pmt compile INPUT.pmc [-o OUT.pmo] [FLAGS]

FLAGS:
  -g                 record debug info (labels + .pmc lines)
  -O0 | -O1          optimization level (default -O0)
  --strip-debugger   drop `brk` at codegen
  --debug            preset: -g -O0
  --release          preset: -O1 --strip-debugger
  -S                 emit the generated .pma instead of an object
  --emit-ir[=STAGE]  write the CFG IR JSON next to the output
                     (STAGE: lowered | after:<pass> | final; default final;
                      repeated stages resolve last-wins)
  --fno-<pass>       disable one optimizer pass (repeatable)
  -Werror            treat warnings as errors
  -v                 render the compile report (passes, rounds)
";

fn out_path(input: &Path, explicit: Option<String>, extension: &str) -> PathBuf {
    match explicit {
        Some(path) => PathBuf::from(path),
        None => input.with_extension(extension),
    }
}

fn render_warnings(stderr: &mut String, input: &Path, report: &CompileReport) {
    for w in &report.warnings {
        let _ = writeln!(stderr, "{}:{}: warning: {}", input.display(), w.line, w.message);
    }
}

fn render_opt_report(stderr: &mut String, report: &CompileReport) {
    let _ = writeln!(stderr, "opt: {} round(s)", report.opt.rounds);
    for change in &report.opt.changes {
        let _ = writeln!(
            stderr,
            "  {} {}: {} change(s)",
            change.pass, change.function, change.changes
        );
    }
}

pub(super) fn compile(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(COMPILE_USAGE.into(), String::new()));
    }
    let debug_preset = args.flag("--debug");
    let release_preset = args.flag("--release");
    let mut options = CompileOptions {
        debug_info: debug_preset || args.flag("-g"),
        strip_debugger: release_preset || args.flag("--strip-debugger"),
        opt_level: if release_preset { OptLevel::O1 } else { OptLevel::O0 },
        ..Default::default()
    };
    if args.flag("-O0") {
        options.opt_level = OptLevel::O0;
    }
    if args.flag("-O1") {
        options.opt_level = OptLevel::O1;
    }
    let emit_asm = args.flag("-S");
    let werror = args.flag("-Werror");
    let verbose = args.flag("-v");
    let emit_ir = take_emit_ir(&mut args)?;
    take_disabled_passes(&mut args, &mut options.disabled_passes);
    options.capture_ir = matches!(emit_ir, Some(Some(_)));
    let explicit_out = args.value("-o")?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("compile takes exactly one input\n\n{COMPILE_USAGE}"));
    };
    let input = Path::new(input);

    let source = fs::read_to_string(input)
        .map_err(|e| format!("cannot read {}: {e}", input.display()))?;
    // CompileError's own Display self-prefixes "line L:C:"; the CLI
    // renders the kind under its file:line:col prefix instead (ratified
    // fix 2026-07-06 — Task-5 implementer caught the doubled prefix).
    // CompileErrorKind gains its own Display in the same fix; CompileError's
    // Display delegates to it, output byte-identical.
    let out = compile_source(&source, options)
        .map_err(|e| format!("{}:{}:{}: error: {}", input.display(), e.line, e.col, e.kind))?;

    let mut stderr = String::new();
    render_warnings(&mut stderr, input, &out.report);
    if verbose {
        render_opt_report(&mut stderr, &out.report);
    }
    if werror && !out.report.warnings.is_empty() {
        return Err(format!(
            "{stderr}-Werror: {} warning(s) treated as errors",
            out.report.warnings.len()
        ));
    }

    let target = out_path(input, explicit_out, if emit_asm { "pma" } else { "pmo" });
    if emit_asm {
        fs::write(&target, &out.pma)
            .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    } else {
        fs::write(&target, out.object.to_bytes())
            .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    }

    if let Some(stage) = emit_ir {
        let ir_path = target.with_extension("ir.json");
        let json = match stage.as_deref() {
            None | Some("final") => out.ir.to_json(),
            Some(label) => out
                .ir_snapshots
                .iter()
                .rev() // last-wins (ruling R4)
                .find(|(l, _)| l == label)
                .map(|(_, program)| program.to_json())
                .ok_or_else(|| format!("no IR snapshot labeled `{label}` was captured"))?,
        };
        fs::write(&ir_path, json)
            .map_err(|e| format!("cannot write {}: {e}", ir_path.display()))?;
    }

    Ok(CliOutput::ok(String::new(), stderr))
}
```

`take_emit_ir` must NOT use `Args::value` (a bare `--emit-ir input.pmc` would eat the input as the flag's value). Bare token → default-stage form; `--emit-ir=STAGE` prefix → validated stage form:

```rust
/// `--emit-ir` → Some(None); `--emit-ir=STAGE` → Some(Some(stage)).
fn take_emit_ir(args: &mut Args) -> Result<Option<Option<String>>, String> {
    if args.flag("--emit-ir") {
        return Ok(Some(None));
    }
    for slot in &mut args.tokens {
        if let Some(tok) = slot.as_deref() {
            if let Some(stage) = tok.strip_prefix("--emit-ir=") {
                let stage = stage.to_string();
                *slot = None;
                let known = stage == "lowered"
                    || stage == "final"
                    || stage.starts_with("after:");
                if !known {
                    return Err(format!(
                        "unknown IR stage `{stage}` (lowered | after:<pass> | final)"
                    ));
                }
                return Ok(Some(Some(stage)));
            }
        }
    }
    Ok(None)
}
```

(Give `Args.tokens` `pub(super)` visibility, or add a `take_prefixed(&mut self, prefix: &str)` helper — implementer's choice, keep it in `mod.rs` next to `Args`.)

`--fno-<pass>`:

```rust
fn take_disabled_passes(args: &mut Args, disabled: &mut Vec<String>) {
    for slot in &mut args.tokens {
        if let Some(tok) = slot.as_deref() {
            if let Some(pass) = tok.strip_prefix("--fno-") {
                disabled.push(pass.to_string());
                *slot = None;
            }
        }
    }
}
```

(Unknown pass names already no-op harmlessly in `OptOptions.disabled`; do not validate here — the optimizer owns pass names.)

`asm`:

```rust
const ASM_USAGE: &str = "\
USAGE: pmt asm INPUT.pma [-o OUT.pmo] [-g]
";

pub(super) fn asm(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(ASM_USAGE.into(), String::new()));
    }
    let with_debug = args.flag("-g");
    let explicit_out = args.value("-o")?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("asm takes exactly one input\n\n{ASM_USAGE}"));
    };
    let input = Path::new(input);
    let source = fs::read_to_string(input)
        .map_err(|e| format!("cannot read {}: {e}", input.display()))?;
    let object = crate::asm::assemble(&source, with_debug)
        .map_err(|e| format!("{}: {e}", input.display()))?;
    let target = out_path(input, explicit_out, "pmo");
    fs::write(&target, object.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}
```

(Adjust `assemble`'s error rendering to the real `AsmError` Display — it carries its own line info.)

`link`:

```rust
const LINK_USAGE: &str = "\
USAGE: pmt link INPUT.pmo... [-o OUT.pmx] [FLAGS]

FLAGS:
  --no-relax    keep every symbol site in far form
  --nostdlib    do not link the built-in std
  -L DIR        add a library search directory (repeatable, in order)
  -l NAME       link NAME.pmo from the search path (repeatable)
  -v            render the link report (dropped functions, relaxation)

Writes OUT.pmx and the OUT.pmx.map sidecar (function ranges; label/line
info when the objects carry -g debug data).
";

pub(super) fn link(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LINK_USAGE.into(), String::new()));
    }
    let relax = !args.flag("--no-relax");
    let nostdlib = args.flag("--nostdlib");
    let verbose = args.flag("-v");
    let search_dirs = args.values("-L")?;
    let lib_names = args.values("-l")?;
    let explicit_out = args.value("-o")?;
    let inputs = args.positionals()?;
    if inputs.is_empty() {
        return Err(format!("link needs at least one object\n\n{LINK_USAGE}"));
    }

    let mut objects = Vec::new();
    for path in &inputs {
        objects.push(read_object(Path::new(path))?);
    }
    let mut libraries = Vec::new();
    for name in &lib_names {
        libraries.push(find_library(name, &search_dirs)?);
    }
    if !nostdlib {
        libraries.push(stdlib::object().clone());
    }

    let linked = crate::asm::link(&objects, &libraries, LinkOptions { relax })
        .map_err(|e| e.to_string())?;

    let target = out_path(Path::new(&inputs[0]), explicit_out, "pmx");
    fs::write(&target, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    let map_path = sidecar_path(&target);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    let mut stderr = String::new();
    if verbose {
        let r = &linked.report;
        let _ = writeln!(
            stderr,
            "link: dropped [{}]; {} site(s) relaxed short, {} far",
            r.dropped.join(", "),
            r.relaxed_calls,
            r.far_calls
        );
    }
    Ok(CliOutput::ok(String::new(), stderr))
}

/// `app.pmx` → `app.pmx.map` (spec-lineage §10: the sidecar keeps the
/// full executable name).
fn sidecar_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".map");
    PathBuf::from(s)
}

fn read_object(path: &Path) -> Result<ObjectFile, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    ObjectFile::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn find_library(name: &str, dirs: &[String]) -> Result<ObjectFile, String> {
    for dir in dirs {
        let candidate = Path::new(dir).join(format!("{name}.pmo"));
        if candidate.exists() {
            return read_object(&candidate);
        }
    }
    Err(format!("library `{name}` not found on the -L search path"))
}
```

(pm1 `crate::asm::link` wrapper signature is `link(objects, libraries, options)`; `assemble(source, with_debug)` — both confirmed. `ObjectFile::from_bytes`/`to_bytes` exist from Plan 1.)

- [ ] **Step 3: integration tests**

Extend `tests/cli_programs.rs` (fail first, then green):

```rust
const HELLO: &str = "main() { 1: mark; 2: right; 3: mark(!); }";

#[test]
fn compile_writes_an_object_and_link_writes_exe_and_map() {
    let dir = scratch("build_pipeline");
    let src = dir.join("hello.pmc");
    fs::write(&src, HELLO).unwrap();

    let out = execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    let obj = dir.join("hello.pmo");
    assert!(obj.exists());

    let exe = dir.join("hello.pmx");
    let out = execute(&args(&[
        "link", obj.to_str().unwrap(), "-o", exe.to_str().unwrap(), "-v",
    ]))
    .unwrap();
    assert!(exe.exists());
    assert!(dir.join("hello.pmx.map").exists());
    assert!(out.stderr.contains("link:"));
}

#[test]
fn compile_dash_s_emits_pma_and_asm_accepts_it() {
    let dir = scratch("s_roundtrip");
    let src = dir.join("hello.pmc");
    fs::write(&src, HELLO).unwrap();
    execute(&args(&["compile", src.to_str().unwrap(), "-S"])).unwrap();
    let pma = dir.join("hello.pma");
    assert!(pma.exists());
    execute(&args(&["asm", pma.to_str().unwrap()])).unwrap();
    assert!(dir.join("hello.pmo").exists());
}

#[test]
fn emit_ir_stage_last_wins_and_validates() {
    let dir = scratch("emit_ir");
    let src = dir.join("hello.pmc");
    fs::write(&src, HELLO).unwrap();
    execute(&args(&[
        "compile", src.to_str().unwrap(), "-O1", "--emit-ir=lowered",
    ]))
    .unwrap();
    let ir = fs::read_to_string(dir.join("hello.ir.json")).unwrap();
    assert!(ir.contains("\"version\": 3"));
    let err = execute(&args(&[
        "compile", src.to_str().unwrap(), "--emit-ir=bogus",
    ]))
    .unwrap_err();
    assert!(err.contains("unknown IR stage"));
}

#[test]
fn werror_fails_on_warnings() {
    let dir = scratch("werror");
    let src = dir.join("warny.pmc");
    // an unused non-exported helper → unused-function warning
    fs::write(&src, "helper() { 1: right(!); }\nmain() { 1: mark(!); }").unwrap();
    let ok = execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    assert!(ok.stderr.contains("warning"));
    let err = execute(&args(&["compile", src.to_str().unwrap(), "-Werror"])).unwrap_err();
    assert!(err.contains("-Werror"));
}

#[test]
fn nostdlib_makes_std_calls_unresolved() {
    let dir = scratch("nostdlib");
    let src = dir.join("uses_std.pmc");
    fs::write(&src, "use std::goToEnd; main() { @goToEnd(!); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    let obj = dir.join("uses_std.pmo");
    // with std (default): links
    execute(&args(&["link", obj.to_str().unwrap()])).unwrap();
    // without: unresolved
    let err = execute(&args(&["link", obj.to_str().unwrap(), "--nostdlib"])).unwrap_err();
    assert!(err.to_lowercase().contains("unresolved"));
}
```

(Adjust the unresolved-message assertion to `LinkError`'s real Display wording — read it, assert a stable substring.)

- [ ] **Step 4: gates and commit**

```bash
git add -A crates/post-machine
git commit -m "feat(cli): pmt binary — compile/asm/link with reports, presets, -Werror, -L/-l/--nostdlib"
```

---

### Task 6: CLI part 2 — `dis` / `run` / `tape` / `ir` (+ `--trace`)

**Files:**
- Create: `crates/post-machine/src/cli/inspect.rs`, `crates/post-machine/src/cli/run.rs`
- Modify: `crates/post-machine/src/cli/mod.rs` (wire the four arms + shared tape renderer), `crates/post-machine/src/ir.rs` (`to_mermaid`)
- Test: extend `crates/post-machine/tests/cli_programs.rs`; mermaid unit tests in `ir.rs`

**Interfaces:**
- Consumes: Task 1 `DebugSession`, Task 2 `listing_executable`/`listing_line`, Task 4 golden files, `formats::sniff`, `TapeBlockFile`, `StrictTape`, `MapFile`.
- Produces: `IrFunction::to_mermaid(&self) -> String`.

- [ ] **Step 1: shared tape rendering in `cli/mod.rs`**

```rust
/// Render one tape with its glyphs: the dense span line plus a caret
/// line under the head. Glyph 0 is blank by convention.
pub(crate) fn render_tape(snapshot: &TapeSnapshot, alphabet: &[String]) -> String {
    let glyph = |index: u8| -> &str {
        alphabet
            .get(usize::from(index))
            .map(String::as_str)
            .unwrap_or("?")
    };
    let mut cells_line = String::new();
    let mut caret_line = String::new();
    for (i, &cell) in snapshot.cells.iter().enumerate() {
        let g = glyph(cell);
        let here = snapshot.origin + i as i64 == snapshot.head;
        cells_line.push_str(g);
        caret_line.push_str(&if here { "^".repeat(g.chars().count().max(1)) }
                             else { " ".repeat(g.chars().count().max(1)) });
    }
    format!(
        "origin {}, head {}\n|{}|\n {}\n",
        snapshot.origin,
        snapshot.head,
        cells_line,
        caret_line.trim_end()
    )
}
```

(Cell borders: a single `|` at each end only — per-cell `|` separators would misalign the caret math; keep exactly this shape and pin it in a unit test: marks {0,2} head 2, glyphs `" "`/`"*"` → lines `|* *|` / `   ^`.)

- [ ] **Step 2: `cli/inspect.rs` — `dis`, `tape`, `ir`**

```rust
//! Inspection subcommands: dis, tape, ir.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::formats::{ContainerKind, sniff};
use mtc_core::linker::MapFile;

use crate::arch::DEFAULT_GLYPHS;
use crate::ir::IrProgram;

use super::{Args, CliOutput, render_tape};

const DIS_USAGE: &str = "\
USAGE: pmt dis FILE.pmo|FILE.pmx [--listing] [--map FILE.pmx.map]

Objects disassemble with real names from the symbol table. Executables
use the .pmx.map sidecar when present (FILE.pmx.map or --map), else
recursive-descent discovery (func_XXXX). --listing prints the debugger
code view: addresses + raw bytes, not reassembleable.
";

pub(super) fn dis(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(DIS_USAGE.into(), String::new()));
    }
    let listing = args.flag("--listing");
    let map_path = args.value("--map")?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("dis takes exactly one input\n\n{DIS_USAGE}"));
    };
    let path = Path::new(input);
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    match sniff(&bytes) {
        Some(ContainerKind::Object) => {
            if listing {
                return Err("--listing applies to executables only".into());
            }
            let obj = ObjectFile::from_bytes(&bytes).map_err(|e| e.to_string())?;
            Ok(CliOutput::ok(crate::asm::disassemble_object(&obj), String::new()))
        }
        Some(ContainerKind::Executable) => {
            let exe = Executable::from_bytes(&bytes).map_err(|e| e.to_string())?;
            let map = load_map(path, map_path)?;
            let text = if listing {
                crate::asm::listing_executable(&exe, map.as_ref())
            } else {
                match &map {
                    Some(m) => crate::asm::disassemble_executable_with_map(&exe, m),
                    None => crate::asm::disassemble_executable(&exe),
                }
            };
            Ok(CliOutput::ok(text, String::new()))
        }
        Some(ContainerKind::TapeBlock) => Err("that is a tape block — use `pmt tape show`".into()),
        None => Err(format!("{}: not a toolchain container", path.display())),
    }
}

/// Explicit --map wins; else FILE.pmx.map next to the input; a present
/// but unparsable explicit map is an error, an unparsable sidecar is
/// silently ignored (stale sidecars must not break plain dis).
fn load_map(exe_path: &Path, explicit: Option<String>) -> Result<Option<MapFile>, String> {
    if let Some(p) = explicit {
        let text = fs::read_to_string(&p).map_err(|e| format!("cannot read {p}: {e}"))?;
        return MapFile::from_json(&text).map(Some).map_err(|e| format!("{p}: {e}"));
    }
    let mut sidecar = exe_path.as_os_str().to_owned();
    sidecar.push(".map");
    let sidecar = PathBuf::from(sidecar);
    Ok(fs::read_to_string(&sidecar)
        .ok()
        .and_then(|text| MapFile::from_json(&text).ok()))
}

const TAPE_USAGE: &str = "\
USAGE: pmt tape build \" * * *\" [--head N] [-o OUT.pmt]
       pmt tape show FILE.pmt

build: cell characters are the PM-1 glyphs (space = blank, * = mark);
the leftmost character is cell 0. show: renders any .pmt with its own
alphabet.
";

pub(super) fn tape(raw: &[String]) -> Result<CliOutput, String> {
    match raw.first().map(String::as_str) {
        Some("build") => tape_build(&raw[1..]),
        Some("show") => tape_show(&raw[1..]),
        _ => Ok(CliOutput::ok(TAPE_USAGE.into(), String::new())),
    }
}

fn tape_build(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let head: i64 = match args.value("--head")? {
        Some(text) => text.parse().map_err(|_| format!("bad --head `{text}`"))?,
        None => 0,
    };
    let out = args.value("-o")?.unwrap_or_else(|| "tape.pmt".into());
    let inputs = args.positionals()?;
    let [pattern] = inputs.as_slice() else {
        return Err(format!("tape build takes exactly one pattern\n\n{TAPE_USAGE}"));
    };
    let cells: Vec<u8> = pattern
        .chars()
        .map(|c| match c {
            ' ' => Ok(0),
            '*' => Ok(1),
            other => Err(format!("bad cell character `{other}` (space or *)")),
        })
        .collect::<Result<_, _>>()?;
    let block = TapeBlockFile {
        alphabet: DEFAULT_GLYPHS.iter().map(|g| g.to_string()).collect(),
        tapes: vec![TapeSnapshot { origin: 0, cells, head }],
    };
    fs::write(&out, block.to_bytes()).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

fn tape_show(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("tape show takes exactly one file\n\n{TAPE_USAGE}"));
    };
    let bytes = fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let block = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{input}: {e}"))?;
    let mut out = format!("alphabet: {:?}\n", block.alphabet);
    for (i, tape) in block.tapes.iter().enumerate() {
        out.push_str(&format!("tape {i}: {}", render_tape(tape, &block.alphabet)));
    }
    Ok(CliOutput::ok(out, String::new()))
}

const IR_USAGE: &str = "\
USAGE: pmt ir graph FILE.ir.json [--function NAME]

Renders --emit-ir output as a Mermaid flowchart (one per function).
";

pub(super) fn ir(raw: &[String]) -> Result<CliOutput, String> {
    match raw.first().map(String::as_str) {
        Some("graph") => ir_graph(&raw[1..]),
        _ => Ok(CliOutput::ok(IR_USAGE.into(), String::new())),
    }
}

fn ir_graph(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let filter = args.value("--function")?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("ir graph takes exactly one file\n\n{IR_USAGE}"));
    };
    let text = fs::read_to_string(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let program = IrProgram::from_json(&text).map_err(|e| format!("{input}: {e}"))?;
    let mut out = String::new();
    for function in &program.functions {
        if filter.as_deref().is_some_and(|f| f != function.name) {
            continue;
        }
        out.push_str(&format!("%% {}\n{}\n", function.name, function.to_mermaid()));
    }
    if out.is_empty() {
        return Err(match filter {
            Some(f) => format!("no function `{f}` in {input}"),
            None => format!("{input}: no functions"),
        });
    }
    Ok(CliOutput::ok(out, String::new()))
}
```

- [ ] **Step 3: `IrFunction::to_mermaid` (the `toMermaid` tradition)**

In `ir.rs`, with unit tests first:

```rust
    /// Mermaid flowchart of the CFG (`pmt ir graph`). Node text: source
    /// labels, then ops, then a terminal marker for block-ending
    /// terminators; edges carry the check/goto semantics.
    pub fn to_mermaid(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("flowchart TD\n");
        for block in &self.blocks {
            let mut lines: Vec<String> = Vec::new();
            for &label in &block.labels {
                lines.push(format!("{label}:"));
            }
            for op in &block.ops {
                lines.push(match op {
                    IrOp::Lft { .. } => "lft".into(),
                    IrOp::Rgt { .. } => "rgt".into(),
                    IrOp::Wr { index, .. } => format!("wr {index}"),
                    IrOp::Brk { .. } => "brk".into(),
                    IrOp::Call { name, .. } => format!("call @{name}"),
                });
            }
            match &block.term {
                IrTerm::Return => lines.push("ret".into()),
                IrTerm::Halt => lines.push("hlt".into()),
                IrTerm::TailCall { name } => lines.push(format!("jmp @{name}")),
                IrTerm::FallThrough { .. } | IrTerm::Goto { .. } | IrTerm::Check { .. } => {}
            }
            if lines.is_empty() {
                lines.push("(empty)".into());
            }
            let _ = writeln!(out, "    B{}[\"{}\"]", block.id, lines.join("<br/>"));
        }
        for block in &self.blocks {
            match &block.term {
                IrTerm::FallThrough { to } => {
                    let _ = writeln!(out, "    B{} --> B{to}", block.id);
                }
                IrTerm::Goto { to } => {
                    let _ = writeln!(out, "    B{} -->|goto| B{to}", block.id);
                }
                IrTerm::Check { marked, blank } => {
                    let _ = writeln!(out, "    B{} -->|MF| B{marked}", block.id);
                    let _ = writeln!(out, "    B{} -->|!MF| B{blank}", block.id);
                }
                IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
            }
        }
        out
    }
```

Unit test: lower `"main() { 1: right; check(1, !); }"` (via `lexer`+`parser`+`lower` as other ir.rs tests do), assert the mermaid output contains `flowchart TD`, a `B0` node with `rgt`, `-->|MF|` and `-->|!MF|` edges, and a node whose text ends with `ret`.

- [ ] **Step 4: `cli/run.rs`**

```rust
//! `pmt run`: execute a .pmx on a tape; the sync front of the VM.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::linker::MapFile;
use mtc_core::vm::{
    ArchRegistry, DebugEvent, InfiniteTape, Machine, Outcome, RunLimits, RunOptions,
    StrictTape, TactProfile, Tape,
};

use crate::arch::{DEFAULT_GLYPHS, Pm1};

use super::{Args, CliOutput, render_tape};

const RUN_USAGE: &str = "\
USAGE: pmt run APP.pmx [FLAGS]

TAPE (default: empty, head 0):
  --tape-block IN.pmt        load the initial tape from a snapshot
  --tape \" * *\" [--head N]   build the initial tape inline
  --save-tape-block OUT.pmt  write the final tape as a snapshot

LIMITS AND SEMANTICS:
  --max-steps N       step budget (default 10000000)
  --no-step-limit     remove the step budget
  --max-tacts N       tact budget
  --strict-cells      trap on double-mark/double-unmark
  --tact-profile M,R,W  device costs (move,read,write; default 1,1,1)

OUTPUT:
  --trace             stream per-instruction listing lines to stderr,
                      live, each with post-state `; MF=<0|1> head=<n>`
  -v                  no extra effect yet (stats always print)

EXIT CODE: 0 stopped | 2 halted (hlt) | 3 trapped | 1 tool error.
";

const DEFAULT_MAX_STEPS: u64 = 10_000_000;

pub(super) fn run(
    raw: &[String],
    trace_out: &mut dyn std::io::Write,
) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(RUN_USAGE.into(), String::new()));
    }
    let trace = args.flag("--trace");
    // -v is accepted and currently a no-op (stats always print) — it must
    // be CONSUMED or positionals() rejects it (ratified 2026-07-06).
    let _verbose = args.flag("-v");
    let strict = args.flag("--strict-cells");
    let no_step_limit = args.flag("--no-step-limit");
    let max_steps = match args.value("--max-steps")? {
        Some(text) => Some(text.parse::<u64>().map_err(|_| format!("bad --max-steps `{text}`"))?),
        None => None,
    };
    let max_tacts = match args.value("--max-tacts")? {
        Some(text) => Some(text.parse::<u64>().map_err(|_| format!("bad --max-tacts `{text}`"))?),
        None => None,
    };
    let profile = match args.value("--tact-profile")? {
        Some(text) => parse_profile(&text)?,
        None => TactProfile::ELECTRONIC,
    };
    let tape_block = args.value("--tape-block")?;
    let tape_inline = args.value("--tape")?;
    let head: i64 = match args.value("--head")? {
        Some(text) => text.parse().map_err(|_| format!("bad --head `{text}`"))?,
        None => 0,
    };
    let save = args.value("--save-tape-block")?;
    let inputs = args.positionals()?;
    let [exe_path] = inputs.as_slice() else {
        return Err(format!("run takes exactly one executable\n\n{RUN_USAGE}"));
    };
    let exe_path = Path::new(exe_path);

    let bytes = fs::read(exe_path).map_err(|e| format!("cannot read {}: {e}", exe_path.display()))?;
    let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{}: {e}", exe_path.display()))?;

    let (mut tape, alphabet) = initial_tape(tape_block.as_deref(), tape_inline.as_deref(), head)?;

    let limits = RunLimits {
        max_steps: if no_step_limit { None } else { Some(max_steps.unwrap_or(DEFAULT_MAX_STEPS)) },
        max_tacts,
    };
    let options = RunOptions { profile, limits, ..Default::default() };

    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(&exe, &registry).map_err(|e| e.to_string())?;

    let map = super::inspect::sidecar_map(exe_path); // shared with dis (see step note)

    let stderr = String::new(); // trace is NOT buffered here (R10)
    // free fn, not a closure: a closure's Fn impl pins one lifetime for
    // its &mut argument, but this is called at two independent sites
    // (Task-6 finding, deviation ratified 2026-07-06)
    fn trace_to(trace: bool, w: &mut dyn std::io::Write) -> Option<&mut dyn std::io::Write> {
        if trace { Some(w) } else { None }
    }
    let (outcome, stats) = if strict {
        let mut wrapped = StrictTape::new(tape);
        let r = drive(&machine, &exe, &mut wrapped, options, trace_to(trace, trace_out), map.as_ref());
        tape = wrapped.into_inner();
        r
    } else {
        drive(&machine, &exe, &mut tape, options, trace_to(trace, trace_out), map.as_ref())
    };

    let snapshot = tape.to_snapshot();
    let mut stdout = String::new();
    let _ = writeln!(stdout, "outcome: {outcome:?}");
    let _ = writeln!(
        stdout,
        "steps {}, core tacts {}, stall tacts {} (total {})",
        stats.steps, stats.core_tacts, stats.stall_tacts, stats.total_tacts()
    );
    stdout.push_str(&render_tape(&snapshot, &alphabet));

    if let Some(out_path) = save {
        let block = TapeBlockFile { alphabet: alphabet.clone(), tapes: vec![snapshot] };
        fs::write(&out_path, block.to_bytes())
            .map_err(|e| format!("cannot write {out_path}: {e}"))?;
    }

    let code = match outcome {
        Outcome::Stopped => 0,
        Outcome::Halted => 2,
        Outcome::Trapped(_) => 3,
    };
    Ok(CliOutput { stdout, stderr, code })
}

fn parse_profile(text: &str) -> Result<TactProfile, String> {
    let parts: Vec<&str> = text.split(',').collect();
    let [m, r, w] = parts.as_slice() else {
        return Err(format!("bad --tact-profile `{text}` (want M,R,W)"));
    };
    let parse = |s: &str| s.trim().parse::<u32>().map_err(|_| format!("bad cost `{s}`"));
    Ok(TactProfile { move_cost: parse(m)?, read_cost: parse(r)?, write_cost: parse(w)? })
}

/// Initial tape + the alphabet used for rendering/saving: a loaded block
/// brings its own glyphs (PM-1 blocks hold exactly one tape); otherwise
/// the arch defaults.
fn initial_tape(
    block: Option<&str>,
    inline: Option<&str>,
    head: i64,
) -> Result<(InfiniteTape, Vec<String>), String> {
    if block.is_some() && inline.is_some() {
        return Err("--tape-block and --tape are mutually exclusive".into());
    }
    let default_alphabet: Vec<String> = DEFAULT_GLYPHS.iter().map(|g| g.to_string()).collect();
    if let Some(path) = block {
        let bytes = fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let file = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))?;
        let [snapshot] = file.tapes.as_slice() else {
            return Err(format!("{path}: PM-1 blocks hold exactly one tape"));
        };
        let tape = InfiniteTape::from_snapshot(snapshot).map_err(|e| format!("{path}: {e:?}"))?;
        return Ok((tape, file.alphabet));
    }
    if let Some(pattern) = inline {
        let cells: Result<Vec<bool>, String> = pattern
            .chars()
            .map(|c| match c {
                ' ' => Ok(false),
                '*' => Ok(true),
                other => Err(format!("bad cell character `{other}`")),
            })
            .collect();
        return Ok((InfiniteTape::from_cells(cells?, 0, head), default_alphabet));
    }
    Ok((InfiniteTape::new(), default_alphabet))
}

/// Plain run, or traced run: DebugSession stepping with one listing
/// line per executed instruction streamed LIVE to the writer, carrying
/// the post-execution state suffix (R10). The line is written after its
/// instruction retires so `MF`/`head` reflect that instruction's effect.
fn drive(
    machine: &Machine,
    exe: &Executable,
    tape: &mut dyn Tape,
    options: RunOptions,
    trace: Option<&mut dyn std::io::Write>,
    map: Option<&MapFile>,
) -> (Outcome, mtc_core::vm::RunStats) {
    let Some(w) = trace else {
        let result = machine.run(tape, options);
        return (result.outcome, result.stats);
    };
    let syntax = crate::asm::pm1_syntax();
    let resolve = |target: u32| -> Option<String> {
        let m = map?;
        m.functions.iter().find_map(|f| {
            if f.start == target {
                return Some(f.name.clone());
            }
            f.labels
                .iter()
                .find(|(_, a)| *a == target)
                .map(|(l, _)| format!("{}.{l}", f.name))
        })
    };
    let mut session = machine.debug(options);
    loop {
        let ip = session.ip();
        let event = session.step_in(tape);
        let (line, _) =
            mtc_core::asm::listing_line(&syntax, &exe.code, ip, &resolve);
        let _ = writeln!(
            w,
            "{line}  ; MF={} head={}",
            u8::from(session.mf()),
            tape.head()
        );
        match event {
            // A trap pause is terminal for a non-interactive trace: the
            // faulting line was just written — looping again would print
            // it twice via the Finished repeat (ratified 2026-07-06;
            // Task-6 implementer verified the doubling empirically).
            DebugEvent::Paused(PauseCause::Trap(_)) => {
                return (
                    session.finished().expect("trap pause implies finished"),
                    session.stats(),
                );
            }
            DebugEvent::Paused(_) => {}
            DebugEvent::Finished(outcome) => return (outcome, session.stats()),
        }
    }
}
```

Notes for the implementer:
- `sidecar_map`: extract the silent-sidecar half of Task 6 Step 2's `load_map` into a `pub(super) fn sidecar_map(exe_path: &Path) -> Option<MapFile>` in `inspect.rs` and reuse from both places.
- A trap pause (`Paused(Trap)`) ends the traced run immediately (see the terminal arm above) — the traced line count includes the faulting instruction exactly once.
- `vm/mod.rs` re-exports: `StrictTape` is NOT currently re-exported — extend to `pub use devices::{InfiniteTape, StrictTape, Tape};`. `Tape`, `Outcome`, `TactProfile`, and (from Task 1) `DebugSession`/`DebugEvent`/`PauseCause` are already importable as written.
- Wire the three `mod` declarations + match arms in `cli/mod.rs` (`mod inspect; mod run;`).

- [ ] **Step 5: integration tests**

Extend `tests/cli_programs.rs`:

```rust
#[test]
fn full_pipeline_reproduces_the_sum_golden() {
    let dir = scratch("pipeline_sum");
    let golden_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let src = dir.join("sum.pmc");
    fs::copy(golden_dir.join("sum.pmc"), &src).unwrap();

    execute(&args(&["compile", src.to_str().unwrap(), "-O1"])).unwrap();
    execute(&args(&["link", dir.join("sum.pmo").to_str().unwrap()])).unwrap();
    execute(&args(&[
        "tape", "build", "*** **", "-o", dir.join("in.pmt").to_str().unwrap(),
    ]))
    .unwrap();
    let out = execute(&args(&[
        "run",
        dir.join("sum.pmx").to_str().unwrap(),
        "--tape-block", dir.join("in.pmt").to_str().unwrap(),
        "--save-tape-block", dir.join("out.pmt").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("outcome: Stopped"));
    assert_eq!(
        fs::read(dir.join("out.pmt")).unwrap(),
        fs::read(golden_dir.join("sum.expected.pmt")).unwrap(),
    );
}

#[test]
fn exit_codes_distinguish_halt_and_trap() {
    let dir = scratch("exit_codes");
    let src = dir.join("h.pmc");
    fs::write(&src, "main() { 1: halt; }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("h.pmo").to_str().unwrap()])).unwrap();
    let out = execute(&args(&["run", dir.join("h.pmx").to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 2);

    // step-limit trap
    let src = dir.join("spin.pmc");
    fs::write(&src, "main() { 1: right(1); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("spin.pmo").to_str().unwrap()])).unwrap();
    let out = execute(&args(&[
        "run", dir.join("spin.pmx").to_str().unwrap(), "--max-steps", "100",
    ]))
    .unwrap();
    assert_eq!(out.code, 3);
    assert!(out.stdout.contains("StepLimit"));
}

#[test]
fn strict_cells_traps_double_mark() {
    let dir = scratch("strict");
    let src = dir.join("dbl.pmc");
    fs::write(&src, "main() { 1: mark; 2: mark(!); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("dbl.pmo").to_str().unwrap()])).unwrap();
    let ok = execute(&args(&["run", dir.join("dbl.pmx").to_str().unwrap()])).unwrap();
    assert_eq!(ok.code, 0); // permissive default
    let strict = execute(&args(&[
        "run", dir.join("dbl.pmx").to_str().unwrap(), "--strict-cells",
    ]))
    .unwrap();
    assert_eq!(strict.code, 3);
}

#[test]
fn trace_streams_lines_with_post_state_into_the_writer() {
    use mtc_post_machine::cli::execute_with;
    let dir = scratch("trace");
    let src = dir.join("t.pmc");
    fs::write(&src, "main() { 1: mark; 2: right(!); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("t.pmo").to_str().unwrap()])).unwrap();
    let mut trace = Vec::new();
    let out = execute_with(
        &args(&["run", dir.join("t.pmx").to_str().unwrap(), "--trace"]),
        &mut trace,
    )
    .unwrap();
    let text = String::from_utf8(trace).unwrap();
    // ent, wr, rgt, stp — one line each; blank tape latches MF=0 at load
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines[0].contains("ent") && lines[0].ends_with("; MF=0 head=0"), "{}", lines[0]);
    assert!(lines[1].contains("wr") && lines[1].ends_with("; MF=1 head=0"), "{}", lines[1]);
    assert!(lines[2].contains("rgt") && lines[2].ends_with("; MF=0 head=1"), "{}", lines[2]);
    assert!(lines[3].contains("stp") && lines[3].ends_with("; MF=0 head=1"), "{}", lines[3]);
    assert!(out.stderr.is_empty(), "trace must stream, not buffer");
}

#[test]
fn dis_listing_and_tape_show_render() {
    let dir = scratch("dis_listing");
    let src = dir.join("d.pmc");
    fs::write(&src, "main() { 1: mark(!); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap(), "-g"])).unwrap();
    execute(&args(&["link", dir.join("d.pmo").to_str().unwrap()])).unwrap();
    let out = execute(&args(&[
        "dis", dir.join("d.pmx").to_str().unwrap(), "--listing",
    ]))
    .unwrap();
    assert!(out.stdout.starts_with("main:"), "{}", out.stdout);
    assert!(out.stdout.contains("0000:"));

    execute(&args(&["tape", "build", " **", "-o", dir.join("s.pmt").to_str().unwrap()])).unwrap();
    let shown = execute(&args(&["tape", "show", dir.join("s.pmt").to_str().unwrap()])).unwrap();
    assert!(shown.stdout.contains("| **|"), "{}", shown.stdout);
}

#[test]
fn ir_graph_renders_mermaid() {
    let dir = scratch("ir_graph");
    let src = dir.join("g.pmc");
    fs::write(&src, "main() { 1: right; 2: check(1, !); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap(), "--emit-ir"])).unwrap();
    let out = execute(&args(&[
        "ir", "graph", dir.join("g.ir.json").to_str().unwrap(),
    ]))
    .unwrap();
    assert!(out.stdout.contains("flowchart TD"));
    assert!(out.stdout.contains("-->|MF|"));
}
```

Trace derivation for `trace_streams…`: `main` compiles to `ent; wr 1; rgt; stp` — 4 lines, each written AFTER its instruction retires, so the suffix is that instruction's post-state: load latches MF=0 (blank tape); `ent` changes nothing (MF=0 head=0); `wr 1` marks and latches (MF=1 head=0); `rgt` moves onto blank (MF=0 head=1); `stp` terminal (state unchanged). If codegen emits different bytes, re-derive — do not relax the assertions to observation.

- [ ] **Step 6: gates and commit**

```bash
git add -A crates/post-machine
git commit -m "feat(cli): dis/run/tape/ir — listing view, traced runs, strict cells, tape snapshots, Mermaid CFGs"
```

---

### Task 7: Documentation, reference migration, spec freeze

**Files:**
- Create: `docs/language.md`, `docs/isa.md`, `docs/formats.md`, `docs/cli.md`, `docs/stdlib.md`, `docs/history.md`
- Modify: `README.md` (rewrite), `docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md` (freeze banner ONLY), code comments across `crates/` (reference migration)

**Interfaces:** none produced; consumes everything. This task freezes the design spec — after it, code cites only durable docs.

- [ ] **Step 1: write the six documentation pages**

Each page is prose written from the spec, the ledger notes, and the shipped code — the spec section is the source, but the page must describe what SHIPPED (v1), not design history. Required content per page (each bullet is a MUST-cover):

**`docs/language.md`** — the `.pmc` reference:
- Program structure: functions, statements `label: command(successor);`, comma groups, builtins (`left right mark unmark halt debugger`), `check(marked, blank)`, `@calls`, successors (number / `!` = return / fall-through), `main` return = program stop.
- Visibility & modularity (from spec §3.4): hidden-by-default, contextual `export`, bare top-level `main` auto-exports; nested functions (flat code, scoped callability, `.`-mangling); `namespace ns { }` (open, nestable, `::`-mangling, name-pool rule); `use path [as alias][, …];` scoping/duplicate rules; qualified calls `@ns::name()` (absolute, self-declaring); the three warnings and `-Werror`.
- Symbol grammar: `std::api.helper` self-decomposition (namespace before last `::`, nesting after).
- Optimization from the user's view: `-O0`/`-O1`, the observable-equivalence guarantee (final tape, termination kind, MF-dependent branches; resource traps excluded; `brk` barrier), `--fno-<pass>` names, interposition (semantic binding, `--fno-inline` for interposable libraries).
- The IR artifact: `--emit-ir` stages, version, last-wins; `pmt ir graph`.

**`docs/isa.md`** — PM-1:
- Registers, buses, sans-I/O core, loading, the §5 opcode table verbatim, `ent` verification, short forms and relaxation.
- Timing model: tact accounting, wait states, core vs stall tacts, profiles.
- Execution: `stp` vs `hlt`, the trap list, `RunResult` fields (ip/stack), DebugSession (pause causes, depth-based stepping).

**`docs/formats.md`** — containers:
- `.pmo` (symbols incl. Local kind, relocations/holes, version 2 accepting 1..=2), `.pmx`, `.pmt` (alphabet ownership: glyphs live ONLY tape-side; `Pm1::DEFAULT_GLYPHS` fallback), `.pma` (grammar, canonical grid, symbol jumps `jmp @name`, `.func name local`, labels colon-free), `.pmx.map` sidecar JSON, IR JSON (version 3, tags).
- Magic/sniffing; CRC; byte layouts as tables.

**`docs/cli.md`** — mirror of every usage string from Tasks 5–6, plus: presets, exit codes (0/1/2/3), `-v` report semantics, sidecar discovery rules, `--listing` vs canonical dis, `--trace` format, defaults (`--max-steps` 10,000,000).

**`docs/stdlib.md`** — the roster table: for each of the 11 routines, signature, precondition, postcondition (R3 wording from `std.pmc` comments verbatim), termination condition, `--strict-cells` notes; linking semantics (implicit std, `--nostdlib`, lazy reachability, overriding via `namespace std { export … }`, semantic-binding caveat R2).

**`docs/history.md`** — the lineage page:
- The four Delphi generations (2002–2012) and what each contributed: the language lineage (`.pms` → `.pmc`), fall-through optimization, `ent`-style call safety the 2007 stack lacked, the PMProcessor disassembler-first mindset.
- Generation D's `AF/BF/EF` flags + `ja/jb/je` jump family: edge/topology conditionals for bounded tapes, never wired (`UpdateFLAGS` stub), superseded by the infinite tape + DeviceFault-on-edge + device-agnostic principle (ledger P2a note).
- Abnormal-stop lineage (ledger note, verbatim facts): 2007/2012 Delphi and both JS generations had only normal stop; `turing-machine-js` haltState-in-subroutine means return; abnormal ends were host JS exceptions; PM-1 `hlt` is the first program-initiated abnormal stop, born from hardware honesty (fault register + HALT line); a matching `abortState` sentinel is being designed for the JS `turing-machine-js` library (name the feature, NOT its issue number — published docs carry substance, never tracker references); the 2-symbol Post machine has no in-band error channel — termination kind IS its only free output channel, which is why the equivalence contract observes it.
- The historic programs: `Sum.pms`/`Ty.pms` and their golden ports.
- A link to the frozen design spec as the historical design record.

**`README.md`** rewrite: what this is (one paragraph incl. the 2002 lineage, link to history.md), build (`cargo build --release`, the `pmt` binary path), quick start (the sum example end-to-end: 5 shell commands — compile, link, tape build, run, dis), the docs index, workspace layout, test invocation. No forge URLs.

- [ ] **Step 2: reference migration and freeze**

1. `grep -rn "spec §" crates/` — for every hit, rewrite the citation to its durable home, keeping the topic keyword: §3/§3.4 → `docs/language.md`; §4.x → `docs/isa.md`; §5 → `docs/isa.md` (opcode table); §6.x → `docs/formats.md`; §7/§7.1 → `docs/language.md` (IR) or `docs/formats.md` (IR JSON) by context; §8 → the equivalence-contract statement in `optimizer/mod.rs` module docs (internal contract — make that doc header self-contained if any citation leans on the spec for the contract's content); §9 → `docs/stdlib.md` (linking/interposition) or `docs/formats.md` (symbols) by context; §10 → `docs/cli.md`. Convention: `docs/language.md (visibility)` — path + topic keyword, no anchors (anchor names are less durable than filenames).
2. Also sweep `crates/` for `spec-lineage §` markers introduced by Tasks 1–6 of this plan — same treatment.
3. Verify: `grep -rn "spec §\|spec-lineage §" crates/` returns nothing.
4. Prepend to the design spec (title line stays first):

```markdown
> **FROZEN — historical design record (2026-07-06).** This document was
> the build-time authority through Plan 7. The durable references now
> live in `README.md` and `docs/` (language, ISA, formats, CLI, stdlib);
> code no longer cites this file, and it is no longer amended. For the
> project's lineage see `docs/history.md`.
```

5. The internal-invariant carve-out (user ruling): module-doc invariants (MF-coupling in `optimizer/dataflow.rs`, closed-terminator-targets, pipeline-order constraint) STAY in module docs — verify each is self-contained (reads correctly with the spec frozen), amend the module doc in place where it isn't.

- [ ] **Step 3: docs cross-check**

Self-check pass (implementer, no subagent): every CLI flag in `cli/*.rs` usage strings appears in `docs/cli.md`; every stdlib routine in `std.pmc` appears in `docs/stdlib.md` with matching pre/post text; the §5 table in `docs/isa.md` matches `pm1_syntax()` entry-for-entry; `docs/formats.md` version numbers match `FORMAT_VERSION` (1), `OBJECT_FORMAT_VERSION` (2), `IR_VERSION` (3). Fix mismatches in the DOCS (code is frozen truth).

- [ ] **Step 4: gates and commit**

`cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check` (docs don't compile, but the migration touched code comments — the suite must stay green).

```bash
git add -A
git commit -m "docs: durable reference set (language/ISA/formats/CLI/stdlib/history), reference migration, design-spec freeze"
```

---

### Task 8: Full comment audit — staleness, citation hygiene, doc back-fill

**Files:**
- Modify: every `crates/**/*.rs` file (COMMENTS ONLY — zero behavior change), plus `docs/*.md` / `README.md` where gaps are found

**Interfaces:** none. Depends on Task 7 (the docs must exist to be linked and extended). Boundary with Task 7 Step 2: that step mechanically migrated the *spec-§ citations*; this task audits EVERY comment in the workspace.

**The rule being enforced (user ruling 2026-07-06):** code comments may reference only `docs/<page>.md` (+ a topic keyword) and `README.md`. When a comment carries information the docs lack, the information moves INTO the docs and the comment shrinks to the reference; the docs are the single source of truth, comments are pointers plus local constraints.

- [ ] **Step 1: file-by-file sweep**

Walk every `.rs` file under `crates/` (use `git ls-files 'crates/**/*.rs'` as the checklist; tick each file off in the report). For every comment — module docs (`//!`), doc comments (`///`), and inline (`//`):

1. **Staleness:** does the comment still describe the shipped code? Fix or delete drifted ones (known ledgered examples to catch: any surviving "each pass is `fn(&IrFunction)`"-style leftovers; scratch comments in tests). When unsure whether a comment is stale, verify against the code — never delete on suspicion.
2. **Citation hygiene:** the only legal reference targets are `docs/<page>.md (topic)` and `README.md`. Rewrite or remove anything else: leftover `spec §` / `spec-lineage §` forms, references to plan documents, the ledger, `docs/superpowers/`, ruling numbers (R1…R10 must not leak into code), issue/PR numbers, provider URLs.
3. **Doc back-fill:** if a comment explains user-facing behavior (format details, CLI semantics, language rules, contract guarantees) that its target doc page does NOT cover, EXTEND the doc page with that content, then shrink the comment to the reference plus at most one line of local context.
4. **The carve-out stands:** implementation-internal invariants (the MF-coupling argument in `optimizer/dataflow.rs`, closed-terminator-targets, the tail-call-before-tail-merge ordering, `Core` phase-machine notes) STAY as self-contained module docs — they are contracts between passes, not user documentation, and they need no external reference at all.
5. **Default is keep-and-tighten.** This is an audit, not a purge: a correct, load-bearing comment that cites nothing needs no change. Delete only what is provably stale or fully duplicated by a doc page it can reference instead.

- [ ] **Step 2: mechanical verification gates**

All of these must come back empty:

```bash
grep -rn "spec §\|spec-lineage §" crates/
grep -rn "docs/superpowers" crates/
grep -rniE "https?://" crates/ --include="*.rs"
grep -rnE "(issue|PR) #?[0-9]+|#[0-9]{2,}\b" crates/ --include="*.rs"
```

(The last pattern will false-positive on things like hex or attribute-like text — inspect hits, don't blind-fix.) And every citation target must exist:

```bash
grep -rhoE "docs/[a-z-]+\.md" crates/ --include="*.rs" | sort -u | while read f; do [ -f "$f" ] || echo "MISSING: $f"; done
```

- [ ] **Step 3: prove the diff is comments-only**

`cargo test --workspace` green, clippy, fmt — AND the reviewer verifies the diff contains no non-comment source changes (no code tokens outside comment context in any hunk). If a stale comment revealed an actual code problem, do NOT fix it here — record it in the report for the final review.

- [ ] **Step 4: commit**

```bash
git add -A
git commit -m "docs(comments): full comment audit — stale text removed, citations normalized to docs/ + README, doc gaps back-filled"
```

---

## Final review & merge

After Task 8: run `scripts/review-package MERGE_BASE HEAD` and dispatch the whole-branch final review on the most capable model, with the Minor roll-up from the ledger (6c leftovers: interposition-test residue comment, `imports.clone()` in flatten, reserved-word import error kind, `is_symbol_name` looseness) attached for triage. Then `superpowers:finishing-a-development-branch`: ff-merge `plan-7-cli-stdlib-docs` to master. Post-merge (controller, outside the repo): update `machines/CLAUDE.md` and the project memory; schedule the v2 brainstorm over the parking lot as the next session's opening move.
