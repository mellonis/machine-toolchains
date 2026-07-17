//! Two-pass assembly with short/far relaxation (docs/formats.md (assembly
//! text); docs/isa.md for the opcode/relaxation table this assembles
//! against).

use std::collections::{BTreeSet, HashMap};

use super::lower::{
    FrameTapeMap, SourceFunction, SourceItem, SourceOperand, SourceRow, SourceTable,
    SourceTapeBinding, SpannedName, VecElem, lower_source,
};
use super::syntax::{ArchSyntax, Flow};
use super::{AsmError, AsmErrorKind};
use crate::diagnostics::Span;
use crate::formats::object::{
    BlobDebug, BoundCall, MapPair, ObjectFile, Relocation, Symbol, SymbolDef, TableFixup,
    TapeBinding,
};
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
    /// A framed call (`call.m target, F`-style): opcode + 8-byte hole.
    /// The first 4 bytes relocate to the target symbol (like a `Call`);
    /// the last 4 are a table-ref hole naming the frame descriptor,
    /// patched with its blob-local table offset and recorded as a
    /// `TableFixup` — the same single-owner attribution as a `TableRef`.
    FramedCall {
        span: Span,
        opcode: u8,
        target: String,
        frame_name: String,
        frame_span: Span,
    },
    /// A declarative binding call (`call name [binding]`): the far-call
    /// opcode + a 4-byte ZERO hole, with NO relocation — the call target
    /// and tape binding ride a `BoundCall` record instead, which the
    /// composition engine lowers at link time (docs/formats.md (bound
    /// calls)). `symbol_span` doubles as the debug-line source, like
    /// `Call`.
    BoundCall {
        symbol_span: Span,
        opcode: u8,
        target: String,
        binding: Vec<TapeBinding>,
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
            Slot::Call { .. } | Slot::TableRef { .. } | Slot::BoundCall { .. } => 5,
            Slot::FramedCall { .. } => 9,
        }
    }
}

/// A `TableRef` operand hole recorded during emit: the 4-byte hole at
/// `offset` within its blob, and the table label it references.
/// `is_frame_ref` distinguishes a `call.m` frame-half hole (which must
/// name a `.frame` descriptor) from a plain `mtc`/`djmp` table reference
/// (which must not) — the kind knowledge `build_tables` enforces.
struct TableRefHole {
    name: String,
    name_span: Span,
    offset: u32,
    is_frame_ref: bool,
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
    bound_calls: Vec<BoundCall>,
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
    let mut bound_calls: Vec<BoundCall> = Vec::new();
    let mut debug = with_debug.then(Vec::new);
    // Per-blob label offsets and TableRef holes, for table building.
    let mut blob_labels: Vec<Vec<(String, u32)>> = Vec::with_capacity(functions.len());
    let mut blob_table_refs: Vec<Vec<TableRefHole>> = Vec::with_capacity(functions.len());

    for (blob_idx, function) in functions.iter().enumerate() {
        // A signed function (its file declares `.routine`) carries an
        // arity; vector operands inside it are statically width-checked
        // against it. Signatures are caps-gated (no `.routine` without the
        // tables cap) and parallel the functions by index when present.
        let sig_arity = lowered.signatures.as_ref().map(|sigs| sigs[blob_idx].arity);
        let assembled = assemble_function(
            syntax,
            function,
            blob_idx as u32,
            sig_arity,
            &mut symbols,
            &mut symbol_index,
        )?;
        blobs.push(assembled.blob);
        relocations.extend(assembled.relocations);
        bound_calls.extend(assembled.bound_calls);
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
    // `.routine` signatures ride the same v3 gate (docs/formats.md
    // (MO)): lowering guarantees they parallel the functions — and so
    // the blobs — when present; absent, the object keeps its v2 shape
    // byte-for-byte.
    object.signatures = lowered.signatures;
    // Declarative binding calls force the v3 object shape (docs/formats.md
    // (bound calls)); an object with none keeps its v2 shape byte-for-byte.
    object.bound_calls = bound_calls;
    Ok(object)
}

fn assemble_function(
    syntax: &ArchSyntax,
    function: &SourceFunction,
    blob_idx: u32,
    sig_arity: Option<u8>,
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
                    (OperandKind::Imm8, SourceOperand::Imm(value)) => {
                        slots.push(Slot::Fixed {
                            span,
                            bytes: vec![*opcode, *value],
                        });
                    }
                    (OperandKind::FramedCall, SourceOperand::FramedCall { target, frame }) => {
                        slots.push(Slot::FramedCall {
                            span,
                            opcode: *opcode,
                            target: target.name.clone(),
                            frame_name: frame.name.clone(),
                            frame_span: frame.span,
                        });
                    }
                    (
                        OperandKind::RelI8 | OperandKind::RelI32,
                        SourceOperand::BoundCallOp { target, binding },
                    ) => {
                        slots.push(Slot::BoundCall {
                            symbol_span: target.span,
                            opcode: *opcode,
                            target: target.name.clone(),
                            binding: binding.iter().map(source_binding_to_object).collect(),
                        });
                    }
                    (OperandKind::SymbolVec, SourceOperand::Ints(ints)) => {
                        // Inside a signed function the static width check is
                        // spelling-independent: the spelled-out `wr 1, 0`
                        // must honor the routine arity exactly as `wr [1, 0]`
                        // does through `vector_slot` (docs/formats.md
                        // (assembly text)).
                        if let Some(arity) = sig_arity
                            && ints.len() != usize::from(arity)
                        {
                            return Err(err(
                                span,
                                AsmErrorKind::BadVector(
                                    "vector width does not match the routine arity",
                                ),
                            ));
                        }
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
                            sig_arity,
                            write_vector_element,
                        )?);
                    }
                    (OperandKind::MoveVec, SourceOperand::Vector(elems, vspan)) => {
                        slots.push(vector_slot(
                            *opcode,
                            span,
                            *vspan,
                            elems,
                            sig_arity,
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
            let mut bound_calls = Vec::new();
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
                        | Slot::TableRef { span, .. }
                        | Slot::FramedCall { span, .. } => span.start.line,
                        Slot::Call { symbol_span, .. } | Slot::BoundCall { symbol_span, .. } => {
                            symbol_span.start.line
                        }
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
                            is_frame_ref: false,
                        });
                        blob.extend([0u8; 4]);
                    }
                    Slot::FramedCall {
                        opcode,
                        target,
                        frame_name,
                        frame_span,
                        ..
                    } => {
                        blob.push(*opcode);
                        // Displacement half: relocates to the target
                        // symbol exactly like a plain call.
                        let sym_idx = *symbol_index.entry(target.clone()).or_insert_with(|| {
                            symbols.push(Symbol {
                                name: target.clone(),
                                def: SymbolDef::External,
                            });
                            (symbols.len() - 1) as u32
                        });
                        relocs.push(Relocation {
                            blob: blob_idx,
                            offset: blob.len() as u32,
                            symbol: sym_idx,
                        });
                        blob.extend([0u8; 4]);
                        // Frame half: a table-ref hole at offset+4, riding
                        // `build_tables`' single-owner attribution and the
                        // same fixup path as a `TableRef` (the hole is 4
                        // bytes, here at offset+4 rather than offset+0).
                        table_refs.push(TableRefHole {
                            name: frame_name.clone(),
                            name_span: *frame_span,
                            offset: blob.len() as u32,
                            is_frame_ref: true,
                        });
                        blob.extend([0u8; 4]);
                    }
                    Slot::BoundCall {
                        opcode,
                        target,
                        binding,
                        ..
                    } => {
                        blob.push(*opcode);
                        // A binding call carries NO relocation: the 4-byte
                        // hole stays zero and the target+binding ride a
                        // `BoundCall` record the composition engine lowers
                        // at link time (docs/formats.md (bound calls)). The
                        // callee may be extern; interning it mirrors a plain
                        // call's external-symbol handling.
                        let sym_idx = *symbol_index.entry(target.clone()).or_insert_with(|| {
                            symbols.push(Symbol {
                                name: target.clone(),
                                def: SymbolDef::External,
                            });
                            (symbols.len() - 1) as u32
                        });
                        bound_calls.push(BoundCall {
                            blob: blob_idx,
                            offset: blob.len() as u32,
                            symbol: sym_idx,
                            binding: binding.clone(),
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
                bound_calls,
            });
        }
    }
}

