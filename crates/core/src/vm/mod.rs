//! The processor VM: sans-I/O core, bus protocol, devices (spec §4).

pub mod arch;
pub mod bus;
pub mod core;
pub mod devices;
pub mod driver;
pub mod machine;
pub mod trap;

pub use arch::{Arch, MicroOp, Operand, OperandKind};
pub use bus::{BusRequest, BusResponse, CoreEvent};
pub use core::Core;
pub use devices::Tape;
pub use driver::{Outcome, ReturnStack, RunLimits, RunResult, RunStats, TactProfile, run};
pub use machine::{ArchRegistry, LoadError, Machine, RunOptions};
pub use trap::{DeviceFault, Trap};
