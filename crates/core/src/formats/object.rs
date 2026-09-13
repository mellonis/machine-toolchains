//! `MO` object container (docs/formats.md).

use super::FormatError;
use super::crc32::{stamp_crc, verify_crc};
use super::io::{Reader, put_u16, put_u32};

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
#[allow(dead_code)] // used by the v4 writer/reader, which lands with the wire format
const FLAG_HAS_INTERFACE: u8 = 0b0010_0000;
/// "No string" in a string-index field (the sentinel `EXTERNAL_BLOB` already
/// uses for "no blob").
#[allow(dead_code)] // used by the v4 writer/reader, which lands with the wire format
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
///   is a written map whose listed pairs are not the whole of it.
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
    /// fills this in.
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
    /// The map was written out — `1{}` (true) versus `1` (false). A
    /// distinction only v4 carries: an omitted map is index identity,
    /// a written empty one is the empty map.
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

/// Build-time string pool: dedups names, hands out u32 indices.
struct StringPool {
    strings: Vec<String>,
}

impl StringPool {
    fn new() -> Self {
        Self {
            strings: Vec::new(),
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return i as u32;
        }
        self.strings.push(s.to_owned());
        (self.strings.len() - 1) as u32
    }
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

    /// True when no v3 data is present, so the object serializes byte-for-byte
    /// as v2. v3 emit gates on the negation of this.
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
    /// a glyph label, a written-empty or open map, or an exit vector. Such
    /// an object serializes as v4; anything else keeps its v2/v3 bytes.
    pub fn is_v4_shape(&self) -> bool {
        self.interface.is_some()
            || !self.grafts.is_empty()
            || self.bound_calls.iter().any(|bc| {
                !bc.exits.is_empty()
                    || bc.binding.iter().any(|tb| {
                        tb.param.is_some()
                            || tb.map_written
                            || tb.open
                            || tb.pairs.iter().any(|p| p.dst_label.is_some())
                    })
            })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        if self.is_v2_shape() {
            self.to_bytes_v2()
        } else {
            self.to_bytes_v3()
        }
    }

    fn to_bytes_v2(&self) -> Vec<u8> {
        let mut pool = StringPool::new();
        let symbol_names: Vec<u32> = self.symbols.iter().map(|s| pool.intern(&s.name)).collect();
        let debug_label_names: Vec<Vec<u32>> = match &self.debug {
            Some(per_blob) => per_blob
                .iter()
                .map(|d| d.labels.iter().map(|(n, _)| pool.intern(n)).collect())
                .collect(),
            None => Vec::new(),
        };

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_OBJECT);
        put_u16(&mut out, OBJECT_FORMAT_VERSION_V2);
        out.push(self.arch);
        out.push(if self.debug.is_some() {
            FLAG_HAS_DEBUG
        } else {
            0
        });
        put_u32(&mut out, 0); // crc placeholder

        put_u32(
            &mut out,
            u32::try_from(pool.strings.len()).expect("string pool fits u32"),
        );
        for s in &pool.strings {
            put_u16(&mut out, u16::try_from(s.len()).expect("string fits u16"));
            out.extend_from_slice(s.as_bytes());
        }

        put_u32(
            &mut out,
            u32::try_from(self.symbols.len()).expect("symbol count fits u32"),
        );
        for (sym, &name_idx) in self.symbols.iter().zip(&symbol_names) {
            put_u32(&mut out, name_idx);
            match sym.def {
                SymbolDef::Defined { blob } => {
                    out.push(1);
                    put_u32(&mut out, blob);
                }
                SymbolDef::Local { blob } => {
                    out.push(2);
                    put_u32(&mut out, blob);
                }
                SymbolDef::External => {
                    out.push(0);
                    put_u32(&mut out, EXTERNAL_BLOB);
                }
            }
        }

        put_u32(
            &mut out,
            u32::try_from(self.blobs.len()).expect("blob count fits u32"),
        );
        for blob in &self.blobs {
            put_u32(&mut out, u32::try_from(blob.len()).expect("blob fits u32"));
            out.extend_from_slice(blob);
        }

        put_u32(
            &mut out,
            u32::try_from(self.relocations.len()).expect("relocation count fits u32"),
        );
        for reloc in &self.relocations {
            put_u32(&mut out, reloc.blob);
            put_u32(&mut out, reloc.offset);
            put_u32(&mut out, reloc.symbol);
        }

        if let Some(per_blob) = &self.debug {
            debug_assert_eq!(
                per_blob.len(),
                self.blobs.len(),
                "debug section must parallel blobs"
            );
            for (d, names) in per_blob.iter().zip(&debug_label_names) {
                put_u32(
                    &mut out,
                    u32::try_from(d.labels.len()).expect("label count fits u32"),
                );
                for ((_, offset), &name_idx) in d.labels.iter().zip(names) {
                    put_u32(&mut out, name_idx);
                    put_u32(&mut out, *offset);
                }
                put_u32(
                    &mut out,
                    u32::try_from(d.lines.len()).expect("line count fits u32"),
                );
                for (code_offset, line) in &d.lines {
                    put_u32(&mut out, *code_offset);
                    put_u32(&mut out, *line);
                }
            }
        }

