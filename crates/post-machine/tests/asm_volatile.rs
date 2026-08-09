//! The `.pma` `.volatile` directive: the text form of a two-column object
//! (docs/formats.md (assembly text)). A `.volatile` line inside a `.func`
//! block tags that blob's build column; one before the first `.func` sets
//! the object's program bit. These tests pin the acceptance rules, the
//! asm-side dedup that mirrors the compiler's, column-coherent call
//! binding, the disassembler's emission, and the standing gate that a
//! directive-free file assembles to exactly the bytes it always did.

use mtc_core::asm::AsmErrorKind;
use mtc_core::formats::object::{BlobVariant, ObjectFile, SymbolDef};
use mtc_post_machine::asm::{assemble, disassemble_object};
use mtc_post_machine::compiler::{CompileOptions, VariantColumns, compile};
use mtc_post_machine::optimizer::OptLevel;

// --- Helpers -------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn tags(obj: &ObjectFile) -> Option<&[BlobVariant]> {
    obj.variants.as_deref()
}

/// `(name, blob, variant)` per defined symbol, in symbol order.
fn defs(obj: &ObjectFile) -> Vec<(&str, u32, BlobVariant)> {
    obj.symbols
        .iter()
        .filter_map(|s| match s.def {
            SymbolDef::Defined { blob } | SymbolDef::Local { blob } => Some((
                s.name.as_str(),
                blob,
                obj.variants.as_ref().expect("tagged object")[blob as usize],
            )),
            SymbolDef::External => None,
        })
        .collect()
}

/// Every call edge as `(caller blob, callee symbol name, callee blob or
/// None for an external)`, sorted for a stable comparison.
fn edges(obj: &ObjectFile) -> Vec<(u32, &str, Option<u32>)> {
    let mut out: Vec<(u32, &str, Option<u32>)> = obj
        .relocations
        .iter()
        .map(|r| {
            let symbol = &obj.symbols[r.symbol as usize];
            let blob = match symbol.def {
                SymbolDef::Defined { blob } | SymbolDef::Local { blob } => Some(blob),
                SymbolDef::External => None,
            };
            (r.blob, symbol.name.as_str(), blob)
        })
        .collect();
    out.sort_unstable();
    out
}

fn err_of(src: &str) -> AsmErrorKind {
    assemble(src, false).expect_err("must be rejected").kind
}

// --- The byte-identity gate ----------------------------------------------

/// A `.pma` file that uses no `.volatile` assembles to EXACTLY the bytes it
/// did before the directive existed. The expected hex was captured from the
/// pre-directive assembler and is pinned here verbatim — never regenerated
/// from this build's own output.
#[test]
fn a_directive_free_file_assembles_byte_identically() {
    const SIMPLE: &str = ".func main\n        rgt\n        wr      1\n        stp\n";
    const RICH: &str = "\
.func main
        call    helper
        call    external
        jm      L1
L1:     nop
        stp
.func helper local
        brk
        wr      0
        ret
";
    // (fixture, with_debug, the object bytes the pre-directive assembler
    // produced) — see docs/formats.md (.pmo) for the wire shape.
    let cases: [(&str, &str, bool, &str); 4] = [
        (
            "SIMPLE",
            SIMPLE,
            false,
            "4d4f0102000100b9b9df630100000004006d61696e01000000000000000100000000010000000500\
             00000d0506810200000000",
        ),
        (
            "SIMPLE",
            SIMPLE,
            true,
            "4d4f0102000101341391ea0100000004006d61696e01000000000000000100000000010000000500\
             00000d05068102000000000000000003000000010000000200000002000000030000000400000004\
             000000",
        ),
        (
            "RICH",
            RICH,
            false,
            "4d4f01020001005359f2400300000004006d61696e060068656c706572080065787465726e616c03\
             0000000000000001000000000100000002010000000200000000ffffffff020000000f0000000d0b\
             000000000b0000000019000102050000000d0e06800c020000000000000002000000010000000000\
             00000700000002000000",
        ),
        (
            "RICH",
            RICH,
            true,
            "4d4f0102000101ad1b5f100400000004006d61696e060068656c706572080065787465726e616c02\
             004c31030000000000000001000000000100000002010000000200000000ffffffff020000000f00\
             00000d0b000000000b0000000019000102050000000d0e06800c02000000000000000200000001000\
             00000000000070000000200000001000000030000000d0000000500000001000000020000000600\
             0000030000000b000000040000000d000000050000000e0000000600000000000000030000000100\
             0000080000000200000009000000040000000a000000",
        ),
    ];
    for (name, src, with_debug, expected) in cases {
        let obj = assemble(src, with_debug).expect("assembles");
        assert_eq!(
            obj.variants, None,
            "{name} (debug={with_debug}): a directive-free file carries no variant records"
        );
        assert!(
            !obj.program_volatile,
            "{name} (debug={with_debug}): a directive-free file sets no program bit"
        );
        assert_eq!(
            hex(&obj.to_bytes()),
            expected.replace(['\n', ' '], ""),
            "{name} (debug={with_debug}): object bytes moved"
        );
    }
}

