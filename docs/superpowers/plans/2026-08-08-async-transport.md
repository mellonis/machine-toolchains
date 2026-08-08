# Async Transport (Plan 1 of the volatile/async/footprint round) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A poll-shaped, non-blocking execution surface in `mtc-core` — async device trait, `SyncAsAsync` adapter, shipped `LatencyTape` device, `AsyncSession` with `pump()` — plus a `no_std` vm build, bit-exact sync ≡ pump equivalence, and the `docs/core.md` rewrite.

**Architecture:** The core (`Core::start`/`resume` → `CoreEvent`) is already a poll-shaped state machine with its continuation in the private `Pending` type; this plan adds a session that serves bus requests the way `driver::step_instruction` does, except device requests go through a new `issue`/`poll` device trait, and a not-ready device suspends the session between `pump()` calls instead of blocking. The sync surface (`driver.rs`, `debug.rs`, `core.rs`, `bus.rs`) is not modified (single allowed diff: adding `Clone` to `RunResult`'s derive if absent).

**Tech Stack:** Rust (edition 2024), no new dependencies. Spec: `docs/superpowers/specs/2026-08-08-volatile-async-footprint-design.md` §3 (internal artifact — do NOT cite it in code comments; cite `docs/core.md (async session)` etc. instead).

## Global Constraints

- **No new dependencies.** `serde`/`serde_json` become *optional* (tied to the `std` feature); nothing is added.
- **`BusResponse`, `CoreEvent`, `core.rs` are untouched.** An unexpected response variant would silently trap in 16 catch-all arms — the design forbids extending the bus protocol. Waiting is resolved in the session, outside the core.
- **`driver.rs` / `debug.rs` behavior is untouched.** Existing tests must pass unmodified. Only allowed diff outside new files: derives, exports, `#[cfg]`/import mechanics for `no_std`, and the two `Machine` constructors.
- **Pending polls never tick the tact counter.** A transaction's whole cost arrives as one number in `Ready { cost }`; `cost: None` means the model price from `TactProfile`.
- **Device slice contract mirrors `DebugSession::step_in_tapes`:** the caller supplies `tape_count` devices per `pump` call; a missing device answers `Fault(NoSuchDevice)` (never a panic).
- **Comments cite durable pages only** (`docs/core.md (async session)`, `docs/core.md (loading)`); never `spec §N`, never issue numbers.
- **Commit style:** conventional with scope — `feat(core):`, `test(post-machine):`, `docs(core):`.
- **Quality gates for every task:** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. From Task 9 on, also `cargo build -p mtc-core --no-default-features`.
- Core test convention: unit tests live inline in the source file's `#[cfg(test)] mod tests` (the crate-private `test_arch` at `crates/core/src/vm/arch.rs` is only reachable from inline tests). Integration tests per crate under `tests/` define their own local helpers — there is no shared test-support module.

---

### Task 1: Async device trait + `SyncAsAsync` adapter

**Files:**
- Create: `crates/core/src/vm/devices/async_device.rs`
- Modify: `crates/core/src/vm/devices/mod.rs` (module + re-exports)
- Modify: `crates/core/src/vm/mod.rs:18` (extend the `devices` re-export line)

**Interfaces:**
- Consumes: `Tape` trait (`devices/mod.rs:17-25`), `DeviceFault` (`vm/trap.rs:4-9`).
- Produces (later tasks rely on these exact shapes):
  ```rust
  pub enum DeviceCmd   { MoveLeft, MoveRight, Read, Write { index: u32 } }
  pub enum DeviceReply { Ok, Symbol(u32), Fault(DeviceFault) }
  pub enum DevicePoll  { Pending, Ready { reply: DeviceReply, cost: Option<u32> } }
  pub trait AsyncTapeDevice {
      fn alphabet_size(&self) -> u32;
      fn head(&self) -> i64;
      fn issue(&mut self, cmd: DeviceCmd);
      fn poll(&mut self) -> DevicePoll;
  }
  pub struct SyncAsAsync<T: Tape> { /* private */ }
  impl<T: Tape> SyncAsAsync<T> { pub fn new(inner: T) -> Self; pub fn into_inner(self) -> T; pub fn get_ref(&self) -> &T; }
  ```

- [ ] **Step 1: Write the failing tests** (inline `#[cfg(test)] mod tests` at the bottom of the new file)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::devices::InfiniteTape;

    #[test]
    fn adapter_executes_each_command_ready_immediately() {
        let mut dev = SyncAsAsync::new(InfiniteTape::new());
        dev.issue(DeviceCmd::Write { index: 1 });
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready { reply: DeviceReply::Ok, cost: None }
        );
        dev.issue(DeviceCmd::Read);
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready { reply: DeviceReply::Symbol(1), cost: None }
        );
        dev.issue(DeviceCmd::MoveRight);
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready { reply: DeviceReply::Ok, cost: None }
        );
        assert_eq!(dev.head(), 1);
        dev.issue(DeviceCmd::MoveLeft);
        dev.poll();
        assert_eq!(dev.head(), 0);
    }

    #[test]
    fn adapter_surfaces_device_faults_as_replies() {
        let mut dev = SyncAsAsync::new(InfiniteTape::new());
        dev.issue(DeviceCmd::Write { index: 7 }); // outside the two-symbol alphabet
        let DevicePoll::Ready { reply, cost } = dev.poll() else {
            panic!("adapter is always ready");
        };
        assert_eq!(cost, None);
        assert!(matches!(reply, DeviceReply::Fault(_)));
    }

    #[test]
    fn poll_without_issue_is_pending() {
        let mut dev = SyncAsAsync::new(InfiniteTape::new());
        assert_eq!(dev.poll(), DevicePoll::Pending);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core adapter_executes -- --nocapture`
Expected: compile FAIL (module does not exist yet).

- [ ] **Step 3: Implement the module**

`crates/core/src/vm/devices/async_device.rs`:

```rust
//! The async device surface (docs/core.md (async session)): a poll-shaped
//! mirror of the bus protocol's device requests. The WAIT/READY reading:
//! `issue` puts the command on the bus; each `poll` samples the READY line
//! on a clock edge — `Pending` is READY low, `Ready` is READY high with
//! the data latched.

use super::Tape;
use crate::vm::trap::DeviceFault;

/// One device transaction, as the bus sees it (minus the device index —
/// a device object IS one device).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCmd {
    MoveLeft,
    MoveRight,
    Read,
    Write { index: u32 },
}

/// The data half of a completed transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceReply {
    Ok,
    Symbol(u32),
    Fault(DeviceFault),
}

/// One READY-line sample. `cost` is the transaction's price in tacts:
/// `None` means "price me at the `TactProfile` model cost"; `Some(n)` is
/// the device's own measurement (a real device's wait is real machine
/// time). The whole transaction's cost arrives as this one number —
/// `Pending` samples never tick the tact counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    Ready { reply: DeviceReply, cost: Option<u32> },
}

/// A tape device the machine can genuinely wait on. Contract: one command
/// in flight per device — `issue` while a command is pending is a caller
/// bug; `poll` with nothing in flight reports `Pending`.
pub trait AsyncTapeDevice {
    fn alphabet_size(&self) -> u32;
    fn head(&self) -> i64;
    fn issue(&mut self, cmd: DeviceCmd);
    fn poll(&mut self) -> DevicePoll;
}

