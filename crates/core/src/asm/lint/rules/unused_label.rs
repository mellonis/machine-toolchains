//! `unused-label` (docs/lint.md): a label nothing in its function
//! references via a jump/call name operand. Function-scoped, the same
//! scope as label resolution — `SourceOperand::SymbolName` (`@name`)
//! targets a function symbol, never a local label, so it never counts
//! as a reference.

use std::collections::HashSet;

use crate::asm::cst::{AsmCst, AsmItemKind, LineCst};
use crate::asm::lint::AsmLintContext;
use crate::asm::lower::{SourceFunction, SourceItem, SourceOperand, SpannedName};
use crate::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

pub(crate) fn check(ctx: &AsmLintContext, out: &mut Vec<Diagnostic>) {
    for function in ctx.functions {
        check_function(function, ctx.cst, out);
    }
}

fn check_function(function: &SourceFunction, cst: &AsmCst, out: &mut Vec<Diagnostic>) {
    let mut referenced: HashSet<&str> = HashSet::new();
    for item in &function.items {
        if let SourceItem::Instr {
            operand: SourceOperand::Name(name),
            ..
        } = item
        {
            referenced.insert(name.name.as_str());
        }
    }

    for item in &function.items {
        let labels: &[SpannedName] = match item {
            SourceItem::Instr { labels, .. } | SourceItem::RawByte { labels, .. } => labels,
        };
        for label in labels {
            if referenced.contains(label.name.as_str()) {
                continue;
            }
            out.push(Diagnostic {
                code: "unused-label",
                span: label.span,
                message: format!(
                    "label `{}` is never referenced (function `{}`)",
                    label.name, function.name
                ),
                fix: Some(Fix {
                    description: "remove the unused label".to_string(),
                    applicability: Applicability::MachineApplicable,
                    edits: vec![Edit {
                        span: edit_span(cst, label),
                        replacement: String::new(),
                    }],
                }),
            });
        }
    }
}

/// The label's own span extended through the `:` and any spaces up to
/// the next token on the same line, so deleting it leaves the line
/// grid-clean rather than a hole of leftover spaces.
fn edit_span(cst: &AsmCst, label: &SpannedName) -> Span {
    let through_colon = Span::new(
        label.span.start.line,
        label.span.start.col,
        label.span.start.line,
        label.span.end.col + 1,
    );
    let Some(line) = find_label_line(cst, label) else {
        return through_colon;
    };
    let index = line
        .labels
        .iter()
        .position(|l| l.span == label.span && l.name == label.name);
    let next_token_col = index
        .and_then(|i| line.labels.get(i + 1))
        .map(|l| l.span.start.col)
        .or_else(|| line.instr.as_ref().map(|instr| instr.word_span.start.col))
        .or_else(|| line.trailing.as_ref().map(|t| t.col));
    match next_token_col {
        Some(col) => Span::new(
            label.span.start.line,
            label.span.start.col,
            label.span.start.line,
            col,
        ),
        None => through_colon,
    }
}

/// Finds the CST line carrying this exact label (matched by span +
/// name, which is unambiguous — a source position occurs once).
fn find_label_line<'a>(cst: &'a AsmCst, label: &SpannedName) -> Option<&'a LineCst> {
    cst.items.iter().find_map(|item| match &item.kind {
        AsmItemKind::Line(line) => line
            .labels
            .iter()
            .any(|l| l.span == label.span && l.name == label.name)
            .then_some(line),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::cst::parse_asm_cst;
    use crate::asm::lower::lower;
    use crate::asm::syntax::fixture::test_syntax;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let syntax = test_syntax();
        let cst = parse_asm_cst(src);
        let functions = lower(&cst, &syntax).unwrap();
        let ctx = AsmLintContext {
            source: src,
            cst: &cst,
            functions: &functions,
            syntax: &syntax,
        };
        let mut out = Vec::new();
        check(&ctx, &mut out);
        out
    }

    #[test]
    fn unreferenced_label_fires_with_a_delete_fix_and_exact_edit_span() {
        let d = findings(".func f\nUNUSED: nop\n        stop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "unused-label");
        assert_eq!(
            d[0].message,
            "label `UNUSED` is never referenced (function `f`)"
        );
        let fix = d[0].fix.as_ref().unwrap();
        assert_eq!(fix.description, "remove the unused label");
        assert!(matches!(
            fix.applicability,
            Applicability::MachineApplicable
        ));
        assert_eq!(fix.edits.len(), 1);
        // "UNUSED: nop": label cols 1..7, colon at 7, space at 8, `nop`
        // starts at 9 — the edit swallows the label, colon, and the one
        // separating space, leaving `nop` to start the line cleanly.
        assert_eq!(fix.edits[0].span, Span::new(2, 1, 2, 9));
        assert_eq!(fix.edits[0].replacement, "");
    }

    #[test]
    fn label_referenced_by_a_jump_is_not_flagged() {
        let d = findings(".func f\n        jmp L1\nL1:     stop\n");
        assert!(d.is_empty());
    }

    #[test]
    fn at_symbol_operand_does_not_count_as_a_label_reference() {
        // `@other` targets the function symbol `other`, never a local
        // label — `TARGET` stays unreferenced even though something in
        // the function has an `@`-operand.
        let d = findings(".func f\nTARGET: nop\n        jmp @other\n        stop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "unused-label");
    }

    #[test]
    fn a_second_label_on_the_same_line_bounds_the_edit_span() {
        // "A: B: nop" — deleting the unused `A:` must stop right before
        // `B`, not swallow it.
        let d = findings(".func f\nA: B: nop\n        jmp B\n        stop\n");
        let a = d.iter().find(|d| d.message.contains('A')).unwrap();
        assert_eq!(a.fix.as_ref().unwrap().edits[0].span, Span::new(2, 1, 2, 4));
    }

    #[test]
    fn label_only_line_with_the_instruction_on_the_next_line_falls_back_to_through_colon() {
        // "UNUSED:" alone on its own line, `nop` on the NEXT line — the
        // label-only line carries no instr, no further label, and no
        // trailing comment of its own, so `edit_span`'s `next_token_col`
        // search comes up empty and the fallback (label span + colon)
        // is what fires, not a span that reaches onto the next line.
        let d = findings(".func f\nUNUSED:\n        nop\n        stop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "unused-label");
        let fix = d[0].fix.as_ref().unwrap();
        assert_eq!(fix.edits.len(), 1);
        assert_eq!(fix.edits[0].span, Span::new(2, 1, 2, 8));
        assert_eq!(fix.edits[0].replacement, "");
    }
}
