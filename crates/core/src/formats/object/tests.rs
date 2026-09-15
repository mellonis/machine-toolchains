use super::*;
use crate::formats::{ARCH_PM1, FormatError};

fn sample() -> ObjectFile {
    ObjectFile::v2(
        ARCH_PM1,
        vec![
            Symbol {
                name: "main".into(),
                def: SymbolDef::Defined { blob: 0 },
            },
            Symbol {
                name: "goToEnd".into(),
                def: SymbolDef::External,
            },
        ],
        // ent, call <4-byte hole>, stp
        vec![vec![0x0D, 0x0B, 0, 0, 0, 0, 0x02]],
        vec![Relocation {
            blob: 0,
            offset: 2,
            symbol: 1,
        }],
        None,
    )
}

/// An object without v3 data serializes byte-for-byte as v2 — this
/// pins what PM-1's compiler emits.
#[test]
fn v2_shape_is_byte_identical_v2() {
    let obj = sample(); // signatures/table_blobs None, fixups/bound_calls empty
    let bytes = obj.to_bytes();
    assert_eq!(&bytes[0..3], b"MO\x01");
    assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2);
    assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
}

#[test]
fn round_trip_without_debug() {
    let bytes = sample().to_bytes();
    assert_eq!(&bytes[0..3], b"MO\x01");
    let back = ObjectFile::from_bytes(&bytes).unwrap();
    assert_eq!(back, sample());
}

#[test]
fn round_trip_with_local_symbol() {
    let mut obj = sample();
    // A second blob for a local-only helper function.
    obj.blobs.push(vec![0x0D, 0x02]); // ent, stp
    obj.symbols.push(Symbol {
        name: "helper".into(),
        def: SymbolDef::Local { blob: 1 },
    });
    let bytes = obj.to_bytes();
    // Wire version field sits right after the 3-byte magic.
    assert_eq!(
        u16::from_le_bytes([bytes[3], bytes[4]]),
        OBJECT_FORMAT_VERSION_V2
    );
    assert_eq!(OBJECT_FORMAT_VERSION_V2, 2);
    let back = ObjectFile::from_bytes(&bytes).unwrap();
    assert_eq!(back, obj);
}

#[test]
fn version_1_bytes_are_still_accepted() {
    // Valid v2 bytes of an object WITHOUT locals, downgraded to v1: the
    // reader must still accept it (1..=OBJECT_FORMAT_VERSION_V2).
    let mut bytes = sample().to_bytes();
    bytes[3..5].copy_from_slice(&1u16.to_le_bytes());
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(ObjectFile::from_bytes(&bytes).is_ok());
}

#[test]
fn local_symbol_with_bad_blob_rejected() {
    let mut obj = sample();
    obj.symbols[0].def = SymbolDef::Local { blob: 7 };
    let bytes = obj.to_bytes();
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("symbol blob index out of range"))
    ));
}

#[test]
fn round_trip_with_debug() {
    let mut obj = sample();
    obj.debug = Some(vec![BlobDebug {
        labels: vec![("L1".into(), 1)],
        lines: vec![(0, 3), (1, 4)],
    }]);
    let bytes = obj.to_bytes();
    let back = ObjectFile::from_bytes(&bytes).unwrap();
    assert_eq!(back, obj);
}

#[test]
fn crc_corruption_rejected() {
    let mut bytes = sample().to_bytes();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::BadCrc { .. })
    ));
}

#[test]
fn reloc_offset_out_of_blob_rejected() {
    let mut obj = sample();
    obj.relocations[0].offset = 5; // 5 + 4 > blob len 7
    let bytes = obj.to_bytes();
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("relocation outside blob"))
    ));
}

#[test]
fn defined_symbol_with_bad_blob_rejected() {
    let mut obj = sample();
    obj.symbols[0].def = SymbolDef::Defined { blob: 7 };
    let bytes = obj.to_bytes();
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("symbol blob index out of range"))
    ));
}

#[test]
fn huge_wire_count_is_rejected_without_allocating() {
    let mut bytes = sample().to_bytes();
    // string count is the first u32 after the 11-byte header
    // (magic 3 + version 2 + arch 1 + flags 1 + crc 4)
    bytes[11..15].copy_from_slice(&u32::MAX.to_le_bytes());
    crate::formats::crc32::stamp_crc(&mut bytes, 7);
    assert!(ObjectFile::from_bytes(&bytes).is_err());
}

#[test]
fn unicode_symbol_names_survive() {
    let mut obj = sample();
    obj.symbols[0].name = "иди_в_конец".into();
    let bytes = obj.to_bytes();
    let back = ObjectFile::from_bytes(&bytes).unwrap();
    assert_eq!(back.symbols[0].name, "иди_в_конец");
}

