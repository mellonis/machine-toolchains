//! Two-column compilation: every `.pmc` compile emits a normal build and
//! a gated (volatile) build of every function, merged into ONE object with
//! per-function dedup. These tests pin the merged object's shape — blob
//! variants, symbol pairing, relocation coherence, the program bit — and
//! the standing gate that the normal column is byte-for-byte today's
//! pipeline.

use mtc_core::formats::object::{BlobDebug, BlobVariant, ObjectFile, SymbolDef};
use mtc_post_machine::compiler::{CompileOptions, CompileOutput, VariantColumns, compile};
use mtc_post_machine::optimizer::OptLevel;

/// `mark; right;` fuses to one `wrr` in the normal column and stays two
/// transactions in the gated one — the smallest program whose columns
/// diverge.
const FUSING: &str = "main() {\n    mark;\n    right;\n}\n";

/// Touches no tape: both columns come out byte-identical and dedup.
const TAPE_FREE: &str = "helper() {\n    halt;\n}\nmain() {\n    @helper();\n}\n";

/// Calls, a loop, and tape writes — a whole-program fixture. The runs
/// that must fuse are deliberately unlabelled: a label starts a new block,
/// and `fuse-tape-ops` only fuses within one.
const MIXED: &str = "\
use std::goToEnd;

scrub() {
    1: unmark;
       right;
       check(1, !);
}

main() {
    @goToEnd();
    mark;
    left;
    @scrub();
}
";

fn opts(level: OptLevel, columns: VariantColumns) -> CompileOptions {
    CompileOptions {
        opt_level: level,
        columns,
        ..Default::default()
    }
}

fn build(src: &str, level: OptLevel, columns: VariantColumns) -> CompileOutput {
    compile(src, opts(level, columns)).expect("compiles")
}

fn variants(obj: &ObjectFile) -> &[BlobVariant] {
    obj.variants
        .as_deref()
        .expect("a compiled object always carries variant tags")
}

/// Every blob a name's symbols point at, in symbol order, with its tag.
fn columns_of(obj: &ObjectFile, name: &str) -> Vec<(u32, BlobVariant)> {
    obj.symbols
        .iter()
        .filter(|s| s.name == name)
        .filter_map(|s| match s.def {
            SymbolDef::Defined { blob } | SymbolDef::Local { blob } => Some(blob),
            SymbolDef::External => None,
        })
        .map(|blob| (blob, variants(obj)[blob as usize]))
        .collect()
}

/// One blob's full record, comparable across objects whose symbol tables
/// are numbered differently: code bytes, its relocations as
/// `(offset, callee NAME)`, and its debug side table.
#[derive(Debug, PartialEq, Eq)]
struct BlobRecord {
    name: String,
    code: Vec<u8>,
    relocs: Vec<(u32, String)>,
    debug: Option<BlobDebug>,
}

fn record(obj: &ObjectFile, blob: u32) -> BlobRecord {
    let name = obj
        .symbols
        .iter()
        .find(|s| {
            matches!(s.def, SymbolDef::Defined { blob: b } | SymbolDef::Local { blob: b } if b == blob)
        })
        .expect("every blob has a symbol")
        .name
        .clone();
    let mut relocs: Vec<(u32, String)> = obj
        .relocations
        .iter()
        .filter(|r| r.blob == blob)
        .map(|r| (r.offset, obj.symbols[r.symbol as usize].name.clone()))
        .collect();
    relocs.sort();
    BlobRecord {
        name,
        code: obj.blobs[blob as usize].clone(),
        relocs,
        debug: obj.debug.as_ref().map(|d| d[blob as usize].clone()),
    }
}

/// The blobs a NORMAL program links: everything tagged `Normal` or `Both`,
/// in blob order.
fn normal_projection(obj: &ObjectFile) -> Vec<BlobRecord> {
    (0..obj.blobs.len() as u32)
        .filter(|&b| !matches!(variants(obj)[b as usize], BlobVariant::Volatile))
        .map(|b| record(obj, b))
        .collect()
}

// --- The governing gate --------------------------------------------------

