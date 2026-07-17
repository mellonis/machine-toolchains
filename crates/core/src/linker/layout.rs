//! Layout, call relaxation, and `MX` code emission (docs/stdlib.md
//! (linking); docs/isa.md for the relaxation width rule itself).
//!
//! Each reached function's ORIGINAL blob is decoded exactly once into a
//! `Piece` list. A monotone shrink fixpoint then decides which far calls
//! become short (jump widths never change — only calls can shrink).
//! Finally every function is re-emitted from that SAME original decode,
//! through an offset map built fresh from the converged widths: calls are
//! patched against the final function bases, and jumps are re-encoded at
//! their original width with the offset recomputed through the map (the
//! shrink-only invariant guarantees it still fits).

use std::collections::{BTreeMap, HashMap, HashSet};

use super::resolve::FuncRef;
use super::{LinkError, MapFunction};
use crate::asm::decode::{self, Body, DecodedOperand};
use crate::asm::{ArchSyntax, Flow};
use crate::vm::OperandKind;

/// Which table a fixup hole references, and its owning opcode's offset —
/// inferred from the referencing operand kind, not the table bytes (a
/// concatenated table blob is not self-describing). A plain `TableRef`
/// operand sits one byte after its opcode (`Match` if the opcode falls
/// through, `Dispatch` if it transfers); a `FramedCall`'s frame half sits
/// five bytes after its opcode (`Frame`). This is the linker's mirror of
/// the disassembler's kind inference.
#[derive(Clone, Copy)]
enum RefKind {
    Match,
    Dispatch,
    Frame,
}

/// The `(opcode offset, RefKind)` for a fixup hole, or `None` when neither
/// a `TableRef` opcode precedes the hole by one byte nor a `FramedCall`
/// opcode precedes it by five (a malformed fixup).
fn ref_kind(syntax: &ArchSyntax, blob: &[u8], hole: u32) -> Option<(u32, RefKind)> {
    if let Some(op) = hole
        .checked_sub(1)
        .and_then(|p| blob.get(p as usize))
        .copied()
        && let Some(entry) = syntax.by_opcode(op)
        && entry.operand == OperandKind::TableRef
    {
        let kind = if entry.flow == Flow::FallThrough {
            RefKind::Match
        } else {
            RefKind::Dispatch
        };
        return Some((hole - 1, kind));
    }
    if let Some(op) = hole
        .checked_sub(5)
        .and_then(|p| blob.get(p as usize))
        .copied()
        && let Some(entry) = syntax.by_opcode(op)
        && entry.operand == OperandKind::FramedCall
    {
        return Some((hole - 5, RefKind::Frame));
    }
    None
}

/// One classified piece of a function's ORIGINAL blob (offsets are
/// blob-relative, i.e. relative to that function's own `ent`).
enum Piece {
    Verbatim {
        orig: u32,
        bytes: Vec<u8>,
    },
    Jump {
        orig: u32,
        opcode: u8,
        /// Operand width in bytes: 1 (`RelI8`) or 4 (`RelI32`).
        width: u8,
        orig_target: u32,
    },
    /// A symbol site (call or relocated tail jump). `orig` is the opcode's
    /// address; the hole is `orig + 1`.
    CallSite {
        orig: u32,
        callee: usize,
    },
    /// A framed call (`call.m`): a fixed 9-byte instruction — opcode, a
    /// 4-byte displacement half (patched to the callee like a far call,
    /// but NEVER relaxed in 5a), and a 4-byte frame-descriptor table half
    /// (patched by the table-fixup pass). The rel hole is `orig + 1`, the
    /// frame hole `orig + 5`.
    FramedCall {
        orig: u32,
        callee: usize,
    },
}

impl Piece {
    fn orig(&self) -> u32 {
        match self {
            Piece::Verbatim { orig, .. }
            | Piece::Jump { orig, .. }
            | Piece::CallSite { orig, .. }
            | Piece::FramedCall { orig, .. } => *orig,
        }
    }
}

pub(super) struct Built {
    pub code: Vec<u8>,
    /// The concatenated table section (docs/formats.md (executable
    /// image)): each reached function's table blob in layout order, its
    /// dispatch entries rebased to absolute code offsets. Empty when no
    /// reached function carries tables.
    pub tables: Vec<u8>,
    pub functions: Vec<MapFunction>,
    pub relaxed_calls: u32,
    pub far_calls: u32,
    /// True when the reached image carries a frame descriptor or a framed
    /// call — the linker emits `PROFILE_FRAMES` iff this holds (else
    /// `PROFILE_BASE`), keeping frameless links byte-identical.
    pub frames_present: bool,
    /// Offset into `tables` where the emitted frames region begins
    /// (docs/formats.md (frames region)), or 0 when no framed call is
    /// present.
    pub frames_offset: u32,
}

/// One raw (hand-authored) framed-call site collected during emission: the
/// absolute code offset of its 4-byte frame half (rewritten to the site
/// index once the directory is known) and the absolute table offset of the
/// descriptor it names. Raw sites lower to constant compose columns
/// (docs/formats.md (frames region)).
struct RawSite {
    code_hole: usize,
    desc_abs: u32,
}

