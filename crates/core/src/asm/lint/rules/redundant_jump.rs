//! `redundant-jump-to-next` (docs/core.md (assembly lint)): a `Flow::Jump` or
//! `Flow::Branch` item whose name operand targets the label bound to
//! the immediately following item in the same function — fall-through
//! already lands there, so an unconditional jump changes nothing and,
//! provided `Branch` opcodes affect only control-flow selection — the
//! same no-other-effect premise `Jump` already relies on — either
//! outcome of a conditional branch lands on that same next instruction
//! with no other observable difference. An opcode whose branch has
//! effects beyond selecting its successor shouldn't be classified
//! `Branch` (compare `Flow::Call`, the carve-out for side-effecting
//! control transfer). Arch-agnostic: arming is `Flow::Jump |
//! Flow::Branch`, not any specific mnemonic (a forced-short
//! jump/branch form is just as inert).

use crate::asm::lint::AsmLintContext;
use crate::asm::lint::rules::delete_instruction_edit_span;
use crate::asm::lower::{SourceItem, SourceOperand, SpannedName};
use crate::asm::syntax::Flow;
use crate::diagnostics::{Applicability, Diagnostic, Edit, Fix};

pub(crate) fn check(ctx: &AsmLintContext, out: &mut Vec<Diagnostic>) {
    for function in ctx.functions {
        for window in function.items.windows(2) {
            let (item, next) = (&window[0], &window[1]);
            let SourceItem::Instr {
                span,
                opcode,
                operand: SourceOperand::Name(target),
                ..
            } = item
            else {
                continue;
            };
            if !matches!(
                ctx.syntax.by_opcode(*opcode).map(|entry| entry.flow),
                Some(Flow::Jump | Flow::Branch)
            ) {
                continue;
            }
            if !labels_of(next).iter().any(|l| l.name == target.name) {
                continue;
            }
            out.push(Diagnostic {
                code: "redundant-jump-to-next",
                span: *span,
                message: format!(
                    "jump/branch to `{}` targets the next instruction — fall-through is identical",
                    target.name
                ),
                fix: Some(Fix {
                    description: "remove the redundant jump".to_string(),
                    applicability: Applicability::MachineApplicable,
                    edits: vec![Edit {
                        span: delete_instruction_edit_span(ctx.cst, *span),
                        replacement: String::new(),
                    }],
                }),
            });
        }
    }
}

