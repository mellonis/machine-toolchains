//! `pmt lint` (docs/cli.md, docs/lint.md): thin renderer over the lint
//! library. Findings go to stdout; exit 0 = clean, 1 = findings or
//! errors anywhere.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use mtc_core::diagnostics::{Applicability, Diagnostic};

use crate::lint::{LintOptions, lint as lint_source};

use super::{Args, CliOutput};

const LINT_USAGE: &str = "\
USAGE: pmt lint PATH [--allow CODE]...

FLAGS:
  --allow CODE   suppress a lint rule by code (repeatable;
                 unknown codes are an error)
";

pub(super) fn lint(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LINT_USAGE.into(), String::new()));
    }
    let allow = args.values("--allow")?;
    let paths = args.positionals()?;
    let [path] = paths.as_slice() else {
        return Err(format!("lint takes exactly one input\n\n{LINT_USAGE}"));
    };
    let path = Path::new(path);

    let source =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let report = lint_source(&source, LintOptions { allow }).map_err(|e| e.to_string())?;

    let mut stdout = String::new();
    render_findings(&mut stdout, path, &report.diagnostics);
    let code = u8::from(!report.diagnostics.is_empty());
    Ok(CliOutput {
        stdout,
        stderr: String::new(),
        code,
    })
}

/// `{file}:{line}:{col}: lint: {message}` plus an indented fix-hint line;
/// a gated fix names its gate so plain `--fix` runs explain themselves.
pub(super) fn render_findings(out: &mut String, path: &Path, diags: &[Diagnostic]) {
    for d in diags {
        let _ = writeln!(
            out,
            "{}:{}:{}: lint: {}",
            path.display(),
            d.span.start.line,
            d.span.start.col,
            d.message
        );
        if let Some(fix) = &d.fix {
            let _ = match fix.applicability {
                Applicability::MachineApplicable => {
                    writeln!(out, "  fix: {}", fix.description)
                }
                Applicability::MaybeIncorrect => {
                    writeln!(out, "  fix (requires --force): {}", fix.description)
                }
            };
        }
    }
}
