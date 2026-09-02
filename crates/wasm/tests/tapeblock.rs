//! Tape blocks in and out: `decode` reads what core wrote, `encode` writes
//! what core reads, the MT version follows the block's shape, and every
//! precondition the codec asserts is an error here. `seeds_from_tape_block`
//! maps by glyph, not by index.

use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_wasm::inner::Lang;
use mtc_wasm::inner::program::{SeedMapError, build};
use mtc_wasm::inner::tapeblock::{
    Tape, TapeBlock, TapeBlockError, TapeBlockInput, TapeInput, decode, encode,
};

const TMC_REPLACE_B: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";
const PMC_INC: &str = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";

fn glyphs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn snapshot(cells: &[u8], head: i64, alphabet: Option<&[&str]>) -> TapeSnapshot {
    TapeSnapshot {
        origin: 0,
        cells: cells.to_vec(),
        head,
        alphabet: alphabet.map(glyphs),
    }
}

fn version(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[3], bytes[4]])
}

fn tape(cells: &[u8], glyphs_: Option<&[&str]>) -> TapeInput {
    TapeInput {
        origin: 0,
        cells: cells.to_vec(),
        head: 0,
        glyphs: glyphs_.map(glyphs),
    }
}

#[test]
fn decode_reads_a_v1_block_pmt_wrote() {
    let bytes = TapeBlockFile {
        alphabet: glyphs(&[" ", "*"]),
        tapes: vec![snapshot(&[1, 0, 1], 2, None)],
    }
    .to_bytes()
    .unwrap();
    assert_eq!(version(&bytes), 1);
    let block = decode(&bytes).unwrap();
    assert_eq!(block.alphabet, glyphs(&[" ", "*"]));
    assert_eq!(
        block.tapes,
        vec![Tape {
            origin: 0,
            cells: vec![1, 0, 1],
            head: 2,
            glyphs: glyphs(&[" ", "*"]),
        }],
        "an inheriting tape reports the block alphabet as its own"
    );
}

#[test]
fn decode_resolves_per_tape_tables_in_a_v2_block() {
    let bytes = TapeBlockFile {
        alphabet: glyphs(&["_", "a", "b"]),
        tapes: vec![
            snapshot(&[2, 1], 0, None),
            snapshot(&[1, 1, 1], 1, Some(&["_", "1"])),
        ],
    }
    .to_bytes()
    .unwrap();
    assert_eq!(version(&bytes), 2);
    let block = decode(&bytes).unwrap();
    assert_eq!(block.tapes[0].glyphs, glyphs(&["_", "a", "b"]));
    assert_eq!(block.tapes[1].glyphs, glyphs(&["_", "1"]));
    assert_eq!(block.tapes[1].head, 1);
}

#[test]
fn decode_refuses_bad_bytes_with_the_codec_message() {
    let bytes = TapeBlockFile {
        alphabet: glyphs(&[" ", "*"]),
        tapes: vec![snapshot(&[1], 0, None)],
    }
    .to_bytes()
    .unwrap();
    let mut flipped = bytes.clone();
    *flipped.last_mut().unwrap() ^= 1;
    assert!(
        matches!(decode(&flipped), Err(TapeBlockError::Format(_))),
        "CRC"
    );
    let mut magic = bytes.clone();
    magic[0] = b'X';
    assert!(
        matches!(decode(&magic), Err(TapeBlockError::Format(_))),
        "magic"
    );
    assert!(
        matches!(decode(&[]), Err(TapeBlockError::Format(_))),
        "empty"
    );
    assert!(
        matches!(decode(b"MX\x01rest"), Err(TapeBlockError::Format(_))),
        "another container"
    );
}

