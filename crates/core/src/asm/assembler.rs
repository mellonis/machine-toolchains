//! Two-pass assembly with short/far relaxation (docs/formats.md (assembly
//! text); docs/isa.md for the opcode/relaxation table this assembles
//! against).

use std::collections::{BTreeSet, HashMap};

use super::lower::{
    SourceFunction, SourceItem, SourceOperand, SourceRow, SourceTable, SpannedName, VecElem,
    lower_source,
};
use super::syntax::{ArchSyntax, Flow};
use super::{AsmError, AsmErrorKind};
use crate::diagnostics::Span;
use crate::formats::object::{BlobDebug, ObjectFile, Relocation, Symbol, SymbolDef, TableFixup};
use crate::vm::{Operand, OperandKind, encode_operand};

fn err(span: Span, kind: AsmErrorKind) -> AsmError {
    AsmError { span, kind }
}

/// One instruction after operand classification, before layout. Each
/// slot carries the spans a later-phase diagnostic points at: the item
/// span for the debug line, and — where a symbol/label name is the
/// subject of a possible error — that name's own span.
enum Slot {
    Fixed {
        span: Span,
        bytes: Vec<u8>,
    },
    Jump {
        span: Span,
        /// The jump target name's span (UnknownLabel / short-offset).
        target_span: Span,
        far: u8,
        short: Option<u8>,
        forced_short: bool,
        target: String,
    },
    /// A symbol site — call or `jmp @name`: far opcode + 4-byte hole +
    /// relocation. `symbol_span` doubles as the debug-line source (the
    /// operand shares the instruction's line).
    Call {
        symbol_span: Span,
        opcode: u8,
        symbol: String,
    },
    /// A table reference (`tmatch T0`-style): opcode + 4-byte hole. The
    /// hole is filled with the table's offset in this blob's OWN table
    /// blob and recorded as a `TableFixup` for the linker's rebase.
    TableRef {
        span: Span,
        name_span: Span,
        opcode: u8,
        name: String,
    },
}

impl Slot {
    fn size(&self, is_far: bool) -> u32 {
        match self {
            Slot::Fixed { bytes, .. } => bytes.len() as u32,
            Slot::Jump { .. } => {
                if is_far {
                    5
                } else {
                    2
                }
            }
            Slot::Call { .. } | Slot::TableRef { .. } => 5,
        }
    }
}

/// A `TableRef` operand hole recorded during emit: the 4-byte hole at
/// `offset` within its blob, and the table label it references.
struct TableRefHole {
    name: String,
    name_span: Span,
    offset: u32,
}

/// One assembled function: the code blob, its relocations, the
/// always-computed debug info (stored in the object only under
/// `with_debug`; its labels double as the dispatch-target map), and the
/// TableRef holes awaiting [`build_tables`].
struct AssembledFunction {
    blob: Vec<u8>,
    relocations: Vec<Relocation>,
    debug: BlobDebug,
    table_refs: Vec<TableRefHole>,
}

pub fn assemble(
    syntax: &ArchSyntax,
    arch_id: u8,
    source: &str,
    with_debug: bool,
) -> Result<ObjectFile, AsmError> {
    // Parse under the dialect's caps: table dialects shape sections and
    // directives; with default caps this is byte-identical to before.
    let lowered = lower_source(
        &super::cst::parse_asm_cst_with(source, syntax.caps),
        syntax,
        source,
    )?;
    let functions = &lowered.functions;

    let mut symbols: Vec<Symbol> = functions
        .iter()
        .enumerate()
        .map(|(i, f)| Symbol {
            name: f.name.clone(),
            def: if f.local {
                SymbolDef::Local { blob: i as u32 }
            } else {
                SymbolDef::Defined { blob: i as u32 }
            },
        })
        .collect();
    let mut symbol_index: HashMap<String, u32> = symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.clone(), i as u32))
        .collect();

    let mut blobs = Vec::with_capacity(functions.len());
    let mut relocations = Vec::new();
    let mut debug = with_debug.then(Vec::new);
    // Per-blob label offsets and TableRef holes, for table building.
    let mut blob_labels: Vec<Vec<(String, u32)>> = Vec::with_capacity(functions.len());
    let mut blob_table_refs: Vec<Vec<TableRefHole>> = Vec::with_capacity(functions.len());

    for (blob_idx, function) in functions.iter().enumerate() {
        let assembled = assemble_function(
            syntax,
            function,
            blob_idx as u32,
            &mut symbols,
            &mut symbol_index,
        )?;
        blobs.push(assembled.blob);
        relocations.extend(assembled.relocations);
        blob_labels.push(assembled.debug.labels.clone());
        blob_table_refs.push(assembled.table_refs);
        if let Some(d) = debug.as_mut() {
            d.push(assembled.debug);
        }
    }

    let mut object = ObjectFile::v2(arch_id, symbols, blobs, relocations, debug);
    // Table building runs when any table exists (it populates the v3
    // fields) or any TableRef hole exists (a reference with no tables at
    // all must still report its unknown label). Without either, the
    // object keeps its v2 shape byte-for-byte.
    let any_refs = blob_table_refs.iter().any(|refs| !refs.is_empty());
    if !lowered.tables.is_empty() || any_refs {
        let (table_blobs, table_fixups) = build_tables(
            &lowered.tables,
            &blob_table_refs,
            &blob_labels,
            &mut object.blobs,
        )?;
        object.table_blobs = Some(table_blobs);
        object.table_fixups = table_fixups;
    }
    Ok(object)
}

