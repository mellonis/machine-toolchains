//! Go-to-definition (docs/lsp.md (navigation)) for `.pma`: the operand
//! token under the cursor, resolved from the total CST — never gated on
//! `fatal`/`lint` (docs/formats.md (assembly text); total CST), so a
//! reference resolves even elsewhere on a document that fails to
//! assemble.
//!
//! Two shapes, split by [`super::operand_role`]:
//! - A LABEL reference (`jm`/`jnm`/a bare `jmp` target) resolves within
//!   the SAME enclosing function only (`enclosing_function_range`) —
//!   labels are function-scoped, mirroring `.pmc`'s own label scoping.
//! - A FUNCTION-symbol reference (`call name`, or any `@name` operand)
//!   resolves doc-wide, to the matching `.func`'s own `name_span`.

use mtc_core::asm::cst::AsmItemKind;
use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::DefTarget;

use crate::asm::pm1_syntax;

use super::{
    OperandRole, PmaDocState, doc_functions, enclosing_function_range, item_at_line, item_lines,
    name_span, operand_role,
};

/// Half-open span containment, 1-based (mirrors `.pmc`'s own
/// `navigate.rs` helper of the same name).
fn span_contains(span: Span, pos: Pos) -> bool {
    pos >= span.start && pos < span.end
}

pub(super) fn definition(state: &PmaDocState, uri: &str, pos: Pos) -> Option<DefTarget> {
    let lines = item_lines(&state.text, &state.cst);
    let item = item_at_line(&state.cst, &lines, pos.line)?;
    let AsmItemKind::Line(line) = &item.kind else {
        return None;
    };
    let instr = line.instr.as_ref()?;
    let syntax = pm1_syntax();
    let entry = syntax.by_mnemonic(&instr.word)?;
    let operand = instr.operands.iter().find(|o| span_contains(o.span, pos))?;

    match operand_role(entry, &operand.text)? {
        OperandRole::Function => {
            let name = operand.text.strip_prefix('@').unwrap_or(&operand.text);
            let target = doc_functions(&state.cst).find(|f| f.name == name)?;
            Some(DefTarget {
                uri: uri.to_string(),
                span: target.name_span,
                origin: Some(name_span(operand)),
            })
        }
        OperandRole::Label => {
            let (_, range) = enclosing_function_range(&state.cst, &lines, pos.line)?;
            let target = state.cst.items[range].iter().find_map(|it| {
                let AsmItemKind::Line(l) = &it.kind else {
                    return None;
                };
                l.labels.iter().find(|label| label.name == operand.text)
            })?;
            Some(DefTarget {
                uri: uri.to_string(),
                span: target.span,
                origin: Some(operand.span),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::PmaLanguageService;
    use super::*;
    use mtc_core::lsp::LanguageService;

    const URI: &str = "untitled:Nav-1";

    /// One function `f` exercising all three operand-reference shapes —
    /// a bare label (`jm L1`), a bare function symbol (`call helper`),
    /// and an `@`-prefixed function symbol (`jmp @helper`) — plus the
    /// `helper` function itself the last two resolve to.
    const NAV_FIXTURE: &str = ".func f\nL1: rgt\n        jm      L1\n        call    helper\n        jmp     @helper\n        ret\n.func helper\n        ret\n";

    #[test]
    fn label_reference_resolves_to_the_label_definition_in_the_same_function() {
        let mut service = PmaLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = Pos { line: 3, col: 18 }; // inside `jm L1`'s operand
        let target = service.definition(URI, pos).expect("L1 is defined in f");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, Span::new(2, 1, 2, 3));
        assert_eq!(target.origin, Some(Span::new(3, 17, 3, 19)));
    }

    #[test]
    fn bare_jmp_label_operand_resolves_like_a_branch_label() {
        // `jmp L1` — Flow::Jump with a BARE operand goes through the
        // same OperandRole::Label arm `jm`/`jnm` do (a bare jump target
        // is a label; only `@name` makes a jump a function symbol,
        // docs/formats.md (symbol jumps)) — pinned separately so the
        // Jump half of the shared match arm isn't test-dead.
        let mut service = PmaLanguageService::new();
        let src = ".func f\nL1: rgt\n        jmp     L1\n";
        service.did_update(URI, src);

        let pos = Pos { line: 3, col: 18 }; // inside `jmp L1`'s operand
        let target = service.definition(URI, pos).expect("L1 is defined in f");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, Span::new(2, 1, 2, 3));
        assert_eq!(target.origin, Some(Span::new(3, 17, 3, 19)));
    }

    #[test]
    fn call_operand_resolves_to_the_func_directives_name_span() {
        let mut service = PmaLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = Pos { line: 4, col: 19 }; // inside `call helper`'s operand
        let target = service
            .definition(URI, pos)
            .expect("helper is declared in this document");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, Span::new(7, 7, 7, 13));
        assert_eq!(
            target.origin,
            Some(Span::new(4, 17, 4, 23)),
            "the whole bare operand — no `@` to exclude"
        );
    }

    #[test]
    fn jmp_at_name_operand_resolves_to_the_func_directives_name_span_excluding_the_sigil() {
        let mut service = PmaLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = Pos { line: 5, col: 20 }; // inside `jmp @helper`'s operand
        let target = service
            .definition(URI, pos)
            .expect("helper is declared in this document");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, Span::new(7, 7, 7, 13));
        assert_eq!(
            target.origin,
            Some(Span::new(5, 18, 5, 24)),
            "excludes the `@` at column 17"
        );
    }

    #[test]
    fn call_to_an_undeclared_function_resolves_to_none() {
        let mut service = PmaLanguageService::new();
        let src = ".func f\n        call    missing\n";
        service.did_update(URI, src);

        let pos = Pos { line: 2, col: 19 };
        assert_eq!(service.definition(URI, pos), None);
    }

    #[test]
    fn a_label_reference_never_crosses_into_a_same_named_label_in_another_function() {
        let mut service = PmaLanguageService::new();
        let src = ".func f\nL1: rgt\n        jm      L1\n.func g\nL1: lft\n        jm      L1\n";
        service.did_update(URI, src);

        let f_label = Span::new(2, 1, 2, 3);
        let g_label = Span::new(5, 1, 5, 3);
        assert_ne!(f_label, g_label, "sanity: distinct positions");

        let f_target = service
            .definition(URI, Pos { line: 3, col: 18 })
            .expect("f's own jm 1 resolves inside f");
        assert_eq!(f_target.span, f_label);
        assert_ne!(f_target.span, g_label);

        let g_target = service
            .definition(URI, Pos { line: 6, col: 18 })
            .expect("g's own jm 1 resolves inside g");
        assert_eq!(g_target.span, g_label);
        assert_ne!(g_target.span, f_label);
    }

    #[test]
    fn a_reference_still_resolves_elsewhere_on_a_document_that_fails_to_assemble() {
        // `bogus` on line 3 is an unknown mnemonic — a fatal — but the
        // total CST still shapes `call helper` on line 2 and `.func
        // helper` on line 4 exactly as it would on a clean document.
        let mut service = PmaLanguageService::new();
        let src = ".func f\n        call    helper\n        bogus\n.func helper\n        ret\n";
        let diags = service.did_update(URI, src);
        assert_eq!(diags[0].code, Some("unknown-mnemonic"), "sanity");

        let pos = Pos { line: 2, col: 19 };
        let target = service
            .definition(URI, pos)
            .expect("helper still resolves despite the broken line elsewhere");
        assert_eq!(target.span, Span::new(4, 7, 4, 13));
        assert_eq!(target.origin, Some(Span::new(2, 17, 2, 23)));
    }
}