#[test]
fn encode_writes_v1_when_every_tape_inherits() {
    // No block alphabet given: the first tape's glyphs become it.
    let bytes = encode(&TapeBlockInput {
        alphabet: None,
        tapes: vec![tape(&[1, 0, 1], Some(&[" ", "*"]))],
    })
    .unwrap();
    assert_eq!(version(&bytes), 1);
    let file = TapeBlockFile::from_bytes(&bytes).unwrap();
    assert_eq!(file.alphabet, glyphs(&[" ", "*"]));
    assert_eq!(file.tapes[0].alphabet, None, "equal to the block: inherits");
    assert_eq!(file.tapes[0].cells, vec![1, 0, 1]);
    // ... byte-identical to what pmt writes for that shape.
    let pmt = TapeBlockFile {
        alphabet: glyphs(&[" ", "*"]),
        tapes: vec![snapshot(&[1, 0, 1], 0, None)],
    }
    .to_bytes()
    .unwrap();
    assert_eq!(bytes, pmt);
}

#[test]
fn encode_writes_v2_when_a_tape_has_its_own_table() {
    let bytes = encode(&TapeBlockInput {
        alphabet: Some(glyphs(&["_", "a", "b"])),
        tapes: vec![
            tape(&[2, 1], None),
            tape(&[1, 1], Some(&["_", "1"])),
            tape(&[0], Some(&["_", "a", "b"])),
        ],
    })
    .unwrap();
    assert_eq!(version(&bytes), 2);
    let file = TapeBlockFile::from_bytes(&bytes).unwrap();
    assert_eq!(file.tapes[0].alphabet, None);
    assert_eq!(file.tapes[1].alphabet, Some(glyphs(&["_", "1"])));
    assert_eq!(file.tapes[2].alphabet, None, "equal to the block: inherits");
}

#[test]
fn encode_of_a_decode_is_the_same_block() {
    let original = TapeBlockFile {
        alphabet: glyphs(&["_", "a", "b"]),
        tapes: vec![
            snapshot(&[2, 1], 0, None),
            snapshot(&[1, 1, 1], -3, Some(&["_", "1"])),
        ],
    }
    .to_bytes()
    .unwrap();
    let block = decode(&original).unwrap();
    let again = encode(&TapeBlockInput {
        alphabet: Some(block.alphabet.clone()),
        tapes: block
            .tapes
            .iter()
            .map(|t| TapeInput {
                origin: t.origin,
                cells: t.cells.clone(),
                head: t.head,
                glyphs: Some(t.glyphs.clone()),
            })
            .collect(),
    })
    .unwrap();
    assert_eq!(decode(&again).unwrap(), block);
    assert_eq!(again, original, "this shape round-trips byte for byte");
}

#[test]
fn encode_refuses_every_shape_the_codec_would_assert_on() {
    let err = |input: TapeBlockInput| encode(&input).unwrap_err();
    assert_eq!(
        err(TapeBlockInput {
            alphabet: None,
            tapes: vec![tape(&[0], None)],
        }),
        TapeBlockError::NoAlphabet
    );
    assert_eq!(
        err(TapeBlockInput {
            alphabet: Some(glyphs(&["_"])),
            tapes: vec![],
        }),
        TapeBlockError::NoTapes
    );
    assert_eq!(
        err(TapeBlockInput {
            alphabet: Some(glyphs(&["_", "a"])),
            tapes: vec![tape(&[0, 2], None)],
        }),
        TapeBlockError::CellOutsideAlphabet {
            tape: 0,
            index: 2,
            width: 2,
        }
    );
    assert_eq!(
        err(TapeBlockInput {
            alphabet: Some(glyphs(&["_", "a"])),
            tapes: vec![tape(&[0], Some(&["_"])), tape(&[1], Some(&["_"]))],
        }),
        TapeBlockError::CellOutsideAlphabet {
            tape: 1,
            index: 1,
            width: 1,
        },
        "validated against the tape's OWN table"
    );
    assert_eq!(
        err(TapeBlockInput {
            alphabet: Some(vec![]),
            tapes: vec![tape(&[], None)],
        }),
        TapeBlockError::EmptyAlphabet { tape: None }
    );
    assert_eq!(
        err(TapeBlockInput {
            alphabet: Some(glyphs(&["_"])),
            tapes: vec![tape(&[], Some(&[]))],
        }),
        TapeBlockError::EmptyAlphabet { tape: Some(0) }
    );
    assert_eq!(
        err(TapeBlockInput {
            alphabet: Some(glyphs(&["_"])),
            tapes: vec![tape(&[], None); 256],
        }),
        TapeBlockError::TooManyTapes(256)
    );
    let wide: Vec<String> = (0..256).map(|i| i.to_string()).collect();
    assert_eq!(
        err(TapeBlockInput {
            alphabet: Some(wide),
            tapes: vec![tape(&[], None)],
        }),
        TapeBlockError::AlphabetTooWide {
            tape: None,
            symbols: 256,
        }
    );
    let long = "x".repeat(65536);
    assert_eq!(
        err(TapeBlockInput {
            alphabet: Some(glyphs(&["_"])),
            tapes: vec![tape(&[], Some(&["_", &long]))],
        }),
        TapeBlockError::GlyphTooLong {
            tape: Some(0),
            glyph: long,
        }
    );
    // The messages name the owner.
    assert!(
        TapeBlockError::AlphabetTooWide {
            tape: Some(3),
            symbols: 300
        }
        .to_string()
        .contains("tape 3")
    );
}

