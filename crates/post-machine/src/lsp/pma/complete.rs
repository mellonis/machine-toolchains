//! Completions (docs/lsp.md (completions)) for `.pma`: four contexts
//! classified from the total CST at the cursor's own line
//! (`item_at_line`) plus that line's own word/operand spans — never
//! gated on `fatal`/`lint`, so completion answers over a document that
//! fails to assemble the same way `document_symbols`/`semantic_tokens`
//! do (docs/formats.md (assembly text); total CST).
//!
//! # Context detection order
//!
//! 1. **Instruction-word position** — nothing on the line yet (a blank
//!    line, a label with nothing after it, or the cursor sitting before
//!    the line's own word even starts), or the cursor on/touching the
//!    line's own instruction word (however it resolves, known mnemonic
//!    or not — an in-progress edit is exactly where this context is most
//!    useful): every `pm1_syntax()` mnemonic plus the `.byte`/`.func`
//!    directives.
//! 2. **Operand position, right after `@`** — the cursor sits on/
//!    touching an operand whose text starts with `@`: the doc's `.func`
//!    names, replacing the name portion only (never the `@` sigil
//!    itself — mirrors `.pmc`'s own call-site spans).
//! 3. **Operand position, `Jump`/`Branch` flow, no `@`** — the line's
//!    word resolves (`pm1_syntax()`) to a `RelI8`/`RelI32` entry with
//!    `Jump`/`Branch` flow: the ENCLOSING function's own labels
//!    (`enclosing_function_range`) — labels are function-scoped.
//! 4. **Operand position, `Call` flow** — same operand kind, `Call` flow
//!    (`call`/`call.s`): the doc's `.func` names.
//!
//! No match (an unknown mnemonic, a `SymbolVec`/`None` operand, a
//! `Func`/`Raw`/`Comment` line, or a blank/EOF position) → empty, the
//! same "no match → empty" rule `.pmc`'s own `complete.rs` documents.

use std::collections::BTreeSet;

use mtc_core::asm::cst::AsmItemKind;
use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::{Candidate, CandidateKind};

use crate::asm::pm1_syntax;

use super::{
    OperandRole, PmaDocState, doc_function_names, enclosing_function_range, item_at_line,
    item_lines, name_span, operand_role,
};

pub(super) fn completion(state: &PmaDocState, pos: Pos) -> Vec<Candidate> {
    let lines = item_lines(&state.text, &state.cst);
    let Some(item) = item_at_line(&state.cst, &lines, pos.line) else {
        return word_position_candidates(zero_span(pos));
    };
    let AsmItemKind::Line(line) = &item.kind else {
        return Vec::new();
    };
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

    // Past the word: operand position. `by_mnemonic` and `operand_role`
    // both gate on the operand kind (RelI8/RelI32 only) — an unknown
    // mnemonic or a SymbolVec/None-operand entry (`wr`, or a no-operand
    // mnemonic) falls straight through to no candidates.
    let syntax = pm1_syntax();
    let Some(entry) = syntax.by_mnemonic(&instr.word) else {
        return Vec::new();
    };
    let current = instr.operands.first().filter(|o| touches(o.span, pos));
    let text_so_far = current.map_or("", |o| o.text.as_str());
    match operand_role(entry, text_so_far) {
        Some(OperandRole::Function) if text_so_far.starts_with('@') => {
            let op = current.expect("a non-empty prefix implies a current operand token");
            function_candidates(state, name_span(op))
        }
        Some(OperandRole::Function) => {
            let replace = current.map_or_else(|| zero_span(pos), |o| o.span);
            function_candidates(state, replace)
        }
        Some(OperandRole::Label) => {
            let replace = current.map_or_else(|| zero_span(pos), |o| o.span);
            enclosing_label_candidates(state, pos.line, &lines, replace)
        }
        None => Vec::new(),
    }
}

/// Whole-token touch, mirroring `.pmc`'s own `prefix_anchor`: `pos`
/// inside the span, or exactly touching either end, counts — the same
/// rule that lets `replace_span` cover the whole token an in-progress
/// edit is sitting on/against, filtered client-side by the already-typed
/// prefix.
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
    Candidate {
        label: label.to_string(),
        kind,
        replace_span,
        insert_text: label.to_string(),
    }
}

/// Context 1: every `pm1_syntax()` mnemonic plus the `.byte`/`.func`
/// directives — all Keyword-kind, sharing `replace_span`.
fn word_position_candidates(replace_span: Span) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = pm1_syntax()
        .entries
        .iter()
        .map(|e| mk_candidate(e.mnemonic, CandidateKind::Keyword, replace_span))
        .collect();
    out.push(mk_candidate(".byte", CandidateKind::Keyword, replace_span));
    out.push(mk_candidate(".func", CandidateKind::Keyword, replace_span));
    out
}

/// Contexts 2 and 4: every `.func` name declared anywhere in the
/// document (exported and local alike), sorted and deduplicated.
fn function_candidates(state: &PmaDocState, replace_span: Span) -> Vec<Candidate> {
    doc_function_names(&state.cst)
        .into_iter()
        .map(|name| mk_candidate(name, CandidateKind::Function, replace_span))
        .collect()
}

