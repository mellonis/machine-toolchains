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
    use Flow::{Branch, Call, FallThrough as FT, Stop};
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
            // A match-flag branch (`jm`-shape): RelI32 Branch to a local
            // label, unpaired (no short form, so it always takes the far
            // path — the mono guard's target, not a relaxation case).
            SyntaxEntry {
                opcode: 0x22,
                mnemonic: "jm",
                operand: OperandKind::RelI32,
                flow: Branch,
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
            volatile: false,
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

/// The old, deliberately-left defect: without a map, an unresolved
/// dispatch target rendered as raw hex plus a defensive comment, which
/// `.targets` refuses on reassembly — 24 of the round's 42 acceptance
/// combinations failed `dispatch targets are label names [bad-table]`
/// for exactly this reason, since `tmt link` on a non-`-g` object writes
/// an empty map `labels` list, so passing `--map` did not help either.
/// Every dispatch target now gets [`synthesized_label`]'s `L<addr>` name
/// instead — defined in the code section right below it — so the text
/// reassembles with no map at all.
#[test]
fn dis_of_a_linked_image_without_map_labels_assembles() {
    let syntax = fake_syntax();
    let out = link_one(asm(SINGLE, false));
    let text = disassemble_executable(&syntax, &out.executable, None);
    assert!(
        text.contains(".routine main, tapes=2, alpha=(3, 5)"),
        "{text}"
    );
    assert!(
        text.contains(".targets L000B, L000C"),
        "synthesized names, not raw hex, without a map:\n{text}"
    );
    assert!(
        !text.contains("0x000b") && !text.contains("unresolved"),
        "no raw-hex/defensive-comment fallback survives:\n{text}"
    );
    // The dispatch-reachable code is still discovered as instructions,
    // each labeled with the exact name `.targets` printed for it.
    assert!(text.contains("L000B:  nop"), "{text}");
    assert!(text.contains("L000C:  stp"), "{text}");
    assert!(!text.contains(".byte"), "{text}");
    let obj2 = assemble(&syntax, ARCH, &text, false).expect("rendered text re-assembles");
    let out2 = link(&syntax, &[obj2], &[], LinkOptions::default()).expect("re-links");
    assert_eq!(
        out2.executable.to_bytes(),
        out.executable.to_bytes(),
        "a mapless dis ∘ link must still reproduce the image byte-for-byte"
    );
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

/// Every operand name printed by a `.targets` or `.exits` line in `text`,
/// continuation lines included — a wrapped list's lines all end in `,`
/// except its last. Local copy of `disassembler.rs`'s test helper of the
/// same shape (per-file-helper convention: this file needs `link`, which
/// that module's test helper does not).
fn listed_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut continuing = false;
    for line in text.lines() {
        let trimmed = line.trim();
        let payload = if continuing {
            trimmed
        } else if let Some((_, rest)) = trimmed.split_once(".targets ") {
            rest
        } else if let Some((_, rest)) = trimmed.split_once(".exits ") {
            rest
        } else {
            continue;
        };
        continuing = payload.ends_with(',');
        names.extend(
            payload
                .split(',')
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string),
        );
    }
    names
}

/// Every name a `.targets`/`.exits` line prints is defined as a label in
/// the CODE section of the same text — independent of how the name was
/// chosen, so it catches a name with no definition, a raw offset, and a
/// position the code walk never reaches.
fn assert_listed_names_are_defined(dis: &str) {
    let (_, code) = dis
        .split_once("\n.section code\n")
        .unwrap_or_else(|| panic!("table-bearing disassembly has a code section:\n{dis}"));
    let listed = listed_names(dis);
    assert!(!listed.is_empty(), "fixture lists no names at all:\n{dis}");
    for name in listed {
        let def = format!("{name}:");
        assert!(
            code.lines().any(|l| l.starts_with(&def)),
            "`{name}` is printed by a list directive but defined nowhere \
             in the code section:\n{dis}"
        );
    }
}

/// A wide table section: a match table (so blank-line separation between
/// TWO tables is observable), a dispatch table wide enough to force
/// `.targets` to wrap, and a frame descriptor whose `.map`/`.exits` are
/// wide enough to force those too — assembled `-g` so the printed names
/// are real (long) source labels, not short synthesized ones, since a
/// short name would defeat the wrap. `fcall main, F0` calls `main`
/// itself (the `TWO_SITES`/`FRAMES` self-referential pattern elsewhere in
/// this file): the frame's exits are labels inside `main`'s own body, so
/// nothing here depends on a second function. It comes FIRST in the
/// body, reached by `ent`'s fall-through — `tdispatch` is a Stop-flow
/// mnemonic in this dialect, so anything placed after it is unreachable
/// dead code the recursive-descent walk would never decode.
fn wide_linked_fixture() -> String {
    let exits: Vec<String> = (0..14).map(|i| format!("Elongname{i}")).collect();
    let pairs: Vec<String> = (1..9).map(|i| format!("{i}->{}", i + 1)).collect();
    let body: String = exits
        .iter()
        .map(|e| format!("{e}: stp\n"))
        .collect::<Vec<_>>()
        .concat();
    format!(
        ".routine main, tapes=2, alpha=(4, 4)\n\
         .section tables\n\
         T0: .row [1, 2]\n\
         D0: .targets {}\n\
         F0: .frame tapes=(1, 0)\n    \
             .map 0, rmap=({}), wmap=({})\n    \
             .exits {}\n\
         .section code\n\
         .func main\n    \
             fcall   main, F0\n    \
             tmatch  T0\n    \
             tdispatch D0\n\
         {body}    stp\n",
        exits.join(", "),
        pairs.join(", "),
        pairs.join(", "),
        exits.join(", "),
    )
}

