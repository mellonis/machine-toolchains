//! `MO` object container (docs/formats.md).

mod read;
mod write;

#[cfg(test)]
mod tests;

pub const MAGIC_OBJECT: [u8; 3] = [b'M', b'O', 0x01];
/// MO format version within epoch 0x01. v2 added symbol kind 2 (Local);
/// it is what PM-1's compiler emits and what the v2-shape path serializes
/// byte-for-byte.
pub const OBJECT_FORMAT_VERSION_V2: u16 = 2;
/// MO v3 adds generic-routine signatures, table blobs, table fixups,
/// declarative bound calls, per-blob build-variant tags, and a
/// program-volatile header bit. An object with any of those present
/// serializes as v3 (see `is_v2_shape`); the reader accepts both v2 and v3.
pub const OBJECT_FORMAT_VERSION_V3: u16 = 3;
/// Version 4 appends the interface section (flags bit 5), the
/// graft-provenance list, and widens bound-call records with parameter
/// names, glyph labels, the written-empty-map flag and exit vectors
/// (docs/formats.md (.pmo)).
pub const OBJECT_FORMAT_VERSION_V4: u16 = 4;
const CRC_OFFSET: usize = 7;
const EXTERNAL_BLOB: u32 = 0xFFFF_FFFF;
const FLAG_HAS_DEBUG: u8 = 0b0000_0001;
const FLAG_HAS_SIGNATURES: u8 = 0b0000_0010;
const FLAG_HAS_TABLES: u8 = 0b0000_0100;
/// Gates the per-blob build-variant tag section, parallel to `blobs` when
/// present.
const FLAG_HAS_VARIANTS: u8 = 0b0000_1000;
/// A pure header bit — no section of its own — set when the object's
/// program is a volatile build (only ever true on the object defining the
/// entry symbol; carried here rather than derived so a tag-free legacy
/// object still links unambiguously as non-volatile).
const FLAG_PROGRAM_VOLATILE: u8 = 0b0001_0000;
/// v4: the interface section is present (docs/formats.md (routine interfaces)).
const FLAG_HAS_INTERFACE: u8 = 0b0010_0000;
/// "No string" in a string-index field (the sentinel `EXTERNAL_BLOB` already
/// uses for "no blob").
const NO_STRING: u32 = 0xFFFF_FFFF;

/// In-memory object: symbols + code blobs + call relocations (+ optional
/// per-blob debug info).
///
/// Invariants — enforced by `from_bytes`, and REQUIRED of any
/// hand-constructed value handed to the linker:
/// - every `Defined`/`Local` symbol indexes into `blobs`;
/// - every relocation's `blob` indexes into `blobs`, its `symbol` into
///   `symbols`, and `offset..offset + 4` lies inside that blob;
/// - each relocation hole is the operand of a far-call instruction at
///   `offset - 1` (the linker re-decodes blobs and rejects holes that
///   land anywhere else);
/// - each blob's first byte is the arch's entry opcode — function bodies
///   begin with their `ent` prologue;
/// - `debug`, when present, parallels `blobs` one-to-one, with label and
///   line offsets on instruction boundaries;
/// - `variants`, when present, parallels `blobs` one-to-one — one tag per
///   blob, same indexing as `debug`/`signatures`/`table_blobs`;
/// - a tape binding marked `open` also has `map_written` set — an open map
///   is a written map whose listed pairs are not the whole of it;
/// - a tape binding that carries pairs also has `map_written` set — a map
///   with pairs was written by definition, which is what lets a v3 stream
///   derive the flag it does not store.
///
/// The six v3 fields (`signatures`, `table_blobs`, `table_fixups`,
/// `bound_calls`, `variants`, `program_volatile`) are absent/default in a
/// v2-shape object — the shape PM-1's compiler emitted before volatile
/// builds, serialized byte-for-byte as v2. When any is present/set the
/// object serializes as v3 (see `is_v2_shape`).
///
/// The two v4 fields (`interface`, `grafts`) and the v4-only parts of a
/// bound call are likewise absent/default in a v2- or v3-shape object;
/// `is_v4_shape` reports when any of them is present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectFile {
    pub arch: u8,
    pub symbols: Vec<Symbol>,
    pub blobs: Vec<Vec<u8>>,
    pub relocations: Vec<Relocation>,
    pub debug: Option<Vec<BlobDebug>>,
    /// Per-blob generic-routine signature, parallel to `blobs` when present
    /// (like `debug`). `None` for architectures without generic routines.
    pub signatures: Option<Vec<RoutineSig>>,
    /// Per-blob table blob (the mtc/djmp jump-table data), parallel to
    /// `blobs` when present.
    pub table_blobs: Option<Vec<Vec<u8>>>,
    /// Operand holes referencing a blob's own table blob; rebased by the
    /// linker into the final table section.
    pub table_fixups: Vec<TableFixup>,
    /// Declarative bound call sites (`call name [binding]`), the composition
    /// engine's input.
    pub bound_calls: Vec<BoundCall>,
    /// Per-blob build-variant tag for volatile builds, parallel to `blobs`
    /// when present. `None` means "no variant records" — a legacy or
    /// assembled/TM object, read back as all-`Normal` by the linker's
    /// selection rule rather than stored as such here (a typed absence, not
    /// a stand-in vector).
    pub variants: Option<Vec<BlobVariant>>,
    /// True when the object's program (the object defining the entry
    /// symbol) is a volatile build. A pure header bit: no section, and
    /// independent of `variants` — an object can set this without carrying
    /// variant tags of its own.
    pub program_volatile: bool,
    /// The interface section, present iff flags bit 5 (v4). Requires
    /// `signatures` to be present too: an interface describes a signed routine.
    pub interface: Option<Interface>,
    /// Library graphs this unit spliced, with the digest of each body it
    /// spliced (v4; the linker compares against the exporter's `Interface::graphs`).
    pub grafts: Vec<GraftProvenance>,
}

