//! Content-Length-framed message transport (LSP 3.17 base protocol),
//! layered over `std::io::BufRead`/`Write` so callers can supply stdio,
//! sockets, or in-memory buffers interchangeably. Frames exactly one
//! JSON-RPC payload per read/write call; JSON-RPC envelope parsing lives
//! above this layer (docs/lsp.md).

use std::io::{BufRead, Write};

/// Transport-layer framing failures.
#[derive(Debug, PartialEq, Eq)]
pub enum TransportError {
    /// Header section ended without a Content-Length header.
    MissingContentLength,
    /// Malformed header line or unparseable length value.
    MalformedHeader(String),
    /// Payload bytes were not valid UTF-8.
    InvalidUtf8,
    /// Underlying read/write failure (message text of the io::Error).
    Io(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingContentLength => write!(f, "missing content-length header"),
            Self::MalformedHeader(line) => write!(f, "malformed header line: {line}"),
            Self::InvalidUtf8 => write!(f, "payload was not valid utf-8"),
            Self::Io(msg) => write!(f, "io error: {msg}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// Reads one framed message. `Ok(None)` = clean EOF before any header byte.
/// Header lines are `Name: value\r\n`; unknown headers (Content-Type, …) are
/// tolerated; the header block ends at an empty line; then exactly
/// Content-Length bytes of UTF-8 payload follow.
pub fn read_message(reader: &mut dyn BufRead) -> Result<Option<String>, TransportError> {
    let mut content_length: Option<usize> = None;
    let mut header_seen = false;

    loop {
        let mut raw = Vec::new();
        let n = reader
            .read_until(b'\n', &mut raw)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        if n == 0 {
            // EOF right at a line boundary: clean if no header bytes have
            // been seen yet, truncated otherwise.
            return if header_seen {
                Err(TransportError::Io("unexpected eof".to_string()))
            } else {
                Ok(None)
            };
        }
        header_seen = true;

        let had_newline = raw.last() == Some(&b'\n');
        if had_newline {
            raw.pop();
            if raw.last() == Some(&b'\r') {
                raw.pop();
            }
        } else {
            // Bytes were read but no terminating '\n' was found: EOF hit
            // mid-line.
            return Err(TransportError::Io("unexpected eof".to_string()));
        }

        let line = String::from_utf8(raw)
            .map_err(|_| TransportError::MalformedHeader("<invalid utf-8 header>".to_string()))?;
        if line.is_empty() {
            break;
        }

        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| TransportError::MalformedHeader(line.clone()))?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            let len: usize = value
                .trim()
                .parse()
                .map_err(|_| TransportError::MalformedHeader(line.clone()))?;
            content_length = Some(len);
        }
    }

    let content_length = content_length.ok_or(TransportError::MissingContentLength)?;
    let mut payload = vec![0u8; content_length];
    reader.read_exact(&mut payload).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            TransportError::Io("unexpected eof".to_string())
        } else {
            TransportError::Io(e.to_string())
        }
    })?;

    String::from_utf8(payload)
        .map(Some)
        .map_err(|_| TransportError::InvalidUtf8)
}

