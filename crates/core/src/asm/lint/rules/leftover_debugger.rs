//! `leftover-debugger` (docs/core.md (assembly lint)): an instruction whose opcode is
//! the arch's declared debugger-break opcode (`ArchSyntax::break_opcode`)
//! — a forgotten debugging aid left in shipped source. Silent when the
//! arch declares no such opcode (`break_opcode == None`), so a core
//! test fixture with no debugger concept never fires this rule.

use crate::asm::lint::AsmLintContext;
use crate::asm::lint::rules::delete_instruction_edit_span;
use crate::asm::lower::SourceItem;
use crate::diagnostics::{Applicability, Diagnostic, Edit, Fix};

pub(crate) fn check(ctx: &AsmLintContext, out: &mut Vec<Diagnostic>) {
    let Some(break_opcode) = ctx.syntax.break_opcode else {
        return;
    };
    for function in ctx.functions {
        for item in &function.items {
            let SourceItem::Instr { span, opcode, .. } = item else {
                continue;
            };
            if *opcode != break_opcode {
                continue;
            }
            out.push(Diagnostic {
                code: "leftover-debugger",
                span: *span,
                message: "leftover debugger break left in source".to_string(),
                fix: Some(Fix {
                    description: "remove the debugger break".to_string(),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![Edit {
                        span: delete_instruction_edit_span(ctx.cst, *span),
                        replacement: String::new(),
                    }],
                }),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::cst::parse_asm_cst;
    use crate::asm::lower::lower;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::asm::syntax::{ArchSyntax, Flow, SyntaxEntry};
    use crate::diagnostics::Span;
    use crate::vm::OperandKind;

    /// `test_syntax()` plus a `dbg` opcode wired as the debugger break —
    /// added locally here, not in the shared fixture, since
    /// `break_opcode` stays `None` in the base fixture by contract
    /// (core carries zero PM-1 knowledge, and no other core test needs
    /// a debugger opcode to exist).
    fn debugger_syntax() -> ArchSyntax {
        let mut syntax = test_syntax();
        syntax.entries.push(SyntaxEntry {
            opcode: 0x0F,
            mnemonic: "dbg",
            operand: OperandKind::None,
            flow: Flow::FallThrough,
        });
        syntax.break_opcode = Some(0x0F);
        syntax
    }

    fn findings(syntax: &ArchSyntax, src: &str) -> Vec<Diagnostic> {
        let cst = parse_asm_cst(src);
        let functions = lower(&cst, syntax, src).unwrap();
        let ctx = AsmLintContext {
            source: src,
            cst: &cst,
            functions: &functions,
            syntax,
        };
        let mut out = Vec::new();
        check(&ctx, &mut out);
        out
    }

    #[test]
    fn silent_when_the_arch_declares_no_break_opcode() {
        // test_syntax() has no `dbg` mnemonic at all, so there is nothing
        // in this source that could even encode a break — the rule's
        // early return on `break_opcode == None` is what this pins.
        let d = findings(&test_syntax(), ".func f\n        nop\n        stop\n");
        assert!(d.is_empty());
    }

    #[test]
    fn fires_under_a_fixture_with_a_break_opcode() {
        let syntax = debugger_syntax();
        let d = findings(&syntax, ".func f\n        dbg\n        stop\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "leftover-debugger");
        assert_eq!(d[0].message, "leftover debugger break left in source");
        assert_eq!(d[0].span.start.line, 2);
        let fix = d[0].fix.as_ref().unwrap();
        assert_eq!(fix.description, "remove the debugger break");
        assert_eq!(fix.applicability, Applicability::MaybeIncorrect);
    }

    #[test]
    fn unlabeled_break_deletes_the_whole_physical_line() {
        let syntax = debugger_syntax();
        let src = ".func f\n        dbg\n        stop\n";
        let d = findings(&syntax, src);
        let edit = &d[0].fix.as_ref().unwrap().edits[0];
        assert_eq!(edit.span, Span::new(2, 1, 3, 1));
        assert_eq!(edit.replacement, "");
    }

    #[test]
    fn labels_on_the_break_line_survive_the_fix() {
        let syntax = debugger_syntax();
        // "L0:     dbg": `dbg` starts at col 9, line ends at col 12.
        let src = ".func f\nL0:     dbg\n        stop\n";
        let d = findings(&syntax, src);
        assert_eq!(d.len(), 1);
        let edit = &d[0].fix.as_ref().unwrap().edits[0];
        assert_eq!(edit.span, Span::new(2, 9, 2, 12));
        assert_eq!(edit.replacement, "");
    }
}
