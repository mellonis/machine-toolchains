//! What the link stage does with each SYMBOLIC binding form the object
//! format and the assembler carry (docs/formats.md (bound calls)). A
//! named entry and a glyph-labelled destination RESOLVE against the
//! callee's interface, an open map is accepted against the callee's
//! `opaque` bits (what it then means is `link_open.rs`), and an exit
//! vector is ACCEPTED under frames rather than refused (what it then
//! means, and what the other two mechanisms do with it, is
//! `link_exits.rs`). The resolving cases are checked under all three call
//! mechanisms, because the pre-pass sits ahead of the point where they
//! diverge.
//!
//! Each fixture is otherwise well-formed: drop the symbolic field and
//! what is left is the legal numeric binding `[1, 0]`, which the control
//! test links. That is what makes these tests discriminate — a resolution
//! test that links for some unrelated reason would match the numeric
//! form's bytes only by accident.
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

/// A two-function program whose `main` bound-calls `sub` once, with
/// `binding` as the call's operand text. The two tapes have equal
/// cardinalities, so a swap `[1, 0]` is a legal non-identity binding: it
/// does not collapse to a plain call, and it needs no hole. Both
/// routines declare their interface, so a symbolic form has something to
/// resolve against; `sub`'s parameters are `p` and `q`, in that order.
fn program(binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4)
.param p, ('_', '0', '1', '2')
.param q, ('_', '0', '1', '2')
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}

/// `program`, but `sub`'s first parameter is declared `opaque` — the
/// precondition an open binding requires.
fn open_program(binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4)
.param p, ('_', '0', '1', '2'), opaque
.param q, ('_', '0', '1', '2')
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}

/// `program`, but `sub` declares `declared` state parameters — which is
/// what makes an exit vector of that length legal at the call site. The
/// count is a parameter because the exit arity is checked on EVERY site
/// into an exit-bearing callee: an exit-FREE control must therefore call
/// a callee declaring `exits=0`, not the same one.
fn exit_program(declared: u8, binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4), exits={declared}
.param p, ('_', '0', '1', '2')
.param q, ('_', '0', '1', '2')
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}

/// `p: 1, q: 0` binds by callee PARAMETER. The linker looks each name up
/// in the callee's interface, reorders the entries into the callee's own
/// tape order, and hands the engine the very binding the positional
/// spelling would have produced — so the image is byte-identical to the
/// numeric form.
///
/// Mutation it catches: make the resolver take a named entry positionally
/// (ignore `param`) and `[p: 1, q: 0]` links as `[1, 0]` while
/// `[q: 0, p: 1]` links as `[0, 1]` — the second assertion below fails.
#[test]
fn a_named_entry_resolves_to_the_numeric_form_under_every_mechanism() {
    let numeric = program("[1, 0]");
    for named in ["[p: 1, q: 0]", "[q: 0, p: 1]"] {
        let src = program(named);
        for mech in MECHS {
            let a = link(&fake_syntax(), &[asm(&src)], &[], opts(mech))
                .unwrap_or_else(|e| panic!("`{named}` must link under {mech}: {e}"));
            let b = link(&fake_syntax(), &[asm(&numeric)], &[], opts(mech))
                .unwrap_or_else(|e| panic!("`[1, 0]` must link under {mech}: {e}"));
            assert_eq!(
                a.executable.to_bytes(),
                b.executable.to_bytes(),
                "`{named}` must link exactly like `[1, 0]` under {mech}"
            );
        }
    }
}

