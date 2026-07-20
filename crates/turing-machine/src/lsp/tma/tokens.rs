//! Semantic tokens for `.tma`: walked straight off the total CST, so this
//! always answers a full stream even over a document that fails to assemble.
//!
//! The legend adds one distinction PM-1 assembly has no need for: a table or
//! frame label rides `type` rather than `variable`, because a dispatch table
//! and a frame descriptor are data structures, not jump targets, and a reader
//! scanning the tables section benefits from seeing them apart from the code
//! labels they point at.
//!
//! An unresolved reference emits nothing for the name — the same quiet cue the
//! sibling PM-1 service uses. So does a templated `.rept` operand: it names no
//! identifier the CST can resolve, and colouring it as one would be a lie.

use std::collections::BTreeSet;

use mtc_core::asm::cst::{AsmItemKind, FrameDirectiveCst, InstrCst, LineCst, TableDirectiveKind};
use mtc_core::lsp::SemToken;
use mtc_core::vm::OperandKind;

use crate::asm::tm1_syntax;

use super::{
    FlatItem, MODIFIER_DECLARATION, OperandRole, TOKEN_TYPE_FUNCTION, TOKEN_TYPE_NUMBER,
    TOKEN_TYPE_TYPE, TOKEN_TYPE_VARIABLE, TmaDocState, doc_callable_names, doc_frames, doc_tables,
    flat_items, is_templated, name_span, operand_name, operand_role,
};

pub(super) fn semantic_tokens(state: &TmaDocState) -> Vec<SemToken> {
    let syntax = tm1_syntax();
    let callables = doc_callable_names(&state.flat);
    let tables: BTreeSet<&str> = doc_tables(&state.flat).map(|(name, _, _)| name).collect();
    let frames: BTreeSet<&str> = doc_frames(&state.flat)
        .map(|h| h.label.name.as_str())
        .collect();
    let code_labels = code_label_names(&state.flat);

    let mut out = Vec::new();
    for item in flat_items(&state.flat) {
        match &item.kind {
            AsmItemKind::Func(f) => out.push(SemToken {
                span: f.name_span,
                token_type: TOKEN_TYPE_FUNCTION,
                modifiers: MODIFIER_DECLARATION,
            }),
            AsmItemKind::RoutineDirective(r) => out.push(SemToken {
                span: r.name_span,
                token_type: TOKEN_TYPE_FUNCTION,
                modifiers: MODIFIER_DECLARATION,
            }),
            AsmItemKind::TableDirective(d) => {
                for label in &d.labels {
                    out.push(SemToken {
                        span: label.span,
                        token_type: TOKEN_TYPE_TYPE,
                        modifiers: MODIFIER_DECLARATION,
                    });
                }
                match d.kind {
                    // A `.row` operand is a symbol vector — numbers and
                    // wildcards, never a name.
                    TableDirectiveKind::Row => {
                        for operand in &d.operands {
                            out.push(SemToken {
                                span: operand.span,
                                token_type: TOKEN_TYPE_NUMBER,
                                modifiers: 0,
                            });
                        }
                    }
                    TableDirectiveKind::Targets | TableDirectiveKind::Target => {
                        for operand in &d.operands {
                            push_code_label_ref(&code_labels, &operand.text, operand.span, &mut out);
                        }
                    }
                }
            }
            AsmItemKind::FrameDirective(FrameDirectiveCst::Header(h)) => out.push(SemToken {
                span: h.label.span,
                token_type: TOKEN_TYPE_TYPE,
                modifiers: MODIFIER_DECLARATION,
            }),
            AsmItemKind::FrameDirective(FrameDirectiveCst::Exits(e)) => {
                for operand in &e.targets {
                    push_code_label_ref(&code_labels, &operand.text, operand.span, &mut out);
                }
            }
            AsmItemKind::Line(line) => emit_line(
                line,
                &syntax,
                &callables,
                &tables,
                &frames,
                &code_labels,
                &mut out,
            ),
            AsmItemKind::FrameDirective(FrameDirectiveCst::Map(_))
            | AsmItemKind::Section(_)
            | AsmItemKind::Rept(_)
            | AsmItemKind::Raw(_)
            | AsmItemKind::Comment(_) => {}
        }
    }
    out.sort_by_key(|t| t.span.start);
    out
}

/// Every code label defined anywhere in the document.
fn code_label_names(flat: &[FlatItem]) -> BTreeSet<&str> {
    flat_items(flat)
        .filter_map(|item| match &item.kind {
            AsmItemKind::Line(l) => Some(&l.labels),
            _ => None,
        })
        .flatten()
        .map(|label| label.name.as_str())
        .collect()
}

fn push_code_label_ref(
    code_labels: &BTreeSet<&str>,
    text: &str,
    span: mtc_core::diagnostics::Span,
    out: &mut Vec<SemToken>,
) {
    if is_templated(text) || !code_labels.contains(text) {
        return;
    }
    out.push(SemToken {
        span,
        token_type: TOKEN_TYPE_VARIABLE,
        modifiers: 0,
    });
}

#[allow(clippy::too_many_arguments)]
fn emit_line(
    line: &LineCst,
    syntax: &mtc_core::asm::ArchSyntax,
    callables: &BTreeSet<&str>,
    tables: &BTreeSet<&str>,
    frames: &BTreeSet<&str>,
    code_labels: &BTreeSet<&str>,
    out: &mut Vec<SemToken>,
) {
    for label in &line.labels {
        out.push(SemToken {
            span: label.span,
            token_type: TOKEN_TYPE_VARIABLE,
            modifiers: MODIFIER_DECLARATION,
        });
    }
    let Some(instr) = &line.instr else {
        return;
    };
    let Some(entry) = syntax.by_mnemonic(&instr.word) else {
        // An unknown mnemonic has no entry to classify operands against; the
        // line's own label definitions above are all it contributes.
        return;
    };

    // Vector and immediate operands are values, not names.
    if matches!(
        entry.operand,
        OperandKind::SymbolVec
            | OperandKind::MoveVec
            | OperandKind::WriteMoveVec
            | OperandKind::Imm8
    ) {
        emit_value_operands(instr, out);
        return;
    }

    for (index, operand) in instr.operands.iter().enumerate() {
        let name = operand_name(operand);
        if is_templated(name) {
            continue;
        }
        match operand_role(entry, index, &operand.text) {
            Some(OperandRole::Label) => {
                push_code_label_ref(code_labels, name, operand.span, out);
            }
            Some(OperandRole::Callable) => {
                if callables.contains(name) {
                    out.push(SemToken {
                        span: name_span(operand),
                        token_type: TOKEN_TYPE_FUNCTION,
                        modifiers: 0,
                    });
                }
            }
            Some(OperandRole::Table) => {
                if tables.contains(name) {
                    out.push(SemToken {
                        span: operand.span,
                        token_type: TOKEN_TYPE_TYPE,
                        modifiers: 0,
                    });
                }
            }
            Some(OperandRole::Frame) => {
                if frames.contains(name) {
                    out.push(SemToken {
                        span: operand.span,
                        token_type: TOKEN_TYPE_TYPE,
                        modifiers: 0,
                    });
                }
            }
            None => {}
        }
    }
}

fn emit_value_operands(instr: &InstrCst, out: &mut Vec<SemToken>) {
    for operand in &instr.operands {
        out.push(SemToken {
            span: operand.span,
            token_type: TOKEN_TYPE_NUMBER,
            modifiers: 0,
        });
    }
}
