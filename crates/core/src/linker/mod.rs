//! `MO` objects → `MX` executables: symbol resolution, reachability,
//! layout, and relaxation (spec §9).

pub(crate) mod resolve;

// TODO(plan4-task3): re-exported for the layout/emission step (`link()`);
// unused until that lands, since resolve.rs's own tests reach `resolve`,
// `Resolved`, and `FuncRef` directly rather than through this re-export.
#[allow(unused_imports)]
pub(crate) use resolve::{FuncRef, Resolved, resolve};

#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    DuplicateSymbol(String),
    Unresolved(Vec<String>),
    NoEntrySymbol,
    ArchMismatch { expected: u8, found: u8 },
    MalformedBlob { symbol: String, at: u32 },
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateSymbol(name) => write!(f, "duplicate symbol: {name}"),
            Self::Unresolved(names) => write!(f, "unresolved symbols: {}", names.join(", ")),
            Self::NoEntrySymbol => write!(f, "no `main` entry symbol"),
            Self::ArchMismatch { expected, found } => write!(
                f,
                "architecture mismatch: expected {expected:#04x}, found {found:#04x}"
            ),
            Self::MalformedBlob { symbol, at } => {
                write!(f, "malformed blob for `{symbol}` at offset {at}")
            }
        }
    }
}

impl std::error::Error for LinkError {}
