//! Binary → canonical `.pma` text (docs/formats.md (assembly text)).
//! Output is valid assembler input, object or linked image, with or
//! without a debug/map sidecar. An object's round trip is byte-exact; a
//! linked image's reassemble-and-relink reproduces an equivalent image,
//! not always the same bytes — a frame that originated from a
//! declarative binding always disassembles to raw `.frame`/`call.m`
//! syntax, and relinking that does not necessarily reorder the tables
//! section the way the original composition did (docs/formats.md
//! (assembly text)).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::decode::{Body, Decoded, DecodedOperand, decode_at, decode_stream};
use super::fmt::wrap_operand_list;
use super::syntax::{ArchSyntax, Flow};
use crate::formats::executable::Executable;
use crate::formats::object::{BlobVariant, BoundCall, ObjectFile, SymbolDef, TapeBinding};
use crate::linker::MapFile;
use crate::vm::OperandKind;

/// Canonical `.pma` trailing-comment column floor (docs/formats.md
/// (assembly text)), the same stop `fmt.rs` uses — needed only to place
/// [`routine_line`]'s `; derived` marker on a synthesized callee
/// signature, so the rendered line still lands on the grid.
const COMMENT_COL: usize = 32;

/// Canonical `.pma` mnemonic column (docs/formats.md (assembly text)),
/// the same stop `fmt.rs` uses.
const MNEMONIC_COL: usize = 8;

/// Canonical `.pma` operand column (docs/formats.md (assembly text)),
/// the same stop `fmt.rs` uses.
const OPERAND_COL: usize = 16;

/// Canonical .pma grid (docs/formats.md (assembly text)): label col 0,
/// mnemonic col 8, operand col 16; trailing spaces trimmed. A label
/// field (name + `:`) of 8+ chars would touch the mnemonic column with
/// no separating space, so it moves to its own line instead — the
/// return value has no trailing newline (callers already append one)
/// but may contain an interior one.
pub fn grid_line(label: Option<&str>, mnemonic: &str, operand: &str) -> String {
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

/// The 0-based column a directive's operand list starts at: the operand
/// column, or one space past a keyword that already reaches it. Same
/// rule as [`grid_line`]'s own layout and as `fmt.rs`'s `pad_to`, which
/// is what lets an emitted list and a reformatted one wrap at the same
/// places.
fn operand_start_col(word: &str) -> usize {
    let col = MNEMONIC_COL + word.chars().count();
    if col < OPERAND_COL {
        OPERAND_COL
    } else {
        col + 1
    }
}

/// One grid line whose operand is an unbounded list — `.targets`,
/// `.exits`, `.map` — packed onto as few physical lines as fit the
/// printer's width budget (docs/formats.md (assembly text)). The packing
/// is `fmt.rs`'s own [`wrap_operand_list`], called at the same start
/// column the printer would compute, so emitted text needs no later
/// formatting pass and cannot drift from what one would produce.
///
/// `elements` are the list's items as the printer sees them: one per
/// name for `.targets`/`.exits`, one per top-level clause (`<k>`,
/// `rmap=(…)`, `wmap=(…)`) for `.map` — never one pre-joined string, or
/// a break would fall in a different place than the printer's.
fn grid_list_line(label: Option<&str>, word: &str, elements: &[String]) -> String {
    let texts: Vec<&str> = elements.iter().map(String::as_str).collect();
    let operand = wrap_operand_list(&texts, operand_start_col(word));
    grid_line(label, word, &operand)
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

/// Renders a decoded `wrmv`-style two-vector operand as `[w…], [m…]`
/// (docs/formats.md (assembly text)): the write group with `-` for the
/// keep marker (`0x7F`), the move group with the move glyphs (0 → `.`,
/// 1 → `<`, 2 → `>`; an out-of-vocabulary code renders raw — defensive,
/// the assembler never emits one). Independent of `caps.vectors`: the
/// kind exists only under vector-capable dialects.
fn write_move_operand_text(writes: &[u32], moves: &[u32]) -> String {
    let w: Vec<String> = writes
        .iter()
        .map(|&e| {
            if e == 0x7F {
                "-".to_string()
            } else {
                e.to_string()
            }
        })
        .collect();
    let m: Vec<String> = moves
        .iter()
        .map(|&e| match e {
            0 => ".".to_string(),
            1 => "<".to_string(),
            2 => ">".to_string(),
            other => other.to_string(),
        })
        .collect();
    format!("[{}], [{}]", w.join(", "), m.join(", "))
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
        // One element per top-level clause, which is where a `.map` list
        // breaks when it outgrows the line budget.
        let mut clauses = vec![k.to_string()];
        if !rmap.is_empty() {
            clauses.push(format!("rmap=({})", dense_map_pairs(rmap)));
        }
        if !wmap.is_empty() {
            clauses.push(format!("wmap=({})", dense_map_pairs(wmap)));
        }
        out.push_str(&grid_list_line(None, ".map", &clauses));
        out.push('\n');
    }
    if !frame.exits.is_empty() {
        let names: Vec<String> = frame.exits.iter().map(|&o| exit_name(o)).collect();
        out.push_str(&grid_list_line(None, ".exits", &names));
        out.push('\n');
    }
}

/// `(blob index, blob-local table offset) -> synthesized `Tn` label`: the
/// code section looks up a table reference's display name here.
type TableLabels = HashMap<(u32, u32), String>;

/// `(blob index, blob-local CODE offset) -> the one label that names that
/// position`: what the tables section prints for a dispatch entry or a
/// frame exit, and what the code section defines at the address. An
/// executable has one logical blob (index 0, absolute addresses standing
/// in for blob-local offsets), so [`disassemble_object`] and
/// [`disassemble_executable`] share this type and the
/// [`table_code_labels`] walk that fills it.
type CodeLabels = HashMap<(u32, u32), String>;

/// The synthesized name for a code position with no debug label — the
/// single `L<addr>` synthesizer, shared by jump targets and by the
/// positions the tables section names.
fn synthesized_label(addr: u32) -> String {
    format!("L{addr:04X}")
}

/// The label naming a code position: `known` when the caller already has
/// one on file — an object's own `-g` debug label, or a linked image's
/// map-sidecar function label — else the synthesized
/// [`synthesized_label`] name. One rule for both renderers and both
/// sections, so a `.targets`/`.exits` operand and the code line it
/// points at always agree.
fn code_label(known: Option<String>, offset: u32) -> String {
    known.unwrap_or_else(|| synthesized_label(offset))
}

/// Every code position the tables section will name — a dispatch table's
/// entries and a frame descriptor's exits — with the label chosen for it
/// by [`code_label`]. `.targets` and `.exits` take label NAMES, never
/// offsets (docs/formats.md (assembly text)), so the disassembly can only
/// reassemble if each name it prints is also defined at that address;
/// choosing every name here, once, is what makes the two sections meet.
///
/// `known(blob, offset)` supplies the debug/map label already on file at
/// a position — an object's per-blob `-g` labels for
/// [`disassemble_object`], a linked image's map-sidecar function labels
/// (one flat address space, blob always 0) for
/// [`disassemble_executable`] — and [`code_label`] falls back to a
/// synthesized name where it returns `None`. The walk over table kinds
/// and table bytes is identical for both callers; only the label source
/// differs, which is why it lives here once instead of being forked per
/// renderer.
///
/// The names a position can carry are exactly the ones an assembler
/// could have written: at every position listed here, a label already on
/// file is replayed, and an unlabeled one gets an address. Every
/// position listed here is an instruction start in an assembler- or
/// linker-produced image (a source label can land nowhere else), which
/// is what guarantees the code section reaches it and defines the label.
fn table_code_labels(
    table_blobs: &[Vec<u8>],
    per_blob: &[BTreeMap<u32, TableKind>],
    known: impl Fn(u32, u32) -> Option<String>,
) -> CodeLabels {
    let mut labels = CodeLabels::new();
    for (blob, starts) in per_blob.iter().enumerate() {
        let Some(tb) = table_blobs.get(blob) else {
            continue;
        };
        let bounds: Vec<u32> = starts.keys().copied().collect();
        for (idx, (&start, &kind)) in starts.iter().enumerate() {
            let end = bounds.get(idx + 1).copied().unwrap_or(tb.len() as u32);
            let offsets = match kind {
                TableKind::Dispatch => dispatch_entries_within(tb, start, end),
                TableKind::Frame => parse_frame_descriptor(tb, start)
                    .map(|f| f.exits)
                    .unwrap_or_default(),
                TableKind::Match => continue,
            };
            for offset in offsets {
                labels
                    .entry((blob as u32, offset))
                    .or_insert_with(|| code_label(known(blob as u32, offset), offset));
            }
        }
    }
    labels
}

/// Renders the `.section tables` body for `obj`'s table blobs and returns
/// it with a `(blob, table-offset) -> synthesized label` map for the code
/// section to reference an operand by name, plus the `(blob, code
/// offset) -> label` map the code section must define (see
/// [`table_code_labels`]).
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
fn render_tables_section(
    syntax: &ArchSyntax,
    obj: &ObjectFile,
) -> Option<(String, TableLabels, CodeLabels)> {
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

    // Every code position the section is about to name, resolved once so
    // both sections agree on it.
    let code_labels = table_code_labels(table_blobs, &per_blob, |blob, offset| {
        obj.debug
            .as_ref()
            .and_then(|d| d.get(blob as usize))
            .and_then(|bd| bd.labels.iter().find(|(_, o)| *o == offset))
            .map(|(n, _)| n.clone())
    });

    let (body, labels) = render_tables(table_blobs, &per_blob, &code_labels);
    Some((body, labels, code_labels))
}

/// Renders every table `table_blobs`/`per_blob` describe into one
/// `.section tables` body — the walk both [`disassemble_object`] and
/// [`disassemble_executable`] drive, extracted here rather than kept as
/// two near-identical copies (a second copy is exactly how the
/// executable path's naming/wrapping/blank-line defects happened the
/// first time). `code_labels` supplies every name a `.targets`/`.exits`
/// entry needs ([`table_code_labels`]'s doc); a miss there is an
/// invariant break for either caller, not a data case. Returns the body
/// text (no leading or trailing `.section` line — callers own those) and
/// the `(blob, table-offset) -> synthesized label` map the code section
/// needs for a `TableAddr`/`FramedCall` operand.
///
/// Frame descriptors get synthesized `F<n>` labels, match/dispatch
/// tables `T<n>` — the code section references each by its kind's
/// operand (a `call.m` names an `F`, an `mtc`/`djmp` a `T`).
fn render_tables(
    table_blobs: &[Vec<u8>],
    per_blob: &[BTreeMap<u32, TableKind>],
    code_labels: &CodeLabels,
) -> (String, TableLabels) {
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
            // The rendered name comes only from the map the code section
            // will define labels from. A name minted here instead would
            // be absent from that map, so the code section would never
            // define it and the text would silently stop reassembling —
            // exactly the defect the one-map rule removes. Both passes
            // walk the same tables through the same readers, so a miss is
            // a broken invariant, not a data case.
            let named = |offset: u32| -> String {
                code_labels
                    .get(&(blob as u32, offset))
                    .cloned()
                    .expect("table_code_labels named every entry the tables section renders")
            };
            let mut table = String::new();
            match kind {
                TableKind::Match => render_match_table(&mut table, &name, tb, start, end),
                TableKind::Dispatch => {
                    render_dispatch_table(&mut table, &name, tb, start, end, &named)
                }
                TableKind::Frame => {
                    if let Some(frame) = parse_frame_descriptor(tb, start) {
                        render_frame_table(&mut table, &name, &frame, named);
                    }
                }
            }
            if table.is_empty() {
                continue;
            }
            // One blank line between tables — what a person writing
            // assembly puts there, and what keeps a wide table's width
            // from setting the trailing-comment column for every other
            // line in the section (docs/formats.md (assembly text): the
            // comment column is per group, and a blank line ends a
            // group). The first table gets none: `body` is empty only
            // before it.
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&table);
        }
    }
    (body, labels)
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
/// `entry_count × u32 LE` code offsets — blob-relative in an object,
/// absolute in a linked image) as a `.targets` line. `named` turns each
/// entry offset into the label that names it — a debug/map label already
/// on file, or a synthesized one — and the code section defines every one
/// of them at its address, so the rendered text reassembles. Shared by
/// both renderers; the list wraps at the printer's width budget.
fn render_dispatch_table(
    out: &mut String,
    name: &str,
    tb: &[u8],
    start: u32,
    end: u32,
    named: &impl Fn(u32) -> String,
) {
    let names: Vec<String> = dispatch_entries_within(tb, start, end)
        .into_iter()
        .map(named)
        .collect();
    if names.is_empty() {
        return;
    }
    out.push_str(&grid_list_line(Some(name), ".targets", &names));
    out.push('\n');
}

