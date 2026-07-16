//! PM-1's sibling: the TM-1 instruction set as an arch module for the
//! mtc-core VM. Pure table — no state beyond the tape count. TM-1 is a
//! multi-tape Turing machine: up to sixteen tapes, one head each, driven in
//! lockstep by the vector `rd`/`wr`/`mov` instructions and branched by the
//! shared match/dispatch table engine.
//!
//! Unlike PM-1, TM-1 never latches the match register from a tape op. Its
//! match register (MR) is written only by `mtc` (the match-table walk);
//! `rd`/`wr`/`mov` leave it untouched, so `jm`/`jnm` test the most recent
//! `mtc` outcome regardless of intervening tape motion. No lowering here
//! ever emits `LatchMatch` — that per-op marking is a PM-1-ism.

use mtc_core::vm::{Arch, MicroOp, Operand, OperandKind, Trap};

pub mod opcodes {
    pub const NOP: u8 = 0x01;
    pub const STP: u8 = 0x02;
    pub const HLT: u8 = 0x03;
    /// Vector read: latch every tape head into its TR slot in one fetch.
    pub const RD: u8 = 0x04;
    /// Walk the match table against TR, setting MR.
    pub const MTC: u8 = 0x05;
    /// Jump through the dispatch table indexed by MR.
    pub const DJMP: u8 = 0x06;
    /// Vector write: one symbol per tape, `0x7F` keeps a cell untouched.
    pub const WR: u8 = 0x07;
    pub const JMP: u8 = 0x08;
    pub const JM: u8 = 0x09;
    pub const JNM: u8 = 0x0A;
    pub const CALL: u8 = 0x0B;
    pub const RET: u8 = 0x0C;
    pub const ENT: u8 = 0x0D;
    pub const BRK: u8 = 0x0E;
    /// Vector move: one step per tape (0 stay, 1 left, 2 right).
    pub const MOV: u8 = 0x0F;
    /// Short form: far `| 0x10`. Only `call` has one so far; the linker
    /// selects it during relaxation — the assembler always emits far.
    pub const CALL_S: u8 = 0x1B;

    // Reserved TM-1 opcodes — numbered but deliberately not defined or
    // lowered here yet, so `operand_kind` returns `None` and a program that
    // uses one traps on fetch until the producer that emits it lands:
    //   0x11 trap   — raise a typed trap explicitly (future linker stubs)
    //   0x12 wrmv   — fused per-tape write+move in one fetch (future codegen)
    //   0x13 call.m — medium-range call form (future linker relaxation)
    //   0x14 retx   — extended return unwinding a call frame (future frames)
}

use opcodes::*;

/// `wr` element value that leaves a tape's current cell untouched (the
/// keep marker); every other value ≤ `0x7E` is a symbol to write.
const KEEP: u32 = 0x7F;
/// `mov` element values: no motion, one step left, one step right.
const STAY: u32 = 0;
const LEFT: u32 = 1;
const RIGHT: u32 = 2;

/// The TM-1 architecture, parameterized by how many tapes its programs
/// drive. The count fixes the width of the vector `rd`/`wr`/`mov`
/// instructions and the number of TR slots `rd` fills.
pub struct Tm1 {
    tape_count: u8,
}

impl Tm1 {
    /// `tape_count` must be in `1..=16` — one head per tape, and the TR
    /// bank (each tape's `rd` slot) is sixteen wide, so slot == tape index
    /// is always in range.
    pub fn new(tape_count: u8) -> Self {
        assert!(
            (1..=16).contains(&tape_count),
            "TM-1 supports 1..=16 tapes, got {tape_count}"
        );
        Self { tape_count }
    }
}

impl Arch for Tm1 {
    fn arch_id(&self) -> u8 {
        mtc_core::formats::ARCH_TM1
    }

