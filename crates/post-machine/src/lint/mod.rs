//! `.pmc` lint layer (docs/lint.md): hygiene findings over the compiler's
//! analysis. Library-only — the CLI renders (docs/cli.md). Strict channel
//! split: lint reports lint findings ONLY; the compile warnings stay on
//! the compile channel and are never re-reported here.

pub mod rules;

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::compiler::{self, CompileError, ScopeSummary};
use crate::ir::IrProgram;
use crate::lexer::Token;
use crate::parser::Program;

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
    /// Unoptimized CFG — rules judge source hygiene, not optimizer output.
    #[allow(dead_code)]
    pub ir: &'a IrProgram,
    #[allow(dead_code)]
    pub scopes: &'a ScopeSummary,
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
];

pub fn lint(source: &str, options: LintOptions) -> Result<LintReport, LintError> {
    for code in &options.allow {
        if !RULES.iter().any(|(c, _)| c == code) {
            return Err(LintError::UnknownAllowCode(code.clone()));
        }
    }
    let analysis = compiler::analyze(source)?;
    let ctx = LintContext {
        source,
        tokens: &analysis.tokens,
        ast: &analysis.ast,
        ir: &analysis.ir,
        scopes: &analysis.scopes,
    };
    let mut diagnostics = Vec::new();
    for (code, rule) in RULES {
        if options.allow.iter().any(|a| a == code) {
            continue;
        }
        rule(&ctx, &mut diagnostics);
    }
    diagnostics.sort_by_key(|d| d.span.start); // stable; Pos is Ord
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
}
