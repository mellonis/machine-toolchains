//! The processor VM: sans-I/O core, bus protocol, devices (spec §4).

pub mod arch;
pub mod bus;
pub mod trap;

pub use arch::{Arch, MicroOp, Operand, OperandKind};
pub use bus::{BusRequest, BusResponse, CoreEvent};
pub use trap::{DeviceFault, Trap};
