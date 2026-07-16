//! Binary container formats shared by all machine toolchains
//! (docs/formats.md). Pure byte codecs: no I/O, no architecture knowledge.

pub mod crc32;
pub(crate) mod io;

/// Architecture ids carried in `MO`/`MX` headers. The formats layer
/// stores them verbatim; only the VM's arch registry judges them.
pub const ARCH_PM1: u8 = 0x01;
/// TM-1, the multi-tape Turing architecture.
pub const ARCH_TM1: u8 = 0x02;

/// Execution-profile ids carried in the `MX` v2 header (docs/formats.md
/// (executable image)). The base profile is the frameless single-machine
/// execution model every architecture starts from; the frames profile
/// adds the FR register, frame-descriptor loads, and the framed
/// call/return instructions. The formats layer stores the byte verbatim;
/// the VM's loader judges whether it implements the profile.
pub const PROFILE_BASE: u8 = 0;
/// The frames execution profile (docs/formats.md (executable image)).
pub const PROFILE_FRAMES: u8 = 1;

pub mod executable;
pub mod object;
pub mod tapeblock;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Object,
    Executable,
    TapeBlock,
}

/// Identify a container by magic (tools never dispatch on extensions —
/// docs/formats.md).
pub fn sniff(bytes: &[u8]) -> Option<ContainerKind> {
    match bytes.get(..3)? {
        m if m == executable::MAGIC_EXECUTABLE => Some(ContainerKind::Executable),
        m if m == object::MAGIC_OBJECT => Some(ContainerKind::Object),
        m if m == tapeblock::MAGIC_TAPEBLOCK => Some(ContainerKind::TapeBlock),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_recognizes_all_containers_and_rejects_noise() {
        assert!(matches!(sniff(b"MO\x01rest"), Some(ContainerKind::Object)));
        assert!(matches!(
            sniff(b"MX\x01rest"),
            Some(ContainerKind::Executable)
        ));
        assert!(matches!(
            sniff(b"MT\x01rest"),
            Some(ContainerKind::TapeBlock)
        ));
        assert!(sniff(b"MZ\x01").is_none());
        assert!(sniff(b"MO").is_none()); // too short
        assert!(sniff(b"MO\x02xx").is_none()); // wrong epoch
    }
}
