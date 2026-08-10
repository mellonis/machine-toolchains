//! The async device surface (docs/core.md (async session)): a poll-shaped
//! mirror of the bus protocol's device requests. The WAIT/READY reading:
//! `issue` puts the command on the bus; each `poll` samples the READY line
//! on a clock edge — `Pending` is READY low, `Ready` is READY high with
//! the data latched.

use super::Tape;
use crate::vm::trap::DeviceFault;

/// One device transaction, as the bus sees it (minus the device index —
/// a device object IS one device).
///
/// Deliberately exhaustive — no `#[non_exhaustive]`: a device must execute
/// every command it is sent; an unknown command silently ignored would
/// corrupt the tape contract. Protocol growth is a compile-visible,
/// semver-visible event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCmd {
    MoveLeft,
    MoveRight,
    Read,
    Write { index: u32 },
}

/// The data half of a completed transaction. Exhaustive on the same
/// contract as [`DeviceCmd`]: every consumer handles every case, so
/// protocol growth is compile-visible.
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
/// `Pending` samples never tick the tact counter. Exhaustive on the same
/// contract as [`DeviceCmd`]: every consumer handles every case, so
/// protocol growth is compile-visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    Ready {
        reply: DeviceReply,
        cost: Option<u32>,
    },
}

/// A tape device the machine can genuinely wait on. Contract: one command
/// in flight per device — `issue` while a command is pending is a caller
/// bug; `poll` with nothing in flight reports `Pending`. The accessor methods
/// `head()` and `alphabet_size()` report the device's settled state — the state
/// as of the last completed transaction; they do not reflect an in-flight command.
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
        Self {
            inner,
            pending: None,
        }
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
            DevicePoll::Ready {
                reply: DeviceReply::Ok,
                cost: None
            }
        );
        dev.issue(DeviceCmd::Read);
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready {
                reply: DeviceReply::Symbol(1),
                cost: None
            }
        );
        dev.issue(DeviceCmd::MoveRight);
        assert_eq!(
            dev.poll(),
            DevicePoll::Ready {
                reply: DeviceReply::Ok,
                cost: None
            }
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

    #[test]
    #[should_panic(expected = "one command in flight")]
    fn issue_while_in_flight_panics() {
        let mut dev = SyncAsAsync::new(InfiniteTape::new());
        dev.issue(DeviceCmd::Write { index: 1 });
        dev.issue(DeviceCmd::Read);
    }
}