fn sample_v3_sigs() -> ObjectFile {
    let mut obj = sample();
    obj.signatures = Some(vec![RoutineSig {
        arity: 2,
        cardinalities: vec![3, 128],
    }]);
    obj.table_blobs = Some(vec![vec![2, 1, 0, 1, 0x7F]]);
    obj
}

#[test]
fn v3_signatures_and_tables_round_trip() {
    let obj = sample_v3_sigs();
    let bytes = obj.to_bytes();
    assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
    assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
}

#[test]
fn v3_signature_arity_bounds_enforced() {
    for bad_arity in [0u8, 17] {
        let mut obj = sample_v3_sigs();
        obj.signatures = Some(vec![RoutineSig {
            arity: bad_arity,
            cardinalities: vec![3; bad_arity as usize],
        }]);
        assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
    }
}

#[test]
fn v3_zero_cardinality_rejected() {
    let mut obj = sample_v3_sigs();
    obj.signatures = Some(vec![RoutineSig {
        arity: 1,
        cardinalities: vec![0],
    }]);
    assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
}

/// The two flag-gated v3 sections are independent: a signatures-only
/// object (no table blobs) round-trips.
#[test]
fn v3_signatures_only_round_trips() {
    let mut obj = sample_v3_sigs();
    obj.table_blobs = None;
    let bytes = obj.to_bytes();
    assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
    assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
}

#[test]
fn v2_file_still_loads_with_empty_v3_fields() {
    let v2 = sample();
    let back = ObjectFile::from_bytes(&v2.to_bytes()).unwrap();
    assert!(back.signatures.is_none() && back.table_blobs.is_none());
    assert!(back.table_fixups.is_empty() && back.bound_calls.is_empty());
}

fn sample_v3_full() -> ObjectFile {
    let mut obj = sample_v3_sigs();
    obj.table_fixups = vec![TableFixup {
        blob: 0,
        offset: 2,
        table_offset: 0,
    }];
    obj.bound_calls = vec![BoundCall {
        blob: 0,
        offset: 1,
        symbol: 0,
        binding: vec![TapeBinding {
            caller_tape: 2,
            param: None,
            // A map with pairs is a written map — and one v3 stores
            // completely, which is why it is not a v4 trigger.
            map_written: true,
            open: false,
            pairs: vec![
                MapPair {
                    src: 1,
                    dst: 3,
                    dst_label: None,
                    one_way: false,
                },
                MapPair {
                    src: 4,
                    dst: 0,
                    dst_label: None,
                    one_way: true,
                }, // '^' => blank
            ],
        }],
        exits: Vec::new(),
    }];
    obj
}

#[test]
fn v3_full_round_trip_preserves_one_way() {
    let obj = sample_v3_full();
    assert!(!obj.is_v4_shape(), "a written map with pairs is v3 content");
    let bytes = obj.to_bytes();
    assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
    let back = ObjectFile::from_bytes(&bytes).unwrap();
    assert_eq!(back, obj);
    assert!(back.bound_calls[0].binding[0].pairs[1].one_way);
    assert!(!back.bound_calls[0].binding[0].pairs[0].one_way);
}

#[test]
fn v3_bound_call_indices_validated() {
    // blob out of range
    let mut obj = sample_v3_full();
    obj.bound_calls[0].blob = 99;
    assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
    // symbol out of range
    let mut obj = sample_v3_full();
    obj.bound_calls[0].symbol = 99;
    assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
    // caller_tape >= 16
    let mut obj = sample_v3_full();
    obj.bound_calls[0].binding[0].caller_tape = 16;
    assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
}

#[test]
fn v3_fixup_indices_validated() {
    let mut obj = sample_v3_full();
    obj.table_fixups[0].blob = 99;
    assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
}

#[test]
fn v3_fixup_without_tables_rejected() {
    // A fixup rebases into its blob's table blob; with no table section
    // there is nothing to rebase into, so the object is malformed.
    let mut obj = sample_v3_full();
    obj.table_blobs = None;
    assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
}

#[test]
fn v3_bound_call_offset_overrun_rejected() {
    // The offset's 4-byte hole must satisfy offset..offset + 4 <= blob len.
    // offset == len - 3 leaves the hole overrunning the blob, so it is
    // rejected — while the sample's in-bounds offset still round-trips.
    let mut obj = sample_v3_full();
    let blob_len = obj.blobs[obj.bound_calls[0].blob as usize].len() as u32;
    obj.bound_calls[0].offset = blob_len - 3;
    assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
    assert!(ObjectFile::from_bytes(&sample_v3_full().to_bytes()).is_ok());
}

