//! `MO` object container (spec §6.2).

use super::crc32::{stamp_crc, verify_crc};
use super::io::{Reader, put_u16, put_u32};
use super::{FORMAT_VERSION, FormatError};

pub const MAGIC_OBJECT: [u8; 3] = [b'M', b'O', 0x01];
const CRC_OFFSET: usize = 7;
const EXTERNAL_BLOB: u32 = 0xFFFF_FFFF;
const FLAG_HAS_DEBUG: u8 = 0b0000_0001;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectFile {
    pub arch: u8,
    pub symbols: Vec<Symbol>,
    pub blobs: Vec<Vec<u8>>,
    pub relocations: Vec<Relocation>,
    pub debug: Option<Vec<BlobDebug>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub def: SymbolDef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolDef {
    Defined { blob: u32 },
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
    pub blob: u32,
    pub offset: u32,
    pub symbol: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlobDebug {
    pub labels: Vec<(String, u32)>,
    pub lines: Vec<(u32, u32)>,
}

/// Build-time string pool: dedups names, hands out u32 indices.
struct StringPool {
    strings: Vec<String>,
}

impl StringPool {
    fn new() -> Self {
        Self {
            strings: Vec::new(),
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return i as u32;
        }
        self.strings.push(s.to_owned());
        (self.strings.len() - 1) as u32
    }
}

impl ObjectFile {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut pool = StringPool::new();
        let symbol_names: Vec<u32> = self.symbols.iter().map(|s| pool.intern(&s.name)).collect();
        let debug_label_names: Vec<Vec<u32>> = match &self.debug {
            Some(per_blob) => per_blob
                .iter()
                .map(|d| d.labels.iter().map(|(n, _)| pool.intern(n)).collect())
                .collect(),
            None => Vec::new(),
        };

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_OBJECT);
        put_u16(&mut out, FORMAT_VERSION);
        out.push(self.arch);
        out.push(if self.debug.is_some() {
            FLAG_HAS_DEBUG
        } else {
            0
        });
        put_u32(&mut out, 0); // crc placeholder

        put_u32(
            &mut out,
            u32::try_from(pool.strings.len()).expect("string pool fits u32"),
        );
        for s in &pool.strings {
            put_u16(&mut out, u16::try_from(s.len()).expect("string fits u16"));
            out.extend_from_slice(s.as_bytes());
        }

        put_u32(
            &mut out,
            u32::try_from(self.symbols.len()).expect("symbol count fits u32"),
        );
        for (sym, &name_idx) in self.symbols.iter().zip(&symbol_names) {
            put_u32(&mut out, name_idx);
            match sym.def {
                SymbolDef::Defined { blob } => {
                    out.push(1);
                    put_u32(&mut out, blob);
                }
                SymbolDef::External => {
                    out.push(0);
                    put_u32(&mut out, EXTERNAL_BLOB);
                }
            }
        }

        put_u32(
            &mut out,
            u32::try_from(self.blobs.len()).expect("blob count fits u32"),
        );
        for blob in &self.blobs {
            put_u32(&mut out, u32::try_from(blob.len()).expect("blob fits u32"));
            out.extend_from_slice(blob);
        }

        put_u32(
            &mut out,
            u32::try_from(self.relocations.len()).expect("relocation count fits u32"),
        );
        for reloc in &self.relocations {
            put_u32(&mut out, reloc.blob);
            put_u32(&mut out, reloc.offset);
            put_u32(&mut out, reloc.symbol);
        }

        if let Some(per_blob) = &self.debug {
            debug_assert_eq!(
                per_blob.len(),
                self.blobs.len(),
                "debug section must parallel blobs"
            );
            for (d, names) in per_blob.iter().zip(&debug_label_names) {
                put_u32(
                    &mut out,
                    u32::try_from(d.labels.len()).expect("label count fits u32"),
                );
                for ((_, offset), &name_idx) in d.labels.iter().zip(names) {
                    put_u32(&mut out, name_idx);
                    put_u32(&mut out, *offset);
                }
                put_u32(
                    &mut out,
                    u32::try_from(d.lines.len()).expect("line count fits u32"),
                );
                for (code_offset, line) in &d.lines {
                    put_u32(&mut out, *code_offset);
                    put_u32(&mut out, *line);
                }
            }
        }

        stamp_crc(&mut out, CRC_OFFSET);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < 3 {
            return Err(FormatError::Truncated);
        }
        if bytes[0..3] != MAGIC_OBJECT {
            return Err(FormatError::BadMagic);
        }
        verify_crc(bytes, CRC_OFFSET)?;

        let mut r = Reader::new(&bytes[3..]);
        let version = r.u16()?;
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let arch = r.u8()?;
        let flags = r.u8()?;
        let _crc = r.u32()?;

        let string_count = r.u32()? as usize;
        let mut strings = Vec::new();
        for _ in 0..string_count {
            let len = r.u16()? as usize;
            let raw = r.bytes(len)?;
            let s =
                std::str::from_utf8(raw).map_err(|_| FormatError::Malformed("string not utf-8"))?;
            strings.push(s.to_owned());
        }
        let name_of = |idx: u32| -> Result<String, FormatError> {
            strings
                .get(idx as usize)
                .cloned()
                .ok_or(FormatError::Malformed("string index out of range"))
        };

        let symbol_count = r.u32()? as usize;
        let mut raw_symbols = Vec::new();
        for _ in 0..symbol_count {
            let name_idx = r.u32()?;
            let kind = r.u8()?;
            let blob = r.u32()?;
            raw_symbols.push((name_idx, kind, blob));
        }

        let blob_count = r.u32()? as usize;
        let mut blobs = Vec::new();
        for _ in 0..blob_count {
            let len = r.u32()? as usize;
            blobs.push(r.bytes(len)?.to_vec());
        }

        let reloc_count = r.u32()? as usize;
        let mut relocations = Vec::new();
        for _ in 0..reloc_count {
            relocations.push(Relocation {
                blob: r.u32()?,
                offset: r.u32()?,
                symbol: r.u32()?,
            });
        }

        let debug = if flags & FLAG_HAS_DEBUG != 0 {
            let mut per_blob = Vec::new();
            for _ in 0..blob_count {
                let label_count = r.u32()? as usize;
                let mut labels = Vec::new();
                for _ in 0..label_count {
                    let name = name_of(r.u32()?)?;
                    let offset = r.u32()?;
                    labels.push((name, offset));
                }
                let line_count = r.u32()? as usize;
                let mut lines = Vec::new();
                for _ in 0..line_count {
                    lines.push((r.u32()?, r.u32()?));
                }
                per_blob.push(BlobDebug { labels, lines });
            }
            Some(per_blob)
        } else {
            None
        };

        r.finish()?;

        let mut symbols = Vec::new();
        for (name_idx, kind, blob) in raw_symbols {
            let name = name_of(name_idx)?;
            let def = match kind {
                0 => {
                    if blob != EXTERNAL_BLOB {
                        return Err(FormatError::Malformed("external symbol carries a blob"));
                    }
                    SymbolDef::External
                }
                1 => {
                    if blob as usize >= blobs.len() {
                        return Err(FormatError::Malformed("symbol blob index out of range"));
                    }
                    SymbolDef::Defined { blob }
                }
                _ => return Err(FormatError::Malformed("unknown symbol kind")),
            };
            symbols.push(Symbol { name, def });
        }

        for reloc in &relocations {
            let blob = blobs
                .get(reloc.blob as usize)
                .ok_or(FormatError::Malformed("relocation blob index out of range"))?;
            if reloc.symbol as usize >= symbols.len() {
                return Err(FormatError::Malformed(
                    "relocation symbol index out of range",
                ));
            }
            if u64::from(reloc.offset) + 4 > blob.len() as u64 {
                return Err(FormatError::Malformed("relocation outside blob"));
            }
        }

        Ok(Self {
            arch,
            symbols,
            blobs,
            relocations,
            debug,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::{ARCH_PM1, FormatError};

    fn sample() -> ObjectFile {
        ObjectFile {
            arch: ARCH_PM1,
            symbols: vec![
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
            blobs: vec![vec![0x0D, 0x0B, 0, 0, 0, 0, 0x02]],
            relocations: vec![Relocation {
                blob: 0,
                offset: 2,
                symbol: 1,
            }],
            debug: None,
        }
    }

    #[test]
    fn round_trip_without_debug() {
        let bytes = sample().to_bytes();
        assert_eq!(&bytes[0..3], b"MO\x01");
        let back = ObjectFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, sample());
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
}
