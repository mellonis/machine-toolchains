//! `MO` objects → `MX` executables: symbol resolution, reachability,
//! layout, and relaxation (docs/stdlib.md (linking)).

pub(crate) mod binding_label;
pub(crate) mod compose;
mod engine;
mod layout;
pub(crate) mod resolve;
mod stamp;

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
    /// The link brings in table content or routine signatures, so the
    /// executable needs a sectioned header — but the entry function has
    /// no signature to fill it. Carries the entry function's name.
    MissingSignature(String),
    /// A declarative bound call could not be lowered: the binding is
    /// illegal (arity, caller/callee symbol range, blank pinning, a
    /// non-injective completed bijection, a per-direction conflict). The
    /// message carries the callee name and the specific reason
    /// (docs/formats.md (bound calls)).
    BadBinding {
        callee: String,
        message: String,
    },
    /// A frame descriptor is inconsistent with the entry signature: a
    /// physical-tape index at or past the machine's arity, or an
    /// undecodable hand-authored descriptor. Carries the owning function's
    /// name and the specific reason (docs/formats.md (frame descriptors)).
    BadFrameDescriptor {
        symbol: String,
        message: String,
    },
    /// The composition engine was asked to lower bound calls under a
    /// mechanism it does not implement yet. FRAMES is complete; `Mono` and
    /// `Hybrid` land with the stamping engine. Internal inter-task state.
    UnsupportedCallMech(CallMech),
    /// A raw hand-authored framed call (`call.m` / a `.frame` descriptor)
    /// was reached under `--call-mech=mono`: a mono image runs on the base
    /// profile, which has no frames machinery to activate the descriptor.
    /// Carries the offending function's name (docs/formats.md (frames
    /// profile)).
    MonoRawFrame(String),
    /// Under `--call-mech=mono` a holey binding makes the stamp synthesize
    /// unmapped-read trap rows into the callee's match table — but only a
    /// dispatch jump routes those rows to the trap stub. This callee reads a
    /// match result through a conditional branch (or leaves it unconsumed),
    /// so a hole symbol would match a prepended trap row and take the branch
    /// as if it had matched: a silent misroute. Carries the callee's name
    /// (docs/formats.md (frames profile)).
    MonoHoleyMatchBranch(String),
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
            Self::MissingSignature(name) => {
                write!(
                    f,
                    "entry function `{name}` has no routine signature to fill the \
                     sectioned executable header"
                )
            }
            Self::BadBinding { callee, message } => {
                write!(f, "bad binding to `{callee}`: {message}")
            }
            Self::BadFrameDescriptor { symbol, message } => {
                write!(f, "bad frame descriptor in `{symbol}`: {message}")
            }
            Self::UnsupportedCallMech(mech) => write!(
                f,
                "the {mech} call mechanism is not implemented yet \
                 (it lands with the stamping engine)"
            ),
            Self::MonoRawFrame(symbol) => write!(
                f,
                "`{symbol}` uses a raw framed call, which the mono call \
                 mechanism cannot lower onto the base profile; build with \
                 --call-mech=frames or hybrid"
            ),
            Self::MonoHoleyMatchBranch(symbol) => write!(
                f,
                "a holey binding needs `{symbol}`'s match tables consumed by \
                 dispatch jumps, but `{symbol}` reads a match result through a \
                 conditional branch; the synthesized unmapped-read trap rows \
                 would misroute — build with --call-mech=frames or hybrid"
            ),
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

/// One virtual tape of a composite binding, decoded to the sparse
/// structured truth (docs/formats.md (sidecar bindings)): the physical tape
/// it projects onto, its non-identity read pairs (`(src, dst, one_way)` —
/// identity is implicit), and the read/write hole sets. A machine consumer
/// (a debugger, a DAP adapter) reads this; the human-readable
/// [`MapBinding::label`] is derived from the same descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapBindingTape {
    pub phys: u8,
    /// `(src, dst, one_way)`; `one_way` marks a `=>` read-only pair (no
    /// write-back inverse). Non-identity pairs only.
    pub pairs: Vec<(u32, u32, bool)>,
    pub read_holes: Vec<u32>,
    pub write_holes: Vec<u32>,
}