/// Any sync `Tape` as an always-ready async device, priced at the model
/// cost (`cost: None`). This adapter is what makes a pumped run of an
/// in-memory program bit-identical to a sync run.
#[derive(Debug)]
pub struct SyncAsAsync<T: Tape> {
    inner: T,
    pending: Option<DeviceCmd>,
}

impl<T: Tape> SyncAsAsync<T> {
    pub fn new(inner: T) -> Self {
        Self { inner, pending: None }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    pub fn get_ref(&self) -> &T {
        &self.inner
    }
}

/// Execute one command against a sync tape. Shared with `LatencyTape`.
pub(crate) fn execute_on_tape<T: Tape>(tape: &mut T, cmd: DeviceCmd) -> DeviceReply {
    match cmd {
        DeviceCmd::MoveLeft => {
            tape.left();
            DeviceReply::Ok
        }
        DeviceCmd::MoveRight => {
            tape.right();
            DeviceReply::Ok
        }
        DeviceCmd::Read => DeviceReply::Symbol(tape.read()),
        DeviceCmd::Write { index } => match tape.write(index) {
            Ok(()) => DeviceReply::Ok,
            Err(fault) => DeviceReply::Fault(fault),
        },
    }
}

impl<T: Tape> AsyncTapeDevice for SyncAsAsync<T> {
    fn alphabet_size(&self) -> u32 {
        self.inner.alphabet_size()
    }

    fn head(&self) -> i64 {
        self.inner.head()
    }

    fn issue(&mut self, cmd: DeviceCmd) {
        debug_assert!(self.pending.is_none(), "one command in flight per device");
        self.pending = Some(cmd);
    }

    fn poll(&mut self) -> DevicePoll {
        match self.pending.take() {
            None => DevicePoll::Pending,
            Some(cmd) => DevicePoll::Ready {
                reply: execute_on_tape(&mut self.inner, cmd),
                cost: None,
            },
        }
    }
}
```

`crates/core/src/vm/devices/mod.rs` — add after the existing `mod` lines and `pub use` lines:

```rust
mod async_device;
pub use async_device::{AsyncTapeDevice, DeviceCmd, DevicePoll, DeviceReply, SyncAsAsync};
```

`crates/core/src/vm/mod.rs` — extend the devices re-export:

```rust
pub use devices::{
    AsyncTapeDevice, DeviceCmd, DevicePoll, DeviceReply, InfiniteTape, StrictTape, SyncAsAsync,
    Tape, WideTape,
};
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p mtc-core async_device`
Expected: 3 PASS.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo test -p mtc-core`
Then:

```bash
git add crates/core/src/vm/devices/async_device.rs crates/core/src/vm/devices/mod.rs crates/core/src/vm/mod.rs
git commit -m "feat(core): poll-shaped async device trait with a sync-tape adapter"
```

---

### Task 2: `LatencyTape` — the shipped async device

**Files:**
- Create: `crates/core/src/vm/devices/latency_tape.rs`
- Modify: `crates/core/src/vm/devices/mod.rs`, `crates/core/src/vm/mod.rs` (re-exports: `LatencyProfile`, `LatencyTape`)

**Interfaces:**
- Consumes: `AsyncTapeDevice`, `DeviceCmd`, `DevicePoll`, `DeviceReply`, `execute_on_tape` (Task 1), `Tape`.
- Produces:
  ```rust
  pub struct LatencyProfile {
      pub move_polls: u32, pub read_polls: u32, pub write_polls: u32,
      pub move_cost: u32,  pub read_cost: u32,  pub write_cost: u32,
  }
  impl LatencyProfile { pub const IMMEDIATE_ELECTRONIC: LatencyProfile; }
  pub struct LatencyTape<T: Tape> { /* private */ }
  impl<T: Tape> LatencyTape<T> { pub fn new(inner: T, profile: LatencyProfile) -> Self; pub fn into_inner(self) -> T; }
  ```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::devices::{DeviceCmd, DevicePoll, DeviceReply, InfiniteTape};

    const TWO_POLLS: LatencyProfile = LatencyProfile {
        move_polls: 2,
        read_polls: 2,
        write_polls: 2,
        move_cost: 3,
        read_cost: 4,
        write_cost: 5,
    };

