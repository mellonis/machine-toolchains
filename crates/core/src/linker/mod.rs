//! `MO` objects → `MX` executables: symbol resolution, reachability,
//! layout, and relaxation (docs/core.md (linking)).

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
    /// mechanism it does not implement. All three mechanisms (mono, frames,
    /// hybrid) have landed, so nothing constructs this today; it is kept
    /// for future mechanism gating. Internal inter-task state.
    UnsupportedCallMech(CallMech),
    /// A raw hand-authored framed call (`call.m` / a `.frame` descriptor)
    /// was reached under `--call-mech=mono`: a mono image runs on the base
    /// profile, which has no frames machinery to activate the descriptor.
    /// `hybrid` hits the identical refusal whenever no OTHER bound site
    /// forces the frames path (docs/core.md (call mechanisms)), so the
    /// advice recommends `frames` outright rather than sending the caller in
    /// a circle. Carries the offending function's name (docs/core.md (the
    /// composition engine)).
    MonoRawFrame(String),
    /// Under `--call-mech=mono` a holey binding makes the stamp synthesize
    /// unmapped-read trap rows into the callee's match table — but only a
    /// dispatch jump routes those rows to the trap stub. This callee reads a
    /// match result through a conditional branch (or leaves it unconsumed),
    /// so a hole symbol would match a synthesized trap row and take the branch
    /// as if it had matched: a silent misroute. `hybrid` hits the identical
    /// refusal whenever the holeyness sits one hop past whatever bound site
    /// its classifier inspected — a nested bound call under an outer
    /// bijection seed, say (docs/core.md (call mechanisms)) — so the advice
    /// recommends `frames` outright. Carries the callee's name (docs/core.md
    /// (the composition engine)).
    MonoHoleyMatchBranch(String),
    /// A freshly minted mono-stamp name (`<routine>.<digest8>`) already
    /// names another routine in this link, or an earlier stamp — a checked
    /// refusal, not a silent rename, because the `.`-separated suffix is
    /// legal in a hand-written identifier and cannot rule out a collision by
    /// character choice alone the way the reserved-and-unlexable `$`
    /// separator it replaced could (docs/core.md (the composition engine)).
    /// Carries the colliding name. Astronomically unlikely in practice: it
    /// needs either a hand-written routine that happens to spell exactly
    /// `<routine>.<digest8>`, or two distinct composites whose 32-bit
    /// digests collide. `frames` is always a valid escape, the same as the
    /// two mono refusals above — it mints no stamp names, so it cannot
    /// collide.
    StampNameCollision(String),
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
                "`{symbol}` uses a raw framed call, which cannot be lowered \
                 onto the base profile; build with --call-mech=frames"
            ),
            Self::MonoHoleyMatchBranch(symbol) => write!(
                f,
                "a holey binding needs `{symbol}`'s match tables consumed by \
                 dispatch jumps, but `{symbol}` reads a match result through a \
                 conditional branch; the synthesized unmapped-read trap rows \
                 would misroute — build with --call-mech=frames"
            ),
            Self::StampNameCollision(name) => write!(
                f,
                "a mono-stamped routine copy would be named `{name}`, which \
                 already names another routine or an earlier stamp in this \
                 link; build with --call-mech=frames"
            ),
        }
    }
}

impl std::error::Error for LinkError {}

/// Which mechanism the composition engine uses to lower a declarative
/// bound call; the three produce different images. `Mono` stamps a
/// specialized copy of the callee per distinct composite and stays on the
/// base profile (no frames region). `Frames` keeps one generic copy per
/// routine and resolves every binding through descriptors + the runtime
/// compose region (docs/formats.md (frames region)). `Hybrid` (the
/// default) classifies per call site: a completed bijection stamps like
/// mono, anything holey or one-way frames.
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

/// The BFS entry symbol a link resolves from when [`LinkOptions::entry`]
/// is `None` (docs/core.md (linking)).
///
/// Exported because the choice is observable outside the link: a caller
/// that has to know WHICH object supplies the entry — and therefore which
/// object's header the link reads its per-object flags from — must look up
/// the same name this does, and a private literal here would let the two
/// drift apart silently.
pub const DEFAULT_ENTRY: &str = "main";

