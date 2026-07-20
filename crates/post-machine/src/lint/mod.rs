//! `.pmc` lint layer (docs/pmt/lint.md): hygiene findings over the compiler's
//! analysis. Library-only — the CLI renders (docs/pmt/cli.md). Strict channel
//! split: lint reports lint findings ONLY; the compile warnings stay on
//! the compile channel and are never re-reported here.

pub mod fixes;
pub mod rules;

pub use fixes::{FixOutcome, apply_fixes};

use std::collections::HashMap;

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::compiler::{self, CompileError, ScopeSummary};
use crate::lexer::Token;
use crate::parser::{FnDoc, Program};

#[derive(Debug, Clone, Default)]
pub struct LintOptions {
    /// Rule codes to suppress. Unknown codes are an error (typo protection).
    pub allow: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LintReport {
    /// Lint findings only, source-ordered by span start (stable).
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub enum LintError {
    /// Lint requires a program that parses and resolves.
    Compile(CompileError),
    /// `--allow` named a code no rule declares.
    UnknownAllowCode(String),
}

impl std::fmt::Display for LintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintError::Compile(e) => write!(f, "{e}"),
            LintError::UnknownAllowCode(code) => {
                write!(f, "unknown lint rule `{code}` in --allow")
            }
        }
    }
}

impl std::error::Error for LintError {}

impl From<CompileError> for LintError {
    fn from(e: CompileError) -> Self {
        LintError::Compile(e)
    }
}

/// Everything a rule may read. Rules never mutate the program.
pub(crate) struct LintContext<'a> {
    pub source: &'a str,
    pub tokens: &'a [Token],
    /// FLATTENED program: function names are fully qualified
    /// (`std::api.helper`); statement/item shapes are untouched.
    pub ast: &'a Program,
    pub scopes: &'a ScopeSummary,
    /// Every documented function's [`FnDoc`], keyed by the same
    /// fully-qualified name carried on `ast`'s `Function::name` /
    /// `Item::Call::name` (`Analysis.docs`/`AnalysisOutput.docs`).
    pub docs: &'a HashMap<String, FnDoc>,
}

/// A lint rule: reads the analysis context, pushes any findings.
type Rule = fn(&LintContext, &mut Vec<Diagnostic>);

/// The rule table. One entry per rule, keyed by its defect-named code;
/// registration order is irrelevant (findings are sorted by span).
pub(crate) const RULES: &[(&str, Rule)] = &[
    ("line-too-long", rules::line_too_long::check),
    ("leading-zeros", rules::leading_zeros::check),
    ("unused-label", rules::unused_label::check),
    ("redundant-jump-to-next", rules::redundant_jump::check),
    ("identical-check-arms", rules::identical_check_arms::check),
    ("leftover-debugger", rules::leftover_debugger::check),
    ("namespaced-main", rules::namespaced_main::check),
    ("shadowed-import", rules::shadowed_import::check),
    ("non-camel-case", rules::non_camel_case::check),
    ("confusable-names", rules::confusable_names::check),
    ("deprecated-call", rules::deprecated_call::check),
];

/// `--allow` codes must each name a real rule (typo protection), over the
/// UNION of this crate's `.pmc` rule table and core's `.pma` asm rule
/// table (`mtc_core::asm::lint::RULES`): a `pmt.json` shared by both
/// languages carries one `lint.allow` list, so a `.pma`-only code must
/// not error when validated for a `.pmc` file, and vice versa. Split out
/// of `lint()` so the LSP (a future `PmcLanguageService`) can validate an
/// IDE-settings or `pmt.json` allow-list up front, independently of
/// running the rules over any particular analysis.
pub(crate) fn validate_allow(codes: &[String]) -> Result<(), LintError> {
    for code in codes {
        let known = RULES.iter().any(|(c, _)| c == code)
            || mtc_core::asm::lint::RULES.iter().any(|(c, _)| c == code);
        if !known {
            return Err(LintError::UnknownAllowCode(code.clone()));
        }
    }
    Ok(())
}

/// Run every non-allowed rule over `ctx`, source-ordered by span start
/// (stable). Split out of `lint()` so the LSP can lint an `Analysis` it
/// already staged, instead of re-running `compiler::analyze`.
pub(crate) fn run_rules(ctx: &LintContext, allow: &[String]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (code, rule) in RULES {
        if allow.iter().any(|a| a == code) {
            continue;
        }
        rule(ctx, &mut diagnostics);
    }
    diagnostics.sort_by_key(|d| d.span.start); // stable; Pos is Ord
    diagnostics
}

