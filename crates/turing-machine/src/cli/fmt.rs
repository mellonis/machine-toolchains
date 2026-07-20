//! `tmt fmt` (docs/tmt/cli.md (tmt fmt)): a thin renderer over the formatters. The `.tma` side wires core's
//! canonical-grid printer (`mtc_core::asm::format_asm_with` under
//! `tm1_syntax()`'s caps, so sections / table directives / `.rept` blocks /
//! frame descriptors / vector operands all normalize); the `.tmc` side wires
//! the crate's own CST printer ([`crate::fmt::format`]). Both are
//! whitespace-only and idempotent, so `--check` is a safe CI gate for either
//! language. Batch model (`PATH...`) is IDENTICAL to `tmt lint`'s, so
//! it shares [`super::lint::collect_sources`] rather than duplicating the
//! walk, and the per-file parse fatal reuses [`super::lint::render_fatal`].
//! Mirrors the PM-1 `pmt fmt` shape (`crates/post-machine/src/cli/fmt.rs`).

use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use mtc_core::asm::format_asm_with;

use crate::asm::tm1_syntax;

use super::lint::{collect_sources, render_fatal};
use super::{Args, CliOutput};

const FMT_USAGE: &str = "\
USAGE: tmt fmt PATH... [--exclude PATH]... [--check]
       tmt fmt - [--check] [--lang tmc|tma]

PATH is a .tmc or .tma file, or a directory; directories are walked
recursively for *.tmc and *.tma (sorted order, symlinks not followed,
dot-entries skipped). `-` reads one source from stdin and writes the
result to stdout; it cannot be combined with PATH arguments.

.tma sources format through the canonical assembly grid; .tmc sources
through the language's own canonical form (the state-block grid, the
80-column argument-list threshold). Both rewrites are whitespace-only.

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs)
  --check         do not write; with PATH..., list files that would be
                  reformatted and exit 1 if any would change; with -,
                  exit 1 if stdin would change (CI mode)
  --lang LANG     stdin's language: tmc (default) or tma; applies to
                  stdin (-) only — an error alongside PATH arguments,
                  whose language always comes from the file extension
";

/// stdin's language for `tmt fmt -`, defaulted from `--lang`. PATH batches
/// never need this: each file's extension already says which formatter
/// applies.
#[derive(Clone, Copy)]
enum Lang {
    Tmc,
    Tma,
}

fn parse_lang(value: Option<&str>) -> Result<Lang, String> {
    match value {
        None | Some("tmc") => Ok(Lang::Tmc),
        Some("tma") => Ok(Lang::Tma),
        Some(_) => Err(format!("`--lang` takes tmc or tma\n\n{FMT_USAGE}")),
    }
}

/// Format one `.tma` source through core's canonical grid under the TM-1
/// caps. `Err` is the structural gate (a Raw, non-assembly line); everything
/// else formats. The span/kind/code triple renders through [`render_fatal`].
fn format_tma(source: &str) -> Result<String, (mtc_core::diagnostics::Span, String, &'static str)> {
    format_asm_with(source, tm1_syntax().caps)
        .map_err(|e| (e.span, e.kind.to_string(), e.kind.code()))
}

/// Format one `.tmc` source. A lex/parse fatal reports through the same
/// span/kind/code triple as the `.tma` route, so both languages render one
/// diagnostic shape.
fn format_tmc(source: &str) -> Result<String, (mtc_core::diagnostics::Span, String, &'static str)> {
    crate::fmt::format(source).map_err(|e| (e.span, e.kind.to_string(), e.kind.code()))
}

pub(super) fn fmt(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(FMT_USAGE.into(), String::new()));
    }
    let check = args.flag("--check");
    let lang = args.value("--lang")?;
    let excludes: Vec<PathBuf> = args
        .values("--exclude")?
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let paths = args.positionals()?;

    if paths.iter().any(|p| p == "-") {
        if paths.len() != 1 {
            return Err(format!(
                "`-` (stdin) cannot be combined with other paths\n\n{FMT_USAGE}"
            ));
        }
        return fmt_stdin(check, parse_lang(lang.as_deref())?);
    }
    if lang.is_some() {
        return Err(format!("--lang applies to stdin (-) only\n\n{FMT_USAGE}"));
    }
    if paths.is_empty() {
        return Err(format!(
            "fmt takes at least one PATH (or `-`)\n\n{FMT_USAGE}"
        ));
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
    // Writing a changed file (default mode) is still success (exit 0) — only
    // `--check` turns "would change" into a nonzero exit; a parse fatal or a
    // not-yet-implemented `.tmc` is nonzero regardless of `--check`.
    let mut would_change = false;
    let mut had_error = false;
    for file in &files {
        let source =
            fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let formatted = match file.extension().and_then(|x| x.to_str()) {
            Some("tma") => Some(format_tma(&source)),
            Some("tmc") => Some(format_tmc(&source)),
            _ => None,
        };
        match formatted {
            Some(Ok(formatted)) => {
                if formatted != source {
                    would_change = true;
                    if check {
                        let _ = writeln!(stdout, "{}", file.display());
                    } else {
                        fs::write(file, &formatted)
                            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
                    }
                }
            }
            Some(Err((span, kind, code))) => {
                // Per-file fatal: report, keep going (batch model).
                had_error = true;
                render_fatal(&mut stderr, file, span, &kind, code);
            }
            None => {
                // Only reachable for an explicitly listed file — the directory
                // walk only ever collects `.tmc`/`.tma` extensions.
                had_error = true;
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
        code: u8::from(had_error || (check && would_change)),
    })
}

/// `-`: read one source from stdin (language selected by `lang`, default
/// tmc), format it, write to stdout (or, under `--check`, write nothing and
/// only signal via the exit code). A single input, so a fatal is a
/// whole-tool error, not a batch entry.
fn fmt_stdin(check: bool, lang: Lang) -> Result<CliOutput, String> {
    let mut source = String::new();
    std::io::stdin()
        .read_to_string(&mut source)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    let result = match lang {
        Lang::Tmc => format_tmc(&source),
        Lang::Tma => format_tma(&source),
    };
    match result {
        Ok(formatted) => {
            if check {
                Ok(CliOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    code: u8::from(formatted != source),
                })
            } else {
                Ok(CliOutput::ok(formatted, String::new()))
            }
        }
        Err((span, kind, code)) => {
            let mut stderr = String::new();
            render_fatal(&mut stderr, Path::new("<stdin>"), span, &kind, code);
            Err(stderr.trim_end().to_string())
        }
    }
}
