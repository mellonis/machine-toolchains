//! dis → asm byte identity over the interface surface (docs/formats.md
//! (routine interfaces), (bound calls)), on a neutral fake dialect.
//!
//! The dialect is local to this file, per the repo's per-file-helper
//! convention: core carries zero PM-1/TM-1 knowledge, so the interface
//! surface is proven on a made-up architecture (id `0x7F`) whose caps
//! turn `interface` on.

use mtc_core::asm::{
    ArchSyntax, AsmCaps, AsmErrorKind, Flow, RelaxPair, SyntaxEntry, assemble, disassemble_object,
    format_asm_with,
};
use mtc_core::formats::object::ObjectFile;
use mtc_core::vm::OperandKind;

const ARCH: u8 = 0x7F;

/// nop 0x01 | stop 0x02 | jmp 0x20 far / 0x30 short | call 0x21 (far,
/// symbol operand — the binding call's carrier) | ent 0x0E.
fn syntax() -> ArchSyntax {
    use Flow::{Call, FallThrough as FT, Jump, Stop};
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
                mnemonic: "stop",
                operand: OperandKind::None,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: 0x20,
                mnemonic: "jmp",
                operand: OperandKind::RelI32,
                flow: Jump,
            },
            SyntaxEntry {
                opcode: 0x30,
                mnemonic: "jmp.s",
                operand: OperandKind::RelI8,
                flow: Jump,
            },
            SyntaxEntry {
                opcode: 0x21,
                mnemonic: "call",
                operand: OperandKind::RelI32,
                flow: Call,
            },
            SyntaxEntry {
                opcode: 0x0E,
                mnemonic: "ent",
                operand: OperandKind::None,
                flow: FT,
            },
        ],
        relax_pairs: vec![RelaxPair {
            far: 0x20,
            short: 0x30,
        }],
        entry_opcode: 0x0E,
        break_opcode: None,
        trap_opcode: None,
        return_opcode: None,
        caps: caps(),
    }
}

fn caps() -> AsmCaps {
    AsmCaps {
        tables: true,
        rept: true,
        vectors: true,
        volatile: false,
        interface: true,
    }
}

/// Every interface form the assembler can write, in one object: both
/// digest directives, the full `.param` suffix set, the `.routine` tail,
/// all four map states, an empty binding, and exit vectors in the second
/// blob.
const SOURCE: &str = "\
.graph lib::findA, 3735928559
.grafted other::h, 42
.routine f, tapes=1, alpha=(2)
.param ctl, ('_', '1')
.func f
        stop
.routine main, tapes=2, alpha=(5, 3), exits=2, noreturn
.param data, ('_', 'a', 'b', '0', '1'), writes=('0', '1'), enters=('a'), leaves=('0', '1'), opaque
.param ctl, ('_', '0', '1')
.func main
        call    g [num: 1{3->'0',4=>'1'}, ctl: 0{}] exits=(won, lost)
        call    g [1, 0{*}]
        call    g [1{3->'0',*}, 0]
        call    h [] exits=(won)
won:    stop
lost:   stop
";

/// [`SOURCE`] as the disassembler spells it — the canonical grid for
/// every new form. The only difference from the source is the two exit
/// labels: their names never reach the object (no `-g` here), so they
/// come back synthesized from their offsets.
const CANONICAL: &str = "\
.graph lib::findA, 3735928559
.grafted other::h, 42
.routine f, tapes=1, alpha=(2)
.param ctl, ('_', '1')
.func f
        stop
.routine main, tapes=2, alpha=(5, 3), exits=2, noreturn
.param data, ('_', 'a', 'b', '0', '1'), writes=('0', '1'), enters=('a'), leaves=('0', '1'), opaque
.param ctl, ('_', '0', '1')
.func main
        call    g [num: 1{3->'0',4=>'1'}, ctl: 0{}] exits=(L0015, L0016)
        call    g [1, 0{*}]
        call    g [1{3->'0',*}, 0]
        call    h [] exits=(L0015)
L0015:  stop
L0016:  stop
";