/// `3=>'1'` names the callee symbol by GLYPH. The linker looks the label
/// up in the callee's declared glyph list for that tape and fills in the
/// index, so the image is byte-identical to the spelling that wrote the
/// index directly. `sub`'s tapes are `('_', '0', '1', '2')`, so `'1'` is
/// index 2 and `'2'` is index 3.
///
/// Mutation it catches: leave `dst_label` unresolved and the pair reads
/// the `dst: 0` a labelled pair is written with — `'1'` would bind blank.
/// The two spellings below then diverge.
#[test]
fn a_glyph_labelled_destination_resolves_under_every_mechanism() {
    let labelled = program("[1{3=>'1'}, 0]");
    let indexed = program("[1{3=>2}, 0]");
    for mech in MECHS {
        let a = link(&fake_syntax(), &[asm(&labelled)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("the labelled form must link under {mech}: {e}"));
        let b = link(&fake_syntax(), &[asm(&indexed)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("the indexed form must link under {mech}: {e}"));
        assert_eq!(
            a.executable.to_bytes(),
            b.executable.to_bytes(),
            "`3=>'1'` must link exactly like `3=>2` under {mech}"
        );
    }
}

/// `{*}` says the listed pairs are not the whole map: every unlisted
/// caller symbol reads as the OPAQUE index — the callee's cardinality,
/// an index no callee row names. `sub`'s tapes are 4 wide, so the opaque
/// index is 4 and the binding is legal only because `sub` declares that
/// tape `opaque`.
///
/// `{*}` no longer refuses. What an open map MEANS is pinned in
/// `link_open.rs`, not here — the open rule bites on this fixture too
/// (its rmap sends 1, 2 and 3 onto the opaque index 4 and its wmap holes
/// all three), but `sub`'s body is `ret` and nothing else: it reads no
/// tape and writes none, so no comparison run against it could observe
/// either half. This test pins only that the link is accepted.
///
/// Mutation it catches: make the resolution pre-pass reject a binding
/// with `open` set (a `BadBinding` on any open entry) and this link
/// fails under every mechanism.
#[test]
fn an_open_map_no_longer_refuses() {
    let src = open_program("[1{*}, 0]");
    for mech in MECHS {
        link(&fake_syntax(), &[asm(&src)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("an open map must link under {mech}: {e}"));
    }
}

/// `exits=(L)` names where the callee's exits land. Under FRAMES it is
/// ACCEPTED rather than refused — the site's descriptor carries the
/// vector — which is the whole of what this file claims about the form.
///
/// The inequality below is a SMOKE CHECK, not an isolation claim: the
/// two programs differ in their `.routine` line as well as in the
/// binding, because the exit arity is checked on every site and an
/// exit-free control must therefore call an `exits=0` callee. What the
/// vector actually becomes — the exact absolute addresses, in order, in
/// the descriptor's own exit field — is pinned in `link_exits.rs`.
#[test]
fn an_exit_vector_links_under_frames_instead_of_refusing() {
    let with_exits = exit_program(1, "[1, 0] exits=(L)");
    let without = exit_program(0, "[1, 0]");
    let a = link(
        &fake_syntax(),
        &[asm(&with_exits)],
        &[],
        opts(CallMech::Frames),
    )
    .expect("an exit-bearing site must link under frames");
    let b = link(
        &fake_syntax(),
        &[asm(&without)],
        &[],
        opts(CallMech::Frames),
    )
    .expect("the exit-free site links");
    assert_ne!(
        a.executable.to_bytes(),
        b.executable.to_bytes(),
        "an exit vector must reach the image"
    );
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

/// Resolution is reachability-gated like every other link error: an
/// unreachable function may carry anything, an unresolvable binding
/// included (docs/core.md (linking)). The two programs below differ ONLY
/// in which function holds the bad call — `ghost`, which the BFS from
/// `main` never reaches, or `main` itself — so the contrast pins the
/// gating and nothing else. Both carry a REACHED bound call (`main`'s
/// numeric one), so the pre-pass really walks a binding in each.
///
/// The bad call names a parameter `sub` does not declare, which is a
/// RESOLUTION error rather than the retired blanket refusal: had the
/// fixture kept a form that merely used to be refused, this test would
/// go green for the wrong reason once that form resolved.
///
/// Mutation it catches: move the pre-pass ahead of reachability (run it
/// over every object rather than over `order`) and the first link fails.
#[test]
fn resolution_is_gated_on_reachability() {
    let program = |unreached_body: &str, main_call: &str| {
        format!(
            "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4)
.param p, ('_', '0', '1', '2')
.param q, ('_', '0', '1', '2')
.routine ghost, tapes=2, alpha=(4, 4)
.param g, ('_', '0', '1', '2')
.param h, ('_', '0', '1', '2')
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
    const BAD: &str = "[nosuch: 1, q: 0]";
    let unreached = program(BAD, "[1, 0]");
    let reached = program("[1, 0]", BAD);
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
            matches!(&err, LinkError::BadBinding { message, .. }
                if message.contains("parameter `nosuch`")),
            "under {mech}: {err:?}"
        );
    }
}
