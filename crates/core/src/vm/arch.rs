//! The architecture interface: all instruction knowledge enters here
//! (README (workspace layout): core is arch-agnostic by contract, and
//! this trait is where PM-1-specific knowledge is supplied from outside).

use super::trap::{RaisedTrapKind, Trap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandKind {
    None,
    RelI8,
    RelI32,
    /// Self-delimiting symbol vector: 7-bit payloads, high bit on the last.
    SymbolVec,
    /// Absolute table-section offset: 4 bytes, u32 LE (RelI32's width but
    /// unsigned and absolute — table walks address the table space, not
    /// instruction-relative code).
    TableRef,
    /// Self-delimiting move vector: [`SymbolVec`]'s wire form exactly
    /// (7-bit payloads, high bit on the last), carrying per-tape move
    /// codes — 0 = stay, 1 = left, 2 = right — instead of symbol
    /// indices. It decodes to [`Operand::Symbols`] like `SymbolVec`;
    /// the distinction is an assembly-vocabulary and rendering matter
    /// (docs/formats.md (assembly text)), not a fetch matter.
    MoveVec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    None,
    I8(i8),
    I32(i32),
    Symbols(Vec<u32>),
    /// A [`OperandKind::TableRef`] operand: the absolute table offset.
    Table(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicroOp {
    MoveLeft {
        dev: u8,
    },
    MoveRight {
        dev: u8,
    },
    Write {
        dev: u8,
        index: u32,
    },
    /// Latch the symbol under `dev`'s head into TR slot `slot`.
    Read {
        dev: u8,
        slot: u8,
    },
    LatchMatch(u32),
    JumpRel(i32),
    JumpRelIf {
        off: i32,
        when_match: bool,
    },
    Call(i32),
    Ret,
    /// Walk the match table at byte offset `table` in table ROM against TR;
    /// set MR (0 = no row matched).
    MatchTable {
        table: u32,
    },
    /// Jump through the dispatch table at `table` by MR;
    /// MR = 0 traps NoTransition.
    DispatchJump {
        table: u32,
    },
    /// Trap explicitly with a typed kind (the `trap #kind` instruction family).
    Raise {
        kind: RaisedTrapKind,
    },
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
/// core's fetch-time decoding — property-tested against it. A move
/// vector ([`OperandKind::MoveVec`]) arrives as [`Operand::Symbols`]
/// and encodes identically to a symbol vector; its payloads are the
/// move codes 0/1/2, comfortably within the 7-bit element budget.
pub fn encode_operand(operand: &Operand) -> Result<Vec<u8>, &'static str> {
    Ok(match operand {
        Operand::None => Vec::new(),
        Operand::I8(v) => vec![*v as u8],
        Operand::I32(v) => v.to_le_bytes().to_vec(),
        Operand::Table(v) => v.to_le_bytes().to_vec(),
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
/// 0x0A call rel32 | 0x0B ret | 0x0E entry marker (lowers to Nop) |
/// 0x10 read dev0→slot0 + dev1→slot1 (probes TR latching) |
/// 0x11 mtc @table | 0x12 djmp @table (TableRef: abs u32 table offset; probes the table engine) |
/// 0x13 wr(vec) on dev 1 | 0x14 left on dev 1 (probes device-indexed tape micro-ops) |
/// 0x15 raise unmapped-read | 0x16 raise unmapped-write (probes Raise micro-op) |
/// 0x17 read dev0→slot0 (single-tape TR latch, for the table-program end-to-end test)
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
                0x01..=0x06 | 0x0B | 0x0E | 0x10 | 0x14..=0x17 => Some(OperandKind::None),
                0x07 | 0x13 => Some(OperandKind::SymbolVec),
                0x08 => Some(OperandKind::RelI8),
                0x09 | 0x0A => Some(OperandKind::RelI32),
                0x11 | 0x12 => Some(OperandKind::TableRef),
                _ => None,
            }
        }

        fn lower(&self, opcode: u8, operand: &Operand) -> Result<Vec<MicroOp>, Trap> {
            Ok(match (opcode, operand) {
                (0x01, _) | (0x0E, _) => vec![MicroOp::Nop],
                (0x02, _) => vec![MicroOp::Stop],
                (0x03, _) => vec![MicroOp::Halt],
                (0x04, _) => vec![MicroOp::Brk],
                (0x05, _) => vec![MicroOp::MoveLeft { dev: 0 }, MicroOp::LatchMatch(1)],
                (0x06, _) => vec![MicroOp::MoveRight { dev: 0 }, MicroOp::LatchMatch(1)],
                (0x07, Operand::Symbols(s)) if s.len() == 1 => {
                    vec![
                        MicroOp::Write {
                            dev: 0,
                            index: s[0],
                        },
                        MicroOp::LatchMatch(1),
                    ]
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
                (0x10, _) => vec![
                    MicroOp::Read { dev: 0, slot: 0 },
                    MicroOp::Read { dev: 1, slot: 1 },
                ],
                (0x11, Operand::Table(o)) => vec![MicroOp::MatchTable { table: *o }],
                (0x12, Operand::Table(o)) => vec![MicroOp::DispatchJump { table: *o }],
                (0x13, Operand::Symbols(s)) if s.len() == 1 => {
                    vec![MicroOp::Write {
                        dev: 1,
                        index: s[0],
                    }]
                }
                (0x14, _) => vec![MicroOp::MoveLeft { dev: 1 }],
                (0x15, _) => vec![MicroOp::Raise {
                    kind: RaisedTrapKind::UnmappedRead,
                }],
                (0x16, _) => vec![MicroOp::Raise {
                    kind: RaisedTrapKind::UnmappedWrite,
                }],
                (0x17, _) => vec![MicroOp::Read { dev: 0, slot: 0 }],
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
        assert!(matches!(a.operand_kind(0x11), Some(OperandKind::TableRef)));
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
        // TableRef: u32 LE, absolute — the high bit is a value bit, not a sign.
        assert_eq!(
            encode_operand(&Operand::Table(7)).unwrap(),
            vec![7, 0, 0, 0]
        );
        assert_eq!(
            encode_operand(&Operand::Table(0x8000_0001)).unwrap(),
            vec![0x01, 0x00, 0x00, 0x80]
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
