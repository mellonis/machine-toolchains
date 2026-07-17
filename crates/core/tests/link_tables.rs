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
    // Exit vector is the descriptor's trailing two u32s: done=10, other=11.
    let tables = &exe.tables;
    let exits_at = tables.len() - 8;
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
fn a_reachable_bound_call_is_refused_naming_the_callee() {
    // `main` bound-calls `sub` (defined). Resolve reaches `sub`, then the
    // guard refuses the still-unsupported binding, naming the callee.
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
    assert_eq!(e, LinkError::UnsupportedBindings("sub".into()));
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
