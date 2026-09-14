//! Every way a symbolic binding can fail to resolve against the callee's
//! interface (docs/core.md (symbolic resolution)), on a neutral fake
//! dialect.

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

/// A caller/callee pair whose callee declares two parameters `p`, `q`
/// over a 4-symbol alphabet each. `p` and `q` deliberately spell `'1'`
/// at DIFFERENT indices (2 and 1) — resolving a label against the wrong
/// tape's glyph list is a discriminating failure only when the two
/// lists disagree on where the glyph sits.
fn program(binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4)
.param p, ('_', '0', '1', '2')
.param q, ('_', '1', '0', '2')
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}

/// The same pair with NO `.param` lines on the callee: it describes no
/// interface, so nothing symbolic can reach it.
fn interfaceless(binding: &str) -> String {
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

#[track_caller]
fn refused(src: &str, needle: &str) {
    let err = link(&fake_syntax(), &[asm(src)], &[], opts(CallMech::Frames))
        .expect_err("the binding must be refused");
    let LinkError::BadBinding { callee, message } = &err else {
        panic!("expected a BadBinding, got {err:?}");
    };
    assert_eq!(callee, "sub");
    assert!(message.contains(needle), "{message}");
}

/// Mutation it catches: drop the `position` lookup's `None` arm and an
/// unknown name silently binds nothing.
#[test]
fn an_unknown_parameter_is_refused() {
    refused(
        &program("[p: 1, zz: 0]"),
        "parameter `zz`, which `sub` does not declare",
    );
}

/// Mutation it catches: drop the `slots[k].is_some()` guard and the
/// second `p` silently overwrites the first, leaving `q` unbound.
#[test]
fn a_parameter_bound_twice_is_refused() {
    refused(&program("[p: 1, p: 0]"), "names parameter `p` twice");
}

/// A one-entry named list against a two-parameter callee. Mutation it
/// catches: let `reorder_named` fill a missing slot with a default and
/// the call binds tape 1 to caller tape 0 by accident.
#[test]
fn a_missing_parameter_is_refused() {
    refused(&program("[p: 1]"), "does not bind parameter `q`");
}

/// Mutation it catches: make `require_interface` fall back to positional
/// resolution and a symbolic call into an interfaceless callee links
/// silently — the exact hazard the refusal existed for.
#[test]
fn a_named_entry_into_an_interfaceless_callee_is_refused() {
    refused(
        &interfaceless("[p: 1, q: 0]"),
        "describes no interface; only a transparent call can reach it",
    );
}

/// Mutation it catches: delete the mixed-form guard and the fully-named
/// path runs over a partly-positional binding — `reorder_named` reads
/// every entry's `param` unconditionally and panics
/// (`expect("checked fully named")`) on the first positional one instead
/// of the intended refusal.
#[test]
fn a_mixed_named_and_positional_list_is_refused() {
    // The assembler rejects a mixed list at parse time, so this fixture
    // is built by hand rather than assembled: it is the hand-crafted
    // object a third-party producer could emit.
    use mtc_core::formats::object::TapeBinding;
    let mut obj = asm(&program("[1, 0]"));
    obj.bound_calls[0].binding[0] = TapeBinding {
        param: Some("p".to_string()),
        ..obj.bound_calls[0].binding[0].clone()
    };
    let err = link(&fake_syntax(), &[obj], &[], opts(CallMech::Frames))
        .expect_err("a mixed list must be refused");
    assert!(
        matches!(&err, LinkError::BadBinding { message, .. }
            if message.contains("mixes named and positional entries")),
        "{err:?}"
    );
}

/// Mutation it catches: make the unknown-glyph arm fall back to `dst: 0`
/// and a typo'd glyph silently binds blank.
#[test]
fn an_unknown_glyph_is_refused() {
    refused(
        &program("[1{3=>'9'}, 0]"),
        "names glyph `9`, which is not in `sub`'s alphabet for parameter `p`",
    );
}

/// Mutation it catches: make `require_interface` optional for labels and
/// an interfaceless callee links with every labelled pair reading 0.
#[test]
fn a_glyph_label_into_an_interfaceless_callee_is_refused() {
    refused(
        &interfaceless("[1{3=>'1'}, 0]"),
        "describes no interface; only a transparent call can reach it",
    );
}

/// Names and labels in ONE binding: the reorder must run first, or the
/// label is looked up in the wrong tape's glyph list. Mutation it
/// catches: resolve labels before reordering and `q: 1{3=>'1'}` resolves
/// `'1'` against parameter `p`'s glyphs instead of `q`'s.
#[test]
fn a_named_entry_carrying_a_glyph_label_resolves_in_callee_tape_order() {
    let mixed = program("[q: 1{3=>'1'}, p: 0]");
    let indexed = program("[0, 1{3=>1}]");
    for mech in MECHS {
        let a = link(&fake_syntax(), &[asm(&mixed)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        let b = link(&fake_syntax(), &[asm(&indexed)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        assert_eq!(
            a.executable.to_bytes(),
            b.executable.to_bytes(),
            "under {mech}"
        );
    }
}
