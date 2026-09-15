//! A caller object and a callee object, linked together: numeric and
//! symbolic bindings, under every mechanism. Every other bound-call test
//! in the suite links ONE object, so the cross-object path has until now
//! been correct only by inspection (docs/core.md (linking)).

use mtc_core::asm::{ArchSyntax, AsmCaps, Flow, RelaxPair, SyntaxEntry, assemble};
use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::{CallMech, LinkOptions, link};
use mtc_core::vm::OperandKind;

const ARCH: u8 = 0x7E;

/// A neutral fake dialect with the interface capability on: nop/stp/ret/
/// ent, a relaxable far/short call pair, the framed call the frames path
/// lowers into, and the read/write/move/trap surface mono stamping
/// projects.
fn fake_syntax() -> ArchSyntax {
    use Flow::{Call, FallThrough as FT, Stop};
    ArchSyntax {
        entries: vec![
            SyntaxEntry {
                opcode: 0x01,
                mnemonic: "nop",
                operand: OperandKind::None,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x02,
                mnemonic: "stp",
                operand: OperandKind::None,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: 0x0B,
                mnemonic: "ret",
                operand: OperandKind::None,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: 0x21,
                mnemonic: "call",
                operand: OperandKind::RelI32,
                flow: Call,
            },
            SyntaxEntry {
                opcode: 0x31,
                mnemonic: "call.s",
                operand: OperandKind::RelI8,
                flow: Call,
            },
            SyntaxEntry {
                opcode: 0x14,
                mnemonic: "fcall",
                operand: OperandKind::FramedCall,
                flow: Call,
            },
            SyntaxEntry {
                opcode: 0x04,
                mnemonic: "rd",
                operand: OperandKind::None,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x07,
                mnemonic: "wr",
                operand: OperandKind::SymbolVec,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x0F,
                mnemonic: "mov",
                operand: OperandKind::MoveVec,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x18,
                mnemonic: "trap",
                operand: OperandKind::Imm8,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x0E,
                mnemonic: "ent",
                operand: OperandKind::None,
                flow: FT,
            },
        ],
        relax_pairs: vec![RelaxPair {
            far: 0x21,
            short: 0x31,
        }],
        entry_opcode: 0x0E,
        break_opcode: None,
        trap_opcode: Some(0x18),
        return_opcode: Some(0x0B),
        caps: AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
            volatile: false,
            interface: true,
        },
    }
}

fn asm(src: &str) -> ObjectFile {
    assemble(&fake_syntax(), ARCH, src, false).expect("assembles")
}

const MECHS: [CallMech; 3] = [CallMech::Mono, CallMech::Frames, CallMech::Hybrid];

fn opts(mech: CallMech) -> LinkOptions {
    LinkOptions {
        call_mech: mech,
        ..Default::default()
    }
}

/// The callee, alone in its own object, declaring its interface.
const CALLEE: &str = "\
.routine mylib::plusOne, tapes=1, alpha=(3)
.param num, ('_', '0', '1'), writes=('0', '1')
.section code
.func mylib::plusOne
        wr      [2]
        ret
";

/// The caller, in its own object, naming the callee by symbol and
/// binding it SYMBOLICALLY.
const CALLER_SYMBOLIC: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', '0', '1')
.section code
.func main
        call    mylib::plusOne [num: 0{3->'0', 4->'1'}]
        stp
";

/// The same caller with the binding written numerically.
const CALLER_NUMERIC: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', '0', '1')
.section code
.func main
        call    mylib::plusOne [0{3->1, 4->2}]
        stp
";

/// The whole point: a symbolic cross-object binding links to the same
/// image the numeric one does, under every mechanism.
///
/// Mutation it catches: resolve against the CALLER's interface instead of
/// the callee's (a one-index slip in `FuncRef::interface`) and `'0'`
/// resolves to 3 rather than 1 — the two images diverge.
#[test]
fn a_symbolic_cross_object_binding_links_like_the_numeric_one() {
    for mech in MECHS {
        let a = link(
            &fake_syntax(),
            &[asm(CALLER_SYMBOLIC), asm(CALLEE)],
            &[],
            opts(mech),
        )
        .unwrap_or_else(|e| panic!("the symbolic form must link under {mech}: {e}"));
        let b = link(
            &fake_syntax(),
            &[asm(CALLER_NUMERIC), asm(CALLEE)],
            &[],
            opts(mech),
        )
        .unwrap_or_else(|e| panic!("the numeric form must link under {mech}: {e}"));
        assert_eq!(
            a.executable.to_bytes(),
            b.executable.to_bytes(),
            "cross-object symbolic != numeric under {mech}"
        );
    }
}

/// The callee as a LIBRARY rather than a user object: the same
/// resolution, through the first-wins library namespace.
///
/// Mutation it catches: restrict the interface lookup to user objects and
/// a library callee resolves nothing.
#[test]
fn a_symbolic_binding_resolves_against_a_library_callee() {
    for mech in MECHS {
        link(
            &fake_syntax(),
            &[asm(CALLER_SYMBOLIC)],
            &[asm(CALLEE)],
            opts(mech),
        )
        .unwrap_or_else(|e| panic!("a library callee must resolve under {mech}: {e}"));
    }
}
