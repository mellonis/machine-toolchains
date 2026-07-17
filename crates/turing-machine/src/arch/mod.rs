//! PM-1's sibling: the TM-1 instruction set as an arch module for the
//! mtc-core VM. Pure table — no state at all: lowering is width-agnostic
//! (`rd` expands to `ReadAll` at execution, `wr`/`mov` accept any
//! `1..=16`-wide vector). TM-1 is a multi-tape Turing machine: up to
//! sixteen tapes, one head each, driven in lockstep by the vector
//! `rd`/`wr`/`mov` instructions and branched by the shared match/dispatch
//! table engine.
//!
//! Unlike PM-1, TM-1 never latches the match register from a tape op. Its
//! match register (MR) is written only by `mtc` (the match-table walk);
//! `rd`/`wr`/`mov` leave it untouched, so `jm`/`jnm` test the most recent
//! `mtc` outcome regardless of intervening tape motion. No lowering here
//! ever emits `LatchMatch` — that per-op marking is a PM-1-ism.

use mtc_core::vm::{Arch, MicroOp, Operand, OperandKind, RaisedTrapKind, Trap};

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
    /// Fused per-tape write+move (`wrmv [w…], [m…]`): the write vector then
    /// the move vector, one formal step — all writes precede all moves
    /// (behaviorally the `wr; mov` pair). `0x7F` keeps a cell in the write
    /// group; the move group uses the same 0 stay / 1 left / 2 right codes.
    pub const WRMV: u8 = 0x12;
    /// Raise a typed trap explicitly (`trap #kind`): kind `0` = unmapped
    /// read, `1` = unmapped write. The frames stubs a linker composition
    /// emits reach for these to signal a crossed map hole.
    pub const TRAP: u8 = 0x11;
    /// Framed call (`call.m target, F`): call `target` and activate the
    /// frame descriptor at table label `F` for the callee. The caller's
    /// frame is restored on return.
    pub const CALL_M: u8 = 0x13;
    /// Multi-exit frame return (`retx #k`): leave the active frame through
    /// exit `k` of its exit vector (the pushed return address is discarded).
    pub const RETX: u8 = 0x14;
    /// Short form: far `| 0x10`. Only `call` has one so far; the linker
    /// selects it during relaxation — the assembler always emits far.
    pub const CALL_S: u8 = 0x1B;
}

use opcodes::*;

/// `wr` element value that leaves a tape's current cell untouched (the
/// keep marker); every other value ≤ `0x7E` is a symbol to write.
const KEEP: u32 = 0x7F;
/// `mov` element values: no motion, one step left, one step right.
const STAY: u32 = 0;
const LEFT: u32 = 1;
const RIGHT: u32 = 2;

/// The TM-1 architecture. Lowering is width-agnostic — `rd` expands to
/// `ReadAll` at execution, and `wr`/`mov` accept any `1..=16`-wide vector,
/// deriving each device index from the vector position — so the arch holds
/// no per-machine state. The frame (or, under the identity frame, the
/// machine width wired into the VM) validates device indices at run time.
pub struct Tm1;

