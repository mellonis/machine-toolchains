//! PM-1: the Post-machine instruction set (spec §5), as an arch module
//! for the mtc-core VM. Pure table — no state.

use mtc_core::vm::{Arch, MicroOp, Operand, OperandKind, Trap};

pub mod opcodes {
    pub const NOP: u8 = 0x01;
    pub const STP: u8 = 0x02;
    pub const HLT: u8 = 0x03;
    pub const LFT: u8 = 0x04;
    pub const RGT: u8 = 0x05;
    pub const WR: u8 = 0x06;
    pub const JMP: u8 = 0x08;
    pub const JM: u8 = 0x09;
    pub const JNM: u8 = 0x0A;
    pub const CALL: u8 = 0x0B;
    pub const RET: u8 = 0x0C;
    pub const ENT: u8 = 0x0D;
    pub const BRK: u8 = 0x0E;
    // Short forms: far | 0x10 (spec §5).
    pub const JMP_S: u8 = 0x18;
    pub const JM_S: u8 = 0x19;
    pub const JNM_S: u8 = 0x1A;
    pub const CALL_S: u8 = 0x1B;
}

use opcodes::*;

/// PM-1 matches against the mark index (spec §4.1).
const MARK: u32 = 1;

/// Default rendering glyphs (index 0 = blank, 1 = mark) for tooling with
/// no tape at hand; a loaded `.pmt`'s own alphabet always wins (spec §6.3).
pub const DEFAULT_GLYPHS: [&str; 2] = [" ", "*"];

pub struct Pm1;

impl Arch for Pm1 {
    fn arch_id(&self) -> u8 {
        mtc_core::formats::ARCH_PM1
    }

    fn operand_kind(&self, opcode: u8) -> Option<OperandKind> {
        match opcode {
            NOP | STP | HLT | LFT | RGT | RET | ENT | BRK => Some(OperandKind::None),
            WR => Some(OperandKind::SymbolVec),
            JMP | JM | JNM | CALL => Some(OperandKind::RelI32),
            JMP_S | JM_S | JNM_S | CALL_S => Some(OperandKind::RelI8),
            _ => None,
        }
    }

    fn lower(&self, opcode: u8, operand: &Operand) -> Result<Vec<MicroOp>, Trap> {
        let off32 = |o: &Operand| match o {
            Operand::I32(v) => Ok(*v),
            _ => Err(Trap::BadOperand { at: 0 }),
        };
        let off8 = |o: &Operand| match o {
            Operand::I8(v) => Ok(i32::from(*v)),
            _ => Err(Trap::BadOperand { at: 0 }),
        };
        Ok(match opcode {
            NOP | ENT => vec![MicroOp::Nop],
            STP => vec![MicroOp::Stop],
            HLT => vec![MicroOp::Halt],
            BRK => vec![MicroOp::Brk],
            LFT => vec![MicroOp::MoveLeft, MicroOp::LatchMatch(MARK)],
            RGT => vec![MicroOp::MoveRight, MicroOp::LatchMatch(MARK)],
            WR => match operand {
                Operand::Symbols(s) if s.len() == 1 => {
                    vec![MicroOp::Write(s[0]), MicroOp::LatchMatch(MARK)]
                }
                _ => return Err(Trap::BadOperand { at: 0 }),
            },
            JMP => vec![MicroOp::JumpRel(off32(operand)?)],
            JMP_S => vec![MicroOp::JumpRel(off8(operand)?)],
            JM => vec![MicroOp::JumpRelIf {
                off: off32(operand)?,
                when_match: true,
            }],
            JM_S => vec![MicroOp::JumpRelIf {
                off: off8(operand)?,
                when_match: true,
            }],
            JNM => vec![MicroOp::JumpRelIf {
                off: off32(operand)?,
                when_match: false,
            }],
            JNM_S => vec![MicroOp::JumpRelIf {
                off: off8(operand)?,
                when_match: false,
            }],
            CALL => vec![MicroOp::Call(off32(operand)?)],
            CALL_S => vec![MicroOp::Call(off8(operand)?)],
            RET => vec![MicroOp::Ret],
            _ => return Err(Trap::InvalidOpcode { opcode, at: 0 }),
        })
    }

    fn is_entry_marker(&self, byte: u8) -> bool {
        byte == ENT
    }
}

#[cfg(test)]
mod tests {
    use super::opcodes::*;
    use super::*;
    use mtc_core::vm::{MicroOp, Operand, OperandKind};

