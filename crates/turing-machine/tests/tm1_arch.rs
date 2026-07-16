//! End-to-end: a hand-assembled TM-1 program driven through the mtc-core
//! VM driver over two tape devices and a table ROM. Core's own tests prove
//! the core side is arch-agnostic (its crate-private fake arch); this proves
//! TM-1's lowering composes with the driver over N devices and the shared
//! match/dispatch table engine.

use mtc_core::vm::{
    Core, InfiniteTape, Operand, Outcome, ReturnStack, RunLimits, TactProfile, Tape,
    encode_operand, run,
};
use mtc_turing_machine::arch::{Tm1, opcodes};

#[test]
fn two_tape_read_match_dispatch_move_write_stop() {
    // Program (2 tapes):
    //   rd            ; latch both heads → TR[0], TR[1]
    //   mtc @0        ; walk match table → MR
    //   djmp @5       ; dispatch by MR to the target below
    // target:
    //   mov [2, 2]    ; step both heads right onto a blank cell
    //   wr  [1, 1]    ; write symbol 1 at both new heads
    //   stp
    let mut code = Vec::new();
    code.push(opcodes::RD);
    code.push(opcodes::MTC);
    code.extend(encode_operand(&Operand::Table(0)).unwrap());
    code.push(opcodes::DJMP);
    code.extend(encode_operand(&Operand::Table(5)).unwrap());
    let target = code.len() as u32; // dispatch lands here
    code.push(opcodes::MOV);
    code.extend(encode_operand(&Operand::Symbols(vec![2, 2])).unwrap());
    code.push(opcodes::WR);
    code.extend(encode_operand(&Operand::Symbols(vec![1, 1])).unwrap());
    code.push(opcodes::STP);

    // Table ROM: a match table at offset 0 (width 2, one row [1, 1]) then a
    // dispatch table at offset 5 (MR = 1 → the target address).
    let mut tables = vec![2u8, 1, 0, 1, 1]; // width=2, count=1, row [1,1]
    tables.extend([1u8, 0]); // dispatch: one entry
    tables.extend(target.to_le_bytes());

    let arch = Tm1::new(2);
    let mut core = Core::new(&arch, 0);
    let mut stack = ReturnStack::new(4);
    let mut tape0 = InfiniteTape::new();
    let mut tape1 = InfiniteTape::new();
    tape0.write(1).unwrap(); // both heads read 1 → the match row fires
    tape1.write(1).unwrap();
    let mut devices: Vec<&mut dyn Tape> = vec![&mut tape0, &mut tape1];
    let result = run(
        &mut core,
        &code,
        &mut stack,
        &mut devices,
        &tables,
        TactProfile::ELECTRONIC,
        RunLimits::default(),
    );
    drop(devices); // release the mutable borrows before inspecting the tapes

    assert_eq!(result.outcome, Outcome::Stopped);
    // rd → mtc → djmp all fired, then mov stepped right and wr marked the
    // new cell: each tape now carries marks at {0, 1} with the head at 1.
    for tape in [&tape0, &tape1] {
        assert_eq!(tape.marked_cells(), vec![0, 1]);
        assert_eq!(tape.head(), 1);
    }
}