#[test]
fn interface_surface_round_trips_byte_identically() {
    let obj = assemble(&syntax(), ARCH, SOURCE, false).expect("assembles");
    let iface = obj.interface.as_ref().expect("an interface section");
    assert_eq!(iface.routines.len(), 2, "one routine record per blob");
    assert_eq!(iface.graphs.len(), 1);
    assert_eq!(obj.grafts.len(), 1);

    let main = &iface.routines[1];
    assert_eq!(main.params, vec!["data".to_string(), "ctl".to_string()]);
    assert_eq!(main.writes[0], vec!["0".to_string(), "1".to_string()]);
    assert_eq!(main.writes[1], Vec::<String>::new());
    assert_eq!(main.enters[0].as_deref(), Some(&["a".to_string()][..]));
    assert_eq!(
        main.leaves[0].as_deref(),
        Some(&["0".to_string(), "1".to_string()][..])
    );
    assert!(main.opaque[0] && !main.opaque[1]);
    assert_eq!(main.exits, 2);
    assert!(!main.returns);

    let bc = &obj.bound_calls[0];
    assert_eq!(bc.exits.len(), 2);
    assert_eq!(bc.binding[0].param.as_deref(), Some("num"));
    assert_eq!(bc.binding[0].pairs[0].dst_label.as_deref(), Some("0"));
    // A labelled pair carries NO index: `dst` is a placeholder the linker
    // fills once it can resolve the label against the callee's alphabet,
    // and the wire carries the label in its place — so it must be built
    // with 0 or it does not survive a round trip.
    assert_eq!(
        bc.binding[0].pairs[0].dst, 0,
        "a labelled pair carries dst 0"
    );
    assert_eq!(bc.binding[0].pairs[1].dst_label.as_deref(), Some("1"));
    assert_eq!(bc.binding[0].pairs[1].dst, 0);
    assert!(bc.binding[1].map_written && bc.binding[1].pairs.is_empty());
    assert!(obj.bound_calls[1].binding[1].open);
    assert!(obj.bound_calls[2].binding[0].open);
    assert!(obj.bound_calls[3].binding.is_empty());

    // The serializer carries everything the in-memory value holds: a
    // field dropped on the way out (or on the way back) shows up here,
    // not only in the two-in-memory-objects comparison below.
    assert_eq!(
        ObjectFile::from_bytes(&obj.to_bytes()).expect("reads back"),
        obj,
        "the object survives its own bytes"
    );

    let text = disassemble_object(&syntax(), &obj);
    assert_eq!(text, CANONICAL, "the interface listing's canonical grid");
    let again = assemble(&syntax(), ARCH, &text, false).expect("re-assembles");
    assert_eq!(
        again.to_bytes(),
        obj.to_bytes(),
        "dis → asm is byte-identical:\n{text}"
    );
    // The listing is already on the canonical grid: fmt over it is the
    // identity, so `dis` output never needs reformatting.
    assert_eq!(format_asm_with(&text, caps()).unwrap(), text, "{text}");
}

/// The four map states are four distinct spellings; none collapses into
/// another on the way out (docs/formats.md (bound calls)).
#[test]
fn every_map_state_keeps_its_own_spelling() {
    let obj = assemble(&syntax(), ARCH, SOURCE, false).unwrap();
    let text = disassemble_object(&syntax(), &obj);
    assert!(
        text.contains("call    g [num: 1{3->'0',4=>'1'}, ctl: 0{}] exits=(L0015, L0016)"),
        "{text}"
    );
    assert!(text.contains("call    g [1, 0{*}]\n"), "{text}");
    assert!(text.contains("call    g [1{3->'0',*}, 0]\n"), "{text}");
    assert!(text.contains("call    h [] exits=(L0015)\n"), "{text}");
}

/// `writes=` is omitted when the set is empty (an absent suffix and a
/// written `writes=()` are the same object), and the three contract
/// suffixes print in their one legal order.
#[test]
fn param_suffixes_print_only_when_they_carry_something() {
    let obj = assemble(&syntax(), ARCH, SOURCE, false).unwrap();
    let text = disassemble_object(&syntax(), &obj);
    assert!(
        text.contains(
            "\n.param data, ('_', 'a', 'b', '0', '1'), writes=('0', '1'), \
             enters=('a'), leaves=('0', '1'), opaque\n"
        ),
        "{text}"
    );
    assert!(text.contains("\n.param ctl, ('_', '0', '1')\n"), "{text}");
    assert!(!text.contains("writes=()"), "{text}");
    assert!(
        text.contains("\n.routine main, tapes=2, alpha=(5, 3), exits=2, noreturn\n"),
        "{text}"
    );
    assert!(
        text.contains("\n.routine f, tapes=1, alpha=(2)\n"),
        "{text}"
    );
}

