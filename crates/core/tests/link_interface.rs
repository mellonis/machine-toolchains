//! The link stage refuses the SYMBOLIC binding forms it does not resolve
//! (docs/formats.md (bound calls)). The object format and the assembler
//! carry a named entry, a glyph-labelled destination, an open map and an
//! exit vector; nothing below the assembler reads them, so a link would
//! silently produce a wrong image. Every refusal is checked under all
//! three call mechanisms, because the guard sits ahead of the point where
//! they diverge.
//!
//! Each fixture is otherwise well-formed: drop the symbolic field and
//! what is left is the legal numeric binding `[1, 0]`, which the control
//! test links. That is what makes these tests discriminate — neutralize
//! the guard and they link instead of failing for some other reason.
//!
//! Everything runs on a neutral fake dialect (per-file-helper convention),
//! so core stays provably arch-agnostic.

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

/// A two-function program whose `main` bound-calls `sub` once, with
/// `binding` as the call's operand text. The two tapes have equal
/// cardinalities, so a swap `[1, 0]` is a legal non-identity binding: it
/// does not collapse to a plain call, and it needs no hole.
fn program(binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}

/// The refusal every symbolic form takes, under every mechanism: a
/// `BadBinding` naming the callee and the form that fired.
#[track_caller]
fn refused_under_every_mechanism(binding: &str, form: &str) {
    let src = program(binding);
    for mech in MECHS {
        let err = link(&fake_syntax(), &[asm(&src)], &[], opts(mech))
            .expect_err("the symbolic form must be refused");
        let LinkError::BadBinding { callee, message } = &err else {
            panic!("expected a BadBinding under {mech}, got {err:?}");
        };
        assert_eq!(callee, "sub", "under {mech}");
        assert!(
            message.contains(form) && message.contains("does not resolve yet"),
            "under {mech}: {message}"
        );
    }
}

/// `num: 1` binds a callee PARAMETER. Nothing below the assembler reads
/// `param`, so a link would take the entry positionally instead — a
/// silently different image whenever the two disagree.
#[test]
fn a_named_entry_is_refused_under_every_mechanism() {
    refused_under_every_mechanism("[num: 1, ctl: 0]", "a named entry");
}

/// `3=>'0'` names the callee symbol by GLYPH. A labelled pair is written
/// with `dst: 0` — the field the wire gives to the label — so linking one
/// unresolved would map onto symbol 0 whatever the glyph meant.
#[test]
fn a_glyph_labelled_destination_is_refused_under_every_mechanism() {
    refused_under_every_mechanism("[1{3=>'0'}, 0]", "a glyph-labelled destination");
}

/// `{*}` says the listed pairs are not the whole map. Ignoring the flag
/// links it as a closed map — the one reading that is certainly wrong.
#[test]
fn an_open_map_is_refused_under_every_mechanism() {
    refused_under_every_mechanism("[1{*}, 0]", "an open map");
}

/// `exits=(L)` names where the callee's exits land. The linker wires no
/// exits, so an unresolved vector would simply vanish from the image.
#[test]
fn an_exit_vector_is_refused_under_every_mechanism() {
    refused_under_every_mechanism("[1, 0] exits=(L)", "an exit vector");
}

/// The control, and the boundary of the refusal: a written-EMPTY map is
/// a deliberate identity, not a symbolic form — it needs nothing
/// resolved. It links, and it links to the very same image the
/// brace-less spelling does, under every mechanism.
#[test]
fn a_written_empty_map_still_links_exactly_like_the_bare_form() {
    let written = program("[1{}, 0]");
    let bare = program("[1, 0]");
    for mech in MECHS {
        let a = link(&fake_syntax(), &[asm(&written)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("`1{{}}` must link under {mech}: {e}"));
        let b = link(&fake_syntax(), &[asm(&bare)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("`1` must link under {mech}: {e}"));
        assert_eq!(
            a.executable.to_bytes(),
            b.executable.to_bytes(),
            "a written-empty map is index identity under {mech}"
        );
    }
}

/// The refusal is reachability-gated like every other link error: an
/// unreachable function may carry anything, symbolic bindings included
/// (docs/core.md (linking)). The two programs below differ ONLY in which
/// function holds the symbolic call — `ghost`, which the BFS from `main`
/// never reaches, or `main` itself — so the contrast pins the gating and
/// nothing else. Both carry a REACHED bound call (`main`'s numeric one),
/// so the guard's loop really walks a binding in each.
#[test]
fn the_refusal_is_gated_on_reachability() {
    let program = |unreached_body: &str, main_call: &str| {
        format!(
            "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.routine ghost, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub {main_call}
M:      stp
.func sub
        ret
.func ghost
        call    sub {unreached_body}
G:      ret
"
        )
    };
    const SYMBOLIC: &str = "[num: 1{3=>'0',*}, ctl: 0] exits=(G)";
    let unreached = program(SYMBOLIC, "[1, 0]");
    let reached = program("[1, 0]", &SYMBOLIC.replace("(G)", "(M)"));
    for mech in MECHS {
        let out = link(&fake_syntax(), &[asm(&unreached)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("an unreached binding must not refuse under {mech}: {e}"));
        assert!(
            out.report.dropped.contains(&"ghost".to_string()),
            "`ghost` must be the unreached one under {mech}: {:?}",
            out.report.dropped
        );
        // The very same call, moved into the reached `main`, is refused.
        let err = link(&fake_syntax(), &[asm(&reached)], &[], opts(mech))
            .expect_err("the same binding in a reached function must be refused");
        assert!(
            matches!(err, LinkError::BadBinding { .. }),
            "under {mech}: {err:?}"
        );
    }
}
