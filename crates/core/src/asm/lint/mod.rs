//! Assembly lint layer (docs/lint.md). Arch-agnostic: control flow via
//! `ArchSyntax::Flow`, the break opcode via `ArchSyntax::break_opcode`.
//! Library-only — the CLI renders (docs/cli.md, thin-renderer rule).

pub(crate) mod rules;

use super::cst::{AsmCst, parse_asm_cst};
use super::lower::{SourceFunction, lower};
use super::{ArchSyntax, AsmError, assemble};
use crate::diagnostics::Diagnostic;

/// Everything a rule may read. Rules never mutate the program.
pub struct AsmLintContext<'a> {
    pub source: &'a str,
    pub cst: &'a AsmCst,
    pub functions: &'a [SourceFunction],
    pub syntax: &'a ArchSyntax,
}

/// A lint rule: reads the assembled context, pushes any findings.
pub type Rule = fn(&AsmLintContext, &mut Vec<Diagnostic>);

/// The rule table, keyed by its defect-named kebab code. Public so the
/// pmt lint layer can validate `allow` codes over the cross-language
/// union.
pub const RULES: &[(&str, Rule)] = &[
    ("unreachable-code", rules::unreachable_code::check),
    ("unused-label", rules::unused_label::check),
    ("redundant-jump-to-next", rules::redundant_jump::check),
    ("line-too-long", rules::line_too_long::check),
    ("leftover-debugger", rules::leftover_debugger::check),
];

