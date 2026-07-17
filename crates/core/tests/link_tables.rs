//! Linked table-section emission and executable-level table disassembly
//! (docs/formats.md (executable image)). Expected sections are derived
//! independently in each test and byte-compared against the linker's
//! output; everything runs through a neutral fake dialect (caps all on)
//! so core stays provably arch-agnostic.

use mtc_core::asm::{
    ArchSyntax, AsmCaps, Flow, RelaxPair, SyntaxEntry, assemble, disassemble_executable,
};
use mtc_core::formats::object::{BoundCall, ObjectFile, Symbol, SymbolDef};
use mtc_core::linker::{LinkError, LinkOptions, LinkOutput, link};
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
            // A framed call (`call.m`-shape): FramedCall operand, Call flow;
            // never relaxed in 5a.
            SyntaxEntry {
                opcode: 0x14,
                mnemonic: "fcall",
                operand: OperandKind::FramedCall,
                flow: Call,
            },
            // Read/write/move + the trap instruction: the surface the mono
            // stamping engine projects and synthesizes (a fake mirror of the
            // TM-1 shapes so core stays arch-agnostic).
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
        // The unmapped-symbol trap the mono stamping engine synthesizes.
        trap_opcode: Some(0x18),
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

/// A single-function frames program: `main` calls itself framed through a
/// `.frame` descriptor with a non-identity rmap (a `->`, a one-way `=>`,
/// and a hole) and two exits into `main`. Single-function so the
/// disassembler's entry-only `.routine` synthesis re-assembles.
const FRAMES: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
F0: .frame tapes=(1, 0)
    .map 0, rmap=(1->2, 3=>1)
    .exits done, other
.section code
.func main
        fcall   main, F0
done:   stp
other:  stp
";

#[test]
fn frames_link_selects_the_frames_profile_with_absolute_exits() {
    // A frame descriptor + a framed call ⇒ PROFILE_FRAMES. The framed call
    // is fixed 9 bytes (never relaxed); its displacement half patches to
    // the callee like a far call. The exit vector's blob-relative offsets
    // rebase to ABSOLUTE code addresses.
    let out = link(
        &fake_syntax(),
        &[asm(FRAMES, true)],
        &[],
        LinkOptions::default(),
    )
    .expect("links");
    let exe = &out.executable;
    assert_eq!(exe.profile, 1, "frames image ⇒ PROFILE_FRAMES");
    // ent@0, fcall@1 (opcode + 4-byte rel + 4-byte frame ref = 1..10),
    // done: stp@10, other: stp@11. The framed call self-targets main (0),
    // so the displacement (instruction end 10 → target 0) is -10.
    assert_eq!(exe.code[1], 0x14, "framed-call opcode kept");
    let rel = i32::from_le_bytes(exe.code[2..6].try_into().unwrap());
    assert_eq!(rel, -10, "displacement patched to the callee base");
    // The single framed call becomes site 0 (the frame half is the site
    // index now, not the descriptor offset).
    assert_eq!(
        u32::from_le_bytes(exe.code[6..10].try_into().unwrap()),
        0,
        "the raw framed call is site 0"
    );
    // The descriptor is the whole tables section EXCEPT the trailing frames
    // region, so its exit vector's two u32s end right at frames_offset:
    // done=10, other=11.
    let tables = &exe.tables;
    let exits_at = exe.frames_offset as usize - 8;
    assert_eq!(&tables[exits_at..exits_at + 4], &10u32.to_le_bytes());
    assert_eq!(&tables[exits_at + 4..exits_at + 8], &11u32.to_le_bytes());
}

#[test]
fn frames_dis_round_trips_the_linked_image() {
    // asm → link → dis(with map) → asm → link is byte-identical: the frame
    // surface (`.frame`/`.map`/`.exits`) and the framed call all reproduce.
    let out = link(
        &fake_syntax(),
        &[asm(FRAMES, true)],
        &[],
        LinkOptions::default(),
    )
    .expect("links");
    let text = disassemble_executable(&fake_syntax(), &out.executable, Some(&out.map));
    assert!(text.contains("F0:     .frame  tapes=(1, 0)"), "{text}");
    assert!(text.contains(".map    0, rmap=(1->2, 3->1)"), "{text}");
    assert!(text.contains(".exits  done, other"), "{text}");
    assert!(text.contains("fcall   main, F0"), "{text}");
    let out2 = link(
        &fake_syntax(),
        &[asm(&text, false)],
        &[],
        LinkOptions::default(),
    )
    .expect("re-links");
    assert_eq!(out2.executable.to_bytes(), out.executable.to_bytes());
}

