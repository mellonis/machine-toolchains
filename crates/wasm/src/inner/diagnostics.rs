//! Findings and fatals in the shape the editor consumes, and the two
//! source-text services that produce them without building: `check`
//! (the lint channel) and `format`. Both serve all four languages; on
//! assembly the fatal is the assembler's refusal, the same gate `pmt
//! lint`/`tmt lint` put in front of a `.pma`/`.tma`.

use mtc_core::asm::AsmError;
use mtc_core::diagnostics::{Applicability, Diagnostic};

use super::Lang;
use super::positions::Utf16Index;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub from: u32,
    pub to: u32,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub description: String,
    pub machine_applicable: bool,
    pub edits: Vec<Edit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub code: String,
    pub severity: Severity,
    pub from: u32,
    pub to: u32,
    pub message: String,
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    pub allow: Vec<String>,
    /// TM-1's opt-in warn tier (`state-may-trap`, `index-identity-map`);
    /// ignored for the other three languages, which have no such tier.
    pub warn: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// A rule name in `allow`/`warn` the lint layer does not know — a
    /// caller bug, thrown rather than reported as a finding.
    UnknownAllowCode(String),
}

pub fn from_core(idx: &Utf16Index, d: &Diagnostic, severity: Severity) -> Diag {
    let (from, to) = idx.span(&d.span);
    Diag {
        code: d.code.to_string(),
        severity,
        from,
        to,
        message: d.message.clone(),
        fix: d.fix.as_ref().map(|f| Fix {
            description: f.description.clone(),
            machine_applicable: matches!(f.applicability, Applicability::MachineApplicable),
            edits: f
                .edits
                .iter()
                .map(|e| {
                    let (from, to) = idx.span(&e.span);
                    Edit {
                        from,
                        to,
                        replacement: e.replacement.clone(),
                    }
                })
                .collect(),
        }),
    }
}

pub fn pm_fatal(idx: &Utf16Index, e: &mtc_post_machine::compiler::CompileError) -> Diag {
    let (from, to) = idx.span(&e.span);
    Diag {
        code: e.kind.code().to_string(),
        severity: Severity::Error,
        from,
        to,
        message: e.to_string(),
        fix: None,
    }
}

pub fn tm_fatal(idx: &Utf16Index, e: &mtc_turing_machine::compiler::CompileError) -> Diag {
    let (from, to) = idx.span(&e.span);
    Diag {
        code: e.kind.code().to_string(),
        severity: Severity::Error,
        from,
        to,
        message: e.to_string(),
        fix: None,
    }
}

/// The assembler's refusal as the one error, the way a compile fatal is:
/// its coded kind, its span, its own rendering.
pub fn asm_fatal(idx: &Utf16Index, e: &AsmError) -> Diag {
    let (from, to) = idx.span(&e.span);
    Diag {
        code: e.kind.code().to_string(),
        severity: Severity::Error,
        from,
        to,
        message: e.to_string(),
        fix: None,
    }
}

/// The lint channel: findings as warnings, a compile (or assemble) fatal
/// as one error.
pub fn check(lang: Lang, source: &str, opts: &CheckOptions) -> Result<Vec<Diag>, CheckError> {
    let idx = Utf16Index::new(source);
    let warnings = |ds: &[Diagnostic]| {
        ds.iter()
            .map(|d| from_core(&idx, d, Severity::Warning))
            .collect::<Vec<_>>()
    };
    match lang {
        Lang::Pmc => {
            use mtc_post_machine::lint::{LintError, LintOptions, lint};
            let options = LintOptions {
                allow: opts.allow.clone(),
            };
            match lint(source, options) {
                Ok(report) => Ok(warnings(&report.diagnostics)),
                Err(LintError::Compile(e)) => Ok(vec![pm_fatal(&idx, &e)]),
                Err(LintError::UnknownAllowCode(c)) => Err(CheckError::UnknownAllowCode(c)),
            }
        }
        Lang::Tmc => {
            use mtc_turing_machine::lint::{LintError, LintOptions, lint};
            let options = LintOptions {
                allow: opts.allow.clone(),
                warn: opts.warn.clone(),
            };
            match lint(source, options) {
                Ok(report) => Ok(warnings(&report.diagnostics)),
                Err(LintError::Compile(e)) => Ok(vec![tm_fatal(&idx, &e)]),
                Err(LintError::UnknownAllowCode(c)) => Err(CheckError::UnknownAllowCode(c)),
            }
        }
        // Core's asm lint validates no allow codes itself; each arch crate
        // owns the shared namespace, the same split its CLI keeps.
        Lang::Pma => {
            use mtc_post_machine::lint::{LintError, validate_allow};
            if let Err(LintError::UnknownAllowCode(c)) = validate_allow(&opts.allow) {
                return Err(CheckError::UnknownAllowCode(c));
            }
            let syntax = mtc_post_machine::asm::pm1_syntax();
            match mtc_core::asm::lint::lint(&syntax, source, &opts.allow) {
                Ok(ds) => Ok(warnings(&ds)),
                Err(e) => Ok(vec![asm_fatal(&idx, &e)]),
            }
        }
        Lang::Tma => {
            use mtc_turing_machine::lint::{LintError, tma::lint_tma, validate_allow};
            if let Err(LintError::UnknownAllowCode(c)) = validate_allow(&opts.allow) {
                return Err(CheckError::UnknownAllowCode(c));
            }
            match lint_tma(source, &opts.allow) {
                Ok(ds) => Ok(warnings(&ds)),
                Err(e) => Ok(vec![asm_fatal(&idx, &e)]),
            }
        }
    }
}

/// Canonical, whitespace-only formatting of the whole text; the fatal a
/// broken source raises comes back as the diagnostic it would be in `check`.
pub fn format(lang: Lang, source: &str) -> Result<String, Diag> {
    let idx = Utf16Index::new(source);
    match lang {
        Lang::Pmc => mtc_post_machine::fmt::format(source).map_err(|e| pm_fatal(&idx, &e)),
        Lang::Tmc => mtc_turing_machine::fmt::format(source).map_err(|e| tm_fatal(&idx, &e)),
        Lang::Pma => mtc_post_machine::asm::format_asm(source).map_err(|e| asm_fatal(&idx, &e)),
        Lang::Tma => {
            mtc_core::asm::format_asm_with(source, mtc_turing_machine::asm::tm1_syntax().caps)
                .map_err(|e| asm_fatal(&idx, &e))
        }
    }
}