fn assemble_function(
    syntax: &ArchSyntax,
    function: &SourceFunction,
    blob_idx: u32,
    symbols: &mut Vec<Symbol>,
    symbol_index: &mut HashMap<String, u32>,
) -> Result<AssembledFunction, AsmError> {
    // Pass A: classify items into slots; collect label → slot-index.
    let mut slots: Vec<Slot> = Vec::new();
    let mut label_slot: HashMap<String, usize> = HashMap::new();
    for item in &function.items {
        let (span, labels) = match item {
            SourceItem::Instr { span, labels, .. } | SourceItem::RawByte { span, labels, .. } => {
                (*span, labels)
            }
        };
        for label in labels {
            if label_slot.insert(label.name.clone(), slots.len()).is_some() {
                return Err(err(
                    label.span,
                    AsmErrorKind::DuplicateLabel(label.name.clone()),
                ));
            }
        }
        match item {
            SourceItem::RawByte { value, .. } => {
                slots.push(Slot::Fixed {
                    span,
                    bytes: vec![*value],
                });
            }
            SourceItem::Instr {
                opcode, operand, ..
            } => {
                let entry = syntax
                    .by_opcode(*opcode)
                    .expect("lowering guarantees known opcode");
                match (&entry.operand, operand) {
                    (OperandKind::None, SourceOperand::None) => {
                        slots.push(Slot::Fixed {
                            span,
                            bytes: vec![*opcode],
                        });
                    }
                    (OperandKind::TableRef, SourceOperand::Name(name)) => {
                        slots.push(Slot::TableRef {
                            span,
                            name_span: name.span,
                            opcode: *opcode,
                            name: name.name.clone(),
                        });
                    }
                    (OperandKind::SymbolVec, SourceOperand::Ints(ints)) => {
                        let sym: Result<Vec<u32>, _> = ints
                            .iter()
                            .map(|&i| u32::try_from(i).map_err(|_| "negative symbol index"))
                            .collect();
                        let encoded = sym
                            .and_then(|s| encode_operand(&Operand::Symbols(s)))
                            .map_err(|m| err(span, AsmErrorKind::EncodeError(m)))?;
                        let mut bytes = vec![*opcode];
                        bytes.extend(encoded);
                        slots.push(Slot::Fixed { span, bytes });
                    }
                    // Bracket-form vectors, routed by OperandKind: the
                    // assembler is arch-agnostic, so element vocabulary
                    // is a property of the operand kind, never of a
                    // mnemonic (docs/formats.md (assembly text)).
                    (OperandKind::SymbolVec, SourceOperand::Vector(elems, vspan)) => {
                        slots.push(vector_slot(
                            *opcode,
                            span,
                            *vspan,
                            elems,
                            write_vector_element,
                        )?);
                    }
                    (OperandKind::MoveVec, SourceOperand::Vector(elems, vspan)) => {
                        slots.push(vector_slot(
                            *opcode,
                            span,
                            *vspan,
                            elems,
                            move_vector_element,
                        )?);
                    }
                    (OperandKind::RelI8 | OperandKind::RelI32, SourceOperand::SymbolName(name)) => {
                        match entry.flow {
                            Flow::Call => {
                                return Err(err(
                                    span,
                                    AsmErrorKind::BadOperand(
                                        "call operands are already symbols; drop the `@`",
                                    ),
                                ));
                            }
                            Flow::Jump => {
                                if entry.operand == OperandKind::RelI8 {
                                    return Err(err(
                                        span,
                                        AsmErrorKind::BadOperand(
                                            "jmp.s width is linker-selected; write jmp @name",
                                        ),
                                    ));
                                }
                                slots.push(Slot::Call {
                                    symbol_span: name.span,
                                    opcode: *opcode,
                                    symbol: name.name.clone(),
                                });
                            }
                            _ => {
                                return Err(err(
                                    span,
                                    AsmErrorKind::BadOperand(
                                        "conditional jumps take labels, not symbols",
                                    ),
                                ));
                            }
                        }
                    }
                    (OperandKind::RelI8 | OperandKind::RelI32, SourceOperand::Name(name)) => {
                        if syntax.is_call(*opcode) {
                            if entry.operand == OperandKind::RelI8 {
                                return Err(err(
                                    span,
                                    AsmErrorKind::BadOperand(
                                        "call.s width is linker-selected; write call",
                                    ),
                                ));
                            }
                            slots.push(Slot::Call {
                                symbol_span: name.span,
                                opcode: *opcode,
                                symbol: name.name.clone(),
                            });
                        } else {
                            // Is this the short half of a pair (forced) or a
                            // bare far mnemonic (relaxable) or unpaired?
                            let far_of_short =
                                syntax.relax_pairs.iter().find(|p| p.short == *opcode);
                            if let Some(pair) = far_of_short {
                                slots.push(Slot::Jump {
                                    span,
                                    target_span: name.span,
                                    far: pair.far,
                                    short: Some(*opcode),
                                    forced_short: true,
                                    target: name.name.clone(),
                                });
                            } else {
                                slots.push(Slot::Jump {
                                    span,
                                    target_span: name.span,
                                    far: *opcode,
                                    short: syntax.short_of(*opcode),
                                    forced_short: false,
                                    target: name.name.clone(),
                                });
                            }
                        }
                    }
                    _ => {
                        return Err(err(span, AsmErrorKind::BadOperand("operand kind mismatch")));
                    }
                }
            }
        }
    }

    // Pass B: relaxation fixpoint. is_far[i] applies to Jump slots only.
    // Start short wherever a short form exists (start-short-and-grow).
    let mut is_far: Vec<bool> = slots
        .iter()
        .map(|s| match s {
            Slot::Jump { short, .. } => short.is_none(),
            _ => true, // unused for non-jumps
        })
        .collect();
    loop {
        // Layout: addresses per slot (blob starts with the implicit ent byte).
        let mut addr = 1u32;
        let mut starts = Vec::with_capacity(slots.len());
        for (i, slot) in slots.iter().enumerate() {
            starts.push(addr);
            addr += slot.size(is_far[i]);
        }
        let resolve = |name: &str, span: Span| -> Result<u32, AsmError> {
            label_slot
                .get(name)
                .map(|&i| starts[i])
                .ok_or_else(|| err(span, AsmErrorKind::UnknownLabel(name.to_string())))
        };

        let mut grew = false;
        for (i, slot) in slots.iter().enumerate() {
            if let Slot::Jump {
                target,
                target_span,
                forced_short,
                ..
            } = slot
            {
                if is_far[i] {
                    continue;
                }
                let end = starts[i] + slot.size(false);
                let target_addr = resolve(target, *target_span)?;
                let off = i64::from(target_addr) - i64::from(end);
                if i8::try_from(off).is_err() {
                    if *forced_short {
                        return Err(err(
                            *target_span,
                            AsmErrorKind::ShortOffsetOutOfRange {
                                target: target.clone(),
                            },
                        ));
                    }
                    is_far[i] = true;
                    grew = true;
                }
            }
        }
        if !grew {
            // Final emit.
            let mut blob = vec![syntax.entry_opcode];
            let mut relocs = Vec::new();
            let mut lines = Vec::new();
            let mut table_refs = Vec::new();
            for (i, slot) in slots.iter().enumerate() {
                // The MO debug section stores a source line (docs/formats.md
                // (MO)); an operand shares its instruction's line, so a
                // Call reads its symbol_span's line.
                lines.push((
                    starts[i],
                    match slot {
                        Slot::Fixed { span, .. }
                        | Slot::Jump { span, .. }
                        | Slot::TableRef { span, .. } => span.start.line,
                        Slot::Call { symbol_span, .. } => symbol_span.start.line,
                    },
                ));
                match slot {
                    Slot::Fixed { bytes, .. } => blob.extend_from_slice(bytes),
                    Slot::Jump {
                        target_span,
                        far,
                        short,
                        target,
                        ..
                    } => {
                        let end = starts[i] + slot.size(is_far[i]);
                        let off = i64::from(resolve(target, *target_span)?) - i64::from(end);
                        if is_far[i] {
                            blob.push(*far);
                            blob.extend((off as i32).to_le_bytes());
                        } else {
                            blob.push(short.expect("short exists when !is_far"));
                            blob.push((off as i8) as u8);
                        }
                    }
                    Slot::Call { opcode, symbol, .. } => {
                        blob.push(*opcode);
                        let sym_idx = *symbol_index.entry(symbol.clone()).or_insert_with(|| {
                            symbols.push(Symbol {
                                name: symbol.clone(),
                                def: SymbolDef::External,
                            });
                            (symbols.len() - 1) as u32
                        });
                        relocs.push(Relocation {
                            blob: blob_idx,
                            offset: (blob.len()) as u32,
                            symbol: sym_idx,
                        });
                        blob.extend([0u8; 4]);
                    }
                    Slot::TableRef {
                        opcode,
                        name,
                        name_span,
                        ..
                    } => {
                        blob.push(*opcode);
                        // A zeroed hole for now; `build_tables` patches it
                        // with the table's blob-local offset once every
                        // table's placement is known.
                        table_refs.push(TableRefHole {
                            name: name.clone(),
                            name_span: *name_span,
                            offset: blob.len() as u32,
                        });
                        blob.extend([0u8; 4]);
                    }
                }
            }
            let labels = label_slot
                .iter()
                .map(|(name, &i)| (name.clone(), starts[i]))
                .collect::<Vec<_>>();
            let mut labels = labels;
            labels.sort_by(|a, b| (a.1, a.0.as_str()).cmp(&(b.1, b.0.as_str())));
            return Ok(AssembledFunction {
                blob,
                relocations: relocs,
                debug: BlobDebug { labels, lines },
                table_refs,
            });
        }
    }
}