#[test]
fn v3_pair_reserved_flags_rejected() {
    // Hand-corrupt the pair-flags byte to set a reserved bit, restamp CRC.
    let obj = sample_v3_full();
    let mut bytes = obj.to_bytes();
    // The LAST pair-flags byte in the file is the final byte before nothing
    // else follows it in this sample (bound_calls is the last section and
    // the one-way pair is its last pair): flags byte == 0x01 at the end.
    let pos = bytes.len() - 1;
    assert_eq!(
        bytes[pos], 0x01,
        "layout assumption: trailing one-way flag byte"
    );
    bytes[pos] = 0x03; // set a reserved bit
    crate::formats::crc32::stamp_crc(&mut bytes, 7);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("reserved map-pair flags"))
    ));
}

/// A byte string captured from the pre-variant-tags encoder, pinned so
/// adding `variants`/`program_volatile` to `ObjectFile` cannot shift a
/// single byte for an object that leaves both fields at their defaults
/// (`None`/`false`) — no shape drift for tag-free objects, ever.
#[rustfmt::skip]
const V3_FULL_LEGACY_BYTES: [u8; 155] = [
    0x4d, 0x4f, 0x01, 0x03, 0x00, 0x01, 0x06, 0x9a, 0x8d, 0x41, 0x37, 0x02,
    0x00, 0x00, 0x00, 0x04, 0x00, 0x6d, 0x61, 0x69, 0x6e, 0x07, 0x00, 0x67,
    0x6f, 0x54, 0x6f, 0x45, 0x6e, 0x64, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
    0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00,
    0x0d, 0x0b, 0x00, 0x00, 0x00, 0x00, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
    0x03, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
    0x02, 0x01, 0x00, 0x01, 0x7f, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x02, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00,
    0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];

#[test]
fn variants_absent_stays_byte_identical_to_pre_variant_encoding() {
    let obj = sample_v3_full();
    assert!(obj.variants.is_none() && !obj.program_volatile);
    assert_eq!(obj.to_bytes(), V3_FULL_LEGACY_BYTES);
}

#[test]
fn variants_and_program_volatile_round_trip() {
    let mut obj = sample_v3_full();
    obj.variants = Some(vec![BlobVariant::Both]); // one blob in sample_v3_full
    obj.program_volatile = true;
    let bytes = obj.to_bytes();
    assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
}

#[test]
fn multi_blob_variants_round_trip() {
    let mut obj = sample();
    obj.blobs.push(vec![0x0D, 0x02]); // ent, stp
    obj.symbols.push(Symbol {
        name: "helper".into(),
        def: SymbolDef::Local { blob: 1 },
    });
    obj.variants = Some(vec![BlobVariant::Normal, BlobVariant::Volatile]);
    obj.program_volatile = true;
    let bytes = obj.to_bytes();
    assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
    assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
}

/// One blob per tag, exercising all three `BlobVariant` values in a
/// single object alongside `program_volatile`.
#[test]
fn all_three_variant_tags_round_trip() {
    let mut obj = sample();
    obj.blobs.push(vec![0x0D, 0x02]); // ent, stp
    obj.symbols.push(Symbol {
        name: "helper_a".into(),
        def: SymbolDef::Local { blob: 1 },
    });
    obj.blobs.push(vec![0x0D, 0x02]); // ent, stp
    obj.symbols.push(Symbol {
        name: "helper_b".into(),
        def: SymbolDef::Local { blob: 2 },
    });
    obj.variants = Some(vec![
        BlobVariant::Normal,
        BlobVariant::Volatile,
        BlobVariant::Both,
    ]);
    obj.program_volatile = true;
    let bytes = obj.to_bytes();
    assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
    assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
}

/// Legacy bytes (no variants flag) decode to `None`/`false`, not a
/// stringly-typed "no variants" marker.
#[test]
fn legacy_object_decodes_to_no_variants() {
    let obj = sample_v3_full();
    let back = ObjectFile::from_bytes(&obj.to_bytes()).unwrap();
    assert_eq!(back.variants, None);
    assert!(!back.program_volatile);
}

#[test]
fn program_volatile_without_variants_round_trips() {
    // The two new fields are independent: the header bit can be set with
    // no per-blob variant section present at all.
    let mut obj = sample();
    obj.program_volatile = true;
    let bytes = obj.to_bytes();
    assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
    let back = ObjectFile::from_bytes(&bytes).unwrap();
    assert_eq!(back, obj);
}

