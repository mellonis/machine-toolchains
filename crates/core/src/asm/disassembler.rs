//! Binary → canonical `.pma` text (docs/formats.md (assembly text)).
//! Output is valid assembler input; object round-trips are exact.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::decode::{Body, Decoded, DecodedOperand, decode_at, decode_stream};
use super::syntax::{ArchSyntax, Flow};
use crate::formats::executable::Executable;
use crate::formats::object::{BoundCall, ObjectFile, SymbolDef, TapeBinding};
use crate::linker::MapFile;
use crate::vm::OperandKind;

/// Canonical `.pma` trailing-comment column (docs/formats.md (assembly
/// text)), the same stop `fmt.rs` uses — needed only to place the
/// defensive comment on a dispatch table with unresolved offsets so the
/// rendered line still lands on the grid.
const COMMENT_COL: usize = 32;

/// Canonical .pma grid (docs/formats.md (assembly text)): label col 0,
/// mnemonic col 8, operand col 16; trailing spaces trimmed. A label
/// field (name + `:`) of 8+ chars would touch the mnemonic column with
/// no separating space, so it moves to its own line instead — the
/// return value has no trailing newline (callers already append one)
/// but may contain an interior one.
pub fn grid_line(label: Option<&str>, mnemonic: &str, operand: &str) -> String {
    const MNEMONIC_COL: usize = 8;
    const OPERAND_COL: usize = 16;

    // The mnemonic + operand portion, laid out from the mnemonic column
    // (8 leading spaces). The operand lands at [`OPERAND_COL`], or one
    // space past a mnemonic that reaches or overflows that stop — the
    // same overflow rule as `fmt.rs`'s `pad_to`, so a directive whose
    // keyword is 8+ chars (`.targets`, an arch's `tdispatch`) still keeps
    // one separating space instead of butting against the operand.
    let mut body = " ".repeat(MNEMONIC_COL);
    body.push_str(mnemonic);
    if !operand.is_empty() {
        let col = body.chars().count();
        if col < OPERAND_COL {
            body.push_str(&" ".repeat(OPERAND_COL - col));
        } else {
            body.push(' ');
        }
        body.push_str(operand);
    }
    while body.ends_with(' ') {
        body.pop();
    }

    match label {
        // A label field (name + `:`) reaching the mnemonic column moves to
        // its own line so it never pushes the mnemonic out of alignment.
        Some(l) if l.chars().count() + 1 >= MNEMONIC_COL => format!("{l}:\n{body}"),
        // Otherwise the label overwrites the leading mnemonic-column
        // padding (the first `MNEMONIC_COL` ASCII spaces of `body`).
        Some(l) => {
            let field = format!("{l}:");
            let mut line = field.clone();
            line.push_str(&" ".repeat(MNEMONIC_COL - field.chars().count()));
            line.push_str(body.get(MNEMONIC_COL..).unwrap_or(""));
            line
        }
        None => body,
    }
}

/// Renders a decoded int-vector operand under the dialect's caps
/// (docs/formats.md (assembly text)). With `caps.vectors`, a
/// `SymbolVec` renders in bracket form with the keep marker (`0x7F` →
/// `-`) and a `MoveVec` with the move glyphs (0 → `.`, 1 → `<`, 2 →
/// `>`; an out-of-vocabulary move code renders as its raw number —
/// defensive, the assembler never emits one). With the cap off — every
/// pre-vectors dialect — the classic comma-joined ints text is
/// byte-identical to before the vector kinds existed.
fn ints_operand_text(syntax: &ArchSyntax, kind: OperandKind, v: &[u32]) -> String {
    let plain = |v: &[u32]| v.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
    if !syntax.caps.vectors {
        return plain(v);
    }
    let elems: Vec<String> = match kind {
        OperandKind::SymbolVec => v
            .iter()
            .map(|&e| {
                if e == 0x7F {
                    "-".to_string()
                } else {
                    e.to_string()
                }
            })
            .collect(),
        OperandKind::MoveVec => v
            .iter()
            .map(|&e| match e {
                0 => ".".to_string(),
                1 => "<".to_string(),
                2 => ">".to_string(),
                other => other.to_string(),
            })
            .collect(),
        _ => return plain(v),
    };
    format!("[{}]", elems.join(", "))
}

/// `.byte` fallback: one directive per byte, the label (if any) attached
/// to the first line.
fn push_byte_lines(out: &mut String, label: Option<&str>, bytes: &[u8]) {
    for (k, b) in bytes.iter().enumerate() {
        out.push_str(&grid_line(
            if k == 0 { label } else { None },
            ".byte",
            &b.to_string(),
        ));
        out.push('\n');
    }
}

/// Which table a blob-local table offset holds, inferred from the
/// instruction that references it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TableKind {
    Match,
    Dispatch,
    Frame,
}

/// A decoded frame descriptor (docs/formats.md (frame descriptors)):
/// per-tape physical projection + dense symbol maps, plus the exit vector
/// (blob-relative code offsets in an object, absolute in a linked image).
/// Shared by object- and executable-level rendering.
struct ParsedFrame {
    tapes: Vec<u8>,
    rmaps: Vec<Vec<u16>>,
    wmaps: Vec<Vec<u16>>,
    exits: Vec<u32>,
}

/// Walks a frame descriptor at `start`; `None` on truncation (defensive —
/// the assembler/linker never emit one).
fn parse_frame_descriptor(tb: &[u8], start: u32) -> Option<ParsedFrame> {
    let mut pos = start as usize;
    let arity = *tb.get(pos)?;
    pos += 1;
    let exit_count = u16::from_le_bytes([*tb.get(pos)?, *tb.get(pos + 1)?]) as usize;
    pos += 2;
    let read_map = |pos: &mut usize| -> Option<Vec<u16>> {
        let len = u16::from_le_bytes([*tb.get(*pos)?, *tb.get(*pos + 1)?]) as usize;
        *pos += 2;
        let mut m = Vec::with_capacity(len);
        for _ in 0..len {
            m.push(u16::from_le_bytes([*tb.get(*pos)?, *tb.get(*pos + 1)?]));
            *pos += 2;
        }
        Some(m)
    };
    let mut tapes = Vec::with_capacity(arity as usize);
    let mut rmaps = Vec::with_capacity(arity as usize);
    let mut wmaps = Vec::with_capacity(arity as usize);
    for _ in 0..arity {
        tapes.push(*tb.get(pos)?);
        pos += 1;
        rmaps.push(read_map(&mut pos)?);
        wmaps.push(read_map(&mut pos)?);
    }
    let mut exits = Vec::with_capacity(exit_count);
    for _ in 0..exit_count {
        let bytes = tb.get(pos..pos + 4)?;
        exits.push(u32::from_le_bytes(bytes.try_into().unwrap()));
        pos += 4;
    }
    Some(ParsedFrame {
        tapes,
        rmaps,
        wmaps,
        exits,
    })
}