/// One canonical `.routine` line (newline included), the exact grid
/// `fmt.rs`'s printer normalizes to. `comment`, when given, is a
/// trailing `; <text>` marker padded to the group's comment column — a
/// `.routine` line is always its own single-member group (it and the
/// `.func` line right after it are both `fmt.rs`'s `Structural` pieces,
/// and a structural piece ends the group it starts —
/// `comment_columns`'s doc), so the column is `max(COMMENT_COL, code
/// width + 1)` with no other line's width to account for.
fn routine_line(name: &str, tapes: u8, cardinalities: &[u32], comment: Option<&str>) -> String {
    let alpha = cardinalities
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let code = format!(".routine {name}, tapes={tapes}, alpha=({alpha})");
    let Some(comment) = comment else {
        return format!("{code}\n");
    };
    let len = code.chars().count();
    let mut line = code;
    if len < COMMENT_COL {
        line.push_str(&" ".repeat(COMMENT_COL - len));
    } else {
        line.push(' ');
    }
    line.push_str("; ");
    line.push_str(comment);
    line.push('\n');
    line
}

/// The entries of the dispatch table at `start` in a LINKED table
/// section (`entry_count u16 LE`, then `entry_count × u32 LE` absolute
/// code addresses). Defensive: a truncated table yields the entries
/// that fit rather than panicking (the linker never emits one).
fn dispatch_entries(tables: &[u8], start: u32) -> Vec<u32> {
    dispatch_entries_within(tables, start, tables.len() as u32)
}

/// [`dispatch_entries`] bounded above by `end` — the next table's start
/// in a concatenated blob, which is how the object path stops a
/// truncated table from reading into its neighbour. Naming a dispatch
/// entry and rendering it read the SAME entry list through this, so the
/// tables section can never print a name the label pass did not choose.
fn dispatch_entries_within(tables: &[u8], start: u32, end: u32) -> Vec<u32> {
    let base = start as usize;
    let (Some(&lo), Some(&hi)) = (tables.get(base), tables.get(base + 1)) else {
        return Vec::new();
    };
    let count = u16::from_le_bytes([lo, hi]) as usize;
    let limit = (end as usize).min(tables.len());
    let mut entries = Vec::with_capacity(count);
    let mut pos = base + 2;
    for _ in 0..count {
        if pos + 4 > limit {
            break;
        }
        entries.push(u32::from_le_bytes(tables[pos..pos + 4].try_into().unwrap()));
        pos += 4;
    }
    entries
}