/// Two distinct framed calls to two distinct descriptors: the linker
/// builds a K=2, S=2 frames region and rewrites each call's frame half to
/// its dense site index. The region bytes are derived independently from
/// the layout (docs/formats.md (frames region)) and byte-compared.
const TWO_SITES: &str = "\
.routine main, tapes=1, alpha=(2)
.section tables
F0: .frame tapes=(0)
    .exits A
F1: .frame tapes=(0)
    .exits B
.section code
.func main
        fcall   main, F0
        fcall   main, F1
A:      stp
B:      stp
";

#[test]
fn two_raw_sites_build_the_directory_and_constant_compose_columns() {
    let out = link_one(asm(TWO_SITES, false));
    let exe = &out.executable;
    assert_eq!(exe.profile, 1, "two framed calls ⇒ PROFILE_FRAMES");
    // Each `.frame tapes=(0)` with one exit is 12 bytes: arity(1) +
    // exit_count(2) + tape0 phys/rmap_len/wmap_len(5) + one exit u32(4). So
    // F0 sits at 0, F1 at 12, and the region begins at 24.
    assert_eq!(exe.frames_offset, 24);
    // The two framed calls become dense sites 0 and 1. ent@0, fcall@1..10
    // (frame half at 6), fcall@10..19 (frame half at 15).
    assert_eq!(u32::from_le_bytes(exe.code[6..10].try_into().unwrap()), 0);
    assert_eq!(u32::from_le_bytes(exe.code[15..19].try_into().unwrap()), 1);
    // Region: K=2, S=2, directory=[0, 12] (F0, F1 in ascending order),
    // compose (K+1=3 rows × S=2 cols) all constant columns — site 0 → F0
    // (composite 1), site 1 → F1 (composite 2).
    let base = exe.frames_offset as usize;
    let mut expected = Vec::new();
    expected.extend(2u16.to_le_bytes()); // K
    expected.extend(2u16.to_le_bytes()); // S
    expected.extend(0u32.to_le_bytes()); // directory[0] = F0
    expected.extend(12u32.to_le_bytes()); // directory[1] = F1
    for _ in 0..=2u16 {
        expected.extend(1u16.to_le_bytes()); // compose[F][0] = 1
        expected.extend(2u16.to_le_bytes()); // compose[F][1] = 2
    }
    assert_eq!(&exe.tables[base..], &expected[..]);
    // The descriptors precede the region untouched: F0 at 0, F1 at 12.
    assert_eq!(exe.tables[0], 1, "F0 arity"); // arity byte
    assert_eq!(exe.tables[12], 1, "F1 arity");
}

#[test]
fn two_raw_sites_dis_round_trips_byte_identically() {
    // The strong round trip with a NON-zero descriptor offset (F1 at 12):
    // dis must resolve site 1 through the region to F1, not read F1's
    // offset as if it were the operand. Re-asm + re-link reproduces the
    // region deterministically.
    let syntax = fake_syntax();
    let out = link_one(asm(TWO_SITES, true));
    let text = disassemble_executable(&syntax, &out.executable, Some(&out.map));
    assert!(text.contains("fcall   main, F0"), "site 0 → F0:\n{text}");
    assert!(text.contains("fcall   main, F1"), "site 1 → F1:\n{text}");
    let out2 = link(
        &fake_syntax(),
        &[asm(&text, false)],
        &[],
        LinkOptions::default(),
    )
    .expect("re-links");
    assert_eq!(out2.executable.to_bytes(), out.executable.to_bytes());
}

#[test]
fn frameless_tabled_link_stays_base_profile() {
    // The profile-emission lock in the other direction: a tabled but
    // frameless link is PROFILE_BASE — table support must not flip the
    // profile byte on a frame-free image.
    let out = link_one(asm(SINGLE, false));
    assert_eq!(out.executable.profile, 0);
}