/// Encodes a `[..]` vector operand into a fixed slot, mapping each
/// element through the operand kind's vocabulary. An element outside
/// the vocabulary is a `BadVector` at the operand's own span
/// (`vector_span`); `span` stays the whole item's, for the slot and
/// any encode failure.
fn vector_slot(
    opcode: u8,
    span: Span,
    vector_span: Span,
    elems: &[VecElem],
    vocab: fn(&VecElem) -> Result<u32, &'static str>,
) -> Result<Slot, AsmError> {
    let mut vals = Vec::with_capacity(elems.len());
    for elem in elems {
        vals.push(vocab(elem).map_err(|m| err(vector_span, AsmErrorKind::BadVector(m)))?);
    }
    let encoded = encode_operand(&Operand::Symbols(vals))
        .map_err(|m| err(span, AsmErrorKind::EncodeError(m)))?;
    let mut bytes = vec![opcode];
    bytes.extend(encoded);
    Ok(Slot::Fixed { span, bytes })
}

/// Write-vector vocabulary (docs/formats.md (assembly text)): payloads
/// up to `0x7E` write their symbol; `-` keeps the cell (`0x7F` on the
/// wire, so `0x7F` is not writable as a payload); wildcards belong to
/// `.row` rows and moves to a move vector.
fn write_vector_element(elem: &VecElem) -> Result<u32, &'static str> {
    match elem {
        VecElem::Payload(p) if *p <= 0x7E => Ok(*p),
        VecElem::Payload(_) => Err("write payloads are at most 126"),
        VecElem::Keep => Ok(0x7F),
        VecElem::Wildcard | VecElem::MoveLeft | VecElem::MoveRight | VecElem::Stay => {
            Err("move/wildcard element in a write vector")
        }
    }
}

/// Move-vector vocabulary (docs/formats.md (assembly text)): `.` stays
/// (0), `<` steps left (1), `>` steps right (2); nothing else — moves
/// are never spelled numerically.
fn move_vector_element(elem: &VecElem) -> Result<u32, &'static str> {
    match elem {
        VecElem::Stay => Ok(0),
        VecElem::MoveLeft => Ok(1),
        VecElem::MoveRight => Ok(2),
        VecElem::Payload(_) | VecElem::Wildcard | VecElem::Keep => {
            Err("only `<`, `>`, `.` belong in a move vector")
        }
    }
}