/// Defect 2: even WITH `-g` throughout, the executable table loop
/// neither wrapped a long list nor separated tables with a blank line —
/// a 1048-char `.targets` line and no blank between tables, so `tmt fmt
/// --check` failed. Deleting the raw-hex fallback (defect 1) also
/// deletes the section's only trailing comment, so `fmt --check` passing
/// is NOT by itself evidence the blank line is there — a section with
/// zero trailing comments never widens `comment_columns` regardless of
/// blank lines (the same caveat Task 4 recorded). This test asserts the
/// blank-line and wrap structure directly, then checks `fmt --check`
/// (`format_asm_with` is the identity) as a second, non-discriminating
/// confirmation.
#[test]
fn dis_of_a_linked_image_is_fmt_clean() {
    use mtc_core::asm::{AsmCaps, format_asm_with};
    let syntax = fake_syntax();
    let src = wide_linked_fixture();
    let out = link_one(asm(&src, true));
    let dis = disassemble_executable(&syntax, &out.executable, Some(&out.map));

    // Blank-line separation: three tables, no blank before the first,
    // exactly one before each of the other two, none wider. Table names
    // are synthesized in OUTPUT order (match/dispatch share one `T<n>`
    // counter, frame its own `F<n>`), not the source labels — the
    // dispatch table (source `D0`) renders as `T1`, right after match
    // table `T0`.
    assert!(
        dis.contains(".section tables\nT0:"),
        "no blank line before the first table:\n{dis}"
    );
    assert!(dis.contains("\n\nT1:"), "blank line before T1 (D0):\n{dis}");
    assert!(dis.contains("\n\nF0:"), "blank line before F0:\n{dis}");
    let section = dis.split_once("\n.section code\n").unwrap().0;
    assert_eq!(
        section.matches("\n\n").count(),
        2,
        "one blank line per boundary, no others:\n{dis}"
    );
    assert!(
        !section.contains("\n\n\n"),
        "no boundary carries more than one blank line:\n{dis}"
    );

    // Wrapping: each of `.targets`/`.exits`/`.map` actually wrapped, at
    // the same continuation column the printer would compute.
    for (word, col) in [(".targets", 17), (".exits", 16), (".map", 16)] {
        let line = dis
            .lines()
            .position(|l| l.contains(word))
            .unwrap_or_else(|| panic!("no {word} line in:\n{dis}"));
        let next = dis.lines().nth(line + 1).unwrap_or("");
        assert!(
            dis.lines().nth(line).unwrap().ends_with(','),
            "{word} should have wrapped:\n{dis}"
        );
        assert_eq!(
            next.len() - next.trim_start().len(),
            col,
            "{word}'s continuation lines indent under its first element:\n{dis}"
        );
    }
    for l in dis.lines() {
        assert!(l.chars().count() <= 80, "over-budget line `{l}`:\n{dis}");
    }

    let caps = AsmCaps {
        tables: true,
        rept: true,
        vectors: true,
        volatile: false,
    };
    assert_eq!(
        format_asm_with(&dis, caps).unwrap(),
        dis,
        "fmt --check clean"
    );
    assert_listed_names_are_defined(&dis);
    let obj2 = assemble(&syntax, ARCH, &dis, false).expect("rendered text re-assembles");
    let out2 = link(&syntax, &[obj2], &[], LinkOptions::default()).expect("re-links");
    assert_eq!(out2.executable.to_bytes(), out.executable.to_bytes());
}

/// Defect 3: `a5_call_across_alphabets` failed `bad-signature` even with
/// `-g`, because the executable path emitted `.routine` for the entry
/// only — but reassembling the disassembled text treats it as one
/// source file, and the assembler's all-or-none rule then demands every
/// function in it carry a signature once any one does. `helper`'s frame
/// projects a SMALLER arity (1 virtual tape) than `main`'s own (2), so a
/// fix that just replayed the entry's signature for every function would
/// print `tapes=2` for `helper` too — disagreeing with the `.frame
/// tapes=(1)` line already rendered for it, and this test would still
/// pass under that wrong fix if it only checked "some `.routine` line
/// exists". Asserting the exact `tapes=1` value is what rules that out.
#[test]
fn dis_of_a_linked_image_emits_routine_signatures() {
    let syntax = fake_syntax();
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine helper, tapes=1, alpha=(3)
.section tables
F0: .frame tapes=(1)
    .exits after
.section code
.func main
        fcall   helper, F0
after:  stp
.func helper
        ret
";
    let out = link_one(asm(src, true));
    let dis = disassemble_executable(&syntax, &out.executable, Some(&out.map));
    assert!(dis.contains(".frame  tapes=(1)"), "{dis}");
    // `helper`'s frame projects onto PHYSICAL tape 1, whose cardinality
    // (from `main`'s own `alpha=(4, 4)`) is 4 — the physical tape's
    // cardinality, not `helper`'s true one (3, declared in `src` above
    // and gone once linked): closed to the exact value, not left
    // open-ended, so a fix that emitted the entry's `tapes=2` alongside
    // SOME plausible-looking `alpha=(N)` could not slip past this. The
    // trailing `; derived` marks the line as not the routine's true
    // signature (docs/formats.md (assembly text)).
    assert!(
        dis.contains(".routine helper, tapes=1, alpha=(4) ; derived"),
        "the callee's arity comes from the frame, its alpha from the physical \
         tape, and the line is flagged `; derived`:\n{dis}"
    );
    assert!(
        !dis.contains(".routine helper, tapes=2"),
        "must not just replay the entry's signature:\n{dis}"
    );
    // The entry's OWN `.routine` line is exact — no `; derived` marker.
    assert!(
        dis.contains(".routine main, tapes=2, alpha=(4, 4)\n"),
        "the entry's signature carries no derived marker:\n{dis}"
    );
    let obj2 = assemble(&syntax, ARCH, &dis, false).expect("rendered text re-assembles");
    let out2 = link(&syntax, &[obj2], &[], LinkOptions::default()).expect("re-links");
    assert_eq!(
        out2.executable.to_bytes(),
        out.executable.to_bytes(),
        "dis ∘ link must reproduce the image byte-for-byte"
    );
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

/// A full-arity identity binding collapses to a plain call: no framed
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
/// pass-through and stays a framed call — the projection guard on the
/// identity-collapse rule (a 1-tape identity composite under a 2-tape caller).
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

/// An EMPTY binding into a NARROWER callee is NOT a pass-through: across the
/// unequal alphabets there is no identity completion, so every non-blank
/// caller symbol is a read hole. The site must stay a framed call and
/// materialize a holey descriptor — collapsing it would silently drop the
/// `UnmappedRead` trap (contrast `a_full_identity_binding_collapses_to_a_plain_call`,
/// equal-size).
#[test]
fn a_narrower_identity_binding_stays_a_framed_call() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub [0]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    assert_eq!(
        out.executable.profile, PROFILE_FRAMES,
        "the cardinality hole keeps the site framed"
    );
    let region = parse_region(&out.executable).expect("frames region present");
    assert_eq!(region.k, 1, "the holey composite is real");
    assert_eq!(region.s, 1, "the site stays a framed call");
    assert!(
        out.executable.code.contains(&0x14),
        "framed call emitted, not collapsed"
    );
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
// functions named `<callee>.<digest8>`.

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

/// True when `name` ends in a stamp's `.<digest8>` suffix: a period
/// followed by exactly 8 hex digits (`is_ascii_hexdigit`, so either case
/// matches — a minted stamp is always lowercase since `intern` formats
/// the digest via `{:08x}`, but the check itself doesn't require that). A
/// plain `.contains('.')` would also catch an ordinary dotted routine name
/// (the optimizer's `outline` pass mints `<name>.outline<N>`, for
/// instance), so the check matches the digest's exact shape rather than
/// merely the separator.
fn is_stamp_name(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, tail)) => tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_hexdigit()),
        None => false,
    }
}