// --- Tagging -------------------------------------------------------------

#[test]
fn a_volatile_func_is_tagged_volatile() {
    let obj = assemble(".func f\n.volatile\n        stp\n", false).expect("assembles");
    assert_eq!(tags(&obj), Some(&[BlobVariant::Volatile][..]));
    assert!(!obj.program_volatile);
}

#[test]
fn an_untagged_func_stays_normal_in_a_tagged_file() {
    let src = ".func f\n.volatile\n        stp\n.func g\n        stp\n";
    let obj = assemble(src, false).expect("assembles");
    assert_eq!(
        tags(&obj),
        Some(&[BlobVariant::Volatile, BlobVariant::Normal][..])
    );
}

#[test]
fn a_file_level_volatile_sets_the_program_bit_without_tagging() {
    let src = ".volatile\n.func main\n        stp\n";
    let obj = assemble(src, false).expect("assembles");
    assert!(obj.program_volatile);
    assert_eq!(
        obj.variants, None,
        "the program bit is independent of per-blob tags"
    );
    // This bit-set/tag-free shape is exactly what the linker's counted
    // fallback exists for, so it has to survive the text form too.
    let text = disassemble_object(&obj);
    assert!(text.starts_with(".volatile\n"), "{text}");
    assert_eq!(assemble(&text, false).expect("dis output assembles"), obj);
}

// --- The variant-aware duplicate rules -----------------------------------

#[test]
fn a_same_name_pair_with_distinct_bodies_makes_two_columns() {
    let src = ".func f\n        rgt\n        stp\n.func f\n.volatile\n        lft\n        stp\n";
    let obj = assemble(src, false).expect("a bare/volatile pair is legal");
    assert_eq!(
        defs(&obj),
        vec![
            ("f", 0, BlobVariant::Normal),
            ("f", 1, BlobVariant::Volatile)
        ]
    );
}

#[test]
fn the_volatile_member_may_come_first() {
    let src = ".func f\n.volatile\n        lft\n        stp\n.func f\n        rgt\n        stp\n";
    let obj = assemble(src, false).expect("order within the pair is free");
    // The emitted columns are normalized: normal first, then volatile.
    assert_eq!(
        defs(&obj),
        vec![
            ("f", 0, BlobVariant::Normal),
            ("f", 1, BlobVariant::Volatile)
        ]
    );
    assert_eq!(
        obj.blobs[0],
        assemble(".func f\n rgt\n stp\n", false).unwrap().blobs[0]
    );
}

#[test]
fn two_bare_same_name_funcs_stay_a_duplicate() {
    assert_eq!(
        err_of(".func f\n        stp\n.func f\n        stp\n"),
        AsmErrorKind::DuplicateFunction("f".to_string())
    );
}

#[test]
fn two_volatile_same_name_funcs_are_a_duplicate() {
    let src = ".func f\n.volatile\n        stp\n.func f\n.volatile\n        stp\n";
    assert_eq!(
        err_of(src),
        AsmErrorKind::DuplicateFunction("f".to_string())
    );
}

#[test]
fn three_blocks_of_one_name_are_a_duplicate() {
    let src = ".func f\n        stp\n.func f\n.volatile\n        stp\n.func f\n        stp\n";
    assert_eq!(
        err_of(src),
        AsmErrorKind::DuplicateFunction("f".to_string())
    );
}

#[test]
fn a_pair_must_agree_on_visibility() {
    let src = ".func f\n        stp\n.func f local\n.volatile\n        stp\n";
    assert_eq!(
        err_of(src),
        AsmErrorKind::Syntax("a `.volatile` twin must match its function's visibility")
    );
}

// --- Directive placement -------------------------------------------------

#[test]
fn a_volatile_after_code_is_misplaced() {
    assert_eq!(
        err_of(".func f\n        nop\n.volatile\n        stp\n"),
        AsmErrorKind::Syntax("`.volatile` must directly follow its `.func`")
    );
}

#[test]
fn a_second_volatile_in_one_func_is_misplaced() {
    assert_eq!(
        err_of(".func f\n.volatile\n.volatile\n        stp\n"),
        AsmErrorKind::Syntax("`.volatile` must directly follow its `.func`")
    );
}

#[test]
fn a_volatile_after_a_pending_label_is_misplaced() {
    assert_eq!(
        err_of(".func f\nL1:\n.volatile\n        stp\n"),
        AsmErrorKind::Syntax("`.volatile` must directly follow its `.func`")
    );
}