#[test]
fn normal_column_output_is_byte_identical_to_today() {
    for level in [OptLevel::O0, OptLevel::O1] {
        for debug_info in [false, true] {
            for (name, src) in [
                ("fusing", FUSING),
                ("tape-free", TAPE_FREE),
                ("mixed", MIXED),
            ] {
                let both = compile(
                    src,
                    CompileOptions {
                        debug_info,
                        ..opts(level, VariantColumns::Both)
                    },
                )
                .expect("compiles");
                let normal = compile(
                    src,
                    CompileOptions {
                        debug_info,
                        ..opts(level, VariantColumns::NormalOnly)
                    },
                )
                .expect("compiles");
                assert_eq!(
                    both.pma, normal.pma,
                    "{name} at {level:?} (-g {debug_info}): the -S listing must be the normal column"
                );
                assert_eq!(
                    normal_projection(&both.object),
                    normal_projection(&normal.object),
                    "{name} at {level:?} (-g {debug_info}): the normal column drifted"
                );
                assert_eq!(both.ir, normal.ir, "{name}: `ir` is the normal column");
            }
        }
    }
}

// --- Two columns, dedup, tags -------------------------------------------

#[test]
fn two_columns_differ_on_a_tape_touching_function_at_o1() {
    let out = build(FUSING, OptLevel::O1, VariantColumns::Both);
    let obj = &out.object;
    let cols = columns_of(obj, "main");
    assert_eq!(
        cols.len(),
        2,
        "a fusing function gets two same-name symbols, got {cols:?}"
    );
    assert_eq!(cols[0].1, BlobVariant::Normal);
    assert_eq!(cols[1].1, BlobVariant::Volatile);
    // Adjacent, normal first.
    assert_eq!(cols[1].0, cols[0].0 + 1);
    let normal = &obj.blobs[cols[0].0 as usize];
    let volatile = &obj.blobs[cols[1].0 as usize];
    assert!(
        volatile.len() > normal.len(),
        "the gated column keeps both transactions: normal {normal:?} vs volatile {volatile:?}"
    );
    // Both symbols keep the same visibility.
    let defs: Vec<&SymbolDef> = obj
        .symbols
        .iter()
        .filter(|s| s.name == "main")
        .map(|s| &s.def)
        .collect();
    assert!(
        defs.iter().all(|d| matches!(d, SymbolDef::Defined { .. })),
        "main is exported in both columns: {defs:?}"
    );
}

#[test]
fn identical_columns_dedup_to_both() {
    let out = build(TAPE_FREE, OptLevel::O1, VariantColumns::Both);
    let obj = &out.object;
    assert_eq!(
        variants(obj),
        &[BlobVariant::Both, BlobVariant::Both],
        "a tape-free program dedups every function"
    );
    for name in ["helper", "main"] {
        assert_eq!(
            columns_of(obj, name).len(),
            1,
            "{name} keeps one symbol when its columns dedup"
        );
    }
}

#[test]
fn at_o0_every_function_dedups_to_both() {
    for (name, src) in [
        ("fusing", FUSING),
        ("tape-free", TAPE_FREE),
        ("mixed", MIXED),
    ] {
        let both = build(src, OptLevel::O0, VariantColumns::Both);
        let normal = build(src, OptLevel::O0, VariantColumns::NormalOnly);
        assert!(
            variants(&both.object)
                .iter()
                .all(|v| *v == BlobVariant::Both),
            "{name}: -O0 columns are identical by construction, got {:?}",
            variants(&both.object)
        );
        assert_eq!(
            both.object.blobs.len(),
            normal.object.blobs.len(),
            "{name}: -O0 blob count must equal the single-column build's"
        );
        assert_eq!(both.object.symbols, normal.object.symbols, "{name}");
    }
}

#[test]
fn debug_info_does_not_break_o0_dedup() {
    // Each column's debug lines are remapped with ITS OWN pma line map
    // before the merge; remapping after would corrupt one column and the
    // `BlobDebug` comparison would stop deduping.
    let out = compile(
        MIXED,
        CompileOptions {
            debug_info: true,
            ..opts(OptLevel::O0, VariantColumns::Both)
        },
    )
    .expect("compiles");
    assert!(
        variants(&out.object)
            .iter()
            .all(|v| *v == BlobVariant::Both),
        "-g at -O0 still dedups: {:?}",
        variants(&out.object)
    );
    assert_eq!(
        out.object.debug.as_ref().map(Vec::len),
        Some(out.object.blobs.len()),
        "debug parallels blobs"
    );
}

// --- The program bit -----------------------------------------------------

#[test]
fn volatile_main_sets_the_program_bit() {
    let plain = build(
        "main() {\n    mark;\n}\n",
        OptLevel::O1,
        VariantColumns::Both,
    );
    assert!(!plain.object.program_volatile);
    let volatile = build(
        "volatile main() {\n    mark;\n}\n",
        OptLevel::O1,
        VariantColumns::Both,
    );
    assert!(volatile.object.program_volatile);
    // The bit is the source's declaration, never the built column: a
    // single-column build of a plain program stays bit-false.
    let normal_only = build(
        "main() {\n    mark;\n}\n",
        OptLevel::O1,
        VariantColumns::VolatileOnly,
    );
    assert!(!normal_only.object.program_volatile);
}

