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
}

#[test]
fn empty_vectors_do_not_encode() {
    // Both vector kinds share `Operand::Symbols`, whose encoding is
    // self-delimiting via the high bit on the LAST element — an empty
    // vector has no last element to carry it, so encoding refuses.
    assert!(encode_operand(&Operand::Symbols(vec![])).is_err());
}
