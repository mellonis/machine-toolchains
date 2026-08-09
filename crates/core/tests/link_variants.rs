//! Variant-aware linking (docs/core.md (linking)): an exported name may
//! ship in two build columns, and the linked image takes the one matching
//! the program's volatile bit — falling back to the other, counted, when
//! the wanted column is absent. Everything runs through a neutral fake
//! dialect so core stays provably arch-agnostic; the linked column is
//! asserted from the emitted image's bytes, located through the map
//! sidecar's per-function ranges.

use mtc_core::asm::{ArchSyntax, AsmCaps, Flow, RelaxPair, SyntaxEntry};
use mtc_core::formats::object::{BlobVariant, ObjectFile, Relocation, Symbol, SymbolDef};
use mtc_core::linker::{LinkOptions, LinkOutput, link};
use mtc_core::vm::OperandKind;

const ARCH: u8 = 0x7E;

/// Neutral fake dialect (per-file helper convention): entry/nop/stop plus
/// a relaxable far/short call pair. No caps — nothing here needs tables,
/// `.rept`, or vector operands.
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
                opcode: 0x0E,
                mnemonic: "ent",
                operand: OperandKind::None,
                flow: FT,
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
        ],
        relax_pairs: vec![RelaxPair {
            far: 0x21,
            short: 0x31,
        }],
        entry_opcode: 0x0E,
        break_opcode: None,
        trap_opcode: None,
        caps: AsmCaps::default(),
    }
}

/// How many `nop` filler bytes a column's body carries right after its
/// `ent` — the column marker the assertions read out of the image. It sits
/// before any call site, so relaxation (which only narrows the call
/// operand) can never move or rewrite it.
fn marker(tag: BlobVariant) -> usize {
    match tag {
        BlobVariant::Normal => 1,
        BlobVariant::Volatile => 2,
        BlobVariant::Both => 3,
    }
}

/// One function of one build column: its name, its tag, and the names it
/// calls.
struct Func<'a> {
    name: &'a str,
    tag: BlobVariant,
    calls: &'a [&'a str],
}

fn f<'a>(name: &'a str, tag: BlobVariant, calls: &'a [&'a str]) -> Func<'a> {
    Func { name, tag, calls }
}

/// An object with one blob per [`Func`]: `ent`, the column marker, one
/// far `call` per callee, `stp`. A split callee (two entries under one
/// name) binds **column-coherently** — a `Normal` caller takes the normal
/// symbol, a `Volatile` caller the volatile one, which is the shape the
/// two-column compiler emits. Names in `locals` become `Local` defs; names
/// with no definition here become `External`.
fn object(funcs: &[Func<'_>], locals: &[&str], program_volatile: bool) -> ObjectFile {
    let mut symbols: Vec<Symbol> = funcs
        .iter()
        .enumerate()
        .map(|(i, func)| Symbol {
            name: func.name.into(),
            def: if locals.contains(&func.name) {
                SymbolDef::Local { blob: i as u32 }
            } else {
                SymbolDef::Defined { blob: i as u32 }
            },
        })
        .collect();
    let mut blobs = Vec::new();
    let mut relocations = Vec::new();
    for (bi, func) in funcs.iter().enumerate() {
        let mut blob = vec![0x0E];
        blob.resize(blob.len() + marker(func.tag), 0x01);
        for callee in func.calls {
            let sym = column_symbol(funcs, callee, func.tag).unwrap_or_else(|| {
                symbols.push(Symbol {
                    name: (*callee).into(),
                    def: SymbolDef::External,
                });
                symbols.len() - 1
            });
            blob.push(0x21);
            relocations.push(Relocation {
                blob: bi as u32,
                offset: blob.len() as u32,
                symbol: sym as u32,
            });
            blob.extend([0u8; 4]);
        }
        blob.push(0x02);
        blobs.push(blob);
    }
    let variants: Vec<BlobVariant> = funcs.iter().map(|func| func.tag).collect();
    let mut object = ObjectFile::v2(ARCH, symbols, blobs, relocations, None);
    object.variants = Some(variants);
    object.program_volatile = program_volatile;
    object
}

/// The same shape without variant records — a legacy or hand-assembled
/// object, which the linker reads as an all-normal column.
fn legacy_object(funcs: &[Func<'_>]) -> ObjectFile {
    let mut object = object(funcs, &[], false);
    object.variants = None;
    object
}

/// The symbol index a `caller_tag` column binds for `name`: the sole
/// definition when the name has one, else the matching column.
fn column_symbol(funcs: &[Func<'_>], name: &str, caller_tag: BlobVariant) -> Option<usize> {
    let columns: Vec<usize> = funcs
        .iter()
        .enumerate()
        .filter(|(_, func)| func.name == name)
        .map(|(i, _)| i)
        .collect();
    match columns.len() {
        0 => None,
        1 => Some(columns[0]),
        _ => columns
            .iter()
            .find(|&&i| funcs[i].tag == caller_tag)
            .or(columns.first())
            .copied(),
    }
}

fn link_all(objects: &[ObjectFile], libraries: &[ObjectFile]) -> LinkOutput {
    link(&fake_syntax(), objects, libraries, LinkOptions::default()).expect("links")
}

/// The image bytes of one linked function, sliced out of the emitted code
/// through the map sidecar's range for it.
fn body<'a>(out: &'a LinkOutput, name: &str) -> &'a [u8] {
    let f = out
        .map
        .functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("`{name}` is in the image; map = {:?}", out.map.functions));
    &out.executable.code[f.start as usize..f.end as usize]
}

