//! Little-endian byte cursor + writer helpers for the container codecs.

use super::FormatError;

#[allow(dead_code)]
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

#[allow(dead_code)]
impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self.pos.checked_add(n).ok_or(FormatError::Truncated)?;
        if end > self.buf.len() {
            return Err(FormatError::Truncated);
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.bytes(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub(crate) fn i64(&mut self) -> Result<i64, FormatError> {
        Ok(i64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    pub(crate) fn finish(self) -> Result<(), FormatError> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(FormatError::Malformed("trailing bytes"))
        }
    }
}

#[allow(dead_code)]
pub(crate) fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

#[allow(dead_code)]
pub(crate) fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

#[allow(dead_code)]
pub(crate) fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::FormatError;

    #[test]
    fn reads_back_what_writers_wrote() {
        let mut buf = Vec::new();
        buf.push(0xAB);
        put_u16(&mut buf, 0x1234);
        put_u32(&mut buf, 0xDEAD_BEEF);
        put_i64(&mut buf, -5);
        buf.extend_from_slice(b"xyz");

        let mut r = Reader::new(&buf);
        assert_eq!(r.u8().unwrap(), 0xAB);
        assert_eq!(r.u16().unwrap(), 0x1234);
        assert_eq!(r.u32().unwrap(), 0xDEAD_BEEF);
        assert_eq!(r.i64().unwrap(), -5);
        assert_eq!(r.bytes(3).unwrap(), b"xyz");
        assert!(r.finish().is_ok());
    }

    #[test]
    fn truncation_is_reported() {
        let mut r = Reader::new(&[0x01]);
        assert!(matches!(r.u32(), Err(FormatError::Truncated)));
    }

    #[test]
    fn trailing_bytes_are_reported() {
        let mut r = Reader::new(&[1, 2]);
        r.u8().unwrap();
        assert!(matches!(
            r.finish(),
            Err(FormatError::Malformed("trailing bytes"))
        ));
    }
}