#[test]
fn variant_carrying_objects_serialize_as_mo_v3() {
    let out = build(FUSING, OptLevel::O1, VariantColumns::Both);
    let bytes = out.object.to_bytes();
    // MAGIC (3 bytes) then the u16 format version, little-endian.
    assert_eq!(
        (bytes[3], bytes[4]),
        (3, 0),
        "variant records force the v3 wire shape, got version bytes {:?}",
        &bytes[3..5]
    );
    // And it reads back unchanged.
    let back = ObjectFile::from_bytes(&bytes).expect("round-trips");
    assert_eq!(back, out.object);
}

// --- Single-column compiles ---------------------------------------------

#[test]
fn single_column_compiles_skip_the_other_pipeline() {
    let normal = build(FUSING, OptLevel::O1, VariantColumns::NormalOnly);
    assert!(
        variants(&normal.object)
            .iter()
            .all(|v| *v == BlobVariant::Normal)
    );
    assert!(normal.ir_volatile.is_none(), "no volatile column was built");
    assert_eq!(columns_of(&normal.object, "main").len(), 1);

    let volatile = build(FUSING, OptLevel::O1, VariantColumns::VolatileOnly);
    assert!(
        variants(&volatile.object)
            .iter()
            .all(|v| *v == BlobVariant::Volatile)
    );
    assert!(
        volatile.ir_volatile.is_some(),
        "the volatile column IS built"
    );
    assert_eq!(columns_of(&volatile.object, "main").len(), 1);

    // `pma` always renders the column that was built: the -S byte-identity
    // gate binds the normal listing only.
    assert!(
        normal.pma.contains("wrr"),
        "the normal column fuses: {}",
        normal.pma
    );
    assert!(
        !volatile.pma.contains("wrr") && volatile.pma.contains("rgt"),
        "the volatile listing keeps both transactions: {}",
        volatile.pma
    );
    let both = build(FUSING, OptLevel::O1, VariantColumns::Both);
    assert_eq!(both.pma, normal.pma, "Both renders the normal listing");
    assert_eq!(
        both.ir_volatile.as_ref(),
        volatile.ir_volatile.as_ref(),
        "Both's volatile CFG is the VolatileOnly build's"
    );
}

// --- The merged object's internal wiring ---------------------------------

#[test]
fn relocations_stay_column_coherent() {
    // A call from the volatile column binds the callee's volatile column;
    // a call from the normal column binds the normal one. That is what
    // makes a volatile program's whole call graph gated — the linker's
    // Local-symbol path binds a relocation's blob directly.
    // helper is over the inliner's size limit, so both call sites survive
    // as real relocations; its unlabelled write+move runs fuse, so it
    // splits into two columns.
    let src = "\
helper() {
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
}
main() {
    @helper();
    unmark;
    left;
    @helper();
}
";
    let out = build(src, OptLevel::O1, VariantColumns::Both);
    let obj = &out.object;
    let helper = columns_of(obj, "helper");
    assert_eq!(helper.len(), 2, "helper fuses, so it splits: {helper:?}");
    let main = columns_of(obj, "main");
    assert_eq!(main.len(), 2, "main calls a split callee: {main:?}");

    let callee_blob = |from: u32| -> Vec<u32> {
        obj.relocations
            .iter()
            .filter(|r| r.blob == from)
            .map(|r| match obj.symbols[r.symbol as usize].def {
                SymbolDef::Defined { blob } | SymbolDef::Local { blob } => blob,
                SymbolDef::External => panic!("helper is defined in this object"),
            })
            .collect()
    };
    let (normal_main, volatile_main) = (main[0].0, main[1].0);
    assert!(
        callee_blob(normal_main).iter().all(|&b| b == helper[0].0),
        "the normal column calls the normal helper"
    );
    assert!(
        callee_blob(volatile_main).iter().all(|&b| b == helper[1].0),
        "the volatile column calls the volatile helper"
    );
}

#[test]
fn a_normal_program_links_the_normal_column() {
    use mtc_core::linker::LinkOptions;
    use mtc_post_machine::asm::link;
    use mtc_post_machine::stdlib;

    // Two same-name symbols in one object are a variant pair, not a
    // duplicate: the link succeeds and produces exactly today's image.
    let out = build(MIXED, OptLevel::O1, VariantColumns::Both);
    let single = build(MIXED, OptLevel::O1, VariantColumns::NormalOnly);
    let two_column = link(
        &[out.object],
        std::slice::from_ref(stdlib::object()),
        LinkOptions::default(),
    )
    .expect("a two-column object links")
    .executable;
    let one_column = link(
        &[single.object],
        std::slice::from_ref(stdlib::object()),
        LinkOptions::default(),
    )
    .expect("links")
    .executable;
    assert_eq!(
        two_column.to_bytes(),
        one_column.to_bytes(),
        "the volatile column is unreachable in a normal program and must not reach the image"
    );
}