        stamp_crc(&mut out, CRC_OFFSET);
        out
    }

    /// Serialize a v3-shape object: the v2 body through the debug section
    /// (version field = 3, flags gaining `FLAG_HAS_SIGNATURES` /
    /// `FLAG_HAS_TABLES` / `FLAG_HAS_VARIANTS` / `FLAG_PROGRAM_VOLATILE`
    /// when the respective field is present/set), followed by the v3
    /// sections — per-blob signatures, per-blob table blobs, the per-blob
    /// variant-tag section, the unconditional table-fixup section, and the
    /// unconditional bound-call section. Read back by `from_bytes` in the
    /// same order.
    fn to_bytes_v3(&self) -> Vec<u8> {
        let mut pool = StringPool::new();
        let symbol_names: Vec<u32> = self.symbols.iter().map(|s| pool.intern(&s.name)).collect();
        let debug_label_names: Vec<Vec<u32>> = match &self.debug {
            Some(per_blob) => per_blob
                .iter()
                .map(|d| d.labels.iter().map(|(n, _)| pool.intern(n)).collect())
                .collect(),
            None => Vec::new(),
        };

        let mut flags = 0u8;
        if self.debug.is_some() {
            flags |= FLAG_HAS_DEBUG;
        }
        if self.signatures.is_some() {
            flags |= FLAG_HAS_SIGNATURES;
        }
        if self.table_blobs.is_some() {
            flags |= FLAG_HAS_TABLES;
        }
        if self.variants.is_some() {
            flags |= FLAG_HAS_VARIANTS;
        }
        if self.program_volatile {
            flags |= FLAG_PROGRAM_VOLATILE;
        }

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_OBJECT);
        put_u16(&mut out, OBJECT_FORMAT_VERSION_V3);
        out.push(self.arch);
        out.push(flags);
        put_u32(&mut out, 0); // crc placeholder

        put_u32(
            &mut out,
            u32::try_from(pool.strings.len()).expect("string pool fits u32"),
        );
        for s in &pool.strings {
            put_u16(&mut out, u16::try_from(s.len()).expect("string fits u16"));
            out.extend_from_slice(s.as_bytes());
        }

        put_u32(
            &mut out,
            u32::try_from(self.symbols.len()).expect("symbol count fits u32"),
        );
        for (sym, &name_idx) in self.symbols.iter().zip(&symbol_names) {
            put_u32(&mut out, name_idx);
            match sym.def {
                SymbolDef::Defined { blob } => {
                    out.push(1);
                    put_u32(&mut out, blob);
                }
                SymbolDef::Local { blob } => {
                    out.push(2);
                    put_u32(&mut out, blob);
                }
                SymbolDef::External => {
                    out.push(0);
                    put_u32(&mut out, EXTERNAL_BLOB);
                }
            }
        }

        put_u32(
            &mut out,
            u32::try_from(self.blobs.len()).expect("blob count fits u32"),
        );
        for blob in &self.blobs {
            put_u32(&mut out, u32::try_from(blob.len()).expect("blob fits u32"));
            out.extend_from_slice(blob);
        }

        put_u32(
            &mut out,
            u32::try_from(self.relocations.len()).expect("relocation count fits u32"),
        );
        for reloc in &self.relocations {
            put_u32(&mut out, reloc.blob);
            put_u32(&mut out, reloc.offset);
            put_u32(&mut out, reloc.symbol);
        }

        if let Some(per_blob) = &self.debug {
            debug_assert_eq!(
                per_blob.len(),
                self.blobs.len(),
                "debug section must parallel blobs"
            );
            for (d, names) in per_blob.iter().zip(&debug_label_names) {
                put_u32(
                    &mut out,
                    u32::try_from(d.labels.len()).expect("label count fits u32"),
                );
                for ((_, offset), &name_idx) in d.labels.iter().zip(names) {
                    put_u32(&mut out, name_idx);
                    put_u32(&mut out, *offset);
                }
                put_u32(
                    &mut out,
                    u32::try_from(d.lines.len()).expect("line count fits u32"),
                );
                for (code_offset, line) in &d.lines {
                    put_u32(&mut out, *code_offset);
                    put_u32(&mut out, *line);
                }
            }
        }

        // v3 sections, in the order the reader consumes them.
        if let Some(sigs) = &self.signatures {
            debug_assert_eq!(
                sigs.len(),
                self.blobs.len(),
                "signatures must parallel blobs"
            );
            for sig in sigs {
                debug_assert_eq!(
                    sig.cardinalities.len(),
                    sig.arity as usize,
                    "cardinalities must have arity entries"
                );
                out.push(sig.arity);
                for &c in &sig.cardinalities {
                    put_u32(&mut out, c);
                }
            }
        }

        if let Some(tables) = &self.table_blobs {
            debug_assert_eq!(
                tables.len(),
                self.blobs.len(),
                "table blobs must parallel blobs"
            );
            for table in tables {
                put_u32(
                    &mut out,
                    u32::try_from(table.len()).expect("table fits u32"),
                );
                out.extend_from_slice(table);
            }
        }

        // Unlike signatures/table blobs (implicitly blob-count-many, so a
        // mismatch is only a debug-only invariant), the variant section
        // ALSO carries its own explicit count, so a length mismatch is a
        // decode-time `Malformed` for any object that reaches bytes — not
        // just a debug-build panic on the construction path.
        if let Some(variants) = &self.variants {
            debug_assert_eq!(
                variants.len(),
                self.blobs.len(),
                "variants must parallel blobs"
            );
            put_u32(
                &mut out,
                u32::try_from(variants.len()).expect("variant count fits u32"),
            );
            for v in variants {
                out.push(match v {
                    BlobVariant::Normal => 0,
                    BlobVariant::Volatile => 1,
                    BlobVariant::Both => 2,
                });
            }
        }

        put_u32(
            &mut out,
            u32::try_from(self.table_fixups.len()).expect("fixup count fits u32"),
        );
        for fixup in &self.table_fixups {
            put_u32(&mut out, fixup.blob);
            put_u32(&mut out, fixup.offset);
            put_u32(&mut out, fixup.table_offset);
        }

        put_u32(
            &mut out,
            u32::try_from(self.bound_calls.len()).expect("bound-call count fits u32"),
        );
        for call in &self.bound_calls {
            put_u32(&mut out, call.blob);
            put_u32(&mut out, call.offset);
            put_u32(&mut out, call.symbol);
            out.push(u8::try_from(call.binding.len()).expect("tape count fits u8"));
            for tape in &call.binding {
                out.push(tape.caller_tape);
                put_u16(
                    &mut out,
                    u16::try_from(tape.pairs.len()).expect("pair count fits u16"),
                );
                for pair in &tape.pairs {
                    put_u32(&mut out, pair.src);
                    put_u32(&mut out, pair.dst);
                    out.push(if pair.one_way { 1 } else { 0 });
                }
            }
        }

        stamp_crc(&mut out, CRC_OFFSET);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < 3 {
            return Err(FormatError::Truncated);
        }
        if bytes[0..3] != MAGIC_OBJECT {
            return Err(FormatError::BadMagic);
        }
        verify_crc(bytes, CRC_OFFSET)?;

        let mut r = Reader::new(&bytes[3..]);
        let version = r.u16()?;
        if !(1..=OBJECT_FORMAT_VERSION_V3).contains(&version) {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let arch = r.u8()?;
        let flags = r.u8()?;
        let _crc = r.u32()?;

        let string_count = r.u32()? as usize;
        let mut strings = Vec::new();
        for _ in 0..string_count {
            let len = r.u16()? as usize;
            let raw = r.bytes(len)?;
            let s =
                std::str::from_utf8(raw).map_err(|_| FormatError::Malformed("string not utf-8"))?;
            strings.push(s.to_owned());
        }
        let name_of = |idx: u32| -> Result<String, FormatError> {
            strings
                .get(idx as usize)
                .cloned()
                .ok_or(FormatError::Malformed("string index out of range"))
        };

        let symbol_count = r.u32()? as usize;
        let mut raw_symbols = Vec::new();
        for _ in 0..symbol_count {
            let name_idx = r.u32()?;
            let kind = r.u8()?;
            let blob = r.u32()?;
            raw_symbols.push((name_idx, kind, blob));
        }

        let blob_count = r.u32()? as usize;
        let mut blobs = Vec::new();
        for _ in 0..blob_count {
            let len = r.u32()? as usize;
            blobs.push(r.bytes(len)?.to_vec());
        }

        let reloc_count = r.u32()? as usize;
        let mut relocations = Vec::new();
        for _ in 0..reloc_count {
            relocations.push(Relocation {
                blob: r.u32()?,
                offset: r.u32()?,
                symbol: r.u32()?,
            });
        }

        let debug = if flags & FLAG_HAS_DEBUG != 0 {
            let mut per_blob = Vec::new();
            for _ in 0..blob_count {
                let label_count = r.u32()? as usize;
                let mut labels = Vec::new();
                for _ in 0..label_count {
                    let name = name_of(r.u32()?)?;
                    let offset = r.u32()?;
                    labels.push((name, offset));
                }
                let line_count = r.u32()? as usize;
                let mut lines = Vec::new();
                for _ in 0..line_count {
                    lines.push((r.u32()?, r.u32()?));
                }
                per_blob.push(BlobDebug { labels, lines });
            }
            Some(per_blob)
        } else {
            None
        };

        // v3 sections. Pre-v3 objects must not claim v3 flags; v3 objects
        // read the trailing sections written by `to_bytes_v3`.
        let (signatures, table_blobs, variants, table_fixups, bound_calls) =
            if version >= OBJECT_FORMAT_VERSION_V3 {
                let signatures = if flags & FLAG_HAS_SIGNATURES != 0 {
                    let mut sigs = Vec::new();
                    for _ in 0..blob_count {
                        let arity = r.u8()?;
                        if !(1..=16).contains(&arity) {
                            return Err(FormatError::Malformed("signature arity out of range"));
                        }
                        let mut cardinalities = Vec::new();
                        for _ in 0..arity {
                            let c = r.u32()?;
                            if c == 0 {
                                return Err(FormatError::Malformed("zero cardinality"));
                            }
                            cardinalities.push(c);
                        }
                        sigs.push(RoutineSig {
                            arity,
                            cardinalities,
                        });
                    }
                    Some(sigs)
                } else {
                    None
                };

                let table_blobs = if flags & FLAG_HAS_TABLES != 0 {
                    let mut tables = Vec::new();
                    for _ in 0..blob_count {
                        let len = r.u32()? as usize;
                        tables.push(r.bytes(len)?.to_vec());
                    }
                    Some(tables)
                } else {
                    None
                };

                let variants = if flags & FLAG_HAS_VARIANTS != 0 {
                    let variant_count = r.u32()? as usize;
                    if variant_count != blob_count {
                        return Err(FormatError::Malformed("variants section length mismatch"));
                    }
                    let mut tags = Vec::with_capacity(variant_count);
                    for _ in 0..variant_count {
                        tags.push(match r.u8()? {
                            0 => BlobVariant::Normal,
                            1 => BlobVariant::Volatile,
                            2 => BlobVariant::Both,
                            _ => return Err(FormatError::Malformed("unknown blob variant tag")),
                        });
                    }
                    Some(tags)
                } else {
                    None
                };

                let fixup_count = r.u32()? as usize;
                let mut table_fixups = Vec::new();
                for _ in 0..fixup_count {
                    let blob = r.u32()?;
                    let offset = r.u32()?;
                    let table_offset = r.u32()?;
                    let code = blobs
                        .get(blob as usize)
                        .ok_or(FormatError::Malformed("fixup out of range"))?;
                    if u64::from(offset) + 4 > code.len() as u64 {
                        return Err(FormatError::Malformed("fixup out of range"));
                    }
                    // A fixup addresses its blob's own table blob; without a
                    // table section there is nothing for it to rebase into.
                    let Some(tables) = &table_blobs else {
                        return Err(FormatError::Malformed("fixup out of range"));
                    };
                    let table = tables
                        .get(blob as usize)
                        .ok_or(FormatError::Malformed("fixup out of range"))?;
                    if table_offset as usize >= table.len() {
                        return Err(FormatError::Malformed("fixup out of range"));
                    }
                    table_fixups.push(TableFixup {
                        blob,
                        offset,
                        table_offset,
                    });
                }

                let bound_call_count = r.u32()? as usize;
                let mut bound_calls = Vec::new();
                for _ in 0..bound_call_count {
                    let blob = r.u32()?;
                    let offset = r.u32()?;
                    let symbol = r.u32()?;
                    let tape_count = r.u8()? as usize;
                    if blob as usize >= blob_count {
                        return Err(FormatError::Malformed("bound call blob index out of range"));
                    }
                    if u64::from(offset) + 4 > blobs[blob as usize].len() as u64 {
                        return Err(FormatError::Malformed("bound call offset out of range"));
                    }
                    if symbol as usize >= symbol_count {
                        return Err(FormatError::Malformed(
                            "bound call symbol index out of range",
                        ));
                    }
                    let mut binding = Vec::new();
                    for _ in 0..tape_count {
                        let caller_tape = r.u8()?;
                        if caller_tape >= 16 {
                            return Err(FormatError::Malformed("caller tape index out of range"));
                        }
                        let pair_count = r.u16()? as usize;
                        let mut pairs = Vec::new();
                        for _ in 0..pair_count {
                            let src = r.u32()?;
                            let dst = r.u32()?;
                            let flags_byte = r.u8()?;
                            if (flags_byte & !1) != 0 {
                                return Err(FormatError::Malformed("reserved map-pair flags"));
                            }
                            pairs.push(MapPair {
                                src,
                                dst,
                                dst_label: None,
                                one_way: flags_byte & 1 != 0,
                            });
                        }
                        binding.push(TapeBinding {
                            caller_tape,
                            param: None,
                            map_written: false,
                            open: false,
                            pairs,
                        });
                    }
                    bound_calls.push(BoundCall {
                        blob,
                        offset,
                        symbol,
                        binding,
                        exits: Vec::new(),
                    });
                }

                (signatures, table_blobs, variants, table_fixups, bound_calls)
            } else {
                if flags
                    & (FLAG_HAS_SIGNATURES
                        | FLAG_HAS_TABLES
                        | FLAG_HAS_VARIANTS
                        | FLAG_PROGRAM_VOLATILE)
                    != 0
                {
                    return Err(FormatError::Malformed("v3 flags in pre-v3 object"));
                }
                (None, None, None, Vec::new(), Vec::new())
            };

        r.finish()?;

        let mut symbols = Vec::new();
        for (name_idx, kind, blob) in raw_symbols {
            let name = name_of(name_idx)?;
            let def = match kind {
                0 => {
                    if blob != EXTERNAL_BLOB {
                        return Err(FormatError::Malformed("external symbol carries a blob"));
                    }
                    SymbolDef::External
                }
                1 => {
                    if blob as usize >= blobs.len() {
                        return Err(FormatError::Malformed("symbol blob index out of range"));
                    }
                    SymbolDef::Defined { blob }
                }
                2 => {
                    if blob as usize >= blobs.len() {
                        return Err(FormatError::Malformed("symbol blob index out of range"));
                    }
                    SymbolDef::Local { blob }
                }
                _ => return Err(FormatError::Malformed("unknown symbol kind")),
            };
            symbols.push(Symbol { name, def });
        }

        for reloc in &relocations {
            let blob = blobs
                .get(reloc.blob as usize)
                .ok_or(FormatError::Malformed("relocation blob index out of range"))?;
            if reloc.symbol as usize >= symbols.len() {
                return Err(FormatError::Malformed(
                    "relocation symbol index out of range",
                ));
            }
            if u64::from(reloc.offset) + 4 > blob.len() as u64 {
                return Err(FormatError::Malformed("relocation outside blob"));
            }
        }

        Ok(Self {
            arch,
            symbols,
            blobs,
            relocations,
            debug,
            signatures,
            table_blobs,
            table_fixups,
            bound_calls,
            variants,
            program_volatile: flags & FLAG_PROGRAM_VOLATILE != 0,
            interface: None,
            grafts: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::{ARCH_PM1, FormatError};

    fn sample() -> ObjectFile {
        ObjectFile::v2(
            ARCH_PM1,
            vec![
                Symbol {
                    name: "main".into(),
                    def: SymbolDef::Defined { blob: 0 },
                },
                Symbol {
                    name: "goToEnd".into(),
                    def: SymbolDef::External,
                },
            ],
            // ent, call <4-byte hole>, stp
            vec![vec![0x0D, 0x0B, 0, 0, 0, 0, 0x02]],
            vec![Relocation {
                blob: 0,
                offset: 2,
                symbol: 1,
            }],
            None,
        )
    }

    /// An object without v3 data serializes byte-for-byte as v2 — this
    /// pins what PM-1's compiler emits.
    #[test]
    fn v2_shape_is_byte_identical_v2() {
        let obj = sample(); // signatures/table_blobs None, fixups/bound_calls empty
        let bytes = obj.to_bytes();
        assert_eq!(&bytes[0..3], b"MO\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2);
        assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
    }

    #[test]
    fn round_trip_without_debug() {
        let bytes = sample().to_bytes();
        assert_eq!(&bytes[0..3], b"MO\x01");
        let back = ObjectFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn round_trip_with_local_symbol() {
        let mut obj = sample();
        // A second blob for a local-only helper function.
        obj.blobs.push(vec![0x0D, 0x02]); // ent, stp
        obj.symbols.push(Symbol {
            name: "helper".into(),
            def: SymbolDef::Local { blob: 1 },
        });
        let bytes = obj.to_bytes();
        // Wire version field sits right after the 3-byte magic.
        assert_eq!(
            u16::from_le_bytes([bytes[3], bytes[4]]),
            OBJECT_FORMAT_VERSION_V2
        );
        assert_eq!(OBJECT_FORMAT_VERSION_V2, 2);
        let back = ObjectFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, obj);
    }

    #[test]
    fn version_1_bytes_are_still_accepted() {
        // Valid v2 bytes of an object WITHOUT locals, downgraded to v1: the
        // reader must still accept it (1..=OBJECT_FORMAT_VERSION_V2).
        let mut bytes = sample().to_bytes();
        bytes[3..5].copy_from_slice(&1u16.to_le_bytes());
        crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
        assert!(ObjectFile::from_bytes(&bytes).is_ok());
    }

    #[test]
    fn local_symbol_with_bad_blob_rejected() {
        let mut obj = sample();
        obj.symbols[0].def = SymbolDef::Local { blob: 7 };
        let bytes = obj.to_bytes();
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("symbol blob index out of range"))
        ));
    }

    #[test]
    fn round_trip_with_debug() {
        let mut obj = sample();
        obj.debug = Some(vec![BlobDebug {
            labels: vec![("L1".into(), 1)],
            lines: vec![(0, 3), (1, 4)],
        }]);
        let bytes = obj.to_bytes();
        let back = ObjectFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, obj);
    }

    #[test]
    fn crc_corruption_rejected() {
        let mut bytes = sample().to_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::BadCrc { .. })
        ));
    }

    #[test]
    fn reloc_offset_out_of_blob_rejected() {
        let mut obj = sample();
        obj.relocations[0].offset = 5; // 5 + 4 > blob len 7
        let bytes = obj.to_bytes();
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("relocation outside blob"))
        ));
    }

    #[test]
    fn defined_symbol_with_bad_blob_rejected() {
        let mut obj = sample();
        obj.symbols[0].def = SymbolDef::Defined { blob: 7 };
        let bytes = obj.to_bytes();
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("symbol blob index out of range"))
        ));
    }

    #[test]
    fn huge_wire_count_is_rejected_without_allocating() {
        let mut bytes = sample().to_bytes();
        // string count is the first u32 after the 11-byte header
        // (magic 3 + version 2 + arch 1 + flags 1 + crc 4)
        bytes[11..15].copy_from_slice(&u32::MAX.to_le_bytes());
        crate::formats::crc32::stamp_crc(&mut bytes, 7);
        assert!(ObjectFile::from_bytes(&bytes).is_err());
    }

    #[test]
    fn unicode_symbol_names_survive() {
        let mut obj = sample();
        obj.symbols[0].name = "иди_в_конец".into();
        let bytes = obj.to_bytes();
        let back = ObjectFile::from_bytes(&bytes).unwrap();
        assert_eq!(back.symbols[0].name, "иди_в_конец");
    }

    fn sample_v3_sigs() -> ObjectFile {
        let mut obj = sample();
        obj.signatures = Some(vec![RoutineSig {
            arity: 2,
            cardinalities: vec![3, 128],
        }]);
        obj.table_blobs = Some(vec![vec![2, 1, 0, 1, 0x7F]]);
        obj
    }

    #[test]
    fn v3_signatures_and_tables_round_trip() {
        let obj = sample_v3_sigs();
        let bytes = obj.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
        assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
    }

    #[test]
    fn v3_signature_arity_bounds_enforced() {
        for bad_arity in [0u8, 17] {
            let mut obj = sample_v3_sigs();
            obj.signatures = Some(vec![RoutineSig {
                arity: bad_arity,
                cardinalities: vec![3; bad_arity as usize],
            }]);
            assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
        }
    }

    #[test]
    fn v3_zero_cardinality_rejected() {
        let mut obj = sample_v3_sigs();
        obj.signatures = Some(vec![RoutineSig {
            arity: 1,
            cardinalities: vec![0],
        }]);
        assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
    }

    /// The two flag-gated v3 sections are independent: a signatures-only
    /// object (no table blobs) round-trips.
    #[test]
    fn v3_signatures_only_round_trips() {
        let mut obj = sample_v3_sigs();
        obj.table_blobs = None;
        let bytes = obj.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
        assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
    }

    #[test]
    fn v2_file_still_loads_with_empty_v3_fields() {
        let v2 = sample();
        let back = ObjectFile::from_bytes(&v2.to_bytes()).unwrap();
        assert!(back.signatures.is_none() && back.table_blobs.is_none());
        assert!(back.table_fixups.is_empty() && back.bound_calls.is_empty());
    }

    fn sample_v3_full() -> ObjectFile {
        let mut obj = sample_v3_sigs();
        obj.table_fixups = vec![TableFixup {
            blob: 0,
            offset: 2,
            table_offset: 0,
        }];
        obj.bound_calls = vec![BoundCall {
            blob: 0,
            offset: 1,
            symbol: 0,
            binding: vec![TapeBinding {
                caller_tape: 2,
                param: None,
                map_written: false,
                open: false,
                pairs: vec![
                    MapPair {
                        src: 1,
                        dst: 3,
                        dst_label: None,
                        one_way: false,
                    },
                    MapPair {
                        src: 4,
                        dst: 0,
                        dst_label: None,
                        one_way: true,
                    }, // '^' => blank
                ],
            }],
            exits: Vec::new(),
        }];
        obj
    }

    #[test]
    fn v3_full_round_trip_preserves_one_way() {
        let obj = sample_v3_full();
        let back = ObjectFile::from_bytes(&obj.to_bytes()).unwrap();
        assert_eq!(back, obj);
        assert!(back.bound_calls[0].binding[0].pairs[1].one_way);
        assert!(!back.bound_calls[0].binding[0].pairs[0].one_way);
    }

    #[test]
    fn v3_bound_call_indices_validated() {
        // blob out of range
        let mut obj = sample_v3_full();
        obj.bound_calls[0].blob = 99;
        assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
        // symbol out of range
        let mut obj = sample_v3_full();
        obj.bound_calls[0].symbol = 99;
        assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
        // caller_tape >= 16
        let mut obj = sample_v3_full();
        obj.bound_calls[0].binding[0].caller_tape = 16;
        assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
    }

    #[test]
    fn v3_fixup_indices_validated() {
        let mut obj = sample_v3_full();
        obj.table_fixups[0].blob = 99;
        assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
    }

    #[test]
    fn v3_fixup_without_tables_rejected() {
        // A fixup rebases into its blob's table blob; with no table section
        // there is nothing to rebase into, so the object is malformed.
        let mut obj = sample_v3_full();
        obj.table_blobs = None;
        assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
    }

    #[test]
    fn v3_bound_call_offset_overrun_rejected() {
        // The offset's 4-byte hole must satisfy offset..offset + 4 <= blob len.
        // offset == len - 3 leaves the hole overrunning the blob, so it is
        // rejected — while the sample's in-bounds offset still round-trips.
        let mut obj = sample_v3_full();
        let blob_len = obj.blobs[obj.bound_calls[0].blob as usize].len() as u32;
        obj.bound_calls[0].offset = blob_len - 3;
        assert!(ObjectFile::from_bytes(&obj.to_bytes()).is_err());
        assert!(ObjectFile::from_bytes(&sample_v3_full().to_bytes()).is_ok());
    }

    #[test]
    fn v3_pair_reserved_flags_rejected() {
        // Hand-corrupt the pair-flags byte to set a reserved bit, restamp CRC.
        let obj = sample_v3_full();
        let mut bytes = obj.to_bytes();
        // The LAST pair-flags byte in the file is the final byte before nothing
        // else follows it in this sample (bound_calls is the last section and
        // the one-way pair is its last pair): flags byte == 0x01 at the end.
        let pos = bytes.len() - 1;
        assert_eq!(
            bytes[pos], 0x01,
            "layout assumption: trailing one-way flag byte"
        );
        bytes[pos] = 0x03; // set a reserved bit
        crate::formats::crc32::stamp_crc(&mut bytes, 7);
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("reserved map-pair flags"))
        ));
    }

    /// A byte string captured from the pre-variant-tags encoder, pinned so
    /// adding `variants`/`program_volatile` to `ObjectFile` cannot shift a
    /// single byte for an object that leaves both fields at their defaults
    /// (`None`/`false`) — no shape drift for tag-free objects, ever.
    #[rustfmt::skip]
    const V3_FULL_LEGACY_BYTES: [u8; 155] = [
        0x4d, 0x4f, 0x01, 0x03, 0x00, 0x01, 0x06, 0x9a, 0x8d, 0x41, 0x37, 0x02,
        0x00, 0x00, 0x00, 0x04, 0x00, 0x6d, 0x61, 0x69, 0x6e, 0x07, 0x00, 0x67,
        0x6f, 0x54, 0x6f, 0x45, 0x6e, 0x64, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
        0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00,
        0x0d, 0x0b, 0x00, 0x00, 0x00, 0x00, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
        0x03, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
        0x02, 0x01, 0x00, 0x01, 0x7f, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x02, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    ];

    #[test]
    fn variants_absent_stays_byte_identical_to_pre_variant_encoding() {
        let obj = sample_v3_full();
        assert!(obj.variants.is_none() && !obj.program_volatile);
        assert_eq!(obj.to_bytes(), V3_FULL_LEGACY_BYTES);
    }

    #[test]
    fn variants_and_program_volatile_round_trip() {
        let mut obj = sample_v3_full();
        obj.variants = Some(vec![BlobVariant::Both]); // one blob in sample_v3_full
        obj.program_volatile = true;
        let bytes = obj.to_bytes();
        assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
    }

    #[test]
    fn multi_blob_variants_round_trip() {
        let mut obj = sample();
        obj.blobs.push(vec![0x0D, 0x02]); // ent, stp
        obj.symbols.push(Symbol {
            name: "helper".into(),
            def: SymbolDef::Local { blob: 1 },
        });
        obj.variants = Some(vec![BlobVariant::Normal, BlobVariant::Volatile]);
        obj.program_volatile = true;
        let bytes = obj.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
        assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
    }

    /// One blob per tag, exercising all three `BlobVariant` values in a
    /// single object alongside `program_volatile`.
    #[test]
    fn all_three_variant_tags_round_trip() {
        let mut obj = sample();
        obj.blobs.push(vec![0x0D, 0x02]); // ent, stp
        obj.symbols.push(Symbol {
            name: "helper_a".into(),
            def: SymbolDef::Local { blob: 1 },
        });
        obj.blobs.push(vec![0x0D, 0x02]); // ent, stp
        obj.symbols.push(Symbol {
            name: "helper_b".into(),
            def: SymbolDef::Local { blob: 2 },
        });
        obj.variants = Some(vec![
            BlobVariant::Normal,
            BlobVariant::Volatile,
            BlobVariant::Both,
        ]);
        obj.program_volatile = true;
        let bytes = obj.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
        assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
    }

    /// Legacy bytes (no variants flag) decode to `None`/`false`, not a
    /// stringly-typed "no variants" marker.
    #[test]
    fn legacy_object_decodes_to_no_variants() {
        let obj = sample_v3_full();
        let back = ObjectFile::from_bytes(&obj.to_bytes()).unwrap();
        assert_eq!(back.variants, None);
        assert!(!back.program_volatile);
    }

    #[test]
    fn program_volatile_without_variants_round_trips() {
        // The two new fields are independent: the header bit can be set with
        // no per-blob variant section present at all.
        let mut obj = sample();
        obj.program_volatile = true;
        let bytes = obj.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 3);
        let back = ObjectFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, obj);
    }

    #[test]
    fn variants_length_mismatch_rejected() {
        // Build a VALID object (`variants.len() == blobs.len()`, so the
        // encoder's `debug_assert_eq!` doesn't fire) and hand-corrupt the
        // on-wire count to disagree with the blob count — the shape a
        // decoder actually has to reject; the encoder can no longer produce
        // it directly now that it asserts the invariant on the way out.
        let mut obj = sample(); // 1 blob
        obj.variants = Some(vec![BlobVariant::Normal]);
        assert!(obj.table_fixups.is_empty() && obj.bound_calls.is_empty());
        let mut bytes = obj.to_bytes();
        assert_eq!(&bytes[bytes.len() - 8..], [0, 0, 0, 0, 0, 0, 0, 0]); // both trailing counts = 0
        let count_pos = bytes.len() - 9 - 4; // the section's u32 count, right before its one tag byte
        assert_eq!(&bytes[count_pos..count_pos + 4], &1u32.to_le_bytes());
        bytes[count_pos..count_pos + 4].copy_from_slice(&2u32.to_le_bytes()); // claims 2 tags, blob_count is 1
        crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("variants section length mismatch"))
        ));
    }

    #[test]
    fn variants_bad_tag_byte_rejected() {
        // `sample()` has no signatures/table_blobs and empty table_fixups/
        // bound_calls, so its v3 encoding ends with the variants section
        // (`u32 count = 1` + one tag byte) immediately followed by the two
        // unconditional trailing counts, both zero: the tag byte sits at a
        // fixed, structurally-known offset from the end (len - 4 - 4 - 1),
        // with no risk of an accidental byte collision elsewhere in the file.
        let mut obj = sample(); // 1 blob
        obj.variants = Some(vec![BlobVariant::Normal]);
        assert!(obj.table_fixups.is_empty() && obj.bound_calls.is_empty());
        let mut bytes = obj.to_bytes();
        assert_eq!(&bytes[bytes.len() - 8..], [0, 0, 0, 0, 0, 0, 0, 0]); // both trailing counts = 0
        let tag_pos = bytes.len() - 9;
        assert_eq!(bytes[tag_pos], 0); // BlobVariant::Normal's tag byte
        bytes[tag_pos] = 3; // outside 0..=2
        crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("unknown blob variant tag"))
        ));
    }

    #[test]
    fn pre_v3_object_claiming_flag_has_variants_rejected() {
        let mut bytes = sample().to_bytes(); // v2 shape
        assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 2);
        bytes[6] |= FLAG_HAS_VARIANTS; // flags byte (magic 3 + version 2 + arch 1)
        crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("v3 flags in pre-v3 object"))
        ));
    }

    #[test]
    fn pre_v3_object_claiming_flag_program_volatile_rejected() {
        let mut bytes = sample().to_bytes(); // v2 shape
        bytes[6] |= FLAG_PROGRAM_VOLATILE; // flags byte
        crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("v3 flags in pre-v3 object"))
        ));
    }

    fn v4_sample() -> ObjectFile {
        let mut obj = sample();
        obj.signatures = Some(vec![RoutineSig {
            arity: 1,
            cardinalities: vec![3],
        }]);
        obj.interface = Some(Interface {
            routines: vec![RoutineInterface {
                params: vec!["num".into()],
                glyphs: vec![vec!["_".into(), "0".into(), "1".into()]],
                writes: vec![vec!["0".into(), "1".into()]],
                enters: vec![None],
                leaves: vec![None],
                opaque: vec![false],
                exits: 0,
                returns: true,
            }],
            alphabets: vec![ExportedAlphabet {
                name: "bits".into(),
                glyphs: vec!["_".into(), "0".into(), "1".into()],
            }],
            graphs: vec![ExportedGraph {
                name: "lib::findA".into(),
                digest: 0xDEAD_BEEF,
            }],
        });
        obj.grafts = vec![GraftProvenance {
            graph: "other::g".into(),
            digest: 0x1234_5678,
        }];
        obj
    }

    #[test]
    fn v4_shape_is_detected_and_v3_shape_stays_v3() {
        assert!(!sample().is_v4_shape());
        assert!(v4_sample().is_v4_shape());
        let mut symbolic = sample();
        symbolic.bound_calls.push(BoundCall {
            blob: 0,
            offset: 1,
            symbol: 0,
            binding: vec![TapeBinding {
                caller_tape: 0,
                param: Some("num".into()),
                map_written: false,
                open: false,
                pairs: Vec::new(),
            }],
            exits: Vec::new(),
        });
        assert!(symbolic.is_v4_shape(), "a named binding entry needs v4");
    }

    /// The v4 flag bit is its own: overlapping an existing one would change
    /// how a v2/v3 object reads back, and `NO_STRING` reuses the all-ones
    /// sentinel the blob index already spells.
    #[test]
    fn v4_wire_constants_do_not_collide() {
        let taken = FLAG_HAS_DEBUG
            | FLAG_HAS_SIGNATURES
            | FLAG_HAS_TABLES
            | FLAG_HAS_VARIANTS
            | FLAG_PROGRAM_VOLATILE;
        assert_eq!(FLAG_HAS_INTERFACE & taken, 0);
        assert_eq!(NO_STRING, EXTERNAL_BLOB);
        assert_eq!(OBJECT_FORMAT_VERSION_V4, OBJECT_FORMAT_VERSION_V3 + 1);
    }

    /// One object per remaining `is_v4_shape` trigger: each field alone is
    /// enough, so dropping any one disjunct from the predicate turns a case
    /// red. The `interface`, `grafts` and `param` triggers are covered by
    /// `v4_shape_is_detected_and_v3_shape_stays_v3`.
    #[test]
    fn every_v4_only_field_alone_forces_v4_shape() {
        // A v3-shape bound call: positional, no labels, no exits.
        let v3_call = || BoundCall {
            blob: 0,
            offset: 1,
            symbol: 0,
            binding: vec![TapeBinding {
                caller_tape: 0,
                param: None,
                map_written: false,
                open: false,
                pairs: vec![MapPair {
                    src: 1,
                    dst: 2,
                    dst_label: None,
                    one_way: false,
                }],
            }],
            exits: Vec::new(),
        };
        let mut plain = sample();
        plain.bound_calls.push(v3_call());
        assert!(!plain.is_v4_shape(), "a positional bound call stays v3");

        let mut exits = sample();
        let mut call = v3_call();
        call.exits = vec![6]; // inside the sample blob, like any code offset
        exits.bound_calls.push(call);
        assert!(exits.is_v4_shape(), "an exit vector needs v4");

        let mut written = sample();
        let mut call = v3_call();
        call.binding[0].map_written = true;
        written.bound_calls.push(call);
        assert!(written.is_v4_shape(), "a written-empty map needs v4");

        // `open` alone, deliberately without the `map_written` it implies:
        // setting both would let the `map_written` trigger carry this case
        // and leave a dropped `open` disjunct undetected.
        let mut open = sample();
        let mut call = v3_call();
        call.binding[0].open = true;
        open.bound_calls.push(call);
        assert!(open.is_v4_shape(), "an open map needs v4");

        let mut labelled = sample();
        let mut call = v3_call();
        call.binding[0].pairs[0].dst_label = Some("1".into());
        labelled.bound_calls.push(call);
        assert!(labelled.is_v4_shape(), "a glyph label needs v4");

        let mut grafts = sample();
        grafts.grafts = vec![GraftProvenance {
            graph: "other::g".into(),
            digest: 1,
        }];
        assert!(grafts.is_v4_shape(), "graft provenance needs v4");

        let mut interface = sample();
        interface.signatures = Some(vec![RoutineSig {
            arity: 1,
            cardinalities: vec![3],
        }]); // an interface describes a signed routine
        interface.interface = Some(Interface::default());
        assert!(interface.is_v4_shape(), "an interface section needs v4");
    }
}
