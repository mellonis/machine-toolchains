use super::*;
use crate::formats::crc32::stamp_crc;
use crate::formats::io::{put_u16, put_u32};

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

/// Write one glyph-label list — a `u8` count followed by that many string
/// indices — the shape the v4 interface section repeats for glyphs, written
/// sets, and head clauses (docs/formats.md (routine interfaces)).
fn put_glyphs(out: &mut Vec<u8>, pool: &mut StringPool, glyphs: &[String]) {
    out.push(u8::try_from(glyphs.len()).expect("glyph list fits u8"));
    for glyph in glyphs {
        put_u32(out, pool.intern(glyph));
    }
}

impl ObjectFile {
    /// Serialize at the LOWEST version that carries the object's content.
    /// The v4 shape is asked first on purpose: an object whose only v4
    /// content is graft provenance is v2-shape as well, and a fall-through
    /// dispatch would write it as v2 and drop the grafts.
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.is_v4_shape() {
            self.to_bytes_v3_or_v4(OBJECT_FORMAT_VERSION_V4)
        } else if self.is_v2_shape() {
            self.to_bytes_v2()
        } else {
            self.to_bytes_v3_or_v4(OBJECT_FORMAT_VERSION_V3)
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

    /// Serialize a v3- or v4-shape object: the v2 body through the debug
    /// section (version field = the `version` passed in, flags gaining
    /// `FLAG_HAS_SIGNATURES` / `FLAG_HAS_TABLES` / `FLAG_HAS_VARIANTS` /
    /// `FLAG_PROGRAM_VOLATILE` when the respective field is present/set),
    /// followed by the v3 sections — per-blob signatures, per-blob table
    /// blobs, the per-blob variant-tag section, the unconditional
    /// table-fixup section, and the unconditional bound-call section. At v4
    /// the bound-call records widen (parameter name, binding flags, glyph
    /// labels, exit vector) and two sections follow: the flag-gated
    /// interface section and the unconditional graft-provenance list
    /// (docs/formats.md (routine interfaces)). Read back by `from_bytes` in
    /// the same order.
    fn to_bytes_v3_or_v4(&self, version: u16) -> Vec<u8> {
        let mut pool = StringPool::new();
        let symbol_names: Vec<u32> = self.symbols.iter().map(|s| pool.intern(&s.name)).collect();
        let debug_label_names: Vec<Vec<u32>> = match &self.debug {
            Some(per_blob) => per_blob
                .iter()
                .map(|d| d.labels.iter().map(|(n, _)| pool.intern(n)).collect())
                .collect(),
            None => Vec::new(),
        };
        // The pool is dumped before the sections that use it, so every v4
        // string has to be interned here, ahead of the dump; `intern` is
        // idempotent, so the section writers below hand out the same
        // indices. A string missed here would be appended after the dump
        // and read back as an out-of-range index.
        if version >= OBJECT_FORMAT_VERSION_V4 {
            for call in &self.bound_calls {
                for tape in &call.binding {
                    if let Some(param) = &tape.param {
                        pool.intern(param);
                    }
                    for pair in &tape.pairs {
                        if let Some(label) = &pair.dst_label {
                            pool.intern(label);
                        }
                    }
                }
            }
            if let Some(iface) = &self.interface {
                for routine in &iface.routines {
                    for name in &routine.params {
                        pool.intern(name);
                    }
                    let clauses = routine.enters.iter().chain(&routine.leaves);
                    for glyph in routine
                        .glyphs
                        .iter()
                        .chain(&routine.writes)
                        .chain(clauses.flatten())
                        .flatten()
                    {
                        pool.intern(glyph);
                    }
                }
                for alphabet in &iface.alphabets {
                    pool.intern(&alphabet.name);
                    for glyph in &alphabet.glyphs {
                        pool.intern(glyph);
                    }
                }
                for graph in &iface.graphs {
                    pool.intern(&graph.name);
                }
                for import in &iface.imports {
                    pool.intern(&import.name);
                    for glyph in &import.glyphs {
                        pool.intern(glyph);
                    }
                }
            }
            for graft in &self.grafts {
                pool.intern(&graft.graph);
            }
        }

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
        if version >= OBJECT_FORMAT_VERSION_V4 && self.interface.is_some() {
            flags |= FLAG_HAS_INTERFACE;
        }

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_OBJECT);
        put_u16(&mut out, version);
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
                // A map with pairs was written by definition — the flag is
                // what a v3 stream derives from the pair count, so a value
                // that disagrees reads back as a DIFFERENT binding.
                debug_assert!(
                    tape.pairs.is_empty() || tape.map_written,
                    "a tape binding that carries pairs has `map_written` set"
                );
                out.push(tape.caller_tape);
                if version >= OBJECT_FORMAT_VERSION_V4 {
                    put_u32(
                        &mut out,
                        tape.param
                            .as_deref()
                            .map_or(NO_STRING, |param| pool.intern(param)),
                    );
                    out.push(u8::from(tape.map_written) | (u8::from(tape.open) << 1));
                }
                put_u16(
                    &mut out,
                    u16::try_from(tape.pairs.len()).expect("pair count fits u16"),
                );
                for pair in &tape.pairs {
                    // The wire carries the label in the `dst` field's place,
                    // so a labelled pair's index is not written and comes
                    // back 0: anything else is silently dropped here.
                    debug_assert!(
                        pair.dst_label.is_none() || pair.dst == 0,
                        "a glyph-labelled pair carries `dst: 0`"
                    );
                    put_u32(&mut out, pair.src);
                    // v3 has no glyph labels, and no v3-shape object carries
                    // one — `is_v4_shape` routes any that does to v4.
                    let (dst, label_bit) = match &pair.dst_label {
                        Some(label) => (pool.intern(label), 0b10),
                        None => (pair.dst, 0),
                    };
                    put_u32(&mut out, dst);
                    out.push(u8::from(pair.one_way) | label_bit);
                }
            }
            if version >= OBJECT_FORMAT_VERSION_V4 {
                out.push(u8::try_from(call.exits.len()).expect("exit count fits u8"));
                for &exit in &call.exits {
                    put_u32(&mut out, exit);
                }
            }
        }

        // v4 sections: the flag-gated interface, then the unconditional
        // graft-provenance list.
        if version >= OBJECT_FORMAT_VERSION_V4 {
            if let Some(iface) = &self.interface {
                debug_assert_eq!(
                    iface.routines.len(),
                    self.blobs.len(),
                    "interface must parallel blobs"
                );
                for (blob_index, routine) in iface.routines.iter().enumerate() {
                    let arity = routine.params.len();
                    debug_assert!(
                        routine.glyphs.len() == arity
                            && routine.writes.len() == arity
                            && routine.enters.len() == arity
                            && routine.leaves.len() == arity
                            && routine.opaque.len() == arity,
                        "every per-tape list must have one entry per parameter"
                    );
                    // The writer walks the parameters, the reader walks the
                    // signature's cardinalities: a disagreement writes a
                    // stream that cannot be read back. (A missing signatures
                    // section is not asserted here — the reader rejects an
                    // interface without one, and a value that ill-formed
                    // must reach that rejection, not a panic.)
                    if let Some(sig) = self.signatures.as_ref().and_then(|s| s.get(blob_index)) {
                        debug_assert_eq!(
                            arity, sig.arity as usize,
                            "an interface's parameters must match its signature's arity"
                        );
                    }
                    for k in 0..arity {
                        put_u32(&mut out, pool.intern(&routine.params[k]));
                        put_glyphs(&mut out, &mut pool, &routine.glyphs[k]);
                        put_glyphs(&mut out, &mut pool, &routine.writes[k]);
                        let tape_flags = u8::from(routine.enters[k].is_some())
                            | (u8::from(routine.leaves[k].is_some()) << 1)
                            | (u8::from(routine.opaque[k]) << 2);
                        out.push(tape_flags);
                        for clause in [&routine.enters[k], &routine.leaves[k]]
                            .into_iter()
                            .flatten()
                        {
                            put_glyphs(&mut out, &mut pool, clause);
                        }
                    }
                    out.push(routine.exits);
                    out.push(u8::from(routine.returns));
                }
                put_u32(
                    &mut out,
                    u32::try_from(iface.alphabets.len()).expect("alphabet count fits u32"),
                );
                for alphabet in &iface.alphabets {
                    put_u32(&mut out, pool.intern(&alphabet.name));
                    put_glyphs(&mut out, &mut pool, &alphabet.glyphs);
                }
                put_u32(
                    &mut out,
                    u32::try_from(iface.graphs.len()).expect("graph count fits u32"),
                );
                for graph in &iface.graphs {
                    put_u32(&mut out, pool.intern(&graph.name));
                    put_u32(&mut out, graph.digest);
                }
                put_u32(
                    &mut out,
                    u32::try_from(iface.imports.len()).expect("import count fits u32"),
                );
                for import in &iface.imports {
                    put_u32(&mut out, pool.intern(&import.name));
                    put_u32(
                        &mut out,
                        u32::try_from(import.glyphs.len()).expect("import glyph count fits u32"),
                    );
                    for glyph in &import.glyphs {
                        put_u32(&mut out, pool.intern(glyph));
                    }
                }
            }
            put_u32(
                &mut out,
                u32::try_from(self.grafts.len()).expect("graft count fits u32"),
            );
            for graft in &self.grafts {
                put_u32(&mut out, pool.intern(&graft.graph));
                put_u32(&mut out, graft.digest);
            }
        }

        stamp_crc(&mut out, CRC_OFFSET);
        out
    }
}
