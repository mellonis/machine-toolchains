//! Tape devices behind the device bus (spec §4.2). Index-based; the
//! processor never sees glyphs and never knows the head position.

mod annular_tape;
mod infinite_tape;
mod strict_tape;

pub use annular_tape::AnnularTape;
pub use infinite_tape::InfiniteTape;
pub use strict_tape::StrictTape;

use super::trap::DeviceFault;

pub trait Tape {
    fn alphabet_size(&self) -> u32;
    fn left(&mut self);
    fn right(&mut self);
    fn read(&self) -> u32;
    fn write(&mut self, index: u32) -> Result<(), DeviceFault>;
}
