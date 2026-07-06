//! Traps: the processor's controlled stop on an execution error
//! (docs/isa.md (execution)).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFault {
    IndexOutsideAlphabet { index: u32 },
    StrictCellViolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trap {
    InvalidOpcode { opcode: u8, at: u32 },
    CodeOutOfBounds { at: u32 },
    BadOperand { at: u32 },
    CallTargetNotEntry { target: u32 },
    StackOverflow,
    StackUnderflow,
    StepLimit,
    TactLimit,
    Device { fault: DeviceFault },
}

impl std::fmt::Display for Trap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOpcode { opcode, at } => {
                write!(f, "invalid opcode {opcode:#04x} at {at:#010x}")
            }
            Self::CodeOutOfBounds { at } => {
                write!(f, "execution left the code image at {at:#010x}")
            }
            Self::BadOperand { at } => write!(f, "malformed operand at {at:#010x}"),
            Self::CallTargetNotEntry { target } => {
                write!(f, "call target {target:#010x} is not an entry marker")
            }
            Self::StackOverflow => write!(f, "return-stack overflow"),
            Self::StackUnderflow => write!(f, "return-stack underflow"),
            Self::StepLimit => write!(f, "step limit exceeded"),
            Self::TactLimit => write!(f, "tact limit exceeded"),
            Self::Device { fault } => write!(f, "device fault: {fault:?}"),
        }
    }
}