pub fn lint(source: &str, options: LintOptions) -> Result<LintReport, LintError> {
    validate_allow(&options.allow)?;
    let analysis = compiler::analyze(source)?;
    let ctx = LintContext {
        source,
        tokens: &analysis.tokens,
        ast: &analysis.ast,
        scopes: &analysis.scopes,
        docs: &analysis.docs,
    };
    let diagnostics = run_rules(&ctx, &options.allow);
    Ok(LintReport { diagnostics })
}

/// Slice `source` by a char-counted span (1-based line/col, end-exclusive).
pub(crate) fn span_text(source: &str, span: Span) -> String {
    let mut out = String::new();
    let (mut line, mut col) = (1u32, 1u32);
    for c in source.chars() {
        let pos = mtc_core::diagnostics::Pos { line, col };
        if pos >= span.start && pos < span.end {
            out.push(c);
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_program_with_no_rules_yields_empty_report() {
        let report = lint("main() { right; }", LintOptions::default()).unwrap();
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn unknown_allow_code_is_an_error() {
        let err = lint(
            "main() { right; }",
            LintOptions {
                allow: vec!["no-such-rule".into()],
            },
        )
        .unwrap_err();
        assert!(matches!(err, LintError::UnknownAllowCode(ref c) if c == "no-such-rule"));
        assert!(err.to_string().contains("no-such-rule"));
    }

    #[test]
    fn fatal_parse_error_propagates() {
        let err = lint("main( {", LintOptions::default()).unwrap_err();
        assert!(matches!(err, LintError::Compile(_)));
    }

    #[test]
    fn span_text_slices_by_char_positions() {
        use mtc_core::diagnostics::Span;
        let src = "ab\ncdef\n";
        assert_eq!(span_text(src, Span::new(2, 2, 2, 4)), "de");
        // Multi-line span crosses the newline.
        assert_eq!(span_text(src, Span::new(1, 2, 2, 2)), "b\nc");
    }

    #[test]
    fn validate_allow_accepts_known_codes_and_rejects_unknown_ones() {
        assert!(validate_allow(&["unused-label".to_string()]).is_ok());
        assert!(validate_allow(&[]).is_ok());
        let err = validate_allow(&["no-such-rule".to_string()]).unwrap_err();
        assert!(matches!(err, LintError::UnknownAllowCode(ref c) if c == "no-such-rule"));
    }

    #[test]
    fn validate_allow_also_accepts_asm_only_codes() {
        // "unreachable-code" names no pmc rule — it's asm-only
        // (`mtc_core::asm::lint::RULES`). A pmt.json shared by both
        // languages must not choke on it while validating for `.pmc`.
        assert!(!RULES.iter().any(|(c, _)| *c == "unreachable-code"));
        assert!(
            mtc_core::asm::lint::RULES
                .iter()
                .any(|(c, _)| *c == "unreachable-code")
        );
        assert!(validate_allow(&["unreachable-code".to_string()]).is_ok());
    }

    #[test]
    fn run_rules_filters_allowed_codes_and_sorts_by_span() {
        let src = "\
main() {
007: right;
5:   left;
     goto 007;
     debugger;
}
";
        let analysis = compiler::analyze(src).unwrap();
        let ctx = LintContext {
            source: src,
            tokens: &analysis.tokens,
            ast: &analysis.ast,
            scopes: &analysis.scopes,
            docs: &analysis.docs,
        };

        let all = run_rules(&ctx, &[]);
        assert!(!all.is_empty());
        let mut sorted_starts: Vec<_> = all.iter().map(|d| d.span.start).collect();
        sorted_starts.sort();
        let starts: Vec<_> = all.iter().map(|d| d.span.start).collect();
        assert_eq!(starts, sorted_starts, "run_rules must sort by span start");

        assert!(all.iter().any(|d| d.code == "leftover-debugger"));
        let filtered = run_rules(&ctx, &["leftover-debugger".to_string()]);
        assert!(filtered.iter().all(|d| d.code != "leftover-debugger"));
        assert_eq!(filtered.len() + 1, all.len());
    }

    #[test]
    fn lint_output_is_byte_identical_across_the_validate_allow_run_rules_split() {
        // Regression pin (LSP plan 2, Task 4): `lint()` used to inline the
        // RULES-membership check and the rule loop + span sort; both moved
        // into `validate_allow`/`run_rules`. This fixture, run through the
        // still-public `lint()` entry, must keep producing exactly these
        // findings, in exactly this order, before and after the split.
        const FIXTURE: &str = include_str!("../../tests/lint/unused_labels.pmc");
        let report = lint(FIXTURE, LintOptions::default()).unwrap();
        let codes: Vec<(&str, u32, u32)> = report
            .diagnostics
            .iter()
            .map(|d| (d.code, d.span.start.line, d.span.start.col))
            .collect();
        assert_eq!(codes, vec![("unused-label", 4, 1), ("unused-label", 12, 1)]);
    }
}
