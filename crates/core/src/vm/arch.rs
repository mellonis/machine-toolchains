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
    /// A plain 8-bit immediate: one raw byte, decoded to
    /// [`Operand::Imm`]. Carries a small numeric argument directly in the
    /// instruction (e.g. a trap kind or a return-exit index) — not an
    /// address and not relocatable.
    Imm8,
    /// A framed call operand: 8 bytes — a signed 32-bit code-relative
    /// displacement (LE) followed by an unsigned 32-bit table-section
    /// offset (LE) naming the frame descriptor to activate. The
    /// displacement half relocates like a call target; the offset half is
    /// an absolute table-space address like [`OperandKind::TableRef`].
    FramedCall,
    /// Two self-delimiting compact groups back to back: a WRITE vector
    /// then a MOVE vector, each 7-bit payloads with the high bit
    /// terminating its group ([`OperandKind::SymbolVec`]'s wire form,
    /// twice). Decodes to [`Operand::WriteMove`]; the write group carries
    /// per-tape symbol indices (`0x7F` = keep) and the move group per-tape
    /// move codes (0 = stay, 1 = left, 2 = right). Both groups must be
    /// non-empty. The fused write+move of one formal step (docs/formats.md
    /// (assembly text)).
    WriteMoveVec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    None,
    I8(i8),
    I32(i32),
    Symbols(Vec<u32>),
    /// A [`OperandKind::TableRef`] operand: the absolute table offset.
    Table(u32),
    /// An [`OperandKind::Imm8`] operand: the raw immediate byte.
    Imm(u8),
    /// An [`OperandKind::FramedCall`] operand: the code-relative
    /// displacement of the call target and the table-section offset of
    /// the frame descriptor to activate.
    FramedCall {
        rel: i32,
        table: u32,
    },
    /// An [`OperandKind::WriteMoveVec`] operand: the per-tape write
    /// symbols (`0x7F` = keep) then the per-tape move codes (0/1/2), each
    /// group self-delimiting on the wire.
    WriteMove {
        writes: Vec<u32>,
        moves: Vec<u32>,
    },
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
    /// Latch every visible tape into TR (slot k ← tape k) and set the TR
    /// width to the number of tapes read. Under the identity frame that is
    /// all `device_count` physical tapes; under an active frame it is the
    /// frame's `arity` virtual tapes, each read through its symbol map.
    /// Expands at execution time — an architecture's lowering stays
    /// width-agnostic.
    ReadAll,
    /// A framed call: like [`MicroOp::Call`], plus it activates the frame
    /// resolved for call SITE `site` on the callee — the runtime composes
    /// `FR' = compose[FR][site]` and loads `directory[FR'-1]` (the caller's
    /// frame is restored on return). Requires the frames execution profile.
    CallFrame {
        rel: i32,
        site: u32,
    },
    /// A multi-exit return: leave the active frame through exit `k` of its
    /// exit vector (the pushed return address is discarded). Requires the
    /// frames execution profile.
    RetX {
        k: u8,
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
        Operand::Imm(v) => vec![*v],
        // Displacement first (signed, LE), then the frame table offset
        // (unsigned, LE): 8 bytes, the inverse of the core's FramedCall
        // fetch.
        Operand::FramedCall { rel, table } => {
            let mut out = rel.to_le_bytes().to_vec();
            out.extend(table.to_le_bytes());
            out
        }
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
        // The write group (7-bit payloads, `0x7F` = keep legal, high bit
        // terminating) immediately followed by the move group (same wire
        // form, codes 0/1/2 only). Neither group may be empty — an empty
        // group has no last element to carry its terminator bit.
        Operand::WriteMove { writes, moves } => {
            let Some((w_last, w_init)) = writes.split_last() else {
                return Err("write-move write group must not be empty");
            };
            if writes.iter().any(|&s| s > 0x7F) {
                return Err("write payload exceeds 7 bits");
            }
            let Some((m_last, m_init)) = moves.split_last() else {
                return Err("write-move move group must not be empty");
            };
            if moves.iter().any(|&m| m > 2) {
                return Err("move code exceeds 2");
            }
            let mut out: Vec<u8> = w_init.iter().map(|&s| s as u8).collect();
            out.push(*w_last as u8 | 0x80);
            out.extend(m_init.iter().map(|&m| m as u8));
            out.push(*m_last as u8 | 0x80);
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
/// 0x17 read dev0→slot0 (single-tape TR latch, for the table-program end-to-end test) |
/// 0x18 read-all | 0x19 callframe rel32 → call site 0 |
/// 0x1A retx#0 | 0x1B retx#1 | 0x1C callframe rel32 → call site 1 |
/// 0x1D retx#2 (probe the frames profile; the fake encodings hard-wire the
/// call SITE index / exit index per opcode — rel rides the same rel32 wire
/// as plain call, and the real framed-call operand encoding is an assembler
/// concern outside the core)
#[cfg(test)]
pub(crate) mod test_arch {
    use super::*;

    pub(crate) struct TestArch;

    /// Table-section offset the frames tests place the second fake frame
    /// descriptor at (site 1 resolves here through the region).
    pub(crate) const FRAME2_OFFSET: u32 = 64;

    impl Arch for TestArch {
        fn arch_id(&self) -> u8 {
            0x7F
        }

        fn operand_kind(&self, opcode: u8) -> Option<OperandKind> {
            match opcode {
                0x01..=0x06 | 0x0B | 0x0E | 0x10 | 0x14..=0x18 | 0x1A | 0x1B | 0x1D => {
                    Some(OperandKind::None)
                }
                0x07 | 0x13 => Some(OperandKind::SymbolVec),
                0x08 => Some(OperandKind::RelI8),
                0x09 | 0x0A | 0x19 | 0x1C => Some(OperandKind::RelI32),
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
                (0x18, _) => vec![MicroOp::ReadAll],
                (0x19, Operand::I32(o)) => vec![MicroOp::CallFrame { rel: *o, site: 0 }],
                (0x1A, _) => vec![MicroOp::RetX { k: 0 }],
                (0x1B, _) => vec![MicroOp::RetX { k: 1 }],
                (0x1C, Operand::I32(o)) => vec![MicroOp::CallFrame { rel: *o, site: 1 }],
                (0x1D, _) => vec![MicroOp::RetX { k: 2 }],
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

        // WriteMove: the write group then the move group, each terminated
        // by the high bit on its last element. `0x7F` keep encodes as-is
        // (0xFF once the terminator bit is set); move codes stay 0/1/2.
        assert_eq!(
            encode_operand(&Operand::WriteMove {
                writes: vec![1],
                moves: vec![2],
            })
            .unwrap(),
            vec![0x81, 0x82]
        );
        assert_eq!(
            encode_operand(&Operand::WriteMove {
                writes: vec![3, 0x7F],
                moves: vec![0, 1],
            })
            .unwrap(),
            vec![0x03, 0xFF, 0x00, 0x81]
        );
        // Empty either group refuses (no last element for the terminator).
        assert!(
            encode_operand(&Operand::WriteMove {
                writes: vec![],
                moves: vec![1],
            })
            .is_err()
        );
        assert!(
            encode_operand(&Operand::WriteMove {
                writes: vec![1],
                moves: vec![],
            })
            .is_err()
        );
        // A write payload above 7 bits and a move code above 2 both refuse.
        assert!(
            encode_operand(&Operand::WriteMove {
                writes: vec![0x80],
                moves: vec![0],
            })
            .is_err()
        );
        assert!(
            encode_operand(&Operand::WriteMove {
                writes: vec![0],
                moves: vec![3],
            })
            .is_err()
        );
    }
}