/// A code blob's build variant under volatile builds: which lowering(s) of
/// its function the blob holds. `Both` marks a blob whose normal and
/// volatile columns compiled byte-identical and were deduped into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobVariant {
    Normal,
    Volatile,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub def: SymbolDef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolDef {
    Defined {
        blob: u32,
    },
    /// Defined but NOT exported: bound directly within its own object,
    /// invisible to cross-object resolution (docs/formats.md (.pmo);
    /// docs/core.md (linking) for the visibility rule this backs).
    Local {
        blob: u32,
    },
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
    pub blob: u32,
    pub offset: u32,
    pub symbol: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlobDebug {
    pub labels: Vec<(String, u32)>,
    pub lines: Vec<(u32, u32)>,
}

/// A generic routine's signature: its virtual tape arity and per-tape
/// alphabet cardinality. Parallel to `blobs` when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineSig {
    pub arity: u8,               // 1..=16
    pub cardinalities: Vec<u32>, // len == arity, each >= 1
}

/// An mtc/djmp operand hole: the u32 at `offset` inside `blob`'s code is
/// an offset into that blob's OWN table blob; the linker rebases it into
/// the final table section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableFixup {
    pub blob: u32,
    pub offset: u32,
    pub table_offset: u32,
}

/// One caller-symbol → callee-symbol map entry. `one_way` = read-only
/// (collapse allowed, excluded from write-back; the `=>` pairs of a tape
/// binding).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapPair {
    pub src: u32,
    /// The callee-side index. Ignored when `dst_label` is `Some` — the
    /// linker resolves the label against the callee's declared glyphs and
    /// fills this in. The wire carries the label in this field's place, so
    /// a labelled pair reads back with `dst: 0`: build one with `dst: 0`,
    /// or it will not survive a round trip.
    pub dst: u32,
    /// A glyph label the linker resolves against the callee's interface
    /// (v4 only); `None` is the positional form.
    pub dst_label: Option<String>,
    pub one_way: bool,
}

/// One virtual-tape binding at a call site: which caller tape feeds this
/// callee tape, and the symbol map between their alphabets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeBinding {
    pub caller_tape: u8, // < 16
    /// The callee parameter this entry binds (v4 only); `None` is the
    /// positional form, where the entry's position is the callee tape.
    pub param: Option<String>,
    /// The map was written out — `1{}` (true) versus `1` (false): an
    /// omitted map is index identity, a written empty one is the empty
    /// map. A map with pairs is written by definition, and v3 already
    /// carries its pairs, so only the written-EMPTY map needs v4 to be
    /// expressible; a v3 stream reads back `map_written` = "it has
    /// pairs".
    pub map_written: bool,
    /// The map ends in `*`: the pairs listed are not the whole map, and
    /// the rest stays open for the linker to fill (v4 only). Implies
    /// `map_written` — an open map is a written one.
    pub open: bool,
    pub pairs: Vec<MapPair>,
}

/// A declarative bound call site (`call name [binding]` in .tma): the
/// composition engine's input. `offset` marks the call operand hole in
/// `blob`, like a Relocation; `binding[k]` binds the callee's virtual
/// tape k.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundCall {
    pub blob: u32,
    pub offset: u32,
    pub symbol: u32,
    pub binding: Vec<TapeBinding>,
    /// Where the callee's exits land: blob-relative code offsets in the
    /// calling blob, one per state parameter (v4 only).
    pub exits: Vec<u32>,
}

