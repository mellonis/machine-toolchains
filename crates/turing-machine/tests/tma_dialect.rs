//! The TM-1 `.tma` dialect: every mnemonic assembles and dis-round-trips,
//! a sectioned multi-tape program links and renders both sections, and the
//! canonical-grid formatter is idempotent on it. Mirrors the shape of the
//! core `link_tables.rs` framework tests, but exercises the real TM-1
//! dialect (`tm1_syntax`) end to end.

use mtc_core::linker::{LinkOptions, link};
use mtc_turing_machine::asm::{
    TM1_TMA_DIALECT_VERSION, assemble, disassemble_executable_with_map, disassemble_object,
    tm1_syntax,
};

/// Every source mnemonic EXCEPT `djmp`, across two signed functions with a
/// match table. `djmp`'s dispatch targets resolve only through the link
/// map, so `djmp` round-trips at the executable level (see [`SINGLE`]), not
/// the object level — a dispatch table carries blob-relative offsets, and
/// the object has no unified label map to name them back consistently.
/// `ent` rides in implicitly via `.func`; `call.s` is linker-only.
const OBJECT_MNEMONICS: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.routine helper, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, 1]
    .row [*, *]
.section code
.func main
        rd
        mtc  T0
        jm   L1
        jnm  L2
        brk
        nop
L1:     wr   [1, -]
        mov  [>, .]
        jmp  done
L2:     call helper
done:   hlt
        stp
.func helper
        wr   [-, 0]
        mov  [., <]
        ret
";

/// A single-function sectioned two-tape program with a match table AND a
/// dispatch table (`djmp`). One function so its `.routine` signature is
/// the entry's — the only signature the executable header preserves — and
/// the executable-level disassembly round-trips byte-for-byte.
const SINGLE: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, 1]
    .row [*, *]
D0: .targets hit, miss
.section code
.func main
        rd
        mtc  T0
        djmp D0
hit:    wr   [1, -]
        mov  [>, <]
        stp
miss:   hlt
";

/// The full 0.2 frames surface: a hand-authored `.frame` descriptor
/// (`.frame`/`.map`/`.exits`) activated by the framed call `call.m target,
/// F`, plus `trap #kind` and the multi-exit return `retx #k`. `main`
/// projects its two tapes to a narrower routine through F0 (virtual (0,1)
/// → physical (1,0), a non-identity rmap with a `->`, a one-way `=>`, and a
/// hole) and returns through two exits. Post-T5 a `call.m` must name a
/// `.frame` descriptor, never a match table.
const FRAMES_MNEMONICS: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.routine helper, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, 1]
    .row [*, *]
F0: .frame tapes=(1, 0)
    .map 0, rmap=(1->1, 3=>1)
    .exits done, other
.section code
.func main
        rd
        mtc     T0
        trap    #0
        call.m  helper, F0
done:   stp
other:  hlt
.func helper
        wr      [1, -]
        retx    #1
";

#[test]
fn dialect_version_is_0_2() {
    assert_eq!(TM1_TMA_DIALECT_VERSION, "0.2");
}

#[test]
fn frames_program_assembles_links_and_dis_renders_the_surface() {
    // The full frames surface assembles, and the object-level disassembler
    // renders it back: `.frame`/`.map`/`.exits`, `call.m`, `trap`, `retx`.
    let obj = assemble(FRAMES_MNEMONICS, true).expect("frames program assembles");
    let text = disassemble_object(&obj);
    for needle in [
        ".frame  tapes=(1, 0)",
        ".map    0, rmap=(1->1, 3->1)",
        ".exits  done, other",
        "call.m  helper, F0",
        "trap    #0",
        "retx    #1",
    ] {
        assert!(text.contains(needle), "dis missing `{needle}`:\n{text}");
    }
    // It links to a frames-profile image (runtime is the T7 milestone).
    let out =
        link(&tm1_syntax(), &[obj], &[], LinkOptions::default()).expect("frames program links");
    assert_eq!(
        out.executable.profile,
        mtc_core::formats::PROFILE_FRAMES,
        "a frame descriptor + a framed call ⇒ PROFILE_FRAMES"
    );
    assert_eq!(out.executable.tape_count, 2);
}

