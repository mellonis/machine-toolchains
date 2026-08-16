//! Tape devices behind the device bus (docs/core.md (the tape and device
//! bus)). Index-based; the processor never sees glyphs and never knows
//! the head position.

mod annular_tape;
mod async_device;
mod infinite_tape;
mod latency_tape;
mod strict_tape;
mod wide_tape;

pub use annular_tape::AnnularTape;
pub use async_device::{AsyncTapeDevice, DeviceCmd, DevicePoll, DeviceReply, SyncAsAsync};
pub use infinite_tape::InfiniteTape;
pub use latency_tape::{LatencyProfile, LatencyTape};
pub use strict_tape::StrictTape;
pub use wide_tape::WideTape;

use super::trap::DeviceFault;

pub trait Tape {
    fn alphabet_size(&self) -> u32;
    fn left(&mut self);
    fn right(&mut self);
    fn read(&self) -> u32;
    fn write(&mut self, index: u32) -> Result<(), DeviceFault>;
    /// Current head position (annular tapes: the current index).
    fn head(&self) -> i64;

    /// Direct positional write (docs/core.md (the tape and device bus)):
    /// walks the head to `pos` via `left()`/`right()`, writes `index`, then
    /// walks the head back to where it started — including when the write
    /// itself faults, so a caller (a DAP adapter setting a variable, say)
    /// never leaves the head displaced by a failed probe. `pos` is in the
    /// same coordinate space `head()` reports.
    fn poke(&mut self, pos: i64, index: u32) -> Result<(), DeviceFault> {
        let origin = self.head();
        while self.head() < pos {
            self.right();
        }
        while self.head() > pos {
            self.left();
        }
        let result = self.write(index);
        while self.head() < origin {
            self.right();
        }
        while self.head() > origin {
            self.left();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::devices::{InfiniteTape, StrictTape};

    #[test]
    fn poke_writes_the_target_cell_and_restores_head() {
        let mut tape = InfiniteTape::new(); // head at 0
        assert_eq!(tape.poke(3, 1), Ok(()));
        assert_eq!(tape.head(), 0); // head restored to where it started
        while tape.head() < 3 {
            tape.right();
        }
        assert_eq!(tape.read(), 1); // the poked cell actually got written
    }

    #[test]
    fn poke_restores_head_even_when_the_write_faults() {
        let mut tape = StrictTape::new(InfiniteTape::new()); // head at 0, blank
        tape.left();
        tape.left(); // head at -2, away from the poke target
        assert_eq!(tape.head(), -2);
        // pos 4 is blank; writing 0 there (== its current value) is a
        // strict-cell violation.
        assert_eq!(tape.poke(4, 0), Err(DeviceFault::StrictCellViolation));
        assert_eq!(tape.head(), -2); // restored despite the fault
    }
}