#[test]
fn frame_exits_follow_a_relaxation_shift() {
    // A far call BEFORE an exit label narrows to call.s at link time,
    // moving the label 3 bytes down — the frame exit vector must land on
    // the SHIFTED address, exactly like a dispatch entry.
    let src = "\
.routine main, tapes=1, alpha=(2)
.routine helper, tapes=1, alpha=(2)
.section tables
F0: .frame tapes=(0)
    .exits A
.section code
.func main
        fcall   main, F0
        call    helper
A:      stp
.func helper
        ret
";
    let obj = asm(src, false);
    // In the object: ent@0, fcall@1..10, far call@10..15, A: stp@15.
    // The descriptor is [arity 1][exit_count 1,0][tape0 phys 0, rmap_len
    // 0,0, wmap_len 0,0][exit A u32] — the exit sits at descriptor offset 8.
    let blob_relative = u32::from_le_bytes(
        obj.table_blobs.as_ref().unwrap()[0][8..12]
            .try_into()
            .unwrap(),
    );
    assert_eq!(blob_relative, 15);
    let out = link_one(obj);
    let exe = &out.executable;
    assert_eq!(exe.profile, 1);
    assert_eq!(exe.code[10], 0x31, "call relaxed short");
    // Linked: ent@0, fcall@1..10, call.s@10..12, A: stp@12.
    let exit = u32::from_le_bytes(exe.tables[8..12].try_into().unwrap());
    assert_eq!(exit, 12, "exit follows the shifted label");
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

// --- Entry selection and declarative bound-call reachability ---
//
// Bound calls carry no assembler surface in the fake dialect, so these
// build objects directly. Each function is a minimal `ent; stp` body
// (`[0x0E, 0x02]`, valid fake-dialect code). Bound-call records are added
// by hand; resolve reads only their `symbol`/`offset`, and the linker
// refuses a reachable one before layout ever decodes the blob.

/// An object of ent+stp functions named `names`, all `Defined`, blob i
/// per name i.
fn bare_object(names: &[&str]) -> ObjectFile {
    let symbols = names
        .iter()
        .enumerate()
        .map(|(i, n)| Symbol {
            name: (*n).into(),
            def: SymbolDef::Defined { blob: i as u32 },
        })
        .collect();
    let blobs = names.iter().map(|_| vec![0x0E, 0x02]).collect();
    ObjectFile::v2(ARCH, symbols, blobs, Vec::new(), None)
}

#[test]
fn entry_override_links_a_function_unreachable_from_main() {
    // `alt` is unreachable from `main`; the default entry drops it, but
    // `--entry alt` makes it the root and drops `main` instead.
    let obj = bare_object(&["main", "alt"]);
    let out = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        LinkOptions {
            entry: Some("alt".into()),
            ..Default::default()
        },
    )
    .expect("links with alt as the entry");
    let names: Vec<&str> = out.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["alt"]);
    assert_eq!(out.report.dropped, vec!["main".to_string()]);
}

#[test]
fn a_reachable_bound_call_without_a_signed_entry_is_missing_signature() {
    // `main` bound-calls `sub`, but `main` is unsigned — the composition
    // engine has no machine signature to compose against, so the link is
    // refused for the missing entry signature.
    let mut obj = bare_object(&["main", "sub"]);
    obj.bound_calls.push(BoundCall {
        blob: 0,
        offset: 1,
        symbol: 1, // "sub"
        binding: Vec::new(),
    });
    let e = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        LinkOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e, LinkError::MissingSignature("main".into()));
}

#[test]
fn a_bound_call_in_a_dropped_function_does_not_poison_the_link() {
    // `dead` bound-calls `sub`, but nothing reaches `dead` from `main`, so
    // its binding never runs — the link succeeds (pre-5b the guard fired
    // on ANY bound call, reachable or not).
    let mut obj = bare_object(&["main", "sub", "dead"]);
    obj.bound_calls.push(BoundCall {
        blob: 2, // "dead"
        offset: 1,
        symbol: 1, // "sub"
        binding: Vec::new(),
    });
    let out = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        LinkOptions::default(),
    )
    .expect("a dropped function's binding does not poison the link");
    let names: Vec<&str> = out.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["main"]);
    assert!(out.report.dropped.contains(&"dead".to_string()));
    assert!(out.report.dropped.contains(&"sub".to_string()));
}