#[test]
fn variants_length_mismatch_rejected() {
    // Build a VALID object (`variants.len() == blobs.len()`, so the
    // encoder's `debug_assert_eq!` doesn't fire) and hand-corrupt the
    // on-wire count to disagree with the blob count — the shape a
    // decoder actually has to reject; the encoder can no longer produce
    // it directly now that it asserts the invariant on the way out.
    let mut obj = sample(); // 1 blob
    obj.variants = Some(vec![BlobVariant::Normal]);
    assert!(obj.table_fixups.is_empty() && obj.bound_calls.is_empty());
    let mut bytes = obj.to_bytes();
    assert_eq!(&bytes[bytes.len() - 8..], [0, 0, 0, 0, 0, 0, 0, 0]); // both trailing counts = 0
    let count_pos = bytes.len() - 9 - 4; // the section's u32 count, right before its one tag byte
    assert_eq!(&bytes[count_pos..count_pos + 4], &1u32.to_le_bytes());
    bytes[count_pos..count_pos + 4].copy_from_slice(&2u32.to_le_bytes()); // claims 2 tags, blob_count is 1
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("variants section length mismatch"))
    ));
}

#[test]
fn variants_bad_tag_byte_rejected() {
    // `sample()` has no signatures/table_blobs and empty table_fixups/
    // bound_calls, so its v3 encoding ends with the variants section
    // (`u32 count = 1` + one tag byte) immediately followed by the two
    // unconditional trailing counts, both zero: the tag byte sits at a
    // fixed, structurally-known offset from the end (len - 4 - 4 - 1),
    // with no risk of an accidental byte collision elsewhere in the file.
    let mut obj = sample(); // 1 blob
    obj.variants = Some(vec![BlobVariant::Normal]);
    assert!(obj.table_fixups.is_empty() && obj.bound_calls.is_empty());
    let mut bytes = obj.to_bytes();
    assert_eq!(&bytes[bytes.len() - 8..], [0, 0, 0, 0, 0, 0, 0, 0]); // both trailing counts = 0
    let tag_pos = bytes.len() - 9;
    assert_eq!(bytes[tag_pos], 0); // BlobVariant::Normal's tag byte
    bytes[tag_pos] = 3; // outside 0..=2
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("unknown blob variant tag"))
    ));
}

#[test]
fn pre_v3_object_claiming_flag_has_variants_rejected() {
    let mut bytes = sample().to_bytes(); // v2 shape
    assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 2);
    bytes[6] |= FLAG_HAS_VARIANTS; // flags byte (magic 3 + version 2 + arch 1)
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("v3 flags in pre-v3 object"))
    ));
}

#[test]
fn pre_v3_object_claiming_flag_program_volatile_rejected() {
    let mut bytes = sample().to_bytes(); // v2 shape
    bytes[6] |= FLAG_PROGRAM_VOLATILE; // flags byte
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("v3 flags in pre-v3 object"))
    ));
}

fn v4_sample() -> ObjectFile {
    let mut obj = sample();
    obj.signatures = Some(vec![RoutineSig {
        arity: 1,
        cardinalities: vec![4],
    }]);
    obj.interface = Some(Interface {
        routines: vec![RoutineInterface {
            params: vec!["num".into()],
            glyphs: vec![vec!["_".into(), "0".into(), "1".into(), "$".into()]],
            writes: vec![vec!["0".into(), "1".into()]],
            enters: vec![None],
            leaves: vec![None],
            opaque: vec![false],
            exits: 0,
            returns: true,
        }],
        alphabets: vec![ExportedAlphabet {
            // `#` is in no routine's glyph list: an exported alphabet is
            // its own list, so this is the one glyph that pins the
            // writer's interning of THAT list.
            name: "bits".into(),
            glyphs: vec!["_".into(), "0".into(), "1".into(), "#".into()],
        }],
        graphs: vec![ExportedGraph {
            name: "lib::findA".into(),
            digest: 0xDEAD_BEEF,
        }],
    });
    obj.grafts = vec![GraftProvenance {
        graph: "other::g".into(),
        digest: 0x1234_5678,
    }];
    obj
}

#[test]
fn v4_shape_is_detected_and_v3_shape_stays_v3() {
    assert!(!sample().is_v4_shape());
    assert!(v4_sample().is_v4_shape());
    let mut symbolic = sample();
    symbolic.bound_calls.push(BoundCall {
        blob: 0,
        offset: 1,
        symbol: 0,
        binding: vec![TapeBinding {
            caller_tape: 0,
            param: Some("num".into()),
            map_written: false,
            open: false,
            pairs: Vec::new(),
        }],
        exits: Vec::new(),
    });
    assert!(symbolic.is_v4_shape(), "a named binding entry needs v4");
}