/// Converts a validated source tape binding into its MO wire form
/// (docs/formats.md (bound calls)). The one-way bit is real data here —
/// it distinguishes `->` (bidirectional) from `=>` (read-only) pairs for
/// the composition engine, unlike a frame descriptor where the wire form
/// drops it.
fn source_binding_to_object(b: &SourceTapeBinding) -> TapeBinding {
    TapeBinding {
        caller_tape: b.caller_tape,
        pairs: b
            .pairs
            .iter()
            .map(|&(src, dst, one_way)| MapPair { src, dst, one_way })
            .collect(),
    }
}

/// Encodes a `[..]` vector operand into a fixed slot, mapping each
/// element through the operand kind's vocabulary. An element outside
/// the vocabulary is a `BadVector` at the operand's own span
/// (`vector_span`); `span` stays the whole item's, for the slot and
/// any encode failure.
///
/// `sig_arity` carries the enclosing function's `.routine` arity when it
/// is signed. Inside a signed function every vector operand must be
/// exactly that wide — a routine authored at arity M drives M tapes, so a
/// mismatched vector is a static `BadVector` (docs/formats.md (assembly
/// text)). Unsigned functions keep the arch's full 1..=16 freedom.
fn vector_slot(
    opcode: u8,
    span: Span,
    vector_span: Span,
    elems: &[VecElem],
    sig_arity: Option<u8>,
    vocab: fn(&VecElem) -> Result<u32, &'static str>,
) -> Result<Slot, AsmError> {
    if let Some(arity) = sig_arity
        && elems.len() != usize::from(arity)
    {
        return Err(err(
            vector_span,
            AsmErrorKind::BadVector("vector width does not match the routine arity"),
        ));
    }
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

    // 2. Owner candidates from TableRef operands. Kind knowledge is
    //    directive-driven here (handoff e): a `call.m` frame-half hole
    //    (`is_frame_ref`) must name a `.frame` descriptor, and a plain
    //    `mtc`/`djmp` reference must NOT — enforced at attribution time,
    //    where both the referencing kind and the table kind are known.
    let mut owners: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); tables.len()];
    for (blob, holes) in refs.iter().enumerate() {
        for hole in holes {
            let Some(ti) = table_index(&hole.name) else {
                return Err(err(
                    hole.name_span,
                    AsmErrorKind::UnknownTableLabel(hole.name.clone()),
                ));
            };
            let is_frame_table = matches!(tables[ti], SourceTable::Frame { .. });
            match (hole.is_frame_ref, is_frame_table) {
                (true, false) => {
                    return Err(err(
                        hole.name_span,
                        AsmErrorKind::BadFrame(format!(
                            "`call.m` frame operand `{}` must name a `.frame` descriptor",
                            hole.name
                        )),
                    ));
                }
                (false, true) => {
                    return Err(err(
                        hole.name_span,
                        AsmErrorKind::BadFrame(format!(
                            "`{}` is a `.frame` descriptor; reference it with `call.m`, \
                             not a table operand",
                            hole.name
                        )),
                    ));
                }
                _ => {}
            }
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
            SourceTable::Frame {
                tapes, maps, exits, ..
            } => {
                let descriptor = emit_frame_descriptor(tapes, maps, exits, &labels[owner])?;
                out.extend(descriptor);
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

/// Lays out a frame descriptor's bytes (docs/formats.md (frame
/// descriptors)): `arity u8`, `exit_count u16 LE`, then per virtual tape
/// `[phys u8, rmap_len u16 LE, rmap entries u16 LE, wmap_len u16 LE, wmap
/// entries u16 LE]`, then `exit_count × u32 LE` blob-relative code offsets.
/// A missing `.map k` emits identity (both lengths 0). Exit labels resolve
/// against the OWNING function's labels; an exit absent there is a
/// `BadFrame`. The dense maps arrive already blank-pinning-checked from
/// lower (index 0 pinned to 0; a symbol may otherwise fold onto blank).
fn emit_frame_descriptor(
    tapes: &[u8],
    maps: &[FrameTapeMap],
    exits: &[SpannedName],
    owner_labels: &[(String, u32)],
) -> Result<Vec<u8>, AsmError> {
    let arity = tapes.len(); // lower guarantees 1..=16
    if exits.len() > usize::from(u16::MAX) {
        return Err(err(
            exits[0].span,
            AsmErrorKind::BadFrame("too many frame exits".to_string()),
        ));
    }
    let mut out = Vec::new();
    out.push(arity as u8);
    out.extend((exits.len() as u16).to_le_bytes());
    for (k, &phys) in tapes.iter().enumerate() {
        out.push(phys);
        let map = maps.iter().find(|m| m.k as usize == k);
        let (rmap, wmap): (&[u16], &[u16]) = match map {
            Some(m) => (&m.rmap, &m.wmap),
            None => (&[], &[]),
        };
        out.extend((rmap.len() as u16).to_le_bytes());
        for &v in rmap {
            out.extend(v.to_le_bytes());
        }
        out.extend((wmap.len() as u16).to_le_bytes());
        for &v in wmap {
            out.extend(v.to_le_bytes());
        }
    }
    for exit in exits {
        let offset = owner_labels
            .iter()
            .find(|(n, _)| n == &exit.name)
            .map(|(_, offset)| *offset)
            .ok_or_else(|| {
                err(
                    exit.span,
                    AsmErrorKind::BadFrame(format!(
                        "exit label `{}` is not in the owning function",
                        exit.name
                    )),
                )
            })?;
        out.extend(offset.to_le_bytes());
    }
    Ok(out)
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
    /// vector-capable write mnemonic, `fimm` takes a plain immediate
    /// (Imm8), `fcall` is a framed call (FramedCall, Call flow), plus
    /// nop/stp/ent as usual.
    fn fake_syntax() -> ArchSyntax {
        use crate::asm::syntax::{AsmCaps, SyntaxEntry};
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
                    opcode: 0x20,
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
    fn rept_expanded_same_label_targets_continue_one_table() {
        // The UTM dispatch pattern (the `.target` sibling of the `.row`
        // continuation above): a `.rept` around a LABELED `.target` emits
        // the same label every iteration — the targets accrue into ONE
        // dispatch table instead of clashing.
        let src = "\
.section tables
.rept i, 0, 2
D0: .target T{i}
.endr
.section code
.func main
    tdispatch D0
T0: nop
T1: nop
T2: stp
";
        let obj = asm_fake(src).unwrap();
        let tables = obj.table_blobs.as_ref().unwrap();
        // One dispatch table with three entries (T0, T1, T2) — proof the
        // three same-label `.target` lines merged rather than clashing.
        // Layout: ent@0, tdispatch@1 (hole 2..6), T0: nop@6, T1: nop@7,
        // T2: stp@8. Dispatch layout: entry_count u16 LE + u32 LE offsets.
        let mut expected = vec![3u8, 0];
        expected.extend(6u32.to_le_bytes());
        expected.extend(7u32.to_le_bytes());
        expected.extend(8u32.to_le_bytes());
        assert_eq!(tables[0], expected);
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

    // -- Immediate + framed-call operands (Imm8 / FramedCall) ----------

    #[test]
    fn fimm_emits_opcode_then_immediate_byte() {
        let obj = asm_fake(".func main\n        fimm #7\n        stp\n").unwrap();
        // [ent 0E][fimm 13][7][stp 02] — one raw immediate byte, no vector
        // continuation bit.
        assert_eq!(obj.blobs[0], vec![0x0E, 0x13, 7, 0x02]);
        assert!(obj.is_v2_shape()); // no tables, no refs
    }

    #[test]
    fn fimm_immediate_bounds_and_hash_prefix_are_enforced() {
        // Missing `#` prefix.
        let e = asm_fake(".func main\n        fimm 7\n        stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)), "{e}");
        // Above the byte range.
        let e = asm_fake(".func main\n        fimm #256\n        stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)), "{e}");
        // The bounds themselves assemble.
        assert!(asm_fake(".func main\n        fimm #0\n        stp\n").is_ok());
        assert!(asm_fake(".func main\n        fimm #255\n        stp\n").is_ok());
    }

    #[test]
    fn fcall_emits_reloc_for_the_target_and_frame_fixup_at_offset_plus_4() {
        // `fcall target, F0`: the displacement half relocates to `target`,
        // the frame half is a table-ref hole naming the `.frame` descriptor
        // F0 (post-handoff-e a framed call must name a `.frame`, never a
        // match/dispatch table).
        let src = "\
.section tables
F0: .frame tapes=(0, 1)
.section code
.func main
    fcall target, F0
    stp
.func target
    stp
";
        let obj = asm_fake(src).unwrap();
        // main blob: ent@0, fcall@1 (opcode + 8-byte hole = 1..10), stp@10.
        // Rel hole at 2..6 (zeroed reloc), frame hole at 6..10 (patched to
        // F0's blob-local offset, 0).
        assert_eq!(obj.blobs[0], vec![0x0E, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0x02]);
        // One relocation: the target symbol at the displacement hole (2).
        assert_eq!(obj.relocations.len(), 1);
        assert_eq!(obj.relocations[0].blob, 0);
        assert_eq!(obj.relocations[0].offset, 2);
        assert_eq!(
            obj.symbols[obj.relocations[0].symbol as usize].name,
            "target"
        );
        // One table fixup: the frame hole at offset+4 = 6, table offset 0.
        assert_eq!(
            obj.table_fixups,
            vec![TableFixup {
                blob: 0,
                offset: 6,
                table_offset: 0
            }]
        );
        // F0's descriptor bytes live in main's table blob: arity 2, no
        // exits, both tapes identity (phys 0 / phys 1, empty rmap/wmap).
        let tables = obj.table_blobs.as_ref().unwrap();
        assert_eq!(
            tables[0],
            vec![
                2, 0, 0, /* tape0 */ 0, 0, 0, 0, 0, /* tape1 */ 1, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn fcall_operand_shape_violations_are_rejected() {
        // A single name — the frame half is missing.
        let e =
            asm_fake(".func main\n    fcall target\n    stp\n.func target\n    stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)), "{e}");
        // A `@`-prefixed target: framed-call targets are already symbols.
        let e = asm_fake(
            ".section tables\nT0: .row [1]\n.section code\n.func main\n    fcall @target, T0\n    stp\n.func target\n    stp\n",
        )
        .unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains("drop the `@`")),
            "{e}"
        );
    }

    // -- Declarative binding calls (`call name [binding]`) --------------

    const BINDING_PROGRAM: &str = "\
.func main
    call plusOne [2{1->3,2=>0}, 0]
    stp
.func plusOne
    stp
";

    #[test]
    fn binding_call_emits_zero_hole_and_a_bound_call_record() {
        let obj = asm_fake(BINDING_PROGRAM).unwrap();
        // main blob: ent@0, call@1 (opcode 0x21 + 4-byte ZERO hole at
        // 2..6), stp@6. No relocation rides the hole.
        assert_eq!(obj.blobs[0], vec![0x0E, 0x21, 0, 0, 0, 0, 0x02]);
        assert!(
            obj.relocations.is_empty(),
            "no relocation for a binding call"
        );
        // The bound-call record carries the hole and the binding.
        assert_eq!(obj.bound_calls.len(), 1);
        let bc = &obj.bound_calls[0];
        assert_eq!(bc.blob, 0);
        assert_eq!(bc.offset, 2);
        assert_eq!(obj.symbols[bc.symbol as usize].name, "plusOne");
        // Binding: entry 0 = physical tape 2 with a `->` and a `=>` pair;
        // entry 1 = physical tape 0, no pairs. The one-way bit is real data.
        assert_eq!(bc.binding.len(), 2);
        assert_eq!(bc.binding[0].caller_tape, 2);
        assert_eq!(
            bc.binding[0].pairs,
            vec![
                MapPair {
                    src: 1,
                    dst: 3,
                    one_way: false
                },
                MapPair {
                    src: 2,
                    dst: 0,
                    one_way: true
                },
            ]
        );
        assert_eq!(bc.binding[1].caller_tape, 0);
        assert!(bc.binding[1].pairs.is_empty());
        // A bound call forces the v3 object shape.
        assert!(!obj.is_v2_shape());
    }

    #[test]
    fn binding_call_round_trips_through_object_bytes() {
        let obj = asm_fake(BINDING_PROGRAM).unwrap();
        let back = ObjectFile::from_bytes(&obj.to_bytes()).unwrap();
        assert_eq!(back.bound_calls, obj.bound_calls);
        assert_eq!(back, obj);
    }

    #[test]
    fn binding_call_target_may_be_external() {
        // `plusOne` is undefined in this file — like a plain call, the
        // record's symbol is interned as External.
        let obj = asm_fake(".func main\n    call plusOne [0]\n    stp\n").unwrap();
        assert_eq!(obj.bound_calls.len(), 1);
        let sym = &obj.symbols[obj.bound_calls[0].symbol as usize];
        assert_eq!(sym.name, "plusOne");
        assert_eq!(sym.def, SymbolDef::External);
        assert!(obj.relocations.is_empty());
    }

    #[test]
    fn binding_call_physical_index_over_15_is_rejected() {
        let e = asm_fake(".func main\n    call f [16]\n    stp\n").unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(ref m) if m.contains("< 16")),
            "{e}"
        );
        // The diagnostic points at the `[..]` binding operand on line 2,
        // starting at the `[` (column 12), not the whole line.
        assert_eq!(e.span.start.line, 2);
        assert_eq!(e.span.start.col, 12);
    }

    #[test]
    fn binding_call_duplicate_source_is_rejected() {
        let e = asm_fake(".func main\n    call f [0{1->2,1->3}]\n    stp\n").unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(ref m) if m.contains("duplicate source")),
            "{e}"
        );
    }

    #[test]
    fn binding_call_empty_binding_is_rejected() {
        let e = asm_fake(".func main\n    call f []\n    stp\n").unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(ref m) if m.contains("at least one")),
            "{e}"
        );
    }

    #[test]
    fn binding_call_malformed_pairs_are_rejected() {
        // A non-canonical index (`01`), a lone arrow, and an unbalanced
        // brace each fail structural shaping.
        for src in [
            ".func main\n    call f [01]\n    stp\n",
            ".func main\n    call f [0{1->}]\n    stp\n",
            ".func main\n    call f [0{1->2]\n    stp\n",
        ] {
            let e = asm_fake(src).unwrap_err();
            assert!(matches!(e.kind, AsmErrorKind::BadFrame(_)), "{src}: {e}");
        }
    }

    #[test]
    fn binding_bracket_on_a_non_call_is_rejected() {
        // `jmp` is a RelI32 Jump — a trailing bracket is not a binding.
        let e = asm_fake(".func main\n    jmp L [0]\nL:  stp\n").unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains("only a call")),
            "{e}"
        );
    }

    // -- Frame descriptors (`.frame`/`.map`/`.exits`) -------------------

    #[test]
    fn frame_descriptor_bytes_and_exit_offsets_are_laid_out() {
        // A 2-arity frame projecting virtual (0,1) → physical (2,0), with a
        // non-identity rmap on tape 0 (a `->`, a one-way `=>`, and a hole)
        // and two exits into the owning function `main`.
        let src = "\
.section tables
F0: .frame tapes=(2, 0)
    .map 0, rmap=(1->2, 3=>1)
    .exits done, other
.section code
.func main
    fcall helper, F0
done:   stp
other:  stp
.func helper
    stp
";
        let obj = asm_fake(src).unwrap();
        let tables = obj.table_blobs.as_ref().unwrap();
        // main: ent@0, fcall@1 (9 bytes), done: stp@10, other: stp@11.
        // Descriptor (docs/formats.md (frame descriptors)):
        //   arity 2, exit_count 2,
        //   tape0: phys 2, rmap_len 4 → [0,2,hole,1], wmap_len 0,
        //   tape1: phys 0, rmap_len 0, wmap_len 0,
        //   exits: done=10, other=11 (u32 LE).
        let expected = vec![
            2u8, // arity
            2, 0, // exit_count
            2, // tape0 phys
            4, 0, // rmap_len
            0, 0, 2, 0, 0xFF, 0xFF, 1, 0, // rmap: 0, 2, hole, 1
            0, 0, // wmap_len
            0, // tape1 phys
            0, 0, // rmap_len
            0, 0, // wmap_len
            10, 0, 0, 0, // exit done
            11, 0, 0, 0, // exit other
        ];
        assert_eq!(tables[0], expected);
    }

    #[test]
    fn frame_missing_map_is_identity_and_no_exits_is_zero_count() {
        // A frame with no `.map` and no `.exits`: every tape identity
        // (both lengths 0), exit_count 0.
        let src = "\
.section tables
F0: .frame tapes=(1, 0, 2)
.section code
.func main
    fcall helper, F0
    stp
.func helper
    stp
";
        let obj = asm_fake(src).unwrap();
        let tables = obj.table_blobs.as_ref().unwrap();
        assert_eq!(
            tables[0],
            vec![
                3, 0, 0, /* t0 */ 1, 0, 0, 0, 0, /* t1 */ 0, 0, 0, 0, 0, /* t2 */ 2,
                0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn frame_map_may_collapse_symbols_onto_blank() {
        // A non-blank symbol may fold onto blank (index 0) in either
        // direction. rmap `3->0` reads a foreign boundary marker AS the
        // callee's blank — the flagship one-way pattern; wmap `2->0` writes
        // a virtual symbol back as physical blank — an erase. Only index 0
        // itself stays pinned. The dense bytes carry the 0-valued entries
        // (0 is a legal mapped value; the hole marker is `0xFFFF`).
        let src = "\
.section tables
F0: .frame tapes=(0)
    .map 0, rmap=(1->2, 3->0), wmap=(2->0)
.section code
.func main
    fcall helper, F0
    stp
.func helper
    stp
";
        let obj = asm_fake(src).unwrap();
        let tables = obj.table_blobs.as_ref().unwrap();
        // Descriptor: arity 1, exit_count 0,
        //   tape0: phys 0,
        //     rmap_len 4 → [0, 2, hole, 0]  (idx0 pinned, 1->2, idx2 hole, 3->0)
        //     wmap_len 3 → [0, hole, 0]      (idx0 pinned, idx1 hole, 2->0)
        let expected = vec![
            1u8, // arity
            0, 0, // exit_count
            0, // tape0 phys
            4, 0, // rmap_len
            0, 0, 2, 0, 0xFF, 0xFF, 0, 0, // rmap: 0, 2, hole, 0
            3, 0, // wmap_len
            0, 0, 0xFF, 0xFF, 0, 0, // wmap: 0, hole, 0
        ];
        assert_eq!(tables[0], expected);
    }

    /// Assemble a frames program built from a table-section body and a
    /// `call.m` in `main` referencing `F0`, returning the error.
    fn asm_frame_err(table_body: &str, exits_labels: &str) -> AsmError {
        let src = format!(
            ".section tables\n{table_body}\n.section code\n.func main\n    fcall helper, F0\n{exits_labels}    stp\n.func helper\n    stp\n"
        );
        asm_fake(&src).unwrap_err()
    }

    #[test]
    fn frame_bad_cases_are_bad_frame() {
        // Duplicate `.map k`.
        let e = asm_frame_err("F0: .frame tapes=(1, 0)\n    .map 0\n    .map 0", "");
        assert!(matches!(e.kind, AsmErrorKind::BadFrame(_)), "dup map: {e}");
        // k >= arity.
        let e = asm_frame_err("F0: .frame tapes=(1, 0)\n    .map 2", "");
        assert!(matches!(e.kind, AsmErrorKind::BadFrame(_)), "k>=arity: {e}");
        // Map index past 0xFFFE.
        let e = asm_frame_err("F0: .frame tapes=(1, 0)\n    .map 0, rmap=(65535->1)", "");
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(_)),
            "index max: {e}"
        );
        // `.exits` twice.
        let e = asm_frame_err(
            "F0: .frame tapes=(1, 0)\n    .exits done\n    .exits done",
            "done:   nop\n",
        );
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(_)),
            "exits twice: {e}"
        );
        // Blank pinning: 0 -> non-blank is rejected (blank must map to
        // blank). A non-blank index folding onto 0 is legal — see
        // `frame_map_may_collapse_symbols_onto_blank`.
        let e = asm_frame_err("F0: .frame tapes=(1, 0)\n    .map 0, rmap=(0->3)", "");
        assert!(matches!(e.kind, AsmErrorKind::BadFrame(_)), "0->X: {e}");
        // Exit label absent from the owning function.
        let e = asm_frame_err("F0: .frame tapes=(1, 0)\n    .exits ghost", "");
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(_)),
            "ghost exit: {e}"
        );
    }

    #[test]
    fn wmap_rejects_one_way_pairs_read_direction_only() {
        // `=>` (one-way) is read-direction only: legal in `rmap`, rejected
        // in `wmap` (the write direction). The rejection is spanned at the
        // `wmap=` group and carries the read-direction message.
        let src = "\
.section tables
F0: .frame tapes=(1, 0)
    .map 0, wmap=(1=>2)
.section code
.func main
    fcall helper, F0
    stp
.func helper
    stp
";
        let e = asm_fake(src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(ref m) if m.contains("read-direction only")),
            "expected a read-direction BadFrame, got {e}"
        );
        assert_eq!(e.span, Span::new(3, 18, 3, 24)); // the `(1=>2)` group

        // Scoped to wmap: the same one-way pair in `rmap` assembles.
        let ok = "\
.section tables
F0: .frame tapes=(1, 0)
    .map 0, rmap=(1=>2)