/// Writes `Content-Length: N\r\n\r\n` + payload, then flushes.
pub fn write_message(writer: &mut dyn Write, payload: &str) -> Result<(), TransportError> {
    let bytes = payload.as_bytes();
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())
        .map_err(|e| TransportError::Io(e.to_string()))?;
    writer
        .write_all(bytes)
        .map_err(|e| TransportError::Io(e.to_string()))?;
    writer
        .flush()
        .map_err(|e| TransportError::Io(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::{BufReader, Read};

    #[test]
    fn round_trips_non_ascii_payload_and_counts_bytes() {
        let payload = "{\"x\":\"привет 😀\"}";
        // Sanity: byte length must differ from char count for this fixture,
        // otherwise the "counts bytes" assertion below is vacuous.
        assert_ne!(payload.len(), payload.chars().count());

        let mut buf = Vec::new();
        write_message(&mut buf, payload).unwrap();

        let header = std::str::from_utf8(&buf).unwrap();
        assert!(header.starts_with(&format!("Content-Length: {}\r\n\r\n", payload.len())));

        let mut reader = BufReader::new(&buf[..]);
        let got = read_message(&mut reader).unwrap();
        assert_eq!(got, Some(payload.to_string()));
    }

    #[test]
    fn reads_two_messages_back_to_back() {
        let mut buf = Vec::new();
        write_message(&mut buf, "first").unwrap();
        write_message(&mut buf, "second").unwrap();

        let mut reader = BufReader::new(&buf[..]);
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some("first".to_string())
        );
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some("second".to_string())
        );
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn tolerates_content_type_header_before_content_length() {
        let payload = "hello";
        let raw = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
            payload.len(),
            payload
        );
        let mut reader = BufReader::new(raw.as_bytes());
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(payload.to_string())
        );
    }

    #[test]
    fn content_length_header_name_is_case_insensitive() {
        let payload = "hi";
        let raw = format!("content-LENGTH: {}\r\n\r\n{}", payload.len(), payload);
        let mut reader = BufReader::new(raw.as_bytes());
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(payload.to_string())
        );
    }

    #[test]
    fn missing_content_length_is_an_error() {
        let raw = "Content-Type: application/vscode-jsonrpc\r\n\r\n";
        let mut reader = BufReader::new(raw.as_bytes());
        assert_eq!(
            read_message(&mut reader),
            Err(TransportError::MissingContentLength)
        );
    }

    #[test]
    fn clean_eof_before_any_header_byte_is_ok_none() {
        let raw: &[u8] = b"";
        let mut reader = BufReader::new(raw);
        assert_eq!(read_message(&mut reader), Ok(None));
    }

    #[test]
    fn eof_mid_payload_is_io_error() {
        // Declares 10 payload bytes but only supplies 3.
        let raw = "Content-Length: 10\r\n\r\nabc";
        let mut reader = BufReader::new(raw.as_bytes());
        let err = read_message(&mut reader).unwrap_err();
        assert!(matches!(err, TransportError::Io(_)));
    }

    #[test]
    fn eof_mid_headers_is_io_error() {
        // Header block never reaches the terminating blank line.
        let raw = "Content-Length: 5\r\n";
        let mut reader = BufReader::new(raw.as_bytes());
        let err = read_message(&mut reader).unwrap_err();
        assert!(matches!(err, TransportError::Io(_)));
    }

    #[test]
    fn invalid_utf8_payload_is_an_error() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"Content-Length: 3\r\n\r\n");
        raw.extend_from_slice(&[0xFF, 0xFE, 0xFD]); // not valid UTF-8
        let mut reader = BufReader::new(&raw[..]);
        assert_eq!(read_message(&mut reader), Err(TransportError::InvalidUtf8));
    }

    /// A `Read` impl that hands back exactly one byte per `read()` call,
    /// forcing `BufRead` consumers through repeated short fills.
    struct OneByteAtATime<'a> {
        data: &'a [u8],
        pos: usize,
    }

    impl Read for OneByteAtATime<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() || buf.is_empty() {
                return Ok(0);
            }
            buf[0] = self.data[self.pos];
            self.pos += 1;
            Ok(1)
        }
    }

    #[test]
    fn frames_correctly_when_reader_delivers_one_byte_at_a_time() {
        let payload = "chunked payload across many tiny reads";
        let mut raw = Vec::new();
        write_message(&mut raw, payload).unwrap();

        let source = OneByteAtATime { data: &raw, pos: 0 };
        let mut reader = BufReader::with_capacity(4, source);
        let got = read_message(&mut reader).unwrap();
        assert_eq!(got, Some(payload.to_string()));
    }

    proptest! {
        #[test]
        fn round_trips_arbitrary_string_payloads(payload in ".*") {
            let mut buf = Vec::new();
            write_message(&mut buf, &payload).unwrap();

            let mut reader = BufReader::new(&buf[..]);
            let got = read_message(&mut reader).unwrap();
            prop_assert_eq!(got, Some(payload));
        }

        #[test]
        fn read_message_never_panics_on_noise(noise in proptest::collection::vec(any::<u8>(), 0..256)) {
            let mut reader = &noise[..];
            let _ = read_message(&mut reader); // must return Ok/Err, not panic
        }
    }
}