/// Decode `f`'s original blob into a `Piece` list. Decode failure, a call
/// instruction with no matching hole in `f.calls`, or a hole in `f.calls`
/// that no decoded call instruction ever consumes → `MalformedBlob`. The
/// last case matters just as much as the first two: a hole that lands
/// inside a non-call piece (raw bytes, or the middle of some other
/// decoded instruction) would otherwise be copied verbatim — emitting the
/// relocation's zeroed operand as silent garbage in an otherwise
/// CRC-valid, plausible-looking executable. Also raised when a blob's
/// first byte is not the entry opcode.
fn classify(syntax: &ArchSyntax, f: &FuncRef) -> Result<Vec<Piece>, LinkError> {
    let blob: &[u8] = &f.blob;

    // Every linked function must begin with its `ent` prologue (the ABI
    // `.func` guarantees). A blob that doesn't would trap at its first
    // call landing anyway — fail at link time instead.
    if f.blob.first() != Some(&syntax.entry_opcode) {
        return Err(LinkError::MalformedBlob {
            symbol: f.name.to_string(),
            at: 0,
        });
    }

    let call_holes: HashMap<u32, usize> = f.calls.iter().copied().collect();
    let mut consumed_holes: HashSet<u32> = HashSet::new();
    let decoded = decode::decode_stream(syntax, blob, 0, blob.len() as u32);

    let mut pieces = Vec::with_capacity(decoded.len());
    for d in decoded {
        let addr = d.addr;
        let len = d.len;
        match d.body {
            Body::Raw(_) => {
                return Err(LinkError::MalformedBlob {
                    symbol: f.name.to_string(),
                    at: addr,
                });
            }
            Body::Instr { mnemonic, operand } => {
                let entry = syntax
                    .by_mnemonic(mnemonic)
                    .expect("mnemonic came from a successful decode against this syntax");
                match (entry.flow, operand) {
                    (Flow::Jump | Flow::Branch, DecodedOperand::RelTarget(orig_target)) => {
                        let hole = addr + 1;
                        if let Some(&callee) = call_holes.get(&hole) {
                            // A relocated symbol jump (tail call). Branches
                            // are labels-only in v1 — a holed branch is a
                            // malformed object, not a feature.
                            if entry.flow == Flow::Branch {
                                return Err(LinkError::MalformedBlob {
                                    symbol: f.name.to_string(),
                                    at: hole,
                                });
                            }
                            consumed_holes.insert(hole);
                            pieces.push(Piece::CallSite { orig: addr, callee });
                        } else {
                            pieces.push(Piece::Jump {
                                orig: addr,
                                opcode: entry.opcode,
                                width: (len - 1) as u8,
                                orig_target,
                            });
                        }
                    }
                    (Flow::Call, DecodedOperand::RelTarget(_)) => {
                        let hole = addr + 1;
                        let Some(&callee) = call_holes.get(&hole) else {
                            return Err(LinkError::MalformedBlob {
                                symbol: f.name.to_string(),
                                at: hole,
                            });
                        };
                        consumed_holes.insert(hole);
                        pieces.push(Piece::CallSite { orig: addr, callee });
                    }
                    // A framed call: the displacement half (hole `addr + 1`)
                    // relocates to the callee exactly like a far call; the
                    // frame half (`addr + 5`) is a table fixup, handled by
                    // the table-fixup pass. Never relaxed in 5a.
                    (Flow::Call, DecodedOperand::FramedCall { .. }) => {
                        let hole = addr + 1;
                        let Some(&callee) = call_holes.get(&hole) else {
                            return Err(LinkError::MalformedBlob {
                                symbol: f.name.to_string(),
                                at: hole,
                            });
                        };
                        consumed_holes.insert(hole);
                        pieces.push(Piece::FramedCall { orig: addr, callee });
                    }
                    _ => {
                        pieces.push(Piece::Verbatim {
                            orig: addr,
                            bytes: blob[addr as usize..(addr + len) as usize].to_vec(),
                        });
                    }
                }
            }
        }
    }

    // `f.calls` is already in blob order (see `FuncRef::calls`), so the
    // first unconsumed entry is the lowest-offset one — deterministic.
    if let Some(&(offset, _)) = f
        .calls
        .iter()
        .find(|(off, _)| !consumed_holes.contains(off))
    {
        return Err(LinkError::MalformedBlob {
            symbol: f.name.to_string(),
            at: offset,
        });
    }

    Ok(pieces)
}

fn piece_size(piece: &Piece, is_short: bool) -> u32 {
    match piece {
        Piece::Verbatim { bytes, .. } => bytes.len() as u32,
        Piece::Jump { width, .. } => 1 + u32::from(*width),
        Piece::CallSite { .. } => {
            if is_short {
                2
            } else {
                5
            }
        }
        // opcode + 4-byte displacement + 4-byte frame table ref, fixed.
        Piece::FramedCall { .. } => 9,
    }
}

/// Full relayout from scratch under the current width vector: per-function
/// sizes, prefix-sum bases (main — `functions[0]` — at 0), and each
/// piece's blob-relative offset within its own function.
fn layout_pass(functions: &[Vec<Piece>], is_short: &[Vec<bool>]) -> (Vec<u32>, Vec<Vec<u32>>) {
    let mut sizes = Vec::with_capacity(functions.len());
    let mut offsets = Vec::with_capacity(functions.len());
    for (pieces, shorts) in functions.iter().zip(is_short) {
        let mut off = 0u32;
        let mut piece_offsets = Vec::with_capacity(pieces.len());
        for (piece, &short) in pieces.iter().zip(shorts) {
            piece_offsets.push(off);
            off += piece_size(piece, short);
        }
        offsets.push(piece_offsets);
        sizes.push(off);
    }
    let mut bases = Vec::with_capacity(functions.len());
    let mut base = 0u32;
    for &size in &sizes {
        bases.push(base);
        base += size;
    }
    (bases, offsets)
}

