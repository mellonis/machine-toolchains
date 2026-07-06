//! The architecture interface: all instruction knowledge enters here
//! (README (workspace layout): core is arch-agnostic by contract, and
//! this trait is where PM-1-specific knowledge is supplied from outside).

use super::trap::Trap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandKind {
    None,
    RelI8,
    RelI32,
    /// Self-delimiting symbol vector: 7-bit payloads, high bit on the last.
    SymbolVec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    None,
    I8(i8),
    I32(i32),
    Symbols(Vec<u32>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicroOp {
    MoveLeft,
    MoveRight,
    Write(u32),
    LatchMatch(u32),
    JumpRel(i32),
    JumpRelIf { off: i32, when_match: bool },
    Call(i32),
    Ret,
    Stop,
    Halt,
    Brk,
    Nop,
}

pub trait Arch {
    fn arch_id(&self) -> u8;
    /// `None` means: not an opcode of this architecture (trap on fetch).
    fn operand_kind(&self, opcode: u8) -> Option<OperandKind>;
    fn lower(&self, opcode: u8, operand: &Operand) -> Result<Vec<MicroOp>, Trap>;
    fn is_entry_marker(&self, byte: u8) -> bool;
}

/// Encode an operand to its wire form (docs/isa.md). The inverse of the
/// core's fetch-time decoding — property-tested against it.
pub fn encode_operand(operand: &Operand) -> Result<Vec<u8>, &'static str> {
    Ok(match operand {
        Operand::None => Vec::new(),
        Operand::I8(v) => vec![*v as u8],
        Operand::I32(v) => v.to_le_bytes().to_vec(),
        Operand::Symbols(symbols) => {
            let Some((last, init)) = symbols.split_last() else {
                return Err("symbol vector must not be empty");
            };
            if symbols.iter().any(|&s| s > 0x7F) {
                return Err("symbol payload exceeds 7 bits");
            }
            let mut out: Vec<u8> = init.iter().map(|&s| s as u8).collect();
            out.push(*last as u8 | 0x80);
            out
        }
    })
}

/// Fake architecture for core tests — proves core is arch-agnostic.
/// 0x01 nop | 0x02 stop | 0x03 halt | 0x04 brk | 0x05 left+latch |
/// 0x06 right+latch | 0x07 wr(vec)+latch | 0x08 jmp rel8 | 0x09 jm rel32 |
/// 0x0A call rel32 | 0x0B ret | 0x0E entry marker (lowers to Nop)
#[cfg(test)]
pub(crate) mod test_arch {
    use super::*;

    pub(crate) struct TestArch;

    impl Arch for TestArch {
        fn arch_id(&self) -> u8 {
            0x7F
        }

        fn operand_kind(&self, opcode: u8) -> Option<OperandKind> {
            match opcode {
                0x01..=0x06 | 0x0B | 0x0E => Some(OperandKind::None),
                0x07 => Some(OperandKind::SymbolVec),
                0x08 => Some(OperandKind::RelI8),
                0x09 | 0x0A => Some(OperandKind::RelI32),
                _ => None,
            }
        }

        fn lower(&self, opcode: u8, operand: &Operand) -> Result<Vec<MicroOp>, Trap> {
            Ok(match (opcode, operand) {
                (0x01, _) | (0x0E, _) => vec![MicroOp::Nop],
                (0x02, _) => vec![MicroOp::Stop],
                (0x03, _) => vec![MicroOp::Halt],
                (0x04, _) => vec![MicroOp::Brk],
                (0x05, _) => vec![MicroOp::MoveLeft, MicroOp::LatchMatch(1)],
                (0x06, _) => vec![MicroOp::MoveRight, MicroOp::LatchMatch(1)],
                (0x07, Operand::Symbols(s)) if s.len() == 1 => {
                    vec![MicroOp::Write(s[0]), MicroOp::LatchMatch(1)]
                }
                (0x07, _) => return Err(Trap::BadOperand { at: 0 }),
                (0x08, Operand::I8(o)) => vec![MicroOp::JumpRel(i32::from(*o))],
                (0x09, Operand::I32(o)) => {
                    vec![MicroOp::JumpRelIf {
                        off: *o,
                        when_match: true,
                    }]
                }
                (0x0A, Operand::I32(o)) => vec![MicroOp::Call(*o)],
                (0x0B, _) => vec![MicroOp::Ret],
                _ => return Err(Trap::BadOperand { at: 0 }),
            })
        }

        fn is_entry_marker(&self, byte: u8) -> bool {
            byte == 0x0E
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_arch::TestArch;
    use super::*;

    #[test]
    fn operand_kinds_cover_the_table() {
        let a = TestArch;
        assert!(matches!(a.operand_kind(0x01), Some(OperandKind::None)));
        assert!(matches!(a.operand_kind(0x07), Some(OperandKind::SymbolVec)));
        assert!(matches!(a.operand_kind(0x08), Some(OperandKind::RelI8)));
        assert!(matches!(a.operand_kind(0x09), Some(OperandKind::RelI32)));
        assert!(a.operand_kind(0x55).is_none());
    }

    #[test]
    fn lower_write_requires_exactly_one_symbol() {
        let a = TestArch;
        assert!(a.lower(0x07, &Operand::Symbols(vec![1])).is_ok());
        assert!(a.lower(0x07, &Operand::Symbols(vec![1, 2])).is_err());
        assert!(a.lower(0x07, &Operand::None).is_err());
    }

    #[test]
    fn entry_marker_is_recognized() {
        let a = TestArch;
        assert!(a.is_entry_marker(0x0E));
        assert!(!a.is_entry_marker(0x01));
    }

    #[test]
    fn encode_operand_matches_wire_format() {
        use super::encode_operand;
        assert_eq!(encode_operand(&Operand::None).unwrap(), Vec::<u8>::new());
        assert_eq!(encode_operand(&Operand::I8(-3)).unwrap(), vec![0xFD]);
        assert_eq!(
            encode_operand(&Operand::I32(-6)).unwrap(),
            vec![0xFA, 0xFF, 0xFF, 0xFF]
        );
        assert_eq!(
            encode_operand(&Operand::Symbols(vec![1])).unwrap(),
            vec![0x81]
        );
        assert_eq!(
            encode_operand(&Operand::Symbols(vec![3, 0x7F, 0])).unwrap(),
            vec![0x03, 0x7F, 0x80]
        );
        assert!(encode_operand(&Operand::Symbols(vec![])).is_err());
        assert!(encode_operand(&Operand::Symbols(vec![0x80])).is_err());
    }
}