.section code
.func main
    fcall helper, F0
    stp
.func helper
    stp
";
        assert!(asm_fake(ok).is_ok(), "rmap=(1=>2) must still assemble");
    }

    #[test]
    fn frame_tapes_list_bounds_and_orphans_are_bad_frame() {
        // Empty tapes list.
        let e = asm_fake(".section tables\nF0: .frame tapes=()\n").unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(_)),
            "empty tapes: {e}"
        );
        // Over-16 tapes list.
        let seq = vec!["0"; 17].join(", ");
        let e = asm_fake(&format!(".section tables\nF0: .frame tapes=({seq})\n")).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadFrame(_)), "17 tapes: {e}");
        // Orphan `.map` (no preceding `.frame`).
        let e = asm_fake(".section tables\n    .map 0, rmap=(1->2)\n").unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(_)),
            "orphan map: {e}"
        );
        // Orphan `.exits`.
        let e = asm_fake(".section tables\n    .exits foo\n").unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(_)),
            "orphan exits: {e}"
        );
    }

    #[test]
    fn frame_kind_mismatch_both_directions_is_bad_frame() {
        // `tmatch` (a plain table operand) referencing a `.frame`
        // descriptor.
        let src = "\
.section tables
F0: .frame tapes=(1, 0)
.section code
.func main
    tmatch F0
    stp
";
        let e = asm_fake(src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(_)),
            "tmatch→frame: {e}"
        );
        // `call.m` referencing a match table (the fcall side of handoff e).
        let src = "\
.section tables
T0: .row [1, 2]
.section code
.func main
    fcall helper, T0
    stp
