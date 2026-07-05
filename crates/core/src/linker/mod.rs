//! `MO` objects → `MX` executables: symbol resolution, reachability,
//! layout, and relaxation (spec §9).

mod layout;
pub(crate) mod resolve;

use crate::asm::ArchSyntax;
use crate::formats::executable::Executable;
use crate::formats::object::ObjectFile;

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

/// Linker knobs; `relax` (default `true`) enables the far→short call
/// relaxation fixpoint (spec §9).
#[derive(Debug, Clone, Copy)]
pub struct LinkOptions {
    pub relax: bool,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self { relax: true }
    }
}

/// One linked function's range and (optional) debug info, absolute
/// offsets into the emitted [`Executable`]'s code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFunction {
    pub name: String,
    /// Absolute code offset of the function's `ent`.
    pub start: u32,
    /// Exclusive end offset.
    pub end: u32,
    /// Absolute offsets; empty without `-g` objects.
    pub labels: Vec<(String, u32)>,
    /// (absolute code offset, source line); empty without `-g` objects.
    pub lines: Vec<(u32, u32)>,
}

/// The `.pmx.map` sidecar contents (JSON serialization added in a later
/// task; this is the plain in-memory shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFile {
    pub arch: u8,
    /// Presentation glyphs; empty if unknown to the generic core linker.
    pub alphabet: Vec<String>,
    pub functions: Vec<MapFunction>,
}

/// Structured account of what the linker did — the CLI renders it under
/// `-v` (a later plan); libraries never print (library-first principle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkReport {
    /// Defined but unreachable, sorted (see `resolve::Resolved::dropped`).
    pub dropped: Vec<String>,
    pub relaxed_calls: u32,
    pub far_calls: u32,
}

#[derive(Debug)]
pub struct LinkOutput {
    pub executable: Executable,
    pub map: MapFile,
    pub report: LinkReport,
}

/// `MO` objects → `MX` executable (spec §9): resolve symbols and
/// reachability, then lay out, relax, and emit code for the reached
/// functions.
pub fn link(
    syntax: &ArchSyntax,
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    options: LinkOptions,
) -> Result<LinkOutput, LinkError> {
    let resolved = resolve::resolve(objects, libraries)?;
    let arch = objects
        .first()
        .or_else(|| libraries.first())
        .expect("resolve succeeded => at least one object")
        .arch;

    let built = layout::build(syntax, &resolved.order, options.relax)?;

    Ok(LinkOutput {
        executable: Executable {
            arch,
            entry: 0,
            code: built.code,
        },
        map: MapFile {
            arch,
            alphabet: Vec::new(),
            functions: built.functions,
        },
        report: LinkReport {
            dropped: resolved.dropped,
            relaxed_calls: built.relaxed_calls,
            far_calls: built.far_calls,
        },
    })
}
