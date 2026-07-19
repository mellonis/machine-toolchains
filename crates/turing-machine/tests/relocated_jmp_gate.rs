//! The tail-call gate: a RELOCATED `jmp @target` — an external/cross-function
//! jump — assembles, links (the relocation resolves), and transfers control at
//! run time. This is the surface `tail_call` lowers onto (`CallThen{then:
//! Return}` → `jmp @<callee>`), so the pass may only exist if this holds.
//!
//! The mechanism is arch-agnostic (the core assembler emits a hole + external
//! relocation for a `Flow::Jump` operand written `@name`, and the composition
//! engine's site scan treats a relocated jump as a plain-call closure edge so a
//! callee reachable only through it is still linked). This probe pins it for the
//! TM-1 dialect end-to-end, across every call mechanism the compiler's objects
//! link under.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{assemble, link};

/// `main` writes symbol 1 at position 0 and steps right, then TAIL-JUMPS to
/// `other` (a relocated `jmp @other`); `other` writes symbol 2 at position 1
/// and stops. Both writes surviving proves the transfer resolved and ran —
/// `other` is reachable ONLY through the relocated jump.
const PROGRAM: &str = "\
.routine main, tapes=1, alpha=(3)
.routine other, tapes=1, alpha=(3)
.section code
.func main
        wrmv [1], [>]
        jmp @other
.func other
        wr [2]
        stp
";

fn build(mech: CallMech) -> Executable {
    let obj = assemble(PROGRAM, false).expect("the relocated jmp assembles");
    link(
        &[obj],
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .expect("the relocation resolves at link")
    .executable
}

fn run(exe: &Executable) -> (Outcome, TapeSnapshot) {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = WideTape::new(3);
    let result = {
        let mut devices: Vec<&mut dyn Tape> = vec![&mut tape as &mut dyn Tape];
        machine
            .run_tapes(
                &mut devices,
                RunOptions {
                    limits: RunLimits {
                        max_steps: Some(10_000),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .expect("run set-up ok")
    };
    (result.outcome, tape.to_snapshot())
}

fn cell_at(snap: &TapeSnapshot, pos: i64) -> u8 {
    let idx = pos - snap.origin;
    if idx < 0 || idx as usize >= snap.cells.len() {
        0
    } else {
        snap.cells[idx as usize]
    }
}

#[test]
fn relocated_jmp_transfers_control_across_functions_under_every_mechanism() {
    for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
        let exe = build(mech);
        let (outcome, snap) = run(&exe);
        assert_eq!(outcome, Outcome::Stopped, "{mech}: the tail jump ran to stp");
        // main wrote 1 at 0 and stepped right; other wrote 2 at 1. Both present
        // ⇒ the relocated jump resolved and transferred into `other`.
        assert_eq!(cell_at(&snap, 0), 1, "{mech}: main's write survived");
        assert_eq!(cell_at(&snap, 1), 2, "{mech}: control reached `other`");
        assert_eq!(snap.head, 1, "{mech}: head where `other` left it");
    }
}