#[test]
fn every_mnemonic_assembles_and_object_round_trips() {
    // assemble ∘ dis ∘ assemble is a fixpoint at the object-byte level:
    // the rendered disassembly re-assembles to the identical object.
    // Assembled without `-g`: internal labels are resolved at assembly and
    // never stored, so the disassembler's synthesized `Lxxxx` names carry
    // no debug bytes to diverge on — the fixpoint is exact. (With `-g` the
    // original label *names* would survive in one object but be renamed to
    // address-synthesized ones in the round-tripped object.)
    let obj1 = assemble(OBJECT_MNEMONICS, false).expect("program assembles");
    let text = disassemble_object(&obj1);
    // Sanity: the rendered text names every covered mnemonic, so the
    // round-trip really is exercising them (`djmp` lives in SINGLE).
    for needle in [
        ".routine",
        ".section tables",
        ".section code",
        "rd",
        "mtc",
        "wr",
        "mov",
        "jmp",
        "jm",
        "jnm",
        "call",
        "ret",
        "brk",
        "nop",
        "hlt",
        "stp",
    ] {
        assert!(text.contains(needle), "dis missing `{needle}`:\n{text}");
    }
    let obj2 = assemble(&text, false).expect("rendered object disassembly re-assembles");
    assert_eq!(
        obj1.to_bytes(),
        obj2.to_bytes(),
        "object dis ∘ asm must reproduce the object byte-for-byte:\n{text}"
    );
}

#[test]
fn sectioned_program_links_and_dis_renders_both_sections() {
    let obj = assemble(SINGLE, true).expect("assembles");
    let out = link(&tm1_syntax(), &[obj], &[], LinkOptions::default()).expect("links");
    // The executable header is filled from the entry's `.routine`.
    assert_eq!(out.executable.tape_count, 2);
    assert_eq!(out.executable.alphabet_cardinalities, vec![2, 2]);
    // Sectioned executable disassembly renders BOTH the table section and
    // the code section, resolving labels through the map.
    let text = disassemble_executable_with_map(&out.executable, &out.map);
    assert!(
        text.contains(".section tables"),
        "no table section:\n{text}"
    );
    assert!(text.contains(".row"), "no rows:\n{text}");
    assert!(text.contains(".targets"), "no dispatch targets:\n{text}");
    assert!(text.contains(".section code"), "no code section:\n{text}");
    assert!(text.contains("djmp"), "no djmp:\n{text}");
}

#[test]
fn sectioned_executable_dis_round_trips_byte_identically() {
    // The strong round trip at the executable level (mirrors the core
    // framework test): link, disassemble WITH the map, re-assemble, re-link
    // — the images must be byte-identical. This is where `djmp` round-trips:
    // the map names both the dispatch targets and the code labels.
    let out = link(
        &tm1_syntax(),
        &[assemble(SINGLE, true).unwrap()],
        &[],
        LinkOptions::default(),
    )
    .expect("links");
    let text = disassemble_executable_with_map(&out.executable, &out.map);
    let obj2 = assemble(&text, false).expect("rendered text re-assembles");
    let out2 = link(&tm1_syntax(), &[obj2], &[], LinkOptions::default()).expect("re-links");
    assert_eq!(
        out2.executable.to_bytes(),
        out.executable.to_bytes(),
        "dis ∘ link must reproduce the image byte-for-byte:\n{text}"
    );
}

#[test]
fn formatter_is_idempotent_on_the_sectioned_program() {
    let caps = tm1_syntax().caps;
    let once = mtc_core::asm::format_asm_with(SINGLE, caps).expect("formats");
    let twice = mtc_core::asm::format_asm_with(&once, caps).expect("re-formats");
    assert_eq!(once, twice, "format_asm_with must be idempotent:\n{once}");
}

#[test]
fn call_s_is_linker_only_and_rejected_in_source() {
    // `call.s` is the short form of `call`: it exists in the syntax table
    // for relaxation + disassembly display, but the assembler rejects it by
    // name — only the linker's fixpoint may select it. Mirrors PM-1.
    let src = "\
.routine main, tapes=1, alpha=(2)
.section code
.func main
        call.s main
        stp
";
    let err = assemble(src, false).expect_err("call.s must be rejected in source");
    assert!(
        format!("{:?}", err.kind).contains("call.s width is linker-selected"),
        "unexpected error: {:?}",
        err.kind
    );
}

#[test]
fn routine_vectors_and_rept_are_accepted_together() {
    // A single source using `.routine`, the `[..]` vector operands, and a
    // `.rept` block together — the three caps the TM-1 dialect turns on.
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
        .rept i, 0, 2
        mov  [>, >]
        .endr
        wr   [1, -]
        stp
";
    let obj = assemble(src, false).expect(".routine + vectors + .rept assemble together");
    // The `.rept i, 0, 2` unrolls the `mov [>, >]` three times; a round trip
    // through the object disassembly proves the whole thing re-assembles.
    let text = disassemble_object(&obj);
    let obj2 = assemble(&text, false).expect("re-assembles");
    assert_eq!(obj.to_bytes(), obj2.to_bytes());
}
