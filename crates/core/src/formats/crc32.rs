//! CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320) — docs/formats.md.

use super::FormatError;

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Zero the 4 crc bytes at `at`, compute the crc of the whole buffer,
/// store it at `at` (little-endian). Writers call this last.
pub fn stamp_crc(buf: &mut [u8], at: usize) {
    buf[at..at + 4].fill(0);
    let crc = crc32(buf);
    buf[at..at + 4].copy_from_slice(&crc.to_le_bytes());
}

/// Verify a buffer stamped by [`stamp_crc`]. Readers call this before
/// decoding anything else.
pub fn verify_crc(buf: &[u8], at: usize) -> Result<(), FormatError> {
    if buf.len() < at + 4 {
        return Err(FormatError::Truncated);
    }
    let stored = u32::from_le_bytes(buf[at..at + 4].try_into().unwrap());
    let mut copy = buf.to_vec();
    copy[at..at + 4].fill(0);
    let computed = crc32(&copy);
    if stored != computed {
        return Err(FormatError::BadCrc { stored, computed });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::FormatError;

    #[test]
    fn crc32_check_vector() {
        // The canonical IEEE CRC-32 check value.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn crc32_empty() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn stamp_then_verify_round_trips() {
        let mut buf = vec![b'M', b'X', 0x01, 1, 0, 1, 0, 0, 0, 0, 0, 7, 7];
        stamp_crc(&mut buf, 7);
        assert!(verify_crc(&buf, 7).is_ok());
    }

    #[test]
    fn verify_detects_corruption() {
        let mut buf = vec![0u8; 16];
        stamp_crc(&mut buf, 4);
        buf[12] ^= 0xFF; // flip a payload byte
        match verify_crc(&buf, 4) {
            Err(FormatError::BadCrc { .. }) => {}
            other => panic!("expected BadCrc, got {other:?}"),
        }
    }

    #[test]
    fn verify_truncated_buffer() {
        assert!(matches!(
            verify_crc(&[0u8; 3], 4),
            Err(FormatError::Truncated)
        ));
    }
}
