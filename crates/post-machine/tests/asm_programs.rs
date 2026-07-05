//! PM-1 assembly end-to-end: the spec §6.4 sample, byte-exact, and an
//! assembled program actually running on the Machine.

use mtc_core::formats::object::SymbolDef;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::arch::opcodes::*;
use mtc_post_machine::asm::{assemble, disassemble_object};

/// The spec §6.4 sample, verbatim.
const SPEC_SAMPLE: &str = "\
.func goToEnd                   ; emits ent, defines symbol
L1:     rgt
        jm      L1              ; assembler picks jm.s automatically
        lft
        ret

.func main
        call    goToEnd         ; width decided at link time
        rgt
        wr      1               ; mark
        stp
";

#[test]
fn spec_sample_assembles_byte_exact() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    assert_eq!(obj.arch, mtc_core::formats::ARCH_PM1);
    // goToEnd: ent, rgt, jm.s -3, lft, ret
    assert_eq!(obj.blobs[0], vec![ENT, RGT, JM_S, 0xFD, LFT, RET]);
    // main: ent, call <hole>, rgt, wr 1, stp
    assert_eq!(
        obj.blobs[1],
        vec![ENT, CALL, 0, 0, 0, 0, RGT, WR, 0x81, STP]
    );
    assert_eq!(obj.relocations.len(), 1);
    assert_eq!(obj.relocations[0].blob, 1);
    assert_eq!(obj.relocations[0].offset, 2);
    let sym = &obj.symbols[obj.relocations[0].symbol as usize];
    assert_eq!(sym.name, "goToEnd");
    assert!(matches!(sym.def, SymbolDef::Defined { blob: 0 }));
}

#[test]
fn spec_sample_round_trips_through_disassembly() {
    let obj1 = assemble(SPEC_SAMPLE, false).unwrap();
    let text = disassemble_object(&obj1);
    let obj2 = assemble(&text, false).unwrap();
    assert_eq!(obj1, obj2);
}

#[test]
fn assembled_function_runs_on_the_machine() {
    // Self-contained (no calls): goToEnd's body as main.
    let src = "\
.func main
L:      rgt
        jm      L
        stp
";
    let obj = assemble(src, false).unwrap();
    assert!(obj.relocations.is_empty());
    // A single self-contained blob IS runnable code: entry 0 is its ent.
    let arch = Pm1;
    let machine = Machine::with_arch(&arch, obj.blobs[0].clone(), 0).unwrap();
    let mut tape = InfiniteTape::from_cells([true, true, true], 0, 0);
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Stopped);
    assert_eq!(tape.head(), 3); // stopped on the first blank — assembled, not hand-built
}

#[test]
fn forced_short_and_explicit_far_forms() {
    // jm.s forced short — fits, so identical bytes to relaxed jm.
    let short = assemble(".func f\nL:      rgt\n        jm.s    L\n", false).unwrap();
    let relaxed = assemble(".func f\nL:      rgt\n        jm      L\n", false).unwrap();
    assert_eq!(short.blobs, relaxed.blobs);
}

#[test]
fn errors_carry_lines() {
    let e = assemble(".func f\n        wr\n", false).unwrap_err();
    assert_eq!(e.line, 2);
}
