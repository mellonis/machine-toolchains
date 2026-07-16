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
/// (docs/formats.md). Emit/parse lands in a later task.
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

    pub fn to_bytes(&self) -> Vec<u8> {
        if self.is_v1_shape() {
            self.to_bytes_v1()
        } else {
            self.to_bytes_v2()
        }
    }

    fn to_bytes_v1(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_TAPEBLOCK);
        put_u16(&mut out, MT_FORMAT_VERSION_V1);
        out.push(0); // flags
        put_u32(&mut out, 0); // crc placeholder

        out.push(u8::try_from(self.alphabet.len()).expect("alphabet fits u8"));
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
        out
    }

    fn to_bytes_v2(&self) -> Vec<u8> {
        unimplemented!("MT v2 emit lands in task 5")
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
            // v2 (per-tape glyph tables) parse lands in a later task.
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
        let bytes = sample().to_bytes();
        assert_eq!(&bytes[0..3], b"MT\x01");
        assert_eq!(TapeBlockFile::from_bytes(&bytes).unwrap(), sample());
    }

    /// A shared-alphabet block (all tapes `alphabet: None`) serializes
    /// byte-for-byte as v1 — this pins the committed golden .pmt files.
    #[test]
    fn shared_alphabet_is_byte_identical_v1() {
        let block = sample(); // all tapes alphabet: None after this task's refactor
        let bytes = block.to_bytes();
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
        let back = TapeBlockFile::from_bytes(&block.to_bytes()).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn cell_outside_alphabet_rejected() {
        let mut block = sample();
        block.tapes[0].cells[0] = 9;
        let bytes = block.to_bytes();
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
        let bytes = block.to_bytes();
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("empty alphabet"))
        ));
    }

    #[test]
    fn corruption_rejected() {
        let mut bytes = sample().to_bytes();
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
        let bytes = block.to_bytes();
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("no tapes"))
        ));
    }

    #[test]
    fn non_utf8_glyph_rejected() {
        let mut bytes = sample().to_bytes();
        // header is 10 bytes (magic 3 + version 2 + flags 1 + crc 4); then
        // u8 alphabet count @10, u16 glyph len @11..13, glyph bytes @13.
        bytes[13] = 0xFF; // invalidate the single-byte " " glyph
        crate::formats::crc32::stamp_crc(&mut bytes, 6);
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("glyph not utf-8"))
        ));
    }
}
