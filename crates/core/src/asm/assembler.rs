//! Two-pass assembly with short/far relaxation (docs/formats.md (assembly
//! text); docs/isa.md for the opcode/relaxation table this assembles
//! against).

use std::collections::HashMap;

use super::lower::{SourceFunction, SourceItem, SourceOperand, lower};
use super::syntax::{ArchSyntax, Flow};
use super::{AsmError, AsmErrorKind};
use crate::diagnostics::Span;
use crate::formats::object::{BlobDebug, ObjectFile, Relocation, Symbol, SymbolDef};
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
            Slot::Call { .. } => 5,
        }
    }
}

pub fn assemble(
    syntax: &ArchSyntax,
    arch_id: u8,
    source: &str,
    with_debug: bool,
) -> Result<ObjectFile, AsmError> {
    let functions = lower(&super::cst::parse_asm_cst(source), syntax)?;

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

    for (blob_idx, function) in functions.iter().enumerate() {
        let (blob, relocs, blob_debug) = assemble_function(
            syntax,
            function,
            blob_idx as u32,
            &mut symbols,
            &mut symbol_index,
        )?;
        blobs.push(blob);
        relocations.extend(relocs);
        if let Some(d) = debug.as_mut() {
            d.push(blob_debug);
        }
    }

    Ok(ObjectFile {
        arch: arch_id,
        symbols,
        blobs,
        relocations,
        debug,
    })
}

fn assemble_function(
    syntax: &ArchSyntax,
    function: &SourceFunction,
    blob_idx: u32,
    symbols: &mut Vec<Symbol>,
    symbol_index: &mut HashMap<String, u32>,
) -> Result<(Vec<u8>, Vec<Relocation>, BlobDebug), AsmError> {
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
            for (i, slot) in slots.iter().enumerate() {
                // The MO debug section stores a source line (docs/formats.md
                // (MO)); an operand shares its instruction's line, so a
                // Call reads its symbol_span's line.
                lines.push((
                    starts[i],
                    match slot {
                        Slot::Fixed { span, .. } | Slot::Jump { span, .. } => span.start.line,
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
                }
            }
            let labels = label_slot
                .iter()
                .map(|(name, &i)| (name.clone(), starts[i]))
                .collect::<Vec<_>>();
            let mut labels = labels;
            labels.sort_by(|a, b| (a.1, a.0.as_str()).cmp(&(b.1, b.0.as_str())));
            return Ok((blob, relocs, BlobDebug { labels, lines }));
        }
    }
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
}