/// The map functions whose name marks them a mono stamp (`<callee>.<hex>`).
fn stamp_names(out: &LinkOutput) -> Vec<String> {
    out.map
        .functions
        .iter()
        .filter(|f| is_stamp_name(&f.name))
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
        stamp_names(&out)[0].starts_with("sub."),
        "stamp named after the callee: {:?}",
        stamp_names(&out)
    );
}

/// `sub`'s only caller is the projecting bound call above, and mono
/// retargets that site to the stamp — nothing calls the generic `sub`
/// anymore. The reachability promise (docs/core.md (linking)) applies after
/// stamping exactly as it does before it, so the generic must not ship.
/// Checks the exact name (not a prefix — `sub.<digest8>` the stamp itself
/// starts with `sub.` and would make a `starts_with` check pass vacuously),
/// pairs the absence with a positive control (`main` present, exactly one
/// stamp present) so a fixture that silently failed to link or to stamp
/// couldn't pass this by accident, and closes with the strongest cheap pin:
/// nothing but `main` and stamps survives at all.
#[test]
fn a_generic_orphaned_by_stamping_is_not_shipped() {
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
    let names: Vec<&str> = out.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"main"),
        "the entry always survives: {names:?}"
    );
    assert_eq!(
        stamp_names(&out).len(),
        1,
        "the positive control: exactly one stamp of sub: {names:?}"
    );
    assert!(
        !names.contains(&"sub"),
        "the orphaned generic must not be in the map: {names:?}"
    );
    assert!(
        out.map
            .functions
            .iter()
            .all(|f| f.name == "main" || is_stamp_name(&f.name)),
        "nothing but main and stamps survives when every site to sub is stamped: {names:?}"
    );
    assert_eq!(
        out.report.dropped,
        vec!["sub".to_string()],
        "the orphan is accounted for in the report, not just silently missing from the map: {:?}",
        out.report.dropped
    );
}

/// The same shape, but `sub` is a LOCAL symbol (bound directly within its
/// own object, never through the namespace). `resolve::Resolved::dropped`
/// deliberately omits locals — a pre-lowering drop is name-level and
/// namespace-based, and a local was never a namespace candidate — but a
/// stamping orphan is a different kind of drop: it names an actual `FuncRef`
/// that WAS linked in, independent of whether that name was ever exported.
/// The merged `dropped` list is therefore slightly MORE inclusive than the
/// resolve-time-only wording once suggested: a local generic CAN appear
/// here if stamping orphans it. `dead` is the actual contrast case: a LOCAL
/// the reachability BFS never reaches at all (called by nothing, not even
/// transitively) — `resolve::Resolved::dropped` silently omits it by
/// design, and this asserts it stays omitted from the merged list too, so
/// the fixture exercises both halves of the asymmetry the doc comment
/// claims, not just the orphan half.
#[test]
fn a_local_generic_orphaned_by_stamping_is_reported_dropped() {
    let src = "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.routine dead, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub [0{1->2, 2->1}, 1]
        stp
.func sub local
        wr [1, -]
        ret
.func dead local
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], mono_opts()).expect("links");
    assert_eq!(
        stamp_names(&out).len(),
        1,
        "the positive control: exactly one stamp of sub: {:?}",
        out.map
    );
    assert_eq!(
        out.report.dropped,
        vec!["sub".to_string()],
        "a local orphan is reported, but an unreached local (dead) stays silently omitted \
         exactly as resolve documents: {:?}",
        out.report.dropped
    );
}