#[test]
fn an_unresolved_bound_callee_is_an_unresolved_error() {
    // `main` bound-calls `ghost`, which no object defines: a bound callee
    // enters reachability like a relocation callee, so an undefined one
    // errors as Unresolved.
    let mut obj = bare_object(&["main"]);
    obj.symbols.push(Symbol {
        name: "ghost".into(),
        def: SymbolDef::External,
    });
    obj.bound_calls.push(BoundCall {
        blob: 0,
        offset: 1,
        symbol: 1, // "ghost"
        binding: Vec::new(),
    });
    let e = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        LinkOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e, LinkError::Unresolved(vec!["ghost".into()]));
}

// -- The composition engine (phase 5b): closure + FRAMES lowering --------
//
// Every expected frames region is decoded from the executable here and
// checked structurally; the fake dialect proves core stays arch-agnostic.

use mtc_core::formats::PROFILE_FRAMES;
use mtc_core::linker::CallMech;

/// FRAMES-mode link options (the mechanism this phase implements).
fn frames_opts() -> LinkOptions {
    LinkOptions {
        call_mech: CallMech::Frames,
        ..Default::default()
    }
}

/// The decoded frames region (docs/formats.md (frames region)).
#[derive(Debug, PartialEq, Eq)]
struct Region {
    k: u16,
    s: u16,
    directory: Vec<u32>,
    /// `(k+1)` rows of `s` columns each.
    compose: Vec<Vec<u16>>,
}

fn parse_region(exe: &mtc_core::formats::executable::Executable) -> Option<Region> {
    if exe.frames_offset == 0 {
        return None;
    }
    let t = &exe.tables;
    let mut p = exe.frames_offset as usize;
    let k = u16::from_le_bytes([t[p], t[p + 1]]);
    let s = u16::from_le_bytes([t[p + 2], t[p + 3]]);
    p += 4;
    let mut directory = Vec::new();
    for _ in 0..k {
        directory.push(u32::from_le_bytes(t[p..p + 4].try_into().unwrap()));
        p += 4;
    }
    let mut compose = Vec::new();
    for _ in 0..=k {
        let mut row = Vec::new();
        for _ in 0..s {
            row.push(u16::from_le_bytes([t[p], t[p + 1]]));
            p += 2;
        }
        compose.push(row);
    }
    Some(Region {
        k,
        s,
        directory,
        compose,
    })
}

/// A single non-collapsing bound call links in FRAMES mode: one framed
/// site, one composite, the frames profile.
#[test]
fn a_single_bound_call_lowers_to_one_framed_site() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub [0{1->2, 2->1}, 1]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    assert_eq!(out.executable.profile, PROFILE_FRAMES);
    let region = parse_region(&out.executable).expect("frames region present");
    assert_eq!(region.k, 1, "one composite");
    assert_eq!(region.s, 1, "one framed site");
    // Row 0 (identity): main's site activates composite 1. Row 1 (that
    // composite): unreachable for this site, 0.
    assert_eq!(region.compose[0], vec![1]);
    assert_eq!(region.compose[1], vec![0]);
}

/// Two-level nesting R→Q under two contexts: `main` bound-calls `r` under
/// two distinct composites, and `r` bound-calls `q` at one site. That
/// site's compose column differs by active-frame row — the engine's core
/// behavior (one site, a different composite per context).
#[test]
fn a_site_reached_under_two_contexts_has_a_row_dependent_column() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine r, tapes=2, alpha=(4, 4)
.routine q, tapes=2, alpha=(4, 4)
.section code
.func main
        call    r [0{1->2, 2->1}, 1]
        call    r [0{1->3, 3->1}, 1]
        stp
.func r
        call    q [0{2->3, 3->2}, 1]
        ret
