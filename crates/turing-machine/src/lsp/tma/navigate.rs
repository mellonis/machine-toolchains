//! Go-to-definition for `.tma`: the operand token under the cursor, resolved
//! from the total CST — never gated on `fatal`/`lint`, so a reference resolves
//! even while something else in the document refuses to assemble.
//!
//! # The four reference shapes
//!
//! - A **label** reference (`jm`/`jnm`/a bare `jmp` target) resolves within the
//!   enclosing `.func` only. Code labels are function-scoped, so a same-named
//!   label in a sibling function is never the answer.
//! - A **callable** reference (`call name`, `call.m name, F`, any `@name`)
//!   resolves doc-wide, preferring the `.func` that defines the body and
//!   falling back to the `.routine` signature — a routine whose body lives in
//!   another translation unit is declared here and defined elsewhere, and
//!   jumping to its signature is the best answer this file can give.
//! - A **table** reference (`mtc T`, `djmp D`) resolves doc-wide to the label
//!   on the directive that opens that table. A table's label sits on its first
//!   `.row`/`.targets`/`.target` line, so the label IS the table's definition
//!   site.
//! - A **frame** reference (a `call.m`'s second operand) resolves doc-wide to
//!   the `.frame` header's own label.
//!
//! Two further directions run the other way, from a table or descriptor into
//! the code it dispatches to: a `.targets`/`.target` entry and a `.exits` target
//! both name a code label. Those are the arrows that make a dispatch-table
//! program navigable at all — without them the table section is a dead end.
//! They resolve doc-wide rather than function-scoped, because a table lives in
//! the tables section, outside any `.func`, and so has no enclosing function to
//! scope against; the first matching label wins. That is an approximation the
//! linker's own binding does not make, and it is stated here rather than hidden:
//! it can only pick the wrong same-named label, never invent one.
//!
//! An operand carrying an unexpanded `.rept` marker (`L{v}`) names a template,
//! not an identifier, and resolves to nothing.

use mtc_core::asm::cst::{AsmItemKind, FrameDirectiveCst};
use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::DefTarget;

use crate::asm::tm1_syntax;

use super::{
    FlatItem, OperandRole, TmaDocState, doc_frames, doc_functions, doc_routines, doc_tables,
    enclosing_function_range, flat_items, is_templated, item_at_line, name_span, operand_name,
    operand_role,
};

/// Half-open span containment, 1-based.
fn span_contains(span: Span, pos: Pos) -> bool {
    pos >= span.start && pos < span.end
}

pub(super) fn definition(state: &TmaDocState, uri: &str, pos: Pos) -> Option<DefTarget> {
    let item = item_at_line(&state.flat, pos.line)?;
    match &item.kind {
        AsmItemKind::Line(line) => {
            let instr = line.instr.as_ref()?;
            let syntax = tm1_syntax();
            let entry = syntax.by_mnemonic(&instr.word)?;
            let (index, operand) = instr
                .operands
                .iter()
                .enumerate()
                .find(|(_, o)| span_contains(o.span, pos))?;
            let name = operand_name(operand);
            if is_templated(name) {
                return None;
            }
            match operand_role(entry, index, &operand.text)? {
                OperandRole::Callable => {
                    let target = callable_span(&state.flat, name)?;
                    Some(DefTarget {
                        uri: uri.to_string(),
                        span: target,
                        origin: Some(name_span(operand)),
                    })
                }
                OperandRole::Label => {
                    let (_, range) = enclosing_function_range(&state.flat, pos.line)?;
                    let target = label_span_in(&state.flat[range], name)?;
                    Some(DefTarget {
                        uri: uri.to_string(),
                        span: target,
                        origin: Some(operand.span),
                    })
                }
                OperandRole::Table => {
                    let (_, span, _) = doc_tables(&state.flat).find(|(n, _, _)| *n == name)?;
                    Some(DefTarget {
                        uri: uri.to_string(),
                        span,
                        origin: Some(operand.span),
                    })
                }
                OperandRole::Frame => {
                    let target = doc_frames(&state.flat).find(|h| h.label.name == name)?;
                    Some(DefTarget {
                        uri: uri.to_string(),
                        span: target.label.span,
                        origin: Some(operand.span),
                    })
                }
            }
        }
        // A dispatch entry or an exit target: a code-label reference pointing
        // out of the tables section and into the code.
        AsmItemKind::TableDirective(d) => {
            let operand = d.operands.iter().find(|o| span_contains(o.span, pos))?;
            code_label_target(state, uri, &operand.text, operand.span)
        }
        AsmItemKind::FrameDirective(FrameDirectiveCst::Exits(e)) => {
            let operand = e.targets.iter().find(|o| span_contains(o.span, pos))?;
            code_label_target(state, uri, &operand.text, operand.span)
        }
        _ => None,
    }
}

/// A code-label reference from the tables section: doc-wide, first match wins
/// (the module doc states why this is an approximation).
fn code_label_target(
    state: &TmaDocState,
    uri: &str,
    text: &str,
    origin: Span,
) -> Option<DefTarget> {
    if is_templated(text) {
        return None;
    }
    let target = label_span_in(&state.flat, text)?;
    Some(DefTarget {
        uri: uri.to_string(),
        span: target,
        origin: Some(origin),
    })
}

/// The span of the first code label named `name` among `flat`.
fn label_span_in(flat: &[FlatItem], name: &str) -> Option<Span> {
    flat_items(flat).find_map(|item| {
        let AsmItemKind::Line(l) = &item.kind else {
            return None;
        };
        l.labels
            .iter()
            .find(|label| label.name == name)
            .map(|label| label.span)
    })
}

/// The definition span for a callable: the `.func` that defines it, or — when
/// this file only declares it — the `.routine` signature's own name span.
fn callable_span(flat: &[FlatItem], name: &str) -> Option<Span> {
    if let Some(f) = doc_functions(flat).find(|f| f.name == name) {
        return Some(f.name_span);
    }
    doc_routines(flat)
        .find(|r| r.name == name)
        .map(|r| r.name_span)
}