/// The v4 flag bit is its own: overlapping an existing one would change
/// how a v2/v3 object reads back, and `NO_STRING` reuses the all-ones
/// sentinel the blob index already spells.
#[test]
fn v4_wire_constants_do_not_collide() {
    let taken = FLAG_HAS_DEBUG
        | FLAG_HAS_SIGNATURES
        | FLAG_HAS_TABLES
        | FLAG_HAS_VARIANTS
        | FLAG_PROGRAM_VOLATILE;
    assert_eq!(FLAG_HAS_INTERFACE & taken, 0);
    assert_eq!(NO_STRING, EXTERNAL_BLOB);
    assert_eq!(OBJECT_FORMAT_VERSION_V4, OBJECT_FORMAT_VERSION_V3 + 1);
}

/// One object per `is_v4_shape` trigger: each field alone is enough, so
/// dropping any one disjunct from the predicate turns a case red. The
/// `param` trigger is the one covered elsewhere — by
/// `v4_shape_is_detected_and_v3_shape_stays_v3`; the `interface` and
/// `grafts` triggers are this test's own two trailing cases (that other
/// test's `v4_sample` carries both at once, so it cannot tell them
/// apart), and trimming either leaves its disjunct unpinned.
#[test]
fn every_v4_only_field_alone_forces_v4_shape() {
    // A v3-shape bound call: positional, no labels, no exits.
    let v3_call = || BoundCall {
        blob: 0,
        offset: 1,
        symbol: 0,
        binding: vec![TapeBinding {
            caller_tape: 0,
            param: None,
            map_written: false,
            open: false,
            pairs: vec![MapPair {
                src: 1,
                dst: 2,
                dst_label: None,
                one_way: false,
            }],
        }],
        exits: Vec::new(),
    };
    let mut plain = sample();
    plain.bound_calls.push(v3_call());
    assert!(!plain.is_v4_shape(), "a positional bound call stays v3");

    let mut exits = sample();
    let mut call = v3_call();
    call.exits = vec![6]; // inside the sample blob, like any code offset
    exits.bound_calls.push(call);
    assert!(exits.is_v4_shape(), "an exit vector needs v4");

    // A written map is a v4 trigger only when it is EMPTY: with pairs
    // it says nothing the pairs do not, and v3 carries those.
    let mut written_with_pairs = sample();
    let mut call = v3_call();
    call.binding[0].map_written = true;
    written_with_pairs.bound_calls.push(call);
    assert!(
        !written_with_pairs.is_v4_shape(),
        "a written map with pairs is v3 content"
    );

    let mut written = sample();
    let mut call = v3_call();
    call.binding[0].map_written = true;
    call.binding[0].pairs.clear();
    written.bound_calls.push(call);
    assert!(written.is_v4_shape(), "a written-empty map needs v4");

    // `open` alone, deliberately without the `map_written` it implies:
    // setting both would let the `map_written` trigger carry this case
    // and leave a dropped `open` disjunct undetected.
    let mut open = sample();
    let mut call = v3_call();
    call.binding[0].open = true;
    open.bound_calls.push(call);
    assert!(open.is_v4_shape(), "an open map needs v4");

    let mut labelled = sample();
    let mut call = v3_call();
    call.binding[0].pairs[0].dst_label = Some("1".into());
    labelled.bound_calls.push(call);
    assert!(labelled.is_v4_shape(), "a glyph label needs v4");

    let mut grafts = sample();
    grafts.grafts = vec![GraftProvenance {
        graph: "other::g".into(),
        digest: 1,
    }];
    assert!(grafts.is_v4_shape(), "graft provenance needs v4");

    let mut interface = sample();
    interface.signatures = Some(vec![RoutineSig {
        arity: 1,
        cardinalities: vec![3],
    }]); // an interface describes a signed routine
    interface.interface = Some(Interface::default());
    assert!(interface.is_v4_shape(), "an interface section needs v4");
}

/// Every v4 field at once. Each string-bearing v4 field that can hold a
/// free string carries one that appears nowhere else in the object, so a
/// string the writer forgets to intern BEFORE the pool is written reads
/// back as an out-of-range index instead of quietly working.
#[test]
fn v4_full_round_trip() {
    let mut obj = v4_sample();
    let routine = &mut obj.interface.as_mut().unwrap().routines[0];
    routine.enters = vec![Some(vec!["$".into()])];
    routine.leaves = vec![Some(vec!["$".into()])];
    routine.opaque = vec![true];
    obj.bound_calls.push(BoundCall {
        blob: 0,
        offset: 1,
        symbol: 0,
        binding: vec![TapeBinding {
            caller_tape: 1,
            param: Some("ctl".into()),
            map_written: true,
            open: true,
            pairs: vec![
                MapPair {
                    src: 3,
                    dst: 0,
                    dst_label: Some("q0".into()),
                    one_way: false,
                },
                MapPair {
                    src: 4,
                    dst: 2,
                    dst_label: None,
                    one_way: true,
                },
            ],
        }],
        exits: vec![4],
    });
    let bytes = obj.to_bytes();
    assert_eq!(
        u16::from_le_bytes([bytes[3], bytes[4]]),
        OBJECT_FORMAT_VERSION_V4
    );
    assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
}