.func q
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    let region = parse_region(&out.executable).expect("frames region");
    // Four composites: E1, E2 (main's two calls), then C1, C2 (r's site
    // composed under each). Three framed sites: main's two + r's one.
    assert_eq!(region.k, 4);
    assert_eq!(region.s, 3);
    // Column order = (function, offset): main.b1, main.b2, r.S.
    // Row 0 (identity) activates main's two composites; r's site is not
    // reached at identity.
    assert_eq!(region.compose[0], vec![1, 2, 0]);
    // r's site (column 2) resolves to a DIFFERENT composite under E1 (row 1)
    // than under E2 (row 2) — the same site, a context-dependent frame.
    assert_ne!(region.compose[1][2], region.compose[2][2]);
    assert_eq!(region.compose[1][2], 3);
    assert_eq!(region.compose[2][2], 4);
    // main's sites are not reached under any non-identity row.
    assert_eq!(region.compose[1][0], 0);
    assert_eq!(region.compose[2][1], 0);
}

/// A full-arity identity binding collapses to a plain call (§5.6): no framed
/// site, no frames region — the callee inherits the caller's frame.
#[test]
fn a_full_identity_binding_collapses_to_a_plain_call() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub [0, 1]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    assert_ne!(out.executable.profile, PROFILE_FRAMES, "no frames needed");
    assert_eq!(out.executable.frames_offset, 0);
    // The collapsed bound call is now an ordinary relaxed call, never a
    // framed call (opcode 0x14).
    assert!(
        !out.executable.code.contains(&0x14),
        "no framed call emitted"
    );
}

/// A projecting identity binding (fewer tapes than the caller) is NOT a
/// pass-through and stays a framed call — the projection guard on the §5.6
/// collapse rule (a 1-tape identity composite under a 2-tape caller).
#[test]
fn a_projecting_identity_binding_stays_a_framed_call() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=1, alpha=(4)
.section code
.func main
        call    sub [0]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    assert_eq!(out.executable.profile, PROFILE_FRAMES);
    let region = parse_region(&out.executable).expect("frames region");
    assert_eq!(region.k, 1, "the projecting composite is real");
    assert_eq!(region.s, 1, "the site stays a framed call");
    assert!(out.executable.code.contains(&0x14), "framed call emitted");
}

/// Two sites binding the same callee with the same binding compose to the
/// same composite — deduped to ONE directory entry, but two columns.
#[test]
fn equal_composites_dedup_to_one_directory_entry() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub [0{1->2, 2->1}, 1]
        call    sub [0{1->2, 2->1}, 1]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    let region = parse_region(&out.executable).expect("frames region");
    assert_eq!(region.k, 1, "one deduped composite for two equal sites");
    assert_eq!(region.s, 2, "two framed sites");
    assert_eq!(region.compose[0], vec![1, 1], "both sites -> composite 1");
}

/// An out-of-alphabet caller symbol is a static link error (the caller-side
/// range the algebra leaves to the linker).
#[test]
fn an_out_of_range_caller_symbol_is_a_link_error() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(8, 8)
.section code
.func main
        call    sub [0{5->1}, 1]
        stp
.func sub
        ret
";
    let e = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).unwrap_err();
    match e {
        LinkError::BadBinding { callee, message } => {
            assert_eq!(callee, "sub");
            assert!(
                message.contains('5') && message.contains("caller"),
                "{message}"
            );
        }
        other => panic!("expected BadBinding, got {other:?}"),
    }
}

/// An equal-size binding whose identity completion is non-injective is a
/// static link error (the completed bijection the linker requires).
#[test]
fn a_non_injective_equal_size_binding_is_a_link_error() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub [0{1->2}, 1]
        stp
.func sub
        ret
";
    let e = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).unwrap_err();
    assert!(
        matches!(&e, LinkError::BadBinding { message, .. } if message.contains("injective")),
        "{e:?}"
    );
}

/// Two independent links of the same program are byte-identical — the
/// closure order is deterministic (reproducible builds).
#[test]
fn a_bound_call_link_is_deterministic() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine r, tapes=2, alpha=(4, 4)
.routine q, tapes=2, alpha=(4, 4)
.section code
.func main
        call    r [0{1->2, 2->1}, 1]
        call    r [0{1->3, 3->1}, 1]
        stp
