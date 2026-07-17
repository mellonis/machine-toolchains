//! Semantic tokens (docs/lsp.md (semantic tokens)) for `.pma`: walked
//! straight off the total `AsmCst` — no analysis tier to gate on the way
//! `.pmc`'s does, so this always answers a full stream, even over a
//! document that fails to assemble (docs/formats.md (assembly text);
//! total CST). `.pma`'s legend carries `defaultLibrary` only for
//! shape-symmetry with `.pmc`'s own (docs/lsp.md) — there is no
//! stdlib-call notion in assembly text, so this module never emits that
//! bit.

use std::collections::BTreeSet;

use mtc_core::asm::ArchSyntax;
use mtc_core::asm::cst::{AsmItemKind, FuncCst, LineCst};
use mtc_core::lsp::SemToken;
use mtc_core::vm::OperandKind;

use crate::asm::pm1_syntax;

use super::{
    MODIFIER_DECLARATION, OperandRole, PmaDocState, TOKEN_TYPE_FUNCTION, TOKEN_TYPE_NUMBER,
    TOKEN_TYPE_VARIABLE, doc_function_names, name_span, operand_role,
};

pub(super) fn semantic_tokens(state: &PmaDocState) -> Vec<SemToken> {
    let syntax = pm1_syntax();
    let functions = doc_function_names(&state.cst);
    let mut out = Vec::new();
    for item in &state.cst.items {
        match &item.kind {
            AsmItemKind::Func(f) => emit_func(f, &mut out),
            AsmItemKind::Line(line) => emit_line(line, &syntax, &functions, &mut out),
            // Opt-in caps nodes never appear under PM-1's default caps;
            // no semantic tokens to emit for them here.
            AsmItemKind::Comment(_)
            | AsmItemKind::Raw(_)
            | AsmItemKind::Section(_)
            | AsmItemKind::TableDirective(_)
            | AsmItemKind::Rept(_)
            | AsmItemKind::RoutineDirective(_)
            | AsmItemKind::FrameDirective(_) => {}
        }
    }
    out.sort_by_key(|t| t.span.start);
    out
}

fn emit_func(f: &FuncCst, out: &mut Vec<SemToken>) {
    out.push(SemToken {
        span: f.name_span,
        token_type: TOKEN_TYPE_FUNCTION,
        modifiers: MODIFIER_DECLARATION,
    });
}

