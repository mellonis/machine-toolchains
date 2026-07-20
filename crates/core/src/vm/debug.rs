//! Interactive debugger session (docs/isa.md (DebugSession)): the same
//! surface shape as turing-machine-js v7 sessions — the session owns the
//! run; step/pause with a cause; depth-based stepIn/stepOver/stepOut
//! (depth is just SP). Sync v1: external pause/run-interval throttle is
//! modelled by `run_steps` chunking.

use std::collections::BTreeSet;

use super::Outcome;
use super::core::Core;
use super::devices::Tape;
use super::driver::{ReturnStack, RunLimits, RunStats, StepEvent, TactProfile, step_instruction};
use super::trap::Trap;

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
    /// Match/dispatch table ROM (docs/formats.md (executable image)); empty
    /// for a legacy single-tape session.
    tables: Vec<u8>,
    stats: RunStats,
    profile: TactProfile,
    limits: RunLimits,
    breakpoints: BTreeSet<u32>,
    started: bool,
    /// Latch the initial mark from the mark device on the first step (the
    /// PM-1 loading step). Set for legacy single-tape sessions; cleared by
    /// `with_tables` for the multi-tape v2 shape (mirrors `run_tapes`).
    latch_initial_mark: bool,
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
            tables: Vec::new(),
            stats: RunStats::default(),
            profile,
            limits,
            breakpoints: BTreeSet::new(),
            started: false,
            latch_initial_mark: true,
            finished: None,
            trap_reported: false,
        }
    }

    /// Carry a table ROM into the session (docs/formats.md (executable
    /// image)) and switch it to the multi-tape v2 shape: this also clears the
    /// PM-1 initial-mark latch, mirroring `run_tapes` (MR starts 0; head
    /// symbols enter via explicit read micro-ops). Drive such a session with
    /// `step_in_tapes`.
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

    /// The frame register (0 = the identity composite; non-zero = the
    /// active composite INDEX inside a framed call) — the frames profile's
    /// counterpart to `mf()`. On a base-profile session it stays 0. Note:
    /// if a trap halts a session mid frame-load, `fr()` reports the
    /// resolved-but-not-yet-loaded composite index — FR is set from
    /// `compose[FR][site]` before its descriptor finishes loading, and the
    /// trap freezes state there. Harmless: it is simply the FR at trap time.
    ///
    /// Resolving that composite index to its per-tape binding maps or
    /// canonical label is a debugger-tooling concern layered on the
    /// `.pmx.map` sidecar, not a VM accessor — deliberately deferred until a
    /// debugger surfaces it.
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

    pub fn finished(&self) -> Option<Outcome> {
        self.finished
    }

    /// One raw instruction; the shared bottom of every public motion. The
    /// device set arrives per call — a legacy session wraps its single
    /// device into a one-element slice.
    fn advance(&mut self, devices: &mut [&mut dyn Tape]) -> StepEvent {
        if !self.started && self.latch_initial_mark {
            // Initial MF latch, tact-free (loading, not execution) —
            // mirrors Machine::run; PM-1 matches against mark index 1.
            self.core.set_mf(devices[0].read() == 1);
        }
        let event = step_instruction(
            &mut self.core,
            &self.code,
            &mut self.stack,
            devices,
            &self.tables,
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
        self.step_in_tapes(&mut [device])
    }

    /// Multi-tape single step (docs/formats.md (executable image)): the
    /// device-set analog of `step_in`, mirroring it exactly — one raw
    /// instruction, then a `Step` pause. Like `step_in`, it does not consult
    /// breakpoints (only the `continue`/`step_over`/`step_out` motions do).
    pub fn step_in_tapes(&mut self, devices: &mut [&mut dyn Tape]) -> DebugEvent {
        if let Some(done) = self.gate() {
            return done;
        }
        let event = self.advance(devices);
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
        let event = self.advance(&mut [&mut *device]);
        if self.stack.depth() > depth0
            && let StepEvent::Retired = event
        {
            return self.run_until(device, Some(depth0), None);
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
        // A zero budget pauses immediately without stepping — this also
        // guards the decrement below against u64 underflow on a zero
        // budget.
        if budget == Some(0) {
            return DebugEvent::Paused(PauseCause::Manual);
        }
        loop {
            let event = self.advance(&mut [&mut *device]);
            match event {
                StepEvent::Retired => {
                    if let Some(target) = until_depth
                        && self.stack.depth() <= target
                    {
                        return DebugEvent::Paused(PauseCause::Step);
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
        assert_eq!(
            s.continue_(&mut tape),
            DebugEvent::Finished(Outcome::Stopped)
        );
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
        assert_eq!(
            s.continue_(&mut tape),
            DebugEvent::Finished(Outcome::Stopped)
        );
    }

    #[test]
    fn breakpoint_at_entry_pauses_only_after_leaving_it() {
        // bp at the entry instruction: first continue_ executes it (a
        // fresh session is "paused at" entry already, debugger convention)
        let mut s = session(&[0x01, 0x02]);
        let mut tape = InfiniteTape::new();
        s.add_breakpoint(0);
        assert_eq!(
            s.continue_(&mut tape),
            DebugEvent::Finished(Outcome::Stopped)
        );
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
        assert_eq!(
            s.run_steps(&mut tape, 2),
            DebugEvent::Paused(PauseCause::Manual)
        );
        assert_eq!(s.stats().steps, 2);
        assert_eq!(
            s.run_steps(&mut tape, 10),
            DebugEvent::Finished(Outcome::Stopped)
        );
    }

    #[test]
    fn run_steps_zero_budget_pauses_without_stepping() {
        let mut s = session(&[0x01, 0x02]);
        let mut tape = InfiniteTape::new();
        assert_eq!(
            s.run_steps(&mut tape, 0),
            DebugEvent::Paused(PauseCause::Manual)
        );
        assert_eq!(s.stats().steps, 0);
        assert_eq!(
            s.run_steps(&mut tape, 10),
            DebugEvent::Finished(Outcome::Stopped)
        );
    }

    #[test]
    fn step_in_tapes_drives_two_devices_through_a_table_rom() {
        // Two devices read into TR, a width-2 match table folds them into MR,
        // a dispatch jump lands on stp. `with_tables` must feed the ROM and
        // must NOT preload MF (mirrors run_tapes).
        //   [0] entry; [1] read both; [2] mtc @0; [7] djmp @5; [12] stp
        let mut code = vec![0x0E, 0x10, 0x11];
        code.extend(0u32.to_le_bytes());
        code.push(0x12);
        code.extend(5u32.to_le_bytes());
        code.push(0x02);
        let tables = vec![2, 1, 0, 1, 1, 1, 0, 12, 0, 0, 0];
        static ARCH: TestArch = TestArch;
        let mut s = DebugSession::new(
            Core::new(&ARCH, 0),
            code,
            ReturnStack::new(4),
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        )
        .with_tables(tables);
        let mut t0 = InfiniteTape::from_cells([true], 0, 0);
        let mut t1 = InfiniteTape::from_cells([true], 0, 0);
        let mut devs: [&mut dyn super::Tape; 2] = [&mut t0, &mut t1];
        for expected_ip in [1u32, 2, 7, 12] {
            assert_eq!(
                s.step_in_tapes(&mut devs),
                DebugEvent::Paused(PauseCause::Step)
            );
            assert_eq!(s.ip(), expected_ip);
        }
        assert_eq!(
            s.step_in_tapes(&mut devs),
            DebugEvent::Finished(Outcome::Stopped)
        );
    }

    #[test]
    fn state_accessors_cover_mf_stack_and_trap_ip() {
        // mf(): right onto a marked cell latches MF
        let mut s = session(&[0x06, 0x02]);
        let mut tape = InfiniteTape::from_cells([false, true], 0, 0);
        assert_eq!(s.step_in(&mut tape), DebugEvent::Paused(PauseCause::Step));
        assert!(s.mf());

        // stack(): inside a call, the return address is visible
        // 0: call +2 -> 7; 5: nop; 6: stop; 7: ent; 8: nop; 9: ret
        let code = &[0x0A, 0x02, 0x00, 0x00, 0x00, 0x01, 0x02, 0x0E, 0x01, 0x0B];
        let mut s = session(code);
        let mut tape = InfiniteTape::new();
        assert_eq!(s.step_in(&mut tape), DebugEvent::Paused(PauseCause::Step));
        assert_eq!(s.stack(), &[5]);

        // ip() after a trap pause: the FAULTING instruction's address
        let mut s = session(&[0x0B]); // ret on empty stack at address 0
        let mut tape = InfiniteTape::new();
        assert_eq!(
            s.continue_(&mut tape),
            DebugEvent::Paused(PauseCause::Trap(Trap::StackUnderflow))
        );
        assert_eq!(s.ip(), 0);
    }
}
