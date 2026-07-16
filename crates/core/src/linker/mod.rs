//! `MO` objects → `MX` executables: symbol resolution, reachability,
//! layout, and relaxation (docs/stdlib.md (linking)).

mod layout;
pub(crate) mod resolve;

use crate::asm::ArchSyntax;
use crate::formats::executable::Executable;
use crate::formats::object::ObjectFile;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    DuplicateSymbol(String),
    Unresolved(Vec<String>),
    NoEntrySymbol,
    ArchMismatch {
        expected: u8,
        found: u8,
    },
    /// A blob failed decode, had a relocation hole that no call
    /// instruction consumes (or a call instruction with no matching
    /// hole). Also raised when a blob lacks its entry-opcode prologue,
    /// when a jump targets a non-boundary offset, or when a debug
    /// label/line offset falls off instruction boundaries.
    MalformedBlob {
        symbol: String,
        at: u32,
    },
    /// A function's table blob failed the fixup-driven walk: bytes not
    /// covered by any referenced table, a truncated table header, or a
    /// dispatch entry off its function's instruction boundaries. `at` is
    /// the table-blob-relative offset of the first offending byte.
    MalformedTable {
        symbol: String,
        at: u32,
    },
    /// An object carries declarative call-site binding records, which
    /// the linker cannot honor yet — they are the composition engine's
    /// input. Refusing beats silently dropping call semantics. Carries
    /// the callee symbol name of the first such record.
    UnsupportedBindings(String),
    /// The link brings in table content or routine signatures, so the
    /// executable needs a sectioned header — but the entry function has
    /// no signature to fill it. Carries the entry function's name.
    MissingSignature(String),
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
            Self::MalformedTable { symbol, at } => {
                write!(
                    f,
                    "malformed table data for `{symbol}` at table offset {at}"
                )
            }
            Self::UnsupportedBindings(name) => {
                write!(
                    f,
                    "call-site binding records are not supported yet (call to `{name}`); \
                     they need the composition engine"
                )
            }
            Self::MissingSignature(name) => {
                write!(
                    f,
                    "entry function `{name}` has no routine signature to fill the \
                     sectioned executable header"
                )
            }
        }
    }
}

impl std::error::Error for LinkError {}

/// Linker knobs; `relax` (default `true`) enables the far→short call
/// relaxation fixpoint (docs/isa.md; docs/cli.md for `--no-relax`).
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// The `.pmx.map` sidecar contents: the plain in-memory shape, JSON via
/// [`MapFile::to_json`]/[`MapFile::from_json`] (docs/formats.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapFile {
    pub arch: u8,
    pub functions: Vec<MapFunction>,
}

impl MapFile {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("map serialization is infallible")
    }

    pub fn from_json(s: &str) -> Result<MapFile, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}

/// Structured account of what the linker did — the CLI renders it under
/// `-v` (docs/cli.md); libraries never print (library-first principle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkReport {
    /// Defined but unreachable, sorted (see `resolve::Resolved::dropped`).
    /// Name-level and namespace-based: local symbols never appear here —
    /// unreached locals are silently omitted.
    pub dropped: Vec<String>,
    /// Count of symbol sites (calls and tail jumps) relaxed to their short form.
    pub relaxed_calls: u32,
    /// Count of symbol sites (calls and tail jumps) that stayed far.
    pub far_calls: u32,
}

#[derive(Debug)]
pub struct LinkOutput {
    pub executable: Executable,
    pub map: MapFile,
    pub report: LinkReport,
}

/// `MO` objects → `MX` executable (docs/stdlib.md (linking)): resolve
/// symbols and reachability, then lay out, relax, and emit code for the
/// reached functions.
pub fn link(
    syntax: &ArchSyntax,
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    options: LinkOptions,
) -> Result<LinkOutput, LinkError> {
    // Declarative call-site binding records are the composition engine's
    // input; until that lands the linker refuses them outright — even in
    // functions that would be dropped — rather than silently emitting a
    // call whose tape binding never happens.
    for object in objects.iter().chain(libraries) {
        if let Some(bound) = object.bound_calls.first() {
            let name = object
                .symbols
                .get(bound.symbol as usize)
                .map_or_else(String::new, |s| s.name.clone());
            return Err(LinkError::UnsupportedBindings(name));
        }
    }

    let resolved = resolve::resolve(objects, libraries)?;
    let arch = objects
        .first()
        .or_else(|| libraries.first())
        .expect("resolve succeeded => at least one object")
        .arch;

    let built = layout::build(syntax, &resolved.order, options.relax)?;

    // Emit shape (docs/formats.md (executable image)): table content or
    // routine signatures anywhere in the reached set require the
    // sectioned image, whose header fields come from the ENTRY
    // function's signature — tape count from its arity, per-tape
    // alphabet cardinalities verbatim, profile 0 (base; a nonzero
    // profile awaits the composition engine). Without either, the
    // code-only shape is emitted exactly as before tables existed.
    let any_signature = resolved.order.iter().any(|f| f.signature.is_some());
    let executable = if !built.tables.is_empty() || any_signature {
        let entry = &resolved.order[0];
        let Some(sig) = entry.signature else {
            return Err(LinkError::MissingSignature(entry.name.to_string()));
        };
        Executable::sectioned(
            arch,
            0,
            built.code,
            built.tables,
            sig.arity,
            0,
            sig.cardinalities.clone(),
        )
    } else {
        Executable::code_only(arch, 0, built.code)
    };

    Ok(LinkOutput {
        executable,
        map: MapFile {
            arch,
            functions: built.functions,
        },
        report: LinkReport {
            dropped: resolved.dropped,
            relaxed_calls: built.relaxed_calls,
            far_calls: built.far_calls,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_json_round_trips() {
        let map = MapFile {
            arch: 1,
            functions: vec![MapFunction {
                name: "main".into(),
                start: 0,
                end: 7,
                labels: vec![("X".into(), 3)],
                lines: vec![(1, 2), (3, 4)],
            }],
        };
        let json = map.to_json();
        assert!(json.contains("\"main\""));
        assert!(!json.contains("\"alphabet\""));
        let back = MapFile::from_json(&json).unwrap();
        assert_eq!(back, map);
        assert!(MapFile::from_json("{not json").is_err());
    }
}