/// One line: its own label definitions, then (for a resolved mnemonic)
/// its operands — numeric (`.byte`'s raw value, `wr`'s `SymbolVec`
/// indices), a label reference, or a function-symbol reference, per
/// [`super::operand_role`]. An unknown mnemonic contributes nothing past
/// its own label definitions (no entry to classify operands against).
fn emit_line(
    line: &LineCst,
    syntax: &ArchSyntax,
    functions: &BTreeSet<&str>,
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

    // `.byte` is a synthetic directive with no `pm1_syntax()` entry of
    // its own; `wr`'s SymbolVec operands are indices, not names — both
    // are numbers, never a label or function reference.
    let entry = syntax.by_mnemonic(&instr.word);
    let is_numeric =
        instr.word == ".byte" || entry.is_some_and(|e| e.operand == OperandKind::SymbolVec);
    if is_numeric {
        for operand in &instr.operands {
            out.push(SemToken {
                span: operand.span,
                token_type: TOKEN_TYPE_NUMBER,
                modifiers: 0,
            });
        }
        return;
    }

    let Some(entry) = entry else {
        return;
    };
    for operand in &instr.operands {
        match operand_role(entry, &operand.text) {
            Some(OperandRole::Label) => out.push(SemToken {
                span: operand.span,
                token_type: TOKEN_TYPE_VARIABLE,
                modifiers: 0,
            }),
            Some(OperandRole::Function) => {
                let name = operand.text.strip_prefix('@').unwrap_or(&operand.text);
                // Unresolved (no matching doc-local `.func`): emit
                // nothing for the name — the same quiet visual cue
                // `.pmc`'s own `emit_call_name` uses for an unresolved
                // call.
                if functions.contains(name) {
                    out.push(SemToken {
                        span: name_span(operand),
                        token_type: TOKEN_TYPE_FUNCTION,
                        modifiers: 0,
                    });
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::PmaLanguageService;
    use super::*;
    use mtc_core::diagnostics::Span;
    use mtc_core::lsp::LanguageService;

    const URI: &str = "untitled:Tokens-1";

    fn tok(span: Span, token_type: u32, modifiers: u32) -> SemToken {
        SemToken {
            span,
            token_type,
            modifiers,
        }
    }

    /// The `docs/formats.md` (assembly text) `.pma` example verbatim —
    /// `goToEnd`'s own label def/ref pair, `main`'s resolved `call` to
    /// `goToEnd`, and `main`'s `wr 1` numeric operand exercise every
    /// legend type and the `declaration` modifier in one fixture. Every
    /// span below was computed arithmetically from this exact text and
    /// cross-checked with a throwaway script before being transcribed
    /// (mirrors `.pmc`'s own `tokens.rs` fixture note).
    const DOC_EXAMPLE: &str = "\
.func goToEnd                   ; emits ent, defines symbol
L1:     rgt
        jm      L1              ; assembler picks jm.s automatically
        lft
        ret

.func main
        call    goToEnd         ; width decided at link time
        rgt
        wr      1               ; mark
        stp
";

    #[test]
    fn doc_example_yields_the_exact_token_stream() {
        let mut service = PmaLanguageService::new();
        service.did_update(URI, DOC_EXAMPLE);

        let tokens = service
            .semantic_tokens(URI)
            .expect("total CST always answers");

        assert_eq!(
            tokens,
            vec![
                // `.func goToEnd` — the function declaration.
                tok(
                    Span::new(1, 7, 1, 14),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DECLARATION
                ),
                // `L1:     rgt` — the label definition.
                tok(
                    Span::new(2, 1, 2, 3),
                    TOKEN_TYPE_VARIABLE,
                    MODIFIER_DECLARATION
                ),
                // `jm      L1` — the label reference.
                tok(Span::new(3, 17, 3, 19), TOKEN_TYPE_VARIABLE, 0),
                // `.func main` — the function declaration.
                tok(
                    Span::new(7, 7, 7, 11),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DECLARATION
                ),
                // `call    goToEnd` — resolves to the doc-local goToEnd.
                tok(Span::new(8, 17, 8, 24), TOKEN_TYPE_FUNCTION, 0),
                // `wr      1` — a SymbolVec index, a number.
                tok(Span::new(10, 17, 10, 18), TOKEN_TYPE_NUMBER, 0),
            ]
        );
    }

    #[test]
    fn byte_directive_operand_is_a_number_token() {
        // `.byte` is a synthetic directive with no `pm1_syntax()` entry —
        // the `.byte` half of `emit_line`'s is_numeric split, pinned
        // separately from the SymbolVec half `wr 1` covers in the doc
        // example above.
        let mut service = PmaLanguageService::new();
        let src = ".func f\n        .byte   42\n        stp\n";
        service.did_update(URI, src);

        let tokens = service.semantic_tokens(URI).expect("total CST");
        assert_eq!(
            tokens,
            vec![
                tok(
                    Span::new(1, 7, 1, 8),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DECLARATION
                ),
                tok(Span::new(2, 17, 2, 19), TOKEN_TYPE_NUMBER, 0),
            ]
        );
    }

    #[test]
    fn an_unresolved_call_name_emits_no_token() {
        let mut service = PmaLanguageService::new();
        let src = ".func f\n        call    missing\n";
        service.did_update(URI, src);

        let tokens = service.semantic_tokens(URI).expect("total CST");
        assert_eq!(
            tokens,
            vec![tok(
                Span::new(1, 7, 1, 8),
                TOKEN_TYPE_FUNCTION,
                MODIFIER_DECLARATION
            )],
            "only f's own declaration — nothing for the unresolved `missing`"
        );
    }

    #[test]
    fn tokens_still_answer_around_a_line_that_fails_to_assemble() {
        // `bogus` (line 3) is an unknown mnemonic — a fatal — but `f`'s
        // declaration, `L1`'s definition, and `jm`'s reference to it all
        // still tokenize exactly as they would on a clean document.
        let mut service = PmaLanguageService::new();
        let src = ".func f\nL1: rgt\n        bogus\n        jm      L1\n";
        let diags = service.did_update(URI, src);
        assert_eq!(diags[0].code, Some("unknown-mnemonic"), "sanity");

        let tokens = service.semantic_tokens(URI).expect("total CST");
        assert_eq!(
            tokens,
            vec![
                tok(
                    Span::new(1, 7, 1, 8),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DECLARATION
                ),
                tok(
                    Span::new(2, 1, 2, 3),
                    TOKEN_TYPE_VARIABLE,
                    MODIFIER_DECLARATION
                ),
                tok(Span::new(4, 17, 4, 19), TOKEN_TYPE_VARIABLE, 0),
            ]
        );
    }

    #[test]
    fn drift_guard_every_emitted_token_fits_the_legend_and_covers_every_type() {
        let mut service = PmaLanguageService::new();
        service.did_update(URI, DOC_EXAMPLE);
        let tokens = service
            .semantic_tokens(URI)
            .expect("total CST always answers");
        assert!(!tokens.is_empty(), "sanity: the fixture emits tokens");

        let (types, modifiers) = service.token_legend();
        for expected_type in 0..types.len() as u32 {
            assert!(
                tokens.iter().any(|t| t.token_type == expected_type),
                "legend type index {expected_type} ({}) never emitted",
                types[expected_type as usize]
            );
        }
        // `.pma` never emits `defaultLibrary` (no stdlib-call notion in
        // assembly text) — only `declaration`'s coverage is checked.
        assert!(
            tokens
                .iter()
                .any(|t| t.modifiers & MODIFIER_DECLARATION != 0),
            "declaration bit never emitted"
        );

        for t in &tokens {
            assert!(
                (t.token_type as usize) < types.len(),
                "token_type {} has no legend entry in {types:?}",
                t.token_type
            );
            let mut bits = t.modifiers;
            while bits != 0 {
                let bit_ix = bits.trailing_zeros() as usize;
                assert!(
                    bit_ix < modifiers.len(),
                    "modifier bit {} has no legend entry in {modifiers:?}",
                    1u32 << bit_ix
                );
                bits &= bits - 1; // clear the lowest set bit
            }
        }
    }
}
