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
        let mut devices: [&mut dyn Tape; 1] = [device];
        let event = step_instruction(
            &mut self.core,
            &self.code,
            &mut self.stack,
            &mut devices,
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
            let event = self.advance(device);
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
