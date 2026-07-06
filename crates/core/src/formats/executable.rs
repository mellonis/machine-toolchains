//! `MX` executable container (docs/formats.md).

use super::crc32::{stamp_crc, verify_crc};
use super::io::{Reader, put_u16, put_u32};
use super::{FORMAT_VERSION, FormatError};

pub const MAGIC_EXECUTABLE: [u8; 3] = [b'M', b'X', 0x01];
const CRC_OFFSET: usize = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executable {
    pub arch: u8,
    pub entry: u32,
    pub code: Vec<u8>,
}

impl Executable {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(19 + self.code.len());
        out.extend_from_slice(&MAGIC_EXECUTABLE);
        put_u16(&mut out, FORMAT_VERSION);
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
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
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
        Ok(Self { arch, entry, code })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::{ARCH_PM1, FormatError};

    fn sample() -> Executable {
        Executable {
            arch: ARCH_PM1,
            entry: 0,
            code: vec![0x0D, 0x05, 0x02], // ent, rgt, stp
        }
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
}
