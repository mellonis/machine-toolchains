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

/// A caller bound-calling a 3-symbol callee that READS its tape:
/// `rd`, a match table naming every callee symbol plus a `*` catch-all,
/// and a dispatch on the result — the probe's shape, and the smallest one
/// that can tell an open binding from a closed one. A callee body with no
/// match table stamps byte-identically whatever the composite is (trap
/// rows come only from a table rewrite), so a bodiless fixture could not
/// discriminate under mono or hybrid.
///
/// `main_card` is the caller band's cardinality (5 for the unequal
/// fixtures, 3 for the equal-cardinality write-half pin); `main_param` and
/// `sub_param` are the two `.param` blocks — the object's interface section
/// is all-or-none, so both come or go together — and `binding` the call's
/// operand text.
fn program(main_card: u32, main_param: &str, sub_param: &str, binding: &str) -> String {
    format!(
        "\
.routine main, tapes=1, alpha=({main_card})
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

/// `main`'s parameter block at the EQUAL cardinality — three glyphs, the
/// same width as `sub` — for the write-half pin below.
const MAIN_PARAM_EQ: &str = ".param t, ('_', 'a', 'b')\n";

/// Equal cardinalities with one symbol deliberately left UNNAMED: `1->1`
/// is the only pair, so the open rule sends caller symbol 2 onto the
/// opaque index 3 on the read side and holes callee symbol 2 on the write
/// side. Listing every symbol would hole nothing and pin nothing.
const OPEN_MAP_EQ: &str = "[0{1->1, *}]";

/// Mutation it catches: keep the closed rule for an open tape and this
/// links to the CLOSED program's bytes — which the first assertion
/// forbids — and, under mono, the stamp synthesizes an unmapped-read
/// trap row for each of the two unlisted symbols, which the second
/// forbids. The two halves fail in different mechanisms, so both are
/// asserted.
#[test]
fn an_open_binding_links_under_every_mechanism_and_differs_from_the_closed_one() {
    let open_src = program(5, MAIN_PARAM, SUB_OPAQUE, OPEN_MAP);
    let closed_src = program(5, MAIN_PARAM, SUB_OPAQUE, CLOSED_MAP);
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
    let src = program(5, MAIN_PARAM, SUB_PLAIN, OPEN_MAP);
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
    let src = program(5, "", "", OPEN_MAP);
    let err = link(&fake_syntax(), &[asm(&src)], &[], opts(CallMech::Frames))
        .expect_err("an interfaceless callee must refuse an open binding");
    assert!(
        matches!(&err, LinkError::OpenBindingUnsupported { param: None, .. }),
        "{err:?}"
    );
}

/// The descriptor the frames path emits must carry the opaque index, not
/// the hole sentinel — on the READ map, which is the only direction the
/// allowance is passed to. Mutation it catches: pass `allow_opaque: false`
/// at `materialize`'s rmap call (or leave `dense_map`'s bound at
/// `< codomain_card` outright) and the two opaque symbols come back
/// `0xFFFF`.
#[test]
fn the_frames_descriptor_carries_the_opaque_index_not_a_hole() {
    let src = program(5, MAIN_PARAM, SUB_OPAQUE, OPEN_MAP);
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

/// The open rule's WRITE half stays closed even where the two alphabets
/// are the SAME size — the one place it changes behaviour on equal
/// cardinalities, since the closed path only closes on unequal ones. `sub`
/// writes symbol 2 at `C:`, and `[0{1->1, *}]` names only symbol 1, so the
/// write map holes 2 and the stamped write becomes the dialect's `trap #1`
/// (unmapped write) instead of writing through.
///
/// Mutation it catches: drop `close_unlisted(&mut wmap, card)` from the
/// open branch and the write map identity-completes instead — `wr [2]`
/// survives the stamp as `07 82` and no `trap #1` is emitted.
#[test]
fn the_open_write_half_closes_on_equal_cardinalities() {
    let src = program(3, MAIN_PARAM_EQ, SUB_OPAQUE, OPEN_MAP_EQ);
    let out = link(&fake_syntax(), &[asm(&src)], &[], opts(CallMech::Mono))
        .expect("an open binding on equal cardinalities links under mono");
    let code = &out.executable.code;
    // `trap` is opcode 0x18 with an Imm8; kind 1 is the unmapped write.
    assert!(
        code.windows(2).any(|w| w == [0x18, 0x01]),
        "the unmapped write must lower to `trap #1`: {code:?}"
    );
    // `wr [2]` is opcode 0x07 plus a one-element symbol vector whose only
    // payload carries the terminator bit: 0x80 | 2.
    assert!(
        !code.windows(2).any(|w| w == [0x07, 0x82]),
        "no `wr [2]` may survive the stamp: {code:?}"
    );
    // Non-vacuity: the MAPPED write does survive, so the two assertions
    // above are reading a real stamped body and not an empty one.
    assert!(
        code.windows(2).any(|w| w == [0x07, 0x81]),
        "`wr [1]` is mapped and must survive: {code:?}"
    );
}

/// Hybrid's classifier promises to leave anything holey or one-way on the
/// frames path. An open binding is both at once — its unlisted symbols
/// read onto ONE opaque index (not injective) and write back through
/// nothing (not total) — so `is_bijection` must reject it even where the
/// cardinalities match and no `=>` pair appears.
///
/// Mutation it catches: drop `tb.open` from `is_bijection`'s condition and
/// this equal-size, one-way-free binding classifies as a completed
/// bijection; hybrid mono-stamps it onto the base profile
/// (`instantiations` 1, `composites` 0), where nothing activates the open
/// read map.
#[test]
fn hybrid_keeps_an_open_binding_on_the_frames_path() {
    let src = program(3, MAIN_PARAM_EQ, SUB_OPAQUE, OPEN_MAP_EQ);
    let out = link(&fake_syntax(), &[asm(&src)], &[], opts(CallMech::Hybrid))
        .expect("an open binding links under hybrid");
    assert!(
        out.report.composites >= 1 && out.report.instantiations == 0,
        "an open binding is not a bijection, so hybrid must route it to \
         frames rather than mono-stamp it: {:?}",
        out.report
    );
}

/// Both `OpenBindingUnsupported` Display arms, rendered in full. Mutation
/// it catches: swap the `param`/`tape` arms, or reword either, and the
/// message a user reads stops naming what they wrote.
#[test]
fn the_open_binding_refusal_renders_the_parameter_or_the_tape_number() {
    let named = link(
        &fake_syntax(),
        &[asm(&program(5, MAIN_PARAM, SUB_PLAIN, OPEN_MAP))],
        &[],
        opts(CallMech::Frames),
    )
    .expect_err("a non-opaque tape refuses");
    assert_eq!(
        named.to_string(),
        "an open binding into `sub`'s parameter `n`, which is not declared \
         opaque; every state that reads it must have a `*` row"
    );
    let numbered = link(
        &fake_syntax(),
        &[asm(&program(5, "", "", OPEN_MAP))],
        &[],
        opts(CallMech::Frames),
    )
    .expect_err("an interfaceless callee refuses");
    assert_eq!(
        numbered.to_string(),
        "an open binding into `sub`'s tape 0, which is not declared \
         opaque; every state that reads it must have a `*` row"
    );
}