/// Two SEPARATE objects, each defining its own private `helper` reached only
/// through a projecting bound call (so mono stamps and orphans it, same as
/// the single-object tests above) — but this time both locals share the
/// exact name `helper`. Locals never enter the shared namespace (per-object
/// visibility), so the assembler happily accepts the same name twice across
/// objects; before the merged `dropped` list existed this was moot, but
/// `resolved.dropped ∪ orphaned` combining two SORTED, UNIQUE-by-construction
/// lists does not itself produce a unique result — `resolved.dropped` came
/// from a `BTreeSet` (pre-branch, always unique), but `orphaned` is built by
/// filtering `order`, which CAN hold two distinct `FuncRef`s sharing a name
/// (`resolve.rs`'s `locals_bind_directly_and_may_repeat_across_objects`
/// pins exactly this). Without `.dedup()` after the sort this prints
/// `dropped [helper, helper]` — a duplicate that never occurred before this
/// branch, since the pre-branch field was `resolved.dropped` verbatim.
/// Object B's binding differs from A's (`1->3, 3->1` vs `1->2, 2->1`) so the
/// two composites — and so the two minted stamp names — stay distinct;
/// otherwise `intern`'s collision guard would refuse the link before the
/// duplicate-name question is even reached.
#[test]
fn dropped_deduplicates_two_same_named_locals_orphaned_in_different_objects() {
    let object_a = "\
.routine main, tapes=2, alpha=(4, 4)
.routine helper, tapes=2, alpha=(4, 4)
.section code
.func main
        call    helper [0{1->2, 2->1}, 1]
        call    apiB
        stp
.func helper local
        wr [1, -]
        ret
";
    let object_b = "\
.routine apiB, tapes=2, alpha=(4, 4)
.routine helper, tapes=2, alpha=(4, 4)
.section code
.func apiB
        call    helper [0{1->3, 3->1}, 1]
        ret
.func helper local
        wr [1, -]
        ret
";
    let out = link(
        &fake_syntax(),
        &[asm(object_a, false), asm(object_b, false)],
        &[],
        mono_opts(),
    )
    .expect("links");
    assert_eq!(
        stamp_names(&out).len(),
        2,
        "the positive control: one stamp per object's helper: {:?}",
        out.map
    );
    assert_eq!(
        out.report.dropped,
        vec!["helper".to_string()],
        "two distinct orphans sharing one name collapse to a single report entry, not \
         [helper, helper]: {:?}",
        out.report.dropped
    );
}

/// A hand-written routine that occupies the exact name a mono stamp would
/// mint is a link error, not a silent identity collision — the production
/// path, not just `intern`'s unit tests, must seed its collision guard from
/// every routine already reached. Two-phase and digest-free by
/// construction: phase one links the plain fixture to learn the REAL stamp
/// name the linker would mint (so nothing here hardcodes a digest that a
/// `canonical_key` change could silently invalidate); phase two re-links
/// the identical binding with an extra routine already sitting on that
/// exact name and asserts the refusal. The decoy is reached through a
/// PLAIN call from `sub`'s own body rather than from `main` — reachability
/// resolves every relocation before any bound call
/// (docs/core.md (name resolution)), so a decoy called from `main` would be
/// discovered ahead of `sub` and shift `sub`'s resolved index (and so its
/// digest), invalidating the very name phase one learned; calling it from
/// inside `sub` keeps `sub`'s index — and therefore its digest — identical
/// across both links.
#[test]
fn a_stamp_name_colliding_with_a_hand_written_routine_is_a_link_error() {
    let base = "\
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
    let out = link(&fake_syntax(), &[asm(base, false)], &[], mono_opts()).expect("links");
    let names = stamp_names(&out);
    assert_eq!(names.len(), 1, "exactly one stamp: {:?}", out.map);
    let stamp = names.into_iter().next().unwrap();

    let clash = format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.routine {stamp}, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub [0{{1->2, 2->1}}, 1]
        stp
.func sub
        wr [1, -]
        call    {stamp}
        ret
.func {stamp}
        ret
"
    );
    let err = link(&fake_syntax(), &[asm(&clash, false)], &[], mono_opts())
        .expect_err("the reserved name is already taken by a hand-written routine");
    assert_eq!(err, LinkError::StampNameCollision(stamp));
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
/// routine — no stamp, no frames. `sub` stays main's only callee, so
/// `prune_unreachable` finds everything already reached and takes its
/// documented fast path: the input `Vec` comes back untouched, not a
/// filtered copy that merely happens to contain the same functions. That
/// claim isn't exercised by the frames/hybrid byte-identity checks, which
/// never call the prune at all — this is the one mono fixture where the
/// prune runs AND must find nothing to drop, so it pins both the fast path
/// and the not-too-aggressive direction: `sub` present in the map, and the
/// report's `dropped` list empty (an over-eager prune would show up here
/// first, since `sub` is the collapse target, not a stamp).
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
    let names: Vec<&str> = out.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"sub"),
        "the collapse target survives; the prune must find nothing to drop: {names:?}"
    );
    assert!(
        out.report.dropped.is_empty(),
        "nothing is orphaned here — an over-aggressive prune would show up in this list: {:?}",
        out.report.dropped
    );
}

/// An EMPTY binding into a NARROWER callee does NOT collapse under mono
/// either: across the unequal alphabets there is no identity completion, so
/// every non-blank caller symbol is a read hole. The site is stamped (not
/// lowered to the original) and the stamp gains synthesized unmapped-read trap
/// rows. Collapsing to the original would let those symbols flow into `sub`
/// raw and miss the trap (contrast
/// `an_identity_binding_under_mono_calls_the_original`, equal-size).
#[test]
fn a_narrower_identity_binding_under_mono_stamps_with_trap_rows() {
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
        call    sub [0]
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
    assert_ne!(
        out.executable.profile, PROFILE_FRAMES,
        "mono ⇒ base profile"
    );
    assert_eq!(
        stamp_names(&out).len(),
        1,
        "the narrower callee is stamped, not collapsed to the original: {:?}",
        stamp_names(&out)
    );
    assert!(
        stamp_names(&out)[0].starts_with("sub."),
        "stamp named after the callee: {:?}",
        stamp_names(&out)
    );
    assert!(
        out.report.synthesized_trap_rows >= 1,
        "the read holes (every non-blank caller symbol) synthesize unmapped-read trap rows: {}",
        out.report.synthesized_trap_rows
    );
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
    // The advice recommends the mechanism that actually works. `hybrid`
    // hits this same refusal whenever no other bound site forces the
    // frames path (see the hybrid probe below), so it is no longer offered
    // as an escape.
    let msg = e.to_string();
    assert!(
        msg.contains("--call-mech=frames"),
        "advises the mechanism that works: {msg}"
    );
    assert!(
        !msg.contains("hybrid"),
        "must not send the caller in a circle: {msg}"
    );
}