.func r
        call    q [0{2->3, 3->2}, 1]
        ret
.func q
        ret
";
    let a = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).unwrap();
    let b = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).unwrap();
    assert_eq!(a.executable.to_bytes(), b.executable.to_bytes());
}

/// A bound call in an unreachable function is never lowered — the routine
/// (and its callee) drop, and no frames region appears.
#[test]
fn a_dropped_functions_bound_call_is_not_lowered() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine dead, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        stp
.func dead
        call    sub [0{1->2, 2->1}, 1]
        ret
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    assert_eq!(out.executable.frames_offset, 0, "nothing framed");
    assert_ne!(out.executable.profile, PROFILE_FRAMES);
    assert!(out.report.dropped.contains(&"dead".to_string()));
    assert!(out.report.dropped.contains(&"sub".to_string()));
}

// -- Mono stamping + hybrid classification (phase 5b) --------------------
//
// Mono lowers each bound site to a plain call into a stamped copy on the
// BASE profile; hybrid classifies per site. Stamps are map-visible synthetic
// functions named `<callee>$<digest8>`.

fn mono_opts() -> LinkOptions {
    LinkOptions {
        call_mech: CallMech::Mono,
        ..Default::default()
    }
}

fn hybrid_opts() -> LinkOptions {
    LinkOptions {
        call_mech: CallMech::Hybrid,
        ..Default::default()
    }
}

/// The map functions whose name marks them a mono stamp (`<callee>$<hex>`).
fn stamp_names(out: &LinkOutput) -> Vec<String> {
    out.map
        .functions
        .iter()
        .filter(|f| f.name.contains('$'))
        .map(|f| f.name.clone())
        .collect()
}

/// A single non-collapse bound call under mono stamps one base-profile copy.
#[test]
fn a_mono_bound_call_stamps_a_base_profile_copy() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub [0{1->2, 2->1}, 1]
        stp
.func sub
        wr [1, -]
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], mono_opts()).expect("links");
    assert_ne!(
        out.executable.profile, PROFILE_FRAMES,
        "mono ⇒ base profile"
    );
    assert_eq!(out.executable.frames_offset, 0, "no frames region");
    assert_eq!(
        stamp_names(&out).len(),
        1,
        "exactly one stamp: {:?}",
        out.map
    );
    assert!(
        stamp_names(&out)[0].starts_with("sub$"),
        "stamp named after the callee: {:?}",
        stamp_names(&out)
    );
}

/// Two sites binding the same callee the same way stamp ONE deduped copy.
#[test]
fn mono_dedups_equal_composites() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub [0{1->2, 2->1}, 1]
        call    sub [0{1->2, 2->1}, 1]
        stp
.func sub
        wr [1, -]
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], mono_opts()).expect("links");
    assert_eq!(
        stamp_names(&out).len(),
        1,
        "two equal composites dedup to one stamp: {:?}",
        stamp_names(&out)
    );
}

/// A full-arity identity binding collapses to a plain call into the ORIGINAL
/// routine — no stamp, no frames (§5.6).
#[test]
fn an_identity_binding_under_mono_calls_the_original() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub [0, 1]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], mono_opts()).expect("links");
    assert!(
        stamp_names(&out).is_empty(),
        "identity collapses, no stamp: {:?}",
        stamp_names(&out)
    );
    assert_ne!(out.executable.profile, PROFILE_FRAMES);
    assert!(!out.executable.code.contains(&0x14), "no framed call");
}

/// A raw `call.m` reached under mono is a contradiction (the base profile has
/// no compose machinery) — a clear link error.
#[test]
fn a_raw_call_m_under_mono_is_a_link_error() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.routine leaf, tapes=2, alpha=(4, 4)
.section tables
Fr: .frame  tapes=(0, 1)
    .map    0, rmap=(1->2)
.section code
.func main
        call    sub [0{1->2, 2->1}, 1]
        fcall   leaf, Fr
        stp
.func sub
        ret
.func leaf
        ret
";
    let e = link(&fake_syntax(), &[asm(src, false)], &[], mono_opts()).unwrap_err();
    assert_eq!(e, LinkError::MonoRawFrame("main".into()));
}

