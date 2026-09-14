//! Open bindings (`{…, *}`): every unlisted caller symbol reads as the
//! OPAQUE index — the callee's cardinality — instead of becoming a hole
//! (docs/formats.md (bound calls)). Proven on a neutral fake dialect.

use mtc_core::asm::{ArchSyntax, AsmCaps, Flow, RelaxPair, SyntaxEntry, assemble};
use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::{CallMech, LinkError, LinkOptions, link};
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
                opcode: 0x11,
                mnemonic: "tmatch",
                operand: OperandKind::TableRef,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x12,
                mnemonic: "tdispatch",
                operand: OperandKind::TableRef,
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

/// A 5-symbol caller bound-calling a 3-symbol callee that READS its tape:
/// `rd`, a match table naming every callee symbol plus a `*` catch-all,
/// and a dispatch on the result — the probe's shape, and the smallest one
/// that can tell an open binding from a closed one. A callee body with no
/// match table stamps byte-identically whatever the composite is (trap
/// rows come only from a table rewrite), so a bodiless fixture could not
/// discriminate under mono or hybrid.
///
/// `main_param` and `sub_param` are the two `.param` blocks — the object's
/// interface section is all-or-none, so both come or go together — and
/// `binding` the call's operand text.
fn program(main_param: &str, sub_param: &str, binding: &str) -> String {
    format!(
        "\
.routine main, tapes=1, alpha=(5)
{main_param}.routine sub, tapes=1, alpha=(3)
{sub_param}.section tables
T0:     .row    [0]
        .row    [1]
        .row    [2]
        .row    [*]
D0:     .targets A, B, C, P
.section code
.func main
        call    sub {binding}
        stp
.func sub
        rd
        tmatch  T0
        tdispatch D0
A:      wr      [0]
        ret
B:      wr      [1]
        ret
C:      wr      [2]
        ret
P:      ret
"
    )
}

/// `main`'s five-glyph parameter block.
const MAIN_PARAM: &str = ".param t, ('_', 'a', 'b', 'c', 'd')\n";

/// `sub`'s parameter block with the tape declared `opaque` — the
/// precondition an open binding requires.
const SUB_OPAQUE: &str = ".param n, ('_', 'a', 'b'), writes=('a', 'b'), opaque\n";

/// The same block WITHOUT the `opaque` bit.
const SUB_PLAIN: &str = ".param n, ('_', 'a', 'b'), writes=('a', 'b')\n";

/// `1->1, 2->2` are named; caller symbols 3 and 4 are unlisted, so an
/// open map sends both onto the opaque index 3 (`sub`'s cardinality).
const OPEN_MAP: &str = "[0{1->1, 2->2, *}]";

/// The CLOSED counterpart: the same pairs without `*`, where the unlisted
/// symbols become holes instead.
const CLOSED_MAP: &str = "[0{1->1, 2->2}]";

/// Mutation it catches: keep the closed rule for an open tape and this
/// links to the CLOSED program's bytes — which the first assertion
/// forbids — and, under mono, the stamp synthesizes an unmapped-read
/// trap row for each of the two unlisted symbols, which the second
/// forbids. The two halves fail in different mechanisms, so both are
/// asserted.
#[test]
fn an_open_binding_links_under_every_mechanism_and_differs_from_the_closed_one() {
    let open_src = program(MAIN_PARAM, SUB_OPAQUE, OPEN_MAP);
    let closed_src = program(MAIN_PARAM, SUB_OPAQUE, CLOSED_MAP);
    for mech in MECHS {
        let open = link(&fake_syntax(), &[asm(&open_src)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("the open form must link under {mech}: {e}"));
        let closed = link(&fake_syntax(), &[asm(&closed_src)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("the closed form must link under {mech}: {e}"));
        assert_ne!(
            open.executable.to_bytes(),
            closed.executable.to_bytes(),
            "an open map must not link like a closed one under {mech}"
        );
        assert_eq!(
            open.report.synthesized_trap_rows, 0,
            "an opaque symbol is an IMAGE, not a hole, so it owes no trap row \
             under {mech}: {:?}",
            open.report
        );
    }
    // And the closed form really does hole — otherwise the contrast above
    // would hold for a reason unrelated to the open rule.
    let closed = link(
        &fake_syntax(),
        &[asm(&closed_src)],
        &[],
        opts(CallMech::Mono),
    )
    .expect("the closed form links under mono");
    assert!(
        closed.report.synthesized_trap_rows > 0,
        "the closed counterpart must hole: {:?}",
        closed.report
    );
}

/// Mutation it catches: delete `check_opaque` and a routine that
/// discriminates every glyph accepts opaque input it can only misread.
#[test]
fn an_open_binding_into_a_non_opaque_tape_is_refused() {
    let src = program(MAIN_PARAM, SUB_PLAIN, OPEN_MAP);
    for mech in MECHS {
        let err = link(&fake_syntax(), &[asm(&src)], &[], opts(mech))
            .expect_err("a non-opaque tape must refuse an open binding");
        let LinkError::OpenBindingUnsupported {
            callee,
            tape,
            param,
        } = &err
        else {
            panic!("expected OpenBindingUnsupported under {mech}, got {err:?}");
        };
        assert_eq!(
            (callee.as_str(), *tape, param.as_deref()),
            ("sub", 0, Some("n"))
        );
    }
}

/// Mutation it catches: default `opaque` to `true` when there is no
/// interface and an interfaceless callee silently accepts one.
#[test]
fn an_open_binding_into_an_interfaceless_callee_is_refused() {
    let src = program("", "", OPEN_MAP);
    let err = link(&fake_syntax(), &[asm(&src)], &[], opts(CallMech::Frames))
        .expect_err("an interfaceless callee must refuse an open binding");
    assert!(
        matches!(&err, LinkError::OpenBindingUnsupported { param: None, .. }),
        "{err:?}"
    );
}

/// The descriptor the frames path emits must carry the opaque index, not
/// the hole sentinel. Mutation it catches: leave `dense_map`'s guard at
/// `< codomain_card` and the two opaque symbols come back `0xFFFF`.
#[test]
fn the_frames_descriptor_carries_the_opaque_index_not_a_hole() {
    let src = program(MAIN_PARAM, SUB_OPAQUE, OPEN_MAP);
    let out = link(&fake_syntax(), &[asm(&src)], &[], opts(CallMech::Frames)).expect("links");
    let bytes = out.executable.to_bytes();
    // The dense rmap for a 5-symbol physical band reads
    // [0, 1, 2, 3, 3] — blank pinned, two named, two opaque. Search the
    // image for that little-endian u16 run; `0xFFFF` anywhere in it is
    // the regression.
    let want: Vec<u8> = [0u16, 1, 2, 3, 3]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert!(
        bytes.windows(want.len()).any(|w| w == want),
        "the opaque rmap run is not in the image"
    );
}
