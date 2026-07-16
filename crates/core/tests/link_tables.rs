//! Linked table-section emission and executable-level table disassembly
//! (docs/formats.md (executable image)). Expected sections are derived
//! independently in each test and byte-compared against the linker's
//! output; everything runs through a neutral fake dialect (caps all on)
//! so core stays provably arch-agnostic.

use mtc_core::asm::{
    ArchSyntax, AsmCaps, Flow, RelaxPair, SyntaxEntry, assemble, disassemble_executable,
};
use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::{LinkOptions, LinkOutput, link};
use mtc_core::vm::OperandKind;

const ARCH: u8 = 0x7E;

/// Neutral fake dialect (per-file helper convention): `tmatch` references
/// a match table (FallThrough — a pure lookup), `tdispatch` a dispatch
/// table (Stop — transfers through it), a relaxable far/short call pair,
/// plus nop/stp/ent. Caps all on so `.section`/`.row`/`.targets`/
/// `.routine` shape.
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
        caps: AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        },
    }
}

fn asm(src: &str, with_debug: bool) -> ObjectFile {
    assemble(&fake_syntax(), ARCH, src, with_debug).expect("assembles")
}

fn link_one(obj: ObjectFile) -> LinkOutput {
    link(&fake_syntax(), &[obj], &[], LinkOptions::default()).expect("links")
}

/// Single function, one match + one dispatch table. Code layout (blob-
/// relative == absolute, main at 0, nothing relaxes): ent@0, tmatch@1
/// (hole 2..6), tdispatch@6 (hole 7..11), A: nop@11, B: stp@12.
const SINGLE: &str = "\
.routine main, tapes=2, alpha=(3, 5)
.section tables
T0: .row [1, 2]
    .row [1, *]
D0: .targets A, B
.section code
.func main
        tmatch  T0
        tdispatch D0
A:      nop
B:      stp
";

/// The independently derived table section for [`SINGLE`]: the match
/// table verbatim (rows are symbol indices, no rebasing), then the
/// dispatch table with entries as ABSOLUTE code addresses.
fn single_expected_tables() -> Vec<u8> {
    let mut expected = vec![2u8, 2, 0, 1, 2, 1, 0x7F]; // width 2, 2 rows
    expected.extend(2u16.to_le_bytes()); // dispatch count
    expected.extend(11u32.to_le_bytes()); // A
    expected.extend(12u32.to_le_bytes()); // B
    expected
}

#[test]
fn single_function_tables_link_to_a_sectioned_image() {
    let out = link_one(asm(SINGLE, false));
    let exe = &out.executable;
    assert_eq!(exe.tables, single_expected_tables());
    // Header from the entry's `.routine`: tapes=2, alpha=(3, 5), base profile.
    assert_eq!(exe.tape_count, 2);
    assert_eq!(exe.profile, 0);
    assert_eq!(exe.alphabet_cardinalities, vec![3, 5]);
    // The image serializes as the sectioned format version 2.
    let bytes = exe.to_bytes();
    assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2);
    // TableRef holes patched to section offsets: tmatch -> 0, tdispatch -> 7.
    assert_eq!(&exe.code[2..6], &0u32.to_le_bytes());
    assert_eq!(&exe.code[7..11], &7u32.to_le_bytes());
}

#[test]
fn two_functions_with_tables_get_per_function_bases() {
    // main owns a match table, helper owns a dispatch table; the section
    // concatenates them in layout order (main first), so helper's table
    // base is the match table's size and its TableRef hole is patched
    // with a NONZERO section offset.
    let src = "\
.routine main, tapes=1, alpha=(2)
.routine helper, tapes=1, alpha=(2)
.section tables
TM: .row [1]
    .row [*]
TH: .targets H
.section code
.func main
        tmatch  TM
        call    helper
        stp
.func helper
        tdispatch TH
H:      nop
        stp
";
    let out = link_one(asm(src, false));
    let exe = &out.executable;
    // main: ent@0, tmatch@1 (hole 2..6), call@6 -> call.s (2 bytes,
    // helper is close), stp@8 — size 9; helper base 9: ent@9,
    // tdispatch@10 (hole 11..15), H: nop@15, stp@16.
    assert_eq!(exe.code[6], 0x31, "call relaxed short");
    assert_eq!(out.map.functions[1].start, 9);
    // Section: TM (5 bytes, base 0) then TH (base 5); TH's one entry is
    // H's absolute address 15 = helper base 9 + blob-relative 6.
    let mut expected = vec![1u8, 2, 0, 1, 0x7F];
    expected.extend(1u16.to_le_bytes());
    expected.extend(15u32.to_le_bytes());
    assert_eq!(exe.tables, expected);
    // TableRef holes: main's tmatch -> section 0; helper's tdispatch ->
    // section 5, at absolute hole 9 + 1 + 1 = 11.
    assert_eq!(&exe.code[2..6], &0u32.to_le_bytes());
    assert_eq!(&exe.code[11..15], &5u32.to_le_bytes());
    // Header from the ENTRY function's signature.
    assert_eq!(exe.tape_count, 1);
    assert_eq!(exe.alphabet_cardinalities, vec![2]);
}