#[test]
fn a_column_invariant_entry_splits_when_a_transitive_callee_splits() {
    // The dedup key cannot be per-function alone. `main` and `mid` here
    // are byte-identical in both columns AND reference the same callee
    // NAMES, yet `leaf` fuses and splits — so a `Both` `mid` would bind
    // the normal `leaf` and a `Both` `main` the normal `mid`, leaving the
    // volatile column of a program that declares itself volatile
    // unreachable from its own entry. Deduping is therefore transitive: a
    // function may be `Both` only if every intra-object callee is too.
    //
    // The shape is deliberately adversarial to a single forward pass:
    // the chain is declared entry-first, so `main` is decided before
    // `mid` is known to have demoted. Only a fixpoint gets it right.
    // Sizes keep the inliner out in BOTH columns — `leaf` stays over the
    // op limit even once fused, and `mid`/`main` contain calls.
    let src = "\
volatile main() {
    @mid();
    @mid();
    @mid();
    @mid();
    @mid();
    @mid();
    @mid();
}
mid() {
    @leaf();
    @leaf();
    @leaf();
    @leaf();
    @leaf();
    @leaf();
    @leaf();
}
leaf() {
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
}
";
    let out = build(src, OptLevel::O1, VariantColumns::Both);
    let obj = &out.object;
    assert!(obj.program_volatile, "the fixture declares a volatile main");

    let leaf = columns_of(obj, "leaf");
    let mid = columns_of(obj, "mid");
    let main = columns_of(obj, "main");
    assert_eq!(leaf.len(), 2, "leaf fuses, so it splits: {leaf:?}");
    assert_eq!(
        mid.len(),
        2,
        "mid is column-invariant itself but must follow its split callee: {mid:?}"
    );
    assert_eq!(
        main.len(),
        2,
        "main must follow mid transitively, not dedup: {main:?}"
    );
    for (name, cols) in [("leaf", &leaf), ("mid", &mid), ("main", &main)] {
        assert_eq!(
            (cols[0].1, cols[1].1),
            (BlobVariant::Normal, BlobVariant::Volatile),
            "{name} splits normal-first"
        );
    }

    let callees = |from: u32| -> Vec<u32> {
        obj.relocations
            .iter()
            .filter(|r| r.blob == from)
            .map(|r| match obj.symbols[r.symbol as usize].def {
                SymbolDef::Defined { blob } | SymbolDef::Local { blob } => blob,
                SymbolDef::External => panic!("every callee here is defined in this object"),
            })
            .collect()
    };
    // Both levels of the chain stay inside their own column.
    for (level, caller, callee) in [
        ("main -> mid", main[0].0, mid[0].0),
        ("mid -> leaf", mid[0].0, leaf[0].0),
    ] {
        assert!(
            !callees(caller).is_empty() && callees(caller).iter().all(|&b| b == callee),
            "normal {level}: {:?} should all be blob {callee}",
            callees(caller)
        );
    }
    for (level, caller, callee) in [
        ("main -> mid", main[1].0, mid[1].0),
        ("mid -> leaf", mid[1].0, leaf[1].0),
    ] {
        assert!(
            !callees(caller).is_empty() && callees(caller).iter().all(|&b| b == callee),
            "volatile {level}: {:?} should all be blob {callee}",
            callees(caller)
        );
    }
}

#[test]
fn a_fully_column_invariant_chain_still_dedups() {
    // The transitive rule must not over-split: when nothing in the chain
    // touches the tape, every function stays `Both`. (`debugger` and the
    // calls keep the inliner from collapsing the chain into main.)
    let src = "\
tail() {
    debugger;
    halt;
}
mid() {
    @tail();
}
main() {
    @mid();
}
";
    let out = build(src, OptLevel::O1, VariantColumns::Both);
    let obj = &out.object;
    assert_eq!(
        variants(obj),
        &[BlobVariant::Both, BlobVariant::Both, BlobVariant::Both],
        "a tape-free chain dedups end to end"
    );
    for name in ["tail", "mid", "main"] {
        assert_eq!(columns_of(obj, name).len(), 1, "{name} keeps one symbol");
    }
}