/// Context 3: the ENCLOSING function's own labels only — never a
/// doc-wide list, since labels are function-scoped. `Value`-kind,
/// mirroring `.pmc`'s own `goto`-target label candidates.
fn enclosing_label_candidates(
    state: &PmaDocState,
    line: u32,
    lines: &[u32],
    replace_span: Span,
) -> Vec<Candidate> {
    let Some((_, range)) = enclosing_function_range(&state.cst, lines, line) else {
        return Vec::new();
    };
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::new();
    for item in &state.cst.items[range] {
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

#[cfg(test)]
mod tests {
    use super::super::PmaLanguageService;
    use super::*;
    use mtc_core::lsp::LanguageService;

    const URI: &str = "untitled:Complete-1";

    fn mnemonic_and_directive_count() -> usize {
        pm1_syntax().entries.len() + 2 // + `.byte`, `.func`
    }

    #[test]
    fn mnemonic_list_at_a_blank_line_start() {
        let mut service = PmaLanguageService::new();
        service.did_update(URI, "");

        let pos = Pos { line: 1, col: 1 };
        let candidates = service.completion(URI, pos);

        assert_eq!(
            candidates.len(),
            mnemonic_and_directive_count(),
            "{candidates:?}"
        );
        assert!(candidates.iter().any(|c| c.label == "jm"));
        assert!(candidates.iter().any(|c| c.label == ".byte"));
        assert!(candidates.iter().any(|c| c.label == ".func"));
        assert!(candidates.iter().all(|c| c.kind == CandidateKind::Keyword));
        for c in &candidates {
            assert_eq!(
                c.replace_span,
                Span {
                    start: pos,
                    end: pos
                }
            );
            assert_eq!(c.insert_text, c.label);
        }
    }

    #[test]
    fn mnemonic_list_after_a_label_with_nothing_following_it() {
        let mut service = PmaLanguageService::new();
        let src = ".func f\nL1: ";
        service.did_update(URI, src);

        let pos = Pos { line: 2, col: 5 }; // right after "L1: "
        let candidates = service.completion(URI, pos);

        assert_eq!(
            candidates.len(),
            mnemonic_and_directive_count(),
            "{candidates:?}"
        );
        assert!(candidates.iter().all(|c| c.kind == CandidateKind::Keyword));
        for c in &candidates {
            assert_eq!(
                c.replace_span,
                Span {
                    start: pos,
                    end: pos
                }
            );
        }
    }

    #[test]
    fn mnemonic_list_still_answers_on_the_broken_lines_own_word() {
        // `bogus` is an unknown mnemonic, but the CST still shapes it as
        // an instruction word — the cursor touching it is still context
        // 1, offering the very list that would fix it (total CST).
        let mut service = PmaLanguageService::new();
        let diags = service.did_update(URI, ".func f\n        bogus\n");
        assert_eq!(diags[0].code, Some("unknown-mnemonic"), "sanity");

        let pos = Pos { line: 2, col: 14 }; // touching the end of "bogus"
        let candidates = service.completion(URI, pos);

        assert_eq!(
            candidates.len(),
            mnemonic_and_directive_count(),
            "{candidates:?}"
        );
        assert_eq!(
            candidates[0].replace_span,
            Span::new(2, 9, 2, 14),
            "the whole `bogus` word"
        );
    }

    #[test]
    fn at_completion_lists_every_doc_function() {
        let mut service = PmaLanguageService::new();
        let src = ".func f\n        jmp     @\n.func g\n        ret\n";
        service.did_update(URI, src);

        let pos = Pos { line: 2, col: 18 }; // right after the `@`
        let candidates = service.completion(URI, pos);

        assert_eq!(
            candidates,
            vec![
                mk_candidate("f", CandidateKind::Function, Span::new(2, 18, 2, 18)),
                mk_candidate("g", CandidateKind::Function, Span::new(2, 18, 2, 18)),
            ]
        );
    }

    #[test]
    fn branch_operand_lists_only_the_enclosing_functions_labels() {
        let mut service = PmaLanguageService::new();
        let src = ".func f\nA: rgt\n        jm \n.func g\nB: rgt\n        ret\n";
        service.did_update(URI, src);

        let pos = Pos { line: 3, col: 12 }; // past `jm`'s own word, nothing typed yet
        let candidates = service.completion(URI, pos);

        assert_eq!(
            candidates,
            vec![mk_candidate(
                "A",
                CandidateKind::Value,
                Span::new(3, 12, 3, 12)
            )],
            "only f's own label A — never g's B"
        );
    }

    #[test]
    fn call_operand_lists_every_doc_function() {
        let mut service = PmaLanguageService::new();
        let src = ".func helper\n        ret\n.func main\n        call \n";
        service.did_update(URI, src);

        let pos = Pos { line: 4, col: 14 }; // past `call`'s own word, nothing typed yet
        let candidates = service.completion(URI, pos);

        assert_eq!(
            candidates,
            vec![
                mk_candidate("helper", CandidateKind::Function, Span::new(4, 14, 4, 14)),
                mk_candidate("main", CandidateKind::Function, Span::new(4, 14, 4, 14)),
            ]
        );
    }

    #[test]
    fn no_context_matches_for_a_no_operand_mnemonic() {
        let mut service = PmaLanguageService::new();
        let src = ".func f\n        stp \n";
        service.did_update(URI, src);

        let pos = Pos { line: 2, col: 13 }; // past `stp`, in its (nonexistent) operand slot
        assert_eq!(service.completion(URI, pos), Vec::new());
    }
}