/// One directory composite as a map-sidecar record (docs/formats.md (sidecar
/// bindings)): its 1-based directory index, the callee routine, the derived
/// canonical label, and the per-tape structured truth. Every directory entry
/// gets one — engine-synthesized composites and hand-authored `.frame`
/// descriptors alike (the latter decoded dense→sparse from their bytes; the
/// record shape is the same either way).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapBinding {
    /// The runtime composite index (1..=K), a directory offset+1.
    pub index: u16,
    pub routine: String,
    /// The canonical `name@[…]` label (docs/formats.md (binding labels)),
    /// with any one-image display collision suffixed `.2`, `.3`, ….
    pub label: String,
    pub tapes: Vec<MapBindingTape>,
}

/// The `.pmx.map` sidecar contents: the plain in-memory shape, JSON via
/// [`MapFile::to_json`]/[`MapFile::from_json`] (docs/formats.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapFile {
    pub arch: u8,
    pub functions: Vec<MapFunction>,
    /// Structured composite records for a frames image (docs/formats.md
    /// (sidecar bindings)). Absent (and omitted from the JSON) for a
    /// frameless link, so a pre-bindings sidecar still parses (serde default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<MapBinding>,
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
    /// Mono stamps emitted — one specialized routine copy per distinct
    /// (routine, composite) pair reached under `--call-mech=mono|hybrid`
    /// (0 in frames mode and for frameless links).
    pub instantiations: u32,
    /// The frames directory size K — distinct composites in the image
    /// (engine-synthesized plus hand-authored `.frame` descriptors), 0 with
    /// no frames region.
    pub composites: u32,
    /// Bytes of the compose matrix — `(K+1) × S × 2` (rows = active frame
    /// 0..=K, columns = call sites); excludes the K/S header and directory.
    pub compose_table_bytes: u32,
    /// Stamps and descriptors avoided by interning: how many times a
    /// (routine, composite) pair resolved to an already-built copy — mono
    /// stamp dedup plus frames descriptor dedup.
    pub dedup_savings: u32,
    /// Unmapped-read trap rows synthesized into stamped match tables (mono
    /// stamping); 0 in pure frames mode.
    pub synthesized_trap_rows: u32,
    /// Extra match rows produced by one-way collapse expansion in mono
    /// stamping (the growth beyond one row per original); 0 in frames mode.
    pub expanded_rows: u32,
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

    let arch = objects
        .first()
        .or_else(|| libraries.first())
        .expect("resolve succeeded => at least one object")
        .arch;

    // Every hand-authored frame descriptor's physical-tape indices must lie
    // within the machine arity (docs/formats.md (frame descriptors)); the
    // machine arity is the entry signature's. Validated for every reached
    // function before the engine or layout consumes the descriptors.
    let entry_sig = resolved.order[0].signature;
    if let Some(sig) = entry_sig {
        engine::validate_frame_phys(syntax, &resolved.order, sig)?;
    } else if resolved.order.iter().any(|f| !f.bound.is_empty()) {
        // A reachable declarative bound call needs the machine signature
        // (arity + cardinalities) to compose against; an unsigned entry
        // has none (docs/formats.md (frames profile)).
        return Err(LinkError::MissingSignature(
            resolved.order[0].name.to_string(),
        ));
    }

    // The composition engine lowers declarative bound calls in FRAMES mode:
    // it rewrites each reachable routine's bound-call sites into framed
    // calls and computes the runtime compose table (docs/formats.md (frames
    // profile)). It is a no-op for bindingless links, keeping them on the
    // byte-identical 5a/T2 path.
    let (order, frames_plan, stats) = match entry_sig {
        Some(sig) => engine::lower(syntax, resolved.order, sig, options.call_mech)?,
        None => (resolved.order, None, engine::EngineStats::default()),
    };

    let built = layout::build(syntax, &order, options.relax, frames_plan.as_ref())?;

    // Structured composite records for the map sidecar (docs/formats.md
    // (sidecar bindings)): decode every directory descriptor from the final
    // table section — so the sidecar is provably consistent with the image —
    // and pair it with the callee routine names layout threaded through in
    // directory order. Empty for a frameless link. Built before `built.tables`
    // is moved into the executable below.
    let bindings =
        binding_label::build_bindings(&built.tables, built.frames_offset, &built.frames_routines);

    // Emit shape (docs/formats.md (executable image)): table content or
    // routine signatures anywhere in the reached set require the
    // sectioned image, whose header fields come from the ENTRY
    // function's signature — tape count from its arity, per-tape
    // alphabet cardinalities verbatim. The profile is `PROFILE_FRAMES`
    // iff the image carries a frame descriptor or a framed call, else
    // `PROFILE_BASE` — so frameless links stay byte-identical. Without
    // either tables or a signature, the code-only shape is emitted
    // exactly as before tables existed.
    let any_signature = order.iter().any(|f| f.signature.is_some());
    let profile = if built.frames_present {
        PROFILE_FRAMES
    } else {
        PROFILE_BASE
    };
    let executable = if !built.tables.is_empty() || any_signature {
        let entry = &order[0];
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
            bindings,
        },
        report: LinkReport {
            dropped: resolved.dropped,
            relaxed_calls: built.relaxed_calls,
            far_calls: built.far_calls,
            instantiations: stats.instantiations,
            composites: built.composites,
            compose_table_bytes: built.compose_table_bytes,
            dedup_savings: stats.dedup_savings,
            synthesized_trap_rows: stats.synthesized_trap_rows,
            expanded_rows: stats.expanded_rows,
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
            bindings: Vec::new(),
        };
        let json = map.to_json();
        assert!(json.contains("\"main\""));
        assert!(!json.contains("\"alphabet\""));
        // A frameless map omits the bindings key entirely (skip_serializing_if).
        assert!(
            !json.contains("bindings"),
            "empty bindings must not serialize"
        );
        let back = MapFile::from_json(&json).unwrap();
        assert_eq!(back, map);
        assert!(MapFile::from_json("{not json").is_err());
    }

    #[test]
    fn map_json_round_trips_with_bindings() {
        let map = MapFile {
            arch: 2,
            functions: vec![MapFunction {
                name: "main".into(),
                start: 0,
                end: 10,
                labels: vec![],
                lines: vec![],
            }],
            bindings: vec![MapBinding {
                index: 1,
                routine: "helper".into(),
                label: "helper@[2{1->3},0]".into(),
                tapes: vec![
                    MapBindingTape {
                        phys: 2,
                        pairs: vec![(1, 3, false)],
                        read_holes: vec![],
                        write_holes: vec![2],
                    },
                    MapBindingTape {
                        phys: 0,
                        pairs: vec![],
                        read_holes: vec![],
                        write_holes: vec![],
                    },
                ],
            }],
        };
        let json = map.to_json();
        assert!(json.contains("\"bindings\""));
        assert!(json.contains("helper@[2{1->3},0]"));
        let back = MapFile::from_json(&json).unwrap();
        assert_eq!(back, map);
    }

    #[test]
    fn old_sidecar_without_bindings_parses() {
        // A pre-bindings sidecar (no `bindings` key) still deserializes, with
        // the field defaulting empty (serde default).
        let json =
            r#"{"arch":1,"functions":[{"name":"main","start":0,"end":7,"labels":[],"lines":[]}]}"#;
        let back = MapFile::from_json(json).unwrap();
        assert_eq!(back.arch, 1);
        assert_eq!(back.functions.len(), 1);
        assert!(back.bindings.is_empty());
    }
}