/// Linker knobs; `relax` (default `true`) enables the far→short call
/// relaxation fixpoint (docs/core.md (relaxation); `--no-relax` opts out).
#[derive(Debug, Clone)]
pub struct LinkOptions {
    pub relax: bool,
    /// BFS entry symbol; `None` selects [`DEFAULT_ENTRY`]. Threaded to
    /// `resolve` as the reachability root (the `tmt link --entry` flag).
    pub entry: Option<String>,
    /// The bound-call lowering mechanism the composition engine applies
    /// (see [`CallMech`]); it selects the image `link` emits.
    pub call_mech: CallMech,
    /// Per-input source provenance for the map sidecar (docs/formats.md
    /// (map sidecar)): one entry per input, indexed through the user
    /// objects then the libraries — the same order function origins are
    /// counted in. Each string is stored **verbatim** into every reached
    /// function that input supplied ([`MapFunction::source`]); the linker
    /// applies no path policy of its own, so a caller that wants
    /// sidecar-relative paths computes them before linking. Empty (the
    /// default) stamps nothing; a non-empty vector shorter than the input
    /// list leaves the uncovered tail without provenance.
    pub sources: Vec<Option<String>>,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            relax: true,
            entry: None,
            call_mech: CallMech::Hybrid,
            sources: Vec::new(),
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
    /// The source file the function's defining input was built from,
    /// verbatim as the caller supplied it via [`LinkOptions::sources`] —
    /// the build drivers store it relative to the sidecar's own directory
    /// (docs/formats.md (map sidecar)). `None` (and omitted from the
    /// JSON, so pre-provenance sidecars round-trip unchanged) when the
    /// link ran without provenance — objects loaded from disk have no
    /// known source. A composition-engine mono stamp inherits the origin
    /// of the routine it specializes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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
/// `-v` (docs/core.md (the link report)); libraries never print
/// (library-first principle).
/// The counters are image-level aggregates (their meanings are tabulated in
/// docs/core.md (the composition engine)); a per-routine breakdown is
/// deferred until a consumer needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkReport {
    /// Defined but unreachable, sorted: the union of two drop reasons. Names
    /// the pre-lowering BFS never reached (see `resolve::Resolved::dropped`)
    /// — name-level and namespace-based, so an unreached LOCAL is silently
    /// omitted, since a local was never a namespace candidate to begin with.
    /// And, under `mono`/`hybrid`, generic routines the composition engine
    /// left with no caller once every site was retargeted to a specialized
    /// copy (docs/core.md (linking)) — this second reason names an actual
    /// linked-in function, so unlike the first, a LOCAL orphan does appear.
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
    /// Sorted names that linked the build column NOT matching the
    /// program's volatile bit, because the wanted one was absent
    /// (docs/core.md (linking)). Every name here IS in the image — the
    /// counter reports what shipped, not what the namespace held.
    ///
    /// The counter is SYMMETRIC, because column selection is: a name that
    /// offers only the volatile column is counted for a normal program
    /// exactly as a normal-only name is counted for a volatile one. So
    /// this is empty precisely when every reached name offers the column
    /// [`LinkReport::program_volatile`] selects — never "empty whenever
    /// the bit is clear". A consumer wording a message about it must read
    /// that field rather than assume a direction.
    ///
    /// Note the bit is independent of variant records: an object may set
    /// it while carrying no tags at all (docs/formats.md (MO)), which is
    /// what a hand-assembled volatile program looks like — every name then
    /// offers only the normal column, so EVERY reached name is counted.
    /// That is the intended signal, not a degenerate case.
    pub variant_fallbacks: Vec<String>,
    /// The volatile bit this link resolved with: the bit carried by the
    /// object defining the entry symbol, which selects the column every
    /// name resolves to (docs/core.md (linking)). Reported so a consumer
    /// can say which column a counted fallback was missing.
    pub program_volatile: bool,
}

#[derive(Debug)]
pub struct LinkOutput {
    pub executable: Executable,
    pub map: MapFile,
    pub report: LinkReport,
}