/// v3 content keeps its v3 bytes: the version dispatch picks the lowest
/// version that carries the object's content.
#[test]
fn v3_content_keeps_its_v3_bytes() {
    let bytes = sample_v3_full().to_bytes();
    assert_eq!(
        u16::from_le_bytes([bytes[3], bytes[4]]),
        OBJECT_FORMAT_VERSION_V3
    );
}

/// An object whose only v4 content is graft provenance is ALSO v2-shape
/// (no signatures, no tables, no fixups, no bound calls, no variants):
/// the dispatch has to ask `is_v4_shape` first, or the grafts leave with
/// the bytes.
#[test]
fn grafts_only_object_is_written_as_v4() {
    let mut obj = sample();
    obj.grafts = vec![GraftProvenance {
        graph: "other::g".into(),
        digest: 0x0BAD_F00D,
    }];
    assert!(obj.is_v2_shape(), "the fixture must be v2-shape as well");
    let bytes = obj.to_bytes();
    assert_eq!(
        u16::from_le_bytes([bytes[3], bytes[4]]),
        OBJECT_FORMAT_VERSION_V4
    );
    let back = ObjectFile::from_bytes(&bytes).unwrap();
    assert_eq!(back, obj);
    assert_eq!(back.grafts.len(), 1, "the grafts must survive the trip");
}

#[test]
fn interface_without_signatures_is_rejected_by_the_writer_contract() {
    let mut obj = v4_sample();
    obj.signatures = None;
    let bytes = obj.to_bytes();
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("interface without signatures"))
    ));
}

#[test]
fn interface_glyph_count_must_match_cardinality() {
    let mut obj = v4_sample();
    obj.interface.as_mut().unwrap().routines[0].glyphs[0].push("x".into());
    let bytes = obj.to_bytes();
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed(
            "interface glyph count differs from cardinality"
        ))
    ));
}

#[test]
fn writes_glyph_outside_its_alphabet_rejected() {
    let mut obj = v4_sample();
    obj.interface.as_mut().unwrap().routines[0].writes[0].push("z".into());
    let bytes = obj.to_bytes();
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("writes glyph outside its alphabet"))
    ));
}

/// The head contracts are checked against the tape's own glyph list, on
/// both clauses.
#[test]
fn contract_glyph_outside_its_alphabet_rejected() {
    for clause in ["enters", "leaves"] {
        let mut obj = v4_sample();
        let routine = &mut obj.interface.as_mut().unwrap().routines[0];
        let outside = vec![Some(vec!["z".into()])];
        if clause == "enters" {
            routine.enters = outside;
        } else {
            routine.leaves = outside;
        }
        let bytes = obj.to_bytes();
        assert!(
            matches!(
                ObjectFile::from_bytes(&bytes),
                Err(FormatError::Malformed(
                    "contract glyph outside its alphabet"
                ))
            ),
            "{clause} must be checked against the tape's glyphs"
        );
    }
}

#[test]
fn v4_bound_call_exit_offset_must_lie_in_blob() {
    let mut obj = v4_sample();
    obj.bound_calls.push(BoundCall {
        blob: 0,
        offset: 1,
        symbol: 0,
        binding: vec![TapeBinding {
            caller_tape: 0,
            param: None,
            map_written: false,
            open: false,
            pairs: Vec::new(),
        }],
        exits: vec![10_000],
    });
    let bytes = obj.to_bytes();
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("bound-call exit outside blob"))
    ));
}

#[test]
fn pre_v4_v3_object_claiming_flag_has_interface_rejected() {
    let mut bytes = sample_v3_full().to_bytes();
    assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 3);
    bytes[6] |= FLAG_HAS_INTERFACE; // flags byte (magic 3 + version 2 + arch 1)
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("v4 flags in pre-v4 object"))
    ));
}

#[test]
fn pre_v4_v2_object_claiming_flag_has_interface_rejected() {
    let mut bytes = sample().to_bytes();
    assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 2);
    bytes[6] |= FLAG_HAS_INTERFACE;
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("v4 flags in pre-v4 object"))
    ));
}

