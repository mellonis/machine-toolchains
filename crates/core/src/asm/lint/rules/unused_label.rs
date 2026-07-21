//! `unused-label` (docs/core.md (assembly lint)): a label nothing
//! references. A reference is an in-function jump/call name operand, OR
//! — on a dialect with table sections — a dispatch `.targets`/`.target`
//! entry or a frame `.exits` descriptor that names the label; those
//! table references live outside the code section, so a rule that
//! looked only at in-function operands false-flagged every dispatch and
//! exit target. Function-scoped for the in-function operands, the same
//! scope as label resolution — `SourceOperand::SymbolName` (`@name`)
//! targets a function symbol, never a local label, so it never counts
//! as a reference. The table references are file-scoped: a name a table
//! reaches counts as used in every function. That can only silence a
//! finding, never mint one, so it is the safe direction for a hygiene
//! lint (the rare cost is missing a dead label that shares its name
//! with a table-reached one elsewhere).

use std::collections::HashSet;

use crate::asm::cst::{AsmCst, AsmItemKind, LineCst};
use crate::asm::lint::AsmLintContext;
use crate::asm::lower::{SourceFunction, SourceItem, SourceOperand, SourceTable, SpannedName};
use crate::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

pub(crate) fn check(ctx: &AsmLintContext, out: &mut Vec<Diagnostic>) {
    let table_refs = table_referenced_labels(ctx.tables);
    for function in ctx.functions {
        check_function(function, &table_refs, ctx.cst, out);
    }
}

/// Code-label names referenced from lowered table sections: dispatch
/// `.targets`/`.target` entries and frame `.exits` descriptors. A match
/// table's rows carry symbol payloads and a frame's `.map` clauses carry
/// symbol maps — neither names a code label, so both are skipped.
fn table_referenced_labels(tables: &[SourceTable]) -> HashSet<&str> {
    let mut refs: HashSet<&str> = HashSet::new();
    for table in tables {
        match table {
            SourceTable::Dispatch { targets, .. } => {
                refs.extend(targets.iter().map(|t| t.name.as_str()));
            }
            SourceTable::Frame { exits, .. } => {
                refs.extend(exits.iter().map(|e| e.name.as_str()));
            }
            SourceTable::Match { .. } => {}
        }
    }
    refs
}

fn check_function(
    function: &SourceFunction,
    table_refs: &HashSet<&str>,
    cst: &AsmCst,
    out: &mut Vec<Diagnostic>,
) {
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
            if referenced.contains(label.name.as_str()) || table_refs.contains(label.name.as_str())
            {
                continue;
            }
            out.push(Diagnostic {
                code: "unused-label",
                span: label.span,
                message: format!(
                    "label `{}` is never referenced (function `{}`)",
                    label.name, function.name
                ),
                // A label that occupies a real source line gets a delete
                // fix; one whose span is a `.rept` header (its physical
                // position is a template the block stamps out repeatedly,
                // not a deletable line) is reported without a fix.
                fix: delete_fix(cst, label),
            });
        }
    }
}

/// A machine-applicable fix that deletes the label, or `None` when the
/// label maps to no single source line to edit (a `.rept`-expanded
/// label, whose span is the block header — [`find_label_line`] finds no
/// matching `Line`).
fn delete_fix(cst: &AsmCst, label: &SpannedName) -> Option<Fix> {
    let line = find_label_line(cst, label)?;
    Some(Fix {
        description: "remove the unused label".to_string(),
        applicability: Applicability::MachineApplicable,
        edits: vec![Edit {
            span: edit_span(line, label),
            replacement: String::new(),
        }],
    })
}