/// The same raw `call.m` site, but the only bound call alongside it is a
/// completed bijection — hybrid's mono-or-frames-or-mixed classifier (see
/// `docs/core.md`, the composition engine) finds no holey/one-way site to
/// force the frames path and delegates wholesale to mono, hitting the same
/// contradiction. This is the common failure shape the reworded advice
/// targets: telling the caller to retry with `hybrid` sends them in a
/// circle.
#[test]
fn a_raw_call_m_under_hybrid_with_only_bijection_sites_is_the_same_link_error() {
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
    let e = link(&fake_syntax(), &[asm(src, false)], &[], hybrid_opts()).unwrap_err();
    assert_eq!(
        e,
        LinkError::MonoRawFrame("main".into()),
        "no holey/one-way site to force frames ⇒ hybrid delegates to mono wholesale"
    );
}

/// Same raw `call.m` site once more, under `frames`: no mono stamping is
/// ever attempted, so the descriptor's compose machinery is present and the
/// link succeeds. This is the mechanism the advice should point at.
#[test]
fn a_raw_call_m_under_frames_links() {
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
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    assert_eq!(out.executable.profile, PROFILE_FRAMES);
}

/// The hand-derived read-table rewrite: a machine-width match table with
/// synthesized trap rows PREPENDED, a collapse expanding one row into two,
/// and a no-preimage row DROPPED with the paired dispatch renumbered.
/// `main` (1 tape, alphabet 4) mono-calls `sub` (1 tape, alphabet 3) binding
/// physical 1 (two-way, so virtual 1 writes back) and physical 2 (one-way)
/// both onto virtual 1 (a collapse); physical 3 is unlisted, so across the
/// unequal alphabets it has no virtual image (a read hole).
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
        call    sub [0{1->1, 2=>1}]
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
        .find(|f| f.name.starts_with("sub."))
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

/// A holey mono binding synthesizes unmapped-read trap rows into the callee's
/// match table, but only a dispatch jump can route them to the trap stub.
/// When the callee reads the match result through a conditional branch
/// instead (no dispatch), a hole symbol would match a prepended trap row and
/// take the branch as if it had matched — a silent misroute. The stamp is
/// refused with a clear link error rather than emitted.
#[test]
fn a_holey_mono_binding_with_a_branch_fed_match_table_is_a_link_error() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine sub, tapes=1, alpha=(3)
.section tables
T0: .row [0]
    .row [1]
.section code
.func main
        call    sub [0{1=>1, 2=>1}]
        stp
.func sub
        rd
        tmatch  T0
        jm      L
        ret
L:      ret
";
    // physical 3 has no virtual image → a read hole → a trap row synthesized
    // into T0; `jm` reads the match result, so no dispatch consumes it.
    let e = link(&fake_syntax(), &[asm(src, false)], &[], mono_opts()).unwrap_err();
    assert_eq!(e, LinkError::MonoHoleyMatchBranch("sub".into()));
    // The advice recommends the mechanism that works unconditionally: a
    // nested holey bound call under an outer bijection seed (see the hybrid
    // probe below) hits this same refusal under hybrid too, so hybrid is no
    // longer offered as an escape.
    let msg = e.to_string();
    assert!(
        msg.contains("--call-mech=frames"),
        "advises the mechanism that works: {msg}"
    );
    assert!(
        !msg.contains("hybrid"),
        "must not send the caller in a circle: {msg}"
    );
}

/// The same misroute, mid-body: a trap-bearing match table feeds a branch,
/// then a LATER match table feeds a dispatch. An end-of-body-only guard would
/// see the second table's remap consumed and miss the first — the per-table
/// guard refuses the stamp when the second `tmatch` would overwrite the first
/// table's still-pending trap-bearing remap.
#[test]
fn a_holey_mono_binding_with_an_unconsumed_earlier_match_table_is_a_link_error() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine sub, tapes=1, alpha=(3)
.section tables
T0: .row [0]
T1: .row [0]
    .row [1]
D1: .targets A, B
.section code
.func main
        call    sub [0{1=>1, 2=>1}]
        stp
.func sub
        rd
        tmatch  T0
        jm      M
        nop
M:      rd
        tmatch  T1
        tdispatch D1
A:      ret
B:      ret
";
    let e = link(&fake_syntax(), &[asm(src, false)], &[], mono_opts()).unwrap_err();
    assert_eq!(e, LinkError::MonoHoleyMatchBranch("sub".into()));
}

/// The FIRST world: when the holey, branch-fed site is itself the top-level
/// bound call (no bijection anywhere), hybrid's classifier sees it is not a
/// completed bijection and routes it to frames directly — it never attempts
/// a mono stamp for `sub` at all, so this specific shape never raises
/// `MonoHoleyMatchBranch` under hybrid. Same source as
/// `a_holey_mono_binding_with_a_branch_fed_match_table_is_a_link_error`, one
/// mechanism over.
#[test]
fn a_top_level_holey_branch_fed_site_is_frames_lowered_under_hybrid() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine sub, tapes=1, alpha=(3)
.section tables
T0: .row [0]
    .row [1]
.section code
.func main
        call    sub [0{1=>1, 2=>1}]
        stp
.func sub
        rd
        tmatch  T0
        jm      L
        ret
L:      ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], hybrid_opts()).expect("links");
    assert_eq!(
        out.executable.profile, PROFILE_FRAMES,
        "the holey top-level site is not a bijection ⇒ frames, no mono attempt"
    );
}