    #[test]
    fn operand_kind_table_matches_spec() {
        let a = Pm1;
        for op in [NOP, STP, HLT, LFT, RGT, RET, ENT, BRK] {
            assert!(
                matches!(a.operand_kind(op), Some(OperandKind::None)),
                "opcode {op:#04x}"
            );
        }
        assert!(matches!(a.operand_kind(WR), Some(OperandKind::SymbolVec)));
        for op in [JMP, JM, JNM, CALL] {
            assert!(
                matches!(a.operand_kind(op), Some(OperandKind::RelI32)),
                "opcode {op:#04x}"
            );
        }
        for op in [JMP_S, JM_S, JNM_S, CALL_S] {
            assert!(
                matches!(a.operand_kind(op), Some(OperandKind::RelI8)),
                "opcode {op:#04x}"
            );
            assert_eq!(op, (op - 0x10) | 0x10); // short = far | 0x10 (self-check of constants)
        }
        for invalid in [0x00u8, 0x07, 0x0F, 0x10, 0x17, 0x1C, 0x80, 0xFF] {
            assert!(
                a.operand_kind(invalid).is_none(),
                "opcode {invalid:#04x} must be invalid"
            );
        }
    }

    #[test]
    fn short_form_rule_holds_for_constants() {
        assert_eq!(JMP_S, JMP | 0x10);
        assert_eq!(JM_S, JM | 0x10);
        assert_eq!(JNM_S, JNM | 0x10);
        assert_eq!(CALL_S, CALL | 0x10);
    }

    #[test]
    fn lowerings_match_semantics() {
        let a = Pm1;
        assert_eq!(
            a.lower(LFT, &Operand::None).unwrap(),
            vec![MicroOp::MoveLeft, MicroOp::LatchMatch(1)]
        );
        assert_eq!(
            a.lower(RGT, &Operand::None).unwrap(),
            vec![MicroOp::MoveRight, MicroOp::LatchMatch(1)]
        );
        assert_eq!(
            a.lower(WR, &Operand::Symbols(vec![1])).unwrap(),
            vec![MicroOp::Write(1), MicroOp::LatchMatch(1)]
        );
        assert_eq!(
            a.lower(JMP, &Operand::I32(-6)).unwrap(),
            vec![MicroOp::JumpRel(-6)]
        );
        assert_eq!(
            a.lower(JM_S, &Operand::I8(-3)).unwrap(),
            vec![MicroOp::JumpRelIf {
                off: -3,
                when_match: true
            }]
        );
        assert_eq!(
            a.lower(JNM, &Operand::I32(9)).unwrap(),
            vec![MicroOp::JumpRelIf {
                off: 9,
                when_match: false
            }]
        );
        assert_eq!(
            a.lower(CALL_S, &Operand::I8(1)).unwrap(),
            vec![MicroOp::Call(1)]
        );
        assert_eq!(a.lower(RET, &Operand::None).unwrap(), vec![MicroOp::Ret]);
        assert_eq!(a.lower(STP, &Operand::None).unwrap(), vec![MicroOp::Stop]);
        assert_eq!(a.lower(HLT, &Operand::None).unwrap(), vec![MicroOp::Halt]);
        assert_eq!(a.lower(ENT, &Operand::None).unwrap(), vec![MicroOp::Nop]);
        assert_eq!(a.lower(BRK, &Operand::None).unwrap(), vec![MicroOp::Brk]);
        assert_eq!(a.lower(NOP, &Operand::None).unwrap(), vec![MicroOp::Nop]);
    }

    #[test]
    fn wr_requires_exactly_one_symbol() {
        let a = Pm1;
        assert!(a.lower(WR, &Operand::Symbols(vec![0])).is_ok());
        assert!(a.lower(WR, &Operand::Symbols(vec![1, 2])).is_err());
        assert!(a.lower(WR, &Operand::Symbols(vec![])).is_err());
    }

    #[test]
    fn identity() {
        let a = Pm1;
        assert_eq!(a.arch_id(), mtc_core::formats::ARCH_PM1);
        assert!(a.is_entry_marker(ENT));
        assert!(!a.is_entry_marker(NOP));
    }

    #[test]
    fn default_glyphs_are_blank_then_mark() {
        assert_eq!(DEFAULT_GLYPHS, [" ", "*"]);
    }
}