fn labels_of(item: &SourceItem) -> &[SpannedName] {
    match item {
        SourceItem::Instr { labels, .. } | SourceItem::RawByte { labels, .. } => labels.as_slice(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::cst::parse_asm_cst;
    use crate::asm::lower::lower;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::diagnostics::Span;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let syntax = test_syntax();
        let cst = parse_asm_cst(src);
        let functions = lower(&cst, &syntax, src).unwrap();
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
    fn jump_to_the_next_instruction_is_flagged() {
        let d = findings(".func f\n        jmp L1\nL1:     stop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "redundant-jump-to-next");
        assert_eq!(
            d[0].message,
            "jump/branch to `L1` targets the next instruction — fall-through is identical"
        );
        assert_eq!(d[0].span.start.line, 2); // the whole `jmp L1` item
        let fix = d[0].fix.as_ref().unwrap();
        assert_eq!(fix.description, "remove the redundant jump");
        assert_eq!(fix.applicability, Applicability::MachineApplicable);
    }

    #[test]
    fn jump_over_one_instruction_is_not_flagged() {
        let d = findings(".func f\n        jmp L2\n        nop\nL2:     stop\n");
        assert!(d.is_empty());
    }

    #[test]
    fn forced_short_jump_mnemonics_are_flagged_too() {
        // `jmp.s` is a distinct mnemonic/opcode from `jmp` in the fixture,
        // but shares Flow::Jump — the rule keys on Flow, not spelling.
        let d = findings(".func f\n        jmp.s L1\nL1:     stop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "redundant-jump-to-next");
    }

    #[test]
    fn a_conditional_branch_to_the_next_instruction_is_flagged() {
        // `br` is Flow::Branch — either outcome of a conditional branch
        // to its own fall-through lands on the same next instruction, so
        // any Flow::Branch mnemonic hits this same arm; the rule keys on
        // Flow, not spelling (mirrors forced_short_jump_mnemonics above).
        let d = findings(".func f\n        br L1\nL1:     stop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "redundant-jump-to-next");
        assert_eq!(
            d[0].message,
            "jump/branch to `L1` targets the next instruction — fall-through is identical"
        );
        let fix = d[0].fix.as_ref().unwrap();
        assert_eq!(fix.applicability, Applicability::MachineApplicable);
    }

    #[test]
    fn conditional_branch_over_one_instruction_is_not_flagged() {
        let d = findings(".func f\n        br L2\n        nop\nL2:     stop\n");
        assert!(d.is_empty());
    }

    #[test]
    fn labels_on_the_branch_line_survive_the_fix() {
        // "L0:     br L1" — deleting only the instruction portion keeps
        // "L0:" bound forward to whatever now follows it.
        let src = ".func f\nL0:     br L1\nL1:     stop\n";
        let d = findings(src);
        assert_eq!(d.len(), 1);
        let edit = &d[0].fix.as_ref().unwrap().edits[0];
        // "L0:     br L1": `br` starts at col 9; the trimmed line ends
        // at col 14 (`br L1` is 5 chars from col 9).
        assert_eq!(edit.span, Span::new(2, 9, 2, 14));
        assert_eq!(edit.replacement, "");

        // Re-lowering the fixed source keeps L0 alive: as a label-only
        // line it becomes pending and joins L1 on the very next
        // instruction (`stop`) — the same target the deleted branch
        // named, so both labels now bind to the one surviving item.
        let fixed = format!(
            "{}{}",
            &src[..byte_of(src, edit.span.start)],
            &src[byte_of(src, edit.span.end)..]
        );
        let syntax = test_syntax();
        let funcs = lower(&parse_asm_cst(&fixed), &syntax, &fixed).unwrap();
        assert_eq!(funcs[0].items.len(), 1); // br is gone; only `stop` remains
        match &funcs[0].items[0] {
            SourceItem::Instr { labels, .. } => {
                assert_eq!(
                    labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
                    vec!["L0", "L1"]
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn unlabeled_jump_line_deletes_the_whole_physical_line() {
        let src = ".func f\n        jmp L1\nL1:     stop\n";
        let d = findings(src);
        let edit = &d[0].fix.as_ref().unwrap().edits[0];
        assert_eq!(edit.replacement, "");
        assert_eq!(edit.span, Span::new(2, 1, 3, 1));
    }

    #[test]
    fn labels_on_the_jump_line_survive_the_fix() {
        // "L0:     jmp L1" — deleting only the instruction portion keeps
        // "L0:" bound forward to whatever now follows it.
        let src = ".func f\nL0:     jmp L1\nL1:     stop\n";
        let d = findings(src);
        assert_eq!(d.len(), 1);
        let edit = &d[0].fix.as_ref().unwrap().edits[0];
        // "L0:     jmp L1": `jmp` starts at col 9; the trimmed line ends
        // at col 15 (`jmp L1` is 6 chars from col 9).
        assert_eq!(edit.span, Span::new(2, 9, 2, 15));
        assert_eq!(edit.replacement, "");

        // Re-lowering the fixed source keeps L0 alive: as a label-only
        // line it becomes pending and joins L1 on the very next
        // instruction (`stop`) — the same target the deleted jump named,
        // so both labels now bind to the one surviving item.
        let fixed = format!(
            "{}{}",
            &src[..byte_of(src, edit.span.start)],
            &src[byte_of(src, edit.span.end)..]
        );
        let syntax = test_syntax();
        let funcs = lower(&parse_asm_cst(&fixed), &syntax, &fixed).unwrap();
        assert_eq!(funcs[0].items.len(), 1); // jmp is gone; only `stop` remains
        match &funcs[0].items[0] {
            SourceItem::Instr { labels, .. } => {
                assert_eq!(
                    labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
                    vec!["L0", "L1"]
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Test-local char-counted (line, col) -> byte offset (mirrors the
    /// fixer's own conversion; kept local since no fixer lives in core).
    fn byte_of(source: &str, pos: crate::diagnostics::Pos) -> usize {
        let (mut line, mut col) = (1u32, 1u32);
        for (i, c) in source.char_indices() {
            if line == pos.line && col == pos.col {
                return i;
            }
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        source.len()
    }
}