.func helper
    stp
";
        let e = asm_fake(src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(_)),
            "call.m→match: {e}"
        );
    }

    #[test]
    fn signed_function_ints_form_honors_the_arity_check() {
        // Handoff f: the spelled-out `wr 1, 0` inside a signed function must
        // honor the routine arity just like `wr [1, 0]` — a width-1 spelled
        // vector in an arity-2 routine is a `BadVector`.
        let e =
            asm_fake(".routine main, tapes=2, alpha=(3, 5)\n.func main\n    vwrite 1\n    stp\n")
                .unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadVector(m) if m.contains("arity")),
            "{e}"
        );
        // The matching width still assembles in spelled form.
        assert!(
            asm_fake(
                ".routine main, tapes=2, alpha=(3, 5)\n.func main\n    vwrite 1, 2\n    stp\n"
            )
            .is_ok()
        );
    }

    #[test]
    fn fcall_frame_label_naming_no_table_is_unknown_table_label() {
        // The frame half must resolve to a table defined in the file.
        let e = asm_fake(".func main\n    fcall target, NOPE\n    stp\n.func target\n    stp\n")
            .unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::UnknownTableLabel(ref l) if l == "NOPE"),
            "{e}"
        );
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

    // -- Signed-function static vector-width check ----------------------

    #[test]
    fn signed_function_vector_wrong_width_is_bad_vector() {
        // `main` is signed at arity 2 (a `.routine` with two tapes). A
        // wider write vector and a narrower move vector both mismatch the
        // routine arity — the static width check refuses them at the
        // operand's own span.
        for bad in ["vwrite [1, 2, 3]", "vmove [<]"] {
            let src =
                format!(".routine main, tapes=2, alpha=(3, 5)\n.func main\n    {bad}\n    stp\n");
            let e = asm_fake(&src).unwrap_err();
            assert!(
                matches!(e.kind, AsmErrorKind::BadVector(m) if m.contains("arity")),
                "{bad}: {e:?}"
            );
        }
    }

    #[test]
    fn signed_function_vector_matching_arity_assembles() {
        // Width 2 matches the arity-2 signature: both vector kinds pass.
        assert!(
            asm_fake(
                ".routine main, tapes=2, alpha=(3, 5)\n.func main\n    vwrite [1, 2]\n    vmove [<, >]\n    stp\n"
            )
            .is_ok()
        );
    }

    #[test]
    fn unsigned_function_keeps_full_vector_width_freedom() {
        // No `.routine`, so no arity to match: the assembler width-checks
        // nothing and the arch's own 1..=16 freedom stands. A width-3 write
        // and a width-1 move both assemble.
        assert!(asm_fake(".func main\n    vwrite [1, 2, 3]\n    vmove [<]\n    stp\n").is_ok());
    }

    #[test]
    fn move_vector_requires_bracket_form() {
        // No legacy spelled-out-ints form exists for MoveVec.
        let e = asm_fake(".func main\n        vmove 1, 0\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)), "{e}");
        let e = asm_fake(".func main\n        vmove\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)), "{e}");
    }

    // -- `.routine` signature directive ---------------------------------

    use crate::formats::object::RoutineSig;

    #[test]
    fn routine_directive_populates_signatures() {
        let obj = asm_fake(".routine main, tapes=2, alpha=(3, 5)\n.func main\n    stp\n").unwrap();
        assert_eq!(
            obj.signatures,
            Some(vec![RoutineSig {
                arity: 2,
                cardinalities: vec![3, 5],
            }])
        );
        // The signature is v3 data: the object leaves the v2 shape and
        // round-trips through the container's FLAG_HAS_SIGNATURES path.
        assert!(!obj.is_v2_shape());
        let bytes = obj.to_bytes();
        assert_eq!(
            crate::formats::object::ObjectFile::from_bytes(&bytes).unwrap(),
            obj
        );
    }

    #[test]
    fn routine_signatures_parallel_the_named_funcs_blob() {
        // Signatures declared in the OPPOSITE order of the `.func`s:
        // each lands at ITS function's blob index, not declaration order.
        let src = "\
.routine helper, tapes=1, alpha=(2)
.routine main, tapes=2, alpha=(3, 5)
.func main
    stp
.func helper
    stp
";
        let obj = asm_fake(src).unwrap();
        assert_eq!(
            obj.signatures,
            Some(vec![
                RoutineSig {
                    arity: 2,
                    cardinalities: vec![3, 5],
                },
                RoutineSig {
                    arity: 1,
                    cardinalities: vec![2],
                },
            ])
        );
    }

    #[test]
    fn duplicate_routine_for_one_function_is_bad_signature() {
        let src = "\
.routine main, tapes=1, alpha=(2)
.routine main, tapes=1, alpha=(2)
.func main
    stp
";
        let e = asm_fake(src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(_)), "{e}");
        assert_eq!(e.span, Span::new(2, 10, 2, 14)); // the second `main`
    }

    #[test]
    fn routine_that_precedes_no_func_is_bad_signature() {
        // Never defined at all…
        let e = asm_fake(".routine ghost, tapes=1, alpha=(2)\n.func main\n    stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(_)), "{e}");
        assert_eq!(e.span, Span::new(1, 10, 1, 15)); // `ghost`
        // …and defined only BEFORE the directive: the must-precede rule.
        let e = asm_fake(".func main\n    stp\n.routine main, tapes=1, alpha=(2)\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(_)), "{e}");
        assert_eq!(e.span, Span::new(3, 10, 3, 14));
    }

    #[test]
    fn routine_arity_outside_1_to_16_is_bad_signature() {
        let e = asm_fake(".routine main, tapes=0, alpha=(1)\n.func main\n    stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(_)), "{e}");
        assert_eq!(e.span, Span::new(1, 22, 1, 23)); // the `0`
        let e = asm_fake(".routine main, tapes=17, alpha=(1)\n.func main\n    stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(_)), "{e}");
        // The bound itself is fine: 16 tapes assemble.
        let src = format!(
            ".routine main, tapes=16, alpha=({})\n.func main\n    stp\n",
            vec!["2"; 16].join(", ")
        );
        assert!(asm_fake(&src).is_ok());
    }

    #[test]
    fn routine_zero_cardinality_is_bad_signature() {
        let e =
            asm_fake(".routine main, tapes=2, alpha=(3, 0)\n.func main\n    stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(_)), "{e}");
        assert_eq!(e.span, Span::new(1, 31, 1, 37)); // the `(3, 0)` group
    }

    #[test]
    fn routine_alpha_length_must_equal_tapes() {
        let e = asm_fake(".routine main, tapes=2, alpha=(3)\n.func main\n    stp\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(_)), "{e}");
        assert_eq!(e.span, Span::new(1, 31, 1, 34)); // the `(3)` group
    }

    #[test]
    fn signatures_are_all_or_none_per_file() {
        // The MO signature section is parallel to the blobs, so a file
        // that signs any function must sign every function.
        let src = "\
.routine main, tapes=1, alpha=(2)
.func main
    stp
.func helper
    stp
";
        let e = asm_fake(src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(_)), "{e}");
        assert_eq!(e.span, Span::new(4, 7, 4, 13)); // `helper`, the unsigned one
    }

    #[test]
    fn malformed_routine_line_reports_a_precise_syntax_error() {
        for src in [
            ".routine main\n",                          // fields missing
            ".routine main, tapes=2\n",                 // alpha missing
            ".routine main, tapes=02, alpha=(3, 5)\n",  // non-canonical number
            ".routine main, tapes=2, alpha=()\n",       // empty alpha list
            ".routine main, tapes=2, alpha=(3, 5) x\n", // junk after the list
        ] {
            let e = asm_fake(src).unwrap_err();
            assert!(matches!(e.kind, AsmErrorKind::Syntax(_)), "{src:?}: {e}");
        }
    }

    #[test]
    fn routine_in_tables_section_rejected() {
        let e = asm_fake(".section tables\n.routine main, tapes=1, alpha=(2)\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadTable(_)), "{e}");
    }

    // -- Asm ↔ VM wire agreement (frame descriptors) --------------------
    //
    // A property cross-check between the two sides of the frame-descriptor
    // wire format (docs/formats.md (frame descriptors)): an arbitrary valid
    // authored frame is assembled (the real lower + emit path) and then
    // decoded by the VM's `FrameWalk`, and the decoded cache must equal an
    // independent densification of the authored intent. This is the T5
    // review gap — the asm builder and the VM decoder are the two ends of
    // one contract, and nothing forced them to agree byte-for-byte before.
    mod frame_wire_agreement {
        use super::*;
        use proptest::prelude::*;

        /// One authored virtual tape: physical target and sparse maps.
        /// `rmap` pairs carry the one-way (`=>`) flag; `wmap` never does
        /// (one-way is read-direction only).
        #[derive(Debug, Clone)]
        struct TapeSpec {
            phys: u8,
            rmap: Vec<(u16, u16, bool)>,
            wmap: Vec<(u16, u16)>,
        }

        /// Keep the first pair for each source index — mirrors authoring
        /// distinct indices; `build_dense_map` would otherwise let a later
        /// duplicate win, which the reference densifier does not model.
        fn dedup3(v: Vec<(u16, u16, bool)>) -> Vec<(u16, u16, bool)> {
            let mut seen = std::collections::HashSet::new();
            v.into_iter().filter(|&(f, _, _)| seen.insert(f)).collect()
        }
        fn dedup2(v: Vec<(u16, u16)>) -> Vec<(u16, u16)> {
            let mut seen = std::collections::HashSet::new();
            v.into_iter().filter(|&(f, _)| seen.insert(f)).collect()
        }

        /// The independent oracle: the same dense form `build_dense_map`
        /// materializes — index 0 pinned to blank, unset indices are holes
        /// (`0xFFFF`), an empty list is the identity map. Distinct sources
        /// only (see `dedup*`).
        fn densify(pairs: &[(u16, u16)]) -> Vec<u16> {
            if pairs.is_empty() {
                return Vec::new();
            }
            let max = pairs.iter().map(|&(f, _)| f).max().unwrap() as usize;
            let mut t = vec![0xFFFFu16; max + 1];
            t[0] = 0; // blank pins to blank
            for &(f, to) in pairs {
                t[f as usize] = to;
            }
            t
        }

        fn arb_rmap() -> impl Strategy<Value = Vec<(u16, u16, bool)>> {
            // Sparse sources (1..=6) yield natural holes; `to == 0` folds a
            // symbol onto blank; the bool is the `->`/`=>` spelling.
            proptest::collection::vec((1u16..=6, 0u16..=6, any::<bool>()), 0..=4).prop_map(dedup3)
        }
        fn arb_wmap() -> impl Strategy<Value = Vec<(u16, u16)>> {
            proptest::collection::vec((1u16..=6, 0u16..=6), 0..=4).prop_map(dedup2)
        }
        fn arb_tape() -> impl Strategy<Value = TapeSpec> {
            (0u8..=12, arb_rmap(), arb_wmap()).prop_map(|(phys, rmap, wmap)| TapeSpec {
                phys,
                rmap,
                wmap,
            })
        }
        fn arb_frame() -> impl Strategy<Value = (Vec<TapeSpec>, usize)> {
            (proptest::collection::vec(arb_tape(), 1..=16), 0usize..=3)
        }

        /// Render authored `.frame`/`.map`/`.exits` source (fake dialect).
        /// The `fcall` at offset 1 is 9 bytes, so the exit stubs land at
        /// offsets 10, 11, … — pinned below.
        fn render(tapes: &[TapeSpec], n_exits: usize) -> String {
            let mut s = String::from(".section tables\nF0: .frame tapes=(");
            let list: Vec<String> = tapes.iter().map(|t| t.phys.to_string()).collect();
            s.push_str(&list.join(", "));
            s.push_str(")\n");
            for (k, t) in tapes.iter().enumerate() {
                if t.rmap.is_empty() && t.wmap.is_empty() {
                    continue;
                }
                s.push_str(&format!("    .map {k}"));
                if !t.rmap.is_empty() {
                    let r: Vec<String> = t
                        .rmap
                        .iter()
                        .map(|&(f, to, ow)| format!("{f}{}{to}", if ow { "=>" } else { "->" }))
                        .collect();
                    s.push_str(&format!(", rmap=({})", r.join(", ")));
                }
                if !t.wmap.is_empty() {
                    let w: Vec<String> =
                        t.wmap.iter().map(|&(f, to)| format!("{f}->{to}")).collect();
                    s.push_str(&format!(", wmap=({})", w.join(", ")));
                }
                s.push('\n');
            }
            if n_exits > 0 {
                let e: Vec<String> = (0..n_exits).map(|i| format!("E{i}")).collect();
                s.push_str(&format!("    .exits {}\n", e.join(", ")));
            }
            s.push_str(".section code\n.func main\n    fcall helper, F0\n");
            if n_exits > 0 {
                for i in 0..n_exits {
                    s.push_str(&format!("E{i}: stp\n"));
                }
            } else {
                s.push_str("    stp\n");
            }
            s.push_str(".func helper\n    stp\n");
            s
        }

        /// Drive a `FrameWalk` over the built blob (base 0); off-the-end or
        /// malformed is an error, never a panic.
        fn walk(blob: &[u8]) -> Result<crate::vm::frame::FrameDescriptor, ()> {
            use crate::vm::frame::{FrameStep, FrameWalk};
            let mut w = FrameWalk::new(0);
            let mut input = None;
            loop {
                match w.feed(input) {
                    FrameStep::NeedByte(a) => match blob.get(a as usize) {
                        Some(&b) => input = Some(b),
                        None => return Err(()),
                    },
                    FrameStep::Done(d) => return Ok(d),
                    FrameStep::Malformed => return Err(()),
                }
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(96))]

            #[test]
            fn authored_frames_decode_through_framewalk((tapes, n_exits) in arb_frame()) {
                let src = render(&tapes, n_exits);
                let obj = asm_fake(&src).expect("authored frame assembles");
                let blobs = obj.table_blobs.as_ref().expect("has a table section");
                let desc = walk(&blobs[0]).expect("descriptor decodes via FrameWalk");

                // (a) arity + per-tape projection/maps round-trip through the
                //     assembler bytes and back out of the VM's decoder.
                prop_assert_eq!(desc.entries.len(), tapes.len());
                for (k, spec) in tapes.iter().enumerate() {
                    let rpairs: Vec<(u16, u16)> =
                        spec.rmap.iter().map(|&(f, to, _)| (f, to)).collect();
                    prop_assert_eq!(desc.entries[k].phys, spec.phys);
                    prop_assert_eq!(&desc.entries[k].rmap, &densify(&rpairs));
                    prop_assert_eq!(&desc.entries[k].wmap, &densify(&spec.wmap));

                    // (b) every non-empty builder-produced map pins entry 0 to
                    //     blank — the T1 seam invariant.
                    if !desc.entries[k].rmap.is_empty() {
                        prop_assert_eq!(desc.entries[k].rmap[0], 0);
                    }
                    if !desc.entries[k].wmap.is_empty() {
                        prop_assert_eq!(desc.entries[k].wmap[0], 0);
                    }
                }

                // Exits are blob-relative code offsets: ent@0, fcall@1 (9
                // bytes), then one-byte `stp` stubs at 10, 11, ….
                let expected: Vec<u32> = (0..n_exits as u32).map(|i| 10 + i).collect();
                prop_assert_eq!(desc.exits, expected);
            }
        }
    }
}