/// Appends one function's table blob to the executable's table section
/// (docs/formats.md (executable image)), rebasing every dispatch entry
/// from a blob-relative code offset to its absolute address in the
/// linked code — `code_base` + the entry's post-relaxation offset from
/// the SAME `orig_to_new` map jump re-encoding uses, so entries follow
/// labels that relaxation shifted.
///
/// A table blob is not self-describing: kinds come from the referencing
/// instructions, exactly as object disassembly infers them — the opcode
/// one byte before each fixup hole; a `FallThrough` flow is a pure
/// lookup (match table), any control transfer dispatches THROUGH its
/// table (dispatch table). Match tables span `3 + width*count` bytes
/// (`width u8`, `count u16 LE`, rows), dispatch tables `2 + 4*count`
/// (`count u16 LE`, `count` u32 LE entries). Every byte must belong to
/// exactly one fixup-attributed table — a gap, overlap, truncated
/// header, or dispatch entry off instruction boundaries is a
/// [`LinkError::MalformedTable`] whose `at` is the table-blob-relative
/// offset of the first offending byte.
fn append_function_tables(
    syntax: &ArchSyntax,
    f: &FuncRef,
    code_base: u32,
    orig_to_new: &HashMap<u32, u32>,
    out: &mut Vec<u8>,
) -> Result<(), LinkError> {
    // Distinct table starts and their kinds; duplicate references to one
    // table collapse to the first classifier.
    let mut starts: BTreeMap<u32, RefKind> = BTreeMap::new();
    for &(hole, table_off) in &f.table_fixups {
        let kind = ref_kind(syntax, &f.blob, hole).map_or(RefKind::Match, |(_, k)| k);
        starts.entry(table_off).or_insert(kind);
    }
    let malformed = |at: u32| LinkError::MalformedTable {
        symbol: f.name.to_string(),
        at,
    };

    let tb: &[u8] = &f.table;
    let len = tb.len() as u32;
    let mut pos = 0u32;
    for (&start, &kind) in &starts {
        if start != pos {
            // A gap before this table (uncovered bytes) or an overlap
            // with the previous one.
            return Err(malformed(pos.min(start)));
        }
        let end = match kind {
            RefKind::Dispatch => {
                let header_end = start.checked_add(2).filter(|&e| e <= len);
                let Some(header_end) = header_end else {
                    return Err(malformed(start));
                };
                let count = u32::from(u16::from_le_bytes([
                    tb[start as usize],
                    tb[start as usize + 1],
                ]));
                let end = header_end + 4 * count;
                if end > len {
                    return Err(malformed(start));
                }
                out.extend_from_slice(&tb[start as usize..header_end as usize]);
                for k in 0..count {
                    let at = header_end + 4 * k;
                    rebase_entry(tb, at, code_base, orig_to_new, out, &malformed)?;
                }
                end
            }
            RefKind::Frame => {
                append_frame_descriptor(tb, start, len, code_base, orig_to_new, out, &malformed)?
            }
            RefKind::Match => {
                let header_end = start.checked_add(3).filter(|&e| e <= len);
                let Some(header_end) = header_end else {
                    return Err(malformed(start));
                };
                let width = u32::from(tb[start as usize]);
                let count = u32::from(u16::from_le_bytes([
                    tb[start as usize + 1],
                    tb[start as usize + 2],
                ]));
                let end = header_end + width * count;
                if end > len {
                    return Err(malformed(start));
                }
                // Match rows carry symbol indices, not code offsets —
                // copied verbatim.
                out.extend_from_slice(&tb[start as usize..end as usize]);
                end
            }
        };
        pos = end;
    }
    if pos != len {
        return Err(malformed(pos));
    }
    Ok(())
}

/// Reads a blob-relative code offset from `tb` at `at`, rebases it through
/// `orig_to_new` + `code_base` (an off-boundary offset is malformed table
/// data no rebase can make sense of), and appends the absolute u32 LE.
/// Shared by dispatch entries and frame exit vectors.
fn rebase_entry(
    tb: &[u8],
    at: u32,
    code_base: u32,
    orig_to_new: &HashMap<u32, u32>,
    out: &mut Vec<u8>,
    malformed: &impl Fn(u32) -> LinkError,
) -> Result<(), LinkError> {
    let bytes: [u8; 4] = tb[at as usize..(at + 4) as usize]
        .try_into()
        .expect("bounds checked by caller");
    let orig = u32::from_le_bytes(bytes);
    let Some(&new) = orig_to_new.get(&orig) else {
        return Err(malformed(at));
    };
    out.extend_from_slice(&(code_base + new).to_le_bytes());
    Ok(())
}

/// Walks a frame descriptor (docs/formats.md (frame descriptors)) by its
/// self-describing header: `arity u8`, `exit_count u16 LE`, per-tape
/// `[phys u8, rmap_len u16 + entries, wmap_len u16 + entries]`, then
/// `exit_count × u32 LE` blob-relative code offsets. The header + maps
/// copy verbatim (symbol data, not code offsets); the exit vector rebases
/// exactly like dispatch entries. Returns the descriptor's end offset. A
/// truncated header, an arity outside 1..=16, or an exit off an
/// instruction boundary is `MalformedTable` at the offending offset.
#[allow(clippy::too_many_arguments)]
fn append_frame_descriptor(
    tb: &[u8],
    start: u32,
    len: u32,
    code_base: u32,
    orig_to_new: &HashMap<u32, u32>,
    out: &mut Vec<u8>,
    malformed: &impl Fn(u32) -> LinkError,
) -> Result<u32, LinkError> {
    let u16_at = |p: u32| -> Option<u32> {
        if p + 2 > len {
            return None;
        }
        Some(u32::from(u16::from_le_bytes([
            tb[p as usize],
            tb[p as usize + 1],
        ])))
    };
    // arity u8, exit_count u16.
    let mut pos = start;
    let Some(&arity) = tb.get(pos as usize) else {
        return Err(malformed(start));
    };
    if arity == 0 || arity > 16 {
        return Err(malformed(start));
    }
    pos += 1;
    let Some(exit_count) = u16_at(pos) else {
        return Err(malformed(start));
    };
    pos += 2;
    // Per-tape: phys u8, rmap_len u16 + entries, wmap_len u16 + entries.
    for _ in 0..arity {
        if pos >= len {
            return Err(malformed(start));
        }
        pos += 1; // phys
        for _ in 0..2 {
            let Some(map_len) = u16_at(pos) else {
                return Err(malformed(start));
            };
            pos += 2 + 2 * map_len;
        }
    }
    // The maps end and the exit vector begins here.
    let exits_start = pos;
    if exits_start > len {
        return Err(malformed(start));
    }
    // Header + maps copy verbatim (symbol data).
    out.extend_from_slice(&tb[start as usize..exits_start as usize]);
    let end = exits_start + 4 * exit_count;
    if end > len {
        return Err(malformed(start));
    }
    for k in 0..exit_count {
        rebase_entry(
            tb,
            exits_start + 4 * k,
            code_base,
            orig_to_new,
            out,
            malformed,
        )?;
    }
    Ok(end)
}