pub fn disassemble_object(syntax: &ArchSyntax, obj: &ObjectFile) -> String {
    let mut text = String::new();
    // The program bit leads the dump: `.volatile` before the first `.func`
    // is what sets it on the way back in (docs/formats.md (MO)).
    // Both this and the per-blob tags below are gated on the dialect's own
    // capability — a dialect that cannot parse the directive must never be
    // handed text carrying it.
    if syntax.caps.volatile && obj.program_volatile {
        text.push_str(".volatile\n");
    }
    // Tables render first, in their own section, with `.section code`
    // before the function bodies. A no-tables object emits neither line,
    // so its output stays byte-identical to a pre-tables object.
    let (table_labels, code_labels) = match render_tables_section(syntax, obj) {
        Some((section, labels, code_labels)) => {
            text.push_str(".section tables\n");
            text.push_str(&section);
            text.push_str(".section code\n");
            (labels, code_labels)
        }
        None => (HashMap::new(), CodeLabels::new()),
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
        // The body renders once into `block`; a `Both`-tagged blob then
        // prints it under BOTH headers — bare and `.volatile` — which is
        // what lets the two same-name blocks dedup back into this one blob
        // on the way in (docs/formats.md (MO)).
        let mut block = String::new();
        let out = &mut block;
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

        // Every label this blob defines: a jump target synthesizes its
        // `L<addr>` name, and a position the tables section names takes
        // THAT name — the same map the `.targets`/`.exits` operands were
        // rendered from — so the two sections spell the address alike and
        // the text reassembles. A position that is both keeps the tables
        // section's name, which is the one already printed above.
        let mut labels_at: BTreeMap<u32, String> =
            targets.iter().map(|&t| (t, synthesized_label(t))).collect();
        for ((b, offset), name) in &code_labels {
            if *b == blob {
                labels_at.insert(*offset, name.clone());
            }
        }

        for d in &decoded {
            let label_name = labels_at.get(&d.addr).cloned();
            match &d.body {
                Body::Raw(b) => {
                    push_byte_lines(out, label_name.as_deref(), &[*b]);
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
                                // The label defined at the target, which
                                // is the tables section's name for it
                                // when that section names it too.
                                Some(
                                    labels_at
                                        .get(t)
                                        .cloned()
                                        .unwrap_or_else(|| synthesized_label(*t)),
                                )
                            }
                        }
                        DecodedOperand::Imm(n) => Some(format!("#{n}")),
                        DecodedOperand::WriteMove { writes, moves } => {
                            Some(write_move_operand_text(writes, moves))
                        }
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
                                out,
                                label_name.as_deref(),
                                &code[d.addr as usize..(d.addr + d.len) as usize],
                            );
                        }
                    }
                }
            }
        }

        // One header per build column this blob serves: an untagged or
        // `Normal` blob is bare, a `Volatile` one carries the directive,
        // and a `Both` one prints both ways.
        let gated: &[bool] = match variant_of(obj, blob) {
            _ if !syntax.caps.volatile => &[false],
            BlobVariant::Normal => &[false],
            BlobVariant::Volatile => &[true],
            BlobVariant::Both => &[false, true],
        };
        for &volatile in gated {
            // A signed object re-emits each function's `.routine` line
            // ahead of its `.func`, so dis ∘ asm preserves signatures
            // (they are all-or-none per object, parallel to blobs —
            // docs/formats.md (.pmo)).
            if let Some(sig) = obj.signatures.as_ref().and_then(|s| s.get(blob as usize)) {
                text.push_str(&routine_line(
                    &symbol.name,
                    sig.arity,
                    &sig.cardinalities,
                    None,
                ));
            }
            text.push_str(&format!(
                ".func {}{}\n",
                symbol.name,
                if local { " local" } else { "" }
            ));
            if volatile {
                text.push_str(".volatile\n");
            }
            text.push_str(&block);
        }
    }
    text
}

