//! Tape devices behind the device bus (spec §4.2). Index-based; the
//! processor never sees glyphs and never knows the head position.

mod infinite_tape;

pub use infinite_tape::InfiniteTape;

use super::trap::DeviceFault;

pub trait Tape {
    fn alphabet_size(&self) -> u32;
    fn left(&mut self);
    fn right(&mut self);
    fn read(&self) -> u32;
    fn write(&mut self, index: u32) -> Result<(), DeviceFault>;
}