/// Builds the per-blob table blobs and TableRef fixups (docs/formats.md
/// (MO)). Match-table bytes follow the layout the VM walks (vm/table.rs:
/// width u8, row_count u16 LE, one byte per row position, 0x7F =
/// wildcard); dispatch tables are entry_count u16 LE + u32 LE
/// BLOB-RELATIVE code offsets, resolvable only after function layout.
///
/// Attribution rule: every table belongs to exactly ONE function — the
/// one whose code references it via a TableRef operand, or whose labels
/// its dispatch entries point at. An unreferenced table, or one tied to
/// two functions, is an error (sharing belongs to the composition
/// engine, a later phase). The owner's `table_blobs` entry concatenates
/// its tables in source order; every TableRef hole is patched with its
/// table's offset in that blob's table blob and recorded as a fixup.
fn build_tables(
    tables: &[SourceTable],
    refs: &[Vec<TableRefHole>],
    labels: &[Vec<(String, u32)>],
    blobs: &mut [Vec<u8>],
) -> Result<(Vec<Vec<u8>>, Vec<TableFixup>), AsmError> {
    // 1. Match-table discipline — table-local, so it is validated before
    //    attribution: an ill-formed table is reported even when it is
    //    also unreferenced.
    for table in tables {
        if let SourceTable::Match { name, rows } = table {
            validate_match_discipline(name, rows)?;
        }
    }

    let table_index =
        |name: &str| -> Option<usize> { tables.iter().position(|t| t.name().name == name) };

    // 2. Owner candidates from TableRef operands.
    let mut owners: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); tables.len()];
    for (blob, holes) in refs.iter().enumerate() {
        for hole in holes {
            let Some(ti) = table_index(&hole.name) else {
                return Err(err(
                    hole.name_span,
                    AsmErrorKind::UnknownTableLabel(hole.name.clone()),
                ));
            };
            owners[ti].insert(blob as u32);
        }
    }

    // 3. Dispatch targets must all live in ONE function; that function
    //    joins the owner candidates. Label names are function-scoped, so
    //    one name may exist in several blobs — the running intersection
    //    across targets disambiguates.
    for (ti, table) in tables.iter().enumerate() {
        let SourceTable::Dispatch { name, targets } = table else {
            continue;
        };
        let mut candidates: Option<BTreeSet<u32>> = None;
        for target in targets {
            let blobs_with: BTreeSet<u32> = labels
                .iter()
                .enumerate()
                .filter(|(_, blob_labels)| blob_labels.iter().any(|(n, _)| n == &target.name))
                .map(|(b, _)| b as u32)
                .collect();
            let next: BTreeSet<u32> = match &candidates {
                None => blobs_with,
                Some(so_far) => so_far.intersection(&blobs_with).copied().collect(),
            };
            if next.is_empty() {
                return Err(err(
                    target.span,
                    AsmErrorKind::UnknownTableLabel(target.name.clone()),
                ));
            }
            candidates = Some(next);
        }
        let candidates = candidates.expect("lowering guarantees at least one target");
        if owners[ti].is_empty() {
            if candidates.len() > 1 {
                return Err(err(
                    name.span,
                    AsmErrorKind::BadTable("table is tied to more than one function"),
                ));
            }
            owners[ti] = candidates;
        } else {
            // Every referencing function must resolve every target; a
            // reference from a function that lacks the labels makes those
            // targets unknown THERE.
            for &owner in &owners[ti] {
                if !candidates.contains(&owner) {
                    let missing = targets
                        .iter()
                        .find(|t| !labels[owner as usize].iter().any(|(n, _)| n == &t.name))
                        .unwrap_or(&targets[0]);
                    return Err(err(
                        missing.span,
                        AsmErrorKind::UnknownTableLabel(missing.name.clone()),
                    ));
                }
            }
        }
    }

    // 4. Exactly one owner each.
    for (ti, table) in tables.iter().enumerate() {
        match owners[ti].len() {
            1 => {}
            0 => {
                return Err(err(
                    table.name().span,
                    AsmErrorKind::BadTable("table is never referenced"),
                ));
            }
            _ => {
                return Err(err(
                    table.name().span,
                    AsmErrorKind::BadTable("table is tied to more than one function"),
                ));
            }
        }
    }

    // 5. Emit: per owner, tables concatenate in source order.
    let mut table_blobs: Vec<Vec<u8>> = vec![Vec::new(); blobs.len()];
    let mut offsets: Vec<u32> = vec![0; tables.len()];
    for (ti, table) in tables.iter().enumerate() {
        let owner = *owners[ti].first().expect("attributed above") as usize;
        let out = &mut table_blobs[owner];
        offsets[ti] = out.len() as u32;
        match table {
            SourceTable::Match { rows, .. } => {
                out.push(rows[0].elems.len() as u8);
                out.extend((rows.len() as u16).to_le_bytes());
                for row in rows {
                    for elem in &row.elems {
                        out.push(match elem {
                            VecElem::Payload(p) => *p as u8,
                            VecElem::Wildcard => 0x7F,
                            _ => unreachable!("lowering rejects non-match elements in rows"),
                        });
                    }
                }
            }
            SourceTable::Dispatch { name, targets } => {
                if targets.len() > usize::from(u16::MAX) {
                    return Err(err(
                        name.span,
                        AsmErrorKind::BadTable("too many dispatch targets"),
                    ));
                }
                out.extend((targets.len() as u16).to_le_bytes());
                for target in targets {
                    let offset = labels[owner]
                        .iter()
                        .find(|(n, _)| n == &target.name)
                        .map(|(_, offset)| *offset)
                        .expect("attribution verified every target resolves in the owner");
                    out.extend(offset.to_le_bytes());
                }
            }
        }
    }

    // 6. Fixups — attribution guarantees each hole's table lives in the
    //    hole's own blob; patch the code hole with the table's offset.
    let mut fixups = Vec::new();
    for (blob, holes) in refs.iter().enumerate() {
        for hole in holes {
            let ti = table_index(&hole.name).expect("resolved in step 2");
            let table_offset = offsets[ti];
            let at = hole.offset as usize;
            blobs[blob][at..at + 4].copy_from_slice(&table_offset.to_le_bytes());
            fixups.push(TableFixup {
                blob: blob as u32,
                offset: hole.offset,
                table_offset,
            });
        }
    }
    Ok((table_blobs, fixups))
}

