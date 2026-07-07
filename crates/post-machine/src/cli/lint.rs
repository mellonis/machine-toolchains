//! `pmt lint` (docs/cli.md, docs/lint.md): thin renderer over the lint
//! library. Findings go to stdout; exit 0 = clean, 1 = findings or
//! errors anywhere.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::diagnostics::{Applicability, Diagnostic};

use crate::lint::{LintError, LintOptions, apply_fixes, lint as lint_source};

use super::{Args, CliOutput};

const LINT_USAGE: &str = "\
USAGE: pmt lint PATH... [--exclude PATH]... [--allow CODE]... [--fix [--force]]

PATH is a .pmc file or a directory; directories are walked recursively
for *.pmc (sorted order, symlinks not followed, dot-entries skipped).

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs); exclusion
                  wins even over explicitly listed files
  --allow CODE    suppress a lint rule by code (repeatable;
                  unknown codes are an error)
  --fix           apply machine-applicable fixes in place, then re-lint;
                  the report and exit code reflect what REMAINS
  --force         with --fix: also apply the gated fixes (deletions and
                  rewrites whose diagnosis may have another reading)
";

pub(super) fn lint(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LINT_USAGE.into(), String::new()));
    }
    let allow = args.values("--allow")?;
    let excludes: Vec<PathBuf> = args
        .values("--exclude")?
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let fix = args.flag("--fix");
    let force = args.flag("--force");
    if force && !fix {
        return Err(format!("--force requires --fix\n\n{LINT_USAGE}"));
    }
    let paths = args.positionals()?;
    if paths.is_empty() {
        return Err(format!("lint takes at least one PATH\n\n{LINT_USAGE}"));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    for p in &paths {
        let path = Path::new(p);
        let found = collect_pmc(path, &excludes, &mut files)?;
        if found == 0 {
            return Err(format!("{p}: no .pmc files found"));
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut any = false;
    for file in &files {
        let source =
            fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        match lint_source(
            &source,
            LintOptions {
                allow: allow.clone(),
            },
        ) {
            Ok(report) => {
                let diags = if fix {
                    // Mask fixes outside the allowed tier, apply, rewrite,
                    // then re-lint: the report reflects what REMAINS.
                    let masked: Vec<Diagnostic> = report
                        .diagnostics
                        .iter()
                        .cloned()
                        .map(|mut d| {
                            let gated = matches!(
                                d.fix.as_ref().map(|f| &f.applicability),
                                Some(Applicability::MaybeIncorrect)
                            );
                            if gated && !force {
                                d.fix = None;
                            }
                            d
                        })
                        .collect();
                    let outcome = apply_fixes(&source, &masked);
                    if outcome.applied > 0 {
                        fs::write(file, &outcome.fixed_source)
                            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
                        match lint_source(
                            &outcome.fixed_source,
                            LintOptions {
                                allow: allow.clone(),
                            },
                        ) {
                            Ok(rerun) => rerun.diagnostics,
                            Err(e) => return Err(e.to_string()),
                        }
                    } else {
                        report.diagnostics
                    }
                } else {
                    report.diagnostics
                };
                if !diags.is_empty() {
                    any = true;
                }
                render_findings(&mut stdout, file, &diags);
            }
            Err(LintError::Compile(e)) => {
                // Per-file fatal: report, keep going (batch model).
                any = true;
                let _ = writeln!(
                    stderr,
                    "{}:{}:{}: error: {}",
                    file.display(),
                    e.span.start.line,
                    e.span.start.col,
                    e.kind
                );
            }
            Err(e @ LintError::UnknownAllowCode(_)) => return Err(e.to_string()),
        }
    }
    Ok(CliOutput {
        stdout,
        stderr,
        code: u8::from(any),
    })
}

/// Walk one PATH argument. Returns how many `.pmc` files the PATH
/// yielded BEFORE exclusion (zero = the caller's typo error); excluded
/// files are counted but not collected — an excluded PATH is not a typo.
///
/// `pub(super)`: `cli/fmt.rs` shares this walk verbatim rather than
/// duplicating it (docs/cli.md (pmt fmt) — identical batch model).
pub(super) fn collect_pmc(
    path: &Path,
    excludes: &[PathBuf],
    out: &mut Vec<PathBuf>,
) -> Result<usize, String> {
    let excluded = |p: &Path| excludes.iter().any(|e| p.starts_with(e));
    let meta =
        fs::symlink_metadata(path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
    if meta.is_symlink() {
        return Ok(0); // never followed
    }
    if meta.is_file() {
        // An explicit file is linted as given (any extension) unless excluded.
        if !excluded(path) {
            out.push(path.to_path_buf());
        }
        return Ok(1);
    }
    if excluded(path) {
        return Ok(1); // pruned subtree still "matched" — not a typo
    }
    let mut entries: Vec<_> = fs::read_dir(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    entries.sort_by_key(|e| e.file_name());
    let mut found = 0usize;
    for entry in entries {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue; // dot-entries: .git, scratch dirs
        }
        let child = entry.path();
        let meta = fs::symlink_metadata(&child)
            .map_err(|e| format!("cannot stat {}: {e}", child.display()))?;
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            found += collect_pmc(&child, excludes, out)?;
        } else if child.extension().is_some_and(|x| x == "pmc") {
            found += 1;
            if !excluded(&child) {
                out.push(child);
            }
        }
    }
    Ok(found)
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