/// A blob's build column, reading an object with no variant records at all
/// — a legacy or hand-assembled one — as all-`Normal`, exactly as the
/// linker's selection rule does (docs/formats.md (MO)).
fn variant_of(obj: &ObjectFile, blob: u32) -> BlobVariant {
    obj.variants
        .as_ref()
        .and_then(|tags| tags.get(blob as usize).copied())
        .unwrap_or(BlobVariant::Normal)
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
    // table in `exe.tables`); dispatch entries and frame exits join the
    // work list directly below as label candidates (never as roots) — no
    // separate set of their addresses is kept, since [`table_code_labels`]
    // re-derives the same entries from `table_kinds` when the tables
    // section is rendered.
    let mut table_kinds: BTreeMap<u32, TableKind> = BTreeMap::new();
    // Each discovered `call.m` site's callee address — a `call.m` always calls
    // the same callee, whatever composite its column selects, so this names the
    // routine of every composite reachable through the site (the map-less
    // legend's routine names, docs/formats.md (image-inspectability principle)).
    let mut site_target: HashMap<u32, u32> = HashMap::new();
    // Jump targets whose byte is the entry opcode — candidate function
    // starts, resolved by the cut filter once the walk has finished.
    let mut tail_jump_targets: BTreeSet<u32> = BTreeSet::new();
    // The code edges a table carries, which no `RelTarget` operand shows:
    // each table's selectable code addresses, and every instruction that
    // reaches a table. The cut filter reads both.
    let mut table_targets: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut table_sites: Vec<(u32, u32)> = Vec::new();
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
        if let DecodedOperand::TableAddr(t) = operand {
            // Every reference is recorded, not just the first: two sites in
            // different regions reach the same table, and the cut filter
            // below weighs each site against the entries it can select.
            table_sites.push((addr, *t));
            if !table_kinds.contains_key(t) {
                let kind = if entry.flow == Flow::FallThrough {
                    TableKind::Match
                } else {
                    TableKind::Dispatch
                };
                table_kinds.insert(*t, kind);
                if matches!(kind, TableKind::Dispatch) {
                    let targets = dispatch_entries(&exe.tables, *t);
                    for &target in &targets {
                        work.push(target);
                    }
                    table_targets.insert(*t, targets);
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
            if let Some(desc_off) = resolve_site(exe, *site) {
                table_sites.push((addr, desc_off));
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    table_kinds.entry(desc_off)
                {
                    slot.insert(TableKind::Frame);
                    if let Some(frame) = parse_frame_descriptor(&exe.tables, desc_off) {
                        for &exit in &frame.exits {
                            work.push(exit);
                        }
                        table_targets.insert(desc_off, frame.exits);
                    }
                }
            }
        }
        match (entry.flow, operand) {
            (Flow::FallThrough, _) => work.push(next),
            (Flow::Stop, _) => {}
            (Flow::Jump, DecodedOperand::RelTarget(t)) => {
                // A jump landing on an entry prologue looks like a tail
                // call, which would make its target a function start. Only
                // a candidate here: the walk has not finished, and whether
                // the boundary is real is decided once it has (see the cut
                // filter below).
                if code.get(*t as usize) == Some(&syntax.entry_opcode) {
                    tail_jump_targets.insert(*t);
                }
                work.push(*t);
            }
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
    // single descriptor). Register the whole directory as frame tables so
    // all get an `F<n>` label + render — their exits become label
    // candidates automatically when [`table_code_labels`] walks
    // `table_kinds` below, without a separate candidate set
    // (docs/formats.md (image-inspectability principle)). Their exits are
    // recorded for the cut filter here too, without a site: a descriptor no
    // site resolved to is reachable under some context this walk cannot
    // name, so its exits are real code edges all the same.
    let region = parse_frames_region(exe);
    if let Some(region) = &region {
        for &desc_off in &region.directory {
            if let std::collections::btree_map::Entry::Vacant(slot) = table_kinds.entry(desc_off) {
                slot.insert(TableKind::Frame);
            }
            if let std::collections::btree_map::Entry::Vacant(slot) = table_targets.entry(desc_off)
                && let Some(frame) = parse_frame_descriptor(&exe.tables, desc_off)
            {
                slot.insert(frame.exits);
            }
        }
    }

    // Promote the tail-jump candidates that name a real function start.
    //
    // A function reached ONLY by a tail jump is named by nothing else in a
    // pure image: it has no relocation, no call, and images carry no
    // function table. Folded into its caller's region it renders as a bare
    // local label, which is lossy in BOTH widths — a symbol site's width is
    // the linker's to choose and a local label's is the assembler's, so a
    // far site re-narrows on reassembly, and a narrowed site, shorter in
    // text than the far relocation the object held, shifts the reassembled
    // object's layout under every other jump spanning it and flips their
    // widths too. Rendering it as a root avoids both, because symbol form
    // restores the OBJECT layout: a symbol operand always reassembles far
    // (which is why the `far_mnemonic` display below prints a
    // linker-narrowed site in its far form), and the linker then re-derives
    // the same narrowing. That is what makes dis → asm → link byte-exact.
    //
    // The entry byte is a necessary signal — the linker rejects a function
    // blob that does not open with one — but not a sufficient one, since a
    // dialect may let that byte stand inside a body as an ordinary
    // instruction. Splitting a body there would be worse than the fold it
    // avoids: the halves become separate functions, ordered by the linker's
    // own discovery rather than by the text, so any edge that ran across
    // the invented boundary now runs between two independently placed
    // functions. Where the edge was a local label it cannot even be spelled
    // (it degrades to `.byte`, which the linker then rejects); where it was
    // a fall-through it silently lands somewhere else.
    //
    // So promote only across a genuine CUT of the discovered code:
    //
    //   * nothing falls through INTO it, and the region it opens does not
    //     fall out of its own END (it finishes on a stop or a resolved
    //     jump). Both are the same layout dependency, one boundary apart.
    //   * no local-label edge crosses it in either direction.
    //   * every table it could divide stays whole: a table's SITES and its
    //     code entries must all land in one region. A table is stored per
    //     function and belongs to exactly one, so a boundary between two of
    //     its references leaves the text describing a table tied to two
    //     functions — which the assembler rejects outright — and a boundary
    //     between a reference and an entry leaves the entry spelled as some
    //     other region's label. Both refusals happen before anything runs.
    //
    // A real tail-jump callee passes all three: its caller ends in the
    // jump, the jump itself renders symbolically rather than as a crossing
    // label, and the callee ends the way any function does. Declining costs
    // only the fold, which is what the renderer did before promotion
    // existed.
    if !tail_jump_targets.is_empty() {
        // The successor and label edges the walk above followed:
        // `continue_edges` mirrors the `work.push(next)` arms (everything
        // but a resolved jump and a stop), and `rel_edges` collects the
        // relative operands whose rendering depends on the region holding
        // them.
        let mut continue_edges: Vec<(u32, u32)> = Vec::new();
        let mut rel_edges: Vec<(u32, Flow, u32)> = Vec::new();
        for d in instrs.values() {
            let Body::Instr { mnemonic, operand } = &d.body else {
                continue;
            };
            let flow = syntax.by_mnemonic(mnemonic).unwrap().flow;
            if let DecodedOperand::RelTarget(t) = operand {
                rel_edges.push((d.addr, flow, *t));
            }
            let resolved_jump =
                flow == Flow::Jump && matches!(operand, DecodedOperand::RelTarget(_));
            if flow != Flow::Stop && !resolved_jump {
                continue_edges.push((d.addr, d.addr + d.len));
            }
        }

        // Every address one table ties together: the instructions that
        // reach it and the code it can select. A match table has only the
        // former (its rows are symbol patterns, not addresses) and a
        // directory descriptor no site resolved to has only the latter —
        // both still pin the table to one function, so both are collected
        // under the same key and weighed as one set.
        let mut table_addrs: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for (key, targets) in &table_targets {
            table_addrs.entry(*key).or_default().extend(targets);
        }
        for &(site, key) in &table_sites {
            table_addrs.entry(key).or_default().push(site);
        }
        // A descriptor no site named is reachable under a context this walk
        // cannot resolve, so which region owns it cannot be established at
        // all — only that its exits agree with each other, which is the
        // weaker half of the rule and can miss a descriptor whose exits sit
        // on the far side of a boundary from its owner. Where ownership is
        // unknowable, no boundary is safe to invent.
        let unowned_table = table_targets.iter().any(|(key, targets)| {
            !targets.is_empty() && !table_sites.iter().any(|&(_, k)| k == *key)
        });

        let mut cuts: BTreeSet<u32> = tail_jump_targets
            .into_iter()
            .filter(|t| *t < len && !roots.contains(t))
            .collect();
        // Declining one candidate turns edges that pointed at it into
        // local labels, and moves the end of whatever region preceded it —
        // either can disqualify another candidate, so shrink to a fixpoint.
        // Monotone (the set only loses members), so it ends.
        loop {
            let doomed = cuts.iter().copied().find(|&t| {
                // Where the region `t` opens ends: the next boundary above
                // it, which is itself a moving target while the set shrinks.
                let end = roots
                    .range(t + 1..)
                    .next()
                    .into_iter()
                    .chain(cuts.range(t + 1..).next())
                    .min()
                    .copied()
                    .unwrap_or(len);
                continue_edges
                    .iter()
                    .any(|&(addr, next)| next == t || (addr >= t && addr < end && next >= end))
                    || rel_edges.iter().any(|&(addr, flow, target)| {
                        // A call renders by name, and a jump onto a root renders
                        // in symbol form; neither depends on the region holding
                        // it. Only a local label does.
                        let symbolic = flow == Flow::Call
                            || (flow == Flow::Jump
                                && (roots.contains(&target) || cuts.contains(&target)));
                        !symbolic && ((addr < t && target >= t) || (addr >= t && target <= t))
                    })
                    || unowned_table
                    || table_addrs.values().any(|addrs| {
                        // One region must hold the whole set. An address ON
                        // the boundary is already outside it: that is the
                        // opened region's root, which the text spells as a
                        // function name, never as a label a table can name.
                        addrs.contains(&t)
                            || (addrs.iter().any(|&x| x < t) && addrs.iter().any(|&x| x > t))
                    })
            });
            match doomed {
                Some(t) => cuts.remove(&t),
                None => break,
            };
        }
        roots.extend(cuts);
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

    // Every code position the tables section is about to name — a
    // dispatch table's entries, a frame descriptor's exits — resolved
    // once so the tables section and the code section agree on it,
    // exactly as `render_tables_section` does for an object
    // ([`table_code_labels`]'s doc). An executable is one flat address
    // space, so it plays the per-blob shape with a single blob (index 0)
    // and looks up a name in the map-sidecar function labels rather than
    // an object's per-blob debug labels; a position the map carries no
    // name for still gets [`synthesized_label`]'s fallback, which is the
    // fix for the DEFAULT case — `tmt link` on a non-`-g` object writes
    // an empty `labels` list, so a map with no `-g` data behind it named
    // nothing before this, and `--map` could not help.
    let code_labels = table_code_labels(
        std::slice::from_ref(&exe.tables),
        std::slice::from_ref(&table_kinds),
        |_blob, offset| {
            map.and_then(|m| {
                m.functions.iter().find_map(|f| {
                    f.labels
                        .iter()
                        .find(|(_, a)| *a == offset)
                        .map(|(n, _)| n.clone())
                })
            })
        },
    );

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
            None,
        ));
    }
    // Discovered tables render next in their own section, `T<n>`
    // (match/dispatch) and `F<n>` (frame) labels synthesized in ascending
    // section-offset order; the code section's operands reference them by
    // name below. An executable is one flat address space, so it plays
    // the object path's per-blob shape with a single blob (index 0) and
    // reads back through `(0, offset)` keys — the same [`render_tables`]
    // walk `render_tables_section` drives for an object, not a second
    // copy of it.
    let mut table_labels: TableLabels = HashMap::new();
    // A callee reached through a `.frame` descriptor: its virtual tape
    // count and a per-tape cardinality, filled below and read by the
    // `.routine` line the code loop prints ahead of that callee's
    // `.func` (see that step's doc for what these numbers are and are
    // not).
    let mut callee_signature: HashMap<u32, (u8, Vec<u32>)> = HashMap::new();
    if !table_kinds.is_empty() {
        out.push_str(".section tables\n");
        let (body, labels) = render_tables(
            std::slice::from_ref(&exe.tables),
            std::slice::from_ref(&table_kinds),
            &code_labels,
        );
        table_labels = labels;
        out.push_str(&body);
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
                        .and_then(|&off| table_labels.get(&(0, off)))
                        .cloned()
                        .unwrap_or_else(|| format!("@site{site}")),
                    None => format!("@site{site}"),
                };
                site_operand.insert(site as u32, text);
            }
            // Every callee reached through a frame descriptor: the
            // descriptor's own `tapes` length is its virtual tape count
            // (it MUST agree with the `.frame tapes=(…)` line already
            // rendered for it, above), and a per-tape cardinality is the
            // PHYSICAL tape that virtual tape projects onto — the
            // callee's own alphabet is consumed by the composition engine
            // at link time and does not survive into the linked image, so
            // this is a documented stand-in for it, not the original
            // value (docs/formats.md (frame descriptors)). A `call.m`
            // site always calls the same callee regardless of which
            // composite its column selects, so the first descriptor found
            // for a target is as good as any other.
            for (&site, &target) in &site_target {
                if callee_signature.contains_key(&target) {
                    continue;
                }
                let composites = region.site_composites(site as usize);
                let Some(&composite) = composites.first() else {
                    continue;
                };
                let Some(&desc_off) = region.directory.get(usize::from(composite) - 1) else {
                    continue;
                };
                let Some(frame) = parse_frame_descriptor(&exe.tables, desc_off) else {
                    continue;
                };
                let alpha = frame
                    .tapes
                    .iter()
                    .map(|&phys| {
                        exe.alphabet_cardinalities
                            .get(phys as usize)
                            .copied()
                            .unwrap_or(2)
                    })
                    .collect();
                callee_signature.insert(target, (frame.tapes.len() as u8, alpha));
            }
            out.push_str(&frames_legend(region, map, &site_target, &func_name, exe));
        }
        out.push_str(".section code\n");
    }
    for (i, &root) in roots.iter().enumerate() {
        let end = region_end(i);
        // Every function needs a `.routine` line once the image is
        // sectioned: reassembling the disassembled text treats the whole
        // thing as ONE source file, and the assembler's all-or-none rule
        // (`bad-signature`, docs/core.md (error codes)) — the entry
        // already got a line, above — then demands every function in it
        // carry one too, even a callee whose own object never signed it
        // (a linked image drops that distinction). A callee reached
        // through a frame descriptor gets [`callee_signature`]'s derived
        // values, flagged `; derived` since the alpha is the physical
        // tape's cardinality, not the routine's own (the doc above that
        // map's fill-in explains why); anything else — a plain call, or
        // a mono-stamped specialized copy — runs directly against the
        // machine's own tape space with no separate virtual alphabet, so
        // the entry's own signature is exact for it, not a placeholder,
        // and gets no comment.
        if sectioned && root != exe.entry {
            let (tapes, alpha, comment) = match callee_signature.get(&root) {
                Some((tapes, alpha)) => (*tapes, alpha.clone(), Some("derived")),
                None => (exe.tape_count, exe.alphabet_cardinalities.clone(), None),
            };
            out.push_str(&routine_line(&func_name(root), tapes, &alpha, comment));
        }
        out.push_str(&format!(".func {}\n", func_name(root)));

        // Label names within this region: jump targets synthesize
        // `LXXXX`; a table-code label (map or synthesized) keeps its own
        // name so the `.targets`/`.exits` line above and the code line
        // agree (a shared address takes the tables-section name).
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
                    labels_at.insert(*t, synthesized_label(*t));
                }
            }
        }
        for (&(_, addr), name) in &code_labels {
            if addr > root && addr < end {
                labels_at.insert(addr, name.clone());
            }
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
                                .get(&(0, *t))
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
                                    .unwrap_or_else(|| synthesized_label(*t));
                                Some((entry.mnemonic, label))
                            } else {
                                None // cross-region non-root: .byte fallback
                            }
                        }
                        DecodedOperand::Imm(n) => Some((entry.mnemonic, format!("#{n}"))),
                        DecodedOperand::WriteMove { writes, moves } => {
                            Some((entry.mnemonic, write_move_operand_text(writes, moves)))
                        }
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
/// The pieces a debugger-listing row is assembled from, decoded once:
/// the instruction's byte length, its bytes as space-separated uppercase
/// hex, its mnemonic, and its operand text (empty when it takes none).
///
/// [`listing_line`] concatenates these into the single-line form; callers
/// that need them in separate columns — a DAP `disassemble` response,
/// where the client renders bytes in its own column, or the wrapped
/// listing view — take them apart instead of re-splitting a rendered
/// string (docs/dap.md (the Disassembly view)).
///
/// Precondition: as [`listing_line`], `addr` must be strictly inside
/// `code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingParts {
    pub len: u32,
    pub bytes_hex: String,
    pub mnemonic: String,
    pub operand: String,
}