/// Match-table discipline (docs/formats.md (assembly text)): all rows
/// one width (1..=16); exact rows first, sorted lexicographically by
/// payload vector and pairwise disjoint (strictly ascending covers
/// both); wildcard rows after in source order; an all-wildcard catch-all
/// only last. The assembler validates, the VM trusts.
fn validate_match_discipline(name: &SpannedName, rows: &[SourceRow]) -> Result<(), AsmError> {
    let width = rows[0].elems.len();
    if !(1..=16).contains(&width) {
        return Err(err(
            rows[0].span,
            AsmErrorKind::TableDiscipline("row width must be 1..=16"),
        ));
    }
    if rows.len() > usize::from(u16::MAX) {
        return Err(err(
            name.span,
            AsmErrorKind::TableDiscipline("too many rows"),
        ));
    }
    let mut seen_wildcard = false;
    let mut prev_exact: Option<Vec<u32>> = None;
    for (i, row) in rows.iter().enumerate() {
        if row.elems.len() != width {
            return Err(err(
                row.span,
                AsmErrorKind::TableDiscipline("rows must all have the same width"),
            ));
        }
        // An exact row has a full payload key; any wildcard breaks it.
        let key: Option<Vec<u32>> = row
            .elems
            .iter()
            .map(|elem| match elem {
                VecElem::Payload(p) => Some(*p),
                _ => None,
            })
            .collect();
        match key {
            Some(key) => {
                if seen_wildcard {
                    return Err(err(
                        row.span,
                        AsmErrorKind::TableDiscipline("exact rows come before wildcard rows"),
                    ));
                }
                if let Some(prev) = &prev_exact
                    && *prev >= key
                {
                    return Err(err(
                        row.span,
                        AsmErrorKind::TableDiscipline(
                            "exact rows must be sorted and pairwise disjoint",
                        ),
                    ));
                }
                prev_exact = Some(key);
            }
            None => {
                seen_wildcard = true;
                let all_wildcard = row.elems.iter().all(|e| matches!(e, VecElem::Wildcard));
                if all_wildcard && i + 1 != rows.len() {
                    return Err(err(
                        row.span,
                        AsmErrorKind::TableDiscipline("the all-wildcard row must be last"),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::formats::object::SymbolDef;

    fn asm(src: &str) -> crate::formats::object::ObjectFile {
        assemble(&test_syntax(), 0x7E, src, false).unwrap()
    }

    #[test]
    fn single_function_with_backward_short_jump() {
        // fixture: ent 0x0E auto | nop 0x01 | jmp relaxable 0x20/0x30
        let obj = asm(".func f\nL:      nop\n        jmp L\n        stop\n");
        // blob: [0E] [01] [30 FD] [02]  — jmp relaxed short, off = 1-4 = -3
        assert_eq!(obj.blobs, vec![vec![0x0E, 0x01, 0x30, 0xFD, 0x02]]);
        assert_eq!(obj.symbols.len(), 1);
        assert!(matches!(obj.symbols[0].def, SymbolDef::Defined { blob: 0 }));
        assert!(obj.relocations.is_empty());
    }

    #[test]
    fn forward_jump_and_growth_to_far() {
        // 130 nops between jmp and its target force far form (offset > 127).
        let mut src = String::from(".func f\n        jmp END\n");
        for _ in 0..130 {
            src.push_str("        nop\n");
        }
        src.push_str("END:    stop\n");
        let obj = asm(&src);
        let blob = &obj.blobs[0];
        assert_eq!(blob[1], 0x20); // far jmp
        let off = i32::from_le_bytes(blob[2..6].try_into().unwrap());
        // instr_end = 6; target = 6 + 130 nops = 136; off = 130
        assert_eq!(off, 130);
        assert_eq!(blob.len(), 1 + 5 + 130 + 1);
    }

    #[test]
    fn relaxation_boundaries_are_exact() {
        // Forward: off = +127 stays short; +128 grows far.
        // jmp.s hole is 2 bytes: instr_end = 3; target = 3 + N nops + ... derive:
        // layout: [ent][jmp ...][N nops][END stop]; short: end=3, target=3+N → off=N.
        let make = |n: usize| {
            let mut s = String::from(".func f\n        jmp END\n");
            for _ in 0..n {
                s.push_str("        nop\n");
            }
            s.push_str("END:    stop\n");
            s
        };
        let short = assemble(&test_syntax(), 0x7E, &make(127), false).unwrap();
        assert_eq!(short.blobs[0][1], 0x30, "off +127 must stay short");
        assert_eq!(short.blobs[0][2] as i8, 127);
        let far = assemble(&test_syntax(), 0x7E, &make(128), false).unwrap();
        assert_eq!(far.blobs[0][1], 0x20, "off +128 must grow far");

        // Backward: off = -128 stays short. [ent][L:127 nops? derive]:
        // layout: [ent][L: nop x N][jmp L]: jmp short at 1+N..3+N, end 3+N, target 1 → off = -(N+2).
        // off -128 → N = 126.
        let mut s = String::from(".func f\nL:      nop\n");
        for _ in 0..125 {
            s.push_str("        nop\n");
        }
        s.push_str("        jmp     L\n");
        let back = assemble(&test_syntax(), 0x7E, &s, false).unwrap();
        let blob = &back.blobs[0];
        assert_eq!(blob[1 + 126], 0x30, "off -128 must stay short");
        assert_eq!(blob[1 + 127] as i8, -128);
    }

    #[test]
    fn relaxation_cascade_converges() {
        // Two jumps whose shortness depends on each other's size: jmp A over
        // a 124-nop gap that also contains jmp B — if B grows, A must grow.
        // Build: jmp END ; 124 nops ; jmp END2 ; END: stop ; …(130 nops)… ; END2: stop
        let mut src = String::from(".func f\n        jmp END\n");
        for _ in 0..124 {
            src.push_str("        nop\n");
        }
        src.push_str("        jmp END2\n");
        src.push_str("END:    stop\n");
        for _ in 0..130 {
            src.push_str("        nop\n");
        }
        src.push_str("END2:   stop\n");
        let obj = asm(&src);
        let blob = &obj.blobs[0];
        // inner jmp END2 must be far (>127 away); that pushes jmp END's
        // span to 124 + 5 = 129 > 127 → far as well.
        assert_eq!(blob[1], 0x20, "outer jump must have grown far");
    }

    #[test]
    fn forced_short_out_of_range_errors() {
        let mut src = String::from(".func f\n        jmp.s END\n");
        for _ in 0..130 {
            src.push_str("        nop\n");
        }
        src.push_str("END:    stop\n");
        let e = assemble(&test_syntax(), 0x7E, &src, false).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::ShortOffsetOutOfRange { ref target } if target == "END")
        );
    }

    #[test]
    fn calls_emit_holes_relocations_and_externals() {
        let obj =
            asm(".func f\n        call g\n        call f\n        stop\n.func g\n        ret\n");
        // f blob: [0E][21 00000000][21 00000000][02]
        assert_eq!(obj.blobs[0].len(), 1 + 5 + 5 + 1);
        assert_eq!(&obj.blobs[0][2..6], &[0, 0, 0, 0]);
        assert_eq!(obj.relocations.len(), 2);
        assert_eq!(obj.relocations[0].blob, 0);
        assert_eq!(obj.relocations[0].offset, 2);
        assert_eq!(obj.relocations[1].offset, 7);
        // symbols: f Defined(0), g Defined(1); call g resolves to index 1,
        // call f to index 0 — no externals needed here.
        let g_idx = obj.relocations[0].symbol as usize;
        assert_eq!(obj.symbols[g_idx].name, "g");
        let ext = asm(".func f\n        call missing\n");
        assert!(
            ext.symbols
                .iter()
                .any(|s| s.name == "missing" && s.def == SymbolDef::External)
        );
    }

    #[test]
    fn wr_and_byte_and_errors() {
        let obj = asm(".func f\n        wr 1\n        .byte 200\n");
        assert_eq!(obj.blobs[0], vec![0x0E, 0x07, 0x81, 200]);

        let e = assemble(&test_syntax(), 0x7E, ".func f\n        wr 300\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::EncodeError(_)));

        let e = assemble(&test_syntax(), 0x7E, ".func f\nL: nop\nL: nop\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::DuplicateLabel(ref l) if l == "L"));

        let e = assemble(
            &test_syntax(),
            0x7E,
            ".func f\n        jmp NOWHERE\n",
            false,
        )
        .unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownLabel(ref l) if l == "NOWHERE"));
    }

    #[test]
    fn duplicate_label_points_at_the_second_occurrence() {
        let e = assemble(
            &test_syntax(),
            0x7E,
            ".func f\nL:      nop\nL:      nop\n",
            false,
        )
        .unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::DuplicateLabel(ref l) if l == "L"));
        assert_eq!(e.span, crate::diagnostics::Span::new(3, 1, 3, 2));
    }

    #[test]
    fn unknown_label_points_at_the_jump_operand() {
        let e = assemble(
            &test_syntax(),
            0x7E,
            ".func f\n        jmp NOWHERE\n",
            false,
        )
        .unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownLabel(ref l) if l == "NOWHERE"));
        assert_eq!(e.span, crate::diagnostics::Span::new(2, 13, 2, 20));
    }

    #[test]
    fn debug_labels_and_lines_when_requested() {
        let obj = assemble(
            &test_syntax(),
            0x7E,
            ".func f\nL:      nop\n        stop\n",
            true,
        )
        .unwrap();
        let dbg = obj.debug.as_ref().unwrap();
        assert_eq!(dbg[0].labels, vec![("L".to_string(), 1)]);
        assert_eq!(dbg[0].lines, vec![(1, 2), (2, 3)]); // (blob offset, source line)
        assert_eq!(obj.arch, 0x7E);

        // Multiple labels at the same address sort by name, deterministically.
        let obj = assemble(
            &test_syntax(),
            0x7E,
            ".func f\nB:\nA:\n        nop\n        stop\n",
            true,
        )
        .unwrap();
        let dbg = obj.debug.as_ref().unwrap();
        assert_eq!(
            dbg[0].labels,
            vec![("A".to_string(), 1), ("B".to_string(), 1)]
        );
    }

    #[test]
    fn symbol_jump_emits_hole_and_relocation() {
        // fixture: jmp far = 0x20; g defined → blob 1.
        let obj = asm(".func f\n        jmp @g\n.func g\n        ret\n");
        assert_eq!(obj.blobs[0], vec![0x0E, 0x20, 0, 0, 0, 0]);
        assert_eq!(obj.relocations.len(), 1);
        assert_eq!(obj.relocations[0].offset, 2);
        assert_eq!(obj.symbols[obj.relocations[0].symbol as usize].name, "g");
        // External symbol jump works the same way:
        let ext = asm(".func f\n        jmp @missing\n");
        assert!(
            ext.symbols
                .iter()
                .any(|s| s.name == "missing" && s.def == SymbolDef::External)
        );
    }

    #[test]
    fn symbol_operand_restrictions() {
        let e = assemble(&test_syntax(), 0x7E, ".func f\n        jmp.s @g\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains("linker-selected")));
        let e = assemble(&test_syntax(), 0x7E, ".func f\n        br @g\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains("labels")));
        let e = assemble(&test_syntax(), 0x7E, ".func f\n        call @g\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains("drop the `@`")));
    }

    #[test]
    fn local_functions_get_local_symbols_and_intra_file_calls_bind() {
        let obj =
            asm(".func api\n        call helper\n        stop\n.func helper local\n        ret\n");
        assert!(matches!(obj.symbols[1].def, SymbolDef::Local { blob: 1 }));
        assert_eq!(obj.relocations.len(), 1);
        assert_eq!(
            obj.symbols[obj.relocations[0].symbol as usize].name,
            "helper"
        );
    }

    // -- Tables: blobs, fixups, discipline (fake dialect, caps all on) --

    /// Neutral fake dialect proving zero TM-1 knowledge in core:
    /// `tmatch`/`tdispatch` reference tables (TableRef), `vwrite` is the
    /// vector-capable write mnemonic, plus nop/stp/ent as usual.
    fn fake_syntax() -> ArchSyntax {
        use crate::asm::syntax::{AsmCaps, SyntaxEntry};
        use Flow::{FallThrough as FT, Stop};
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
                // A dispatch transfers through its table — no static
                // successor, so Stop for the disassembler traversal.
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
            caps: AsmCaps {
                tables: true,
                rept: true,
                vectors: true,
            },
        }
    }

    fn asm_fake(src: &str) -> Result<crate::formats::object::ObjectFile, AsmError> {
        assemble(&fake_syntax(), 0x7E, src, false)
    }

    #[test]
    fn builds_match_table_and_fixup() {
        let src = "\
.section tables
T0: .row [1, 2]
    .row [1, *]
.section code
.func main
    tmatch T0
    stp
";
        let obj = asm_fake(src).unwrap();
        let tables = obj.table_blobs.as_ref().unwrap();
        // Match layout (vm/table.rs): width u8, row_count u16 LE, rows of
        // one byte per position, 0x7F = wildcard.
        // width 2, 2 rows: [2, 2, 0, 1, 2, 1, 0x7F]
        assert_eq!(tables[0], vec![2, 2, 0, 1, 2, 1, 0x7F]);
        assert_eq!(obj.table_fixups.len(), 1);
        assert_eq!(obj.table_fixups[0].table_offset, 0);
        // Code: [ent 0E][tmatch 11][4-byte hole at 2..6][stp 02] — the
        // hole holds the table offset (0) and is recorded as the fixup.
        assert_eq!(obj.table_fixups[0].blob, 0);
        assert_eq!(obj.table_fixups[0].offset, 2);
        assert_eq!(obj.blobs[0], vec![0x0E, 0x11, 0, 0, 0, 0, 0x02]);
    }

    #[test]
    fn dispatch_entries_are_blob_relative_code_offsets() {
        // Layout derivation (the encoding, by hand): the blob opens with
        // the implicit ent byte at 0; tdispatch = opcode + 4-byte hole =
        // 5 bytes at 1..6; A: nop at 6; B: stp at 7. Dispatch layout
        // (vm/table.rs): entry_count u16 LE + u32 LE blob-relative code
        // offsets → [2, 0, 6,0,0,0, 7,0,0,0].
        let src = "\
.section tables
D0: .targets A, B
.section code
.func main
    tdispatch D0
A:  nop
B:  stp
";
        let obj = asm_fake(src).unwrap();
        let tables = obj.table_blobs.as_ref().unwrap();
        let mut expected = vec![2u8, 0];
        expected.extend(6u32.to_le_bytes());
        expected.extend(7u32.to_le_bytes());
        assert_eq!(tables[0], expected);
        // One fixup: the tdispatch hole at blob offset 1 + 1 = 2, and D0
        // is the blob's first (only) table, so table_offset = 0.
        assert_eq!(
            obj.table_fixups,
            vec![TableFixup {
                blob: 0,
                offset: 2,
                table_offset: 0
            }]
        );
    }

    #[test]
    fn tables_concatenate_per_function_in_source_order() {
        // T0 = [1, 2, 0, 1, 0x7F] (width 1, two rows) is 5 bytes, so D0
        // lands at table offset 5. Code: ent@0, tmatch@1 (hole 2..6),
        // tdispatch@6 (hole 7..11), A: stp@11 — the dispatch entry is 11.
        let src = "\
.section tables
T0: .row [1]
    .row [*]
D0: .targets A
.section code
.func main
    tmatch T0
    tdispatch D0
A:  stp
";
        let obj = asm_fake(src).unwrap();
        let tables = obj.table_blobs.as_ref().unwrap();
        let mut expected = vec![1u8, 2, 0, 1, 0x7F];
        expected.extend([1u8, 0]);
        expected.extend(11u32.to_le_bytes());
        assert_eq!(tables[0], expected);
        assert_eq!(
            obj.table_fixups,
            vec![
                TableFixup {
                    blob: 0,
                    offset: 2,
                    table_offset: 0
                },
                TableFixup {
                    blob: 0,
                    offset: 7,
                    table_offset: 5
                },
            ]
        );
        // The code holes are patched with the same table offsets.
        assert_eq!(&obj.blobs[0][2..6], &0u32.to_le_bytes());
        assert_eq!(&obj.blobs[0][7..11], &5u32.to_le_bytes());
    }

    #[test]
    fn rept_expanded_same_label_rows_continue_one_table() {
        // The UTM pattern: a `.rept` around a LABELED row emits the same
        // label every iteration — the run continues instead of clashing.
        let src = "\
.section tables
.rept v, 1, 3
T0: .row [{v}]
.endr
.section code
.func main
    tmatch T0
    stp
";
        let obj = asm_fake(src).unwrap();
        let tables = obj.table_blobs.as_ref().unwrap();
        // width 1, 3 exact rows [1] [2] [3] — sorted by construction.
        assert_eq!(tables[0], vec![1, 3, 0, 1, 2, 3]);
    }

    #[test]
    fn discipline_violations_are_rejected() {
        let asm_table = |table_lines: &str| {
            let src = format!(
                ".section tables\n{table_lines}\n.section code\n.func main\n    tmatch T\n    stp\n"
            );
            assemble(&fake_syntax(), 0x7E, &src, false)
        };
        // Exact rows unsorted: [2,2] then [1,2] is descending.
        let e = asm_table("T:  .row [2, 2]\n    .row [1, 2]").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::TableDiscipline(_)), "{e}");
        assert_eq!(e.span.start.line, 3); // the offending second row
        // Wildcard row before an exact row.
        let e = asm_table("T:  .row [1, *]\n    .row [1, 2]").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::TableDiscipline(_)), "{e}");
        assert_eq!(e.span.start.line, 3);
        // All-wildcard catch-all not last.
        let e = asm_table("T:  .row [*, *]\n    .row [1, *]").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::TableDiscipline(_)), "{e}");
        assert_eq!(e.span.start.line, 2); // the misplaced catch-all itself
        // Differing widths.
        let e = asm_table("T:  .row [1]\n    .row [1, 2]").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::TableDiscipline(_)), "{e}");
        assert_eq!(e.span.start.line, 3);
        // The control: sorted exact rows then a catch-all — accepted.
        assert!(asm_table("T:  .row [1, 2]\n    .row [2, 1]\n    .row [*, *]").is_ok());
    }

    #[test]
    fn table_in_code_section_rejected() {
        // The default section is code — a `.row` there is misplaced.
        let e = asm_fake(".func main\nT0: .row [1]\n    stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadTable(_)), "{e}");
        assert_eq!(e.span.start.line, 2);
    }

    #[test]
    fn function_in_tables_section_rejected() {
        let e = asm_fake(".section tables\n.func main\n    stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadTable(_)), "{e}");
    }

    #[test]
    fn code_in_tables_section_rejected() {
        let e = asm_fake(".section tables\n    nop\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadTable(_)), "{e}");
    }

    #[test]
    fn unknown_section_rejected() {
        let e = asm_fake(".section bogus\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadTable(_)), "{e}");
    }

    #[test]
    fn unreferenced_table_rejected() {
        let src = "\
.section tables
T0: .row [1]
.section code
.func main
    stp
";
        let e = asm_fake(src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadTable(_)), "{e}");
        assert_eq!(e.span.start.line, 2); // the table's label
    }

    #[test]
    fn table_referenced_from_two_functions_rejected() {
        let src = "\
.section tables
T0: .row [1]
.section code
.func f
    tmatch T0
    stp
.func g
    tmatch T0
    stp
";
        let e = asm_fake(src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadTable(_)), "{e}");
    }

    #[test]
    fn unknown_table_reference_rejected() {
        let e = asm_fake(".func main\n    tmatch NOPE\n    stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownTableLabel(ref l) if l == "NOPE"));
    }

    #[test]
    fn dispatch_targets_must_live_in_one_function() {
        // A lives in f, B in g — the targets straddle two functions.
        let src = "\
.section tables
D0: .targets A, B
.section code
.func f
    tdispatch D0
A:  stp
.func g
B:  stp
";
        let e = asm_fake(src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownTableLabel(_)), "{e}");
    }

    #[test]
    fn dispatch_target_must_exist() {
        let src = "\
.section tables
D0: .targets MISSING
.section code
.func main
    tdispatch D0
    stp
";
        let e = asm_fake(src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownTableLabel(ref l) if l == "MISSING"));
    }

    #[test]
    fn fake_dialect_without_tables_stays_v2_shape() {
        // No tables, no refs: the v3 fields stay absent even for a
        // caps-on dialect, so the object serializes as v2.
        let obj = asm_fake(".func main\n    nop\n    stp\n").unwrap();
        assert!(obj.is_v2_shape());
    }

    // -- Instruction vector operands (caps.vectors emit arms) ----------

    #[test]
    fn write_vector_assembles_with_keep_as_7f() {
        let obj = asm_fake(".func main\n        vwrite [1, -, 2]\n        stp\n").unwrap();
        // [ent 0E][vwrite 07][1, keep 0x7F, 2 | 0x80 on the last][stp 02]
        assert_eq!(obj.blobs[0], vec![0x0E, 0x07, 0x01, 0x7F, 0x82, 0x02]);
    }

    #[test]
    fn move_vector_assembles_with_move_codes() {
        let obj = asm_fake(".func main\n        vmove [<, ., >]\n        stp\n").unwrap();
        // `<` → 1, `.` → 0, `>` → 2 (high bit on the last element).
        assert_eq!(obj.blobs[0], vec![0x0E, 0x18, 0x01, 0x00, 0x82, 0x02]);
    }

    #[test]
    fn spelled_out_ints_still_assemble_under_vector_caps() {
        // The classic (SymbolVec, Ints) arm is untouched by the vector
        // arms: `vwrite 1` and `vwrite [1]` produce identical bytes.
        let spelled = asm_fake(".func main\n        vwrite 1\n        stp\n").unwrap();
        let bracket = asm_fake(".func main\n        vwrite [1]\n        stp\n").unwrap();
        assert_eq!(spelled.blobs, bracket.blobs);
        assert_eq!(spelled.blobs[0], vec![0x0E, 0x07, 0x81, 0x02]);
    }

    #[test]
    fn write_vector_vocabulary_violations_are_bad_vector_with_the_operand_span() {
        // Wildcards belong to `.row`, moves to a move vector; the span
        // is the `[..]` operand's own.
        for bad in ["[1, *]", "[<]", "[.]", "[>]"] {
            let src = format!(".func main\n        vwrite {bad}\n        stp\n");
            let e = asm_fake(&src).unwrap_err();
            assert!(matches!(e.kind, AsmErrorKind::BadVector(_)), "{bad}: {e}");
            let end = 16 + bad.chars().count() as u32;
            assert_eq!(e.span, Span::new(2, 16, 2, end), "{bad}");
        }
        // 0x7F would alias the keep marker; anything above overflows the
        // 7-bit element budget — both refuse before encoding.
        for bad in ["[127]", "[200]"] {
            let src = format!(".func main\n        vwrite {bad}\n        stp\n");
            let e = asm_fake(&src).unwrap_err();
            assert!(matches!(e.kind, AsmErrorKind::BadVector(_)), "{bad}: {e}");
        }
    }

    #[test]
    fn move_vector_vocabulary_violations_are_bad_vector_with_the_operand_span() {
        // Moves are spelled `<`/`>`/`.` only — numeric payloads, keeps,
        // and wildcards all refuse.
        for bad in ["[1]", "[0]", "[*]", "[-]", "[1, .]"] {
            let src = format!(".func main\n        vmove {bad}\n        stp\n");
            let e = asm_fake(&src).unwrap_err();
            assert!(matches!(e.kind, AsmErrorKind::BadVector(_)), "{bad}: {e}");
        }
        let e = asm_fake(".func main\n        vmove [*, .]\n").unwrap_err();
        assert_eq!(e.span, Span::new(2, 15, 2, 21)); // the `[*, .]` region
    }

    #[test]
    fn move_vector_requires_bracket_form() {
        // No legacy spelled-out-ints form exists for MoveVec.
        let e = asm_fake(".func main\n        vmove 1, 0\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)), "{e}");
        let e = asm_fake(".func main\n        vmove\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)), "{e}");
    }
}
