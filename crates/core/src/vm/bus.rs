//! Bus protocol between the sans-I/O core and its driver (spec §4).

use super::trap::{DeviceFault, Trap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusRequest {
    CodeRead { addr: u32 },
    StackPush { value: u32 },
    StackPop,
    DeviceMoveLeft { dev: u8 },
    DeviceMoveRight { dev: u8 },
    DeviceRead { dev: u8 },
    DeviceWrite { dev: u8, index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusResponse {
    Byte(u8),
    OutOfCode,
    Ok,
    StackFull,
    Value(u32),
    StackEmpty,
    Symbol(u32),
    Fault(DeviceFault),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreEvent {
    Request(BusRequest),
    Step,
    Stopped,
    Halted,
    Trapped(Trap),
}
