//! The pumped execution session (docs/core.md (async session)): the sync
//! driver's serving loop, re-hosted so a device that is not READY suspends
//! the session between `pump` calls instead of blocking the thread. The
//! embedder owns the loop — in hardware the clock generator pumps the
//! processor; here the embedder's `pump` calls play the clock edges.

use std::collections::BTreeSet;

use super::bus::{BusRequest, BusResponse, CoreEvent};
use super::core::Core;
use super::debug::PauseCause;
use super::devices::{AsyncTapeDevice, DeviceCmd, DevicePoll, DeviceReply};
use super::driver::{Outcome, ReturnStack, RunLimits, RunResult, RunStats, TactProfile};
use super::trap::{DeviceFault, Trap};

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
    /// The initial-mark loading read on device 0 (docs/core.md (loading)):
    /// set while `pump_latch`'s read is in flight, cleared once it settles.
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

    /// Next instruction's address. Unlike `DebugSession::ip`, this needs no
    /// trapped-instruction special case: `CoreEvent::Trapped` goes straight
    /// to `finish`, which already stores `instr_start()` in `RunResult.ip`.
    pub fn ip(&self) -> u32 {
        self.core.ip()
    }

    pub fn mf(&self) -> bool {
        self.core.mf()
    }

    /// The frame register (0 = the identity composite; non-zero = the
    /// active composite index inside a framed call) — the frames profile's
    /// counterpart to `mf()`, mirroring `DebugSession::fr()`.
    pub fn fr(&self) -> u32 {
        self.core.fr()
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

    pub fn finished(&self) -> Option<&RunResult> {
        self.finished.as_ref()
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
            BusRequest::DeviceMoveLeft { dev } => {
                (dev, DeviceCmd::MoveLeft, self.profile.move_cost)
            }
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
    fn settle_device(
        &mut self,
        model_cost: u32,
        reply: DeviceReply,
        cost: Option<u32>,
    ) -> BusResponse {
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
        // match, or the resume path (waiting == Waiting::Exec, device now
        // Ready) would retire an instruction and underflow the u64
        // decrement below (debug: panic; release: wraps to u64::MAX, so a
        // zero budget silently becomes unlimited).
        if budget == Some(0) {
            return PumpEvent::BudgetSpent;
        }
        // Loading step (docs/core.md (loading)): on the async path the
        // initial-mark latch is a real device-0 transaction (`pump_latch`)
        // — unaccounted like the sync path's direct read, and itself
        // subject to WAIT. The zero-budget guard above fires first, so a
        // fresh session pumped with `Some(0)` reports `BudgetSpent` before
        // ever latching; the latch runs on the first pump that carries
        // budget.
        let mut remaining = budget;
        // Resume a suspended mid-instruction transaction, if any.
        let mut event: Option<CoreEvent> = match self.waiting {
            Waiting::None => None,
            // Re-poll the same in-flight latch transaction.
            Waiting::Latch => return self.pump_latch(devices, budget),
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
                    // First pump: latch the initial mark before starting.
                    return self.pump_latch(devices, budget);
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
                            BusRequest::TableRead { addr } => {
                                match self.tables.get(addr as usize) {
                                    Some(&byte) => {
                                        self.stats.stall_tacts +=
                                            u64::from(self.profile.table_read_cost);
                                        BusResponse::Byte(byte)
                                    }
                                    None => BusResponse::OutOfTable,
                                }
                            }
                            BusRequest::FrameRead { addr } => {
                                match self.tables.get(addr as usize) {
                                    Some(&byte) => {
                                        self.stats.stall_tacts +=
                                            u64::from(self.profile.frame_load_cost);
                                        BusResponse::Byte(byte)
                                    }
                                    None => BusResponse::OutOfTable,
                                }
                            }
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
                        if self
                            .limits
                            .max_steps
                            .is_some_and(|max| self.stats.steps >= max)
                        {
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

    /// The loading-step latch as a real transaction (docs/core.md
    /// (loading)): read device 0 and match it against the mark index 1 to
    /// set MF — priced at nothing (loading, not execution: neither the
    /// reported cost nor the pending polls reach `stats`), but subject to
    /// WAIT like any other transaction. Mirrors the sync path's direct
    /// `devices[0].read()`, routed through `issue`/`poll` instead so a slow
    /// device genuinely delays loading rather than blocking the thread.
    fn pump_latch(
        &mut self,
        devices: &mut [&mut dyn AsyncTapeDevice],
        budget: Option<u64>,
    ) -> PumpEvent {
        let issued = matches!(self.waiting, Waiting::Latch);
        let Some(device) = devices.get_mut(0) else {
            // No device to latch from: mirror the sync path's panic-free
            // choice for a mismatched device set — treat the mark as
            // unmarked and continue, rather than indexing and panicking.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::devices::{InfiniteTape, LatencyProfile, LatencyTape, SyncAsAsync};
    use crate::vm::{Core, ReturnStack, driver};

    // Mirror driver.rs's test program builder: the fake arch's opcodes are
    // documented at vm/arch.rs::test_arch. Reuse the same byte sequences
    // driver.rs tests run, so sync and pumped runs execute identical images.
    // write-move-stop, assembled from the exact opcode/operand bytes
    // driver.rs's own tests use: 0x07,0x81 (write index 1 —
    // `write_pays_write_then_latch_read`), 0x06 (move right —
    // `tape_instruction_splits_core_and_stall`), 0x02 (stop, ubiquitous).
    const WRITE_MOVE_STOP: [u8; 4] = [0x07, 0x81, 0x06, 0x02];

    fn sync_result(code: &[u8]) -> crate::vm::RunResult {
        let arch = TestArch;
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
        let arch = TestArch;
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
        let code = WRITE_MOVE_STOP;
        let sync = sync_result(&code);
        // Pin the baseline itself, so a program that traps before doing
        // anything can't make the equality below pass vacuously.
        assert_eq!(sync.outcome, Outcome::Stopped);
        assert_eq!(sync.stats.steps, 2); // write, move; stp retires no Step
        let (pumped, _) = pumped_result(&code, None);
        assert_eq!(pumped, sync); // outcome, stats, ip, stack — all of it
    }

    /// The one divergence from `step_instruction`: a device that answers
    /// WAIT suspends the session (`PumpEvent::DeviceWait`) instead of
    /// blocking, and the next `pump` call resumes the same transaction —
    /// through `Waiting::Exec` — without re-issuing it. `LatencyTape`'s
    /// per-op cost is pinned to `TactProfile::ELECTRONIC`'s unit costs so
    /// suspending changes nothing about the final `RunResult`; only the
    /// poll counts differ from the always-ready adapter.
    #[test]
    fn suspends_on_a_not_ready_device_then_matches_the_sync_run() {
        let code = WRITE_MOVE_STOP;
        let sync = sync_result(&code);
        let arch = TestArch;
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        let profile = LatencyProfile {
            move_polls: 1,
            read_polls: 1,
            write_polls: 1,
            move_cost: 1,
            read_cost: 1,
            write_cost: 1,
        };
        let mut tape = LatencyTape::new(InfiniteTape::new(), profile);
        let mut waits = 0;
        let pumped = loop {
            match session.pump(&mut [&mut tape], None) {
                PumpEvent::Finished(result) => break result,
                PumpEvent::DeviceWait => {
                    waits += 1;
                    continue;
                }
                other => panic!("unexpected event on a latency device: {other:?}"),
            }
        };
        assert!(waits > 0, "a not-ready device must suspend at least once");
        assert_eq!(pumped, sync);
    }

    #[test]
    fn budget_chunks_execution_without_changing_the_result() {
        let code = WRITE_MOVE_STOP;
        let sync = sync_result(&code);
        let (pumped, pumps) = pumped_result(&code, Some(1));
        assert_eq!(pumped, sync);
        // Exactly one instruction per pump on an always-ready device, plus
        // one final call to fetch and retire the terminal `stp` (which
        // itself retires no `Step`, so it isn't counted in `sync.stats.steps`).
        assert_eq!(pumps as u64, sync.stats.steps + 1);
    }

    #[test]
    fn zero_budget_returns_budget_spent_without_advancing() {
        let code = WRITE_MOVE_STOP;
        let arch = TestArch;
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
        assert_eq!(session.stats().steps, 0);
    }

    /// Regression: the zero-budget guard must also cover the resume path
    /// (`waiting == Waiting::Exec`), not just a fresh instruction boundary.
    /// Before the fix, a zero budget on a resumed-and-now-ready device let
    /// the instruction retire and then underflowed the `u64` decrement.
    #[test]
    fn zero_budget_on_a_resume_does_not_advance_or_underflow() {
        let code = WRITE_MOVE_STOP;
        let sync = sync_result(&code);
        let arch = TestArch;
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        // One poll of WAIT on the first device transaction, so the second
        // `pump` call re-enters through `Waiting::Exec` with the device now
        // Ready — the resume path the zero-budget guard must also cover.
        let profile = LatencyProfile {
            move_polls: 1,
            read_polls: 1,
            write_polls: 1,
            move_cost: 1,
            read_cost: 1,
            write_cost: 1,
        };
        let mut tape = LatencyTape::new(InfiniteTape::new(), profile);
        assert_eq!(session.pump(&mut [&mut tape], None), PumpEvent::DeviceWait);
        let stats_before = session.stats;
        assert_eq!(
            session.pump(&mut [&mut tape], Some(0)),
            PumpEvent::BudgetSpent
        );
        // Nothing advanced: stats unchanged, still suspended on the same
        // transaction (not reset, not retired).
        assert_eq!(session.stats, stats_before);
        assert!(
            matches!(session.waiting, Waiting::Exec(_)),
            "must still be suspended on the same transaction"
        );
        // A later unlimited pump completes normally with the sync-identical
        // result — the suspension left no trace on the outcome.
        let pumped = loop {
            match session.pump(&mut [&mut tape], None) {
                PumpEvent::Finished(result) => break result,
                PumpEvent::DeviceWait => continue,
                other => panic!("unexpected event on a latency device: {other:?}"),
            }
        };
        assert_eq!(pumped, sync);
    }

    #[test]
    fn missing_device_faults_instead_of_panicking() {
        let code = WRITE_MOVE_STOP;
        let arch = TestArch;
        // Parity baseline: the sync driver over the same code and an empty
        // device slice must reach the identical Device trap.
        let mut sync_core = Core::new(&arch, 0);
        let mut sync_stack = ReturnStack::new(16);
        let sync = driver::run(
            &mut sync_core,
            &code,
            &mut sync_stack,
            &mut [],
            &[],
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        );
        assert!(matches!(sync.outcome, Outcome::Trapped(_)));
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
        let pumped = loop {
            match session.pump(&mut [], None) {
                PumpEvent::Finished(result) => break result,
                PumpEvent::BudgetSpent => continue,
                other => panic!("unexpected: {other:?}"),
            }
        };
        assert_eq!(pumped, sync);
    }

    /// The latch's own no-device branch (`devices.get_mut(0)` returning
    /// `None`): `Machine::run` would panic indexing `devices[0]` here, but
    /// `pump_latch` mirrors this module's other missing-device handling —
    /// treat the mark as unmarked and continue, so the session still faults
    /// (not panics) at the first real device request, identical to a
    /// session built via `with_tables` that skips the latch entirely.
    #[test]
    fn latch_with_no_device_treats_the_mark_as_unmarked_and_continues() {
        let code = WRITE_MOVE_STOP;
        let arch = TestArch;
        let mut sync_core = Core::new(&arch, 0);
        let mut sync_stack = ReturnStack::new(16);
        let sync = driver::run(
            &mut sync_core,
            &code,
            &mut sync_stack,
            &mut [],
            &[],
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        );
        assert!(matches!(sync.outcome, Outcome::Trapped(_)));
        // No `.with_tables(...)`: `latch_initial_mark` stays true, so the
        // first pump reaches `pump_latch` with an empty device slice.
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        );
        let pumped = loop {
            match session.pump(&mut [], None) {
                PumpEvent::Finished(result) => break result,
                PumpEvent::BudgetSpent => continue,
                other => panic!("unexpected: {other:?}"),
            }
        };
        assert_eq!(pumped, sync);
    }

    /// Multi-poll device: LatencyTape with 2 polls per operation. This
    /// strengthens the existing suspends test to multi-poll — each device
    /// transaction fires multiple DeviceWait events before the device
    /// becomes READY. The final result is bit-identical to the sync run
    /// (pending polls are not counted in stats).
    #[test]
    fn latency_device_suspends_and_resumes_mid_instruction() {
        let code = WRITE_MOVE_STOP;
        let sync = sync_result(&code);
        let arch = TestArch;
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        let profile = LatencyProfile {
            move_polls: 2,
            read_polls: 2,
            write_polls: 2,
            move_cost: 1,
            read_cost: 1,
            write_cost: 1,
        };
        let mut tape = LatencyTape::new(InfiniteTape::new(), profile);
        let mut waits = 0;
        let result = loop {
            match session.pump(&mut [&mut tape], None) {
                PumpEvent::Finished(result) => break result,
                PumpEvent::DeviceWait => {
                    waits += 1;
                    continue;
                }
                other => panic!("unexpected event: {other:?}"),
            }
        };
        // Identical result and stats: pending polls don't tick.
        assert_eq!(result, sync);
        // Each instruction has 2 device transactions (op + latch-read), 2 polls each.
        // 2 instructions × 2 transactions × 2 polls = 8 device waits.
        assert_eq!(waits, 8);
    }

    /// Device-reported cost overrides the model price. A LatencyTape with
    /// write_cost=7 and zero polls per operation. The program has exactly
    /// 1 write transaction (opcode 0x07, 0x81); sync run has stall_tacts=4
    /// (4 device transactions: write + its latch-read, move + its latch-read,
    /// each costing 1). The pumped run pays device-reported cost (7 per write),
    /// yielding stall_tacts = sync.stall_tacts + 6.
    #[test]
    fn device_reported_cost_replaces_the_model_price() {
        let code = WRITE_MOVE_STOP;
        let sync = sync_result(&code);
        let arch = TestArch;
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        let profile = LatencyProfile {
            move_polls: 0,
            read_polls: 0,
            write_polls: 0,
            move_cost: 1,
            read_cost: 1,
            write_cost: 7,
        };
        let mut tape = LatencyTape::new(InfiniteTape::new(), profile);
        let result = loop {
            match session.pump(&mut [&mut tape], None) {
                PumpEvent::Finished(result) => break result,
                PumpEvent::BudgetSpent => continue,
                other => panic!("unexpected event: {other:?}"),
            }
        };
        // Program has exactly 1 write transaction (opcode 0x07, 0x81).
        // Sync pays 1 tact per write (model price).
        // Device reports 7 tacts per write.
        // Difference: (7 - 1) per write = 6 tacts per write.
        let writes = 1;
        assert_eq!(
            result.stats.stall_tacts,
            sync.stats.stall_tacts + (7 - 1) * writes
        );
        assert_eq!(result.outcome, sync.outcome);
    }

    /// The 2nd instruction's address, computed from `WRITE_MOVE_STOP`'s byte
    /// layout: `[0x07 write, 0x81 operand, 0x06 move, 0x02 stop]` — write
    /// is a 2-byte instruction (opcode + operand), so instructions start at
    /// addresses 0 (write), 2 (move), 3 (stop). A breakpoint on 2 pauses
    /// after the write retires and before the move executes.
    #[test]
    fn breakpoint_pauses_at_the_boundary_before_the_instruction() {
        let code = WRITE_MOVE_STOP;
        let sync = sync_result(&code);
        let arch = TestArch;
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        session.add_breakpoint(2);
        let mut tape = SyncAsAsync::new(InfiniteTape::new());
        assert_eq!(
            session.pump(&mut [&mut tape], None),
            PumpEvent::Paused(PauseCause::Breakpoint(2))
        );
        assert_eq!(session.ip(), 2);
        assert_eq!(session.stats().steps, 1); // only the write retired
        // Resuming past the breakpoint runs the remaining two instructions
        // to completion in one pump (always-ready device, unlimited budget).
        let PumpEvent::Finished(pumped) = session.pump(&mut [&mut tape], None) else {
            panic!("expected the remainder of the program to finish in one pump");
        };
        // The pause must not perturb accounting: sync-identical result.
        assert_eq!(pumped, sync);
    }

    #[test]
    fn manual_pause_fires_once_at_the_next_boundary() {
        let code = WRITE_MOVE_STOP;
        let sync = sync_result(&code);
        let arch = TestArch;
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        session.pause();
        let mut tape = SyncAsAsync::new(InfiniteTape::new());
        assert_eq!(
            session.pump(&mut [&mut tape], None),
            PumpEvent::Paused(PauseCause::Manual)
        );
        assert_eq!(session.stats().steps, 1);
        // The request does not re-fire: the remaining two instructions run
        // to completion in one pump, with no further Manual pause.
        let PumpEvent::Finished(pumped) = session.pump(&mut [&mut tape], None) else {
            panic!("expected the remainder of the program to finish in one pump");
        };
        assert_eq!(pumped, sync);
    }

    #[test]
    fn stop_returns_the_accounting_snapshot() {
        let code = WRITE_MOVE_STOP;
        let arch = TestArch;
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        let mut tape = SyncAsAsync::new(InfiniteTape::new());
        assert_eq!(
            session.pump(&mut [&mut tape], Some(1)),
            PumpEvent::BudgetSpent
        );
        let stats = session.stop();
        assert_eq!(stats.steps, 1);
    }

    #[test]
    fn finished_session_repeats_its_result() {
        let code = WRITE_MOVE_STOP;
        let arch = TestArch;
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(Vec::new());
        let mut tape = SyncAsAsync::new(InfiniteTape::new());
        let first = loop {
            match session.pump(&mut [&mut tape], None) {
                PumpEvent::Finished(result) => break result,
                PumpEvent::BudgetSpent => continue,
                other => panic!("unexpected event: {other:?}"),
            }
        };
        let second = session.pump(&mut [&mut tape], None);
        assert_eq!(second, PumpEvent::Finished(first.clone()));
        assert_eq!(session.finished(), Some(&first));
    }

    /// Tact limit crossing on a resumed device transaction. The sync path
    /// never exercises the over_tacts check in the Waiting::Exec resume arm
    /// (line 201), since sync blocks on device transactions. This test proves
    /// the async path catches tact-limit crossing during a device completion
    /// on resume — discriminated by exact stats at trap time.
    ///
    /// Sync stats for WRITE_MOVE_STOP: steps=2, core_tacts=6, stall_tacts=4,
    /// total=10. Set max_tacts = 8 (total - 2). When a device transaction
    /// resumes and settles (adding 1 stall tact), the stats at the resume-arm
    /// check become: { steps: 1, core_tacts: 4, stall_tacts: 4, total: 8 },
    /// which equals the limit and triggers over_tacts(). If the resume-arm
    /// check were skipped, the trap would fire later at the step-boundary
    /// check with { steps: 2, core_tacts: 5, stall_tacts: 4, total: 9 } —
    /// exact-stats equality proves the trap fired at line 201, not downstream.
    #[test]
    fn tact_limit_fires_when_a_resumed_transaction_crosses_the_budget() {
        let code = WRITE_MOVE_STOP;
        let sync = sync_result(&code);
        let limit = sync.stats.total_tacts() - 2;
        let arch = TestArch;
        let mut session = AsyncSession::new(
            Core::new(&arch, 0),
            code.to_vec(),
            ReturnStack::new(16),
            TactProfile::ELECTRONIC,
            RunLimits {
                max_tacts: Some(limit),
                max_steps: None,
            },
        )
        .with_tables(Vec::new());
        // LatencyTape with 1 poll per operation so the write suspends.
        let profile = LatencyProfile {
            move_polls: 1,
            read_polls: 1,
            write_polls: 1,
            move_cost: 1,
            read_cost: 1,
            write_cost: 1,
        };
        let mut tape = LatencyTape::new(InfiniteTape::new(), profile);
        // Pump until first DeviceWait, assert suspension, then continue to completion.
        let mut saw_wait = false;
        let result = loop {
            match session.pump(&mut [&mut tape], None) {
                PumpEvent::DeviceWait => {
                    saw_wait = true;
                    // Verify test premise: the session is suspended mid-transaction.
                    assert!(
                        matches!(session.waiting, Waiting::Exec(_)),
                        "premise: the session must be suspended mid-transaction"
                    );
                    continue;
                }
                PumpEvent::Finished(result) => break result,
                other => panic!("unexpected event: {other:?}"),
            }
        };
        // Premise: the session did suspend at some point.
        assert!(saw_wait, "premise failed: no device wait occurred");
        // Discriminator: the trap fired at the resume-arm check with these exact stats.
        assert_eq!(
            result.stats,
            RunStats {
                steps: 1,
                core_tacts: 4,
                stall_tacts: 4
            },
            "exact stats prove the trap fired at the resume-arm check (line 201)"
        );
        // Verify the outcome is TactLimit (redundant once stats match, but explicit).
        assert!(matches!(result.outcome, Outcome::Trapped(Trap::TactLimit)));
    }
}
