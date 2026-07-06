//! Synchronous driver: answers the sans-I/O core's bus requests against
//! in-memory components and does all tact accounting (docs/isa.md
//! (timing model)).

use super::bus::{BusRequest, BusResponse, CoreEvent};
use super::core::Core;
use super::devices::Tape;
use super::trap::Trap;

#[derive(Debug)]
pub struct ReturnStack {
    entries: Vec<u32>,
    capacity: usize,
}

impl ReturnStack {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::new(),
            capacity,
        }
    }

    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[u32] {
        &self.entries
    }

    pub(crate) fn push(&mut self, value: u32) -> bool {
        if self.entries.len() == self.capacity {
            return false;
        }
        self.entries.push(value);
        true
    }

    pub(crate) fn pop(&mut self) -> Option<u32> {
        self.entries.pop()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TactProfile {
    pub move_cost: u32,
    pub read_cost: u32,
    pub write_cost: u32,
}

impl TactProfile {
    pub const ELECTRONIC: TactProfile = TactProfile {
        move_cost: 1,
        read_cost: 1,
        write_cost: 1,
    };
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunLimits {
    pub max_steps: Option<u64>,
    pub max_tacts: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunStats {
    pub steps: u64,
    pub core_tacts: u64,
    pub stall_tacts: u64,
}

impl RunStats {
    pub fn total_tacts(&self) -> u64 {
        self.core_tacts + self.stall_tacts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Stopped,
    Halted,
    Trapped(Trap),
}

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

/// One instruction boundary of the sync driver (docs/isa.md (timing
/// model) accounting). `started` is the fresh/resume flag: `false` before
/// the first call. Callers must not call again after `Finished` (the core
/// is in its terminal phase).
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
    let over_tacts = |stats: &RunStats| {
        limits
            .max_tacts
            .is_some_and(|max| stats.total_tacts() >= max)
    };

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
                    BusRequest::CodeRead { addr } => match code.get(addr as usize) {
                        Some(&byte) => {
                            stats.core_tacts += 1;
                            BusResponse::Byte(byte)
                        }
                        None => BusResponse::OutOfCode,
                    },
                    BusRequest::StackPush { value } => {
                        if stack.push(value) {
                            stats.core_tacts += 1;
                            BusResponse::Ok
                        } else {
                            BusResponse::StackFull
                        }
                    }
                    BusRequest::StackPop => match stack.pop() {
                        Some(value) => {
                            stats.core_tacts += 1;
                            BusResponse::Value(value)
                        }
                        None => BusResponse::StackEmpty,
                    },
                    BusRequest::DeviceMoveLeft { .. } => {
                        device.left();
                        stats.stall_tacts += u64::from(profile.move_cost);
                        BusResponse::Ok
                    }
                    BusRequest::DeviceMoveRight { .. } => {
                        device.right();
                        stats.stall_tacts += u64::from(profile.move_cost);
                        BusResponse::Ok
                    }
                    BusRequest::DeviceRead { .. } => {
                        stats.stall_tacts += u64::from(profile.read_cost);
                        BusResponse::Symbol(device.read())
                    }
                    BusRequest::DeviceWrite { index, .. } => match device.write(index) {
                        Ok(()) => {
                            stats.stall_tacts += u64::from(profile.write_cost);
                            BusResponse::Ok
                        }
                        Err(fault) => BusResponse::Fault(fault),
                    },
                };
                if over_tacts(stats) {
                    return StepEvent::Finished(Outcome::Trapped(Trap::TactLimit));
                }
                event = core.resume(response);
            }
            CoreEvent::Step | CoreEvent::Break => {
                stats.steps += 1;
                stats.core_tacts += 1; // execute base (docs/isa.md (timing model))
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
            core,
            code,
            stack,
            device,
            profile,
            limits,
            &mut stats,
            &mut started,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::Core;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::devices::InfiniteTape;
    use crate::vm::trap::Trap;

    // TestArch: 0x01 nop | 0x02 stop | 0x03 halt | 0x05 left+latch |
    // 0x06 right+latch | 0x07 wr(vec)+latch | 0x08 jmp rel8 |
    // 0x0A call rel32 | 0x0B ret | 0x0E entry(Nop)

    fn drive(code: &[u8], limits: RunLimits, profile: TactProfile) -> (RunResult, InfiniteTape) {
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        let mut stack = ReturnStack::new(4);
        let mut tape = InfiniteTape::new();
        let result = run(&mut core, code, &mut stack, &mut tape, profile, limits);
        (result, tape)
    }

    #[test]
    fn nop_stop_costs_fetch_and_exec_only() {
        // nop: fetch 1 + exec 1; stop: fetch 1 (terminal, no Step)
        let (r, _) = drive(&[0x01, 0x02], RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Stopped);
        assert_eq!(
            r.stats,
            RunStats {
                steps: 1,
                core_tacts: 3,
                stall_tacts: 0
            }
        );
    }

    #[test]
    fn tape_instruction_splits_core_and_stall() {
        // right: fetch 1 + exec 1 core; move 1 + latch-read 1 stall; then stop 1
        let (r, tape) = drive(&[0x06, 0x02], RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Stopped);
        assert_eq!(
            r.stats,
            RunStats {
                steps: 1,
                core_tacts: 3,
                stall_tacts: 2
            }
        );
        assert_eq!(tape.head(), 1);
    }

    #[test]
    fn mechanical_profile_inflates_stall_only() {
        let mech = TactProfile {
            move_cost: 50,
            read_cost: 5,
            write_cost: 10,
        };
        let (r, _) = drive(&[0x06, 0x02], RunLimits::default(), mech);
        assert_eq!(
            r.stats,
            RunStats {
                steps: 1,
                core_tacts: 3,
                stall_tacts: 55
            }
        );
    }

    #[test]
    fn write_pays_write_then_latch_read() {
        // wr(1): fetch 2 + exec 1 core; write 1 + read 1 stall; stop 1
        let (r, tape) = drive(
            &[0x07, 0x81, 0x02],
            RunLimits::default(),
            TactProfile::ELECTRONIC,
        );
        assert_eq!(
            r.stats,
            RunStats {
                steps: 1,
                core_tacts: 4,
                stall_tacts: 2
            }
        );
        assert_eq!(tape.marked_cells(), vec![0]);
    }

    #[test]
    fn call_costs_eight_with_rel32() {
        // [0]=call +1 (target 6 = entry), [5]=stop, [6]=entry, [7]=ret
        // call: fetch 5 + ent-read 1 + push 1 + exec 1 = 8 core (docs/isa.md (timing model))
        // entry(Nop): 2; ret: fetch 1 + pop 1 + exec 1 = 3; stop: 1
        let code = [0x0A, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B];
        let (r, _) = drive(&code, RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Stopped);
        assert_eq!(
            r.stats,
            RunStats {
                steps: 3,
                core_tacts: 14,
                stall_tacts: 0
            }
        );
    }

    #[test]
    fn step_limit_traps() {
        // jmp rel8 -2: instr_end 2, target 0 → infinite loop
        let code = [0x08, 0xFE];
        let limits = RunLimits {
            max_steps: Some(10),
            max_tacts: None,
        };
        let (r, _) = drive(&code, limits, TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Trapped(Trap::StepLimit));
        assert_eq!(r.stats.steps, 10);
    }

    #[test]
    fn tact_limit_traps() {
        let code = [0x08, 0xFE];
        let limits = RunLimits {
            max_steps: None,
            max_tacts: Some(25),
        };
        let (r, _) = drive(&code, limits, TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Trapped(Trap::TactLimit));
        assert!(r.stats.total_tacts() >= 25);
    }

    #[test]
    fn stack_overflow_surfaces_as_trap() {
        // call rel32 -6 → target 0 (the ent) = infinite recursion
        let code = [0x0E, 0x0A, 0xFA, 0xFF, 0xFF, 0xFF, 0x02];
        // entry at 0 is 0x0E (TestArch entry marker), call at 1, instr_end 6, off -6 → 0
        let (r, _) = drive(&code, RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Trapped(Trap::StackOverflow)); // capacity 4
    }

    #[test]
    fn halt_and_device_state_reported() {
        let (r, _) = drive(&[0x03], RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Halted);
        assert_eq!(
            r.stats,
            RunStats {
                steps: 0,
                core_tacts: 1,
                stall_tacts: 0
            }
        );
    }

    #[test]
    fn return_stack_reports_depth_and_entries() {
        let mut s = ReturnStack::new(2);
        assert_eq!(s.depth(), 0);
        assert!(s.push(7));
        assert!(s.push(9));
        assert!(!s.push(11)); // full
        assert_eq!(s.entries(), &[7, 9]);
        assert_eq!(s.pop(), Some(9));
        assert_eq!(s.depth(), 1);
    }
}