/// One routine's interface, parallel to `blobs`
/// (docs/formats.md (routine interfaces)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineInterface {
    /// Parameter names, one per virtual tape; len == the signature's arity.
    pub params: Vec<String>,
    /// Per-tape glyph labels; `glyphs[k].len()` == the signature's
    /// `cardinalities[k]`.
    pub glyphs: Vec<Vec<String>>,
    /// Per-tape written set: each element a subset of `glyphs[k]`.
    pub writes: Vec<Vec<String>>,
    /// Per-tape entry contract: the glyphs the head may stand on when the
    /// routine is entered. len == arity; `None` = no clause, and a `Some`
    /// list is never empty.
    pub enters: Vec<Option<Vec<String>>>,
    /// Per-tape exit contract, same shape as `enters`.
    pub leaves: Vec<Option<Vec<String>>>,
    /// Per-tape opacity: every state that reads this tape reads it as `*`,
    /// so the routine never discriminates its glyphs. len == arity.
    pub opaque: Vec<bool>,
    /// Number of state parameters — the exits a caller must supply.
    pub exits: u8,
    /// False for a `noreturn` routine: control never returns to the caller.
    pub returns: bool,
}

/// An alphabet the unit exports by name, with its glyphs in band order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedAlphabet {
    pub name: String,
    pub glyphs: Vec<String>,
}

/// A graph the unit exports, with the digest of the body a grafting unit
/// must have spliced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedGraph {
    pub name: String,
    pub digest: u32,
}

/// The object's interface section (flags bit 5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Interface {
    /// Parallel to `blobs`, like `signatures`.
    pub routines: Vec<RoutineInterface>,
    pub alphabets: Vec<ExportedAlphabet>,
    pub graphs: Vec<ExportedGraph>,
}

/// A library graph this unit spliced, with the digest of the body it
/// spliced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraftProvenance {
    pub graph: String,
    pub digest: u32,
}

impl ObjectFile {
    /// Construct a v2-shape object: the six v3 fields absent
    /// (`None`/`None`/empty/empty/`None`/`false`), and the two v4 fields
    /// absent too. This is what PM-1's compiler and the assembler emit —
    /// `is_v2_shape` holds for the result.
    pub fn v2(
        arch: u8,
        symbols: Vec<Symbol>,
        blobs: Vec<Vec<u8>>,
        relocations: Vec<Relocation>,
        debug: Option<Vec<BlobDebug>>,
    ) -> Self {
        Self {
            arch,
            symbols,
            blobs,
            relocations,
            debug,
            signatures: None,
            table_blobs: None,
            table_fixups: Vec::new(),
            bound_calls: Vec::new(),
            variants: None,
            program_volatile: false,
            interface: None,
            grafts: Vec::new(),
        }
    }

    /// True when no v3 data is present; v3 emit gates on the negation of
    /// this. NOT on its own a promise of v2 bytes: a v4-shape object can be
    /// v2-shape too (graft provenance is the case — none of the v3 fields,
    /// yet v4 content), which is why `to_bytes` asks `is_v4_shape` first and
    /// only then falls to this one.
    pub fn is_v2_shape(&self) -> bool {
        self.signatures.is_none()
            && self.table_blobs.is_none()
            && self.table_fixups.is_empty()
            && self.bound_calls.is_empty()
            && self.variants.is_none()
            && !self.program_volatile
    }

    /// True when any v4-only content is present: an interface section, a
    /// graft-provenance record, or a bound call carrying a parameter name,
    /// a glyph label, a written-EMPTY or open map, or an exit vector. Such
    /// an object serializes as v4; anything else keeps its v2/v3 bytes.
    ///
    /// A written map WITH pairs is deliberately NOT a v4 trigger: the
    /// pairs express it completely and v3 already stores them, so
    /// promoting it would break the "lowest version that carries the
    /// content" rule. Only `1{}` — written and empty — says something v3
    /// cannot, since there an omitted map means index identity.
    pub fn is_v4_shape(&self) -> bool {
        self.interface.is_some()
            || !self.grafts.is_empty()
            || self.bound_calls.iter().any(|bc| {
                !bc.exits.is_empty()
                    || bc.binding.iter().any(|tb| {
                        tb.param.is_some()
                            || (tb.map_written && tb.pairs.is_empty())
                            || tb.open
                            || tb.pairs.iter().any(|p| p.dst_label.is_some())
                    })
            })
    }
}