/// Lints one `.pma` source. Fatal gate: a full assemble — structural
/// Raw lines and semantic errors (unknown mnemonic, duplicate/unknown
/// label, bad operand, …) alike refuse the file, matching `pmt lint`'s
/// pre-lint compile gate on the `.pmc` side. Does NOT validate `allow`
/// codes — the driver owns that (it knows the cross-language union of
/// rule codes across both languages).
pub fn lint(
    syntax: &ArchSyntax,
    source: &str,
    allow: &[String],
) -> Result<Vec<Diagnostic>, AsmError> {
    let cst = parse_asm_cst(source);
    let functions = lower(&cst, syntax, source)?;
    assemble(syntax, 0, source, false)?;

    let ctx = AsmLintContext {
        source,
        cst: &cst,
        functions: &functions,
        syntax,
    };
    let mut diagnostics = Vec::new();
    for (code, rule) in RULES {
        if allow.iter().any(|a| a == code) {
            continue;
        }
        rule(&ctx, &mut diagnostics);
    }
    diagnostics.sort_by_key(|d| d.span.start); // stable; Pos is Ord
    Ok(diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::AsmErrorKind;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::asm::syntax::{ArchSyntax, Flow, SyntaxEntry};
    use crate::vm::OperandKind;

    #[test]
    fn clean_program_yields_no_findings() {
        let syntax = test_syntax();
        let report = lint(&syntax, ".func f\n        stop\n", &[]).unwrap();
        assert!(report.is_empty());
    }

    #[test]
    fn rules_table_carries_all_five_codes_in_plan_order() {
        let codes: Vec<&str> = RULES.iter().map(|(code, _)| *code).collect();
        assert_eq!(
            codes,
            vec![
                "unreachable-code",
                "unused-label",
                "redundant-jump-to-next",
                "line-too-long",
                "leftover-debugger",
            ]
        );
    }

    #[test]
    fn redundant_jump_finding_runs_through_the_full_lint_entry_point() {
        // End-to-end through `lint()` (fatal gate + registry dispatch),
        // not just the rule's own unit tests.
        let syntax = test_syntax();
        let report = lint(&syntax, ".func f\n        jmp L1\nL1:     stop\n", &[]).unwrap();
        assert!(report.iter().any(|d| d.code == "redundant-jump-to-next"));
    }

    #[test]
    fn fatal_unknown_mnemonic_propagates_as_err() {
        let syntax = test_syntax();
        let err = lint(&syntax, ".func f\n        bogus\n", &[]).unwrap_err();
        assert!(matches!(err.kind, AsmErrorKind::UnknownMnemonic(ref m) if m == "bogus"));
    }

    #[test]
    fn fatal_raw_line_propagates_as_err() {
        let syntax = test_syntax();
        // A disassembly-listing-shaped line is not assembly text.
        let err = lint(&syntax, "<goToEnd>\n", &[]).unwrap_err();
        assert_eq!(err.kind, AsmErrorKind::RawLine);
    }

    #[test]
    fn fatal_gate_catches_errors_lower_alone_cannot_see() {
        // Channel discipline (docs/lint.md): duplicate/unknown labels are
        // never lint findings — they stay fatals. `lower()` alone does
        // not resolve labels (that is layout's job), so this pins that
        // `lint()`'s gate really is the full `assemble()`, not just
        // `lower()`.
        let syntax = test_syntax();
        let err = lint(&syntax, ".func f\nL1: nop\nL1: nop\n", &[]).unwrap_err();
        assert!(matches!(err.kind, AsmErrorKind::DuplicateLabel(ref l) if l == "L1"));

        let err = lint(&syntax, ".func f\n        jmp NOWHERE\n", &[]).unwrap_err();
        assert!(matches!(err.kind, AsmErrorKind::UnknownLabel(ref l) if l == "NOWHERE"));
    }

    #[test]
    fn allowed_code_is_suppressed() {
        let syntax = test_syntax();
        let src = ".func f\nUNUSED: nop\n        stop\n";
        let all = lint(&syntax, src, &[]).unwrap();
        assert!(all.iter().any(|d| d.code == "unused-label"));

        let filtered = lint(&syntax, src, &["unused-label".to_string()]).unwrap();
        assert!(filtered.iter().all(|d| d.code != "unused-label"));
        assert_eq!(filtered.len() + 1, all.len());
    }

    /// `test_syntax()` plus a `dbg` opcode wired as the debugger break —
    /// added locally here (not in the shared fixture, which keeps
    /// `break_opcode: None` by contract), mirroring
    /// `rules::leftover_debugger::tests::debugger_syntax`. This module has
    /// no shared test-support module either, so the helper is duplicated
    /// rather than imported.
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

    #[test]
    fn line_too_long_finding_runs_through_the_full_lint_entry_point() {
        // End-to-end through `lint()`, not just the rule's own unit
        // tests — mirrors `redundant_jump_finding_runs_through_the_full_
        // lint_entry_point`.
        let syntax = test_syntax();
        let long = format!(";{}", "x".repeat(89)); // 1 + 89 = 90 chars
        let src = format!("{long}\n.func f\n        stop\n");
        let report = lint(&syntax, &src, &[]).unwrap();
        assert!(report.iter().any(|d| d.code == "line-too-long"));
    }

    #[test]
    fn leftover_debugger_finding_runs_through_the_full_lint_entry_point() {
        // Needs a syntax fixture that actually declares a break opcode
        // (`test_syntax()` alone never fires this rule) — the local
        // `debugger_syntax()` helper above.
        let syntax = debugger_syntax();
        let report = lint(&syntax, ".func f\n        dbg\n        stop\n", &[]).unwrap();
        assert!(report.iter().any(|d| d.code == "leftover-debugger"));
    }

    #[test]
    fn findings_are_sorted_by_span_start_across_rules() {
        // `unused-label` (registered second in RULES) fires near the top
        // of the source; `unreachable-code` (registered first) fires
        // later, at the dead `nop` after `stop`. Push order therefore
        // disagrees with source order, so this actually exercises the
        // sort rather than passing by accident.
        let syntax = test_syntax();
        let src = ".func f\nUNUSED: nop\n        stop\n        nop\n";
        let report = lint(&syntax, src, &[]).unwrap();
        assert_eq!(report.len(), 2);
        let starts: Vec<_> = report.iter().map(|d| d.span.start).collect();
        let mut sorted = starts.clone();
        sorted.sort();
        assert_eq!(starts, sorted);
        assert_eq!(report[0].code, "unused-label");
        assert_eq!(report[1].code, "unreachable-code");
    }
}
