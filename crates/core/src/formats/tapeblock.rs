//! `MT` tape-block container (docs/formats.md).

use super::FormatError;
use super::crc32::{stamp_crc, verify_crc};
use super::io::{Reader, put_i64, put_u16, put_u32};

pub const MAGIC_TAPEBLOCK: [u8; 3] = [b'M', b'T', 0x01];
const CRC_OFFSET: usize = 6;

/// Shared-alphabet shape: every tape inherits the block-level `alphabet`
/// (docs/formats.md).
pub const MT_FORMAT_VERSION_V1: u16 = 1;
/// Per-tape glyph tables: at least one tape carries its own `alphabet`
/// (docs/formats.md). Emitted by `to_bytes_v2`, parsed by `from_body_v2`.
pub const MT_FORMAT_VERSION_V2: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeBlockFile {
    pub alphabet: Vec<String>,
    pub tapes: Vec<TapeSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeSnapshot {
    pub origin: i64,
    pub cells: Vec<u8>,
    pub head: i64,
    /// v2: this tape's own glyph table. `None` inherits the block-level
    /// `alphabet` (the v1 shape); `Some` triggers v2 emit (docs/formats.md).
    pub alphabet: Option<Vec<String>>,
}

impl TapeBlockFile {
    /// `true` when every tape inherits the block `alphabet` (no per-tape
    /// override) — the v1 shape that serializes byte-identical to the
    /// pre-per-tape-alphabet format.
    fn is_v1_shape(&self) -> bool {
        self.tapes.iter().all(|t| t.alphabet.is_none())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, FormatError> {
        if self.is_v1_shape() {
            self.to_bytes_v1()
        } else {
            self.to_bytes_v2()
        }
    }

    fn to_bytes_v1(&self) -> Result<Vec<u8>, FormatError> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_TAPEBLOCK);
        put_u16(&mut out, MT_FORMAT_VERSION_V1);
        out.push(0); // flags
        put_u32(&mut out, 0); // crc placeholder

        out.push(glyph_count(&self.alphabet)?);
        for glyph in &self.alphabet {
            put_u16(
                &mut out,
                u16::try_from(glyph.len()).expect("glyph fits u16"),
            );
            out.extend_from_slice(glyph.as_bytes());
        }

        out.push(u8::try_from(self.tapes.len()).expect("tape count fits u8"));
        for tape in &self.tapes {
            put_i64(&mut out, tape.origin);
            put_u32(
                &mut out,
                u32::try_from(tape.cells.len()).expect("cells fit u32"),
            );
            out.extend_from_slice(&tape.cells);
            put_i64(&mut out, tape.head);
        }

        stamp_crc(&mut out, CRC_OFFSET);
        Ok(out)
    }

    fn to_bytes_v2(&self) -> Result<Vec<u8>, FormatError> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_TAPEBLOCK);
        put_u16(&mut out, MT_FORMAT_VERSION_V2);
        out.push(0); // flags
        put_u32(&mut out, 0); // crc placeholder

        // Block-level fallback/shared alphabet.
        write_glyphs(&mut out, &self.alphabet)?;

        out.push(u8::try_from(self.tapes.len()).expect("tape count fits u8"));
        for tape in &self.tapes {
            put_i64(&mut out, tape.origin);
            put_u32(
                &mut out,
                u32::try_from(tape.cells.len()).expect("cells fit u32"),
            );
            out.extend_from_slice(&tape.cells);
            put_i64(&mut out, tape.head);
            // Per-tape glyph table: count 0 means "inherit the block alphabet".
            match &tape.alphabet {
                None => out.push(0),
                Some(own) => write_glyphs(&mut out, own)?,
            }
        }

        stamp_crc(&mut out, CRC_OFFSET);
        Ok(out)
    }

    /// v2 body (per-tape glyph tables). `r` is positioned right after the
    /// version field; this reads flags/crc, the block alphabet, and every
    /// tape's snapshot fields plus its optional own glyph table.
    fn from_body_v2(mut r: Reader<'_>) -> Result<Self, FormatError> {
        let _flags = r.u8()?;
        let _crc = r.u32()?;

        let block_count = r.u8()? as usize;
        let block_alphabet = read_glyphs(&mut r, block_count)?;

        let tape_count = r.u8()? as usize;
        if tape_count == 0 {
            return Err(FormatError::Malformed("no tapes"));
        }
        let mut tapes = Vec::with_capacity(tape_count);
        for _ in 0..tape_count {
            let origin = r.i64()?;
            let length = r.u32()? as usize;
            let cells = r.bytes(length)?.to_vec();
            let head = r.i64()?;
            let own_count = r.u8()? as usize;
            let own = if own_count == 0 {
                None
            } else {
                Some(read_glyphs(&mut r, own_count)?)
            };
            // Bounds are checked against the effective alphabet: this tape's
            // own table if present, otherwise the block fallback.
            let effective = own.as_ref().unwrap_or(&block_alphabet);
            if effective.is_empty() {
                return Err(FormatError::Malformed("empty alphabet"));
            }
            if cells.iter().any(|&c| c as usize >= effective.len()) {
                return Err(FormatError::Malformed("cell index outside alphabet"));
            }
            tapes.push(TapeSnapshot {
                origin,
                cells,
                head,
                alphabet: own,
            });
        }
        r.finish()?;

        Ok(Self {
            alphabet: block_alphabet,
            tapes,
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < 3 {
            return Err(FormatError::Truncated);
        }
        if bytes[0..3] != MAGIC_TAPEBLOCK {
            return Err(FormatError::BadMagic);
        }
        verify_crc(bytes, CRC_OFFSET)?;

        let mut r = Reader::new(&bytes[3..]);
        let version = r.u16()?;
        match version {
            MT_FORMAT_VERSION_V1 => {}
            MT_FORMAT_VERSION_V2 => return Self::from_body_v2(r),
            _ => return Err(FormatError::UnsupportedVersion(version)),
        }
        let _flags = r.u8()?;
        let _crc = r.u32()?;

        let alphabet_count = r.u8()? as usize;
        if alphabet_count == 0 {
            return Err(FormatError::Malformed("empty alphabet"));
        }
        let mut alphabet = Vec::with_capacity(alphabet_count);
        for _ in 0..alphabet_count {
            let len = r.u16()? as usize;
            let raw = r.bytes(len)?;
            let glyph =
                std::str::from_utf8(raw).map_err(|_| FormatError::Malformed("glyph not utf-8"))?;
            alphabet.push(glyph.to_owned());
        }

        let tape_count = r.u8()? as usize;
        if tape_count == 0 {
            return Err(FormatError::Malformed("no tapes"));
        }
        let mut tapes = Vec::with_capacity(tape_count);
        for _ in 0..tape_count {
            let origin = r.i64()?;
            let length = r.u32()? as usize;
            let cells = r.bytes(length)?.to_vec();
            let head = r.i64()?;
            if cells.iter().any(|&c| c as usize >= alphabet_count) {
                return Err(FormatError::Malformed("cell index outside alphabet"));
            }
            tapes.push(TapeSnapshot {
                origin,
                cells,
                head,
                alphabet: None,
            });
        }
        r.finish()?;

        Ok(Self { alphabet, tapes })
    }
}

/// Encodes an alphabet's symbol count as the wire `u8`, or
/// `FormatError::AlphabetTooWide` when the alphabet has more symbols than
/// that byte-sized field can hold (docs/formats.md (tape-block snapshot)).
fn glyph_count(glyphs: &[String]) -> Result<u8, FormatError> {
    u8::try_from(glyphs.len()).map_err(|_| FormatError::AlphabetTooWide {
        symbols: glyphs.len(),
        max: usize::from(u8::MAX),
    })
}

/// Writes a glyph table: a `u8` count, then each glyph as `u16` byte-length +
/// its UTF-8 bytes. Shared by the v2 block alphabet and per-tape own tables.
fn write_glyphs(out: &mut Vec<u8>, glyphs: &[String]) -> Result<(), FormatError> {
    out.push(glyph_count(glyphs)?);
    for glyph in glyphs {
        put_u16(out, u16::try_from(glyph.len()).expect("glyph fits u16"));
        out.extend_from_slice(glyph.as_bytes());
    }
    Ok(())
}

/// Reads `count` glyphs (each `u16` byte-length + UTF-8 bytes) into a vector,
/// rejecting non-UTF-8 payloads.
fn read_glyphs(r: &mut Reader<'_>, count: usize) -> Result<Vec<String>, FormatError> {
    let mut glyphs = Vec::with_capacity(count);
    for _ in 0..count {
        let len = r.u16()? as usize;
        let raw = r.bytes(len)?;
        let glyph =
            std::str::from_utf8(raw).map_err(|_| FormatError::Malformed("glyph not utf-8"))?;
        glyphs.push(glyph.to_owned());
    }
    Ok(glyphs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TapeBlockFile {
        TapeBlockFile {
            alphabet: vec![" ".into(), "*".into()],
            tapes: vec![TapeSnapshot {
                origin: -2,
                cells: vec![0, 1, 1, 0, 1],
                head: 1,
                alphabet: None,
            }],
        }
    }

    #[test]
    fn round_trip() {
        let bytes = sample().to_bytes().unwrap();
        assert_eq!(&bytes[0..3], b"MT\x01");
        assert_eq!(TapeBlockFile::from_bytes(&bytes).unwrap(), sample());
    }

    /// A shared-alphabet block (all tapes `alphabet: None`) serializes
    /// byte-for-byte as v1 — this pins the committed golden .pmt files.
    #[test]
    fn shared_alphabet_is_byte_identical_v1() {
        let block = sample(); // all tapes alphabet: None after this task's refactor
        let bytes = block.to_bytes().unwrap();
        assert_eq!(&bytes[0..3], b"MT\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 1);
        assert_eq!(TapeBlockFile::from_bytes(&bytes).unwrap(), block);
    }

    #[test]
    fn multi_tape_and_multibyte_glyphs() {
        let block = TapeBlockFile {
            alphabet: vec!["·".into(), "↵".into(), "★".into()],
            tapes: vec![
                TapeSnapshot {
                    origin: 0,
                    cells: vec![2, 1, 0],
                    head: 0,
                    alphabet: None,
                },
                TapeSnapshot {
                    origin: -100,
                    cells: vec![0],
                    head: -100,
                    alphabet: None,
                },
            ],
        };
        let back = TapeBlockFile::from_bytes(&block.to_bytes().unwrap()).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn cell_outside_alphabet_rejected() {
        let mut block = sample();
        block.tapes[0].cells[0] = 9;
        let bytes = block.to_bytes().unwrap();
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("cell index outside alphabet"))
        ));
    }

    #[test]
    fn empty_alphabet_rejected() {
        let block = TapeBlockFile {
            alphabet: vec![],
            tapes: sample().tapes,
        };
        let bytes = block.to_bytes().unwrap();
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("empty alphabet"))
        ));
    }

    #[test]
    fn corruption_rejected() {
        let mut bytes = sample().to_bytes().unwrap();
        bytes[12] ^= 0xFF;
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::BadCrc { .. })
        ));
    }

    #[test]
    fn no_tapes_rejected() {
        let block = TapeBlockFile {
            alphabet: vec![" ".into(), "*".into()],
            tapes: vec![],
        };
        let bytes = block.to_bytes().unwrap();
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("no tapes"))
        ));
    }

    #[test]
    fn non_utf8_glyph_rejected() {
        let mut bytes = sample().to_bytes().unwrap();
        // header is 10 bytes (magic 3 + version 2 + flags 1 + crc 4); then
        // u8 alphabet count @10, u16 glyph len @11..13, glyph bytes @13.
        bytes[13] = 0xFF; // invalidate the single-byte " " glyph
        crate::formats::crc32::stamp_crc(&mut bytes, 6);
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("glyph not utf-8"))
        ));
    }

    fn sample_v2() -> TapeBlockFile {
        TapeBlockFile {
            alphabet: vec!["_".into()], // block fallback
            tapes: vec![
                TapeSnapshot {
                    origin: 0,
                    cells: vec![0, 1, 2],
                    head: 0,
                    alphabet: Some(vec!["_".into(), "0".into(), "1".into()]),
                },
                TapeSnapshot {
                    origin: 0,
                    cells: vec![0],
                    head: 0,
                    alphabet: None, // inherits block "_"
                },
            ],
        }
    }

    /// Pins the absolute v2 byte offsets so a symmetric field transposition
    /// (e.g. head before cells, or origin/cells_len swapped) would fail.
    /// Layout: magic3 + ver2 + flags1 + crc4 + block-alphabet + tape_count1 +
    /// per-tape (origin8 + cells_len4 + cells + head8 + own-glyph-table).
    #[test]
    fn v2_layout_is_exact() {
        let bytes = sample_v2().to_bytes().unwrap();
        assert_eq!(&bytes[0..3], b"MT\x01"); // magic
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2); // version
        assert_eq!(bytes[5], 0); // flags
        // [6..10] crc
        // Block fallback alphabet ["_"].
        assert_eq!(bytes[10], 1); // block_alphabet_count
        assert_eq!(u16::from_le_bytes(bytes[11..13].try_into().unwrap()), 1); // "_" byte-len
        assert_eq!(bytes[13], b'_'); // "_" glyph
        assert_eq!(bytes[14], 2); // tape_count
        // Tape 0: cells [0,1,2], own alphabet ["_","0","1"].
        assert_eq!(i64::from_le_bytes(bytes[15..23].try_into().unwrap()), 0); // origin
        assert_eq!(u32::from_le_bytes(bytes[23..27].try_into().unwrap()), 3); // cells_len
        assert_eq!(&bytes[27..30], &[0, 1, 2]); // cells
        assert_eq!(i64::from_le_bytes(bytes[30..38].try_into().unwrap()), 0); // head
        assert_eq!(bytes[38], 3); // own_alphabet_count
        assert_eq!(u16::from_le_bytes(bytes[39..41].try_into().unwrap()), 1); // "_" byte-len
        assert_eq!(bytes[41], b'_');
        assert_eq!(u16::from_le_bytes(bytes[42..44].try_into().unwrap()), 1); // "0" byte-len
        assert_eq!(bytes[44], b'0');
        assert_eq!(u16::from_le_bytes(bytes[45..47].try_into().unwrap()), 1); // "1" byte-len
        assert_eq!(bytes[47], b'1');
        // Tape 1: cells [0], own alphabet None → inherit marker.
        assert_eq!(i64::from_le_bytes(bytes[48..56].try_into().unwrap()), 0); // origin
        assert_eq!(u32::from_le_bytes(bytes[56..60].try_into().unwrap()), 1); // cells_len
        assert_eq!(&bytes[60..61], &[0]); // cells
        assert_eq!(i64::from_le_bytes(bytes[61..69].try_into().unwrap()), 0); // head
        assert_eq!(bytes[69], 0); // own_alphabet_count == 0 (inherit block)
    }

    #[test]
    fn v2_round_trips_per_tape_alphabets() {
        let block = sample_v2();
        let bytes = block.to_bytes().unwrap();
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2);
        assert_eq!(TapeBlockFile::from_bytes(&bytes).unwrap(), block);
    }

    #[test]
    fn v2_cell_outside_own_alphabet_rejected() {
        let mut block = sample_v2();
        block.tapes[0].cells[0] = 9; // own alphabet has 3 symbols
        let bytes = block.to_bytes().unwrap();
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("cell index outside alphabet"))
        ));
    }

    #[test]
    fn v2_emoji_glyphs_survive() {
        let block = TapeBlockFile {
            alphabet: vec!["_".into()],
            tapes: vec![TapeSnapshot {
                origin: 0,
                cells: vec![0, 1],
                head: 0,
                alphabet: Some(vec!["_".into(), "😀".into()]),
            }],
        };
        assert_eq!(
            TapeBlockFile::from_bytes(&block.to_bytes().unwrap()).unwrap(),
            block
        );
    }

    #[test]
    fn v1_shared_alphabet_file_still_loads() {
        let v1 = sample(); // all None
        assert_eq!(
            TapeBlockFile::from_bytes(&v1.to_bytes().unwrap()).unwrap(),
            v1
        );
    }

    /// A block-level (v1-shaped) alphabet past the wire glyph-count byte's
    /// range is a typed error, not a panic — the u8 header field caps the
    /// table at 255 glyphs (docs/formats.md (tape-block snapshot)).
    #[test]
    fn oversize_block_alphabet_is_typed_error_not_panic() {
        let block = TapeBlockFile {
            alphabet: (0..300).map(|i| i.to_string()).collect(),
            tapes: sample().tapes,
        };
        assert_eq!(
            block.to_bytes(),
            Err(FormatError::AlphabetTooWide {
                symbols: 300,
                max: 255,
            })
        );
    }

    /// Same invariant on the v2 per-tape path: a tape's own glyph table
    /// past 255 symbols is a typed error, not a panic.
    #[test]
    fn oversize_per_tape_alphabet_is_typed_error_not_panic() {
        let block = TapeBlockFile {
            alphabet: vec!["_".into()],
            tapes: vec![TapeSnapshot {
                origin: 0,
                cells: Vec::new(),
                head: 0,
                alphabet: Some((0..300).map(|i| i.to_string()).collect()),
            }],
        };
        assert_eq!(
            block.to_bytes(),
            Err(FormatError::AlphabetTooWide {
                symbols: 300,
                max: 255,
            })
        );
    }
}