/// The SECOND world: the holeyness sits one hop DEEPER than the classifier
/// looks. `main` reaches `swap` through a completed bijection (equal
/// cardinalities, no one-way pair) — hybrid's classifier inspects only
/// `main`'s own bound sites, finds this one mono-eligible, and (with no
/// other top-level site to force frames) delegates the whole link to mono
/// via the `!any_frames` fast path. `swap` itself then makes a NESTED bound
/// call into `narrow` with a narrowing, branch-fed binding — invisible to
/// the top-level classifier, but reached all the same by the mono stamp
/// closure, which raises the identical refusal. This is the shape the
/// reworded advice targets (mirrors the `MonoRawFrame` probe): hybrid is not
/// an escape whenever the offending site's mono-stamped ancestry starts at
/// a bijection.
#[test]
fn a_nested_holey_bound_call_under_a_bijection_seed_is_the_same_link_error_under_hybrid() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine swap, tapes=1, alpha=(4)
.routine narrow, tapes=1, alpha=(3)
.section tables
T0: .row [0]
    .row [1]
.section code
.func main
        call    swap [0{1->2, 2->1}]
        stp
.func swap
        call    narrow [0{1=>1, 2=>1}]
        ret
.func narrow
        rd
        tmatch  T0
        jm      L
        ret
L:      ret
";
    let e = link(&fake_syntax(), &[asm(src, false)], &[], hybrid_opts()).unwrap_err();
    assert_eq!(
        e,
        LinkError::MonoHoleyMatchBranch("narrow".into()),
        "the outer bijection commits the closure to mono before the nested holeyness is ever seen"
    );
}

/// Same nested shape once more, under `frames`: no mono stamping is ever
/// attempted, so the descriptor path's hole handling applies uniformly and
/// the link succeeds. This is the mechanism the advice should point at.
#[test]
fn a_nested_holey_bound_call_under_a_bijection_seed_links_under_frames() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine swap, tapes=1, alpha=(4)
.routine narrow, tapes=1, alpha=(3)
.section tables
T0: .row [0]
    .row [1]
.section code
.func main
        call    swap [0{1->2, 2->1}]
        stp
.func swap
        call    narrow [0{1=>1, 2=>1}]
        ret
.func narrow
        rd
        tmatch  T0
        jm      L
        ret
L:      ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    assert_eq!(out.executable.profile, PROFILE_FRAMES);
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
        stamp_names(&out)[0].starts_with("swap."),
        "the swap site (a bijection) is the stamp: {:?}",
        stamp_names(&out)
    );
}