/// The smallest object whose v4 interface tail has a known byte layout:
/// one routine, one tape, no `writes`, an `enters` clause, no `leaves`,
/// and empty alphabet/graph/graft lists — so the per-tape flags byte and
/// the `enters` count sit at fixed offsets from the end.
fn minimal_v4_interface() -> ObjectFile {
    let mut obj = sample();
    obj.signatures = Some(vec![RoutineSig {
        arity: 1,
        cardinalities: vec![2],
    }]);
    obj.interface = Some(Interface {
        routines: vec![RoutineInterface {
            params: vec!["p".into()],
            glyphs: vec![vec!["_".into(), "x".into()]],
            writes: vec![Vec::new()],
            enters: vec![Some(vec!["x".into()])],
            leaves: vec![None],
            opaque: vec![false],
            exits: 0,
            returns: true,
        }],
        alphabets: Vec::new(),
        graphs: Vec::new(),
    });
    obj
}

/// A present-but-empty head clause never leaves the writer (the source
/// language rejects `enters {}`), so the reader's guard needs a
/// hand-built stream: patch the `enters` count byte to zero.
#[test]
fn empty_present_head_clause_rejected() {
    let mut bytes = minimal_v4_interface().to_bytes();
    // Tail, after the writes count: tape flags, enters count, one glyph
    // index, the exits and returns bytes, then the three empty counts.
    assert_eq!(
        &bytes[bytes.len() - 12..],
        [0u8; 12],
        "layout assumption: empty alphabet, graph and graft counts"
    );
    let pos = bytes.len() - 19;
    assert_eq!(bytes[pos], 1, "layout assumption: the enters glyph count");
    bytes[pos] = 0;
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("empty head clause"))
    ));
}

#[test]
fn reserved_interface_tape_flags_rejected() {
    let mut bytes = minimal_v4_interface().to_bytes();
    let pos = bytes.len() - 20;
    assert_eq!(
        bytes[pos], 0b001,
        "layout assumption: the per-tape flags byte (enters present)"
    );
    bytes[pos] = 0b1001; // bit 3 is reserved
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("reserved interface tape flags"))
    ));
}

/// The smallest v4 object whose tail is one bound call with one binding:
/// no interface section, no pairs, no exits, no grafts — so the
/// binding-flags byte sits at a fixed offset from the end.
fn minimal_v4_bound_call() -> ObjectFile {
    let mut obj = sample();
    obj.bound_calls = vec![BoundCall {
        blob: 0,
        offset: 1,
        symbol: 0,
        binding: vec![TapeBinding {
            caller_tape: 0,
            param: None,
            map_written: true, // the one v4 trigger, so the tail stays minimal
            open: false,
            pairs: Vec::new(),
        }],
        exits: Vec::new(),
    }];
    obj
}

/// A hand-crafted v4 stream can spell a binding that is OPEN with a
/// cleared `map_written` flag, or one that carries pairs with the flag
/// cleared. The writer's own invariant forbids both values
/// (docs/formats.md (bound calls)), so `from_bytes` would otherwise hand
/// back something `to_bytes` panics on. The reader normalizes.
///
/// Mutation it catches: drop the normalization and `map_written` comes
/// back false, so a caller that re-encodes the value trips the writer's
/// debug assert — a `from_bytes`/`to_bytes` asymmetry no round-trip
/// proptest can reach, because the proptest only ever generates legal
/// values.
#[test]
fn the_reader_normalizes_map_written_from_open() {
    let mut obj = minimal_v4_bound_call();
    obj.bound_calls[0].binding[0].open = true;
    let mut bytes = obj.to_bytes();
    // Same tail as `reserved_binding_flags_rejected`: binding flags,
    // pair count (u16), exit count, graft count (u32).
    let pos = bytes.len() - 8;
    assert_eq!(
        bytes[pos], 0b11,
        "layout assumption: the binding-flags byte (map written | open)"
    );
    bytes[pos] = 0b10; // open, map_written cleared — the illegal spelling
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    let back = ObjectFile::from_bytes(&bytes).expect("reads back");
    assert!(
        back.bound_calls[0].binding[0].map_written,
        "an open map is a written one"
    );
    // And the normalized value survives its own re-encoding.
    assert_eq!(
        ObjectFile::from_bytes(&back.to_bytes()).expect("re-reads"),
        back
    );
}