/// The hand-derived read-table rewrite: a machine-width match table with
/// synthesized trap rows PREPENDED, a one-way collapse expanding one row into
/// two, and a no-preimage row DROPPED with the paired dispatch renumbered.
/// `main` (1 tape, alphabet 4) mono-calls `sub` (1 tape, alphabet 3) binding
/// physical 1 and 2 both onto virtual 1 (a one-way collapse); physical 3 has
/// no virtual image (a read hole).
#[test]
fn mono_read_table_rewrite_is_byte_derived() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine sub, tapes=1, alpha=(3)
.section tables
T0: .row [0]
    .row [1]
    .row [2]
D0: .targets A, B, C
.section code
.func main
        call    sub [0{1=>1, 2=>1}]
        stp
.func sub
        rd
        tmatch  T0
        tdispatch D0
A:      wr [0]
        ret
B:      wr [1]
        ret
C:      wr [2]
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], mono_opts()).expect("links");
    let exe = &out.executable;
    assert_ne!(exe.profile, PROFILE_FRAMES, "mono stays base profile");

    // Locate the stamp function and its code range.
    let stamp = out
        .map
        .functions
        .iter()
        .find(|f| f.name.starts_with("sub$"))
        .expect("one stamp of sub");
    let code = &exe.code;
    let (mut match_off, mut disp_off, mut wr_a) = (None, None, None);
    let mut i = stamp.start as usize;
    while i < stamp.end as usize {
        match code[i] {
            0x11 => {
                // tmatch: 4-byte section offset follows.
                match_off = Some(u32::from_le_bytes(code[i + 1..i + 5].try_into().unwrap()));
                i += 5;
            }
            0x12 => {
                disp_off = Some(u32::from_le_bytes(code[i + 1..i + 5].try_into().unwrap()));
                i += 5;
            }
            0x07 => {
                // First wr in the stamp is `A: wr [0]` → physical 0.
                if wr_a.is_none() {
                    wr_a = Some(code[i + 1]);
                }
                i += 2; // opcode + one self-delimiting byte
            }
            0x04 | 0x0B | 0x02 | 0x0E => i += 1, // rd / ret / stp / ent
            0x18 => i += 2,                      // trap #k
            other => panic!("unexpected opcode {other:#04x} in stamp at {i}"),
        }
    }
    let match_off = match_off.expect("stamp has a match table") as usize;
    let disp_off = disp_off.expect("stamp has a dispatch table") as usize;

    // Match table: width 1, four rows [3][0][1][2] — the trap row for the
    // read hole 3 FIRST, then virtual 0's preimage [0], then virtual 1's two
    // preimages [1] and [2] (the collapse expansion). Virtual 2 (no
    // preimage) dropped.
    let tbl = &exe.tables;
    assert_eq!(tbl[match_off], 1, "machine-width match table");
    assert_eq!(
        u16::from_le_bytes([tbl[match_off + 1], tbl[match_off + 2]]),
        4,
        "trap row + 3 surviving rows"
    );
    assert_eq!(
        &tbl[match_off + 3..match_off + 7],
        &[3u8, 0, 1, 2],
        "rows: [3](trap) [0] [1] [2]"
    );

    // Dispatch: four entries. entry[0] → the trap stub (`trap #0`), entry[1]
    // → A, entries[2] and [3] → B (the collapse points both preimages at the
    // same target). C is dropped (no dispatch entry).
    assert_eq!(
        u16::from_le_bytes([tbl[disp_off], tbl[disp_off + 1]]),
        4,
        "one trap entry + three row entries"
    );
    let entry = |k: usize| {
        let at = disp_off + 2 + k * 4;
        u32::from_le_bytes(tbl[at..at + 4].try_into().unwrap()) as usize
    };
    assert_eq!(code[entry(0)], 0x18, "trap-stub opcode");
    assert_eq!(code[entry(0) + 1], 0, "trap #0 (unmapped read)");
    assert_eq!(code[entry(1)], 0x07, "row 0 → A: wr");
    assert_eq!(
        entry(2),
        entry(3),
        "the collapse expansion shares one target"
    );
    assert_eq!(code[entry(2)], 0x07, "collapse rows → B: wr");

    // The write projection: `A: wr [0]` writes virtual 0 → physical 0. The
    // self-delimiting byte carries payload 0 with the high (last) bit set.
    assert_eq!(
        wr_a.expect("a wr in the stamp"),
        0x80,
        "wr [0] → physical 0"
    );
}

