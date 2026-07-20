//! Completions for `.tma`: contexts classified from the total CST at the
//! cursor's own line plus that line's own word and operand spans — never gated
//! on `fatal`/`lint`, so completion answers over a document that fails to
//! assemble, which is where an in-progress edit spends most of its life.
//!
//! # Context detection order
//!
//! 1. **Instruction-word position** — nothing on the line yet (a blank line, a
//!    label with nothing after it, a cursor before the word starts), or the
//!    cursor on the line's own word however it resolves: every `tm1_syntax()`
//!    mnemonic plus the dialect's directives.
//! 2. **Operand position, right after `@`** — the doc's callable names,
//!    replacing the name portion only, never the sigil.
//! 3. **Operand position by role** ([`super::operand_role`]) — a label operand
//!    offers the ENCLOSING function's own labels (labels are function-scoped);
//!    a callable operand offers the doc's `.func`/`.routine` names; a table
//!    operand offers the labeled tables; a frame operand offers the `.frame`
//!    descriptors.
//! 4. **Inside the tables section** — a `.targets`/`.target` entry and a
//!    `.exits` target both name a code label, so both offer the document's code
//!    labels. These are the completions that make authoring a dispatch table
//!    bearable; they are doc-wide for the same reason go-to-definition is
//!    (a table has no enclosing function to scope against).
//!
//! No match — an unknown mnemonic, an immediate or vector operand, a
//! `Func`/`Raw`/`Comment` line, a blank position — yields an empty list.
//!
//! # Operand hints
//!
//! A mnemonic candidate's `detail` is derived from its entry's operand kind and
//! flow alone, one mapping for the whole table, so a mnemonic added to
//! `tm1_syntax()` gets a hint without a per-mnemonic case here.

use std::collections::BTreeSet;

use mtc_core::asm::cst::{AsmItemKind, FrameDirectiveCst};
use mtc_core::asm::{Flow, SyntaxEntry};
use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::{Candidate, CandidateKind};
use mtc_core::vm::OperandKind;

use crate::asm::tm1_syntax;

use super::{
    FlatItem, OperandRole, TmaDocState, doc_callable_names, doc_frames, doc_tables,
    enclosing_function_range, flat_items, item_at_line, name_span, operand_role, table_kind_word,
};

/// The dialect's directives and their operand hints. These have no
/// `SyntaxEntry` of their own to derive a hint from, so each carries its own
/// fixed string.
const DIRECTIVES: &[(&str, &str)] = &[
    (".func", ".func <name> [local]"),
    (".routine", ".routine <name>, tapes=<n>, alpha=(<n>, …)"),
    (".section", ".section code|tables"),
    (".row", ".row [<symbol>, …]"),
    (".targets", ".targets <label>, …"),
    (".target", ".target <label>"),
    (".frame", ".frame tapes=(<n>, …)"),
    (".map", ".map <k>[, rmap=(<a>-><b>, …)][, wmap=(…)]"),
    (".exits", ".exits <label>, …"),
    (".rept", ".rept <var>, <lo>, <hi>"),
    (".endr", ".endr"),
];

pub(super) fn completion(state: &TmaDocState, pos: Pos) -> Vec<Candidate> {
    let Some(item) = item_at_line(&state.flat, pos.line) else {
        return word_position_candidates(zero_span(pos));
    };
    match &item.kind {
        AsmItemKind::Line(line) => {
            let Some(instr) = &line.instr else {
                return word_position_candidates(zero_span(pos));
            };
            if pos.col <= instr.word_span.end.col {
                let replace = if pos.col >= instr.word_span.start.col {
                    instr.word_span
                } else {
                    zero_span(pos)
                };
                return word_position_candidates(replace);
            }
            operand_candidates(state, instr, pos)
        }
        // A dispatch entry or an exit target: code-label position.
        AsmItemKind::TableDirective(d) => {
            let replace = d
                .operands
                .iter()
                .find(|o| touches(o.span, pos))
                .map_or_else(|| zero_span(pos), |o| o.span);
            match d.kind {
                mtc_core::asm::cst::TableDirectiveKind::Row => Vec::new(),
                _ => code_label_candidates(&state.flat, replace),
            }
        }
        AsmItemKind::FrameDirective(FrameDirectiveCst::Exits(e)) => {
            let replace = e
                .targets
                .iter()
                .find(|o| touches(o.span, pos))
                .map_or_else(|| zero_span(pos), |o| o.span);
            code_label_candidates(&state.flat, replace)
        }
        _ => Vec::new(),
    }
}

/// Past the instruction word: classify by which operand slot the cursor is in.
/// A cursor past the last written operand belongs to the NEXT slot, so typing
/// `call.m helper, ` offers frames rather than repeating the callable list.
fn operand_candidates(
    state: &TmaDocState,
    instr: &mtc_core::asm::cst::InstrCst,
    pos: Pos,
) -> Vec<Candidate> {
    let syntax = tm1_syntax();
    let Some(entry) = syntax.by_mnemonic(&instr.word) else {
        return Vec::new();
    };
    let touched = instr
        .operands
        .iter()
        .enumerate()
        .find(|(_, o)| touches(o.span, pos));
    let (index, current) = match touched {
        Some((i, o)) => (i, Some(o)),
        // Not on a written operand: the slot after the last one that ends
        // before the cursor.
        None => (
            instr
                .operands
                .iter()
                .filter(|o| o.span.end.col <= pos.col)
                .count(),
            None,
        ),
    };
    let text_so_far = current.map_or("", |o| o.text.as_str());
    let replace = match current {
        Some(o) if text_so_far.starts_with('@') => name_span(o),
        Some(o) => o.span,
        None => zero_span(pos),
    };
    match operand_role(entry, index, text_so_far) {
        Some(OperandRole::Callable) => callable_candidates(state, replace),
        Some(OperandRole::Label) => enclosing_label_candidates(state, pos.line, replace),
        Some(OperandRole::Table) => table_candidates(state, replace),
        Some(OperandRole::Frame) => frame_candidates(state, replace),
        None => Vec::new(),
    }
}

