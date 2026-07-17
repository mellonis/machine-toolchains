//! decode(encode(x)) == x, where decode is the real sans-I/O core fetch.

use mtc_core::vm::{BusRequest, BusResponse, Core, CoreEvent, Operand, encode_operand};
use proptest::prelude::*;

// TestArch is crate-private; use a minimal local arch mirroring the operand
// kinds (the codec property only needs operand_kind + lower to accept).
struct CodecArch;
impl mtc_core::vm::Arch for CodecArch {
    fn arch_id(&self) -> u8 {
        0x7E
    }
    fn operand_kind(&self, opcode: u8) -> Option<mtc_core::vm::OperandKind> {
        match opcode {
            0x01 => Some(mtc_core::vm::OperandKind::RelI8),
            0x02 => Some(mtc_core::vm::OperandKind::RelI32),
            0x03 => Some(mtc_core::vm::OperandKind::SymbolVec),
            0x04 => Some(mtc_core::vm::OperandKind::TableRef),
            0x05 => Some(mtc_core::vm::OperandKind::MoveVec),
            0x06 => Some(mtc_core::vm::OperandKind::Imm8),
            0x07 => Some(mtc_core::vm::OperandKind::FramedCall),
            0x08 => Some(mtc_core::vm::OperandKind::WriteMoveVec),
            _ => None,
        }
    }
    fn lower(
        &self,
        opcode: u8,
        operand: &Operand,
    ) -> Result<Vec<mtc_core::vm::MicroOp>, mtc_core::vm::Trap> {
        // The decoded operand is verified here, inside the live core's
        // lower() call — see EXPECTED below.
        EXPECTED.with(|e| {
            let expected = e.borrow();
            assert_eq!(
                (opcode, operand),
                (expected.0, &expected.1),
                "core decoded a different operand than was encoded"
            );
        });
        Ok(vec![mtc_core::vm::MicroOp::Stop])
    }
    fn is_entry_marker(&self, _byte: u8) -> bool {
        false
    }
}

thread_local! {
    static EXPECTED: std::cell::RefCell<(u8, Operand)> =
        const { std::cell::RefCell::new((0, Operand::None)) };
}

fn round_trip(opcode: u8, operand: Operand) {
    let mut code = vec![opcode];
    code.extend(encode_operand(&operand).unwrap());
    EXPECTED.with(|e| *e.borrow_mut() = (opcode, operand));
    let arch = CodecArch;
    let mut core = Core::new(&arch, 0);
    let mut ev = core.start();
    loop {
        match ev {
            CoreEvent::Request(BusRequest::CodeRead { addr }) => {
                let resp = match code.get(addr as usize) {
                    Some(&b) => BusResponse::Byte(b),
                    None => BusResponse::OutOfCode,
                };
                ev = core.resume(resp);
            }
            CoreEvent::Stopped => return, // lower's assert_eq already ran
            other => panic!("unexpected event {other:?}"),
        }
    }
}

proptest! {
    #[test]
    fn rel_i8_round_trips(v in any::<i8>()) {
        round_trip(0x01, Operand::I8(v));
    }

    #[test]
    fn rel_i32_round_trips(v in any::<i32>()) {
        round_trip(0x02, Operand::I32(v));
    }

    #[test]
    fn symbol_vec_round_trips(v in proptest::collection::vec(0u32..0x80, 1..8)) {
        round_trip(0x03, Operand::Symbols(v));
    }

    #[test]
    fn table_ref_round_trips(v in any::<u32>()) {
        // Unsigned absolute: the full u32 range must survive, including
        // values whose top bit would flip an i32 negative.
        round_trip(0x04, Operand::Table(v));
    }

    #[test]
    fn move_vec_round_trips(v in proptest::collection::vec(0u32..3, 1..=16)) {
        // MoveVec shares SymbolVec's wire form and decoded shape
        // (`Operand::Symbols`); the move payloads 0/1/2 stay within the
        // 7-bit element budget by construction.
        round_trip(0x05, Operand::Symbols(v));
    }

    #[test]
    fn imm8_round_trips(v in any::<u8>()) {
        // A plain immediate is one raw byte; every 0..=255 survives.
        round_trip(0x06, Operand::Imm(v));
    }

    #[test]
    fn framed_call_round_trips(rel in any::<i32>(), table in any::<u32>()) {
        // 8 bytes: displacement i32 LE then table offset u32 LE. The full
        // signed × unsigned ranges must survive, including a displacement
        // whose top bit is set and a table offset above i32::MAX.
        round_trip(0x07, Operand::FramedCall { rel, table });
    }

    #[test]
    fn write_move_vec_round_trips(
        writes in proptest::collection::vec(0u32..=0x7F, 1..=16),
        moves in proptest::collection::vec(0u32..=2, 1..=16),
    ) {
        // Two self-delimiting groups back to back: the write payloads
        // (0x7F = keep is a value, not a terminator) then the move codes
        // (0/1/2). Both widths 1..=16, independent of each other; the core
        // splits at the first terminator, so the two groups survive intact.
        round_trip(0x08, Operand::WriteMove { writes, moves });
    }
}