impl Tm1 {
    /// `tape_count` must be in `1..=16` (one head per tape). The arch is
    /// width-agnostic now, but the constructor keeps validating the
    /// machine's declared tape count as a cheap sanity guard on a loaded
    /// image; the value is not otherwise retained.
    pub fn new(tape_count: u8) -> Self {
        assert!(
            (1..=16).contains(&tape_count),
            "TM-1 supports 1..=16 tapes, got {tape_count}"
        );
        Self
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
            // Fused write+move: two self-delimiting groups (the write
            // vector then the move vector), decoding to `Operand::WriteMove`.
            WRMV => Some(OperandKind::WriteMoveVec),
            // `trap #kind` and `retx #k` carry a plain immediate byte;
            // `call.m target, F` carries the framed-call operand (a rel
            // displacement plus a frame table offset).
            TRAP | RETX => Some(OperandKind::Imm8),
            CALL_M => Some(OperandKind::FramedCall),
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
        let imm = |o: &Operand| match o {
            Operand::Imm(v) => Ok(*v),
            _ => Err(Trap::BadOperand { at: 0 }),
        };
        Ok(match opcode {
            NOP | ENT => vec![MicroOp::Nop],
            STP => vec![MicroOp::Stop],
            HLT => vec![MicroOp::Halt],
            BRK => vec![MicroOp::Brk],
            RET => vec![MicroOp::Ret],
            // Latch every visible tape into TR in one micro-op. `ReadAll`
            // expands at execution to the machine width (identity frame) or
            // the active frame's arity, so lowering stays width-agnostic.
            RD => vec![MicroOp::ReadAll],
            MTC => vec![MicroOp::MatchTable {
                table: table(operand)?,
            }],
            DJMP => vec![MicroOp::DispatchJump {
                table: table(operand)?,
            }],
            // One symbol per vector position; the device index IS the
            // position. Any `1..=16`-wide vector lowers — a routine body
            // authored at arity M < the machine width is legal, and the
            // static width check lives in the assembler for signed
            // functions (docs/formats.md (assembly text)). `0x7F` keeps the
            // cell; any other value must be a 7-bit symbol (≤ `0x7E`).
            WR => {
                let syms = match operand {
                    Operand::Symbols(s) if (1..=16).contains(&s.len()) => s,
                    _ => return Err(Trap::BadOperand { at: 0 }),
                };
                let mut ops = Vec::new();
                for (dev, &v) in (0u8..).zip(syms.iter()) {
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
            // One move per vector position: 0 stays (skipped), 1 left, 2
            // right; any other value is a malformed operand. Same
            // width-agnostic rule as `wr`.
            MOV => {
                let moves = match operand {
                    Operand::Symbols(s) if (1..=16).contains(&s.len()) => s,
                    _ => return Err(Trap::BadOperand { at: 0 }),
                };
                let mut ops = Vec::new();
                for (dev, &m) in (0u8..).zip(moves.iter()) {
                    match m {
                        STAY => {}
                        LEFT => ops.push(MicroOp::MoveLeft { dev }),
                        RIGHT => ops.push(MicroOp::MoveRight { dev }),
                        _ => return Err(Trap::BadOperand { at: 0 }),
                    }
                }
                ops
            }
            // `wrmv [w…], [m…]`: the fused write+move of one formal step.
            // ALL writes precede ALL moves — behaviorally the `wr; mov`
            // pair (docs/formats.md (assembly text)). The two vectors share
            // one arity; a width mismatch, an empty/over-16 group, or an
            // out-of-vocabulary payload is a malformed operand (house
            // style: the arch rejects at lower, not the wire codec).
            WRMV => {
                let (writes, moves) = match operand {
                    Operand::WriteMove { writes, moves } => (writes, moves),
                    _ => return Err(Trap::BadOperand { at: 0 }),
                };
                if writes.len() != moves.len() || !(1..=16).contains(&writes.len()) {
                    return Err(Trap::BadOperand { at: 0 });
                }
                let mut ops = Vec::new();
                for (dev, &v) in (0u8..).zip(writes.iter()) {
                    if v == KEEP {
                        continue;
                    }
                    if v > KEEP {
                        return Err(Trap::BadOperand { at: 0 });
                    }
                    ops.push(MicroOp::Write { dev, index: v });
                }
                for (dev, &m) in (0u8..).zip(moves.iter()) {
                    match m {
                        STAY => {}
                        LEFT => ops.push(MicroOp::MoveLeft { dev }),
                        RIGHT => ops.push(MicroOp::MoveRight { dev }),
                        _ => return Err(Trap::BadOperand { at: 0 }),
                    }
                }
                ops
            }
            // `trap #kind`: 0 → unmapped-read, 1 → unmapped-write; any other
            // kind is a malformed operand (numeric kinds leave room for
            // named kinds later without a grammar break).
            TRAP => match imm(operand)? {
                0 => vec![MicroOp::Raise {
                    kind: RaisedTrapKind::UnmappedRead,
                }],
                1 => vec![MicroOp::Raise {
                    kind: RaisedTrapKind::UnmappedWrite,
                }],
                _ => return Err(Trap::BadOperand { at: 0 }),
            },
            // `call.m target, F`: the framed-call operand carries the rel
            // displacement and, post-link, the call SITE index the runtime
            // composes through (docs/formats.md (frames region)). The
            // authoring surface names the descriptor `F`; the linker
            // rewrites the operand half to the site index.
            CALL_M => match operand {
                Operand::FramedCall { rel, table } => vec![MicroOp::CallFrame {
                    rel: *rel,
                    site: *table,
                }],
                _ => return Err(Trap::BadOperand { at: 0 }),
            },
            // `retx #k`: leave the active frame through exit `k`.
            RETX => vec![MicroOp::RetX { k: imm(operand)? }],
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
    const ALL_OPCODES: [u8; 20] = [
        NOP, STP, HLT, RD, MTC, DJMP, WR, JMP, JM, JNM, CALL, RET, ENT, BRK, MOV, WRMV, TRAP,
        CALL_M, RETX, CALL_S,
    ];

    /// A valid operand for `op`'s operand kind, with values that lower
    /// without error for every opcode of that kind (0 → write 0 / stay for
    /// vectors, `#0` → unmapped-read / exit 0 for immediates).
    fn valid_operand(op: u8) -> Operand {
        match Tm1::new(2).operand_kind(op).unwrap() {
            OperandKind::None => Operand::None,
            OperandKind::RelI8 => Operand::I8(0),
            OperandKind::RelI32 => Operand::I32(0),
            // Both vector kinds fetch to `Operand::Symbols`; 0 lowers
            // cleanly for `wr` (write 0) and `mov` (stay) alike.
            OperandKind::SymbolVec | OperandKind::MoveVec => Operand::Symbols(vec![0, 0]),
            // `wrmv` fetches to `Operand::WriteMove`; equal-width groups of
            // 0 lower cleanly (write 0 on every tape, stay on every tape).
            OperandKind::WriteMoveVec => Operand::WriteMove {
                writes: vec![0, 0],
                moves: vec![0, 0],
            },
            OperandKind::TableRef => Operand::Table(0),
            // `trap #0` / `retx #0` lower without error; `call.m` takes a
            // rel displacement plus a frame table offset.
            OperandKind::Imm8 => Operand::Imm(0),
            OperandKind::FramedCall => Operand::FramedCall { rel: 0, table: 0 },
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
        assert!(matches!(
            a.operand_kind(WRMV),
            Some(OperandKind::WriteMoveVec)
        ));
        for op in [TRAP, RETX] {
            assert!(
                matches!(a.operand_kind(op), Some(OperandKind::Imm8)),
                "opcode {op:#04x}"
            );
        }
        assert!(matches!(
            a.operand_kind(CALL_M),
            Some(OperandKind::FramedCall)
        ));
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
        // 0x11 (trap), 0x12 (wrmv), 0x13 (call.m), 0x14 (retx) are all
        // defined now — no TM-1 opcode below 0x15 remains reserved. 0x10 is
        // an unused gap; 0x1A/0x1C are undefined above the range.
        for invalid in [0x00u8, 0x10, 0x1A, 0x1C, 0x80, 0xFF] {
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
    fn rd_lowers_to_a_single_read_all() {
        // `rd` no longer expands per-tape at lower time — it lowers to one
        // width-agnostic `ReadAll`, which the VM expands to the machine
        // width or the active frame's arity at execution. The lowering is
        // independent of the constructor's declared tape count.
        for tapes in [1u8, 4, 16] {
            assert_eq!(
                Tm1::new(tapes).lower(RD, &Operand::None).unwrap(),
                vec![MicroOp::ReadAll]
            );
        }
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
    fn wrmv_lowers_all_writes_then_all_moves() {
        let a = Tm1::new(2);
        // [5, 1], [>, <]: write dev0=5, dev1=1, THEN move dev0 right, dev1
        // left — every write precedes every move (one formal step, the
        // `wr; mov` pair fused).
        assert_eq!(
            a.lower(
                WRMV,
                &Operand::WriteMove {
                    writes: vec![5, 1],
                    moves: vec![RIGHT, LEFT],
                }
            )
            .unwrap(),
            vec![
                MicroOp::Write { dev: 0, index: 5 },
                MicroOp::Write { dev: 1, index: 1 },
                MicroOp::MoveRight { dev: 0 },
                MicroOp::MoveLeft { dev: 1 },
            ]
        );
    }

    #[test]
    fn wrmv_all_keep_write_elides_the_writes() {
        let a = Tm1::new(2);
        // Keep on every tape: no Write micro-ops, only the moves survive.
        assert_eq!(
            a.lower(
                WRMV,
                &Operand::WriteMove {
                    writes: vec![KEEP, KEEP],
                    moves: vec![LEFT, RIGHT],
                }
            )
            .unwrap(),
            vec![MicroOp::MoveLeft { dev: 0 }, MicroOp::MoveRight { dev: 1 }]
        );
    }

    #[test]
    fn wrmv_all_stay_move_elides_the_moves() {
        let a = Tm1::new(2);
        // Stay on every tape: only the writes survive.
        assert_eq!(
            a.lower(
                WRMV,
                &Operand::WriteMove {
                    writes: vec![1, 0],
                    moves: vec![STAY, STAY],
                }
            )
            .unwrap(),
            vec![
                MicroOp::Write { dev: 0, index: 1 },
                MicroOp::Write { dev: 1, index: 0 },
            ]
        );
    }

    #[test]
    fn wrmv_width_mismatch_and_bad_shapes_are_bad_operand() {
        let a = Tm1::new(2);
        // The write and move vectors must share one arity.
        assert!(
            a.lower(
                WRMV,
                &Operand::WriteMove {
                    writes: vec![1, 0],
                    moves: vec![RIGHT],
                }
            )
            .is_err()
        );
        // Empty groups and over-16 are malformed.
        assert!(
            a.lower(
                WRMV,
                &Operand::WriteMove {
                    writes: vec![],
                    moves: vec![],
                }
            )
            .is_err()
        );
        assert!(
            a.lower(
                WRMV,
                &Operand::WriteMove {
                    writes: vec![0; 17],
                    moves: vec![0; 17],
                }
            )
            .is_err()
        );
        // A non-WriteMove operand shape is malformed.
        assert!(a.lower(WRMV, &Operand::None).is_err());
    }

    #[test]
    fn wrmv_rejects_out_of_vocabulary_payloads() {
        let a = Tm1::new(2);
        // A write payload above the keep marker, and a move code above right.
        assert!(
            a.lower(
                WRMV,
                &Operand::WriteMove {
                    writes: vec![0x80, 0],
                    moves: vec![STAY, STAY],
                }
            )
            .is_err()
        );
        assert!(
            a.lower(
                WRMV,
                &Operand::WriteMove {
                    writes: vec![0, 0],
                    moves: vec![3, STAY],
                }
            )
            .is_err()
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
    fn wr_and_mov_accept_any_width_1_to_16() {
        // Width no longer has to equal the machine's tape count: a routine
        // body authored at a narrower arity lowers freely. The arch accepts
        // any `1..=16`-wide vector; the per-function arity match is the
        // assembler's static check (signed functions only).
        let a = Tm1::new(3);
        for op in [WR, MOV] {
            for width in [1usize, 2, 3, 4, 16] {
                assert!(
                    a.lower(op, &Operand::Symbols(vec![0; width])).is_ok(),
                    "opcode {op:#04x} width {width}"
                );
            }
            // Empty and over-wide vectors, and a non-vector operand, are
            // still malformed.
            assert!(a.lower(op, &Operand::Symbols(vec![])).is_err());
            assert!(a.lower(op, &Operand::Symbols(vec![0; 17])).is_err());
            assert!(a.lower(op, &Operand::None).is_err());
        }
    }

    #[test]
    fn vector_device_index_is_the_position_not_the_tape_count() {
        // The device index derives from the vector position, independent of
        // the constructor's tape count: a width-3 vector on a 2-tape arch
        // still targets devices 0..=2.
        let a = Tm1::new(2);
        assert_eq!(
            a.lower(WR, &Operand::Symbols(vec![KEEP, KEEP, 3])).unwrap(),
            vec![MicroOp::Write { dev: 2, index: 3 }]
        );
        assert_eq!(
            a.lower(MOV, &Operand::Symbols(vec![STAY, STAY, RIGHT]))
                .unwrap(),
            vec![MicroOp::MoveRight { dev: 2 }]
        );
    }

    #[test]
    fn trap_lowers_to_the_two_raise_kinds_and_rejects_other_kinds() {
        let a = Tm1::new(2);
        assert_eq!(
            a.lower(TRAP, &Operand::Imm(0)).unwrap(),
            vec![MicroOp::Raise {
                kind: RaisedTrapKind::UnmappedRead
            }]
        );
        assert_eq!(
            a.lower(TRAP, &Operand::Imm(1)).unwrap(),
            vec![MicroOp::Raise {
                kind: RaisedTrapKind::UnmappedWrite
            }]
        );
        // Kind 2 has no meaning yet: a lower-time malformed operand.
        assert_eq!(
            a.lower(TRAP, &Operand::Imm(2)),
            Err(Trap::BadOperand { at: 0 })
        );
        // Wrong operand shape is also malformed.
        assert!(a.lower(TRAP, &Operand::None).is_err());
    }

    #[test]
    fn call_m_lowers_the_rel_and_site_into_a_call_frame() {
        let a = Tm1::new(2);
        // Post-link the operand's second half is the call SITE index; the
        // arch passes it through verbatim into `CallFrame.site`.
        assert_eq!(
            a.lower(CALL_M, &Operand::FramedCall { rel: -12, table: 3 })
                .unwrap(),
            vec![MicroOp::CallFrame { rel: -12, site: 3 }]
        );
        // Wrong operand shape is malformed.
        assert!(a.lower(CALL_M, &Operand::I32(0)).is_err());
    }

    #[test]
    fn retx_passes_the_exit_index_through() {
        let a = Tm1::new(2);
        for k in [0u8, 1, 5] {
            assert_eq!(
                a.lower(RETX, &Operand::Imm(k)).unwrap(),
                vec![MicroOp::RetX { k }]
            );
        }
        // Wrong operand shape is malformed.
        assert!(a.lower(RETX, &Operand::None).is_err());
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
        // 0x10 is an unused gap below the defined range — no lowering.
        assert_eq!(
            a.lower(0x10, &Operand::None),
            Err(Trap::InvalidOpcode {
                opcode: 0x10,
                at: 0
            })
        );
    }

    #[test]
    fn no_lowering_ever_emits_latch_match() {
        let a = Tm1::new(2);
        for op in ALL_OPCODES {
            let ops = a.lower(op, &valid_operand(op)).unwrap();
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
        // The upper bound is a constructor guard now; lowering itself is
        // width-agnostic, so `rd` is one `ReadAll` regardless.
        let a = Tm1::new(16);
        assert_eq!(a.lower(RD, &Operand::None).unwrap(), vec![MicroOp::ReadAll]);
    }
}