/// The same mixed fixture, checked from the map side: `swap`'s only site is
/// the bijection that mono promotes to a stamp, so the generic `swap` must
/// not ship — but `narrow`'s only site stays a framed call (holey), which
/// keeps the generic `narrow` reachable and it must still ship as itself.
/// The hybrid mixed path runs its own `prune_unreachable` call, separate
/// from `lower_mono`'s (`lower_hybrid` inlines the mono retarget rather than
/// calling `lower_mono` when both a stamp and a frames site are present), so
/// this exercises that call specifically rather than relying on the pure
/// mono test to stand in for it.
#[test]
fn hybrid_mixed_drops_an_orphaned_mono_generic_but_keeps_a_frames_generic() {
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
    let names: Vec<&str> = out.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"main"),
        "the entry always survives: {names:?}"
    );
    assert_eq!(
        stamp_names(&out).len(),
        1,
        "the positive control: exactly one stamp of swap: {names:?}"
    );
    assert!(
        names.contains(&"narrow"),
        "narrow's only site stays framed, so its generic stays reachable: {names:?}"
    );
    assert!(
        !names.contains(&"swap"),
        "swap's only site was promoted to a stamp; the orphaned generic must not ship: {names:?}"
    );
    assert_eq!(
        out.report.dropped,
        vec!["swap".to_string()],
        "narrow survives and must not appear here; only the orphan does: {:?}",
        out.report.dropped
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

// -- The link report counters + the map sidecar bindings (phase 5b, T6) --

/// FRAMES mode fills `composites` and `compose_table_bytes` (the compose
/// matrix, `(K+1) × S × 2`) and leaves the mono-only counters zero. The
/// two-context program has K=4, S=3 → 4 composites, 30 matrix bytes.
#[test]
fn frames_report_counts_composites_and_compose_bytes() {
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
    let r = &out.report;
    assert_eq!(r.composites, 4, "K = 4 directory entries");
    assert_eq!(
        r.compose_table_bytes,
        (4 + 1) * 3 * 2,
        "compose matrix bytes"
    );
    assert_eq!(r.instantiations, 0, "no mono stamps in frames mode");
    assert_eq!(r.dedup_savings, 0, "all four composites distinct");
    assert_eq!(r.synthesized_trap_rows, 0);
    assert_eq!(r.expanded_rows, 0);
}

/// FRAMES descriptor interning: two sites binding the same callee the same
/// way share one directory entry, and the second interning is a dedup saving.
#[test]
fn frames_report_counts_descriptor_dedup() {
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
    let r = &out.report;
    assert_eq!(r.composites, 1, "one deduped composite");
    assert_eq!(r.compose_table_bytes, (1 + 1) * 2 * 2, "K=1, S=2");
    assert_eq!(r.dedup_savings, 1, "the second equal composite is deduped");
    assert_eq!(r.instantiations, 0);
}

/// MONO stamping fills `instantiations`, `synthesized_trap_rows`, and
/// `expanded_rows` and leaves the frames counters zero. The hand-derived
/// program stamps one copy of `sub` with one read-hole trap row and one
/// one-way collapse expanding a row (the same shape `mono_read_table_rewrite_
/// is_byte_derived` proves byte-for-byte).
#[test]
fn mono_report_counts_stamps_traps_and_expansions() {
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
    let r = &out.report;
    assert_eq!(r.instantiations, 1, "one stamp of sub");
    assert_eq!(r.synthesized_trap_rows, 1, "physical 3 is a read hole");
    assert_eq!(r.expanded_rows, 1, "the one-way collapse expands one row");
    assert_eq!(r.composites, 0, "mono stays on the base profile");
    assert_eq!(r.compose_table_bytes, 0);
    assert_eq!(r.dedup_savings, 0);
}

/// HYBRID accounts for BOTH mechanisms in one report: the bijection site
/// mono-stamps (a `instantiation`) and the holey site is frames-lowered (a
/// `composite`).
#[test]
fn hybrid_report_counts_both_mechanisms() {
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
    let r = &out.report;
    assert_eq!(r.instantiations, 1, "the swap bijection is stamped");
    assert_eq!(
        r.composites, 1,
        "the narrow holey site is a frames composite"
    );
    assert_eq!(r.compose_table_bytes, 4, "K=1, S=1 → (1+1)×1×2");
}

/// The map sidecar records each composite structurally, with the canonical
/// label. A single equal-size bijection binding: one record naming the
/// callee, its label the swapped pairs, its structured pairs decoded.
#[test]
fn sidecar_records_the_composite_binding() {
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
    assert_eq!(out.map.bindings.len(), 1);
    let b = &out.map.bindings[0];
    assert_eq!(b.index, 1);
    assert_eq!(b.routine, "sub");
    assert_eq!(b.label, "sub@[0{1->2,2->1},1]");
    assert_eq!(b.tapes.len(), 2);
    assert_eq!(b.tapes[0].phys, 0);
    assert_eq!(b.tapes[0].pairs, vec![(1, 2, false), (2, 1, false)]);
    assert!(b.tapes[0].read_holes.is_empty());
    assert_eq!(b.tapes[1].phys, 1);
    assert!(b.tapes[1].pairs.is_empty(), "the passthrough tape is bare");
}

/// A hand-authored (5a) raw-descriptor image gets binding records too, decoded
/// from the descriptor bytes — and two identical labels collide, so the second
/// is disambiguated `.2`.
#[test]
fn sidecar_records_raw_descriptors_with_collision_suffix() {
    let out = link_one(asm(TWO_SITES, false));
    assert_eq!(out.map.bindings.len(), 2, "two raw descriptors");
    assert_eq!(out.map.bindings[0].index, 1);
    assert_eq!(out.map.bindings[1].index, 2);
    assert_eq!(out.map.bindings[0].routine, "main");
    assert_eq!(out.map.bindings[1].routine, "main");
    // Both descriptors are the identity `tapes=(0)`, so the labels collide and
    // the second gets the deterministic `.2` suffix.
    assert_eq!(out.map.bindings[0].label, "main@[0]");
    assert_eq!(out.map.bindings[1].label, "main@[0].2");
}

/// A frameless link carries no binding records — the sidecar's `bindings` is
/// empty and (with `skip_serializing_if`) absent from the JSON entirely.
#[test]
fn frameless_link_has_no_bindings() {
    // SINGLE is signed and tabled but frameless: no directory, no bindings.
    let out = link_one(asm(SINGLE, false));
    assert!(out.map.bindings.is_empty());
    assert!(!out.map.to_json().contains("bindings"));
}

/// The dis frames legend WITHOUT a map: composites are named from the image
/// alone (descriptor decode + the site callees), and two identical labels
/// collide, so the second is `.2` — the same disambiguation the sidecar uses.
/// Legend composites carry the `C<i>` prefix (directory index, 1-based),
/// distinct from the code section's `F<n>` frame-descriptor labels (tables
/// order, 0-based): with two composites the two numberings diverge, so the
/// legend's `C1`/`C2` and the code's `F0`/`F1` coexist without ambiguity.
#[test]
fn dis_legend_names_raw_composites_from_the_image() {
    let out = link_one(asm(TWO_SITES, false));
    // No map: labels derived from the descriptor bytes + the call.m callees.
    let text = disassemble_executable(&fake_syntax(), &out.executable, None);
    assert!(
        text.contains("; frames: 2 composite(s), 2 site(s)"),
        "legend header:\n{text}"
    );
    assert!(text.contains(";   C1: main@[0]"), "composite 1:\n{text}");
    assert!(
        text.contains(";   C2: main@[0].2"),
        "collision suffix:\n{text}"
    );
    // The code section keeps the 0-based `F<n>` table labels — the same string
    // family the legend deliberately avoids. `C1` (legend, composite 1) and
    // `F0` (code, first descriptor) name the SAME descriptor here; the two
    // prefixes keep them unambiguous where the old shared-`F` scheme collided.
    assert!(
        text.contains("fcall   main, F0"),
        "code site 0 → F0:\n{text}"
    );
    assert!(
        text.contains("fcall   main, F1"),
        "code site 1 → F1:\n{text}"
    );
    assert!(
        !text.contains(";   F"),
        "legend uses C, not F, for composites:\n{text}"
    );
    // Both sites are constant, so no site summary line.
    assert!(
        !text.contains(";   site"),
        "no context-dependent site:\n{text}"
    );
}

/// The dis frames legend WITH a map uses the sidecar's binding labels; a
/// single equal-size bijection site is constant, named `F1` inline and in the
/// legend.
#[test]
fn dis_legend_uses_map_binding_labels() {
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
    let text = disassemble_executable(&fake_syntax(), &out.executable, Some(&out.map));
    assert!(
        text.contains("; frames: 1 composite(s), 1 site(s)"),
        "legend header:\n{text}"
    );
    assert!(
        text.contains(";   C1: sub@[0{1->2,2->1},1]"),
        "map-labeled composite:\n{text}"
    );
    // The one site is constant → rendered by its F-label inline (F0, the
    // tables-section descriptor label), not `@site`. Legend `C1` and code `F0`
    // name the same descriptor under distinct prefixes.
    assert!(
        text.contains("fcall   sub, F0"),
        "constant site inline:\n{text}"
    );
    assert!(
        !text.contains("@site"),
        "no context-dependent site:\n{text}"
    );
}

/// A context-dependent site (reached under two composites) renders `@site<N>`
/// in the code and gets a legend summary of the composites it can select.
#[test]
fn dis_legend_summarizes_a_context_dependent_site() {
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
    let text = disassemble_executable(&fake_syntax(), &out.executable, Some(&out.map));
    // r's call.m to q (site 2) resolves to composite 3 under E1 and 4 under
    // E2 — context-dependent, so it renders `@site2`.
    assert!(
        text.contains("fcall   q, @site2"),
        "non-constant site:\n{text}"
    );
    assert!(
        text.contains(";   site2: [C3, C4]"),
        "site summary lists both composites:\n{text}"
    );
    // Four composites in the legend, one per directory entry.
    assert!(
        text.contains("; frames: 4 composite(s), 3 site(s)"),
        "{text}"
    );
    for c in ["C1", "C2", "C3", "C4"] {
        assert!(text.contains(&format!(";   {c}: ")), "{c} listed:\n{text}");
    }
}

/// The exit vector of a descriptor at `off` in `tables` (docs/formats.md
/// (frame descriptors)): arity, exit count, per-tape phys + length-prefixed
/// rmap/wmap, then the `exit_count` absolute-address u32s.
fn descriptor_exits(tables: &[u8], off: u32) -> Vec<u32> {
    let mut p = off as usize;
    let arity = tables[p];
    p += 1;
    let exit_count = u16::from_le_bytes([tables[p], tables[p + 1]]) as usize;
    p += 2;
    for _ in 0..arity {
        p += 1; // phys
        let rlen = u16::from_le_bytes([tables[p], tables[p + 1]]) as usize;
        p += 2 + 2 * rlen;
        let wlen = u16::from_le_bytes([tables[p], tables[p + 1]]) as usize;
        p += 2 + 2 * wlen;
    }
    (0..exit_count)
        .map(|i| u32::from_le_bytes(tables[p + 4 * i..p + 4 * i + 4].try_into().unwrap()))
        .collect()
}

/// Exit vectors follow a WIDENED site, not just a relaxation shift: a raw
/// `.frame`'s exit label sits AFTER a declarative bound call in the same
/// function. In FRAMES mode the engine widens that bound call 5 → 9 bytes, so
/// the exit's code address must shift +4 through the same offset map that
/// carries jumps and dispatch entries (docs/formats.md (frames region)). The
/// dual of `frame_exits_follow_a_relaxation_shift`, on the engine's blob
/// rewrite rather than the linker's relaxation.
#[test]
fn frame_exits_follow_an_engine_widened_site() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine helper, tapes=1, alpha=(4)
.section tables
F0: .frame tapes=(0)
    .exits A
.section code
.func main
        fcall   main, F0
        call    helper [0{1->2, 2->1}]
A:      stp
.func helper
        ret
";
    let obj = asm(src, false);
    // Object: ent@0, fcall@1..10, far bound call@10..15, A: stp@15. F0's exit
    // is the label A at blob offset 15.
    assert_eq!(
        descriptor_exits(&obj.table_blobs.as_ref().unwrap()[0], 0),
        vec![15]
    );
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    let exe = &out.executable;
    assert_eq!(exe.profile, PROFILE_FRAMES);
    // The bound call widened to a 9-byte framed call, pushing A from 15 to 19
    // (main is the entry at 0, so blob-relative == absolute). F0 is the only
    // descriptor carrying an exit vector (engine composites have none), so
    // find it in the directory and read its shifted exit.
    let region = parse_region(exe).expect("frames region");
    let f0_exits = region
        .directory
        .iter()
        .map(|&off| descriptor_exits(&exe.tables, off))
        .find(|ex| !ex.is_empty())
        .expect("F0's exit vector is in the directory");
    assert_eq!(f0_exits, vec![19], "the exit followed the +4 widening");
}

