//! `MO` objects → `MX` executables: symbol resolution, reachability,
//! layout, and relaxation (docs/stdlib.md (linking)).

mod layout;
pub(crate) mod resolve;

use crate::asm::ArchSyntax;
use crate::formats::executable::Executable;
use crate::formats::object::ObjectFile;
use crate::formats::{PROFILE_BASE, PROFILE_FRAMES};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    DuplicateSymbol(String),
    Unresolved(Vec<String>),
    /// The BFS entry symbol (default `main`, or the `--entry` override) is
    /// not defined by any linked object. Carries the entry name so a
    /// mistyped `--entry` reports the name that was looked up.
    NoEntrySymbol(String),
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
            Self::NoEntrySymbol(name) => write!(f, "no `{name}` entry symbol"),
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

/// Which mechanism the composition engine uses to lower a declarative
/// bound call (docs/formats.md (frames profile)). `Mono` stamps a
/// rewritten routine copy per composite; `Frames` keeps one generic copy
/// and resolves the binding through a runtime compose table; `Hybrid`
/// (the default) classifies per call site. CARRIED by `LinkOptions` but
/// not yet consumed — the engine that reads it lands in a later
/// phase-5b task; today all three link identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CallMech {
    Mono,
    Frames,
    #[default]
    Hybrid,
}

impl std::fmt::Display for CallMech {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Mono => "mono",
            Self::Frames => "frames",
            Self::Hybrid => "hybrid",
        })
    }
}

/// Linker knobs; `relax` (default `true`) enables the far→short call
/// relaxation fixpoint (docs/isa.md; docs/cli.md for `--no-relax`).
#[derive(Debug, Clone)]
pub struct LinkOptions {
    pub relax: bool,
    /// BFS entry symbol; `None` selects the default `"main"`. Threaded to
    /// `resolve` as the reachability root (the `tmt link --entry` flag).
    pub entry: Option<String>,
    /// The bound-call lowering mechanism. CARRIED but NOT YET CONSUMED:
    /// the composition engine that reads this lands in a later phase-5b
    /// task, so this field affects no output today.
    pub call_mech: CallMech,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            relax: true,
            entry: None,
            call_mech: CallMech::Hybrid,
        }
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
    let entry = options.entry.as_deref().unwrap_or("main");
    let resolved = resolve::resolve(objects, libraries, entry)?;

    // Declarative call-site binding records are the composition engine's
    // input; until that lands the linker refuses them rather than
    // silently emitting a call whose tape binding never happens. The
    // refusal is scoped to REACHABLE bound calls (a binding inside a
    // dropped/shadowed function never runs, so it must not poison the
    // link); the callee's name comes from the resolved order.
    if let Some(func) = resolved.order.iter().find(|f| !f.bound.is_empty()) {
        let callee = &resolved.order[func.bound[0].1];
        return Err(LinkError::UnsupportedBindings(callee.name.to_string()));
    }

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
    // alphabet cardinalities verbatim. The profile is `PROFILE_FRAMES`
    // iff the image carries a frame descriptor or a framed call, else
    // `PROFILE_BASE` — so frameless links stay byte-identical. Without
    // either tables or a signature, the code-only shape is emitted
    // exactly as before tables existed.
    let any_signature = resolved.order.iter().any(|f| f.signature.is_some());
    let profile = if built.frames_present {
        PROFILE_FRAMES
    } else {
        PROFILE_BASE
    };
    let executable = if !built.tables.is_empty() || any_signature {
        let entry = &resolved.order[0];
        let Some(sig) = entry.signature else {
            return Err(LinkError::MissingSignature(entry.name.to_string()));
        };
        let exe = Executable::sectioned(
            arch,
            0,
            built.code,
            built.tables,
            sig.arity,
            profile,
            sig.cardinalities.clone(),
        );
        // A frames image points at its region (docs/formats.md (frames
        // region)); a frameless one leaves the offset 0 (byte-identity).
        if built.frames_offset != 0 {
            exe.with_frames_offset(built.frames_offset)
        } else {
            exe
        }
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
