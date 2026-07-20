//! The processor VM: sans-I/O core, bus protocol, devices (docs/core.md).

pub mod arch;
pub mod bus;
pub mod core;
pub mod debug;
pub mod devices;
pub mod driver;
pub(crate) mod frame;
pub mod machine;
pub(crate) mod table;
pub mod trap;

pub use arch::{Arch, MicroOp, Operand, OperandKind, encode_operand};
pub use bus::{BusRequest, BusResponse, CoreEvent};
pub use core::{Core, FramesMeta};
pub use debug::{DebugEvent, DebugSession, PauseCause};
pub use devices::{InfiniteTape, StrictTape, Tape, WideTape};
pub use driver::{Outcome, ReturnStack, RunLimits, RunResult, RunStats, TactProfile, run};
pub use machine::{ArchRegistry, LoadError, Machine, RunOptions};
pub use trap::{DeviceFault, RaisedTrapKind, Trap};