pub(super) fn build(
    syntax: &ArchSyntax,
    order: &[FuncRef],
    relax: bool,
    plan: Option<&super::engine::FramesPlan>,
) -> Result<Built, LinkError> {
    let functions: Vec<Vec<Piece>> = order
        .iter()
        .map(|f| classify(syntax, f))
        .collect::<Result<_, _>>()?;

    // Width vector: every call site starts FAR. Only calls whose opcode has
    // a short partner are ever candidates for the fixpoint.
    let mut is_short: Vec<Vec<bool>> = functions.iter().map(|p| vec![false; p.len()]).collect();
    let has_short_partner: Vec<Vec<bool>> = functions
        .iter()
        .zip(order)
        .map(|(pieces, f)| {
            pieces
                .iter()
                .map(|p| match p {
                    Piece::CallSite { orig, .. } => {
                        syntax.short_of(f.blob[*orig as usize]).is_some()
                    }
                    _ => false,
                })
                .collect()
        })
        .collect();

    if relax {
        loop {
            let (bases, offsets) = layout_pass(&functions, &is_short);
            let mut grew = false;
            for (fi, pieces) in functions.iter().enumerate() {
                for (pi, piece) in pieces.iter().enumerate() {
                    let Piece::CallSite { callee, .. } = piece else {
                        continue;
                    };
                    if is_short[fi][pi] || !has_short_partner[fi][pi] {
                        continue;
                    }
                    let instr_end = bases[fi] + offsets[fi][pi] + 5;
                    let off = i64::from(bases[*callee]) - i64::from(instr_end);
                    if i8::try_from(off).is_ok() {
                        is_short[fi][pi] = true;
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
    }

    // Final, converged layout — the ONLY one used for emission.
    let (bases, offsets) = layout_pass(&functions, &is_short);

    let mut code = Vec::new();
    let mut tables = Vec::new();
    let mut map_functions = Vec::with_capacity(order.len());
    let mut relaxed_calls = 0u32;
    let mut far_calls = 0u32;
    let mut frames_present = false;
    // Raw framed-call sites in (function, piece) emission order; used only
    // on the plan-less path (a pure hand-authored / 5a image), where each
    // becomes a dense 0-based call site with a constant compose column.
    let mut raw_sites: Vec<RawSite> = Vec::new();
    // Every framed-call piece's frame-half code hole, in (function, piece)
    // emission order — the engine's compose columns are indexed by this same
    // order. The frames-region pass writes each its dense site index.
    let mut fcall_holes: Vec<usize> = Vec::new();
    // Each function's table-section base, for resolving raw directory
    // entries the engine plan names by (function, table offset).
    let mut func_table_bases: Vec<u32> = Vec::with_capacity(order.len());

    for (fi, f) in order.iter().enumerate() {
        let pieces = &functions[fi];
        let base = bases[fi];
        let piece_offsets = &offsets[fi];
        debug_assert_eq!(code.len() as u32, base);
        // This function's table base — stable across the code loop below
        // (tables grow only in the per-function append that follows it), so
        // a framed call can resolve its descriptor's absolute offset here.
        let table_base = tables.len() as u32;
        func_table_bases.push(table_base);

        // orig blob offset -> new offset within THIS function; needed to
        // resolve jump targets (arbitrary earlier/later instruction
        // boundaries) and to remap debug labels/lines.
        let orig_to_new: HashMap<u32, u32> = pieces
            .iter()
            .enumerate()
            .map(|(pi, piece)| (piece.orig(), piece_offsets[pi]))
            .collect();

        for (pi, piece) in pieces.iter().enumerate() {
            match piece {
                Piece::Verbatim { bytes, .. } => code.extend_from_slice(bytes),
                Piece::Jump {
                    orig,
                    opcode,
                    width,
                    orig_target,
                } => {
                    let Some(&target_new) = orig_to_new.get(orig_target) else {
                        // Not an instruction boundary of this function —
                        // a malformed blob, not a layout bug.
                        return Err(LinkError::MalformedBlob {
                            symbol: f.name.to_string(),
                            at: *orig,
                        });
                    };
                    let new_target = base + target_new;
                    let new_end = base + piece_offsets[pi] + 1 + u32::from(*width);
                    let off = i64::from(new_target) - i64::from(new_end);
                    code.push(*opcode);
                    match *width {
                        1 => {
                            let off8 = i8::try_from(off).expect(
                                "shrink-only invariant: jump still fits its original width",
                            );
                            code.push(off8 as u8);
                        }
                        4 => {
                            let off32 = i32::try_from(off).expect("jump offset fits i32");
                            code.extend(off32.to_le_bytes());
                        }
                        _ => unreachable!("jump operand width is always 1 or 4"),
                    }
                }
                Piece::CallSite { orig, callee } => {
                    let far_opcode = f.blob[*orig as usize];
                    let new_start = piece_offsets[pi];
                    if is_short[fi][pi] {
                        let short_opcode = syntax
                            .short_of(far_opcode)
                            .expect("marked short only when a short partner exists");
                        let new_end = base + new_start + 2;
                        let off = i64::from(bases[*callee]) - i64::from(new_end);
                        let off8 = i8::try_from(off)
                            .expect("relaxation fixpoint guarantees short calls fit i8");
                        code.push(short_opcode);
                        code.push(off8 as u8);
                        relaxed_calls += 1;
                    } else {
                        let new_end = base + new_start + 5;
                        let off = i64::from(bases[*callee]) - i64::from(new_end);
                        let off32 = i32::try_from(off).expect("call offset fits i32");
                        code.push(far_opcode);
                        code.extend(off32.to_le_bytes());
                        far_calls += 1;
                    }
                }
                Piece::FramedCall { orig, callee } => {
                    // Fixed 9 bytes, never relaxed: opcode + displacement
                    // (patched to the callee like a far call) + the frame
                    // table half, which becomes the call SITE index once
                    // the directory is built (docs/formats.md (frames
                    // region)). A placeholder is emitted here and rewritten
                    // below.
                    frames_present = true;
                    let opcode = f.blob[*orig as usize];
                    let new_end = base + piece_offsets[pi] + 9;
                    let off = i64::from(bases[*callee]) - i64::from(new_end);
                    let off32 = i32::try_from(off).expect("framed-call offset fits i32");
                    code.push(opcode);
                    code.extend(off32.to_le_bytes());
                    let code_hole = code.len();
                    code.extend_from_slice(&f.blob[(*orig + 5) as usize..(*orig + 9) as usize]);
                    fcall_holes.push(code_hole);
                    if plan.is_none() {
                        // Plan-less (hand-authored / 5a) path: the frame fixup
                        // names this site's descriptor; its absolute offset is
                        // this function's table base plus the blob-local one.
                        let Some(&(_, table_off)) =
                            f.table_fixups.iter().find(|(hole, _)| *hole == *orig + 5)
                        else {
                            return Err(LinkError::MalformedBlob {
                                symbol: f.name.to_string(),
                                at: *orig + 5,
                            });
                        };
                        raw_sites.push(RawSite {
                            code_hole,
                            desc_abs: table_base + table_off,
                        });
                    }
                    far_calls += 1;
                }
            }
        }

        let end = code.len() as u32;

        // Table section (docs/formats.md (executable image)): this
        // function's tables are appended at the running section length —
        // its table base — with dispatch entries rebased through the
        // same `orig_to_new` map the jumps above used; then every
        // TableRef operand hole in the just-emitted code is patched from
        // its blob-local table offset to the final section offset.
        debug_assert_eq!(table_base, tables.len() as u32);
        if !f.table.is_empty() || !f.table_fixups.is_empty() {
            append_function_tables(syntax, f, base, &orig_to_new, &mut tables)?;
        }
        for &(hole, table_off) in &f.table_fixups {
            // The hole is a `TableRef` operand (opcode + 1) or the frame
            // half of a `FramedCall` (opcode + 5). Locate its opcode
            // boundary by referencing operand kind; a hole whose opcode is
            // not an instruction boundary is a malformed blob, not a
            // layout bug.
            let Some((opcode_at, kind)) = ref_kind(syntax, &f.blob, hole) else {
                return Err(LinkError::MalformedBlob {
                    symbol: f.name.to_string(),
                    at: hole,
                });
            };
            if matches!(kind, RefKind::Frame) {
                // A framed call's frame half is NOT patched to the
                // descriptor offset; it holds the call site index, written
                // once the directory is known (below).
                frames_present = true;
                continue;
            }
            let Some(&instr_new) = orig_to_new.get(&opcode_at) else {
                return Err(LinkError::MalformedBlob {
                    symbol: f.name.to_string(),
                    at: hole,
                });
            };
            // The operand offset within the instruction (1 or 5) carries
            // through the relayout unchanged.
            let patch_at = (base + instr_new + (hole - opcode_at)) as usize;
            code[patch_at..patch_at + 4].copy_from_slice(&(table_base + table_off).to_le_bytes());
        }

        let (labels, lines) = match &f.debug {
            Some(debug) => {
                let mut labels = Vec::with_capacity(debug.labels.len());
                for (name, off) in &debug.labels {
                    let Some(&new) = orig_to_new.get(off) else {
                        return Err(LinkError::MalformedBlob {
                            symbol: f.name.to_string(),
                            at: *off,
                        });
                    };
                    labels.push((name.clone(), base + new));
                }
                let mut lines = Vec::with_capacity(debug.lines.len());
                for (off, line) in &debug.lines {
                    let Some(&new) = orig_to_new.get(off) else {
                        return Err(LinkError::MalformedBlob {
                            symbol: f.name.to_string(),
                            at: *off,
                        });
                    };
                    lines.push((base + new, *line));
                }
                (labels, lines)
            }
            None => (Vec::new(), Vec::new()),
        };

        map_functions.push(MapFunction {
            name: f.name.to_string(),
            start: base,
            end,
            labels,
            lines,
        });
    }

    // The frames region (docs/formats.md (frames region)). Two paths:
    //  * with an engine plan, the composition engine already computed the
    //    directory (as sources) and the full compose matrix; layout resolves
    //    the address-dependent descriptor offsets and emits verbatim;
    //  * without a plan, this is a pure hand-authored (5a) image — build the
    //    directory from the raw sites' descriptors and emit constant columns.
    let frames_offset = match plan {
        Some(plan) => emit_planned_region(
            plan,
            &mut code,
            &mut tables,
            &fcall_holes,
            &func_table_bases,
        ),
        None if !raw_sites.is_empty() => {
            let mut directory: Vec<u32> = raw_sites.iter().map(|s| s.desc_abs).collect();
            directory.sort_unstable();
            directory.dedup();
            let composite_of = |desc_abs: u32| -> u16 {
                let idx = directory
                    .iter()
                    .position(|&d| d == desc_abs)
                    .expect("every site descriptor is in the directory");
                u16::try_from(idx + 1).expect("composite index fits u16")
            };
            // Rewrite each site's placeholder frame half to its site index.
            for (site, rs) in raw_sites.iter().enumerate() {
                let site = u32::try_from(site).expect("site index fits u32");
                code[rs.code_hole..rs.code_hole + 4].copy_from_slice(&site.to_le_bytes());
            }
            let k = u16::try_from(directory.len()).expect("composite count fits u16");
            let s = u16::try_from(raw_sites.len()).expect("site count fits u16");
            let frames_offset = u32::try_from(tables.len()).expect("frames offset fits u32");
            tables.extend(k.to_le_bytes());
            tables.extend(s.to_le_bytes());
            for &off in &directory {
                tables.extend(off.to_le_bytes());
            }
            // Rows = active FR 0..=K; every row of a raw site's column carries
            // the same composite index (the site is context-independent).
            for _fr in 0..=k {
                for rs in &raw_sites {
                    tables.extend(composite_of(rs.desc_abs).to_le_bytes());
                }
            }
            frames_offset
        }
        None => 0,
    };

    Ok(Built {
        code,
        tables,
        functions: map_functions,
        relaxed_calls,
        far_calls,
        frames_present,
        frames_offset,
    })
}

/// Emit the engine-planned frames region (docs/formats.md (frames region)):
/// append the synthesized descriptors, resolve the directory's
/// address-dependent offsets, write each framed-call piece its dense site
/// index (piece order matches the plan's compose columns), then emit the
/// `K u16, S u16`, directory, and `(K+1) × S` compose matrix. Returns the
/// region's offset into the table section.
fn emit_planned_region(
    plan: &super::engine::FramesPlan,
    code: &mut [u8],
    tables: &mut Vec<u8>,
    fcall_holes: &[usize],
    func_table_bases: &[u32],
) -> u32 {
    use super::engine::DirSource;

    // Append the synthesized (address-independent) descriptors.
    let mut engine_offsets: Vec<u32> = Vec::with_capacity(plan.engine_descriptors.len());
    for desc in &plan.engine_descriptors {
        engine_offsets.push(u32::try_from(tables.len()).expect("table offset fits u32"));
        tables.extend_from_slice(desc);
    }
    // Resolve the directory to absolute descriptor offsets.
    let directory: Vec<u32> = plan
        .directory
        .iter()
        .map(|src| match src {
            DirSource::Engine(i) => engine_offsets[*i],
            DirSource::Raw { func, table_offset } => func_table_bases[*func] + table_offset,
        })
        .collect();
    // Write each framed-call piece its dense site index.
    for (site, &hole) in fcall_holes.iter().enumerate() {
        let idx = u32::try_from(site).expect("site index fits u32");
        code[hole..hole + 4].copy_from_slice(&idx.to_le_bytes());
    }
    let k = u16::try_from(directory.len()).expect("composite count fits u16");
    let s = u16::try_from(fcall_holes.len()).expect("site count fits u16");
    let frames_offset = u32::try_from(tables.len()).expect("frames offset fits u32");
    tables.extend(k.to_le_bytes());
    tables.extend(s.to_le_bytes());
    for &off in &directory {
        tables.extend(off.to_le_bytes());
    }
    for row in &plan.compose {
        for &v in row {
            tables.extend(v.to_le_bytes());
        }
    }
    frames_offset
}

#[cfg(test)]
mod tests {
    use super::super::{LinkOptions, link};
    use crate::asm::assemble;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::asm::{Flow, RelaxPair, SyntaxEntry};
    use crate::vm::OperandKind;

    /// Fixture + a short call (0x31) so relaxation has a target form.
    fn syntax_with_short_call() -> crate::asm::ArchSyntax {
        let mut s = test_syntax();
        s.entries.push(SyntaxEntry {
            opcode: 0x31,
            mnemonic: "call.s",
            operand: OperandKind::RelI8,
            flow: Flow::Call,
        });
        s.relax_pairs.push(RelaxPair {
            far: 0x21,
            short: 0x31,
        });
        s
    }

    const TWO_FUNCS: &str = "\
.func main
        call    go
        stop
.func go
        nop
        ret
";

    #[test]
    fn links_two_functions_with_relaxed_call() {
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, TWO_FUNCS, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // main: [0E][31 off][02] = 4 bytes; go at 4: [0E][01][0B].
        // call.s at 1, end 3, target 4 → off +1.
        assert_eq!(
            out.executable.code,
            vec![0x0E, 0x31, 0x01, 0x02, 0x0E, 0x01, 0x0B]
        );
        assert_eq!(out.executable.entry, 0);
        assert_eq!(out.map.functions.len(), 2);
        assert_eq!(
            (
                out.map.functions[0].name.as_str(),
                out.map.functions[0].start,
                out.map.functions[0].end
            ),
            ("main", 0, 4)
        );
        assert_eq!(
            (
                out.map.functions[1].name.as_str(),
                out.map.functions[1].start,
                out.map.functions[1].end
            ),
            ("go", 4, 7)
        );
    }

    #[test]
    fn no_relax_keeps_far_calls() {
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, TWO_FUNCS, false).unwrap();
        let out = link(
            &syntax,
            &[obj],
            &[],
            LinkOptions {
                relax: false,
                ..Default::default()
            },
        )
        .unwrap();
        // main: [0E][21 off32][02] = 7 bytes; go at 7; call end 6 → off +1.
        assert_eq!(
            out.executable.code,
            vec![0x0E, 0x21, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x01, 0x0B]
        );
    }

    #[test]
    fn jump_spanning_a_shrunk_call_is_repatched() {
        // THE approved-design case: a backward jump over a call site.
        // L: nop ; call go ; jmp L ; stop  — the jmp crosses the call hole.
        let src = "\
.func main
L:      nop
        call    go
        jmp     L
        stop
.func go
        ret
";
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        // Original blob: [0E][01][21 hole][30 off][02]: jmp.s at 7..9, end 9,
        // target 1 → orig off = -8.
        assert_eq!(obj.blobs[0][7], 0x30);
        assert_eq!(obj.blobs[0][8] as i8, -8);
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // After shrink: [0E][01][31 off][30 off'][02] = 7 bytes; go at 7.
        // call.s at 2, end 4, target 7 → +3. jmp.s at 4..6, end 6, target 1 → -5.
        assert_eq!(
            out.executable.code,
            vec![0x0E, 0x01, 0x31, 0x03, 0x30, 0xFB, 0x02, 0x0E, 0x0B]
        );
    }

    #[test]
    fn debug_offsets_are_remapped() {
        let src = "\
.func main
        call    go
X:      stop
.func go
        ret
";
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, src, true).unwrap();
        // Original: X at blob offset 6 (after ent + 5-byte call).
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // Relaxed: call.s is 2 bytes → X moves to absolute 3.
        assert_eq!(out.map.functions[0].labels, vec![("X".to_string(), 3)]);
        assert!(!out.map.functions[0].lines.is_empty());
    }

    #[test]
    fn far_call_when_out_of_short_range() {
        // Pad main so the callee lands beyond +127 from the call site.
        let mut src = String::from(".func main\n        call    go\n");
        for _ in 0..150 {
            src.push_str("        nop\n");
        }
        src.push_str("        stop\n.func go\n        ret\n");
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, &src, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        assert_eq!(out.executable.code[1], 0x21, "call must stay far");
    }

    #[test]
    fn unconsumed_call_hole_is_malformed() {
        use crate::formats::object::{ObjectFile, Relocation, Symbol, SymbolDef};
        let syntax = syntax_with_short_call();
        // Blob is all nops — the reloc's hole at offset 2 sits inside plain
        // instructions and no call opcode precedes it.
        let obj = ObjectFile::v2(
            0x7E,
            vec![
                Symbol {
                    name: "main".into(),
                    def: SymbolDef::Defined { blob: 0 },
                },
                Symbol {
                    name: "go".into(),
                    def: SymbolDef::External,
                },
            ],
            vec![vec![0x0E, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02]],
            vec![Relocation {
                blob: 0,
                offset: 2,
                symbol: 1,
            }],
            None,
        );
        // `go` must resolve so we reach layout: provide it via a library.
        let lib = {
            let s = ".func go\n        ret\n";
            assemble(&syntax, 0x7E, s, false).unwrap()
        };
        let e = link(&syntax, &[obj], &[lib], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedBlob {
                symbol: "main".into(),
                at: 2
            }
        );
    }

    #[test]
    fn blob_without_ent_prologue_is_malformed() {
        use crate::formats::object::{ObjectFile, Symbol, SymbolDef};
        let syntax = syntax_with_short_call();
        let obj = ObjectFile::v2(
            0x7E,
            vec![Symbol {
                name: "main".into(),
                def: SymbolDef::Defined { blob: 0 },
            }],
            vec![vec![0x01, 0x02]], // nop, stop — no leading ent
            vec![],
            None,
        );
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedBlob {
                symbol: "main".into(),
                at: 0
            }
        );
    }

    #[test]
    fn jump_to_mid_instruction_is_malformed() {
        use crate::formats::object::{ObjectFile, Symbol, SymbolDef};
        let syntax = syntax_with_short_call();
        // [0E][30 FF][02]: jmp.s at 1 ends at 3, offset −1 → target 2 = the
        // middle of the jmp.s itself; boundaries are 0, 1, 3.
        let obj = ObjectFile::v2(
            0x7E,
            vec![Symbol {
                name: "main".into(),
                def: SymbolDef::Defined { blob: 0 },
            }],
            vec![vec![0x0E, 0x30, 0xFF, 0x02]],
            vec![],
            None,
        );
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedBlob {
                symbol: "main".into(),
                at: 1
            }
        );
    }

    #[test]
    fn debug_label_off_instruction_boundary_is_malformed() {
        use crate::formats::object::{BlobDebug, ObjectFile, Symbol, SymbolDef};
        let syntax = syntax_with_short_call();
        // [0E][30 00][02]: a VALID jump (target 3 = the stop) so layout
        // succeeds — but the debug label at 2 points into the jmp.s.
        let obj = ObjectFile::v2(
            0x7E,
            vec![Symbol {
                name: "main".into(),
                def: SymbolDef::Defined { blob: 0 },
            }],
            vec![vec![0x0E, 0x30, 0x00, 0x02]],
            vec![],
            Some(vec![BlobDebug {
                labels: vec![("X".into(), 2)],
                lines: vec![],
            }]),
        );
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedBlob {
                symbol: "main".into(),
                at: 2
            }
        );
    }

    #[test]
    fn tail_jump_relaxes_like_a_call() {
        let syntax = syntax_with_short_call();
        // main tail-jumps g: [ent][jmp @g] → linked short: [0E][30 off][0E][0B].
        let src = ".func main\n        jmp @g\n.func g\n        ret\n";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // jmp.s at 1, end 3, g at 3 → off 0.
        assert_eq!(out.executable.code, vec![0x0E, 0x30, 0x00, 0x0E, 0x0B]);
        assert_eq!(out.report.relaxed_calls, 1);
    }

    // -- Tables: section emission, guards, and the PM-1-shape lock ------

    /// `syntax_with_short_call()` + the neutral table mnemonics (per the
    /// per-file-helper convention, mirroring `assembler.rs`): `tmatch`
    /// references a match table (FallThrough), `tdispatch` a dispatch
    /// table (Stop), caps all on so `.section`/`.row`/`.targets`/
    /// `.routine` shape.
    fn fake_table_syntax() -> crate::asm::ArchSyntax {
        use crate::asm::AsmCaps;
        let mut s = syntax_with_short_call();
        s.entries.push(SyntaxEntry {
            opcode: 0x11,
            mnemonic: "tmatch",
            operand: OperandKind::TableRef,
            flow: Flow::FallThrough,
        });
        s.entries.push(SyntaxEntry {
            opcode: 0x12,
            mnemonic: "tdispatch",
            operand: OperandKind::TableRef,
            flow: Flow::Stop,
        });
        s.caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        s
    }

    const TABLED_MAIN: &str = "\
.routine main, tapes=2, alpha=(3, 5)
.section tables
T0: .row [1, 2]
    .row [1, *]