/// `MO` objects → `MX` executable (docs/core.md (linking)): resolve
/// symbols and reachability, then lay out, relax, and emit code for the
/// reached functions.
pub fn link(
    syntax: &ArchSyntax,
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    options: LinkOptions,
) -> Result<LinkOutput, LinkError> {
    let entry = options.entry.as_deref().unwrap_or(DEFAULT_ENTRY);
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
        // has none (docs/core.md (the composition engine)).
        return Err(LinkError::MissingSignature(
            resolved.order[0].name.to_string(),
        ));
    }

    // The composition engine lowers declarative bound calls in FRAMES mode:
    // it rewrites each reachable routine's bound-call sites into framed
    // calls and computes the runtime compose table (docs/core.md (the
    // composition engine)). It is a no-op for bindingless links, keeping
    // them on the byte-identical bindingless path.
    let (order, frames_plan, stats, orphaned) = match entry_sig {
        Some(sig) => engine::lower(syntax, resolved.order, sig, options.call_mech)?,
        None => (
            resolved.order,
            None,
            engine::EngineStats::default(),
            Vec::new(),
        ),
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

    // `dropped` covers both reasons a name doesn't ship: `resolved.dropped`
    // (never reached by the pre-lowering BFS) and `orphaned` (reached then,
    // but left with no caller once mono/hybrid stamping retargeted every
    // site — docs/core.md (linking)). The two lists are disjoint as SITES —
    // a namespace site in `resolved.dropped` never entered `order`, so it
    // cannot also be a stamping orphan — but that is not a disjointness of
    // NAME STRINGS: `order` can (and does, per `resolve.rs`'s
    // `locals_bind_directly_and_may_repeat_across_objects`) hold two
    // distinct `FuncRef`s sharing one name, e.g. two objects each defining
    // their own private `helper`. Both arrive pre-sorted, so `dedup` after
    // one concatenated sort is enough — no merge, and no risk of dropping a
    // genuine duplicate, since `dedup` only collapses ADJACENT equal
    // strings, which a sort already guarantees are adjacent.
    let variant_fallbacks = resolved.variant_fallbacks;
    let program_volatile = resolved.program_volatile;
    let mut dropped = resolved.dropped;
    dropped.extend(orphaned);
    dropped.sort();
    dropped.dedup();

    // Source provenance (docs/formats.md (map sidecar)): layout emits one
    // map record per `order` entry in sequence, so the two are parallel;
    // each function takes its origin input's caller-supplied source string
    // verbatim. A mono stamp's `origin` is the specialized routine's, so
    // stamps inherit its provenance without a special case.
    let mut functions = built.functions;
    if !options.sources.is_empty() {
        for (mf, f) in functions.iter_mut().zip(&order) {
            mf.source = options.sources.get(f.origin).cloned().flatten();
        }
    }

    Ok(LinkOutput {
        executable,
        map: MapFile {
            arch,
            functions,
            bindings,
        },
        report: LinkReport {
            dropped,
            relaxed_calls: built.relaxed_calls,
            far_calls: built.far_calls,
            instantiations: stats.instantiations,
            composites: built.composites,
            compose_table_bytes: built.compose_table_bytes,
            dedup_savings: stats.dedup_savings,
            synthesized_trap_rows: stats.synthesized_trap_rows,
            expanded_rows: stats.expanded_rows,
            variant_fallbacks,
            program_volatile,
        },
    })
}

/// Which input supplied a winning definition: index into the user-object
/// list or the library list as passed to [`resolve_names`] / [`link`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolOrigin {
    Object(usize),
    Library(usize),
}

/// One reached function: its linked symbol name and where its winning
/// definition came from (docs/core.md (name resolution)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedName {
    pub name: String,
    pub origin: SymbolOrigin,
}

/// The linker's name-resolution answer, without layout: which symbols the
/// reachability BFS reaches (in BFS order, with provenance) and which
/// winning definitions it drops. This is the query surface editor tooling
/// compares itself against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNames {
    pub reached: Vec<ResolvedName>,
    pub dropped: Vec<String>,
}

/// The name-resolution half of [`link`] as a standalone query
/// (docs/core.md (name resolution)): the same namespace-building and BFS
/// reachability `link` runs, reported with provenance and without layout
/// or relaxation. Consumers that only need "what does the linker resolve,
/// and from where" — editor tooling comparing itself against the real
/// linker, say — call this instead of running a full link.
pub fn resolve_names(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    entry: &str,
) -> Result<ResolvedNames, LinkError> {
    let resolved = resolve::resolve(objects, libraries, entry)?;
    let n = objects.len();
    Ok(ResolvedNames {
        reached: resolved
            .order
            .iter()
            .map(|f| ResolvedName {
                name: f.name.clone().into_owned(),
                origin: if f.origin < n {
                    SymbolOrigin::Object(f.origin)
                } else {
                    SymbolOrigin::Library(f.origin - n)
                },
            })
            .collect(),
        dropped: resolved.dropped.clone(),
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
                source: None,
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
        // A provenance-less function omits `source` the same way, so a
        // pre-provenance sidecar's byte shape is preserved — and parses
        // back with the serde default (docs/formats.md (map sidecar)).
        assert!(!json.contains("source"), "absent source must not serialize");
        let back = MapFile::from_json(&json).unwrap();
        assert_eq!(back, map);
        assert!(MapFile::from_json("{not json").is_err());

        let mut with_source = map;
        with_source.functions[0].source = Some("../src/main.pmc".to_string());
        let json = with_source.to_json();
        assert!(json.contains("\"source\": \"../src/main.pmc\""));
        assert_eq!(MapFile::from_json(&json).unwrap(), with_source);
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
                source: None,
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