/// Hybrid: one image with BOTH a mono-stamped bijection site and a
/// frames-lowered holey site. The image is FRAMES (a frames site survives),
/// carries a frames region, AND a mono stamp.
#[test]
fn hybrid_mixes_a_stamp_and_a_frames_site() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine swap, tapes=1, alpha=(4)
.routine narrow, tapes=1, alpha=(2)
.section code
.func main
        call    swap [0{1->2, 2->1}]
        call    narrow [0{1=>0}]
        stp
.func swap
        wr [1]
        ret
.func narrow
        wr [1]
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], hybrid_opts()).expect("links");
    // swap is an equal-size bijection → mono stamp; narrow (alphabet 2 vs the
    // 4-symbol machine, holey) → frames.
    assert_eq!(
        out.executable.profile, PROFILE_FRAMES,
        "a frames site ⇒ FRAMES"
    );
    assert!(
        out.executable.frames_offset != 0,
        "hybrid emits a frames region for the frames site"
    );
    assert_eq!(
        stamp_names(&out).len(),
        1,
        "the bijection site is mono-stamped: {:?}",
        stamp_names(&out)
    );
    assert!(
        stamp_names(&out)[0].starts_with("swap$"),
        "the swap site (a bijection) is the stamp: {:?}",
        stamp_names(&out)
    );
}

/// A raw `call.m` inside an engine-composed routine keeps its constant
/// compose column — the hand-authored descriptor stays absolute, activated
/// regardless of the active frame (5a semantics preserved under nesting).
#[test]
fn a_raw_call_m_inside_a_composed_routine_has_a_constant_column() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine r, tapes=2, alpha=(4, 4)
.routine leaf, tapes=2, alpha=(4, 4)
.section tables
Fr: .frame  tapes=(0, 1)
    .map    0, rmap=(1->2)
.section code
.func main
        call    r [0{1->3, 3->1}, 1]
        stp
.func r
        fcall   leaf, Fr
        ret
.func leaf
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    let region = parse_region(&out.executable).expect("frames region");
    // Two directory entries: the engine composite E (main's bound call) and
    // the raw descriptor Fr. Two framed sites: main's bound call, r's
    // raw call.m.
    assert_eq!(region.k, 2);
    assert_eq!(region.s, 2);
    // Column 0 (main's bound call) activates E (composite 1) at identity.
    assert_eq!(region.compose[0][0], 1);
    // Column 1 (r's raw call.m) is CONSTANT = the raw descriptor's index (2)
    // in EVERY row — it still activates Fr when r runs under E (row 1).
    for row in 0..=2 {
        assert_eq!(
            region.compose[row][1], 2,
            "raw call.m column constant at row {row}"
        );
    }
}

/// A function that BOTH frames a bound call (widening 5→9 bytes) AND owns a
/// dispatch table whose target sits after the widened site: the engine's
/// blob rewrite must shift the dispatch entry's blob-relative code offset,
/// so it still lands on the target after layout rebases it.
#[test]
fn a_widened_site_shifts_a_later_dispatch_entry() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section tables
D:  .targets A
.section code
.func main
        call    sub [0{1->2, 2->1}, 1]
        tdispatch D
A:      stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    let exe = &out.executable;
    // Rewritten main: ent@0, fcall@1..10 (widened), tdispatch@10..15,
    // A: stp@15. `A` sits at absolute 15 (was blob-relative 11 before the
    // +4 widen). The dispatch table (main's, first in the section) is
    // `count(1) u16` then the entry `u32` = A's absolute address.
    assert_eq!(u16::from_le_bytes([exe.tables[0], exe.tables[1]]), 1);
    let entry = u32::from_le_bytes(exe.tables[2..6].try_into().unwrap());
    assert_eq!(entry, 15, "dispatch entry follows the widened site to A");
    // And A really is the stp at absolute 15.
    assert_eq!(exe.code[15], 0x02, "stp at A");
}
