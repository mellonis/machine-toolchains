//! Traps: the processor's controlled stop on an execution error
//! (docs/isa.md (execution)).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFault {
    IndexOutsideAlphabet { index: u32 },
    StrictCellViolation,
    NoSuchDevice { dev: u8 },
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
    NoTransition { at: u32 },
    TableOutOfBounds { at: u32 },
    DispatchOutOfRange { at: u32 },
    UnmappedRead { at: u32 },
    UnmappedWrite { at: u32 },
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
            Self::NoTransition { at } => write!(f, "no applicable transition at {at:#010x}"),
            Self::TableOutOfBounds { at } => write!(f, "table read out of bounds at {at:#010x}"),
            Self::DispatchOutOfRange { at } => {
                write!(f, "dispatch index out of range at {at:#010x}")
            }
            Self::UnmappedRead { at } => write!(f, "unmapped symbol read at {at:#010x}"),
            Self::UnmappedWrite { at } => write!(f, "unmapped symbol write at {at:#010x}"),
        }
    }
}

/// Trap kinds an architecture may raise explicitly via `MicroOp::Raise`
/// (the `trap #kind` instruction family).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaisedTrapKind {
    UnmappedRead,
    UnmappedWrite,
}