    fn operand_kind(&self, opcode: u8) -> Option<OperandKind> {
        match opcode {
            NOP | STP | HLT | RD | RET | ENT | BRK => Some(OperandKind::None),
            MTC | DJMP => Some(OperandKind::TableRef),
            WR => Some(OperandKind::SymbolVec),
            // Same self-delimiting wire form as `wr`, but the move-vector
            // kind selects the `[<, ., >]` assembly vocabulary and
            // rendering; both fetch to `Operand::Symbols`, so the
            // lowering below reads them uniformly.
            MOV => Some(OperandKind::MoveVec),
            JMP | JM | JNM | CALL => Some(OperandKind::RelI32),
            CALL_S => Some(OperandKind::RelI8),
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
        let table = |o: &Operand| match o {
            Operand::Table(v) => Ok(*v),
            _ => Err(Trap::BadOperand { at: 0 }),
        };
        let n = usize::from(self.tape_count);
        Ok(match opcode {
            NOP | ENT => vec![MicroOp::Nop],
            STP => vec![MicroOp::Stop],
            HLT => vec![MicroOp::Halt],
            BRK => vec![MicroOp::Brk],
            RET => vec![MicroOp::Ret],
            // One read per tape; slot == device index (TR bank is 16 wide,
            // tape cap 16 makes the mapping total).
            RD => (0..self.tape_count)
                .map(|dev| MicroOp::Read { dev, slot: dev })
                .collect(),
            MTC => vec![MicroOp::MatchTable {
                table: table(operand)?,
            }],
            DJMP => vec![MicroOp::DispatchJump {
                table: table(operand)?,
            }],
            // One symbol per tape. `0x7F` keeps the cell; any other value
            // must be a 7-bit symbol (≤ `0x7E`) — a wider payload is a
            // malformed operand.
            WR => {
                let syms = match operand {
                    Operand::Symbols(s) if s.len() == n => s,
                    _ => return Err(Trap::BadOperand { at: 0 }),
                };
                let mut ops = Vec::new();
                for (dev, &v) in (0u8..self.tape_count).zip(syms.iter()) {
                    if v == KEEP {
                        continue;
                    }
                    if v > KEEP {
                        return Err(Trap::BadOperand { at: 0 });
                    }
                    ops.push(MicroOp::Write { dev, index: v });
                }
                ops
            }
            // One move per tape: 0 stays (skipped), 1 left, 2 right; any
            // other value is a malformed operand.
            MOV => {
                let moves = match operand {
                    Operand::Symbols(s) if s.len() == n => s,
                    _ => return Err(Trap::BadOperand { at: 0 }),
                };
                let mut ops = Vec::new();
                for (dev, &m) in (0u8..self.tape_count).zip(moves.iter()) {
                    match m {
                        STAY => {}
                        LEFT => ops.push(MicroOp::MoveLeft { dev }),
                        RIGHT => ops.push(MicroOp::MoveRight { dev }),
                        _ => return Err(Trap::BadOperand { at: 0 }),
                    }
                }
                ops
            }
            JMP => vec![MicroOp::JumpRel(off32(operand)?)],
            JM => vec![MicroOp::JumpRelIf {
                off: off32(operand)?,
                when_match: true,
            }],
            JNM => vec![MicroOp::JumpRelIf {
                off: off32(operand)?,
                when_match: false,
            }],
            CALL => vec![MicroOp::Call(off32(operand)?)],
            CALL_S => vec![MicroOp::Call(off8(operand)?)],
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

    /// Every defined TM-1 opcode.
    const ALL_OPCODES: [u8; 16] = [
        NOP, STP, HLT, RD, MTC, DJMP, WR, JMP, JM, JNM, CALL, RET, ENT, BRK, MOV, CALL_S,
    ];

    /// A valid operand for `op`'s operand kind, filling symbol vectors to
    /// `arch.tape_count` with values that lower without error for both `wr`
    /// (0 → write 0) and `mov` (0 → stay).
    fn valid_operand(arch: &Tm1, op: u8) -> Operand {
        match arch.operand_kind(op).unwrap() {
            OperandKind::None => Operand::None,
            OperandKind::RelI8 => Operand::I8(0),
            OperandKind::RelI32 => Operand::I32(0),
            // Both vector kinds fetch to `Operand::Symbols`; 0 lowers
            // cleanly for `wr` (write 0) and `mov` (stay) alike.
            OperandKind::SymbolVec | OperandKind::MoveVec => {
                Operand::Symbols(vec![0; usize::from(arch.tape_count)])
            }
            OperandKind::TableRef => Operand::Table(0),
            // No TM-1 opcode carries an immediate or a framed-call operand
            // yet (those instructions arrive with the frames dialect).
            OperandKind::Imm8 | OperandKind::FramedCall => {
                unreachable!("no TM-1 opcode uses this operand kind yet")
            }
        }
    }

    #[test]
    fn operand_kind_table_matches_spec() {
        let a = Tm1::new(2);
        for op in [NOP, STP, HLT, RD, RET, ENT, BRK] {
            assert!(
                matches!(a.operand_kind(op), Some(OperandKind::None)),
                "opcode {op:#04x}"
            );
        }
        for op in [MTC, DJMP] {
            assert!(
                matches!(a.operand_kind(op), Some(OperandKind::TableRef)),
                "opcode {op:#04x}"
            );
        }
        assert!(matches!(a.operand_kind(WR), Some(OperandKind::SymbolVec)));
        assert!(matches!(a.operand_kind(MOV), Some(OperandKind::MoveVec)));
        for op in [JMP, JM, JNM, CALL] {
            assert!(
                matches!(a.operand_kind(op), Some(OperandKind::RelI32)),
                "opcode {op:#04x}"
            );
        }
        assert!(matches!(a.operand_kind(CALL_S), Some(OperandKind::RelI8)));
    }

    #[test]
    fn operand_kind_is_none_for_unknown_and_reserved_opcodes() {
        let a = Tm1::new(2);
        for invalid in [0x00u8, 0x10, 0x11, 0x12, 0x13, 0x14, 0x1A, 0x1C, 0x80, 0xFF] {
            assert!(
                a.operand_kind(invalid).is_none(),
                "opcode {invalid:#04x} must be unknown"
            );
        }
    }

    #[test]
    fn short_form_rule_holds_for_constants() {
        assert_eq!(CALL_S, CALL | 0x10);
    }

    #[test]
    fn nullary_lowerings() {
        let a = Tm1::new(2);
        assert_eq!(a.lower(NOP, &Operand::None).unwrap(), vec![MicroOp::Nop]);
        assert_eq!(a.lower(ENT, &Operand::None).unwrap(), vec![MicroOp::Nop]);
        assert_eq!(a.lower(STP, &Operand::None).unwrap(), vec![MicroOp::Stop]);
        assert_eq!(a.lower(HLT, &Operand::None).unwrap(), vec![MicroOp::Halt]);
        assert_eq!(a.lower(BRK, &Operand::None).unwrap(), vec![MicroOp::Brk]);
        assert_eq!(a.lower(RET, &Operand::None).unwrap(), vec![MicroOp::Ret]);
    }

    #[test]
    fn rd_reads_every_tape_with_slot_equal_dev() {
        let a = Tm1::new(4);
        assert_eq!(
            a.lower(RD, &Operand::None).unwrap(),
            vec![
                MicroOp::Read { dev: 0, slot: 0 },
                MicroOp::Read { dev: 1, slot: 1 },
                MicroOp::Read { dev: 2, slot: 2 },
                MicroOp::Read { dev: 3, slot: 3 },
            ]
        );
        // A single-tape machine reads exactly one device.
        assert_eq!(
            Tm1::new(1).lower(RD, &Operand::None).unwrap(),
            vec![MicroOp::Read { dev: 0, slot: 0 }]
        );
    }

    #[test]
    fn table_ops_carry_the_u32() {
        let a = Tm1::new(2);
        assert_eq!(
            a.lower(MTC, &Operand::Table(0x1234)).unwrap(),
            vec![MicroOp::MatchTable { table: 0x1234 }]
        );
        assert_eq!(
            a.lower(DJMP, &Operand::Table(7)).unwrap(),
            vec![MicroOp::DispatchJump { table: 7 }]
        );
        // Wrong operand shape is a malformed operand.
        assert!(a.lower(MTC, &Operand::None).is_err());
        assert!(a.lower(DJMP, &Operand::I32(0)).is_err());
    }

    #[test]
    fn wr_writes_per_tape_and_honors_the_keep_marker() {
        let a = Tm1::new(3);
        // [5, keep, 0] → write dev 0 = 5, skip dev 1, write dev 2 = 0.
        assert_eq!(
            a.lower(WR, &Operand::Symbols(vec![5, KEEP, 0])).unwrap(),
            vec![
                MicroOp::Write { dev: 0, index: 5 },
                MicroOp::Write { dev: 2, index: 0 },
            ]
        );
        // All-keep lowers to no work at all.
        assert_eq!(
            a.lower(WR, &Operand::Symbols(vec![KEEP, KEEP, KEEP]))
                .unwrap(),
            vec![]
        );
    }

    #[test]
    fn mov_moves_per_tape_and_skips_stays() {
        let a = Tm1::new(3);
        // [left, stay, right] → move dev 0 left, skip dev 1, move dev 2 right.
        assert_eq!(
            a.lower(MOV, &Operand::Symbols(vec![LEFT, STAY, RIGHT]))
                .unwrap(),
            vec![MicroOp::MoveLeft { dev: 0 }, MicroOp::MoveRight { dev: 2 },]
        );
        // All-stay lowers to nothing.
        assert_eq!(
            a.lower(MOV, &Operand::Symbols(vec![STAY, STAY, STAY]))
                .unwrap(),
            vec![]
        );
    }

    #[test]
    fn relative_and_call_lowerings() {
        let a = Tm1::new(2);
        assert_eq!(
            a.lower(JMP, &Operand::I32(-6)).unwrap(),
            vec![MicroOp::JumpRel(-6)]
        );
        assert_eq!(
            a.lower(JM, &Operand::I32(9)).unwrap(),
            vec![MicroOp::JumpRelIf {
                off: 9,
                when_match: true
            }]
        );
        assert_eq!(
            a.lower(JNM, &Operand::I32(-3)).unwrap(),
            vec![MicroOp::JumpRelIf {
                off: -3,
                when_match: false
            }]
        );
        assert_eq!(
            a.lower(CALL, &Operand::I32(12)).unwrap(),
            vec![MicroOp::Call(12)]
        );
        assert_eq!(
            a.lower(CALL_S, &Operand::I8(-1)).unwrap(),
            vec![MicroOp::Call(-1)]
        );
        // Wrong operand widths are malformed operands.
        assert!(a.lower(JMP, &Operand::I8(0)).is_err());
        assert!(a.lower(CALL_S, &Operand::I32(0)).is_err());
    }

    #[test]
    fn wr_and_mov_require_operand_length_equal_to_tape_count() {
        let a = Tm1::new(3);
        for op in [WR, MOV] {
            assert!(
                a.lower(op, &Operand::Symbols(vec![0, 0, 0])).is_ok(),
                "opcode {op:#04x} length 3 on a 3-tape machine"
            );
            assert!(a.lower(op, &Operand::Symbols(vec![0, 0])).is_err());
            assert!(a.lower(op, &Operand::Symbols(vec![0, 0, 0, 0])).is_err());
            assert!(a.lower(op, &Operand::None).is_err());
        }
    }

    #[test]
    fn wr_rejects_payload_above_the_keep_marker() {
        let a = Tm1::new(2);
        // 0x7F is the legal keep marker; 0x80 is not a 7-bit symbol.
        assert!(a.lower(WR, &Operand::Symbols(vec![KEEP, KEEP])).is_ok());
        assert!(a.lower(WR, &Operand::Symbols(vec![0x80, 0])).is_err());
    }

    #[test]
    fn mov_rejects_values_above_right() {
        let a = Tm1::new(2);
        assert!(a.lower(MOV, &Operand::Symbols(vec![3, 0])).is_err());
    }

    #[test]
    fn invalid_opcode_lowers_to_invalid_opcode_trap() {
        let a = Tm1::new(2);
        assert_eq!(
            a.lower(0x11, &Operand::None),
            Err(Trap::InvalidOpcode {
                opcode: 0x11,
                at: 0
            })
        );
    }

    #[test]
    fn no_lowering_ever_emits_latch_match() {
        let a = Tm1::new(2);
        for op in ALL_OPCODES {
            let ops = a.lower(op, &valid_operand(&a, op)).unwrap();
            assert!(
                !ops.iter().any(|m| matches!(m, MicroOp::LatchMatch(_))),
                "opcode {op:#04x} must not latch the match register"
            );
        }
    }

    #[test]
    fn identity() {
        let a = Tm1::new(2);
        assert_eq!(a.arch_id(), mtc_core::formats::ARCH_TM1);
        assert!(a.is_entry_marker(ENT));
        assert!(!a.is_entry_marker(NOP));
    }

    #[test]
    #[should_panic(expected = "1..=16 tapes")]
    fn zero_tapes_panics() {
        Tm1::new(0);
    }

    #[test]
    #[should_panic(expected = "1..=16 tapes")]
    fn seventeen_tapes_panics() {
        Tm1::new(17);
    }

    #[test]
    fn sixteen_tapes_is_the_upper_bound() {
        let a = Tm1::new(16);
        assert_eq!(a.lower(RD, &Operand::None).unwrap().len(), 16);
    }
}
