//! A latency decorator over any sync tape (docs/core.md (async session)):
//! the `StrictTape` pattern applied to time. Holds READY low for a
//! configured number of polls per operation, then performs the operation
//! and reports a configured cost. The reference implementation of the
//! async device contract, the transport counterpart of a mechanical
//! `TactProfile`, and the test vehicle for the pumped session.

use super::Tape;
use super::async_device::{AsyncTapeDevice, DeviceCmd, DevicePoll, execute_on_tape};

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
        Self {
            inner,
            profile,
            in_flight: None,
        }
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
            DevicePoll::Ready {
                reply: DeviceReply::Ok,
                cost: Some(5)
            }
        );
        // The write really landed:
        dev.issue(DeviceCmd::Read);
        dev.poll();
        dev.poll();
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready {
                reply: DeviceReply::Symbol(1),
                cost: Some(4)
            }
        );
    }

    #[test]
    fn immediate_electronic_is_ready_at_once_with_unit_costs() {
        let mut dev = LatencyTape::new(InfiniteTape::new(), LatencyProfile::IMMEDIATE_ELECTRONIC);
        dev.issue(DeviceCmd::MoveRight);
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready {
                reply: DeviceReply::Ok,
                cost: Some(1)
            }
        );
    }

    #[test]
    fn poll_without_issue_is_pending() {
        let mut dev = LatencyTape::new(InfiniteTape::new(), TWO_POLLS);
        assert_eq!(dev.poll(), DevicePoll::Pending);
    }

    #[test]
    #[should_panic(expected = "one command in flight")]
    fn issue_while_in_flight_panics() {
        let mut dev = LatencyTape::new(InfiniteTape::new(), TWO_POLLS);
        dev.issue(DeviceCmd::Write { index: 1 });
        dev.issue(DeviceCmd::Read);
    }
}
