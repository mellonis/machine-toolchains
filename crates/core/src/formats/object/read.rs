use super::*;
use crate::formats::FormatError;
use crate::formats::crc32::verify_crc;
use crate::formats::io::Reader;

/// Resolve a string-pool index, the reader's one lookup failure.
fn string_at(strings: &[String], idx: u32) -> Result<String, FormatError> {
    strings
        .get(idx as usize)
        .cloned()
        .ok_or(FormatError::Malformed("string index out of range"))
}

/// Read one glyph-label list written by `put_glyphs`.
fn read_glyphs(r: &mut Reader<'_>, strings: &[String]) -> Result<Vec<String>, FormatError> {
    let count = r.u8()? as usize;
    let mut glyphs = Vec::new();
    for _ in 0..count {
        glyphs.push(string_at(strings, r.u32()?)?);
    }
    Ok(glyphs)
}

impl ObjectFile {
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
        if !(1..=OBJECT_FORMAT_VERSION_V4).contains(&version) {
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
        let name_of = |idx: u32| -> Result<String, FormatError> { string_at(&strings, idx) };

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
                        // v4 widens the entry with the callee parameter it
                        // binds and the binding flags; v3 entries are
                        // positional and never open, and their written-ness
                        // is read off the pairs below.
                        let (param, v4_map_written, open) = if version >= OBJECT_FORMAT_VERSION_V4 {
                            let param_idx = r.u32()?;
                            let param = if param_idx == NO_STRING {
                                None
                            } else {
                                Some(name_of(param_idx)?)
                            };
                            let binding_flags = r.u8()?;
                            if binding_flags & !0b11 != 0 {
                                return Err(FormatError::Malformed("reserved binding flags"));
                            }
                            (param, binding_flags & 0b01 != 0, binding_flags & 0b10 != 0)
                        } else {
                            (None, false, false)
                        };
                        let pair_count = r.u16()? as usize;
                        let mut pairs = Vec::new();
                        for _ in 0..pair_count {
                            let src = r.u32()?;
                            let dst = r.u32()?;
                            let flags_byte = r.u8()?;
                            // Bit 1 — "the destination is a glyph label" —
                            // is v4's; a v3 stream setting it is malformed.
                            let allowed = if version >= OBJECT_FORMAT_VERSION_V4 {
                                0b11
                            } else {
                                0b01
                            };
                            if (flags_byte & !allowed) != 0 {
                                return Err(FormatError::Malformed("reserved map-pair flags"));
                            }
                            let dst_label = if flags_byte & 0b10 != 0 {
                                Some(name_of(dst)?)
                            } else {
                                None
                            };
                            pairs.push(MapPair {
                                src,
                                dst: if dst_label.is_some() { 0 } else { dst },
                                dst_label,
                                one_way: flags_byte & 1 != 0,
                            });
                        }
                        // v3 carries no flag, but it does carry the pairs,
                        // and a map with pairs was written by definition —
                        // which is exactly why only the written-EMPTY map
                        // forces v4. v4 streams state the flag outright.
                        let map_written = if version >= OBJECT_FORMAT_VERSION_V4 {
                            v4_map_written
                        } else {
                            !pairs.is_empty()
                        };
                        // The writer's invariant: a binding with pairs, and
                        // an open binding, both have `map_written` set — a
                        // map with pairs was written by definition, and an
                        // open map is a written one
                        // (docs/formats.md (bound calls)). A hand-crafted
                        // stream can spell the flag clear anyway, and the
                        // value would then be one the writer refuses to
                        // re-encode, so normalize on the way in. One
                        // expression covers both branches above: v3's
                        // `map_written` is already `!pairs.is_empty()`, so
                        // only the v4 branch's effective value changes.
                        let map_written = map_written || open || !pairs.is_empty();
                        binding.push(TapeBinding {
                            caller_tape,
                            param,
                            map_written,
                            open,
                            pairs,
                        });
                    }
                    let exits = if version >= OBJECT_FORMAT_VERSION_V4 {
                        let exit_count = r.u8()? as usize;
                        let mut exits = Vec::new();
                        for _ in 0..exit_count {
                            let exit = r.u32()?;
                            if u64::from(exit) >= blobs[blob as usize].len() as u64 {
                                return Err(FormatError::Malformed("bound-call exit outside blob"));
                            }
                            exits.push(exit);
                        }
                        exits
                    } else {
                        Vec::new()
                    };
                    bound_calls.push(BoundCall {
                        blob,
                        offset,
                        symbol,
                        binding,
                        exits,
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

        // v4 sections: the flag-gated interface, then the unconditional
        // graft-provenance list (docs/formats.md (routine interfaces)). A
        // pre-v4 stream must not claim the interface flag — the section it
        // announces is not there to read, and reading it back as "no
        // interface" would silently accept a corrupted header.
        let (interface, grafts) = if version >= OBJECT_FORMAT_VERSION_V4 {
            // Bits 6 and 7 are unassigned. A v4 stream setting one was
            // written by something this reader does not understand, so it
            // says so instead of decoding half a file; the pre-v4 arms keep
            // their historical tolerance for bits they never defined.
            const KNOWN_FLAGS: u8 = FLAG_HAS_DEBUG
                | FLAG_HAS_SIGNATURES
                | FLAG_HAS_TABLES
                | FLAG_HAS_VARIANTS
                | FLAG_PROGRAM_VOLATILE
                | FLAG_HAS_INTERFACE;
            if flags & !KNOWN_FLAGS != 0 {
                return Err(FormatError::Malformed("reserved object flag bits set"));
            }
            let interface = if flags & FLAG_HAS_INTERFACE != 0 {
                let Some(sigs) = &signatures else {
                    return Err(FormatError::Malformed("interface without signatures"));
                };
                let mut routines = Vec::new();
                for sig in sigs {
                    let mut params = Vec::new();
                    let mut glyphs = Vec::new();
                    let mut writes = Vec::new();
                    let mut enters = Vec::new();
                    let mut leaves = Vec::new();
                    let mut opaque = Vec::new();
                    for &card in &sig.cardinalities {
                        params.push(name_of(r.u32()?)?);
                        let tape_glyphs = read_glyphs(&mut r, &strings)?;
                        if tape_glyphs.len() as u64 != u64::from(card) {
                            return Err(FormatError::Malformed(
                                "interface glyph count differs from cardinality",
                            ));
                        }
                        let tape_writes = read_glyphs(&mut r, &strings)?;
                        for glyph in &tape_writes {
                            if !tape_glyphs.contains(glyph) {
                                return Err(FormatError::Malformed(
                                    "writes glyph outside its alphabet",
                                ));
                            }
                        }
                        let tape_flags = r.u8()?;
                        if tape_flags & !0b111 != 0 {
                            return Err(FormatError::Malformed("reserved interface tape flags"));
                        }
                        let mut clause = |present: bool| -> Result<_, FormatError> {
                            if !present {
                                return Ok(None);
                            }
                            let listed = read_glyphs(&mut r, &strings)?;
                            // The source languages reject `enters {}`, so a
                            // present-but-empty clause never leaves a writer.
                            if listed.is_empty() {
                                return Err(FormatError::Malformed("empty head clause"));
                            }
                            for glyph in &listed {
                                if !tape_glyphs.contains(glyph) {
                                    return Err(FormatError::Malformed(
                                        "contract glyph outside its alphabet",
                                    ));
                                }
                            }
                            Ok(Some(listed))
                        };
                        enters.push(clause(tape_flags & 0b001 != 0)?);
                        leaves.push(clause(tape_flags & 0b010 != 0)?);
                        opaque.push(tape_flags & 0b100 != 0);
                        glyphs.push(tape_glyphs);
                        writes.push(tape_writes);
                    }
                    let exits = r.u8()?;
                    let returns = match r.u8()? {
                        0 => false,
                        1 => true,
                        _ => return Err(FormatError::Malformed("returns byte")),
                    };
                    routines.push(RoutineInterface {
                        params,
                        glyphs,
                        writes,
                        enters,
                        leaves,
                        opaque,
                        exits,
                        returns,
                    });
                }
                let alphabet_count = r.u32()? as usize;
                let mut alphabets = Vec::new();
                for _ in 0..alphabet_count {
                    let name = name_of(r.u32()?)?;
                    alphabets.push(ExportedAlphabet {
                        name,
                        glyphs: read_glyphs(&mut r, &strings)?,
                    });
                }
                let graph_count = r.u32()? as usize;
                let mut graphs = Vec::new();
                for _ in 0..graph_count {
                    graphs.push(ExportedGraph {
                        name: name_of(r.u32()?)?,
                        digest: r.u32()?,
                    });
                }
                Some(Interface {
                    routines,
                    alphabets,
                    graphs,
                })
            } else {
                None
            };
            let graft_count = r.u32()? as usize;
            let mut grafts = Vec::new();
            for _ in 0..graft_count {
                grafts.push(GraftProvenance {
                    graph: name_of(r.u32()?)?,
                    digest: r.u32()?,
                });
            }
            (interface, grafts)
        } else {
            if flags & FLAG_HAS_INTERFACE != 0 {
                return Err(FormatError::Malformed("v4 flags in pre-v4 object"));
            }
            (None, Vec::new())
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
            interface,
            grafts,
        })
    }
}
