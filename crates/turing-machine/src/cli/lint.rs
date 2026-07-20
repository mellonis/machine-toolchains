//! `tmt lint` (docs/cli.md, once it lands; substance in prose until then): a
//! thin renderer over the `.tmc` lint library. Findings go to stdout; exit
//! 0 = clean, 1 = findings or errors anywhere. Mirrors `pmt lint`'s shape —
//! dirs-and-files positionals, per-file `tmt.json` union, batch-keeps-going —
//! with two `.tmc`-family differences: a `--warn` flag turns on the opt-in
//! rules, and there is no `--fix` (no `.tmc` or `.tma` rule emits a
//! machine-applicable fix — the fix surface is the PM-1 crate's for now).
//! Both languages lint by extension: `.tmc` through the `.tmc` rule table,
//! `.tma` through core's five arch-agnostic asm rules plus the TM-1
//! additions.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::config;
use crate::lint::{LintError, LintOptions, lint as lint_source};

use super::{Args, CliOutput};

const LINT_USAGE: &str = "\
USAGE: tmt lint PATH... [--exclude PATH]... [--allow CODE]... [--warn CODE]... [--no-config]

PATH is a .tmc or .tma file, or a directory; directories are walked
recursively for *.tmc and *.tma (sorted order, symlinks not followed,
dot-entries skipped). .tmc sources lint through the .tmc rule table;
.tma sources through the five arch-agnostic asm rules plus the TM-1
additions (shadowed rows, retx exit bounds, unused rept vars).

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs); exclusion
                  wins even over explicitly listed files
  --allow CODE    suppress a lint rule by code (repeatable; unknown codes
                  are an error)
  --warn CODE     enable an opt-in rule by code (repeatable; e.g.
                  state-may-trap, off unless named here)
  --no-config     ignore tmt.json project files
";

pub(super) fn lint(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LINT_USAGE.into(), String::new()));
    }
    let allow = args.values("--allow")?;
    let warn = args.values("--warn")?;
    // Up front, over the shared namespace: a typo'd `--allow`/`--warn` aborts
    // the whole run before any file is touched. Per-file `tmt.json` merges are
    // validated separately at load time; this only covers the flags' codes.
    crate::lint::validate_allow(&allow).map_err(|e| e.to_string())?;
    crate::lint::validate_allow(&warn).map_err(|e| e.to_string())?;
    let excludes: Vec<PathBuf> = args
        .values("--exclude")?
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let no_config = args.flag("--no-config");
    let paths = args.positionals()?;
    if paths.is_empty() {
        return Err(format!("lint takes at least one PATH\n\n{LINT_USAGE}"));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    for p in &paths {
        let found = collect_sources(Path::new(p), &excludes, &mut files)?;
        if found == 0 {
            return Err(format!("{p}: no .tmc or .tma files found"));
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut any = false;
    'files: for file in &files {
        // Per-file project config: the nearest `tmt.json` ancestor, unless
        // suppressed by `--no-config`. A bad config is a per-file fatal —
        // unlike a bad `--allow` flag above, which aborts the whole run.
        let mut effective_allow = allow.clone();
        if !no_config && let Some(config_path) = file.parent().and_then(config::discover) {
            match config::load(&config_path) {
                Ok(project) => {
                    for code in project.allow {
                        if !effective_allow.contains(&code) {
                            effective_allow.push(code);
                        }
                    }
                }
                Err(e) => {
                    any = true;
                    let _ = writeln!(stderr, "{}: error: {}", e.path().display(), e.detail());
                    continue 'files;
                }
            }
        }

        let source =
            fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;

        match file.extension().and_then(|x| x.to_str()) {
            Some("tmc") => match lint_source(
                &source,
                LintOptions {
                    allow: effective_allow.clone(),
                    warn: warn.clone(),
                },
            ) {
                Ok(report) => {
                    if !report.diagnostics.is_empty() {
                        any = true;
                    }
                    render_findings(&mut stdout, file, &report.diagnostics);
                }
                Err(LintError::Compile(e)) => {
                    // Per-file fatal: report, keep going (batch model).
                    any = true;
                    render_fatal(&mut stderr, file, e.span, &e.kind, e.kind.code());
                }
                Err(e @ LintError::UnknownAllowCode(_)) => return Err(e.to_string()),
            },
            Some("tma") => {
                // Cheap per-file re-check over the shared namespace: the flag
                // allow was validated once up front, but a per-file `tmt.json`
                // may have merged codes in. `lint_tma` does not validate allow
                // itself (core's asm lint owns none of it), so this crate does
                // — same as the `.tmc` route above and pmt's `.pma` route.
                if let Err(e) = crate::lint::validate_allow(&effective_allow) {
                    return Err(e.to_string());
                }
                match crate::lint::tma::lint_tma(&source, &effective_allow) {
                    Ok(diags) => {
                        if !diags.is_empty() {
                            any = true;
                        }
                        render_findings(&mut stdout, file, &diags);
                    }
                    Err(e) => {
                        // Per-file fatal (the assemble gate): report, keep
                        // going (batch model, same shape as the `.tmc` route).
                        any = true;
                        render_fatal(&mut stderr, file, e.span, &e.kind, e.kind.code());
                    }
                }
            }
            _ => {
                // Only reachable for an explicitly listed file — the directory
                // walk only ever collects `.tmc`/`.tma` extensions.
                any = true;
                let _ = writeln!(
                    stderr,
                    "{}: error: unknown source extension (expected .tmc or .tma)",
                    file.display()
                );
            }
        }
    }
    Ok(CliOutput {
        stdout,
        stderr,
        code: u8::from(any),
    })
}

/// Walk one PATH argument. Returns how many `.tmc`/`.tma` files the PATH
/// yielded BEFORE exclusion (zero = the caller's typo error); excluded files
/// are counted but not collected. Mirrors `pmt lint`'s walk; `.tma` is
/// collected so the directory sweep is complete even though its lint is not
/// wired yet.
fn collect_sources(
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
            found += collect_sources(&child, excludes, out)?;
        } else if child.extension().is_some_and(|x| x == "tmc" || x == "tma") {
            found += 1;
            if !excluded(&child) {
                out.push(child);
            }
        }
    }
    Ok(found)
}

/// The per-file fatal line: `{file}:{line}:{col}: error: {kind} [{code}]`.
fn render_fatal(
    stderr: &mut String,
    file: &Path,
    span: Span,
    kind: &dyn std::fmt::Display,
    code: &str,
) {
    let _ = writeln!(
        stderr,
        "{}:{}:{}: error: {} [{}]",
        file.display(),
        span.start.line,
        span.start.col,
        kind,
        code
    );
}

/// `{file}:{line}:{col}: lint: {message}`, one line per finding.
fn render_findings(out: &mut String, path: &Path, diags: &[Diagnostic]) {
    for d in diags {
        let _ = writeln!(
            out,
            "{}:{}:{}: lint: {}",
            path.display(),
            d.span.start.line,
            d.span.start.col,
            d.message
        );
    }
}