/// Feeds `code` (an opcode + fewer operand bytes than the kind needs) to
/// a live core; the bus answers `OutOfCode` past the end. A truncated
/// operand must TRAP (`CodeOutOfBounds`), never panic and never Stop.
fn expect_trap_on_truncated(code: &[u8]) {
    // A permissive arch: lower is never reached (the fetch traps first).
    struct TrapArch;
    impl mtc_core::vm::Arch for TrapArch {
        fn arch_id(&self) -> u8 {
            0x7E
        }
        fn operand_kind(&self, opcode: u8) -> Option<mtc_core::vm::OperandKind> {
            match opcode {
                0x06 => Some(mtc_core::vm::OperandKind::Imm8),
                0x07 => Some(mtc_core::vm::OperandKind::FramedCall),
                0x08 => Some(mtc_core::vm::OperandKind::WriteMoveVec),
                _ => None,
            }
        }
        fn lower(
            &self,
            _opcode: u8,
            _operand: &Operand,
        ) -> Result<Vec<mtc_core::vm::MicroOp>, mtc_core::vm::Trap> {
            Ok(vec![mtc_core::vm::MicroOp::Stop])
        }
        fn is_entry_marker(&self, _byte: u8) -> bool {
            false
        }
    }
    let arch = TrapArch;
    let mut core = Core::new(&arch, 0);
    let mut ev = core.start();
    loop {
        match ev {
            CoreEvent::Request(BusRequest::CodeRead { addr }) => {
                let resp = match code.get(addr as usize) {
                    Some(&b) => BusResponse::Byte(b),
                    None => BusResponse::OutOfCode,
                };
                ev = core.resume(resp);
            }
            CoreEvent::Trapped(_) => return, // the expected outcome
            other => panic!("expected a trap on truncated operand, got {other:?}"),
        }
    }
}

#[test]
fn truncated_framed_call_traps_not_panics() {
    // opcode + only 3 of the 8 operand bytes, then the code runs out.
    expect_trap_on_truncated(&[0x07, 1, 2, 3]);
    // The empty-operand extreme: opcode alone.
    expect_trap_on_truncated(&[0x07]);
}

#[test]
fn truncated_imm8_traps_not_panics() {
    // opcode with no immediate byte following.
    expect_trap_on_truncated(&[0x06]);
}

#[test]
fn empty_vectors_do_not_encode() {
    // Both vector kinds share `Operand::Symbols`, whose encoding is
    // self-delimiting via the high bit on the LAST element — an empty
    // vector has no last element to carry it, so encoding refuses.
    assert!(encode_operand(&Operand::Symbols(vec![])).is_err());
}

#[test]
fn truncated_write_move_vec_traps_not_panics() {
    // Opcode alone: no write group at all.
    expect_trap_on_truncated(&[0x08]);
    // Mid-write-group: two write bytes with no high-bit terminator, then
    // the code runs out.
    expect_trap_on_truncated(&[0x08, 0x01, 0x02]);
    // Mid-move-group: the write group terminates (0x81) but the move group
    // starts (0x00) and never terminates.
    expect_trap_on_truncated(&[0x08, 0x81, 0x00]);
}

#[test]
fn write_move_empty_groups_do_not_encode() {
    // Each group is self-delimiting via the high bit on its LAST element,
    // so an empty group has no element to carry the terminator — encoding
    // refuses either empty group.
    assert!(
        encode_operand(&Operand::WriteMove {
            writes: vec![],
            moves: vec![1]
        })
        .is_err()
    );
    assert!(
        encode_operand(&Operand::WriteMove {
            writes: vec![1],
            moves: vec![]
        })
        .is_err()
    );
}