    #[test]
    fn stays_pending_for_the_configured_polls_then_completes_with_cost() {
        let mut dev = LatencyTape::new(InfiniteTape::new(), TWO_POLLS);
        dev.issue(DeviceCmd::Write { index: 1 });
        assert_eq!(dev.poll(), DevicePoll::Pending);
        assert_eq!(dev.poll(), DevicePoll::Pending);
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready { reply: DeviceReply::Ok, cost: Some(5) }
        );
        // The write really landed:
        dev.issue(DeviceCmd::Read);
        dev.poll();
        dev.poll();
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready { reply: DeviceReply::Symbol(1), cost: Some(4) }
        );
    }

    #[test]
    fn immediate_electronic_is_ready_at_once_with_unit_costs() {
        let mut dev = LatencyTape::new(InfiniteTape::new(), LatencyProfile::IMMEDIATE_ELECTRONIC);
        dev.issue(DeviceCmd::MoveRight);
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready { reply: DeviceReply::Ok, cost: Some(1) }
        );
    }

    #[test]
    fn poll_without_issue_is_pending() {
        let mut dev = LatencyTape::new(InfiniteTape::new(), TWO_POLLS);
        assert_eq!(dev.poll(), DevicePoll::Pending);
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p mtc-core latency_tape` → compile FAIL.

- [ ] **Step 3: Implement**

```rust
//! A latency decorator over any sync tape (docs/core.md (async session)):
//! the `StrictTape` pattern applied to time. Holds READY low for a
//! configured number of polls per operation, then performs the operation
//! and reports a configured cost. The reference implementation of the
//! async device contract, the transport counterpart of a mechanical
//! `TactProfile`, and the test vehicle for the pumped session.

use super::async_device::{execute_on_tape, AsyncTapeDevice, DeviceCmd, DevicePoll};
use super::Tape;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyProfile {
    pub move_polls: u32,
    pub read_polls: u32,
    pub write_polls: u32,
    pub move_cost: u32,
    pub read_cost: u32,
    pub write_cost: u32,
}

impl LatencyProfile {
    /// Ready immediately, priced like the electronic `TactProfile` — the
    /// configuration under which a pumped run matches a sync run bit-exactly.
    pub const IMMEDIATE_ELECTRONIC: LatencyProfile = LatencyProfile {
        move_polls: 0,
        read_polls: 0,
        write_polls: 0,
        move_cost: 1,
        read_cost: 1,
        write_cost: 1,
    };

    fn polls_for(&self, cmd: DeviceCmd) -> u32 {
        match cmd {
            DeviceCmd::MoveLeft | DeviceCmd::MoveRight => self.move_polls,
            DeviceCmd::Read => self.read_polls,
            DeviceCmd::Write { .. } => self.write_polls,
        }
    }

    fn cost_for(&self, cmd: DeviceCmd) -> u32 {
        match cmd {
            DeviceCmd::MoveLeft | DeviceCmd::MoveRight => self.move_cost,
            DeviceCmd::Read => self.read_cost,
            DeviceCmd::Write { .. } => self.write_cost,
        }
    }
}

#[derive(Debug)]
pub struct LatencyTape<T: Tape> {
    inner: T,
    profile: LatencyProfile,
    in_flight: Option<(DeviceCmd, u32)>, // (command, polls remaining)
}

impl<T: Tape> LatencyTape<T> {
    pub fn new(inner: T, profile: LatencyProfile) -> Self {
        Self { inner, profile, in_flight: None }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Tape> AsyncTapeDevice for LatencyTape<T> {
    fn alphabet_size(&self) -> u32 {
        self.inner.alphabet_size()
    }

    fn head(&self) -> i64 {
        self.inner.head()
    }

    fn issue(&mut self, cmd: DeviceCmd) {
        debug_assert!(self.in_flight.is_none(), "one command in flight per device");
        self.in_flight = Some((cmd, self.profile.polls_for(cmd)));
    }

    fn poll(&mut self) -> DevicePoll {
        let Some((cmd, remaining)) = self.in_flight else {
            return DevicePoll::Pending;
        };
        if remaining > 0 {
            self.in_flight = Some((cmd, remaining - 1));
            return DevicePoll::Pending;
        }
        self.in_flight = None;
        DevicePoll::Ready {
            reply: execute_on_tape(&mut self.inner, cmd),
            cost: Some(self.profile.cost_for(cmd)),
        }
    }
}
```

Add `mod latency_tape;` + `pub use latency_tape::{LatencyProfile, LatencyTape};` in `devices/mod.rs`; add both names to the `vm/mod.rs` devices re-export.

- [ ] **Step 4: Run tests** — `cargo test -p mtc-core latency_tape` → 3 PASS.
- [ ] **Step 5: Gates + commit**

```bash
git add crates/core/src/vm/devices/latency_tape.rs crates/core/src/vm/devices/mod.rs crates/core/src/vm/mod.rs
git commit -m "feat(core): LatencyTape — the shipped waiting device, StrictTape pattern applied to time"
```

---

### Task 3: `AsyncSession` skeleton — always-ready pump, budget

**Files:**
- Create: `crates/core/src/vm/session.rs`
- Modify: `crates/core/src/vm/mod.rs` (`pub mod session;` + `pub use session::{AsyncSession, PumpEvent};`)
- Modify: `crates/core/src/vm/driver.rs:94` — ensure `RunResult` derives `Clone` (add if absent; keep `Debug, PartialEq, Eq` as found)

**Interfaces:**
- Consumes: `Core` (`start`/`resume`/`ip`/`instr_start`/`set_mf`), `BusRequest`/`BusResponse`/`CoreEvent`, `ReturnStack`, `RunStats`, `RunLimits`, `TactProfile`, `Outcome`, `RunResult`, `Trap`, `PauseCause` (from `debug.rs`), `AsyncTapeDevice` family (Task 1).
- Produces:
  ```rust
  pub enum PumpEvent { DeviceWait, BudgetSpent, Paused(PauseCause), Finished(RunResult) }
  pub struct AsyncSession<'a> { /* private */ }
  impl<'a> AsyncSession<'a> {
      pub fn new(core: Core<'a>, code: Vec<u8>, stack: ReturnStack,
                 profile: TactProfile, limits: RunLimits) -> Self;
      pub fn with_tables(self, tables: Vec<u8>) -> Self;   // clears the initial-mark latch, like DebugSession
      pub fn pump(&mut self, devices: &mut [&mut dyn AsyncTapeDevice],
                  budget: Option<u64>) -> PumpEvent;
  }
  ```
  (Debug controls and accessors arrive in Task 5; the latch in Task 6 — `new` sets `latch_initial_mark: true` from birth, but this task's tests construct with `with_tables` or pre-latched cores so the latch path is dormant until Task 6 implements it. To keep Task 3 self-contained, implement the latch FIELD and `with_tables` clearing now, and have `pump` treat a set latch flag as a real device-0 read transaction — the full test for it lands in Task 6.)

The serving loop mirrors `driver::step_instruction` (`crates/core/src/vm/driver.rs:116-238`) arm by arm — same accounting, same limit checks, same order. Read that function before writing this one. The one divergence: device requests go through `issue`/`poll`, and a `Pending` poll suspends.

- [ ] **Step 1: Write the failing tests** (inline; use the crate-private test arch exactly as `driver.rs`'s own tests do — open `crates/core/src/vm/driver.rs:276-360` and reuse its program-building helpers' style)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::arch::test_arch;
    use crate::vm::devices::{InfiniteTape, SyncAsAsync};
    use crate::vm::{driver, Core, ReturnStack};

    // Mirror driver.rs's test program builder: the fake arch's opcodes are
    // documented at vm/arch.rs::test_arch. Reuse the same byte sequences
    // driver.rs tests run, so sync and pumped runs execute identical images.
    // (Copy the smallest complete program driver.rs uses: write-move-stop.)

    fn sync_result(code: &[u8]) -> crate::vm::RunResult {
        let arch = test_arch::TestArch; // module with a unit struct, not a function
        let mut core = Core::new(&arch, 0);
        let mut stack = ReturnStack::new(16);
        let mut tape = InfiniteTape::new();
        driver::run(
            &mut core,
            code,
            &mut stack,
            &mut [&mut tape],
            &[],
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
    }

    fn pumped_result(code: &[u8], budget: Option<u64>) -> (crate::vm::RunResult, u32) {
        let arch = test_arch::TestArch; // module with a unit struct, not a function
        let core = Core::new(&arch, 0);
        let mut session = AsyncSession::new(
            core,
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new()); // multi-tape shape: no initial latch
        let mut tape = SyncAsAsync::new(InfiniteTape::new());
        let mut pumps = 0;
        loop {
            pumps += 1;
            match session.pump(&mut [&mut tape], budget) {
                PumpEvent::Finished(result) => return (result, pumps),
                PumpEvent::BudgetSpent => continue,
                other => panic!("unexpected event on an always-ready device: {other:?}"),
            }
        }
    }

    #[test]
    fn pumped_run_matches_sync_run_bit_exactly() {
        let code = /* the write-move-stop program bytes from driver.rs tests */;
        let sync = sync_result(&code);
        let (pumped, _) = pumped_result(&code, None);
        assert_eq!(pumped, sync); // outcome, stats, ip, stack — all of it
    }

    #[test]
    fn budget_chunks_execution_without_changing_the_result() {
        let code = /* same program */;
        let sync = sync_result(&code);
        let (pumped, pumps) = pumped_result(&code, Some(1));
        assert_eq!(pumped, sync);
        assert!(pumps as u64 >= sync.stats.steps); // one instruction per pump
    }

    #[test]
    fn zero_budget_returns_budget_spent_without_advancing() {
        let code = /* same program */;
        let arch = test_arch::TestArch; // module with a unit struct, not a function
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        let mut tape = SyncAsAsync::new(InfiniteTape::new());
        assert!(matches!(
            session.pump(&mut [&mut tape], Some(0)),
            PumpEvent::BudgetSpent
        ));
        assert_eq!(session_stats_steps(&session), 0); // expose via stats() in Task 5; here read the field
    }

    #[test]
    fn missing_device_faults_instead_of_panicking() {
        let code = /* the write-move-stop program */;
        let arch = test_arch::TestArch; // module with a unit struct, not a function
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        // No devices at all: the first device request must resolve to a
        // Device trap (Fault(NoSuchDevice) through the core), not a panic.
        let outcome = loop {
            match session.pump(&mut [], None) {
                PumpEvent::Finished(result) => break result.outcome,
                PumpEvent::BudgetSpent => continue,
                other => panic!("unexpected: {other:?}"),
            }
        };
        assert!(matches!(outcome, Outcome::Trapped(_)));
    }
}
```

The `/* program bytes */` placeholders above are to be filled from `driver.rs`'s own test programs — copy a complete byte sequence from there verbatim (it is the same fake arch); do not invent opcodes. `session_stats_steps` is shorthand for reading the session's stats field directly within the module.

- [ ] **Step 2: Run to verify failure** — `cargo test -p mtc-core session` → compile FAIL.

- [ ] **Step 3: Implement `session.rs`**

```rust
//! The pumped execution session (docs/core.md (async session)): the sync
//! driver's serving loop, re-hosted so a device that is not READY suspends
//! the session between `pump` calls instead of blocking the thread. The
//! embedder owns the loop — in hardware the clock generator pumps the
//! processor; here the embedder's `pump` calls play the clock edges.

use alloc_or_std_imports; // resolved concretely in Task 9; until then plain `std` paths

use super::bus::{BusRequest, BusResponse, CoreEvent};
use super::core::Core;
use super::debug::PauseCause;
use super::devices::{AsyncTapeDevice, DeviceCmd, DevicePoll, DeviceReply};
use super::driver::{Outcome, ReturnStack, RunLimits, RunResult, RunStats, TactProfile};
use super::trap::{DeviceFault, Trap};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpEvent {
    /// A device held READY low; nothing advanced past it. Pump again later.
    DeviceWait,
    /// The per-call instruction budget was spent.
    BudgetSpent,
    /// Instruction-boundary pause: breakpoint, `brk`, or external `pause()`.
    Paused(PauseCause),
    /// Terminal: the program stopped, halted, or trapped.
    Finished(RunResult),
}

/// What the session is waiting on between `pump` calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waiting {
    /// Nothing in flight; next `pump` continues at an instruction boundary.
    None,
    /// The initial-mark loading read on device 0 (docs/core.md (loading)).
    Latch,
    /// A mid-instruction device transaction: the unresumed bus request.
    Exec(BusRequest),
}

pub struct AsyncSession<'a> {
    core: Core<'a>,
    code: Vec<u8>,
    stack: ReturnStack,
    tables: Vec<u8>,
    stats: RunStats,
    profile: TactProfile,
    limits: RunLimits,
    breakpoints: BTreeSet<u32>,
    started: bool,
    latch_initial_mark: bool,
    pause_requested: bool,
    waiting: Waiting,
    finished: Option<RunResult>,
}

impl<'a> AsyncSession<'a> {
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
            tables: Vec::new(),
            stats: RunStats::default(),
            profile,
            limits,
            breakpoints: BTreeSet::new(),
            started: false,
            latch_initial_mark: true,
            pause_requested: false,
            waiting: Waiting::None,
            finished: None,
        }
    }

    /// Multi-tape shape: carries the table ROM and does not preload the
    /// mark, mirroring `DebugSession::with_tables`.
    pub fn with_tables(mut self, tables: Vec<u8>) -> Self {
        self.tables = tables;
        self.latch_initial_mark = false;
        self
    }

    fn over_tacts(&self) -> bool {
        self.limits
            .max_tacts
            .is_some_and(|max| self.stats.total_tacts() >= max)
    }

    fn finish(&mut self, outcome: Outcome) -> PumpEvent {
        let result = RunResult {
            outcome,
            stats: self.stats,
            ip: self.core.instr_start(),
            stack: self.stack.entries().to_vec(),
        };
        self.finished = Some(result.clone());
        PumpEvent::Finished(result)
    }

    /// Map a device-side bus request to a command + model price. Only the
    /// four device variants reach this.
    fn device_parts(&self, request: BusRequest) -> (u8, DeviceCmd, u32) {
        match request {
            BusRequest::DeviceMoveLeft { dev } => (dev, DeviceCmd::MoveLeft, self.profile.move_cost),
            BusRequest::DeviceMoveRight { dev } => {
                (dev, DeviceCmd::MoveRight, self.profile.move_cost)
            }
            BusRequest::DeviceRead { dev } => (dev, DeviceCmd::Read, self.profile.read_cost),
            BusRequest::DeviceWrite { dev, index } => {
                (dev, DeviceCmd::Write { index }, self.profile.write_cost)
            }
            _ => unreachable!("device_parts is called for device requests only"),
        }
    }

    /// Account a completed transaction and translate the reply for the
    /// core. Mirrors the sync driver's pricing: successful transactions
    /// pay `cost` (device-reported, else the model price); faulted ones
    /// pay nothing (the sync write-fault arm adds no stall tacts).
    fn settle_device(&mut self, model_cost: u32, reply: DeviceReply, cost: Option<u32>) -> BusResponse {
        match reply {
            DeviceReply::Ok => {
                self.stats.stall_tacts += u64::from(cost.unwrap_or(model_cost));
                BusResponse::Ok
            }
            DeviceReply::Symbol(symbol) => {
                self.stats.stall_tacts += u64::from(cost.unwrap_or(model_cost));
                BusResponse::Symbol(symbol)
            }
            DeviceReply::Fault(fault) => BusResponse::Fault(fault),
        }
    }

    /// Begin (or, for `already_issued`, re-sample) a device transaction.
    /// `Some(response)` completes it; `None` means READY is low.
    fn poll_device(
        &mut self,
        request: BusRequest,
        devices: &mut [&mut dyn AsyncTapeDevice],
        already_issued: bool,
    ) -> Option<BusResponse> {
        let (dev, cmd, model_cost) = self.device_parts(request);
        let Some(device) = devices.get_mut(dev as usize) else {
            return Some(BusResponse::Fault(DeviceFault::NoSuchDevice { dev }));
        };
        if !already_issued {
            device.issue(cmd);
        }
        match device.poll() {
            DevicePoll::Pending => None,
            DevicePoll::Ready { reply, cost } => Some(self.settle_device(model_cost, reply, cost)),
        }
    }

    pub fn pump(
        &mut self,
        devices: &mut [&mut dyn AsyncTapeDevice],
        budget: Option<u64>,
    ) -> PumpEvent {
        if let Some(result) = &self.finished {
            return PumpEvent::Finished(result.clone());
        }
        // A zero budget spends immediately — checked BEFORE the waiting
        // match, or the resume path would retire an instruction and
        // underflow the u64 decrement (plan fix after Task 3 review).
        if budget == Some(0) {
            return PumpEvent::BudgetSpent;
        }
        // Loading step (docs/core.md (loading)): on the async path the
        // initial-mark latch is a real device-0 transaction — still
        // unaccounted, and itself subject to WAIT. Implemented in Task 6;
        // until then `latch_initial_mark` is always false here.
        let mut remaining = budget;
        // Resume a suspended mid-instruction transaction, if any.
        let mut event: Option<CoreEvent> = match self.waiting {
            Waiting::None => None,
            Waiting::Latch => return self.pump_latch(devices, budget), // Task 6
            Waiting::Exec(request) => match self.poll_device(request, devices, true) {
                None => return PumpEvent::DeviceWait,
                Some(response) => {
                    self.waiting = Waiting::None;
                    if self.over_tacts() {
                        return self.finish(Outcome::Trapped(Trap::TactLimit));
                    }
                    Some(self.core.resume(response))
                }
            },
        };
        loop {
            // Instruction boundary (only when nothing is mid-flight).
            if event.is_none() {
                if self.latch_initial_mark {
                    return self.pump_latch(devices, budget); // Task 6
                }
                event = Some(if self.started {
                    self.core.resume(BusResponse::Ok) // ack the StepAck phase
                } else {
                    self.started = true;
                    self.core.start()
                });
            }
            // Serve until the next boundary — the sync driver's loop
            // (driver.rs (step_instruction)), device arms re-routed.
            let boundary_is_break = loop {
                match event.take().expect("event is set in this arm") {
                    CoreEvent::Request(request) => {
                        let response = match request {
                            BusRequest::CodeRead { addr } => match self.code.get(addr as usize) {
                                Some(&byte) => {
                                    self.stats.core_tacts += 1;
                                    BusResponse::Byte(byte)
                                }
                                None => BusResponse::OutOfCode,
                            },
                            BusRequest::StackPush { value } => {
                                if self.stack.push(value) {
                                    self.stats.core_tacts += 1;
                                    BusResponse::Ok
                                } else {
                                    BusResponse::StackFull
                                }
                            }
                            BusRequest::StackPop => match self.stack.pop() {
                                Some(value) => {
                                    self.stats.core_tacts += 1;
                                    BusResponse::Value(value)
                                }
                                None => BusResponse::StackEmpty,
                            },
                            BusRequest::TableRead { addr } => match self.tables.get(addr as usize) {
                                Some(&byte) => {
                                    self.stats.stall_tacts +=
                                        u64::from(self.profile.table_read_cost);
                                    BusResponse::Byte(byte)
                                }
                                None => BusResponse::OutOfTable,
                            },
                            BusRequest::FrameRead { addr } => match self.tables.get(addr as usize) {
                                Some(&byte) => {
                                    self.stats.stall_tacts +=
                                        u64::from(self.profile.frame_load_cost);
                                    BusResponse::Byte(byte)
                                }
                                None => BusResponse::OutOfTable,
                            },
                            device_request @ (BusRequest::DeviceMoveLeft { .. }
                            | BusRequest::DeviceMoveRight { .. }
                            | BusRequest::DeviceRead { .. }
                            | BusRequest::DeviceWrite { .. }) => {
                                match self.poll_device(device_request, devices, false) {
                                    None => {
                                        self.waiting = Waiting::Exec(device_request);
                                        return PumpEvent::DeviceWait;
                                    }
                                    Some(response) => response,
                                }
                            }
                        };
                        if self.over_tacts() {
                            return self.finish(Outcome::Trapped(Trap::TactLimit));
                        }
                        event = Some(self.core.resume(response));
                    }
                    boundary @ (CoreEvent::Step | CoreEvent::Break) => {
                        self.stats.steps += 1;
                        self.stats.core_tacts += 1; // execute base (docs/core.md (timing model))
                        if self.limits.max_steps.is_some_and(|max| self.stats.steps >= max) {
                            return self.finish(Outcome::Trapped(Trap::StepLimit));
                        }
                        if self.over_tacts() {
                            return self.finish(Outcome::Trapped(Trap::TactLimit));
                        }
                        break matches!(boundary, CoreEvent::Break);
                    }
                    CoreEvent::Stopped => return self.finish(Outcome::Stopped),
                    CoreEvent::Halted => return self.finish(Outcome::Halted),
                    CoreEvent::Trapped(trap) => return self.finish(Outcome::Trapped(trap)),
                }
            };
            // Instruction retired.
            if boundary_is_break {
                return PumpEvent::Paused(PauseCause::Brk);
            }
            if self.pause_requested {
                self.pause_requested = false;
                return PumpEvent::Paused(PauseCause::Manual);
            }
            if self.breakpoints.contains(&self.core.ip()) {
                return PumpEvent::Paused(PauseCause::Breakpoint(self.core.ip()));
            }
            if let Some(budget_left) = &mut remaining {
                *budget_left -= 1;
                if *budget_left == 0 {
                    return PumpEvent::BudgetSpent;
                }
            }
        }
    }

    fn pump_latch(
        &mut self,
        _devices: &mut [&mut dyn AsyncTapeDevice],
        _budget: Option<u64>,
    ) -> PumpEvent {
        unimplemented!("Task 6") // replaced there; unreachable until latch flag can be set
    }
}
```

Notes for the implementer:
- The `use alloc_or_std_imports;` line is a marker, not code — write plain `std`-path imports now (`std::collections::BTreeSet`); Task 9 sweeps them to `alloc`.
- `Waiting::Latch` and `pump_latch` are declared now so the type is complete, but until Task 6 nothing sets `latch_initial_mark` on a constructed-with-`with_tables` session, so tests here never reach them. Task 3 tests must all construct via `.with_tables(...)`.
- `RunResult` must be `Clone` — check `driver.rs:94`; if the derive lacks `Clone`, add it (behavior-neutral).
- Order of checks at the boundary (break > pause > breakpoint > budget) is the contract; keep it and keep the tests aligned.

- [ ] **Step 4: Run tests** — `cargo test -p mtc-core session` → 4 PASS. Also `cargo test -p mtc-core` (nothing else broke).
- [ ] **Step 5: Gates + commit**

```bash
git add crates/core/src/vm/session.rs crates/core/src/vm/mod.rs crates/core/src/vm/driver.rs
git commit -m "feat(core): AsyncSession — pumped execution over async devices, sync-parity serving loop"
```

---

### Task 4: Device waiting and cost accounting

**Files:**
- Modify: `crates/core/src/vm/session.rs` (tests only — the mechanics landed in Task 3)

**Interfaces:** consumes Tasks 1-3. Produces: none new — this task PROVES the waiting path.

- [ ] **Step 1: Write the failing tests** (append to `session.rs` tests)

```rust
#[test]
fn latency_device_suspends_and_resumes_mid_instruction() {
    // Same program as Task 3; device: LatencyTape with 2 polls per op.
    // Expect: DeviceWait exactly (polls) times per device transaction —
    // one READY sample per pump — then the same final result.
    let code = /* write-move-stop program bytes */;
    let sync = sync_result(&code);
    let arch = test_arch::TestArch; // module with a unit struct, not a function
    let mut session = AsyncSession::new(
        Core::new(&arch, 0),
        code.to_vec(),
        ReturnStack::new(16),
        TactProfile::ELECTRONIC,
        RunLimits::default(),
    )
    .with_tables(Vec::new());
    let profile = LatencyProfile {
        move_polls: 2, read_polls: 2, write_polls: 2,
        move_cost: 1, read_cost: 1, write_cost: 1, // electronic prices
    };
    let mut tape = LatencyTape::new(InfiniteTape::new(), profile);
    let mut waits = 0;
    let result = loop {
        match session.pump(&mut [&mut tape], None) {
            PumpEvent::Finished(result) => break result,
            PumpEvent::DeviceWait => waits += 1,
            other => panic!("unexpected: {other:?}"),
        }
    };
    assert_eq!(result, sync); // identical result AND stats: pending polls don't tick
    assert!(waits > 0);
}

#[test]
fn device_reported_cost_replaces_the_model_price() {
    // LatencyTape with cost 7 per write, 0 polls: stall_tacts must reflect
    // 7 per write transaction where the sync run pays 1.
    let code = /* write-move-stop program bytes */;
    let sync = sync_result(&code);
    /* build session as above */
    let profile = LatencyProfile {
        move_polls: 0, read_polls: 0, write_polls: 0,
        move_cost: 1, read_cost: 1, write_cost: 7,
    };
    /* pump to completion */
    let writes = /* count of write transactions in the program — derive from the program: it has exactly N writes */;
    assert_eq!(
        result.stats.stall_tacts,
        sync.stats.stall_tacts + (7 - 1) * writes
    );
    assert_eq!(result.outcome, sync.outcome);
}
```

Fill the `/* … */` from the actual program chosen in Task 3 (its write count is known by construction — count the write opcodes in the byte sequence).

- [ ] **Step 2: Run to verify the tests fail or pass honestly** — these should PASS if Task 3 was faithful; a failure here is a Task 3 bug. Investigate any failure — do not adjust expectations to make it pass. `cargo test -p mtc-core session`.
- [ ] **Step 3: Commit**

```bash
git add crates/core/src/vm/session.rs
git commit -m "test(core): waiting-path and cost-reporting proofs for the pumped session"
```

---

### Task 5: Debug controls and accessors

**Files:**
- Modify: `crates/core/src/vm/session.rs`

**Interfaces:**
- Produces (mirror `DebugSession`'s accessor block at `debug.rs:92-141`):
  ```rust
  impl<'a> AsyncSession<'a> {
      pub fn add_breakpoint(&mut self, addr: u32);
      pub fn remove_breakpoint(&mut self, addr: u32);
      pub fn pause(&mut self);                    // Paused(Manual) at the next boundary
      pub fn stop(self) -> RunStats;              // final accounting snapshot
      pub fn ip(&self) -> u32;
      pub fn mf(&self) -> bool;
      pub fn depth(&self) -> usize;
      pub fn stack(&self) -> &[u32];
      pub fn stats(&self) -> RunStats;
      pub fn finished(&self) -> Option<&RunResult>;
  }
  ```

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn breakpoint_pauses_at_the_boundary_before_the_instruction() {
    // Program with >= 3 instructions; set a breakpoint on the 2nd
    // instruction's address (compute from the byte layout of the chosen
    // program), pump, expect Paused(Breakpoint(addr)) with ip() == addr,
    // then pump again to Finished with the sync-identical result.
}

#[test]
fn manual_pause_fires_once_at_the_next_boundary() {
    // pause() before the first pump: first pump returns Paused(Manual)
    // after exactly one instruction; the request does not re-fire.
}

#[test]
fn stop_returns_the_accounting_snapshot() {
    // pump one instruction (budget 1), stop() the session, assert the
    // returned RunStats shows steps == 1.
}

#[test]
fn finished_session_repeats_its_result() {
    // pump to completion; pump again; expect the same Finished(result).
}
```

Write these as real code against the Task 3 helpers (same program bytes; breakpoint addresses computed from the program's instruction sizes — document the arithmetic in a comment).

- [ ] **Step 2: Run to verify failure** — accessor methods don't exist → compile FAIL.
- [ ] **Step 3: Implement** — the accessor block copies `DebugSession`'s shapes (`debug.rs:92-141`): `ip = core.ip()`, `mf = core.mf()`, `depth = stack.depth()`, `stack = stack.entries()`, `stats = self.stats`, plus:

```rust
pub fn add_breakpoint(&mut self, addr: u32) {
    self.breakpoints.insert(addr);
}

pub fn remove_breakpoint(&mut self, addr: u32) {
    self.breakpoints.remove(&addr);
}

/// Request a pause; the session honors it at the next instruction
/// boundary (`Paused(Manual)`). The RUN/HALT line of the software tier.
pub fn pause(&mut self) {
    self.pause_requested = true;
}

/// Finalize: consume the session, keep the accounting.
pub fn stop(self) -> RunStats {
    self.stats
}

pub fn finished(&self) -> Option<&RunResult> {
    self.finished.as_ref()
}
```

- [ ] **Step 4: Run tests** — `cargo test -p mtc-core session` → all PASS.
- [ ] **Step 5: Gates + commit**

```bash
git add crates/core/src/vm/session.rs
git commit -m "feat(core): AsyncSession debug controls — breakpoints, pause, stop, accessors"
```

---

### Task 6: `Machine` constructors and the initial-mark latch

**Files:**
- Modify: `crates/core/src/vm/machine.rs` (two constructors after `debug_tapes`, `machine.rs:331-341`)
- Modify: `crates/core/src/vm/session.rs` (implement `pump_latch`)

**Interfaces:**
- Produces:
  ```rust
  impl<'a> Machine<'a> {
      /// Legacy single-tape shape: preloads the mark, no table ROM —
      /// mirrors `run`/`debug`.
      pub fn async_session(&self, opts: RunOptions) -> AsyncSession<'a>;
      /// Multi-tape shape: table ROM, no mark preload — mirrors
      /// `run_tapes`/`debug_tapes`.
      pub fn async_session_tapes(&self, opts: RunOptions) -> AsyncSession<'a>;
  }
  ```

- [ ] **Step 1: Write the failing tests** (inline in `machine.rs`'s existing test module, next to its `run`/`debug` tests — reuse their image-building helpers)

```rust
#[test]
fn async_session_latches_the_initial_mark_like_run() {
    // Build the same single-tape image machine.rs tests use for run().
    // Pre-mark the tape cell under the head, run() vs pumped
    // async_session(): identical RunResult (the latch made MF true in
    // both). Then repeat with an unmarked cell: again identical.
}

#[test]
fn async_session_latch_waits_on_a_slow_device_and_stays_unaccounted() {
    // async_session() + LatencyTape (read_polls: 3): expect exactly 3
    // DeviceWait events BEFORE the first instruction retires, and final
    // stats identical to the sync run (the latch read is tact-free:
    // neither its cost nor its polls appear anywhere in RunStats).
}

#[test]
fn async_session_tapes_does_not_latch() {
    // Mirror: async_session_tapes() over the same image with a marked
    // cell must equal run_tapes() (no preload) — not run().
}
```

- [ ] **Step 2: Run to verify failure** — constructors missing → compile FAIL.
- [ ] **Step 3: Implement**

`machine.rs` (mirror `debug`/`debug_tapes` exactly — same core/stack/tables plumbing):

```rust
/// A pumped async session over this machine's image (docs/core.md
/// (async session)). Legacy single-tape shape: preloads the mark as a
/// real device-0 read transaction (tact-free loading step), no table
/// ROM — mirrors `run`/`debug`.
pub fn async_session(&self, opts: RunOptions) -> AsyncSession<'a> {
    AsyncSession::new(
        self.build_core(),
        self.code.clone(),
        ReturnStack::new(opts.stack_depth),
        opts.profile,
        opts.limits,
    )
}

/// A multi-tape pumped session: carries the table ROM and does not
/// preload the mark, mirroring `run_tapes`/`debug_tapes`.
pub fn async_session_tapes(&self, opts: RunOptions) -> AsyncSession<'a> {
    self.async_session(opts).with_tables(self.tables.clone())
}
```

`session.rs` — replace the `pump_latch` stub:

```rust
/// The loading-step latch as a real transaction (docs/core.md (loading)):
/// read device 0, match against the mark index 1, set MF — priced at
/// nothing (loading, not execution: neither cost nor polls reach the
/// stats), but subject to WAIT like any transaction.
fn pump_latch(&mut self, devices: &mut [&mut dyn AsyncTapeDevice], budget: Option<u64>) -> PumpEvent {
    let issued = matches!(self.waiting, Waiting::Latch);
    let Some(device) = devices.get_mut(0) else {
        // No device to latch from: mirror the sync path's panic-free
        // choice — treat as unmarked and continue.
        self.latch_initial_mark = false;
        self.waiting = Waiting::None;
        return self.pump(devices, budget);
    };
    if !issued {
        device.issue(DeviceCmd::Read);
        self.waiting = Waiting::Latch;
    }
    match device.poll() {
        DevicePoll::Pending => PumpEvent::DeviceWait,
        DevicePoll::Ready { reply, .. } => {
            if let DeviceReply::Symbol(symbol) = reply {
                self.core.set_mf(symbol == 1);
            }
            self.latch_initial_mark = false;
            self.waiting = Waiting::None;
            self.pump(devices, budget)
        }
    }
}
```

And in `pump`, the Task 3 comment placeholder resolves: the boundary-check block's `if self.latch_initial_mark { return self.pump_latch(devices, budget); }` line was already written in Task 3 — verify it now runs BEFORE `core.start()` on the first pump (it does: `started` is still false and the latch branch precedes the start branch). Re-entry after the latch goes through `self.pump(...)` recursion with the flag cleared — budget untouched (the latch retires no instruction).

Note the sync latch reads the device directly (`machine.rs:261`, `debug.rs:149-153`); the async path routes the read through `issue`/`poll` so a slow device genuinely delays loading — but the accounting outcome is identical: nothing recorded.

- [ ] **Step 4: Run tests** — `cargo test -p mtc-core` → all PASS (machine + session + existing).
- [ ] **Step 5: Gates + commit**

```bash
git add crates/core/src/vm/machine.rs crates/core/src/vm/session.rs
git commit -m "feat(core): Machine::async_session{,_tapes} and the waitable loading latch"
```

---

### Task 7: PM-1 corpus equivalence — sync ≡ pump on real programs

**Files:**
- Create: `crates/post-machine/tests/async_equivalence.rs`

**Interfaces:** consumes only public APIs: `mtc_post_machine`'s compile/link path and `mtc_core::vm::{Machine, ArchRegistry, RunOptions, InfiniteTape, SyncAsAsync, LatencyTape, LatencyProfile, PumpEvent}`.

- [ ] **Step 1: Open `crates/post-machine/tests/golden_programs.rs` and copy its local build helper** (source → `.pmx` `Executable` + machine construction + the PM-1 arch registry). Repo convention: each integration test file defines its own helpers — copy, don't import.

- [ ] **Step 2: Write the test** (real code; the corpus is a `&[(&str, &str)]` of name + `.pmc` source)

```rust
//! sync ≡ pump equivalence over real PM-1 programs: a pumped run through
//! always-ready adapters must match `Machine::run` bit-exactly — outcome,
//! stats, ip, stack — at -O0 and -O1; a latency device must change
//! nothing but the number of pump calls.

/* local helpers copied per golden_programs.rs */

const CORPUS: &[(&str, &str)] = &[
    ("mark_run", "main() { mark; right; mark; right; mark; }"),
    ("walk_and_erase", r#"
        export eraseOne() { unmark; right; }
        main() { mark; right; mark; left; @eraseOne(); @eraseOne(); }
    "#),
    ("branchy", r#"
        main() {
         1: mark;
            right;
            check(3, 5);
         3: unmark;
         5: mark;
            left;
        }
    "#),
    ("stdlib_user", "main() { mark; right; mark; right; mark; @std::goToBegin(); @std::eraseSection(); }"),
];

fn pump_to_end(machine: &Machine, opts: RunOptions) -> RunResult {
    let mut session = machine.async_session(opts);
    let mut tape = SyncAsAsync::new(InfiniteTape::new());
    loop {
        match session.pump(&mut [&mut tape], None) {
            PumpEvent::Finished(result) => return result,
            PumpEvent::DeviceWait | PumpEvent::BudgetSpent => continue,
            other => panic!("unexpected: {other:?}"),
        }
    }
}

#[test]
fn pumped_runs_match_sync_runs_across_the_corpus() {
    for (name, source) in CORPUS {
        for opt in ["-O0", "-O1"] {
            let machine = /* compile+link via the copied helper at `opt` */;
            let mut sync_tape = InfiniteTape::new();
            let sync = machine.run(&mut sync_tape, RunOptions::default());
            let pumped = pump_to_end(&machine, RunOptions::default());
            assert_eq!(pumped, sync, "{name} at {opt}");
        }
    }
}

#[test]
fn latency_changes_nothing_but_the_pump_count() {
    // One corpus entry, LatencyTape { *_polls: 3, electronic costs }:
    // identical RunResult; DeviceWait observed.
}
```

The `/* compile+link */` slot is the copied golden_programs helper invocation — same option shapes it uses, with the optimization level parameterized.

- [ ] **Step 3: Run** — `cargo test -p mtc-post-machine --test async_equivalence` → PASS both.
- [ ] **Step 4: Commit**

```bash
git add crates/post-machine/tests/async_equivalence.rs
git commit -m "test(post-machine): sync≡pump equivalence over a real PM-1 corpus"
```

---

### Task 8: TM-1 corpus equivalence

**Files:**
- Create: `crates/turing-machine/tests/async_equivalence.rs`

**Interfaces:** consumes public APIs only; uses `Machine::run_tapes` vs `async_session_tapes` (multi-tape shape, table ROM, no latch).

- [ ] **Step 1: Open `crates/turing-machine/tests/cli_programs.rs` and copy its local compile+link+machine helper** (`.tmc` source → linked MX → `Machine`; and its tape-device construction from cardinalities — TM devices are `WideTape`s sized per band).

- [ ] **Step 2: Write the test** — two small `.tmc` programs (single-tape and two-tape; write real sources in the file — a bit-flipper and a two-band copier, modeled on `docs/tmt/language.md` §examples), each at `-O0` and `-O1`, each through `--call-mech mono` defaults the helper uses:

```rust
fn pump_tapes_to_end(machine: &Machine, cards: &[u32], opts: RunOptions) -> RunResult {
    let mut inner: Vec<WideTape> = cards.iter().map(|&c| WideTape::new(c)).collect();
    let mut devices: Vec<SyncAsAsync<&mut WideTape>> = /* wrap each */;
    /* pump loop as in Task 7 but over the device slice */
}

#[test]
fn pumped_tm_runs_match_run_tapes() {
    /* for each (program, opt): run_tapes vs pump — assert_eq!(pumped, sync) */
}
```

Note `SyncAsAsync<T: Tape>` needs `T: Tape`; `&mut WideTape` implements `Tape`? It does not automatically — wrap owned tapes instead (`SyncAsAsync<WideTape>`) and build the `&mut dyn AsyncTapeDevice` slice from them.

- [ ] **Step 3: Run** — `cargo test -p mtc-turing-machine --test async_equivalence` → PASS.
- [ ] **Step 4: Commit**

```bash
git add crates/turing-machine/tests/async_equivalence.rs
git commit -m "test(turing-machine): sync≡pump equivalence for multi-tape TM-1 images"
```

---

### Task 9: `no_std` vm build

**Files:**
- Modify: `crates/core/Cargo.toml` (features; optional serde)
- Modify: `crates/core/src/lib.rs` (crate attr, module gating)
- Modify: `crates/core/src/vm/*.rs` and `crates/core/src/vm/devices/*.rs` (import sweep)
- Modify: `CLAUDE.md` (the Commands block gains the gate command)

**Interfaces:** none new; everything must keep compiling both ways.

- [ ] **Step 1: Cargo features**

```toml
[features]
default = ["std"]
std = ["dep:serde", "dep:serde_json"]

[dependencies]
serde = { version = "1", features = ["derive"], optional = true }
serde_json = { version = "1", optional = true }
```

(Probed: serde is used only by `linker/` and `lsp/` — both std-gated below.)

- [ ] **Step 2: Gate the crate root** — `lib.rs`:

```rust
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod asm;         // -> #[cfg(feature = "std")]
pub mod diagnostics; // -> #[cfg(feature = "std")]
pub mod formats;     // -> #[cfg(feature = "std")]
pub mod linker;      // -> #[cfg(feature = "std")]
pub mod lsp;         // -> #[cfg(feature = "std")]
pub mod vm;          // stays unconditional
```

- [ ] **Step 3: Sweep the vm module.** Mechanical rules (apply per file: `core.rs`, `bus.rs`, `driver.rs`, `debug.rs`, `machine.rs`, `session.rs`, `frame.rs`, `table.rs`, `trap.rs`, `arch.rs`, `devices/*.rs`; `#[cfg(test)]` blocks are exempt — tests build with std):
  - `std::collections::VecDeque` → `alloc::collections::VecDeque`
  - `std::collections::BTreeSet` → `alloc::collections::BTreeSet`
  - `std::collections::HashMap` (in `infinite_tape.rs:5`, `wide_tape.rs:8`) → `alloc::collections::BTreeMap` — **a real container swap**: page/cell sparse maps become ordered maps. Behavior-identical (value semantics unchanged; run stats and golden tapes unaffected — the map is internal storage), method surface is the same for the operations used (`entry`/`get`/`insert`).
  - `std::mem` → `core::mem`; `std::fmt` → `core::fmt`
  - `impl std::error::Error for …` (machine.rs:62, :110) → `impl core::error::Error for …`
  - Add per-file `use alloc::vec::Vec;` / `use alloc::boxed::Box;` / `use alloc::string::String;` / `use alloc::vec;` / `use alloc::format;` where the preludeless build demands them — let the compiler drive: build with `--no-default-features` and fix each error site.
  - `Machine::from_executable` and `ArchRegistry`… `from_executable` takes `&Executable` (a `formats` type) → gate `#[cfg(feature = "std")]` on that one method (and its import). `LoadError` stays unconditional (`with_arch` uses it).

- [ ] **Step 4: Verify both ways**

Run: `cargo build -p mtc-core --no-default-features`
Expected: clean build (proves the vm tree references no `std::`).
Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: everything green with default features — behavior unchanged.

- [ ] **Step 5: Record the gate** — `CLAUDE.md` Commands block, after the fmt line:

```
cargo build -p mtc-core --no-default-features        # no_std vm gate (docs/core.md (async session))
```

(There is no build CI in this repo — `.github/workflows/` has only audit.yml — so the gate lives in the local quality-gate list, like clippy/fmt.)

- [ ] **Step 6: Commit**

```bash
git add crates/core/Cargo.toml crates/core/src/lib.rs crates/core/src/vm/ CLAUDE.md
git commit -m "feat(core): no_std vm — std feature gate, alloc sweep, BTreeMap tape pages"
```

---

### Task 10: `docs/core.md` — the async surface; final gates

**Files:**
- Modify: `docs/core.md` — three edits: the device-bus section (`### The tape and device bus`, line ~128), the Wait-states paragraph (line ~254), a new `### AsyncSession` subsection after `### DebugSession` (line ~305)

**Steps:**

- [ ] **Step 1: Amend the Wait-states paragraph** (docs/core.md:254). After the existing sentence about stall accounting, append:

> Under the async device surface the same accounting holds with one
> refinement: a device may *report* its transaction's cost in tacts
> (a real device's wait is real machine time — in hardware the counter
> ticks through a WAIT stall); a device that reports nothing is priced at
> the profile's model cost. READY sampling itself is free — pending polls
> never tick the counter, so stats stay deterministic regardless of how
> often the embedder pumps.

- [ ] **Step 2: Extend the device-bus section** with the async trait (after the `Tape` trait description): describe `AsyncTapeDevice` (`issue`/`poll`, one command in flight, WAIT/READY reading — `issue` puts the command on the bus, each `poll` samples READY on a clock edge), `SyncAsAsync` (any tape as an always-ready device), and `LatencyTape` (the shipped waiting device: configurable polls-until-ready and reported cost per operation; the reference implementation of the contract and the transport counterpart of a mechanical tact profile).

- [ ] **Step 3: Add the `### AsyncSession` subsection** after DebugSession, covering: the pump model (embedder owns the loop; one `pump` retires instructions until a device holds READY low, the per-call budget runs out, a pause condition fires, or the program ends); the clock correspondence (in hardware the clock generator pumps the processor — `pump` calls are clock edges; the Rust core doubles as the golden model for hardware implementations, being a per-tact `BusRequest`/`BusResponse` state machine); debug controls (breakpoints, `pause()`, `stop()`, the same accessors as DebugSession); the loading latch as a real, waitable, still-unaccounted transaction; device-slice contract; the `no_std` note (the vm builds without std for firmware embedding — formats/linker/assembler/LSP stay std-only); and the non-goals stated as contract: the bus protocol is not extended, `ReadAll`-style per-instruction reads serialize in device order, there is no device timeout — "never ready" is the embedder's judgement.

Write these as flowing prose in the page's existing voice (read the DebugSession section first and match it). Forge-agnostic: no issue numbers, no spec references.

- [ ] **Step 4: Verify every claim against the code** — each API name, each contract sentence, replayed against the shipped types (`cargo doc -p mtc-core --no-deps` builds clean; names spot-checked against `vm/mod.rs` exports).

- [ ] **Step 5: Full final gates**

Run, capturing exit codes directly (no pipes):
`cargo test --workspace` → all pass;
`cargo clippy --workspace --all-targets -- -D warnings` → clean;
`cargo fmt --check` → clean;
`cargo build -p mtc-core --no-default-features` → clean.

- [ ] **Step 6: Commit**

```bash
git add docs/core.md
git commit -m "docs(core): the async session — WAIT/READY, pump model, cost reporting, no_std surface"
```