/// Cross-path descriptor dedup isolation: the SAME (routine, composite) pair
/// reached by two DIFFERENT call chains is ONE directory entry — not one per
/// path. `main` plain-calls both `A` and `B` (identity context); each binds
/// `S` the same way, so `S` is reached under one composite via two chains. The
/// closure must intern it once (docs/core.md (the composition engine)) — distinct
/// from `frames_report_counts_descriptor_dedup`, whose two sites live in one
/// function.
#[test]
fn a_composite_reached_by_two_chains_is_one_directory_entry() {
    let src = "\
.routine main, tapes=1, alpha=(4)
.routine A, tapes=1, alpha=(4)
.routine B, tapes=1, alpha=(4)
.routine S, tapes=1, alpha=(4)
.section code
.func main
        call    A
        call    B
        stp
.func A
        call    S [0{1->2, 2->1}]
        ret
.func B
        call    S [0{1->2, 2->1}]
        ret
.func S
        ret
";
    let out = link(&fake_syntax(), &[asm(src, false)], &[], frames_opts()).expect("links");
    // A and B are plain calls (no composite of their own); only S's shared
    // composite frames. Two framed sites (A's and B's `call S`), ONE composite.
    assert_eq!(
        out.report.composites, 1,
        "the shared composite is interned once"
    );
    let region = parse_region(&out.executable).expect("frames region");
    assert_eq!(region.k, 1, "one directory entry");
    assert_eq!(region.s, 2, "two sites feed it");
    assert_eq!(region.directory.len(), 1);
    // Both sites, reached under the identity frame (A and B inherit it),
    // resolve to composite 1.
    assert_eq!(region.compose[0], vec![1, 1]);
    // The sidecar carries exactly one binding record, for routine S.
    assert_eq!(out.map.bindings.len(), 1);
    assert_eq!(out.map.bindings[0].routine, "S");
}
