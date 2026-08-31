//! Assembly lint layer (docs/core.md (assembly lint)). Arch-agnostic: control flow via
//! `ArchSyntax::Flow`, the break opcode via `ArchSyntax::break_opcode`.
//! Library-only — the CLI renders (docs/core.md, thin-renderer rule).

pub(crate) mod rules;

use super::cst::{AsmCst, parse_asm_cst_with};
use super::lexer::{AsmTokenKind, lex_line};
use super::lower::{SourceFunction, SourceTable, lower_source};
use super::syntax::AsmCaps;
use super::{ArchSyntax, AsmError, assemble_lowered};
use crate::diagnostics::{Diagnostic, Span};

/// Everything a rule may read. Rules never mutate the program.
pub struct AsmLintContext<'a> {
    pub source: &'a str,
    pub cst: &'a AsmCst,
    pub functions: &'a [SourceFunction],
    /// The file-scoped tables lowered from `.section tables` (empty for
    /// every cap-off dialect — PM-1 never shapes a table). Rules read
    /// these for the label references TM-1 keeps outside the code
    /// section: `unused-label` counts a code label as used when a
    /// dispatch `.targets`/`.target` entry or a frame `.exits` descriptor
    /// names it (docs/core.md (assembly lint)).
    pub tables: &'a [SourceTable],
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
    // Parse under the dialect's caps, matching `assemble` (identical to
    // the default parse for every cap-off dialect), then run the shared
    // body — a single parse+lower for both the gate and the rule context.
    let cst = parse_asm_cst_with(source, syntax.caps);
    lint_cst(syntax, source, &cst, allow)
}

/// [`lint`] over an already-parsed CST. A caller that has parsed the
/// source once — the `.pma`/`.tma` language services parse the CST for
/// their document state — passes it here to lint without a re-parse
/// (docs/core.md (assembly lint)). The CST MUST have been parsed under
/// `syntax.caps` (`parse_asm_cst_with(source, syntax.caps)`); a mismatch
/// would lower a differently-shaped CST. Byte-identical findings and
/// fatals to [`lint`] on the same source.
pub fn lint_cst(
    syntax: &ArchSyntax,
    source: &str,
    cst: &AsmCst,
    allow: &[String],
) -> Result<Vec<Diagnostic>, AsmError> {
    // `lower_source` (not the functions-only `lower`) so the rules can
    // reach the tables — the code-label references TM-1 keeps in the
    // lowered table section. Cap-off dialects lower no tables, so this is
    // an empty slice there. The lowering feeds BOTH the fatal gate and
    // the rule context: `assemble_lowered` reuses this one lower rather
    // than re-parsing + re-lowering the source as a full `assemble` would.
    let lowered = lower_source(cst, syntax, source)?;
    assemble_lowered(syntax, 0, &lowered, false)?;

    let ctx = AsmLintContext {
        source,
        cst,
        functions: &lowered.functions,
        tables: &lowered.tables,
        syntax,
    };
    let mut diagnostics = Vec::new();
    for (code, rule) in RULES {
        if allow.iter().any(|a| a == code) {
            continue;
        }
        rule(&ctx, &mut diagnostics);
    }
    // The comment guard: a fix whose edit span touches a comment token is
    // withheld — the finding stays, the remedy goes — because applying it
    // would silently delete the comment; both toolchains' source-language
    // lints hold the same posture (docs/pmt/lint.md and docs/tmt/lint.md
    // (quickfix availability)). ONE chokepoint over every rule's output
    // rather than a check inside each rule, so a fix-emitting rule added
    // later is covered by construction. In practice today the exposure is
    // the whole-line "delete this instruction" edit on an unlabeled line,
    // whose span swallows a trailing comment; the labeled variant and the
    // unused-label edit both end before the comment by CST construction.
    let comments = comment_spans(source, syntax.caps);
    for d in &mut diagnostics {
        let withheld = d.fix.as_ref().is_some_and(|f| {
            f.edits
                .iter()
                .any(|e| span_touches_a_comment(&comments, e.span))
        });
        if withheld {
            d.fix = None;
        }
    }
    diagnostics.sort_by_key(|d| d.span.start); // stable; Pos is Ord
    Ok(diagnostics)
}