/// `minimal_v4_bound_call`, but the one binding carries a single written
/// pair — so the tail widens past the fixed-offset shape that helper
/// deliberately keeps minimal. A written map WITH pairs is, on its own,
/// not a v4 trigger (`is_v4_shape`'s documented rule: the pairs already
/// express it completely, and v3 stores them too) — so this binding also
/// names its parameter, which IS a v4-only field, to keep the object in
/// v4 shape independent of the map/pairs flags under test.
fn one_pair_v4_bound_call() -> ObjectFile {
    let mut obj = sample();
    obj.bound_calls = vec![BoundCall {
        blob: 0,
        offset: 1,
        symbol: 0,
        binding: vec![TapeBinding {
            caller_tape: 0,
            param: Some("p".into()),
            map_written: true,
            open: false,
            pairs: vec![MapPair {
                src: 1,
                dst: 1,
                dst_label: None,
                one_way: false,
            }],
        }],
        exits: Vec::new(),
    }];
    obj
}

/// A hand-crafted v4 stream can spell a binding that carries pairs with
/// the `map_written` flag cleared. The writer's own invariant forbids
/// that value (docs/formats.md (bound calls)), so `from_bytes` would
/// otherwise hand back something `to_bytes` panics on. The reader
/// normalizes.
///
/// Mutation it catches: drop the normalization and `map_written` comes
/// back false, so a caller that re-encodes the value trips the writer's
/// debug assert — a `from_bytes`/`to_bytes` asymmetry no round-trip
/// proptest can reach, because the proptest only ever generates legal
/// values.
#[test]
fn the_reader_normalizes_map_written_from_pairs() {
    let obj = one_pair_v4_bound_call();
    let mut bytes = obj.to_bytes();
    // Tail: binding flags, pair count (u16), one pair (src u32, dst u32,
    // flags u8 = 9 bytes), exit count, graft count (u32). The param
    // field sits before the flags byte, so it does not shift this
    // fixed-width offset from the end.
    let pos = bytes.len() - 17;
    assert_eq!(
        bytes[pos], 0b01,
        "layout assumption: the binding-flags byte (map written)"
    );
    bytes[pos] = 0b00; // pairs present, map_written cleared — the illegal spelling
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    let back = ObjectFile::from_bytes(&bytes).expect("reads back");
    assert!(
        back.bound_calls[0].binding[0].map_written,
        "a map with pairs is a written one"
    );
    // And the normalized value survives its own re-encoding.
    assert_eq!(
        ObjectFile::from_bytes(&back.to_bytes()).expect("re-reads"),
        back
    );
}

#[test]
fn reserved_binding_flags_rejected() {
    let mut bytes = minimal_v4_bound_call().to_bytes();
    // Tail: binding flags, pair count (u16), exit count, graft count (u32).
    let pos = bytes.len() - 8;
    assert_eq!(
        bytes[pos], 0b01,
        "layout assumption: the binding-flags byte (map written)"
    );
    bytes[pos] = 0b101; // bit 2 is reserved
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("reserved binding flags"))
    ));
}

/// A v4 stream setting an unassigned header flag bit was written by
/// something this reader does not understand, so it refuses the file
/// rather than decoding the parts it recognizes.
#[test]
fn reserved_object_flag_bits_rejected_in_v4() {
    let obj = minimal_v4_bound_call();
    let mut bytes = obj.to_bytes();
    assert_eq!(
        u16::from_le_bytes([bytes[3], bytes[4]]),
        OBJECT_FORMAT_VERSION_V4
    );
    assert_eq!(
        ObjectFile::from_bytes(&bytes).unwrap(),
        obj,
        "the stream must be valid but for the patched bit"
    );
    assert_eq!(bytes[6], 0, "layout assumption: the flags byte, all clear");
    bytes[6] = 0b0100_0000; // bit 6 is unassigned
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("reserved object flag bits set"))
    ));
}

/// v4 widens the map-pair flags by one bit (the glyph label) and no
/// further: bit 2 is still reserved.
#[test]
fn v4_pair_reserved_flags_rejected() {
    let mut obj = minimal_v4_bound_call();
    // Adding a pair takes the written-EMPTY map away as the v4
    // trigger, so the open marker carries the object to v4 instead.
    obj.bound_calls[0].binding[0].open = true;
    obj.bound_calls[0].binding[0].pairs.push(MapPair {
        src: 1,
        dst: 2,
        dst_label: None,
        one_way: true,
    });
    let mut bytes = obj.to_bytes();
    // Tail: pair src (u32), pair dst (u32), pair flags, exit count,
    // graft count (u32).
    let pos = bytes.len() - 6;
    assert_eq!(
        bytes[pos], 0b01,
        "layout assumption: the map-pair flags byte (one-way)"
    );
    bytes[pos] = 0b101;
    crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
    assert!(matches!(
        ObjectFile::from_bytes(&bytes),
        Err(FormatError::Malformed("reserved map-pair flags"))
    ));
}