/// A dense symbol map (`0xFFFF` = hole) as `<idx>-><val>` pairs: index 0 is
/// the forced identity and holes are implicit, so both are dropped — the
/// canonical form the assembler re-materializes byte-for-byte.
fn dense_map_pairs(dense: &[u16]) -> String {
    dense
        .iter()
        .enumerate()
        .skip(1)
        .filter(|&(_, &v)| v != 0xFFFF)
        .map(|(i, &v)| format!("{i}->{v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders a declarative call's tape binding as `[<entry>, …]`
/// (docs/formats.md (bound calls)). Each entry is the caller physical
/// tape, optionally followed by a `{ <pairs> }` symbol map; `->` spells a
/// bidirectional pair and `=>` a one-way one — the one-way bit IS wire
/// data here, so it re-emits exactly. Passthrough entries (no pairs) drop
/// the braces, which the assembler re-parses to the same empty pair list.
fn render_binding(binding: &[TapeBinding]) -> String {
    let entries = binding
        .iter()
        .map(|tb| {
            if tb.pairs.is_empty() {
                return tb.caller_tape.to_string();
            }
            let pairs = tb
                .pairs
                .iter()
                .map(|p| {
                    let arrow = if p.one_way { "=>" } else { "->" };
                    format!("{}{arrow}{}", p.src, p.dst)
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{}{{{pairs}}}", tb.caller_tape)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{entries}]")
}

/// Renders one frame descriptor as `.frame`/`.map`/`.exits` lines. The
/// descriptor label sits on the `.frame` line; a `.map` line prints per
/// tape with a non-identity map; `.exits` resolves each code offset
/// through `exit_name`.
fn render_frame_table(
    out: &mut String,
    name: &str,
    frame: &ParsedFrame,
    exit_name: impl Fn(u32) -> String,
) {
    let tapes = frame
        .tapes
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&grid_line(
        Some(name),
        ".frame",
        &format!("tapes=({tapes})"),
    ));
    out.push('\n');
    for (k, (rmap, wmap)) in frame.rmaps.iter().zip(&frame.wmaps).enumerate() {
        if rmap.is_empty() && wmap.is_empty() {
            continue;
        }
        let mut operand = k.to_string();
        if !rmap.is_empty() {
            operand.push_str(&format!(", rmap=({})", dense_map_pairs(rmap)));
        }
        if !wmap.is_empty() {
            operand.push_str(&format!(", wmap=({})", dense_map_pairs(wmap)));
        }
        out.push_str(&grid_line(None, ".map", &operand));
        out.push('\n');
    }
    if !frame.exits.is_empty() {
        let names = frame
            .exits
            .iter()
            .map(|&o| exit_name(o))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&grid_line(None, ".exits", &names));
        out.push('\n');
    }
}

/// `(blob index, blob-local table offset) -> synthesized `Tn` label`: the
/// code section looks up a table reference's display name here.
type TableLabels = HashMap<(u32, u32), String>;

/// Renders the `.section tables` body for `obj`'s table blobs and returns
/// it with a `(blob, table-offset) -> synthesized label` map for the code
/// section to reference an operand by name.
///
/// Table labels are synthesized `T0`, `T1`, … scanning blobs in index
/// order and, within each blob, tables in ascending table-offset order
/// (one global counter, so names are unique across the single tables
/// section). Returns `None` when the object carries no discoverable
/// tables — its disassembly is then byte-identical to a pre-tables
/// object (no `.section` lines at all).
///
/// A table's kind is read from the instruction that references it, not
/// from the bytes (a concatenated table blob is not self-describing): the
/// opcode one byte before each fixup hole. A `FallThrough` op is a pure
/// lookup (a match table); any control-transfer flow means the op
/// dispatches THROUGH its table (a dispatch table). This keeps core
/// arch-agnostic — `Flow` is arch-supplied per-opcode data the
/// disassembler already consumes, never a mnemonic-string match.
fn render_tables_section(syntax: &ArchSyntax, obj: &ObjectFile) -> Option<(String, TableLabels)> {
    let table_blobs = obj.table_blobs.as_ref()?;

    // Kind inference keys on the REFERENCING operand kind, not the table
    // bytes: a plain `TableRef` sits one byte after its opcode (Match if it
    // falls through, else Dispatch); a `FramedCall`'s frame half sits five
    // bytes after its opcode (Frame). The object blob zeroes the framed
    // call's displacement half, so a frame hole's `hole - 1` byte is never
    // a TableRef opcode — the TableRef test is checked first without
    // aliasing a frame reference.
    let kind_of = |blob: u32, hole: u32| -> TableKind {
        let Some(code) = obj.blobs.get(blob as usize) else {
            return TableKind::Match;
        };
        let opcode_at = |off: Option<u32>| {
            off.and_then(|p| code.get(p as usize))
                .copied()
                .and_then(|op| syntax.by_opcode(op))
        };
        if let Some(entry) = opcode_at(hole.checked_sub(1))
            && entry.operand == OperandKind::TableRef
        {
            return if entry.flow == Flow::FallThrough {
                TableKind::Match
            } else {
                TableKind::Dispatch
            };
        }
        if let Some(entry) = opcode_at(hole.checked_sub(5))
            && entry.operand == OperandKind::FramedCall
        {
            return TableKind::Frame;
        }
        TableKind::Match
    };

    // Distinct table start offsets (and their kinds) per blob. Every
    // assembler-emitted table is referenced, so every start appears here;
    // duplicate references to one table collapse to its first classifier.
    let mut per_blob: Vec<BTreeMap<u32, TableKind>> = vec![BTreeMap::new(); table_blobs.len()];
    for fixup in &obj.table_fixups {
        if let Some(starts) = per_blob.get_mut(fixup.blob as usize) {
            starts
                .entry(fixup.table_offset)
                .or_insert_with(|| kind_of(fixup.blob, fixup.offset));
        }
    }
    if per_blob.iter().all(BTreeMap::is_empty) {
        return None;
    }

    // Frame descriptors get synthesized `F<n>` labels, match/dispatch
    // tables `T<n>` — the code section references each by its kind's
    // operand (a `call.m` names an `F`, an `mtc`/`djmp` a `T`).
    let mut labels: TableLabels = HashMap::new();
    let mut body = String::new();
    let mut next_t = 0u32;
    let mut next_f = 0u32;
    for (blob, starts) in per_blob.iter().enumerate() {
        let tb = &table_blobs[blob];
        let bounds: Vec<u32> = starts.keys().copied().collect();
        for (idx, (&start, &kind)) in starts.iter().enumerate() {
            let name = if kind == TableKind::Frame {
                let n = format!("F{next_f}");
                next_f += 1;
                n
            } else {
                let n = format!("T{next_t}");
                next_t += 1;
                n
            };
            labels.insert((blob as u32, start), name.clone());
            let end = bounds.get(idx + 1).copied().unwrap_or(tb.len() as u32);
            match kind {
                TableKind::Match => render_match_table(&mut body, &name, tb, start, end),
                TableKind::Dispatch => {
                    render_dispatch_table(&mut body, &name, tb, start, end, blob as u32, obj)
                }
                TableKind::Frame => {
                    if let Some(frame) = parse_frame_descriptor(tb, start) {
                        render_frame_table(&mut body, &name, &frame, |offset| {
                            frame_exit_debug_name(obj, blob as u32, offset)
                        });
                    }
                }
            }
        }
    }
    Some((body, labels))
}

/// One match table (vm/table.rs layout: `width u8`, `row_count u16 LE`,
/// then `row_count × width` bytes, `0x7F` = wildcard) as `.row [..]`
/// lines — the label on the first row, the rest continuing the run. A
/// truncated table stops cleanly rather than panicking (defensive; the
/// assembler never emits one).
fn render_match_table(out: &mut String, name: &str, tb: &[u8], start: u32, end: u32) {
    let base = start as usize;
    let (Some(&width_b), Some(&lo), Some(&hi)) = (tb.get(base), tb.get(base + 1), tb.get(base + 2))
    else {
        return;
    };
    let width = width_b as usize;
    let row_count = u16::from_le_bytes([lo, hi]) as usize;
    let limit = (end as usize).min(tb.len());
    let mut pos = base + 3;
    for row in 0..row_count {
        if width == 0 || pos + width > limit {
            break;
        }
        let elems: Vec<String> = tb[pos..pos + width]
            .iter()
            .map(|&b| {
                if b == 0x7F {
                    "*".to_string()
                } else {
                    b.to_string()
                }
            })
            .collect();
        let operand = format!("[{}]", elems.join(", "));
        out.push_str(&grid_line((row == 0).then_some(name), ".row", &operand));
        out.push('\n');
        pos += width;
    }
}

/// One dispatch table (vm/table.rs layout: `entry_count u16 LE`, then
/// `entry_count × u32 LE` blob-relative code offsets) as a `.targets`
/// line. Each entry offset resolves to a code label from the owning
/// blob's debug info when present; an unresolved offset renders as its
/// raw number, flagged by a comment at the grid comment column (defensive
/// — `-g` assembler output always resolves). Reassembly of the resolved
/// form still needs the code section to define those labels, which
/// object disassembly does not yet emit — dispatch rendering is read-only
/// for now.
fn render_dispatch_table(
    out: &mut String,
    name: &str,
    tb: &[u8],
    start: u32,
    end: u32,
    blob: u32,
    obj: &ObjectFile,
) {
    let base = start as usize;
    let (Some(&lo), Some(&hi)) = (tb.get(base), tb.get(base + 1)) else {
        return;
    };
    let count = u16::from_le_bytes([lo, hi]) as usize;
    let name_at = |offset: u32| -> Option<&str> {
        obj.debug
            .as_ref()?
            .get(blob as usize)?
            .labels
            .iter()
            .find(|(_, o)| *o == offset)
            .map(|(n, _)| n.as_str())
    };
    let limit = (end as usize).min(tb.len());
    let mut pos = base + 2;
    let mut names = Vec::with_capacity(count);
    let mut any_raw = false;
    for _ in 0..count {
        if pos + 4 > limit {
            break;
        }
        let target = u32::from_le_bytes(tb[pos..pos + 4].try_into().unwrap());
        match name_at(target) {
            Some(n) => names.push(n.to_string()),
            None => {
                any_raw = true;
                names.push(target.to_string());
            }
        }
        pos += 4;
    }
    if names.is_empty() {
        return;
    }
    let line = grid_line(Some(name), ".targets", &names.join(", "));
    out.push_str(&line);
    if any_raw {
        // grid_line emits no comment: pad the last physical line to the
        // comment column by hand so the flag still lands on the grid.
        let last_len = line.rsplit('\n').next().unwrap_or(&line).chars().count();
        if last_len < COMMENT_COL {
            out.push_str(&" ".repeat(COMMENT_COL - last_len));
        } else {
            out.push(' ');
        }
        out.push_str("; unresolved dispatch offsets (no debug labels)");
    }
    out.push('\n');
}

/// An object frame exit's code label: the owning blob's debug label at
/// that blob-relative offset, or the raw offset when no `-g` label names
/// it (a read-only rendering, like an unresolved dispatch entry).
fn frame_exit_debug_name(obj: &ObjectFile, blob: u32, offset: u32) -> String {
    obj.debug
        .as_ref()
        .and_then(|d| d.get(blob as usize))
        .and_then(|bd| bd.labels.iter().find(|(_, o)| *o == offset))
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| offset.to_string())
}

/// One canonical `.routine` line (newline included), the exact grid
/// `fmt.rs`'s printer normalizes to.
fn routine_line(name: &str, tapes: u8, cardinalities: &[u32]) -> String {
    let alpha = cardinalities
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(".routine {name}, tapes={tapes}, alpha=({alpha})\n")
}

/// The entries of the dispatch table at `start` in a LINKED table
/// section (`entry_count u16 LE`, then `entry_count × u32 LE` absolute
/// code addresses). Defensive: a truncated table yields the entries
/// that fit rather than panicking (the linker never emits one).
fn dispatch_entries(tables: &[u8], start: u32) -> Vec<u32> {
    let base = start as usize;
    let (Some(&lo), Some(&hi)) = (tables.get(base), tables.get(base + 1)) else {
        return Vec::new();
    };
    let count = u16::from_le_bytes([lo, hi]) as usize;
    let mut entries = Vec::with_capacity(count);
    let mut pos = base + 2;
    for _ in 0..count {
        let Some(bytes) = tables.get(pos..pos + 4) else {
            break;
        };
        entries.push(u32::from_le_bytes(bytes.try_into().unwrap()));
        pos += 4;
    }
    entries
}

/// One dispatch table of a LINKED image as a `.targets` line: entries
/// are absolute code addresses, resolved through the map-derived label
/// names in `labels`. An unresolved entry renders as raw hex, flagged
/// by a comment at the grid comment column — a mapless rendering is
/// read-only, since only label names make the text reassembleable.
fn render_linked_dispatch_table(
    out: &mut String,
    name: &str,
    tables: &[u8],
    start: u32,
    labels: &BTreeMap<u32, String>,
) {
    let entries = dispatch_entries(tables, start);
    if entries.is_empty() {
        return;
    }
    let mut names = Vec::with_capacity(entries.len());
    let mut any_raw = false;
    for target in entries {
        match labels.get(&target) {
            Some(n) => names.push(n.clone()),
            None => {
                any_raw = true;
                names.push(format!("{target:#06x}"));
            }
        }
    }
    let line = grid_line(Some(name), ".targets", &names.join(", "));
    out.push_str(&line);
    if any_raw {
        // grid_line emits no comment: pad the last physical line to the
        // comment column by hand so the flag still lands on the grid.
        let last_len = line.rsplit('\n').next().unwrap_or(&line).chars().count();
        if last_len < COMMENT_COL {
            out.push_str(&" ".repeat(COMMENT_COL - last_len));
        } else {
            out.push(' ');
        }
        out.push_str("; unresolved dispatch targets (no map labels)");
    }
    out.push('\n');
}

pub fn disassemble_object(syntax: &ArchSyntax, obj: &ObjectFile) -> String {
    let mut out = String::new();
    // Tables render first, in their own section, with `.section code`
    // before the function bodies. A no-tables object emits neither line,
    // so its output stays byte-identical to a pre-tables object.
    let table_labels = match render_tables_section(syntax, obj) {
        Some((section, labels)) => {
            out.push_str(".section tables\n");
            out.push_str(&section);
            out.push_str(".section code\n");
            labels
        }
        None => HashMap::new(),
    };
    // reloc lookup: (blob, hole offset) -> symbol name
    let reloc_at: BTreeMap<(u32, u32), &str> = obj
        .relocations
        .iter()
        .map(|r| {
            (
                (r.blob, r.offset),
                obj.symbols[r.symbol as usize].name.as_str(),
            )
        })
        .collect();
    // bound-call lookup: (blob, hole offset) -> the record. A binding
    // call's hole carries no relocation — the target and tape binding
    // ride this record instead (docs/formats.md (bound calls)).
    let bound_at: BTreeMap<(u32, u32), &BoundCall> = obj
        .bound_calls
        .iter()
        .map(|bc| ((bc.blob, bc.offset), bc))
        .collect();

    for symbol in &obj.symbols {
        let (blob, local) = match symbol.def {
            SymbolDef::Defined { blob } => (blob, false),
            SymbolDef::Local { blob } => (blob, true),
            SymbolDef::External => continue,
        };
        let code = &obj.blobs[blob as usize];
        // A signed object re-emits each function's `.routine` line ahead
        // of its `.func`, so dis ∘ asm preserves signatures (they are
        // all-or-none per object, parallel to blobs — docs/formats.md
        // (.pmo)).
        if let Some(sig) = obj.signatures.as_ref().and_then(|s| s.get(blob as usize)) {
            out.push_str(&routine_line(&symbol.name, sig.arity, &sig.cardinalities));
        }
        out.push_str(&format!(
            ".func {}{}\n",
            symbol.name,
            if local { " local" } else { "" }
        ));
        // Skip the leading entry byte if present (implied by .func).
        let start = if code.first() == Some(&syntax.entry_opcode) {
            1
        } else {
            0
        };
        let decoded = decode_stream(syntax, code, start, code.len() as u32);

        let mut targets = BTreeSet::new();
        for d in &decoded {
            if let Body::Instr {
                operand: DecodedOperand::RelTarget(t),
                mnemonic,
            } = &d.body
            {
                let is_call = syntax
                    .by_mnemonic(mnemonic)
                    .is_some_and(|e| syntax.is_call(e.opcode));
                if !is_call && !reloc_at.contains_key(&(blob, d.addr + 1)) {
                    targets.insert(*t);
                }
            }
        }

        for d in &decoded {
            let label_name = targets
                .contains(&d.addr)
                .then(|| format!("L{:04X}", d.addr));
            match &d.body {
                Body::Raw(b) => {
                    push_byte_lines(&mut out, label_name.as_deref(), &[*b]);
                }
                Body::Instr { mnemonic, operand } => {
                    let entry = syntax.by_mnemonic(mnemonic).unwrap();
                    let text: Option<String> = match operand {
                        DecodedOperand::None => Some(String::new()),
                        DecodedOperand::Ints(v) => {
                            Some(ints_operand_text(syntax, entry.operand, v))
                        }
                        // Table reference: the synthesized `Tn` label of
                        // the table at this blob-local offset (from
                        // `render_tables_section`), falling back to the
                        // raw offset if the object carried a reference to
                        // a table the section pass could not place.
                        DecodedOperand::TableAddr(t) => Some(
                            table_labels
                                .get(&(blob, *t))
                                .cloned()
                                .unwrap_or_else(|| t.to_string()),
                        ),
                        DecodedOperand::RelTarget(t) => {
                            if syntax.is_call(entry.opcode) {
                                // The hole starts one byte after the opcode.
                                reloc_at
                                    .get(&(blob, d.addr + 1))
                                    .map(|name| (*name).to_string())
                                    // A reloc-less call site is either a
                                    // declarative binding call (rendered
                                    // from its record) or a genuine gap
                                    // (.byte fallback below).
                                    .or_else(|| {
                                        bound_at.get(&(blob, d.addr + 1)).map(|bc| {
                                            format!(
                                                "{} {}",
                                                obj.symbols[bc.symbol as usize].name,
                                                render_binding(&bc.binding),
                                            )
                                        })
                                    })
                            } else if let Some(name) = reloc_at.get(&(blob, d.addr + 1)) {
                                // Relocated symbol jump — always far in objects.
                                Some(format!("@{name}"))
                            } else {
                                Some(format!("L{t:04X}"))
                            }
                        }
                        DecodedOperand::Imm(n) => Some(format!("#{n}")),
                        // A framed call: the displacement half relocates
                        // like a call (rendered from the reloc symbol), the
                        // frame half is a table-space label. A missing
                        // target reloc falls back to `.byte`, like a
                        // reloc-less call.
                        DecodedOperand::FramedCall { table, .. } => {
                            reloc_at.get(&(blob, d.addr + 1)).map(|name| {
                                let frame = table_labels
                                    .get(&(blob, *table))
                                    .cloned()
                                    .unwrap_or_else(|| table.to_string());
                                format!("{name}, {frame}")
                            })
                        }
                    };
                    match text {
                        Some(operand_text) => {
                            out.push_str(&grid_line(
                                label_name.as_deref(),
                                mnemonic,
                                &operand_text,
                            ));
                            out.push('\n');
                        }
                        None => {
                            push_byte_lines(
                                &mut out,
                                label_name.as_deref(),
                                &code[d.addr as usize..(d.addr + d.len) as usize],
                            );
                        }
                    }
                }
            }
        }
    }
    out
}

/// Decode ONE instruction at `addr` (None = unknown opcode / truncated).
fn decode_one(syntax: &ArchSyntax, code: &[u8], addr: u32) -> Option<Decoded> {
    decode_at(syntax, code, addr, code.len() as u32)
}

/// Resolve an executable `call.m` site index to its frame descriptor's
/// table offset through the frames region (docs/formats.md (frames
/// region)). A raw hand-authored site has a CONSTANT compose column, so
/// the identity row (FR=0) resolves it; the value is `directory[c-1]`
/// where `c = compose[0][site]`. `None` when the image carries no region,
/// the site is out of range, or the column is reserved-invalid (0).
fn resolve_site(exe: &Executable, site: u32) -> Option<u32> {
    let base = exe.frames_offset;
    if base == 0 {
        return None;
    }
    let tb = &exe.tables;
    let u16_at = |p: u32| -> Option<u16> {
        let p = p as usize;
        Some(u16::from_le_bytes([*tb.get(p)?, *tb.get(p + 1)?]))
    };
    let k = u32::from(u16_at(base)?);
    let s = u32::from(u16_at(base + 2)?);
    if site >= s {
        return None;
    }
    let composite = u16_at(base + 4 + k * 4 + site * 2)?;
    if composite == 0 {
        return None;
    }
    let dir_at = base + 4 + (u32::from(composite) - 1) * 4;
    let bytes = tb.get(dir_at as usize..dir_at as usize + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// The decoded frames region (docs/formats.md (frames region)): the K
/// directory descriptor offsets and the `(K+1) × S` compose matrix. `tmt
/// dis`'s legend and `call.m` rendering read composite columns from it — a
/// constant column names one descriptor, a context-dependent one lists the
/// composites it can select.
struct FramesRegion {
    /// Descriptor offset per composite index: `directory[c - 1]` for composite
    /// `c` (1..=K).
    directory: Vec<u32>,
    /// `(K + 1)` rows (active frame 0..=K) of `S` columns (call sites).
    compose: Vec<Vec<u16>>,
}

impl FramesRegion {
    fn site_count(&self) -> usize {
        self.compose.first().map_or(0, Vec::len)
    }

    /// The distinct non-zero composite indices a site's column can select,
    /// ascending (0 = unreachable pair, dropped).
    fn site_composites(&self, site: usize) -> Vec<u16> {
        let mut v: Vec<u16> = self
            .compose
            .iter()
            .filter_map(|row| row.get(site).copied())
            .filter(|&c| c != 0)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// `Some(c)` when the site's column resolves to exactly one composite (a
    /// constant site — hand-authored, or an engine site reached under one
    /// context); `None` when it is context-dependent (or unreachable).
    fn constant(&self, site: usize) -> Option<u16> {
        let v = self.site_composites(site);
        (v.len() == 1).then(|| v[0])
    }
}

/// Decode the frames region into a [`FramesRegion`] (docs/formats.md (frames
/// region)); `None` when the image carries no region.
fn parse_frames_region(exe: &Executable) -> Option<FramesRegion> {
    let base = exe.frames_offset;
    if base == 0 {
        return None;
    }
    let tb = &exe.tables;
    let u16_at = |p: u32| -> Option<u16> {
        let p = p as usize;
        Some(u16::from_le_bytes([*tb.get(p)?, *tb.get(p + 1)?]))
    };
    let k = usize::from(u16_at(base)?);
    let s = usize::from(u16_at(base + 2)?);
    let dir_base = base as usize + 4;
    let mut directory = Vec::with_capacity(k);
    for i in 0..k {
        let at = dir_base + i * 4;
        directory.push(u32::from_le_bytes(tb.get(at..at + 4)?.try_into().ok()?));
    }
    let comp_base = dir_base + k * 4;
    let mut compose = Vec::with_capacity(k + 1);
    for r in 0..=k {
        let mut row = Vec::with_capacity(s);
        for c in 0..s {
            let at = comp_base + (r * s + c) * 2;
            row.push(u16::from_le_bytes([*tb.get(at)?, *tb.get(at + 1)?]));
        }
        compose.push(row);
    }
    Some(FramesRegion { directory, compose })
}

/// The `tmt dis` frames legend (docs/formats.md (frames region)): a comment
/// block naming every directory composite `C<i>` (`i` = composite index /
/// frame-register value, 1-based) by its canonical binding label, then a
/// one-line summary of each context-dependent site's composites. The `C`
/// prefix is deliberately distinct from the code section's `F<n>` table
/// labels: `F…` names frame descriptors by tables-section order (0-based),
/// `C…` names composites by directory index, and with ≥2 composites the two
/// numberings diverge — sharing `F` would make `F1` ambiguous. Labels come
/// from the
/// map sidecar's `bindings` when present; without a map they are derived from
/// the descriptor bytes alone (image-inspectability), named by the site
/// callees. Every line is a `;` comment at column 0, so re-assembly ignores it
/// and the round trip is unaffected.
fn frames_legend(
    region: &FramesRegion,
    map: Option<&MapFile>,
    site_target: &HashMap<u32, u32>,
    func_name: &impl Fn(u32) -> String,
    exe: &Executable,
) -> String {
    let k = region.directory.len();
    let s = region.site_count();

    // (composite index, canonical label) in composite-index order.
    let labeled: Vec<(u16, String)> = match map {
        Some(m) if !m.bindings.is_empty() => m
            .bindings
            .iter()
            .map(|b| (b.index, b.label.clone()))
            .collect(),
        _ => {
            // No sidecar labels: derive each composite's routine name from a
            // site that reaches it (a `call.m` always calls the same routine),
            // then render labels from the descriptors themselves.
            let mut routines = vec![String::new(); k];
            for (&site, &target) in site_target {
                for c in region.site_composites(site as usize) {
                    let i = usize::from(c) - 1;
                    if i < routines.len() && routines[i].is_empty() {
                        routines[i] = func_name(target);
                    }
                }
            }
            crate::linker::binding_label::build_bindings(&exe.tables, exe.frames_offset, &routines)
                .into_iter()
                .map(|b| (b.index, b.label))
                .collect()
        }
    };

    let mut out = String::new();
    out.push_str(&format!("; frames: {k} composite(s), {s} site(s)\n"));
    for (index, label) in &labeled {
        out.push_str(&format!(";   C{index}: {label}\n"));
    }
    // Context-dependent sites (a row-varying compose column) get a summary of
    // the composites they can select, by composite index (`C<i>`); constant
    // sites already render their code-section `F`-label inline in the code.
    for site in 0..s {
        if region.constant(site).is_none() {
            let comps = region.site_composites(site);
            if !comps.is_empty() {
                let list = comps
                    .iter()
                    .map(|c| format!("C{c}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!(";   site{site}: [{list}]\n"));
            }
        }
    }
    out
}

pub fn disassemble_executable(
    syntax: &ArchSyntax,
    exe: &Executable,
    map: Option<&MapFile>,
) -> String {
    use crate::asm::syntax::SyntaxEntry;
    let code = &exe.code;
    let len = code.len() as u32;

    // Recursive-descent discovery (exact in v1: no indirect control flow).
    // instrs: every reachable instruction; roots: entry + all call targets.
    let mut instrs: BTreeMap<u32, Decoded> = BTreeMap::new();
    let mut roots: BTreeSet<u32> = BTreeSet::from([exe.entry]);
    let mut work: Vec<u32> = vec![exe.entry];
    // Table starts discovered from TableRef operands (each names one
    // table in `exe.tables`), and every dispatch table's entry addresses.
    let mut table_kinds: BTreeMap<u32, TableKind> = BTreeMap::new();
    let mut dispatch_targets: BTreeSet<u32> = BTreeSet::new();
    // Each discovered `call.m` site's callee address — a `call.m` always calls
    // the same callee, whatever composite its column selects, so this names the
    // routine of every composite reachable through the site (the map-less
    // legend's routine names, docs/formats.md (image-inspectability principle)).
    let mut site_target: HashMap<u32, u32> = HashMap::new();
    while let Some(addr) = work.pop() {
        if addr >= len || instrs.contains_key(&addr) {
            continue;
        }
        let Some(d) = decode_one(syntax, code, addr) else {
            continue; // unknown byte ends this path; gap pass will .byte it
        };
        let Body::Instr { mnemonic, operand } = &d.body else {
            unreachable!()
        };
        let entry = syntax.by_mnemonic(mnemonic).unwrap();
        let next = addr + d.len;
        // A TableRef operand names a table start; the kind comes from
        // THIS instruction's flow (the same inference as object-level
        // rendering — a FallThrough op is a pure lookup, any transfer
        // dispatches THROUGH its table). A dispatch table's entries are
        // code addresses the flow walk below cannot see, so they join
        // the work list — as label candidates, never roots.
        if let DecodedOperand::TableAddr(t) = operand
            && !table_kinds.contains_key(t)
        {
            let kind = if entry.flow == Flow::FallThrough {
                TableKind::Match
            } else {
                TableKind::Dispatch
            };
            table_kinds.insert(*t, kind);
            if matches!(kind, TableKind::Dispatch) {
                for target in dispatch_entries(&exe.tables, *t) {
                    dispatch_targets.insert(target);
                    work.push(target);
                }
            }
        }
        // A framed call names a call SITE (`table`) that resolves through
        // the frames region to a frame descriptor — a Frame table — and a
        // callee (`target`, a call root). The descriptor's exit vector
        // holds code addresses the flow walk cannot see, so they join the
        // work list as label candidates (like dispatch entries).
        if let DecodedOperand::FramedCall {
            table: site,
            target,
        } = operand
        {
            site_target.insert(*site, *target);
            if let Some(desc_off) = resolve_site(exe, *site)
                && let std::collections::btree_map::Entry::Vacant(slot) =
                    table_kinds.entry(desc_off)
            {
                slot.insert(TableKind::Frame);
                if let Some(frame) = parse_frame_descriptor(&exe.tables, desc_off) {
                    for &exit in &frame.exits {
                        dispatch_targets.insert(exit);
                        work.push(exit);
                    }
                }
            }
        }
        match (entry.flow, operand) {
            (Flow::FallThrough, _) => work.push(next),
            (Flow::Stop, _) => {}
            (Flow::Jump, DecodedOperand::RelTarget(t)) => work.push(*t),
            (Flow::Branch, DecodedOperand::RelTarget(t)) => {
                work.push(*t);
                work.push(next);
            }
            (Flow::Call, DecodedOperand::RelTarget(t)) => {
                roots.insert(*t);
                work.push(*t);
                work.push(next);
            }
            // A framed call is a call: the target becomes a root.
            (Flow::Call, DecodedOperand::FramedCall { target, .. }) => {
                roots.insert(*target);
                work.push(*target);
                work.push(next);
            }
            _ => work.push(next), // malformed flow/operand combo: keep walking
        }
        instrs.insert(addr, d);
    }

    // Every directory descriptor is inspectable, whether or not any constant
    // site named it during the walk (a context-dependent site resolves to no
    // single descriptor). Register the whole directory as frame tables so all
    // get an `F<n>` label + render, and add their exits as label candidates
    // (docs/formats.md (image-inspectability principle)).
    let region = parse_frames_region(exe);
    if let Some(region) = &region {
        for &desc_off in &region.directory {
            if let std::collections::btree_map::Entry::Vacant(slot) = table_kinds.entry(desc_off) {
                slot.insert(TableKind::Frame);
                if let Some(frame) = parse_frame_descriptor(&exe.tables, desc_off) {
                    for &exit in &frame.exits {
                        dispatch_targets.insert(exit);
                    }
                }
            }
        }
    }

    let roots: Vec<u32> = roots.into_iter().filter(|&r| r < len).collect();
    // The entry root is named `main`: the linker guarantees the entry
    // symbol is literally `main` (docs/formats.md (.pmx entry)),
    // so the synthesis is faithful and restores docs/formats.md (assembly
    // text)'s round-trip claim (dis → asm → link reproduces the
    // executable). All other roots keep the address-derived name. When a
    // map is supplied, its function names take priority (a debugger view
    // faithful to the linked source); `main`/`func_XXXX` synthesis is the
    // `None`-map fallback used by the round-trip law.
    let func_name = |addr: u32| {
        if let Some(m) = map
            && let Some(f) = m.functions.iter().find(|f| f.start == addr)
        {
            return f.name.clone();
        }
        if addr == exe.entry {
            "main".to_string()
        } else {
            format!("func_{addr:04X}")
        }
    };
    let region_end = |i: usize| roots.get(i + 1).copied().unwrap_or(len);
    // A short opcode displays as its far partner when the operand is
    // printed in symbol form (the two are interchangeable at source
    // level; only far is canonical for symbol sites).
    let far_mnemonic = |entry: &SyntaxEntry| -> &'static str {
        if let Some(pair) = syntax.relax_pairs.iter().find(|p| p.short == entry.opcode)
            && let Some(far) = syntax.by_opcode(pair.far)
        {
            return far.mnemonic;
        }
        entry.mnemonic
    };

    // Map-resolved names for dispatch targets: the label at that
    // absolute address when the map carries one. Unresolved targets
    // render as raw hex and get no code label — that rendering is
    // read-only, since only label names make the text reassembleable.
    let dispatch_label: BTreeMap<u32, String> = dispatch_targets
        .iter()
        .filter_map(|&addr| {
            map?.functions.iter().find_map(|f| {
                f.labels
                    .iter()
                    .find(|(_, a)| *a == addr)
                    .map(|(n, _)| (addr, n.clone()))
            })
        })
        .collect();

    let mut out = String::new();
    // Each `call.m` site's operand text: a constant column names its one
    // descriptor by `F<n>` label; a context-dependent column renders `@site<N>`
    // and the legend summarizes its composites. Filled once the frame labels
    // are known (below), read by the code section.
    let mut site_operand: HashMap<u32, String> = HashMap::new();
    // A sectioned (version-2) image opens with a synthesized `.routine`
    // for the entry function: the header's tape count and per-tape
    // alphabet cardinalities are exactly what the directive declares
    // (docs/formats.md (executable image)). A code-only image emits
    // nothing extra — byte-compatible with the pre-tables renderer.
    let sectioned = exe.tape_count != 1
        || exe.profile != 0
        || !exe.alphabet_cardinalities.is_empty()
        || !exe.tables.is_empty();
    if sectioned {
        out.push_str(&routine_line(
            &func_name(exe.entry),
            exe.tape_count,
            &exe.alphabet_cardinalities,
        ));
    }
    // Discovered tables render next in their own section, `T<n>`
    // (match/dispatch) and `F<n>` (frame) labels synthesized in ascending
    // section-offset order; the code section's operands reference them by
    // name below.
    let mut table_labels: HashMap<u32, String> = HashMap::new();
    if !table_kinds.is_empty() {
        out.push_str(".section tables\n");
        let bounds: Vec<u32> = table_kinds.keys().copied().collect();
        let mut next_t = 0u32;
        let mut next_f = 0u32;
        for (idx, (&start, &kind)) in table_kinds.iter().enumerate() {
            let name = if kind == TableKind::Frame {
                let n = format!("F{next_f}");
                next_f += 1;
                n
            } else {
                let n = format!("T{next_t}");
                next_t += 1;
                n
            };
            table_labels.insert(start, name.clone());
            let end = bounds
                .get(idx + 1)
                .copied()
                .unwrap_or(exe.tables.len() as u32);
            match kind {
                TableKind::Match => render_match_table(&mut out, &name, &exe.tables, start, end),
                TableKind::Dispatch => render_linked_dispatch_table(
                    &mut out,
                    &name,
                    &exe.tables,
                    start,
                    &dispatch_label,
                ),
                TableKind::Frame => {
                    if let Some(frame) = parse_frame_descriptor(&exe.tables, start) {
                        render_frame_table(&mut out, &name, &frame, |offset| {
                            dispatch_label
                                .get(&offset)
                                .cloned()
                                .unwrap_or_else(|| format!("{offset:#06x}"))
                        });
                    }
                }
            }
        }
        // The frames legend (docs/formats.md (frames region)): resolve each
        // `call.m` site's operand text, then a comment block naming every
        // directory composite and summarizing each context-dependent site.
        // Comments are trivia, so re-assembly ignores them and the round trip
        // is unaffected; emitted at column 0 (before the first `.func`), which
        // `fmt` leaves in place.
        if let Some(region) = &region {
            for site in 0..region.site_count() {
                let text = match region.constant(site) {
                    Some(c) => region
                        .directory
                        .get(usize::from(c) - 1)
                        .and_then(|off| table_labels.get(off))
                        .cloned()
                        .unwrap_or_else(|| format!("@site{site}")),
                    None => format!("@site{site}"),
                };
                site_operand.insert(site as u32, text);
            }
            out.push_str(&frames_legend(region, map, &site_target, &func_name, exe));
        }
        out.push_str(".section code\n");
    }
    for (i, &root) in roots.iter().enumerate() {
        let end = region_end(i);
        out.push_str(&format!(".func {}\n", func_name(root)));

        // Label names within this region: jump targets synthesize
        // `LXXXX`; a map-resolved dispatch target keeps its map name so
        // the `.targets` line above and the code line agree (a shared
        // address takes the dispatch name).
        let mut labels_at: BTreeMap<u32, String> = BTreeMap::new();
        for (_, d) in instrs.range(root..end) {
            if let Body::Instr {
                mnemonic,
                operand: DecodedOperand::RelTarget(t),
            } = &d.body
            {
                let e = syntax.by_mnemonic(mnemonic).unwrap();
                if e.flow != Flow::Call && *t > root && *t < end && roots.binary_search(t).is_err()
                {
                    labels_at.insert(*t, format!("L{t:04X}"));
                }
            }
        }
        for (&addr, name) in dispatch_label.range(root + 1..end) {
            labels_at.insert(addr, name.clone());
        }

        let mut addr = root;
        let mut first = true;
        while addr < end {
            let label_name = labels_at.get(&addr).cloned();
            match instrs.get(&addr) {
                None => {
                    push_byte_lines(
                        &mut out,
                        label_name.as_deref(),
                        &code[addr as usize..addr as usize + 1],
                    );
                    addr += 1;
                }
                Some(d) => {
                    let Body::Instr { mnemonic, operand } = &d.body else {
                        unreachable!()
                    };
                    let entry = syntax.by_mnemonic(mnemonic).unwrap();
                    // The root's leading entry instruction is implied by .func.
                    if first && entry.opcode == syntax.entry_opcode {
                        first = false;
                        addr += d.len;
                        continue;
                    }
                    first = false;
                    let text: Option<(&'static str, String)> = match operand {
                        DecodedOperand::None => Some((entry.mnemonic, String::new())),
                        DecodedOperand::Ints(v) => {
                            Some((entry.mnemonic, ints_operand_text(syntax, entry.operand, v)))
                        }
                        // Table reference: the synthesized `Tn` label of
                        // the table at this section offset, falling back
                        // to the raw offset if the section pass could not
                        // place it (defensive — the operand itself is
                        // what discovered every table).
                        DecodedOperand::TableAddr(t) => Some((
                            entry.mnemonic,
                            table_labels
                                .get(t)
                                .cloned()
                                .unwrap_or_else(|| t.to_string()),
                        )),
                        DecodedOperand::RelTarget(t) => {
                            if entry.flow == Flow::Call && roots.binary_search(t).is_ok() {
                                Some((far_mnemonic(entry), func_name(*t)))
                            } else if entry.flow == Flow::Jump && roots.binary_search(t).is_ok() {
                                // Tail jump to a function: symbol form.
                                Some((far_mnemonic(entry), format!("@{}", func_name(*t))))
                            } else if entry.flow != Flow::Call && *t > root && *t < end {
                                let label = labels_at
                                    .get(t)
                                    .cloned()
                                    .unwrap_or_else(|| format!("L{t:04X}"));
                                Some((entry.mnemonic, label))
                            } else {
                                None // cross-region non-root: .byte fallback
                            }
                        }
                        DecodedOperand::Imm(n) => Some((entry.mnemonic, format!("#{n}"))),
                        // A framed call: the callee is a call root (rendered
                        // by name); the frame half is the call SITE index. A
                        // constant compose column (a hand-authored site, or an
                        // engine site reached under one context) names its one
                        // descriptor by `F`-label; a context-dependent column
                        // renders `@site<N>` and the legend lists the composites
                        // it can select. A target that never became a root
                        // falls back to `.byte`.
                        DecodedOperand::FramedCall {
                            target,
                            table: site,
                        } => {
                            if roots.binary_search(target).is_ok() {
                                let frame = site_operand
                                    .get(site)
                                    .cloned()
                                    .unwrap_or_else(|| format!("@site{site}"));
                                Some((entry.mnemonic, format!("{}, {frame}", func_name(*target))))
                            } else {
                                None
                            }
                        }
                    };
                    match text {
                        Some((mnemonic, operand_text)) => {
                            out.push_str(&grid_line(
                                label_name.as_deref(),
                                mnemonic,
                                &operand_text,
                            ));
                            out.push('\n');
                        }
                        None => {
                            push_byte_lines(
                                &mut out,
                                label_name.as_deref(),
                                &code[addr as usize..(addr + d.len) as usize],
                            );
                        }
                    }
                    addr += d.len;
                }
            }
        }
    }
    out
}

/// One formatted debugger-listing line at `addr` (no trailing newline) +
/// the decoded instruction's byte length. Unknown opcode and truncated
/// operand both fall back to `.byte`, length 1 (mirrors [`decode_one`],
/// which returns `None` for exactly those cases). `resolve` maps a
/// branch/call/jump target address to an optional display name.
///
/// Precondition: `addr` must be strictly inside `code` (`addr <
/// code.len() as u32`) — this indexes `code[addr as usize]` directly.
/// Callers rendering a fault address (e.g. a fetch that ran off the end
/// of the code image) must guard the call themselves; see `pmt run
/// --trace`'s handling of traced runs in `crates/post-machine/src/cli/run.rs`.
pub fn listing_line(
    syntax: &ArchSyntax,
    code: &[u8],
    addr: u32,
    resolve: &dyn Fn(u32) -> Option<String>,
) -> (String, u32) {
    let (len, mnemonic, operand): (u32, &str, String) = match decode_one(syntax, code, addr) {
        None => (1, ".byte", code[addr as usize].to_string()),
        Some(Decoded {
            len,
            body: Body::Instr { mnemonic, operand },
            ..
        }) => {
            let operand_text = match operand {
                DecodedOperand::None => String::new(),
                DecodedOperand::Ints(v) => {
                    // The mnemonic came out of a successful decode, so
                    // the entry lookup cannot miss.
                    let entry = syntax
                        .by_mnemonic(mnemonic)
                        .expect("decoded mnemonic is in the table");
                    ints_operand_text(syntax, entry.operand, &v)
                }
                // Table-space offset — never resolved against code labels.
                DecodedOperand::TableAddr(t) => format!("{t:#06x}"),
                DecodedOperand::RelTarget(t) => match resolve(t) {
                    Some(name) => format!("{t:#06x} <{name}>"),
                    None => format!("{t:#06x}"),
                },
                DecodedOperand::Imm(n) => format!("#{n}"),
                DecodedOperand::FramedCall { target, table } => {
                    let tgt = match resolve(target) {
                        Some(name) => format!("{target:#06x} <{name}>"),
                        None => format!("{target:#06x}"),
                    };
                    format!("{tgt}, {table:#06x}")
                }
            };
            (len, mnemonic, operand_text)
        }
        Some(Decoded {
            body: Body::Raw(_), ..
        }) => unreachable!("decode_one/decode_at only ever produces Body::Instr"),
    };
    let bytes_hex = code[addr as usize..(addr + len) as usize]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    let line = format!("  {addr:04x}:  {bytes_hex:<15} {mnemonic:<8}{operand}");
    (line.trim_end().to_string(), len)
}

/// Debugger code view (addresses + raw bytes + mnemonics): every byte
/// accounted for, function headers from `map` when supplied, jump/call
/// targets resolved to `function`/`function.label` names. NOT
/// reassembleable — this is a read-only rendering, unlike
/// [`disassemble_executable`]'s canonical `.pma` text.
pub fn listing_executable(syntax: &ArchSyntax, exe: &Executable, map: Option<&MapFile>) -> String {
    let code = &exe.code;
    let len = code.len() as u32;

    let name_at = |addr: u32| -> Option<String> {
        map.and_then(|m| {
            m.functions.iter().find_map(|f| {
                if f.start == addr {
                    return Some(f.name.clone());
                }
                f.labels
                    .iter()
                    .find(|(_, a)| *a == addr)
                    .map(|(label, _)| format!("{}.{}", f.name, label))
            })
        })
    };

    let mut out = String::new();
    let mut addr = 0u32;
    while addr < len {
        if let Some(m) = map
            && let Some(f) = m.functions.iter().find(|f| f.start == addr)
        {
            out.push_str(&f.name);
            out.push_str(":\n");
        }
        let (line, ilen) = listing_line(syntax, code, addr, &name_at);
        out.push_str(&line);
        out.push('\n');
        addr += ilen;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::assembler::assemble;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::asm::syntax::{Flow, RelaxPair, SyntaxEntry};
    use crate::formats::executable::Executable;
    use crate::vm::OperandKind;

    /// Neutral fake dialect proving zero PM-1 knowledge in core (replica
    /// of `assembler.rs`'s test helper, per the repo's per-file-helper
    /// convention): `tmatch` references a match table (FallThrough → a
    /// lookup), `tdispatch` references a dispatch table (Stop → transfers
    /// through it), `vwrite` is the vector-capable write, `fimm` takes a
    /// plain immediate (Imm8), `fcall` is a framed call (FramedCall, Call
    /// flow), plus nop/stp/ent.
    fn fake_syntax() -> ArchSyntax {
        use crate::asm::AsmCaps;
        use crate::vm::OperandKind;
        use Flow::{Call, FallThrough as FT, Stop};
        ArchSyntax {
            entries: vec![
                SyntaxEntry {
                    opcode: 0x01,
                    mnemonic: "nop",
                    operand: OperandKind::None,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x02,
                    mnemonic: "stp",
                    operand: OperandKind::None,
                    flow: Stop,
                },
                SyntaxEntry {
                    opcode: 0x13,
                    mnemonic: "fimm",
                    operand: OperandKind::Imm8,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x14,
                    mnemonic: "fcall",
                    operand: OperandKind::FramedCall,
                    flow: Call,
                },
                SyntaxEntry {
                    opcode: 0x21,
                    mnemonic: "call",
                    operand: OperandKind::RelI32,
                    flow: Call,
                },
                SyntaxEntry {
                    opcode: 0x07,
                    mnemonic: "vwrite",
                    operand: OperandKind::SymbolVec,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x18,
                    mnemonic: "vmove",
                    operand: OperandKind::MoveVec,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x11,
                    mnemonic: "tmatch",
                    operand: OperandKind::TableRef,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x12,
                    mnemonic: "tdispatch",
                    operand: OperandKind::TableRef,
                    flow: Stop,
                },
                SyntaxEntry {
                    opcode: 0x0E,
                    mnemonic: "ent",
                    operand: OperandKind::None,
                    flow: FT,
                },
            ],
            relax_pairs: vec![],
            entry_opcode: 0x0E,
            break_opcode: None,
            trap_opcode: None,
            caps: AsmCaps {
                tables: true,
                rept: true,
                vectors: true,
            },
        }
    }

    #[test]
    fn object_dis_renders_match_table_and_references_it_by_label() {
        use crate::asm::{AsmCaps, format_asm_with};
        let syntax = fake_syntax();
        let src = "\
.section tables
T0: .row [1, 2]
    .row [1, *]
.section code
.func main
    tmatch T0
    stp
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        let expected = "\
.section tables
T0:     .row    [1, 2]
        .row    [1, *]
.section code
.func main
        tmatch  T0
        stp
";
        assert_eq!(dis, expected, "match-table disassembly:\n{dis}");
        // The pieces the brief calls out: a tables section, a `.row` line,
        // the wildcard as `*`, the reference by synthesized label.
        assert!(dis.contains(".section tables"));
        assert!(dis.contains("T0:     .row    [1, 2]"));
        assert!(dis.contains("[1, *]")); // wildcard byte rendered as `*`
        assert!(dis.contains("tmatch  T0"));
        // Already canonical: fmt over it (caps on) is the identity.
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        // And it reassembles to the identical object — a full round trip.
        assert_eq!(assemble(&syntax, 0x7E, &dis, false).unwrap(), obj);
    }

    #[test]
    fn object_dis_renders_dispatch_targets_by_debug_label() {
        use crate::asm::{AsmCaps, format_asm_with};
        let syntax = fake_syntax();
        // `-g` so the owning blob's debug labels resolve the entry offsets.
        let src = "\
.section tables
D0: .targets A, B
.section code
.func main
    tdispatch D0
A:  nop
B:  stp
";
        let obj = assemble(&syntax, 0x7E, src, true).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        // Dispatch entries resolve to their debug label names; the code
        // instruction references the table by synthesized label.
        assert!(dis.contains("T0:     .targets A, B"), "{dis}");
        assert!(dis.contains("tdispatch T0"), "{dis}");
        // Still canonical under fmt (the rendering lands on the grid).
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        // NOTE: not a full round trip — object disassembly does not emit
        // the dispatch targets' code labels (`A:`/`B:`) in the code
        // section, so reassembly would fault on unknown labels. Closing
        // that is the linked-image table path (phase 4b), out of scope
        // here; dispatch rendering is read-only for now.
    }

    #[test]
    fn object_dis_renders_routine_signatures_and_round_trips() {
        use crate::asm::{AsmCaps, format_asm_with};
        let syntax = fake_syntax();
        // Signatures are all-or-none per object: both functions signed.
        let src = "\
.routine main, tapes=2, alpha=(3, 5)
.routine helper, tapes=1, alpha=(2)
.func main
        stp
.func helper
        nop
        stp
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        // Each `.routine` re-emits immediately ahead of its `.func`
        // (the directive must precede its function).
        let expected = "\
.routine main, tapes=2, alpha=(3, 5)
.func main
        stp
.routine helper, tapes=1, alpha=(2)
.func helper
        nop
        stp
";
        assert_eq!(dis, expected, "signed-object disassembly:\n{dis}");
        // Already canonical under fmt, and dis ∘ asm preserves the
        // signatures — the round trip Task 2 left lossy.
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        assert_eq!(assemble(&syntax, 0x7E, &dis, false).unwrap(), obj);
    }

    #[test]
    fn vector_operands_render_bracket_form_and_round_trip() {
        use crate::asm::{AsmCaps, format_asm_with};
        // Under caps.vectors a SymbolVec renders `[..]` with the keep
        // marker (`0x7F` → `-`) and a MoveVec with the move glyphs
        // (0 → `.`, 1 → `<`, 2 → `>`); assemble ∘ dis is a fixpoint.
        let syntax = fake_syntax();
        let src = "\
.func main
        vwrite  [1, -, 2]
        vmove   [<, ., >]
        stp
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        assert_eq!(dis, src, "vector disassembly:\n{dis}");
        // Already canonical under fmt, and reassembly is exact.
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        assert_eq!(assemble(&syntax, 0x7E, &dis, false).unwrap(), obj);
    }

    #[test]
    fn caps_off_symbol_vec_rendering_is_unchanged() {
        // The byte-compat pin for the vector-rendering lever: a caps-off
        // dialect (PM-1's shape) keeps the classic comma-joined ints —
        // never the bracket form, and never `-` for a 0x7F payload.
        let syntax = test_syntax();
        let src = ".func f\n        wr      1, 127\n";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        assert_eq!(dis, src);
        assert!(!dis.contains('['), "caps-off must not render brackets");
    }

    #[test]
    fn no_tables_object_dis_is_byte_compatible() {
        // The byte-compat guard: an object without tables disassembles
        // with NO `.section` lines — byte-identical to a pre-tables build.
        let syntax = test_syntax();
        let src = "\
.func f
L0001:  nop
        jmp.s   L0001
        wr      1
        call    g
        stop
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        let expected = "\
.func f
L0001:  nop
        jmp.s   L0001
        wr      1
        call    g
        stop
";
        assert_eq!(dis, expected);
        assert!(!dis.contains(".section"), "no tables → no section markers");
    }

    #[test]
    fn fimm_operand_renders_hash_form_and_round_trips() {
        use crate::asm::{AsmCaps, format_asm_with};
        let syntax = fake_syntax();
        let src = "\
.func main
        fimm    #7
        stp
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        assert_eq!(dis, src, "fimm disassembly:\n{dis}");
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        assert_eq!(assemble(&syntax, 0x7E, &dis, false).unwrap(), obj);
    }

    #[test]
    fn fcall_operand_renders_target_and_frame_label_and_round_trips() {
        use crate::asm::{AsmCaps, format_asm_with};
        let syntax = fake_syntax();
        // A framed call to a defined function `target`, activating a real
        // `.frame` descriptor F0 (post-handoff-e a `call.m` must name a
        // `.frame`, never a match/dispatch table). The descriptor has a
        // non-identity rmap on tape 0 (a `->`, a one-way `=>`, and a hole);
        // its `=>` re-renders as `->` (the wire form has no one-way bit).
        let src = "\
.section tables
F0:     .frame  tapes=(2, 0)
        .map    0, rmap=(1->2, 3->1)
.section code
.func main
        fcall   target, F0
        stp
.func target
        stp
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        assert_eq!(dis, src, "fcall disassembly:\n{dis}");
        // The displacement half renders from the reloc symbol, the frame
        // half from the synthesized frame label.
        assert!(dis.contains("fcall   target, F0"), "{dis}");
        assert!(dis.contains("F0:     .frame  tapes=(2, 0)"), "{dis}");
        assert!(dis.contains(".map    0, rmap=(1->2, 3->1)"), "{dis}");
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        // Full object round trip (no exits here, so it round-trips at the
        // object level; exit labels round-trip through the linked image).
        assert_eq!(assemble(&syntax, 0x7E, &dis, false).unwrap(), obj);
    }

    #[test]
    fn binding_call_operand_renders_and_round_trips_with_one_way_bits() {
        use crate::asm::{AsmCaps, format_asm_with};
        let syntax = fake_syntax();
        // A declarative binding call: entry 0 projects physical tape 2 with
        // a `->` (bidirectional) and a `=>` (one-way) pair; entry 1 is a
        // bare passthrough of physical tape 0. The one-way bit is wire data
        // here, so `=>` re-emits verbatim (unlike a frame descriptor).
        let src = "\
.func main
        call    plusOne [2{1->3,2=>0}, 0]
        stp
.func plusOne
        stp
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        assert_eq!(dis, src, "binding-call disassembly:\n{dis}");
        assert!(dis.contains("call    plusOne [2{1->3,2=>0}, 0]"), "{dis}");
        // Canonical under fmt (the binding operand rides the grid intact).
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        // Full object round trip: the bound-call records — including every
        // one_way bit — survive asm ∘ dis ∘ asm exactly.
        let reasm = assemble(&syntax, 0x7E, &dis, false).unwrap();
        assert_eq!(reasm.bound_calls, obj.bound_calls);
        assert_eq!(reasm, obj);
    }

    #[test]
    fn linked_frame_descriptor_round_trips_with_exits() {
        // The strong round trip at the executable level, single-function
        // form (the executable disassembler synthesizes only the ENTRY
        // `.routine`, and the assembler's all-or-none signature rule then
        // demands the reached set be one function — so the frame's caller
        // is `main` itself). A `.frame` with two exits into `main`,
        // assembled with `-g`, linked, disassembled WITH the map,
        // re-assembled, and re-linked — the images must be byte-identical.
        // The exit vector's absolute code addresses resolve back to their
        // map label names.
        use crate::asm::{AsmCaps, format_asm_with};
        use crate::linker::{LinkOptions, link};
        let syntax = fake_syntax();
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
F0: .frame tapes=(1, 0)
    .map 0, rmap=(1->2, 3=>1)
    .exits done, other
.section code
.func main
    fcall main, F0
done:   stp
other:  stp
";
        let obj = assemble(&syntax, 0x7E, src, true).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // The frames profile is selected because a frame descriptor + a
        // framed call are present.
        assert_eq!(out.executable.profile, crate::formats::PROFILE_FRAMES);
        // The exit vector carries ABSOLUTE code addresses after the link:
        // `done`/`other` are the two `stp`s just past the 9-byte framed
        // call (ent@0, call.m@1..10, done@10, other@11).
        let done = out.map.functions[0]
            .labels
            .iter()
            .find(|(n, _)| n == "done")
            .unwrap()
            .1;
        let other = out.map.functions[0]
            .labels
            .iter()
            .find(|(n, _)| n == "other")
            .unwrap()
            .1;
        assert_eq!((done, other), (10, 11));
        let tables = &out.executable.tables;
        // Descriptor: arity 1, exit_count 2, tape0 phys 1 rmap_len 4 (0,2,
        // hole,1) wmap_len 0, then exits done, other as ABSOLUTE u32 LE. The
        // descriptor ends where the frames region begins, so the exit
        // vector's two u32s sit just before frames_offset.
        let exits_at = out.executable.frames_offset as usize - 8;
        assert_eq!(&tables[exits_at..exits_at + 4], &done.to_le_bytes());
        assert_eq!(&tables[exits_at + 4..exits_at + 8], &other.to_le_bytes());
        let text = disassemble_executable(&syntax, &out.executable, Some(&out.map));
        assert!(text.contains("F0:"), "no frame table:\n{text}");
        assert!(text.contains(".map    0, rmap="), "no map:\n{text}");
        assert!(
            text.contains(".exits  done, other"),
            "exits not resolved:\n{text}"
        );
        assert!(
            text.contains("fcall   main, F0"),
            "framed call not rendered:\n{text}"
        );
        // Canonical, and the round trip reproduces the image byte-for-byte.
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        assert_eq!(format_asm_with(&text, caps).unwrap(), text);
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        let out2 = link(&syntax, &[obj2], &[], LinkOptions::default()).unwrap();
        assert_eq!(
            out2.executable.to_bytes(),
            out.executable.to_bytes(),
            "dis ∘ link must reproduce the image byte-for-byte:\n{text}"
        );
    }

    #[test]
    fn frame_map_collapse_onto_blank_round_trips() {
        // A `Y->0` fold (a marker read as blank in rmap, an erase in wmap)
        // is a legal, non-hole dense entry: the disassembler re-emits it (0
        // is not the `0xFFFF` hole) and the image re-assembles byte-for-byte.
        use crate::asm::{AsmCaps, format_asm_with};
        use crate::linker::{LinkOptions, link};
        let syntax = fake_syntax();
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
F0: .frame tapes=(1, 0)
    .map 0, rmap=(1->2, 3->0), wmap=(2->0)
    .exits done
.section code
.func main
    fcall main, F0
done:   stp
";
        let obj = assemble(&syntax, 0x7E, src, true).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        assert_eq!(out.executable.profile, crate::formats::PROFILE_FRAMES);
        let text = disassemble_executable(&syntax, &out.executable, Some(&out.map));
        // The fold pairs survive disassembly verbatim (0-valued, not holes).
        assert!(
            text.contains("rmap=(1->2, 3->0)"),
            "rmap fold dropped:\n{text}"
        );
        assert!(text.contains("wmap=(2->0)"), "wmap fold dropped:\n{text}");
        // Canonical, and the round trip reproduces the image byte-for-byte.
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        };
        assert_eq!(format_asm_with(&text, caps).unwrap(), text);
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        let out2 = link(&syntax, &[obj2], &[], LinkOptions::default()).unwrap();
        assert_eq!(
            out2.executable.to_bytes(),
            out.executable.to_bytes(),
            "dis ∘ link must reproduce the image byte-for-byte:\n{text}"
        );
    }

    #[test]
    fn grid_line_long_label_own_line() {
        // Case 11: an 8+-char label field moves to its own line, the
        // instruction line follows with no label.
        assert_eq!(
            grid_line(Some("verylongname"), "nop", ""),
            "verylongname:\n        nop"
        );
        assert_eq!(
            grid_line(Some("verylongname"), "wr", "1, 2"),
            "verylongname:\n        wr      1, 2"
        );
    }

    #[test]
    fn grid_line_seven_char_field_stays_inline() {
        // "abcdef:" is exactly 7 chars — the largest field that still
        // fits before the mnemonic column.
        assert_eq!(grid_line(Some("abcdef"), "nop", ""), "abcdef: nop");
    }

    #[test]
    fn grid_line_short_labels_are_unchanged_vs_today() {
        assert_eq!(grid_line(Some("L1"), "rgt", ""), "L1:     rgt");
        assert_eq!(grid_line(Some("L0001"), "nop", ""), "L0001:  nop");
        assert_eq!(grid_line(None, "wr", "1"), "        wr      1");
        assert_eq!(grid_line(None, "stop", ""), "        stop");
    }

    #[test]
    fn grid_line_is_total_on_empty_mnemonic() {
        // Must not panic; renders the label alone.
        let line = grid_line(Some("L1"), "", "");
        assert!(line.starts_with("L1:"));
    }

    #[test]
    fn object_disassembly_uses_canonical_grid() {
        let syntax = test_syntax();
        let src = ".func f\nL0001:  nop\n        jmp.s   L0001\n        wr      1\n        call    g\n        stop\n";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let text = disassemble_object(&syntax, &obj);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], ".func f");
        assert_eq!(lines[1], "L0001:  nop");
        assert_eq!(lines[2], "        jmp.s   L0001");
        assert_eq!(lines[3], "        wr      1");
        assert_eq!(lines[4], "        call    g");
        assert_eq!(lines[5], "        stop");
    }

    #[test]
    fn round_trip_law() {
        let syntax = test_syntax();
        let src = "\
.func f
START:  nop
        jmp     START
        wr      1, 2
        call    g
        call    missing
        stop
.func g
        wr      0
        ret
";
        let obj1 = assemble(&syntax, 0x7E, src, false).unwrap();
        let text = disassemble_object(&syntax, &obj1);
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(obj1, obj2);
    }

    #[test]
    fn unknown_byte_falls_back_to_byte_directive_and_round_trips() {
        let syntax = test_syntax();
        // Hand-build an object with an undecodable byte (0x55 not in table).
        let obj = crate::formats::object::ObjectFile::v2(
            0x7E,
            vec![crate::formats::object::Symbol {
                name: "f".into(),
                def: crate::formats::object::SymbolDef::Defined { blob: 0 },
            }],
            vec![vec![0x0E, 0x55, 0x02]],
            vec![],
            None,
        );
        let text = disassemble_object(&syntax, &obj);
        assert!(text.contains(".byte   85"));
        let back = assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(back.blobs, obj.blobs);
    }

    #[test]
    fn executable_disassembly_discovers_functions_by_traversal() {
        let syntax = test_syntax();
        // f at 0 calls g at 7: f = [0E][21 off=+1][02] (call end 6; 7-6=1),
        // g = [0E][0B].
        let code = vec![0x0E, 0x21, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B];
        let exe = Executable::code_only(0x7E, 0, code);
        let text = disassemble_executable(&syntax, &exe, None);
        assert!(text.contains(".func main")); // entry root is named main
        assert!(text.contains(".func func_0007"));
        assert!(text.contains("call    func_0007"));
        assert!(text.contains("ret"));
    }

    #[test]
    fn entry_valued_operand_byte_does_not_split_functions() {
        let syntax = test_syntax();
        // f calls g at 20 (0x14): call offset = 20 - 6 = 14 = 0x0E — the
        // operand's first LE byte EQUALS the entry opcode. A byte-scanning
        // discoverer would invent a bogus function at addr 2; traversal
        // must not. Bytes 7..20 are unreachable padding → .byte lines.
        let mut code = vec![0x0E, 0x21, 0x0E, 0x00, 0x00, 0x00, 0x02];
        code.extend(std::iter::repeat_n(0x01, 13)); // unreachable nops
        code.extend([0x0E, 0x0B]); // g at 20
        let exe = Executable::code_only(0x7E, 0, code);
        let text = disassemble_executable(&syntax, &exe, None);
        assert!(text.contains(".func main")); // entry root is named main
        assert!(text.contains(".func func_0014"));
        assert!(
            !text.contains("func_0002"),
            "operand byte must not become a function"
        );
        assert!(text.contains("call    func_0014"));
        assert!(
            text.contains(".byte   1"),
            "unreachable padding dumps as bytes"
        );
    }

    #[test]
    fn branch_traversal_discovers_fall_through() {
        let syntax = test_syntax();
        // 0: ent | 1: br +1 -> 4 | 3: stop (fall-through, must be discovered) | 4: ret
        let code = vec![0x0E, 0x22, 0x01, 0x02, 0x0B];
        let exe = Executable::code_only(0x7E, 0, code);
        let text = disassemble_executable(&syntax, &exe, None);
        assert!(
            text.contains("stop"),
            "fall-through path must be discovered"
        );
        assert!(text.contains("ret"));
        assert!(text.contains("br      L0004"));
        assert!(!text.contains(".byte"), "everything reachable, no gaps");
    }

    #[test]
    fn cross_region_jump_falls_back_to_bytes() {
        let syntax = test_syntax();
        // f calls g (so g is a root) AND jumps into g's BODY (addr 13):
        // 0: ent | 1: call +6 -> 12 | 6: jmp +2 -> 13 | 11: stop | 12: ent | 13: ret
        let code = vec![
            0x0E, 0x21, 0x06, 0x00, 0x00, 0x00, 0x20, 0x02, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B,
        ];
        let exe = Executable::code_only(0x7E, 0, code);
        let text = disassemble_executable(&syntax, &exe, None);
        assert!(text.contains(".func func_000C"));
        assert!(text.contains("call    func_000C"));
        // the jmp into g's body cannot be a local label -> whole instruction as bytes
        assert!(text.contains(".byte   32")); // 0x20 opcode byte
        assert!(
            !text.contains("jmp"),
            "cross-region jmp must not print as jmp"
        );
        assert!(text.contains("ret"));
    }

    #[test]
    fn short_call_in_executable_prints_far_mnemonic() {
        let syntax = test_syntax();
        // Add a short-call opcode to a LOCAL syntax copy: fixture has none.
        let mut syntax = syntax;
        syntax.entries.push(SyntaxEntry {
            opcode: 0x31,
            mnemonic: "call.s",
            operand: OperandKind::RelI8,
            flow: Flow::Call,
        });
        syntax.relax_pairs.push(RelaxPair {
            far: 0x21,
            short: 0x31,
        });
        // f at 0 short-calls g at 4: call.s at 1, end 3, off = +1.
        let code = vec![0x0E, 0x31, 0x01, 0x02, 0x0E, 0x0B];
        let exe = Executable::code_only(0x7E, 0, code);
        let text = disassemble_executable(&syntax, &exe, None);
        assert!(
            text.contains("call    func_0004"),
            "short call prints far mnemonic:\n{text}"
        );
        assert!(!text.contains("call.s"), "call.s must not appear:\n{text}");
    }

    // test_syntax() + the 0x21/0x31 call pair, exactly as
    // `short_call_in_executable_prints_far_mnemonic` builds it inline
    // (same shape as layout.rs's `syntax_with_short_call()`).
    fn syntax_with_pairs() -> crate::asm::syntax::ArchSyntax {
        let mut syntax = test_syntax();
        syntax.entries.push(SyntaxEntry {
            opcode: 0x31,
            mnemonic: "call.s",
            operand: OperandKind::RelI8,
            flow: Flow::Call,
        });
        syntax.relax_pairs.push(RelaxPair {
            far: 0x21,
            short: 0x31,
        });
        syntax
    }

    #[test]
    fn executable_tail_jump_prints_symbol_form_and_reassembles() {
        let syntax = syntax_with_pairs();
        // main calls f (root), f tail-jumps main: infinite loop program.
        let src = "\
.func main
        call    f
        stop
.func f
        jmp     @main
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let out = crate::linker::link(&syntax, &[obj], &[], crate::linker::LinkOptions::default())
            .unwrap();
        let text = disassemble_executable(&syntax, &out.executable, None);
        assert!(text.contains("jmp     @main"), "{text}");
        assert!(!text.contains(".byte"), "{text}");
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        let out2 =
            crate::linker::link(&syntax, &[obj2], &[], crate::linker::LinkOptions::default())
                .unwrap();
        assert_eq!(out2.executable.code, out.executable.code);
    }

    #[test]
    fn object_call_without_relocation_falls_back_to_bytes() {
        let syntax = test_syntax();
        let obj = crate::formats::object::ObjectFile::v2(
            0x7E,
            vec![crate::formats::object::Symbol {
                name: "f".into(),
                def: crate::formats::object::SymbolDef::Defined { blob: 0 },
            }],
            // ent, call with a PATCHED (non-hole) offset and NO reloc, stop
            vec![vec![0x0E, 0x21, 0x02, 0x00, 0x00, 0x00, 0x02]],
            vec![],
            None,
        );
        let text = disassemble_object(&syntax, &obj);
        assert!(
            text.contains(".byte   33"),
            "0x21 opcode dumps as byte:\n{text}"
        );
        assert!(!text.contains("L0"), "no phantom labels:\n{text}");
        // Round-trip still holds through the fallback:
        let back = crate::asm::assembler::assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(back.blobs, obj.blobs);
    }

    #[test]
    fn object_symbol_jump_prints_at_form_and_round_trips() {
        let syntax = test_syntax();
        let src = ".func f\n        jmp @g\n        stop\n.func g\n        ret\n";
        let obj1 = assemble(&syntax, 0x7E, src, false).unwrap();
        let text = disassemble_object(&syntax, &obj1);
        assert!(text.contains("jmp     @g"), "{text}");
        assert!(
            !text.contains("L0"),
            "no phantom label for the reloc'd jump: {text}"
        );
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(obj1, obj2);
    }

    #[test]
    fn self_recursive_tail_jump_round_trips() {
        // A jump to one's OWN root prints in symbol form and survives
        // the round trip.
        let syntax = test_syntax();
        let src = ".func main\n        jmp @main\n";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let out = crate::linker::link(&syntax, &[obj], &[], crate::linker::LinkOptions::default())
            .unwrap();
        let text = disassemble_executable(&syntax, &out.executable, None);
        assert!(text.contains("jmp     @main"), "{text}");
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        let out2 =
            crate::linker::link(&syntax, &[obj2], &[], crate::linker::LinkOptions::default())
                .unwrap();
        assert_eq!(out2.executable.code, out.executable.code);
    }

    #[test]
    fn local_functions_round_trip_through_object_disassembly() {
        let syntax = test_syntax();
        let src = ".func api\n        call helper\n        stop\n.func helper local\n        ret\n";
        let obj1 = assemble(&syntax, 0x7E, src, false).unwrap();
        let text = disassemble_object(&syntax, &obj1);
        assert!(text.contains(".func helper local"), "{text}");
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(obj1, obj2);
    }

    #[test]
    fn map_aware_executable_dis_prefers_map_names_none_pins_today() {
        use crate::linker::{MapFile, MapFunction};
        let syntax = test_syntax();
        // Same shape as `executable_disassembly_discovers_functions_by_traversal`:
        // f at 0 calls g at 7 (call end 6; 7-6=1), g = [0E][0B].
        let code = vec![0x0E, 0x21, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B];
        let exe = Executable::code_only(0x7E, 0, code);

        // `None` -> byte-identical to today's synthesized name (pinned).
        let text_no_map = disassemble_executable(&syntax, &exe, None);
        assert!(text_no_map.contains(".func main"));
        assert!(text_no_map.contains(".func func_0007"));
        assert!(text_no_map.contains("call    func_0007"));

        // A map naming the callee root wins over `func_XXXX` synthesis.
        let map = MapFile {
            arch: 0x7E,
            functions: vec![MapFunction {
                name: "helper".into(),
                start: 7,
                end: 9,
                labels: vec![],
                lines: vec![],
            }],
            bindings: vec![],
        };
        let text_with_map = disassemble_executable(&syntax, &exe, Some(&map));
        assert!(text_with_map.contains(".func helper"), "{text_with_map}");
        assert!(text_with_map.contains("call    helper"), "{text_with_map}");
        assert!(!text_with_map.contains("func_0007"), "{text_with_map}");
    }

    /// The core crate cannot depend on PM-1: a minimal local `ArchSyntax`
    /// with exactly the entries the derived golden uses (docs/isa.md opcodes),
    /// mirroring `fixture::test_syntax()`.
    fn pm1_like_syntax() -> crate::asm::syntax::ArchSyntax {
        use Flow::{Branch, FallThrough as FT, Stop};
        crate::asm::syntax::ArchSyntax {
            entries: vec![
                SyntaxEntry {
                    opcode: 0x0D,
                    mnemonic: "ent",
                    operand: OperandKind::None,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x05,
                    mnemonic: "rgt",
                    operand: OperandKind::None,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x06,
                    mnemonic: "wr",
                    operand: OperandKind::SymbolVec,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x19,
                    mnemonic: "jm.s",
                    operand: OperandKind::RelI8,
                    flow: Branch,
                },
                SyntaxEntry {
                    opcode: 0x02,
                    mnemonic: "stp",
                    operand: OperandKind::None,
                    flow: Stop,
                },
            ],
            relax_pairs: vec![],
            entry_opcode: 0x0D,
            break_opcode: None,
            trap_opcode: None,
            caps: crate::asm::AsmCaps::default(),
        }
    }

    #[test]
    fn listing_renders_the_derived_golden() {
        use crate::linker::{MapFile, MapFunction};
        // 0: ent | 1: rgt | 2-3: wr 1 (0x06 0x81) | 4-5: jm.s -5 → 1 | 6: stp
        let exe = Executable::code_only(0x01, 0, vec![0x0D, 0x05, 0x06, 0x81, 0x19, 0xFB, 0x02]);
        let map = MapFile {
            arch: 0x01,
            functions: vec![MapFunction {
                name: "main".into(),
                start: 0,
                end: 7,
                labels: vec![("L1".into(), 1)],
                lines: vec![],
            }],
            bindings: vec![],
        };
        let listing = listing_executable(&pm1_like_syntax(), &exe, Some(&map));
        let expected = "\
main:
  0000:  0D              ent
  0001:  05              rgt
  0002:  06 81           wr      1
  0004:  19 FB           jm.s    0x0001 <main.L1>
  0006:  02              stp
";
        assert_eq!(listing, expected);
    }

    #[test]
    fn listing_line_symbol_vec_reports_len_and_joined_operand() {
        let syntax = pm1_like_syntax();
        let code = [0x06, 0x01, 0x82];
        let (line, len) = listing_line(&syntax, &code, 0, &|_| None);
        assert_eq!(len, 3);
        assert!(line.ends_with("wr      1, 2"), "{line}");
    }

    #[test]
    fn listing_line_lengths_cover_the_golden_exe() {
        let syntax = pm1_like_syntax();
        let code: Vec<u8> = vec![0x0D, 0x05, 0x06, 0x81, 0x19, 0xFB, 0x02];
        let mut addr = 0u32;
        let mut total = 0u32;
        while (addr as usize) < code.len() {
            let (_, len) = listing_line(&syntax, &code, addr, &|_| None);
            total += len;
            addr += len;
        }
        assert_eq!(total, code.len() as u32);
    }
}
