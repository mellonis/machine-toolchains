//! Binary container formats shared by all machine toolchains (spec §6).
//! Pure byte codecs: no I/O, no architecture knowledge.

pub mod crc32;
pub(crate) mod io;

/// Format version written into every v1 container.
pub const FORMAT_VERSION: u16 = 1;

/// Architecture ids carried in `MO`/`MX` headers. The formats layer
/// stores them verbatim; only the VM's arch registry judges them.
pub const ARCH_PM1: u8 = 0x01;

pub mod executable;
pub mod object;

#[derive(Debug, PartialEq, Eq)]
pub enum FormatError {
    BadMagic,
    BadCrc { stored: u32, computed: u32 },
    UnsupportedVersion(u16),
    Truncated,
    Malformed(&'static str),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "bad magic"),
            Self::BadCrc { stored, computed } => {
                write!(
                    f,
                    "crc mismatch: stored {stored:#010x}, computed {computed:#010x}"
                )
            }
            Self::UnsupportedVersion(v) => write!(f, "unsupported format version {v}"),
            Self::Truncated => write!(f, "truncated file"),
            Self::Malformed(what) => write!(f, "malformed file: {what}"),
        }
    }
}

impl std::error::Error for FormatError {}