#[test]
fn seeds_map_by_glyph_not_by_index() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    // The block spells the alphabet in another order: its 1 is `b`.
    let block = TapeBlock {
        alphabet: glyphs(&["_", "b", "a"]),
        tapes: vec![Tape {
            origin: -1,
            cells: vec![1, 1, 2, 0],
            head: 1,
            glyphs: glyphs(&["_", "b", "a"]),
        }],
    };
    let seeds = program.seeds_from_tape_block(&block).unwrap();
    assert_eq!(seeds.len(), 1);
    assert_eq!(
        seeds[0].cells,
        vec![2, 2, 1, 0],
        "b→2, a→1 in the program's alphabet"
    );
    assert_eq!((seeds[0].origin, seeds[0].head), (-1, 1));

    let (pm, _) = build(Lang::Pmc, PMC_INC, 1).unwrap();
    let block = decode(
        &TapeBlockFile {
            alphabet: glyphs(&[" ", "*"]),
            tapes: vec![snapshot(&[1, 0, 1], 2, None)],
        }
        .to_bytes()
        .unwrap(),
    )
    .unwrap();
    let seeds = pm.seeds_from_tape_block(&block).unwrap();
    assert_eq!(seeds[0].cells, vec![1, 0, 1]);
    assert_eq!(seeds[0].head, 2);
}

#[test]
fn seeds_refuse_unknown_glyphs_and_extra_tapes() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    let one = |cells: Vec<u8>, g: &[&str]| Tape {
        origin: 0,
        cells,
        head: 0,
        glyphs: glyphs(g),
    };
    let err = program
        .seeds_from_tape_block(&TapeBlock {
            alphabet: glyphs(&["_", "x"]),
            tapes: vec![one(vec![0, 1], &["_", "x"])],
        })
        .unwrap_err();
    assert_eq!(
        err,
        SeedMapError::UnknownGlyph {
            tape: 0,
            glyph: "x".to_string(),
            band: "main".to_string(),
        }
    );
    assert!(err.to_string().contains("`x`") && err.to_string().contains("`main`"));
    let err = program
        .seeds_from_tape_block(&TapeBlock {
            alphabet: glyphs(&["_"]),
            tapes: vec![one(vec![0], &["_"]), one(vec![0], &["_"])],
        })
        .unwrap_err();
    assert_eq!(err, SeedMapError::TooManyTapes { given: 2, bands: 1 });
    // A block naming only glyphs the program knows maps, whatever its width.
    let seeds = program
        .seeds_from_tape_block(&TapeBlock {
            alphabet: glyphs(&["_", "b"]),
            tapes: vec![one(vec![1, 1], &["_", "b"])],
        })
        .unwrap();
    assert_eq!(seeds[0].cells, vec![2, 2]);
}