pub fn listing_parts(
    syntax: &ArchSyntax,
    code: &[u8],
    addr: u32,
    resolve: &dyn Fn(u32) -> Option<String>,
) -> ListingParts {
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
                DecodedOperand::WriteMove { writes, moves } => {
                    write_move_operand_text(&writes, &moves)
                }
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
    ListingParts {
        len,
        bytes_hex,
        mnemonic: mnemonic.to_string(),
        operand,
    }
}

pub fn listing_line(
    syntax: &ArchSyntax,
    code: &[u8],
    addr: u32,
    resolve: &dyn Fn(u32) -> Option<String>,
) -> (String, u32) {
    let p = listing_parts(syntax, code, addr, resolve);
    let line = format!(
        "  {addr:04x}:  {:<15} {:<8}{}",
        p.bytes_hex, p.mnemonic, p.operand
    );
    (line.trim_end().to_string(), p.len)
}

/// Bytes per row in the listing's byte column.
const BYTES_PER_ROW: usize = 5;

/// Width of the listing's operand column. Sized so the widest legal
/// vector — sixteen tapes, TM-1's ceiling — never has to break: it
/// renders 49 characters.
const OPERAND_LANE: usize = 50;

/// The operand column's rows. An operand that fits its lane is never
/// broken. One that does not breaks at the seam BETWEEN bracketed
/// vectors first — "what is written" and "where the heads move" each get
/// a row — and only inside a vector if a single one still cannot fit.
///
/// [`OPERAND_LANE`] is sized so that the widest legal vector never has
/// to: sixteen tapes, TM-1's ceiling, render 49 characters.
fn operand_lanes(operand: &str) -> Vec<String> {
    if operand.len() <= OPERAND_LANE {
        return vec![operand.to_string()];
    }
    let groups = if operand.contains('[') {
        split_at_seams(operand)
    } else {
        vec![operand.to_string()]
    };
    groups.into_iter().flat_map(pack_elements).collect()
}