/// The column a linked function came from, read back from its image bytes:
/// `ent` then the marker run.
fn linked_column(out: &LinkOutput, name: &str) -> BlobVariant {
    let bytes = body(out, name);
    assert_eq!(bytes[0], 0x0E, "every function opens with `ent`");
    let fillers = bytes[1..].iter().take_while(|&&b| b == 0x01).count();
    [
        BlobVariant::Normal,
        BlobVariant::Volatile,
        BlobVariant::Both,
    ]
    .into_iter()
    .find(|&tag| marker(tag) == fillers)
    .unwrap_or_else(|| panic!("`{name}` carries {fillers} filler bytes: {bytes:?}"))
}

#[test]
fn a_normal_program_links_the_normal_column_of_a_two_column_library() {
    let user = object(&[f("main", BlobVariant::Normal, &["lib_fn"])], &[], false);
    let lib = object(
        &[
            f("lib_fn", BlobVariant::Normal, &[]),
            f("lib_fn", BlobVariant::Volatile, &[]),
        ],
        &[],
        false,
    );
    let out = link_all(&[user], &[lib]);
    assert_eq!(linked_column(&out, "lib_fn"), BlobVariant::Normal);
    assert!(
        out.report.variant_fallbacks.is_empty(),
        "the wanted column was present: {:?}",
        out.report.variant_fallbacks
    );
}

#[test]
fn a_volatile_program_links_the_volatile_column() {
    // The program bit rides on the object defining the entry symbol.
    let user = object(&[f("main", BlobVariant::Volatile, &["lib_fn"])], &[], true);
    let lib = object(
        &[
            f("lib_fn", BlobVariant::Normal, &[]),
            f("lib_fn", BlobVariant::Volatile, &[]),
        ],
        &[],
        false,
    );
    let out = link_all(&[user], &[lib]);
    assert_eq!(linked_column(&out, "main"), BlobVariant::Volatile);
    assert_eq!(linked_column(&out, "lib_fn"), BlobVariant::Volatile);
    assert!(
        out.report.variant_fallbacks.is_empty(),
        "both columns were present: {:?}",
        out.report.variant_fallbacks
    );
}

#[test]
fn a_volatile_program_counts_a_legacy_library_fallback() {
    // A tag-free library is all-normal: a volatile program links it
    // anyway, and the report names what it had to fall back on.
    let user = object(&[f("main", BlobVariant::Volatile, &["lib_fn"])], &[], true);
    let lib = legacy_object(&[f("lib_fn", BlobVariant::Normal, &[])]);
    let out = link_all(&[user], &[lib]);
    assert_eq!(linked_column(&out, "lib_fn"), BlobVariant::Normal);
    assert_eq!(out.report.variant_fallbacks, vec!["lib_fn".to_string()]);
}

#[test]
fn a_both_library_links_into_either_program_without_a_fallback() {
    let lib = object(&[f("lib_fn", BlobVariant::Both, &[])], &[], false);
    for program_volatile in [false, true] {
        let tag = if program_volatile {
            BlobVariant::Volatile
        } else {
            BlobVariant::Normal
        };
        let user = object(&[f("main", tag, &["lib_fn"])], &[], program_volatile);
        let out = link_all(&[user], std::slice::from_ref(&lib));
        assert_eq!(
            linked_column(&out, "lib_fn"),
            BlobVariant::Both,
            "the one deduped column serves both programs"
        );
        assert!(
            out.report.variant_fallbacks.is_empty(),
            "a Both column is never a fallback (program_volatile = {program_volatile}): {:?}",
            out.report.variant_fallbacks
        );
    }
}

#[test]
fn an_all_both_library_reports_no_fallbacks() {
    // The stdlib shape: every routine deduped to one column, linked into a
    // volatile program. Nothing falls back, so nothing is reported.
    let user = object(&[f("main", BlobVariant::Volatile, &["alpha"])], &[], true);
    let lib = object(
        &[
            f("alpha", BlobVariant::Both, &["beta"]),
            f("beta", BlobVariant::Both, &[]),
        ],
        &[],
        false,
    );
    let out = link_all(&[user], &[lib]);
    for name in ["alpha", "beta"] {
        assert_eq!(linked_column(&out, name), BlobVariant::Both);
    }
    assert!(
        out.report.variant_fallbacks.is_empty(),
        "{:?}",
        out.report.variant_fallbacks
    );
}

#[test]
fn a_split_entry_links_its_whole_volatile_call_graph() {
    // The two-column compiler's own shape: a split `main` whose private
    // helper split with it, relocations column-coherent. A volatile
    // program must enter the volatile `main` AND reach the volatile
    // helper — not the normal one its sibling blob binds.
    let user = object(
        &[
            f("main", BlobVariant::Normal, &["helper"]),
            f("main", BlobVariant::Volatile, &["helper"]),
            f("helper", BlobVariant::Normal, &[]),
            f("helper", BlobVariant::Volatile, &[]),
        ],
        &["helper"],
        true,
    );
    let out = link_all(std::slice::from_ref(&user), &[]);
    assert_eq!(linked_column(&out, "main"), BlobVariant::Volatile);
    assert_eq!(linked_column(&out, "helper"), BlobVariant::Volatile);
    assert_eq!(
        out.map.functions.len(),
        2,
        "one column of each function ships: {:?}",
        out.map.functions
    );
    assert!(
        out.report.variant_fallbacks.is_empty(),
        "{:?}",
        out.report.variant_fallbacks
    );

    // The same object with the bit cleared is today's image: the normal
    // column, end to end.
    let mut normal = user;
    normal.program_volatile = false;
    let out = link_all(&[normal], &[]);
    assert_eq!(linked_column(&out, "main"), BlobVariant::Normal);
    assert_eq!(linked_column(&out, "helper"), BlobVariant::Normal);
}
