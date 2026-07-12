//! `unreachable-code` (docs/lint.md): an item with no label sitting
//! right after an unconditional jump or stop. Arch-agnostic — the
//! arming condition is the syntax entry's [`Flow`], not any specific
//! mnemonic: `Stop` and `Jump` arm it (there is provably no fall-through
//! successor); `Branch` does not (a conditional branch may fall
//! through, docs/isa.md (control flow)). A label resets the arm — it is
//! a fresh entry point, reachable from wherever jumps to it — even when
//! the labeled item is itself a terminator, which re-arms for whatever
//! follows.

use crate::asm::ArchSyntax;
use crate::asm::lint::AsmLintContext;
use crate::asm::lower::{SourceItem, SpannedName};
use crate::asm::syntax::Flow;
use crate::diagnostics::{Diagnostic, Span};

pub(crate) fn check(ctx: &AsmLintContext, out: &mut Vec<Diagnostic>) {
    for function in ctx.functions {
        let mut armed = false;
        for item in &function.items {
            let (span, labels) = item_span_and_labels(item);
            if armed && labels.is_empty() {
                out.push(Diagnostic {
                    code: "unreachable-code",
                    span,
                    message: "unreachable code: no label between here and the preceding \
                              unconditional jump/stop"
                        .to_string(),
                    fix: None,
                });
            }
            if !labels.is_empty() {
                armed = false;
            }
            if terminates(item, ctx.syntax) {
                armed = true;
            }
        }
    }
}

fn item_span_and_labels(item: &SourceItem) -> (Span, &[SpannedName]) {
    match item {
        SourceItem::Instr { span, labels, .. } => (*span, labels.as_slice()),
        SourceItem::RawByte { span, labels, .. } => (*span, labels.as_slice()),
    }
}

/// Whether this item's own control flow leaves nothing falling through
/// to the next item — an unconditional jump or a stop. Data (`.byte`)
/// carries no opcode and never arms on its own.
fn terminates(item: &SourceItem, syntax: &ArchSyntax) -> bool {
    match item {
        SourceItem::Instr { opcode, .. } => matches!(
            syntax.by_opcode(*opcode).map(|entry| entry.flow),
            Some(Flow::Stop) | Some(Flow::Jump)
        ),
        SourceItem::RawByte { .. } => false,
    }
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
    fn flags_code_after_an_unconditional_jump() {
        let d = findings(".func f\n        jmp L1\n        nop\nL1:     stop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "unreachable-code");
        assert_eq!(
            d[0].message,
            "unreachable code: no label between here and the preceding unconditional jump/stop"
        );
        assert!(d[0].fix.is_none());
        assert_eq!(d[0].span.start.line, 3); // the dead `nop`
    }

    #[test]
    fn flags_code_after_stop() {
        let d = findings(".func f\n        stop\n        nop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].span.start.line, 3);
    }

    #[test]
    fn flags_code_after_ret() {
        let d = findings(".func f\n        ret\n        nop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].span.start.line, 3);
    }

    #[test]
    fn branch_does_not_arm() {
        // `br` may fall through when not taken, so the item right after
        // it stays reachable and is never flagged.
        let d = findings(".func f\n        br L1\n        nop\nL1:     stop\n");
        assert!(d.is_empty());
    }

    #[test]
    fn a_labeled_successor_is_not_flagged() {
        let d = findings(".func f\n        jmp L1\nL1:     nop\n        stop\n");
        assert!(d.is_empty());
    }

    #[test]
    fn arming_resets_after_a_labeled_item_and_does_not_carry_past_it() {
        let d = findings(".func f\n        jmp L1\n        nop\nL1:     nop\n        nop\n");
        // Only the dead `nop` right after `jmp` (line 3) is flagged: L1's
        // label resets the arm on line 4 (itself a plain fall-through, so
        // it does not re-arm), and the trailing `nop` on line 5 stays
        // reachable.
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].span.start.line, 3);
    }

    #[test]
    fn a_terminator_that_is_itself_labeled_still_arms_for_what_follows() {
        let d = findings(".func f\n        jmp L1\nL1:     stop\n        nop\n");
        // L1 resets the arm (so `stop` itself is not flagged), but stop's
        // own flow re-arms for the unlabeled `nop` after it.
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].span.start.line, 4);
    }

    #[test]
    fn byte_data_after_a_terminator_with_no_label_is_flagged() {
        let d = findings(".func f\n        stop\n        .byte 42\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].span.start.line, 3);
    }

    #[test]
    fn call_does_not_arm() {
        // `call` is `Flow::Call`, neither `Stop` nor `Jump` — it may
        // return and fall through, so the unlabeled instruction right
        // after it stays reachable. (`g` need not resolve to a real
        // function: this rule reads `SourceFunction`/`ArchSyntax` only,
        // never the assembler's label table.)
        let d = findings(".func f\n        call g\n        nop\n        stop\n");
        assert!(d.is_empty());
    }

    #[test]
    fn empty_function_body_yields_no_finding_and_does_not_panic() {
        let d = findings(".func f\n");
        assert!(d.is_empty());
    }
}