/// Whole-token touch: the cursor inside the span or exactly at either end, so
/// `replace_span` covers the token an in-progress edit is sitting against.
fn touches(span: Span, pos: Pos) -> bool {
    pos.line == span.start.line && pos.col >= span.start.col && pos.col <= span.end.col
}

fn zero_span(pos: Pos) -> Span {
    Span {
        start: pos,
        end: pos,
    }
}

fn mk_candidate(label: &str, kind: CandidateKind, replace_span: Span) -> Candidate {
    mk_candidate_with_detail(label, kind, replace_span, None)
}

fn mk_candidate_with_detail(
    label: &str,
    kind: CandidateKind,
    replace_span: Span,
    detail: Option<String>,
) -> Candidate {
    Candidate {
        label: label.to_string(),
        kind,
        replace_span,
        insert_text: label.to_string(),
        detail,
        // Assembly text has no attribute grammar, so nothing here is ever
        // deprecation-tagged.
        deprecated: false,
    }
}

/// The operand-hint `detail` for a mnemonic candidate, from its operand kind
/// and flow alone.
fn operand_hint_detail(entry: &SyntaxEntry) -> Option<String> {
    let shape = match entry.operand {
        OperandKind::None => return None,
        OperandKind::SymbolVec => "[<symbols>]",
        OperandKind::MoveVec => "[<moves>]",
        OperandKind::WriteMoveVec => "[<symbols>], [<moves>]",
        OperandKind::TableRef => "<table>",
        OperandKind::Imm8 => "#<n>",
        OperandKind::FramedCall => "<target>, <frame>",
        OperandKind::RelI8 | OperandKind::RelI32 => match entry.flow {
            Flow::Call => "<function>",
            Flow::Jump | Flow::Branch => "<label>",
            // No relative-operand entry in `tm1_syntax()` carries a
            // fall-through or stop flow today; degrade to no hint rather than
            // assert, so an arch addition that did loses a hint instead of
            // taking the server down.
            Flow::FallThrough | Flow::Stop => return None,
        },
    };
    Some(format!("{} {shape}", entry.mnemonic))
}

/// Context 1: every mnemonic plus the dialect's directives, all Keyword-kind.
fn word_position_candidates(replace_span: Span) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = tm1_syntax()
        .entries
        .iter()
        .map(|e| {
            mk_candidate_with_detail(
                e.mnemonic,
                CandidateKind::Keyword,
                replace_span,
                operand_hint_detail(e),
            )
        })
        .collect();
    out.extend(DIRECTIVES.iter().map(|(name, hint)| {
        mk_candidate_with_detail(
            name,
            CandidateKind::Keyword,
            replace_span,
            Some((*hint).to_string()),
        )
    }));
    out
}

/// Every `.func`/`.routine` name in the document, sorted and deduplicated.
fn callable_candidates(state: &TmaDocState, replace_span: Span) -> Vec<Candidate> {
    doc_callable_names(&state.flat)
        .into_iter()
        .map(|name| mk_candidate(name, CandidateKind::Function, replace_span))
        .collect()
}

/// Every labeled table, with its own directive keyword as the hint so a `.row`
/// match table and a `.targets` dispatch table are told apart in the menu.
fn table_candidates(state: &TmaDocState, replace_span: Span) -> Vec<Candidate> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::new();
    for (name, _, directive) in doc_tables(&state.flat) {
        if seen.insert(name) {
            out.push(mk_candidate_with_detail(
                name,
                CandidateKind::Module,
                replace_span,
                Some(table_kind_word(directive.kind).to_string()),
            ));
        }
    }
    out
}

/// Every `.frame` descriptor, with its arity as the hint.
fn frame_candidates(state: &TmaDocState, replace_span: Span) -> Vec<Candidate> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::new();
    for header in doc_frames(&state.flat) {
        if seen.insert(header.label.name.as_str()) {
            out.push(mk_candidate_with_detail(
                &header.label.name,
                CandidateKind::Module,
                replace_span,
                Some(format!(".frame, {} tapes", header.tapes.len())),
            ));
        }
    }
    out
}

/// Context 3: the enclosing function's own labels only.
fn enclosing_label_candidates(
    state: &TmaDocState,
    line: u32,
    replace_span: Span,
) -> Vec<Candidate> {
    let Some((_, range)) = enclosing_function_range(&state.flat, line) else {
        return Vec::new();
    };
    label_candidates(&state.flat[range], replace_span)
}

/// Context 4: every code label in the document (a table has no enclosing
/// function to scope against).
fn code_label_candidates(flat: &[FlatItem], replace_span: Span) -> Vec<Candidate> {
    label_candidates(flat, replace_span)
}

fn label_candidates(flat: &[FlatItem], replace_span: Span) -> Vec<Candidate> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::new();
    for item in flat_items(flat) {
        if let AsmItemKind::Line(l) = &item.kind {
            for label in &l.labels {
                if seen.insert(label.name.as_str()) {
                    out.push(mk_candidate(
                        &label.name,
                        CandidateKind::Value,
                        replace_span,
                    ));
                }
            }
        }
    }
    out
}
