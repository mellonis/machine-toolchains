//! The processor VM: sans-I/O core, bus protocol, devices (docs/isa.md).

pub mod arch;
pub mod bus;
pub mod core;
pub mod debug;
pub mod devices;
pub mod driver;
pub mod machine;
pub(crate) mod table;
pub mod trap;

pub use arch::{Arch, MicroOp, Operand, OperandKind, encode_operand};
pub use bus::{BusRequest, BusResponse, CoreEvent};
pub use core::Core;
pub use debug::{DebugEvent, DebugSession, PauseCause};
pub use devices::{InfiniteTape, StrictTape, Tape};
pub use driver::{Outcome, ReturnStack, RunLimits, RunResult, RunStats, TactProfile, run};
pub use machine::{ArchRegistry, LoadError, Machine, RunOptions};
pub use trap::{DeviceFault, RaisedTrapKind, Trap};