#[test]
fn a_duplicate_program_level_volatile_is_rejected() {
    assert_eq!(
        err_of(".volatile\n.volatile\n.func main\n        stp\n"),
        AsmErrorKind::Syntax("duplicate `.volatile`")
    );
}

#[test]
fn a_volatile_with_operands_is_rejected() {
    assert_eq!(
        err_of(".func f\n.volatile 1\n        stp\n"),
        AsmErrorKind::Syntax("`.volatile` takes no operands")
    );
}

#[test]
fn a_labeled_volatile_is_an_unknown_mnemonic() {
    // Mirrors `.func`: the directive is only a directive unlabeled.
    assert_eq!(
        err_of(".func f\nL1: .volatile\n        stp\n"),
        AsmErrorKind::UnknownMnemonic(".volatile".to_string())
    );
}

#[test]
fn a_comment_between_the_func_and_its_volatile_is_trivia() {
    let src = ".func f\n; the gated column\n.volatile\n        stp\n";
    let obj = assemble(src, false).expect("comments are trivia");
    assert_eq!(tags(&obj), Some(&[BlobVariant::Volatile][..]));
}

// --- Asm-side dedup ------------------------------------------------------

#[test]
fn an_identical_pair_dedups_to_one_both_blob() {
    let src = ".func f\n        stp\n.func f\n.volatile\n        stp\n";
    let obj = assemble(src, false).expect("assembles");
    assert_eq!(defs(&obj), vec![("f", 0, BlobVariant::Both)]);
    assert_eq!(obj.blobs.len(), 1);
}

/// The producer guarantee the linker checks: a `Both` blob only ever
/// references `Both` blobs inside its own object.
#[test]
fn a_both_blob_only_calls_both_blobs() {
    let src = "\
.func main
        call    helper
        stp
.func main
.volatile
        call    helper
        stp
.func helper
        ret
.func helper
.volatile
        ret
";
    let obj = assemble(src, false).expect("assembles");
    assert_eq!(
        defs(&obj),
        vec![
            ("main", 0, BlobVariant::Both),
            ("helper", 1, BlobVariant::Both)
        ]
    );
    let variants = tags(&obj).expect("tagged");
    for (caller, name, callee) in edges(&obj) {
        if variants[caller as usize] == BlobVariant::Both {
            assert_eq!(
                callee.map(|b| variants[b as usize]),
                Some(BlobVariant::Both),
                "a Both blob calls `{name}`, which is not Both"
            );
        }
    }
}

#[test]
fn a_dedup_candidate_calling_a_split_callee_splits_with_it() {
    // `main`'s two blocks are byte-identical; `helper`'s are not, so the
    // dedup demotes transitively rather than pinning one column.
    let src = "\
.func main
        call    helper
        stp
.func main
.volatile
        call    helper
        stp
.func helper
        rgt
        ret
.func helper
.volatile
        lft
        ret
";
    let obj = assemble(src, false).expect("assembles");
    assert_eq!(
        defs(&obj),
        vec![
            ("main", 0, BlobVariant::Normal),
            ("main", 1, BlobVariant::Volatile),
            ("helper", 2, BlobVariant::Normal),
            ("helper", 3, BlobVariant::Volatile),
        ]
    );
    // Column-coherent: each `main` binds its own column's `helper`.
    assert_eq!(
        edges(&obj),
        vec![(0, "helper", Some(2)), (1, "helper", Some(3))]
    );
}

#[test]
fn a_call_with_no_matching_column_becomes_external() {
    // `helper` ships only a normal column, so the volatile `main` cannot
    // bind it locally — the name leaves the object and the linker resolves
    // it (falling back to the one column, counted).
    let src = "\
.func main
.volatile
        call    helper
        stp
.func helper
        ret
";
    let obj = assemble(src, false).expect("assembles");
    assert_eq!(edges(&obj), vec![(0, "helper", None)]);
    assert!(
        obj.symbols
            .iter()
            .any(|s| s.name == "helper" && matches!(s.def, SymbolDef::External)),
        "the unmatched column mints an external, not a cross-column bind"
    );
}

// --- Disassembly ---------------------------------------------------------

#[test]
fn dis_emits_the_program_bit_first_and_a_directive_per_tag() {
    let src = "\
.volatile
.func main
        call    helper
        stp
.func main
.volatile
        rgt
        call    helper
        stp
.func helper
        ret
.func helper
.volatile
        ret
";
    let obj = assemble(src, false).expect("assembles");
    let text = disassemble_object(&obj);
    let structural: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with(".func") || *l == ".volatile")
        .collect();
    assert_eq!(
        structural,
        vec![
            ".volatile",
            ".func main",
            ".func main",
            ".volatile",
            // `helper` deduped to Both: printed once bare, once gated.
            ".func helper",
            ".func helper",
            ".volatile",
        ],
        "dis text:\n{text}"
    );
    assert!(
        text.starts_with(".volatile\n"),
        "the program bit leads the dump:\n{text}"
    );
}