/// Every comment token's span in the source, by a caps-faithful lex —
/// the same tokenization the CST was parsed under, so a character that
/// is a comment there is a comment here.
fn comment_spans(source: &str, caps: AsmCaps) -> Vec<Span> {
    let mut out = Vec::new();
    for (i, text) in source.lines().enumerate() {
        for token in lex_line(text, i as u32 + 1, caps) {
            if matches!(token.kind, AsmTokenKind::Comment(_)) {
                out.push(token.span());
            }
        }
    }
    out
}

/// Half-open overlap between a fix edit's span and any comment span.
fn span_touches_a_comment(comments: &[Span], span: Span) -> bool {
    comments
        .iter()
        .any(|c| c.start < span.end && span.start < c.end)
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
        // Channel discipline (docs/core.md (assembly lint)): duplicate/unknown labels are
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
    fn lint_cst_matches_lint_on_the_same_source() {
        // The CST-consuming entry produces byte-identical findings AND
        // fatals to `lint` — it runs the same rules over the same fatal
        // gate, reusing the one parse the caller supplies (docs/core.md
        // (assembly lint)). This cannot compile unless `lint_cst` exists,
        // so it doubles as the single-parse proof.
        let syntax = test_syntax();

        // Findings path: an unused label plus dead code.
        let src = ".func f\nUNUSED: nop\n        stop\n        nop\n";
        let cst = parse_asm_cst_with(src, syntax.caps);
        assert_eq!(
            lint_cst(&syntax, src, &cst, &[]).unwrap(),
            lint(&syntax, src, &[]).unwrap(),
        );

        // Fatal-gate path: an unknown mnemonic refuses the file identically.
        let bad = ".func f\n        bogus\n";
        let cst = parse_asm_cst_with(bad, syntax.caps);
        assert_eq!(
            lint_cst(&syntax, bad, &cst, &[]).unwrap_err(),
            lint(&syntax, bad, &[]).unwrap_err(),
        );
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

    #[test]
    fn a_fix_deleting_a_line_with_a_trailing_comment_is_withheld() {
        // Unlabeled redundant jump: the fix deletes the whole physical
        // line, and the trailing comment sits inside that span — the
        // finding must still report, the fix must go.
        let syntax = test_syntax();
        let src = ".func f\n        jmp L1 ; keep me\nL1:     stop\n";
        let report = lint(&syntax, src, &[]).unwrap();
        let d = report
            .iter()
            .find(|d| d.code == "redundant-jump-to-next")
            .expect("the finding itself must survive the guard");
        assert!(d.fix.is_none(), "fix over a comment must be withheld");
    }

    #[test]
    fn a_labeled_deletion_stops_before_the_comment_and_keeps_its_fix() {
        // "L0:     jmp L1 ; note" — the label-preserving edit runs from
        // the instruction word to the line's trimmed end, which the CST
        // computes EXCLUDING the trailing comment, so the fix touches no
        // comment and stays offered.
        let syntax = test_syntax();
        let src = ".func f\nL0:     jmp L1 ; note\nL1:     stop\n";
        let report = lint(&syntax, src, &[]).unwrap();
        let d = report
            .iter()
            .find(|d| d.code == "redundant-jump-to-next")
            .unwrap();
        assert!(d.fix.is_some(), "an edit clear of comments keeps its fix");
    }

    #[test]
    fn leftover_debugger_fix_is_withheld_over_a_trailing_comment() {
        // Same whole-line deletion shape as the redundant jump, through
        // the other deleting rule.
        let syntax = debugger_syntax();
        let src = ".func f\n        dbg ; breadcrumb\n        stop\n";
        let report = lint(&syntax, src, &[]).unwrap();
        let d = report
            .iter()
            .find(|d| d.code == "leftover-debugger")
            .expect("the finding itself must survive the guard");
        assert!(d.fix.is_none(), "fix over a comment must be withheld");
    }

    #[test]
    fn a_comment_outside_the_edit_span_does_not_withhold() {
        // An own-line comment above the jump is no reason to void its
        // fix — the guard keys on the EDIT span, not the line vicinity.
        let syntax = test_syntax();
        let src = ".func f\n; setup\n        jmp L1\nL1:     stop\n";
        let report = lint(&syntax, src, &[]).unwrap();
        let d = report
            .iter()
            .find(|d| d.code == "redundant-jump-to-next")
            .unwrap();
        assert!(d.fix.is_some());
    }
}