D0: .targets A, B
.section code
.func main
        tmatch  T0
        tdispatch D0
A:      nop
B:      stop
";

    #[test]
    fn a_bound_call_in_an_unsigned_entry_is_missing_signature() {
        // The composition engine needs the machine signature to compose a
        // binding; an unsigned entry has none, so a reachable bound call is
        // refused for the missing signature (the guard the engine replaced).
        use crate::formats::object::BoundCall;
        let syntax = fake_table_syntax();
        let mut obj = assemble(&syntax, 0x7E, TWO_FUNCS, false).unwrap();
        obj.bound_calls.push(BoundCall {
            blob: 0,
            offset: 2,
            symbol: 1, // `go`
            binding: vec![],
        });
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(e, crate::linker::LinkError::MissingSignature("main".into()));
    }

    #[test]
    fn assembled_binding_call_assembles_and_needs_a_signed_entry() {
        // A declarative binding call assembles (producing the MO record, no
        // relocation); the linker then routes it through the composition
        // engine, which refuses an unsigned entry for the missing signature.
        let syntax = fake_table_syntax();
        let src = "\
.func main
        call    go [2{1->3,2=>0}, 0]
        stop
.func go
        nop
        ret
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        // Assembly succeeded and recorded the binding (no relocation).
        assert_eq!(obj.bound_calls.len(), 1);
        assert_eq!(obj.symbols[obj.bound_calls[0].symbol as usize].name, "go");
        assert!(obj.relocations.is_empty());
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(e, crate::linker::LinkError::MissingSignature("main".into()));
    }

    #[test]
    fn tables_without_entry_signature_are_missing_signature() {
        // TABLED_MAIN minus its `.routine` line: table content present,
        // entry unsigned.
        let src = TABLED_MAIN
            .lines()
            .filter(|l| !l.starts_with(".routine"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let syntax = fake_table_syntax();
        let obj = assemble(&syntax, 0x7E, &src, false).unwrap();
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(e, crate::linker::LinkError::MissingSignature("main".into()));
    }

    #[test]
    fn stray_table_bytes_are_malformed_table() {
        // A trailing byte no fixup-attributed table covers.
        let syntax = fake_table_syntax();
        let mut obj = assemble(&syntax, 0x7E, TABLED_MAIN, false).unwrap();
        let table = &mut obj.table_blobs.as_mut().unwrap()[0];
        let at = table.len() as u32;
        table.push(0xEE);
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedTable {
                symbol: "main".into(),
                at
            }
        );
    }

    #[test]
    fn dispatch_entry_off_instruction_boundary_is_malformed_table() {
        // Tamper the first dispatch entry to point mid-instruction.
        // Table layout: match (3 + 2*2 = 7 bytes), dispatch header at 7,
        // first entry at 9 — the tampered value 2 is inside tmatch's
        // operand, not a boundary.
        let syntax = fake_table_syntax();
        let mut obj = assemble(&syntax, 0x7E, TABLED_MAIN, false).unwrap();
        let table = &mut obj.table_blobs.as_mut().unwrap()[0];
        table[9..13].copy_from_slice(&2u32.to_le_bytes());
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedTable {
                symbol: "main".into(),
                at: 9
            }
        );
    }

    #[test]
    fn tableless_link_is_byte_identical_to_the_code_only_shape() {
        // The PM-1 lock: a tableless, unsigned link must produce the
        // version-1 code-only image byte-for-byte — table support must
        // not leak into the classic path.
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, TWO_FUNCS, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        let bytes = out.executable.to_bytes();
        assert_eq!(
            u16::from_le_bytes(bytes[3..5].try_into().unwrap()),
            1,
            "code-only image stays format version 1"
        );
        let expected = crate::formats::executable::Executable::code_only(
            0x7E,
            0,
            vec![0x0E, 0x31, 0x01, 0x02, 0x0E, 0x01, 0x0B],
        );
        assert_eq!(bytes, expected.to_bytes());
    }

    #[test]
    fn signatures_without_tables_still_emit_a_sectioned_image() {
        let src = "\
.routine main, tapes=2, alpha=(3, 5)
.func main
        stop
";
        let syntax = fake_table_syntax();
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        let exe = &out.executable;
        assert_eq!(exe.tape_count, 2);
        assert_eq!(exe.profile, 0);
        assert_eq!(exe.alphabet_cardinalities, vec![3, 5]);
        assert!(exe.tables.is_empty());
        let bytes = exe.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2);
    }

    #[test]
    fn holed_branch_is_malformed() {
        use crate::formats::object::{ObjectFile, Relocation, Symbol, SymbolDef};
        let syntax = syntax_with_short_call();
        // [0E][22 xx][02]: br (Flow::Branch, RelI8) with a reloc hole at 2.
        let obj = ObjectFile::v2(
            0x7E,
            vec![
                Symbol {
                    name: "main".into(),
                    def: SymbolDef::Defined { blob: 0 },
                },
                Symbol {
                    name: "g".into(),
                    def: SymbolDef::External,
                },
            ],
            vec![vec![0x0E, 0x22, 0x00, 0x02]],
            vec![Relocation {
                blob: 0,
                offset: 2,
                symbol: 1,
            }],
            None,
        );
        let lib = assemble(&syntax, 0x7E, ".func g\n        ret\n", false).unwrap();
        let e = link(&syntax, &[obj], &[lib], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedBlob {
                symbol: "main".into(),
                at: 2
            }
        );
    }
}
