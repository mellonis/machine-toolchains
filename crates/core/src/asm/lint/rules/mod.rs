//! One file per assembly lint rule (docs/core.md (assembly lint)). Each rule exposes
//! `pub(crate) fn check(&AsmLintContext, &mut Vec<Diagnostic>)` and is
//! registered in `super::RULES` under its defect-named code.

pub(crate) mod leftover_debugger;
pub(crate) mod line_too_long;
pub(crate) mod redundant_jump;
pub(crate) mod unreachable_code;
pub(crate) mod unused_label;

use crate::asm::cst::{AsmCst, AsmItemKind, LineCst};
use crate::diagnostics::Span;

/// The label-preserving "delete this instruction" edit shared by
/// `redundant-jump-to-next` and `leftover-debugger`: both rules delete
/// one instruction item and must not orphan any label bound to its
/// line. When the line carries no label, the whole physical line
/// (including its trailing newline) goes; when it carries one or more,
/// only the instruction portion goes — from the instruction word's own
/// start column through the line's trimmed end — leaving the label(s)
/// on a now label-only line, binding forward to whatever follows
/// (docs/formats.md (assembly text), label-only lines).
///
/// `item_span` must be a `SourceItem::Instr` span, which lowering sets
/// verbatim from the owning `LineCst.span` (`lower.rs`), so it is an
/// exact key into the CST. Falls back to deleting just `item_span` if
/// no such line is found (defensive; every caller's item span is a
/// real line span in practice).
pub(crate) fn delete_instruction_edit_span(cst: &AsmCst, item_span: Span) -> Span {
    let Some(line) = find_line(cst, item_span) else {
        return item_span;
    };
    if line.labels.is_empty() {
        return Span::new(item_span.start.line, 1, item_span.start.line + 1, 1);
    }
    let Some(instr) = &line.instr else {
        return item_span;
    };
    Span::new(
        instr.word_span.start.line,
        instr.word_span.start.col,
        line.span.end.line,
        line.span.end.col,
    )
}

fn find_line(cst: &AsmCst, span: Span) -> Option<&LineCst> {
    cst.items.iter().find_map(|item| match &item.kind {
        AsmItemKind::Line(line) if line.span == span => Some(line),
        _ => None,
    })
}