/// Split on the commas BETWEEN bracketed groups, never the ones inside.
fn split_at_seams(operand: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for ch in operand.chars() {
        cur.push(ch);
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => {}
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Last resort: fill lanes element by element, breaking only after a
/// comma so an element is never split down the middle.
fn pack_elements(group: String) -> Vec<String> {
    if group.len() <= OPERAND_LANE {
        return vec![group];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for tok in group.split_inclusive(',') {
        if !cur.is_empty() && cur.trim_end().len() + tok.trim_end().len() > OPERAND_LANE {
            out.push(cur.trim_end().to_string());
            cur = tok.trim_start().to_string();
        } else {
            cur.push_str(tok);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim_end().to_string());
    }
    out
}

/// One instruction's rows in the debugger listing. The byte column wraps
/// at [`BYTES_PER_ROW`] onto continuation lines rather than growing, so
/// the mnemonic keeps its column however wide the instruction is —
/// TM-1's fused `wrmv` overruns the column at three tapes already, and
/// grows two bytes per tape from there.
fn listing_rows(addr: u32, p: &ListingParts) -> Vec<String> {
    let bytes: Vec<&str> = p.bytes_hex.split(' ').collect();
    let byte_lanes: Vec<String> = bytes.chunks(BYTES_PER_ROW).map(|c| c.join(" ")).collect();
    let op_lanes = operand_lanes(&p.operand);

    (0..byte_lanes.len().max(op_lanes.len()))
        .map(|i| {
            let b = byte_lanes.get(i).map(String::as_str).unwrap_or("");
            let o = op_lanes.get(i).map(String::as_str).unwrap_or("");
            let row = if i == 0 {
                format!("  {addr:04x}:  {b:<15} {:<8}{o}", p.mnemonic)
            } else {
                format!("         {b:<15} {:<8}{o}", "")
            };
            row.trim_end().to_string()
        })
        .collect()
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
        let parts = listing_parts(syntax, code, addr, &name_at);
        for row in listing_rows(addr, &parts) {
            out.push_str(&row);
            out.push('\n');
        }
        addr += parts.len;
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
    /// flow), `jmp` is an unconditional jump (the only non-call target
    /// producer, so a fixture can put a jump target and a dispatch target
    /// at one address), plus nop/stp/ent.
    fn fake_syntax() -> ArchSyntax {
        use crate::asm::AsmCaps;
        use crate::vm::OperandKind;
        use Flow::{Call, FallThrough as FT, Jump, Stop};
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
                    opcode: 0x22,
                    mnemonic: "jmp",
                    operand: OperandKind::RelI32,
                    flow: Jump,
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
                    opcode: 0x19,
                    mnemonic: "vwrmv",
                    operand: OperandKind::WriteMoveVec,
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
                volatile: false,
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
            volatile: false,
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
            volatile: false,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        // And the code section DEFINES the two names the table printed,
        // which is what makes the text reassemble (see
        // `dis_output_assembles_with_a_debug_map` for the round trip).
        assert!(dis.contains("\nA:      nop"), "{dis}");
        assert!(dis.contains("\nB:      stp"), "{dis}");
    }

    // ------------------------------------------------------------------
    // Assemblable disassembly: `.targets`/`.exits` print label names and
    // the code section defines every one of them
    // ------------------------------------------------------------------

    /// The fixture both round-trip tests use: a match table, a dispatch
    /// table, and a frame descriptor whose exits are the SAME two code
    /// positions the dispatch table names. Neither position is a jump
    /// target, so nothing but the table-naming path can define `A:`/`B:`
    /// — with that path absent, the labels are missing and reassembly
    /// fails.
    const TABLES_FIXTURE: &str = "\
.section tables
T0: .row [1]
    .row [*]
D0: .targets A, B
F0: .frame tapes=(1, 0)
    .map 0, rmap=(1->2, 3->4)
    .exits A, B
.section code
.func main
    tmatch T0
    tdispatch D0
    fcall helper, F0
A:  nop
B:  stp
.func helper
    stp
";

    /// Every operand name printed by a `.targets` or `.exits` line in
    /// `text`, continuation lines included — a wrapped list's lines all
    /// end in `,` except its last, which is exactly how a continuation is
    /// recognized here.
    fn listed_names(text: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut continuing = false;
        for line in text.lines() {
            let trimmed = line.trim();
            let payload = if continuing {
                trimmed
            } else if let Some((_, rest)) = trimmed.split_once(".targets ") {
                rest
            } else if let Some((_, rest)) = trimmed.split_once(".exits ") {
                rest
            } else {
                continue;
            };
            continuing = payload.ends_with(',');
            names.extend(
                payload
                    .split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string),
            );
        }
        names
    }

    /// The invariant the whole naming mechanism exists to hold: every
    /// name a `.targets`/`.exits` line prints is defined as a label in
    /// the CODE section of the same text. Independent of how the name was
    /// chosen, so it catches a debug name with no definition, a raw
    /// offset (which defines nothing and is not even a name), and a
    /// position the code walk never reaches.
    fn assert_listed_names_are_defined(dis: &str) {
        let (_, code) = dis
            .split_once("\n.section code\n")
            .unwrap_or_else(|| panic!("table-bearing disassembly has a code section:\n{dis}"));
        let listed = listed_names(dis);
        assert!(!listed.is_empty(), "fixture lists no names at all:\n{dis}");
        for name in listed {
            let def = format!("{name}:");
            assert!(
                code.lines().any(|l| l.starts_with(&def)),
                "`{name}` is printed by a list directive but defined nowhere \
                 in the code section:\n{dis}"
            );
        }
    }

    #[test]
    fn dis_output_assembles_without_a_debug_map() {
        let syntax = fake_syntax();
        let obj = assemble(&syntax, 0x7E, TABLES_FIXTURE, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        // With no debug info the names are synthesized from the target
        // addresses — the same `L<addr>` shape a jump target gets.
        assert!(dis.contains(".targets L"), "synthesized names:\n{dis}");
        assert_listed_names_are_defined(&dis);
        // The full round trip: the rendered text reassembles, and to the
        // very same object.
        let obj2 = assemble(&syntax, 0x7E, &dis, false).expect("disassembly reassembles");
        assert_eq!(obj2, obj, "dis ∘ asm is a fixpoint:\n{dis}");
    }

    #[test]
    fn dis_output_assembles_with_a_debug_map() {
        let syntax = fake_syntax();
        let obj = assemble(&syntax, 0x7E, TABLES_FIXTURE, true).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        // A `-g` object replays its own source labels rather than
        // synthesizing addresses — and defines them.
        assert!(dis.contains(".targets A, B"), "debug names:\n{dis}");
        assert!(dis.contains(".exits  A, B"), "debug names:\n{dis}");
        assert_listed_names_are_defined(&dis);
        // Reassembling (without `-g`) must produce the same object the
        // stripped source does: the debug names resolved to the same
        // addresses the offsets held.
        let stripped = assemble(&syntax, 0x7E, TABLES_FIXTURE, false).unwrap();
        let obj2 = assemble(&syntax, 0x7E, &dis, false).expect("disassembly reassembles");
        assert_eq!(obj2, stripped, "dis ∘ asm is a fixpoint:\n{dis}");
    }

    #[test]
    fn a_dispatch_target_that_is_also_a_jump_target_gets_one_name() {
        // Both naming paths want to name address `A`: the jump-target
        // synthesizer and the tables section. They must agree, or the
        // `.targets` line names something the code section never defines.
        let syntax = fake_syntax();
        let src = "\
.section tables
D0: .targets A, B
.section code
.func main
    tdispatch D0
A:  nop
    jmp A
B:  stp
";
        let obj = assemble(&syntax, 0x7E, src, true).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        // The debug name wins, and the jump operand follows it — there is
        // no second, synthesized name for the same address.
        assert!(dis.contains("\nA:      nop"), "{dis}");
        assert!(dis.contains("jmp     A"), "{dis}");
        assert!(
            !dis.contains("L0001"),
            "the address-derived name must not survive alongside `A`:\n{dis}"
        );
        assert_listed_names_are_defined(&dis);
        let obj2 = assemble(&syntax, 0x7E, &dis, false).expect("disassembly reassembles");
        assert_eq!(obj2, assemble(&syntax, 0x7E, src, false).unwrap());
    }

    #[test]
    fn dis_separates_tables_with_blank_lines() {
        // A blank line before every table but the FIRST — what a person
        // writing assembly does, and what stops the whole section from
        // being one alignment group.
        let syntax = fake_syntax();
        let src = "\
.section tables
T0: .row [1]
T1: .row [2]
D0: .targets A, B
.section code
.func main
    tmatch T0
    tmatch T1
    tdispatch D0
A:  nop
B:  stp
";
        let obj = assemble(&syntax, 0x7E, src, true).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        // The first table opens the section with no blank line above it.
        assert!(
            dis.starts_with(".section tables\nT0:"),
            "no blank line before the first table:\n{dis}"
        );
        // Each later table is preceded by exactly one.
        assert!(dis.contains("\n\nT1:"), "blank line before T1:\n{dis}");
        assert!(dis.contains("\n\nT2:"), "blank line before T2:\n{dis}");
        // …and nowhere else in the section: three tables, two boundaries.
        // `str::matches` is non-overlapping, so a run of two blank lines
        // ("\n\n\n") still counts as one "\n\n" match — the count alone
        // can't tell one blank line from two. The `contains` check below
        // rules out that wider run directly.
        let section = dis.split_once("\n.section code\n").unwrap().0;
        assert_eq!(
            section.matches("\n\n").count(),
            2,
            "one blank line per boundary, no others:\n{dis}"
        );
        assert!(
            !section.contains("\n\n\n"),
            "no boundary carries more than one blank line:\n{dis}"
        );
    }

    #[test]
    fn wide_lists_are_emitted_wrapped_the_way_the_printer_wraps_them() {
        // `.targets`, `.exits` and `.map` all outgrow the line budget.
        // The disassembler wraps them itself, at the same places the
        // printer would — so its output needs no formatting pass, and a
        // formatting pass changes nothing.
        use crate::asm::{AsmCaps, format_asm_with};
        let syntax = fake_syntax();
        let exits: Vec<String> = (0..14).map(|i| format!("Elongname{i}")).collect();
        // Each `rmap=(…)`/`wmap=(…)` clause fits the budget on its own —
        // the `.map` list breaks BETWEEN clauses, never inside one, so a
        // clause wider than the budget would print over-budget by design
        // and say nothing about the wrap.
        let pairs: Vec<String> = (1..9).map(|i| format!("{i}->{}", i + 1)).collect();
        let body: String = exits
            .iter()
            .map(|e| format!("{e}: nop\n"))
            .collect::<Vec<_>>()
            .concat();
        let src = format!(
            ".section tables\n\
             D0: .targets {}\n\
             F0: .frame tapes=(1, 0)\n    \
                 .map 0, rmap=({}), wmap=({})\n    \
                 .exits {}\n\
             .section code\n\
             .func main\n    \
                 tdispatch D0\n    \
                 fcall helper, F0\n\
             {body}    stp\n\
             .func helper\n    stp\n",
            exits.join(", "),
            pairs.join(", "),
            pairs.join(", "),
            exits.join(", "),
        );
        let obj = assemble(&syntax, 0x7E, &src, true).unwrap();
        let dis = disassemble_object(&syntax, &obj);

        // Each of the three actually wrapped — otherwise this test would
        // pass on output that never reached the budget.
        for (word, col) in [(".targets", 17), (".exits", 16), (".map", 16)] {
            let line = dis
                .lines()
                .position(|l| l.contains(word))
                .unwrap_or_else(|| panic!("no {word} line in:\n{dis}"));
            let next = dis.lines().nth(line + 1).unwrap_or("");
            assert!(
                dis.lines().nth(line).unwrap().ends_with(','),
                "{word} should have wrapped:\n{dis}"
            );
            assert_eq!(
                next.len() - next.trim_start().len(),
                col,
                "{word}'s continuation lines indent under its first element:\n{dis}"
            );
        }
        // No code line runs past the budget the wrap exists to respect.
        for l in dis.lines() {
            assert!(l.chars().count() <= 80, "over-budget line `{l}`:\n{dis}");
        }
        // The printer is the identity on it, and it reassembles.
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
            volatile: false,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        assert_listed_names_are_defined(&dis);
        let stripped = assemble(&syntax, 0x7E, &src, false).unwrap();
        assert_eq!(
            assemble(&syntax, 0x7E, &dis, false).expect("disassembly reassembles"),
            stripped
        );
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
            volatile: false,
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
            volatile: false,
        };
        assert_eq!(format_asm_with(&dis, caps).unwrap(), dis);
        assert_eq!(assemble(&syntax, 0x7E, &dis, false).unwrap(), obj);
    }

    #[test]
    fn write_move_vector_renders_two_groups_and_round_trips() {
        use crate::asm::{AsmCaps, format_asm_with};
        // A fused `wrmv` renders both groups `[w…], [m…]` — the write group
        // with `-` for keep, the move group with the move glyphs — and
        // assemble ∘ dis ∘ assemble is a byte fixpoint.
        let syntax = fake_syntax();
        let src = "\
.func main
        vwrmv   [1, -, 2], [<, ., >]
        stp
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let dis = disassemble_object(&syntax, &obj);
        assert_eq!(dis, src, "wrmv disassembly:\n{dis}");
        let caps = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
            volatile: false,
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
            volatile: false,
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
            volatile: false,
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
            volatile: false,
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
            volatile: false,
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
            volatile: false,
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
    fn jump_only_callee_stays_a_root_so_its_site_keeps_symbol_form() {
        // `g` is reached ONLY by main's tail jump — never called, never the
        // entry — so nothing but the jump itself marks it as a function.
        // The boundary at `g` is a genuine cut (main ends in the jump, and
        // the jump renders symbolically), so `g` stays a root and the site
        // keeps the symbol form whose width belongs to the linker; folded
        // into main's region it would become a local label, whose width
        // belongs to the assembler.
        let syntax = test_syntax();
        let src = ".func main\n        jmp @g\n.func g\n        stop\n";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        // Relaxation disabled: main = [ent][jmp<i32>] = 6 bytes, so g sits
        // at 6; the far jump ends at 6 too, displacement 6 − 6 = 0.
        let opts = crate::linker::LinkOptions {
            relax: false,
            ..Default::default()
        };
        let out = crate::linker::link(&syntax, &[obj], &[], opts.clone()).unwrap();
        assert_eq!(
            out.executable.code,
            vec![0x0E, 0x20, 0, 0, 0, 0, 0x0E, 0x02]
        );
        let text = disassemble_executable(&syntax, &out.executable, None);
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        let out2 = crate::linker::link(&syntax, &[obj2], &[], opts).unwrap();
        assert_eq!(out2.executable.code, out.executable.code, "{text}");
        assert!(text.contains(".func func_0006"), "{text}");
        assert!(text.contains("jmp     @func_0006"), "{text}");
    }

    #[test]
    fn an_entry_byte_in_a_body_that_cuts_cleanly_still_reads_as_a_function_start() {
        // What the cut filter deliberately does NOT rescue, pinned so the
        // residue stays visible: here the entry byte stands inside a body,
        // but the boundary it opens is a genuine cut — nothing falls into
        // it, no local edge spans it — so it is indistinguishable from a
        // function start and is promoted. The text describes the same
        // program either way (the halves are independent, so re-ordering
        // them is harmless); only the bytes can differ, because the site's
        // width authority moves from the assembler to the linker.
        //
        // The ambiguity is not removable: a pure image records no function
        // boundaries at all. What the filter does remove is every case
        // where splitting would change the program or produce text that
        // will not link, which is the class worth paying for.
        let syntax = test_syntax();
        let src = ".func main\n        jmp L1\nL1:     ent\n        stop\n";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let out = crate::linker::link(&syntax, &[obj], &[], crate::linker::LinkOptions::default())
            .unwrap();
        let text = disassemble_executable(&syntax, &out.executable, None);
        assert!(text.contains(".func func_0003"), "{text}");
        assert!(text.contains("jmp     @func_0003"), "{text}");
    }

    #[test]
    fn a_body_entry_byte_that_does_not_cut_is_left_folded() {
        // The three ways a boundary fails to be a cut. Images are built
        // here rather than assembled because the fixture's `br` is the
        // unpaired-RelI8 traversal opcode, which assembler tests may not
        // use; the rendering decision is what these pin, and the round-trip
        // law over the same three shapes is pinned against a real dialect
        // in the post-machine crate's link tests.
        let syntax = test_syntax();
        for (name, code) in [
            // Fall-through into the entry byte at 6 (`nop` at 5 runs into
            // it): splitting here would leave the first half running off
            // its end into whatever the linker ordered next.
            (
                "fall-through",
                vec![0x0E, 0x22, 0x02, 0x30, 0x01, 0x01, 0x0E, 0x02],
            ),
            // A branch below the entry byte at 5 targeting 7, above it:
            // after a split the target is outside its region and can only
            // be spelled as raw bytes.
            (
                "forward edge",
                vec![0x0E, 0x22, 0x04, 0x30, 0x00, 0x0E, 0x01, 0x01, 0x02],
            ),
            // …and the mirror image: a branch above the entry byte at 4
            // targeting 1, below it.
            (
                "backward edge",
                vec![0x0E, 0x01, 0x30, 0x00, 0x0E, 0x22, 0xFA, 0x02],
            ),
        ] {
            let exe = Executable::code_only(0x7E, 0, code);
            let text = disassemble_executable(&syntax, &exe, None);
            assert_eq!(text.matches(".func").count(), 1, "{name}:\n{text}");
            assert!(!text.contains('@'), "{name}: no invented root:\n{text}");
            assert!(
                text.contains("ent"),
                "{name}: the fold keeps the byte inline:\n{text}"
            );
        }
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
                source: None,
            }],
            bindings: vec![],
        };
        let text_with_map = disassemble_executable(&syntax, &exe, Some(&map));
        assert!(text_with_map.contains(".func helper"), "{text_with_map}");
        assert!(text_with_map.contains("call    helper"), "{text_with_map}");
        assert!(!text_with_map.contains("func_0007"), "{text_with_map}");
    }

    /// The core crate cannot depend on PM-1: a minimal local `ArchSyntax`
    /// with exactly the entries the derived golden uses (docs/core.md (the
    /// assembler framework)),
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
                // A two-vector operand, for the listing view's seam-first
                // operand wrapping. Shaped like TM-1's fused `wrmv`.
                SyntaxEntry {
                    opcode: 0x12,
                    mnemonic: "wrmv",
                    operand: OperandKind::WriteMoveVec,
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
                source: None,
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

    /// `listing_parts` hands back the four pieces `listing_line`
    /// concatenates, so a caller that needs them in separate columns — a
    /// DAP `DisassembledInstruction`, or the wrapped listing view — does
    /// not have to re-split a rendered string.
    /// An instruction whose bytes overrun the byte column wraps them
    /// onto continuation lines instead of shoving the mnemonic right.
    /// TM-1's fused `wrmv` overruns at three tapes already, so this is
    /// the ordinary case for a multi-tape listing, not an exotic one.
    /// Instructions that fit are untouched.
    /// An operand too wide for its lane breaks at the seam BETWEEN the
    /// bracketed vectors, so "what is written" and "where the heads move"
    /// each get their own row. It breaks inside a vector only when a
    /// single vector cannot fit — and the lane is sized so that the
    /// widest legal one (sixteen tapes) never has to.
    /// `listing_line` never wraps, however wide the instruction. This is
    /// a regression pin rather than a red-green cycle — it passes on
    /// arrival, and it exists because a traced run prints exactly one row
    /// per retired instruction and renders that row through this
    /// function — never through `listing_executable`. Wrapping added here
    /// rather than in the executable-listing path would therefore grow a
    /// traced run's rows without any test noticing.
    #[test]
    fn listing_line_stays_one_row_however_wide_the_instruction() {
        let mut code = vec![0x12];
        code.extend(std::iter::repeat_n(0x01u8, 11));
        code.push(0x81);
        code.extend(std::iter::repeat_n(0x02u8, 11));
        code.push(0x82);
        let (line, len) = listing_line(&pm1_like_syntax(), &code, 0, &|_| None);
        assert_eq!(len, 25);
        assert!(!line.contains('\n'), "one row only, got: {line:?}");
    }

    #[test]
    fn listing_executable_breaks_a_wide_operand_at_the_vector_seam() {
        let mut code = vec![0x12];
        code.extend(std::iter::repeat_n(0x01u8, 11));
        code.push(0x81);
        code.extend(std::iter::repeat_n(0x02u8, 11));
        code.push(0x82);
        let exe = Executable::code_only(0x01, 0, code);
        let listing = listing_executable(&pm1_like_syntax(), &exe, None);
        let expected = concat!(
            "  0000:  12 01 01 01 01  wrmv    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],\n",
            "         01 01 01 01 01          [>, >, >, >, >, >, >, >, >, >, >, >]\n",
            "         01 01 81 02 02\n",
            "         02 02 02 02 02\n",
            "         02 02 02 02 82\n",
        );
        assert_eq!(listing, expected);
    }

    #[test]
    fn listing_executable_wraps_wide_byte_runs_and_holds_the_mnemonic_column() {
        // 0..8: wr with eight symbols (nine bytes) | 9: stp
        let exe = Executable::code_only(
            0x01,
            0,
            vec![0x06, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x81, 0x02],
        );
        let listing = listing_executable(&pm1_like_syntax(), &exe, None);
        // Written line by line: a `\`-continued literal would strip the
        // leading spaces this layout is entirely about.
        let expected = concat!(
            "  0000:  06 01 01 01 01  wr      1, 1, 1, 1, 1, 1, 1, 1\n",
            "         01 01 01 81\n",
            "  0009:  02              stp\n",
        );
        assert_eq!(listing, expected);
    }

    #[test]
    fn listing_parts_splits_what_listing_line_concatenates() {
        let syntax = pm1_like_syntax();
        let code = [0x06, 0x01, 0x82];
        let parts = listing_parts(&syntax, &code, 0, &|_| None);

        assert_eq!(parts.len, 3);
        assert_eq!(parts.bytes_hex, "06 01 82");
        assert_eq!(parts.mnemonic, "wr");
        assert_eq!(parts.operand, "1, 2");

        // The one-line rendering is exactly these pieces in the old shape.
        let (line, len) = listing_line(&syntax, &code, 0, &|_| None);
        assert_eq!(len, parts.len);
        assert_eq!(
            line,
            format!(
                "  0000:  {:<15} {:<8}{}",
                parts.bytes_hex, parts.mnemonic, parts.operand
            )
            .trim_end()
        );
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
