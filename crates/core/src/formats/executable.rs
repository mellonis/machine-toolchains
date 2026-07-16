//! `MX` executable container (docs/formats.md).

use super::FormatError;
use super::PROFILE_BASE;
use super::crc32::{stamp_crc, verify_crc};
use super::io::{Reader, put_u16, put_u32};

pub const MAGIC_EXECUTABLE: [u8; 3] = [b'M', b'X', 0x01];
const CRC_OFFSET: usize = 7;

pub const MX_FORMAT_VERSION_V1: u16 = 1;
pub const MX_FORMAT_VERSION_V2: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executable {
    pub arch: u8,
    pub entry: u32,
    pub code: Vec<u8>,
    /// v2 header fields; the v1 code-only shape leaves them at defaults
    /// (`tape_count: 1`, `profile: PROFILE_BASE`, empty cardinalities,
    /// empty tables) and serializes as version 1 (docs/formats.md).
    pub tape_count: u8,
    pub profile: u8,
    pub alphabet_cardinalities: Vec<u32>,
    pub tables: Vec<u8>,
}

impl Executable {
    /// A version-1 code-only image (the shape PM-1 emits).
    pub fn code_only(arch: u8, entry: u32, code: Vec<u8>) -> Self {
        Self {
            arch,
            entry,
            code,
            tape_count: 1,
            profile: PROFILE_BASE,
            alphabet_cardinalities: Vec::new(),
            tables: Vec::new(),
        }
    }

    /// A version-2 sectioned image (the shape TM-1 emits).
    #[allow(clippy::too_many_arguments)]
    pub fn sectioned(
        arch: u8,
        entry: u32,
        code: Vec<u8>,
        tables: Vec<u8>,
        tape_count: u8,
        profile: u8,
        alphabet_cardinalities: Vec<u32>,
    ) -> Self {
        Self {
            arch,
            entry,
            code,
            tape_count,
            profile,
            alphabet_cardinalities,
            tables,
        }
    }

    /// True when the image carries no v2-only data and must serialize as v1.
    fn is_v1_shape(&self) -> bool {
        self.tape_count == 1
            && self.profile == PROFILE_BASE
            && self.alphabet_cardinalities.is_empty()
            && self.tables.is_empty()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        if self.is_v1_shape() {
            self.to_bytes_v1()
        } else {
            self.to_bytes_v2()
        }
    }

    fn to_bytes_v1(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(19 + self.code.len());
        out.extend_from_slice(&MAGIC_EXECUTABLE);
        put_u16(&mut out, MX_FORMAT_VERSION_V1);
        out.push(self.arch);
        out.push(0); // flags
        put_u32(&mut out, 0); // crc placeholder
        put_u32(&mut out, self.entry);
        put_u32(
            &mut out,
            u32::try_from(self.code.len()).expect("code fits u32"),
        );
        out.extend_from_slice(&self.code);
        stamp_crc(&mut out, CRC_OFFSET);
        out
    }

    fn to_bytes_v2(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_EXECUTABLE);
        put_u16(&mut out, MX_FORMAT_VERSION_V2);
        out.push(self.arch);
        out.push(0); // flags
        put_u32(&mut out, 0); // crc placeholder
        out.push(self.tape_count);
        out.push(self.profile);
        put_u32(&mut out, self.entry);
        put_u32(
            &mut out,
            u32::try_from(self.code.len()).expect("code fits u32"),
        );
        put_u32(
            &mut out,
            u32::try_from(self.tables.len()).expect("tables fit u32"),
        );
        for &card in &self.alphabet_cardinalities {
            put_u32(&mut out, card);
        }
        out.extend_from_slice(&self.code);
        out.extend_from_slice(&self.tables);
        stamp_crc(&mut out, CRC_OFFSET);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < 3 {
            return Err(FormatError::Truncated);
        }
        if bytes[0..3] != MAGIC_EXECUTABLE {
            return Err(FormatError::BadMagic);
        }
        verify_crc(bytes, CRC_OFFSET)?;