#[test]
fn a_hand_written_two_column_file_round_trips_through_dis() {
    let src = "\
.volatile
.func main
        call    helper
        stp
.func main
.volatile
        rgt
        call    helper
        stp
.func helper
        ret
.func helper
.volatile
        ret
";
    let obj = assemble(src, false).expect("assembles");
    let text = disassemble_object(&obj);
    let back =
        assemble(&text, false).unwrap_or_else(|e| panic!("dis output must assemble: {e}\n{text}"));
    assert_eq!(back, obj, "dis -> asm diverged\n{text}");
}

#[test]
fn a_compiled_two_column_object_round_trips_through_dis() {
    // The round's text-expressibility gate: a real merged object (a `Both`
    // blob, a split pair, and the program bit) reassembles from its own
    // disassembly, byte for byte.
    const SRC: &str = "\
helper() {
    halt;
}

volatile main() {
    mark;
    right;
    @helper();
}
";
    for level in [OptLevel::O0, OptLevel::O1] {
        let out = compile(
            SRC,
            CompileOptions {
                opt_level: level,
                columns: VariantColumns::Both,
                ..Default::default()
            },
        )
        .expect("compiles");
        assert!(out.object.program_volatile);
        let text = disassemble_object(&out.object);
        let back = assemble(&text, false)
            .unwrap_or_else(|e| panic!("{level:?}: dis output must assemble: {e}\n{text}"));
        assert_eq!(
            back.to_bytes(),
            out.object.to_bytes(),
            "{level:?}: dis -> asm diverged\n{text}"
        );
    }
}

// --- Debug info and the lint surface -------------------------------------

/// A `Both` blob keeps ONE debug entry (the bare block's), a split pair
/// keeps one each — so the per-blob debug section stays parallel to the
/// blobs, which is a format invariant `from_bytes` enforces and nothing
/// upstream of it checks.
#[test]
fn debug_info_stays_parallel_to_the_merged_blobs() {
    const DEDUPS: &str = ".func f\n        stp\n.func f\n.volatile\n        stp\n";
    const SPLITS: &str =
        ".func f\n        rgt\n        stp\n.func f\n.volatile\n        lft\n        stp\n";
    for (name, src, blobs) in [("dedups", DEDUPS, 1usize), ("splits", SPLITS, 2)] {
        let obj = assemble(src, true).expect("assembles with debug");
        assert_eq!(obj.blobs.len(), blobs, "{name}");
        let debug = obj.debug.as_ref().expect("with_debug carries debug info");
        assert_eq!(debug.len(), obj.blobs.len(), "{name}: debug/blob mismatch");
        assert_eq!(
            tags(&obj).expect("tagged").len(),
            obj.blobs.len(),
            "{name}: variants/blob mismatch"
        );
        // The format layer is the real judge of parallelism.
        let back = ObjectFile::from_bytes(&obj.to_bytes()).expect("the object is well-formed");
        assert_eq!(back, obj, "{name}: wire round trip diverged");
    }
}

/// Whether `-g` was passed must not change an object's variant STRUCTURE —
/// the dedup record reads code and call sites, never the source-line map.
#[test]
fn debug_info_does_not_change_the_variant_structure() {
    const SRC: &str = "\
.func main
        call    helper
        stp
.func main
.volatile
        call    helper
        stp
.func helper
        ret
.func helper
.volatile
        ret
";
    let plain = assemble(SRC, false).expect("assembles");
    let debugged = assemble(SRC, true).expect("assembles with debug");
    assert_eq!(plain.variants, debugged.variants);
    assert_eq!(plain.blobs, debugged.blobs);
    assert_eq!(plain.symbols, debugged.symbols);
    assert_eq!(plain.relocations, debugged.relocations);
}

/// `pmt lint`'s `.pma` route is the remaining consumer of the lowered
/// functions, and a two-column listing is the first input that hands it
/// two same-name blocks. It must read one like any other file.
#[test]
fn a_two_column_listing_lints_clean() {
    const SRC: &str = "\
.volatile
.func main
        call    helper
        stp
.func main
.volatile
        rgt
        call    helper
        stp
.func helper
        ret
.func helper
.volatile
        ret
";
    let syntax = mtc_post_machine::asm::pm1_syntax();
    let findings = mtc_core::asm::lint::lint(&syntax, SRC, &[]).expect("the listing lints");
    assert!(findings.is_empty(), "{findings:?}");
}