/// `exits=` and `noreturn` are two independent fields of the tail, not a
/// pair: each prints on its own, in its own slot, and neither implies the
/// other. `exits=0` is the field's default and prints nothing at all.
#[test]
fn the_routine_tail_prints_its_two_fields_independently() {
    for (tail, expected) in [
        (", exits=1", ".routine f, tapes=1, alpha=(2), exits=1\n"),
        (", noreturn", ".routine f, tapes=1, alpha=(2), noreturn\n"),
        (
            ", exits=1, noreturn",
            ".routine f, tapes=1, alpha=(2), exits=1, noreturn\n",
        ),
        ("", ".routine f, tapes=1, alpha=(2)\n"),
    ] {
        let src = format!(
            ".routine f, tapes=1, alpha=(2){tail}\n.param ctl, ('_', '1')\n.func f\n        stop\n"
        );
        let obj = assemble(&syntax(), ARCH, &src, false).expect("assembles");
        let text = disassemble_object(&syntax(), &obj);
        assert!(text.starts_with(expected), "{tail:?} gave:\n{text}");
        // Exactly one field where only one was written.
        assert_eq!(
            text.contains("exits="),
            tail.contains("exits="),
            "{tail:?}:\n{text}"
        );
        assert_eq!(
            text.contains("noreturn"),
            tail.contains("noreturn"),
            "{tail:?}:\n{text}"
        );
        assert_eq!(
            assemble(&syntax(), ARCH, &text, false).unwrap().to_bytes(),
            obj.to_bytes(),
            "{tail:?}:\n{text}"
        );
        assert_eq!(format_asm_with(&text, caps()).unwrap(), text, "{text}");
    }
}

/// Graft provenance lives OUTSIDE the interface section on the wire, so a
/// file that only grafts — and defines no function at all — carries
/// `grafts` with `interface: None`. The `.grafted` lines must print from
/// that field, not from the interface.
#[test]
fn grafts_print_without_an_interface_section() {
    let src = ".grafted other::h, 42\n.grafted lib::k, 7\n";
    let obj = assemble(&syntax(), ARCH, src, false).expect("assembles");
    assert!(obj.interface.is_none());
    assert_eq!(obj.grafts.len(), 2);
    let text = disassemble_object(&syntax(), &obj);
    assert_eq!(text, src, "graft-only disassembly");
    assert_eq!(
        assemble(&syntax(), ARCH, &text, false).unwrap().to_bytes(),
        obj.to_bytes()
    );
}

/// An exit label names a position in the calling function, so it resolves
/// through the same function-local label map a jump target does.
#[test]
fn exit_labels_must_exist_in_the_function() {
    let src = "\
.routine main, tapes=1, alpha=(2)
.param ctl, ('_', '1')
.func main
        call    g [0] exits=(NOWHERE)
        stop
";
    let e = assemble(&syntax(), ARCH, src, false).expect_err("undefined exit label");
    assert!(
        matches!(e.kind, AsmErrorKind::UnknownLabel(ref l) if l == "NOWHERE"),
        "{:?}",
        e.kind
    );
}

/// `-g` does NOT bring the written exit-label names back. An exit target
/// is named the way a jump target is — synthesized from its offset —
/// because the blob's debug labels are not consulted for either; only a
/// position the TABLES section names keeps a name of its own. A recorded
/// limit of the listing, not a defect: the object is identical either way.
#[test]
fn exit_labels_are_synthesized_even_in_a_debug_build() {
    let with_debug = assemble(&syntax(), ARCH, SOURCE, true).expect("assembles");
    let text = disassemble_object(&syntax(), &with_debug);
    assert!(text.contains("exits=(L0015, L0016)"), "{text}");
    assert!(!text.contains("won"), "{text}");
    // The names are the only thing that moves: the exit vector on the
    // wire is the same offsets, and the debug section re-reads with the
    // synthesized names at those same addresses (a `-g` listing renames
    // every unnamed-in-tables position, exit targets and jump targets
    // alike — the object is otherwise unchanged).
    let again = assemble(&syntax(), ARCH, &text, true).expect("re-assembles");
    assert_eq!(again.bound_calls, with_debug.bound_calls);
    let no_debug = assemble(&syntax(), ARCH, SOURCE, false).unwrap();
    assert_eq!(
        assemble(&syntax(), ARCH, &text, false).unwrap().to_bytes(),
        no_debug.to_bytes(),
        "{text}"
    );
}

#[test]
fn v3_objects_disassemble_exactly_as_before() {
    // A source using none of the interface surface must produce a v3
    // object and the same text the pre-v4 disassembler printed.
    let src = "\
.func main
        call    g [0{1->1}]
        stop
.func g
        stop
";
    let obj = assemble(&syntax(), ARCH, src, false).unwrap();
    let bytes = obj.to_bytes();
    assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 3);
    let text = disassemble_object(&syntax(), &obj);
    assert_eq!(text, src, "{text}");
    assert!(!text.contains(".param"), "{text}");
}