/// The label's own span extended through the `:` and any spaces up to
/// the next token on the same line, so deleting it leaves the line
/// grid-clean rather than a hole of leftover spaces. `line` is the CST
/// line already found to carry this label.
fn edit_span(line: &LineCst, label: &SpannedName) -> Span {
    let through_colon = Span::new(
        label.span.start.line,
        label.span.start.col,
        label.span.start.line,
        label.span.end.col + 1,
    );
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
    use crate::asm::cst::parse_asm_cst_with;
    use crate::asm::lower::lower_source;
    use crate::asm::syntax::AsmCaps;
    use crate::asm::syntax::fixture::test_syntax;

    /// Lower under the given dialect and run only this rule, threading the
    /// lowered tables into the context (empty for a cap-off dialect).
    fn findings_with(src: &str, syntax: &crate::asm::ArchSyntax) -> Vec<Diagnostic> {
        let cst = parse_asm_cst_with(src, syntax.caps);
        let lowered = lower_source(&cst, syntax, src).unwrap();
        let ctx = AsmLintContext {
            source: src,
            cst: &cst,
            functions: &lowered.functions,
            tables: &lowered.tables,
            syntax,
        };
        let mut out = Vec::new();
        check(&ctx, &mut out);
        out
    }

    fn findings(src: &str) -> Vec<Diagnostic> {
        findings_with(src, &test_syntax())
    }

    /// `test_syntax()` with the tables and rept caps on, so `.section`
    /// markers, dispatch directives, and `.rept` blocks shape and lower —
    /// the surface that carries the table references this rule now reads.
    fn caps_syntax() -> crate::asm::ArchSyntax {
        let mut syntax = test_syntax();
        syntax.caps = AsmCaps {
            tables: true,
            rept: true,
            ..Default::default()
        };
        syntax
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

    #[test]
    fn a_dispatch_target_counts_as_a_reference_while_a_dead_label_still_fires() {
        // `seen` is reached only through a `.targets` dispatch entry — a
        // reference that lives in the lowered table section, named by no
        // in-function operand — so it must NOT be flagged. `dead` is
        // reached by nothing, so it must. This is the positive control:
        // it fails if the rule ignores the table feed (both flagged) and
        // if the rule stopped flagging entirely (neither).
        let src = "\
.section tables
D0: .targets seen
.section code
.func f
seen:   nop
        stop
dead:   nop
        stop
";
        let d = findings_with(src, &caps_syntax());
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, "unused-label");
        assert!(d[0].message.contains("`dead`"), "{}", d[0].message);
    }

    #[test]
    fn a_frame_exit_counts_as_a_reference() {
        // A `.exits` descriptor names a code label in the owning function;
        // that reference lives in the frame descriptor, not in any
        // operand, so the label must not be flagged.
        let src = "\
.section tables
F0: .frame tapes=(0)
    .exits done
.section code
.func f
done:   stop
";
        let d = findings_with(src, &caps_syntax());
        assert!(d.iter().all(|f| f.code != "unused-label"), "{d:?}");
    }

    #[test]
    fn tables_off_leaves_the_reference_set_empty_so_behavior_is_unchanged() {
        // With the tables cap off — every PM-1-style dialect — no table
        // shapes, the table-reference set is empty, and a dead label fires
        // exactly as it did before the table feed existed. This pins the
        // change inert wherever `AsmCaps.tables` is off.
        let d = findings(".func f\nUNUSED: nop\n        stop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "unused-label");
        assert!(d[0].fix.is_some(), "a real source line still gets a fix");
    }

    #[test]
    fn a_dead_rept_expanded_label_reports_at_the_header_span_without_a_fix() {
        // `L{v}:` stamps out L0..L2, none referenced. An expanded label's
        // only physical position is the template line, so the finding is
        // anchored at the `.rept` header (line 2) — a usable span, not the
        // re-parsed line-1 artifact — and carries no delete fix, because
        // there is no single source line to edit.
        let src = ".func f\n.rept v, 0, 2\nL{v}: nop\n.endr\n        stop\n";
        let d = findings_with(src, &caps_syntax());
        assert_eq!(d.len(), 3, "{d:?}");
        for f in &d {
            assert_eq!(f.code, "unused-label");
            assert_eq!(f.span.start.line, 2, "anchored at the `.rept` header");
            assert!(f.fix.is_none(), "no fix for a templated label: {f:?}");
        }
    }
}