#[test]
fn dispatch_entries_follow_a_relaxation_shift() {
    // A far call BEFORE the dispatch-target label narrows to call.s at
    // link time, moving the label 3 bytes down — the dispatch entry must
    // land on the SHIFTED address, not the blob-relative original.
    let src = "\
.routine main, tapes=1, alpha=(2)
.routine helper, tapes=1, alpha=(2)
.section tables
D:  .targets A
.section code
.func main
        tdispatch D
        call    helper
A:      stp
.func helper
        ret
";
    let obj = asm(src, false);
    // In the object, A sits at blob offset 11 (ent@0, tdispatch@1..6,
    // far call@6..11).
    let blob_relative = u32::from_le_bytes(
        obj.table_blobs.as_ref().unwrap()[0][2..6]
            .try_into()
            .unwrap(),
    );
    assert_eq!(blob_relative, 11);
    let out = link_one(obj);
    let exe = &out.executable;
    assert_eq!(exe.code[6], 0x31, "call relaxed short");
    // Linked: ent@0, tdispatch@1..6, call.s@6..8, A: stp@8.
    let mut expected = Vec::new();
    expected.extend(1u16.to_le_bytes());
    expected.extend(8u32.to_le_bytes());
    assert_eq!(exe.tables, expected, "entry follows the shifted label");
}

#[test]
fn table_ref_holes_follow_a_relaxation_shift() {
    // The dual of the dispatch-entry case: here the TABLE REFERENCE
    // itself sits after a far call that narrows, so the hole's final
    // code position moves — the patch must land at the shifted offset,
    // not the blob-relative one.
    let src = "\
.routine main, tapes=1, alpha=(2)
.routine helper, tapes=1, alpha=(2)
.section tables
T:  .row [1]
    .row [*]
.section code
.func main
        call    helper
        tmatch  T
        stp
.func helper
        ret
";
    let out = link_one(asm(src, false));
    let exe = &out.executable;
    // Object blob: ent@0, far call@1..6, tmatch@6 (hole 7..11), stp@11.
    // Linked: call.s@1..3 (helper at 9, end 3 -> off 6), tmatch@3
    // (hole 4..8, patched to section offset 0), stp@8; helper@9.
    assert_eq!(
        exe.code,
        vec![
            0x0E, 0x31, 0x06, 0x11, 0x00, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B
        ]
    );
    assert_eq!(exe.tables, vec![1, 2, 0, 1, 0x7F]);
}

// ------------------------------------------------------------------
// Executable-level disassembly + the strong round trip
// ------------------------------------------------------------------

#[test]
fn executable_dis_renders_routine_and_tables_with_map_labels() {
    let syntax = fake_syntax();
    let out = link_one(asm(SINGLE, true)); // -g: the map carries A/B
    let text = disassemble_executable(&syntax, &out.executable, Some(&out.map));
    let expected = "\
.routine main, tapes=2, alpha=(3, 5)
.section tables
T0:     .row    [1, 2]
        .row    [1, *]
T1:     .targets A, B
.section code
.func main
        tmatch  T0
        tdispatch T1
A:      nop
B:      stp
";
    assert_eq!(text, expected, "sectioned disassembly:\n{text}");
}

#[test]
fn executable_dis_without_map_renders_raw_hex_targets() {
    let syntax = fake_syntax();
    let out = link_one(asm(SINGLE, false));
    let text = disassemble_executable(&syntax, &out.executable, None);
    assert!(
        text.contains(".routine main, tapes=2, alpha=(3, 5)"),
        "{text}"
    );
    assert!(
        text.contains(".targets 0x000b, 0x000c"),
        "raw hex without a map:\n{text}"
    );
    assert!(
        text.contains("; unresolved dispatch targets"),
        "defensive comment flags the raw form:\n{text}"
    );
    // The dispatch-reachable code is still discovered as instructions.
    assert!(text.contains("nop"), "{text}");
    assert!(!text.contains(".byte"), "{text}");
}

/// The strong round trip: link, disassemble WITH the map, re-assemble
/// the rendered text, re-link — the executable images must be
/// byte-identical.
#[test]
fn sectioned_disassembly_round_trips_byte_identically() {
    let syntax = fake_syntax();
    let out = link_one(asm(SINGLE, true));
    let text = disassemble_executable(&syntax, &out.executable, Some(&out.map));
    let obj2 = assemble(&syntax, ARCH, &text, false).expect("rendered text re-assembles");
    let out2 = link(&syntax, &[obj2], &[], LinkOptions::default()).expect("re-links");
    assert_eq!(
        out2.executable.to_bytes(),
        out.executable.to_bytes(),
        "dis ∘ link must reproduce the image byte-for-byte"
    );
}

/// A tableless code-only image must disassemble with NO `.routine` and
/// NO `.section` lines — byte-compatible with the pre-tables renderer.
#[test]
fn code_only_dis_is_byte_compatible() {
    let syntax = fake_syntax();
    let src = ".func main\n        call    helper\n        stp\n.func helper\n        ret\n";
    let out = link_one(asm(src, false));
    let text = disassemble_executable(&syntax, &out.executable, Some(&out.map));
    assert!(!text.contains(".routine"), "{text}");
    assert!(!text.contains(".section"), "{text}");
}