        let mut r = Reader::new(&bytes[3..]);
        let version = r.u16()?;
        match version {
            MX_FORMAT_VERSION_V1 => {
                let arch = r.u8()?;
                let _flags = r.u8()?;
                let _crc = r.u32()?;
                let entry = r.u32()?;
                let code_size = r.u32()? as usize;
                let code = r.bytes(code_size)?.to_vec();
                r.finish()?;
                if entry as usize >= code.len() {
                    return Err(FormatError::Malformed("entry offset outside code"));
                }
                Ok(Self::code_only(arch, entry, code))
            }
            MX_FORMAT_VERSION_V2 => {
                let arch = r.u8()?;
                let _flags = r.u8()?;
                let _crc = r.u32()?;
                let tape_count = r.u8()?;
                if tape_count == 0 || tape_count > 16 {
                    return Err(FormatError::Malformed("tape count out of range"));
                }
                let profile = r.u8()?;
                let entry = r.u32()?;
                let code_size = r.u32()? as usize;
                let table_size = r.u32()? as usize;
                let mut alphabet_cardinalities = Vec::with_capacity(tape_count as usize);
                for _ in 0..tape_count {
                    alphabet_cardinalities.push(r.u32()?);
                }
                let code = r.bytes(code_size)?.to_vec();
                let tables = r.bytes(table_size)?.to_vec();
                r.finish()?;
                if entry as usize >= code.len() {
                    return Err(FormatError::Malformed("entry offset outside code"));
                }
                Ok(Self::sectioned(
                    arch,
                    entry,
                    code,
                    tables,
                    tape_count,
                    profile,
                    alphabet_cardinalities,
                ))
            }
            other => Err(FormatError::UnsupportedVersion(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::{ARCH_PM1, ARCH_TM1, FormatError, PROFILE_FRAMES};

    fn sample() -> Executable {
        Executable::code_only(ARCH_PM1, 0, vec![0x0D, 0x05, 0x02]) // ent, rgt, stp
    }

    #[test]
    fn round_trip() {
        let bytes = sample().to_bytes();
        let back = Executable::from_bytes(&bytes).unwrap();
        assert_eq!(back.arch, ARCH_PM1);
        assert_eq!(back.entry, 0);
        assert_eq!(back.code, vec![0x0D, 0x05, 0x02]);
    }

    #[test]
    fn layout_is_exact() {
        let bytes = sample().to_bytes();
        assert_eq!(&bytes[0..3], b"MX\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 1); // version
        assert_eq!(bytes[5], ARCH_PM1);
        assert_eq!(bytes[6], 0); // flags
        // [7..11] crc, [11..15] entry, [15..19] code size
        assert_eq!(u32::from_le_bytes(bytes[15..19].try_into().unwrap()), 3);
        assert_eq!(bytes.len(), 19 + 3);
    }

    #[test]
    fn corruption_is_rejected_before_decode() {
        let mut bytes = sample().to_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::BadCrc { .. })
        ));
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut bytes = sample().to_bytes();
        bytes[1] = b'Z';
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::BadMagic)
        ));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut bytes = sample().to_bytes();
        bytes[3] = 9; // version 9
        crate::formats::crc32::stamp_crc(&mut bytes, 7);
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::UnsupportedVersion(9))
        ));
    }

    #[test]
    fn entry_outside_code_is_rejected() {
        let mut exe = sample();
        exe.entry = 99;
        let bytes = exe.to_bytes();
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::Malformed("entry offset outside code"))
        ));
    }

    /// The v1 code-only shape must serialize byte-for-byte as before the
    /// v2 refactor — this pins PM-1's .pmx output.
    #[test]
    fn code_only_is_byte_identical_v1() {
        let exe = Executable::code_only(ARCH_PM1, 0, vec![0x0D, 0x05, 0x02]);
        let bytes = exe.to_bytes();
        // magic + version(1) + arch + flags + crc(4) + entry(4) + size(4) + code(3)
        assert_eq!(&bytes[0..3], b"MX\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 1);
        assert_eq!(bytes[5], ARCH_PM1);
        assert_eq!(bytes[6], 0);
        assert_eq!(u32::from_le_bytes(bytes[15..19].try_into().unwrap()), 3);
        assert_eq!(bytes.len(), 19 + 3);
        assert_eq!(Executable::from_bytes(&bytes).unwrap(), exe);
    }

    #[test]
    fn truncated_and_trailing_are_rejected() {
        let bytes = sample().to_bytes();
        assert!(matches!(
            Executable::from_bytes(&bytes[..bytes.len() - 1]),
            Err(FormatError::BadCrc { .. }) | Err(FormatError::Truncated)
        ));
        let mut extended = bytes.clone();
        extended.push(0);
        assert!(Executable::from_bytes(&extended).is_err());
    }

    fn sample_v2() -> Executable {
        Executable::sectioned(
            ARCH_TM1,         // arch (TM-1)
            0,                // entry
            vec![0x10, 0x02], // code (rd; stp — placeholder)
            vec![1, 1, 0, 5], // tables (a tiny match-table blob)
            2,                // tape_count
            PROFILE_FRAMES,   // profile = frames
            vec![3, 128],     // per-tape alphabet cardinalities
        )
    }

    #[test]
    fn v2_round_trips() {
        let exe = sample_v2();
        let bytes = exe.to_bytes();
        assert_eq!(&bytes[0..3], b"MX\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2);
        assert_eq!(Executable::from_bytes(&bytes).unwrap(), exe);
    }

    /// Pins the absolute v2 header byte offsets so a symmetric field
    /// transposition (e.g. swapping tape_count/profile or entry/code_size)
    /// would fail. Layout: magic3 + ver2 + arch1 + flags1 + crc4 +
    /// tape_count1 + profile1 + entry4 + code_size4 + table_size4 + cards.
    #[test]
    fn v2_layout_is_exact() {
        let bytes = sample_v2().to_bytes();
        assert_eq!(&bytes[0..3], b"MX\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2); // version
        assert_eq!(bytes[5], ARCH_TM1); // arch (TM-1)
        assert_eq!(bytes[6], 0); // flags
        // [7..11] crc
        assert_eq!(bytes[11], 2); // tape_count
        assert_eq!(bytes[12], PROFILE_FRAMES); // profile = frames
        assert_eq!(u32::from_le_bytes(bytes[13..17].try_into().unwrap()), 0); // entry
        assert_eq!(u32::from_le_bytes(bytes[17..21].try_into().unwrap()), 2); // code_size
        assert_eq!(u32::from_le_bytes(bytes[21..25].try_into().unwrap()), 4); // table_size
        assert_eq!(u32::from_le_bytes(bytes[25..29].try_into().unwrap()), 3); // cardinality[0]
        assert_eq!(u32::from_le_bytes(bytes[29..33].try_into().unwrap()), 128); // cardinality[1]
        // Sections follow the cardinalities: code first, then tables. Pinning
        // both catches a symmetric code|tables swap.
        assert_eq!(&bytes[33..35], &[0x10, 0x02]); // code section
        assert_eq!(&bytes[35..39], &[1, 1, 0, 5]); // tables section
    }

    #[test]
    fn v1_still_parses_after_v2_lands() {
        let v1 = Executable::code_only(ARCH_PM1, 0, vec![0x0D, 0x05, 0x02]);
        assert_eq!(Executable::from_bytes(&v1.to_bytes()).unwrap(), v1);
    }

    #[test]
    fn v2_corruption_rejected_before_decode() {
        let mut bytes = sample_v2().to_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::BadCrc { .. })
        ));
    }

    #[test]
    fn v2_tape_count_zero_or_over_16_rejected() {
        for bad in [0u8, 17] {
            let mut exe = sample_v2();
            exe.tape_count = bad;
            exe.alphabet_cardinalities = vec![3; bad.max(1) as usize];
            let bytes = exe.to_bytes();
            assert!(matches!(
                Executable::from_bytes(&bytes),
                Err(FormatError::Malformed("tape count out of range"))
            ));
        }
    }

    #[test]
    fn v2_cardinality_count_must_match_tape_count() {
        // Hand-corrupt: tape_count says 2 but only 1 cardinality present.
        let exe = sample_v2();
        let mut bytes = exe.to_bytes();
        // tape_count is at offset 11 (after magic3+ver2+arch1+flags1+crc4)
        bytes[11] = 3; // claim 3 tapes; only 2 cardinalities follow → truncation/mismatch
        crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
        assert!(Executable::from_bytes(&bytes).is_err());
    }
}
