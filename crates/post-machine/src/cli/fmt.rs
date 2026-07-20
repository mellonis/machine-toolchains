//! `pmt fmt` (`docs/cli.md (pmt fmt)`): thin renderer over the fmt
//! library. The library
//! [`format`](crate::fmt::format) stays print-free (`crate::fmt`'s own
//! doc comment) — this module is the ONLY place that prints or touches
//! the filesystem for the fmt surface, same discipline as
//! [`super::lint`]. Batch model (`PATH...`) is IDENTICAL to `pmt lint`'s,
//! so it shares [`super::lint::collect_sources`] rather than duplicating
//! the walk; per-file extension routing (`.pmc` through the pmc printer,
//! `.pma` through core's canonical-grid printer) mirrors `pmt lint`'s
//! two-route dispatch, and the per-file fatal line reuses
//! [`super::lint::render_fatal`].

use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use mtc_core::asm::format_asm;
use mtc_core::diagnostics::Span;

use crate::fmt::format as format_source;

use super::lint::{collect_sources, render_fatal};
use super::{Args, CliOutput};

const FMT_USAGE: &str = "\
USAGE: pmt fmt PATH... [--exclude PATH]... [--check]
       pmt fmt - [--check] [--lang pmc|pma]

PATH is a .pmc or .pma file, or a directory; directories are walked
recursively for *.pmc and *.pma (sorted order, symlinks not followed,
dot-entries skipped). `-` reads one source from stdin and writes the
result to stdout; it cannot be combined with PATH arguments.

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs)
  --check         do not write; with PATH..., list files that would be
                  reformatted and exit 1 if any would change; with -,
                  exit 1 if stdin would change (CI mode)
  --lang LANG     stdin's language: pmc (default) or pma; applies to
                  stdin (-) only — an error alongside PATH arguments,
                  whose language always comes from the file extension
";

/// stdin's language for `pmt fmt -`, defaulted from `--lang`
/// (docs/cli.md (pmt fmt)). PATH batches never need this: each file's
/// extension already says which formatter applies.
#[derive(Clone, Copy)]
enum Lang {
    Pmc,
    Pma,
}

fn parse_lang(value: Option<&str>) -> Result<Lang, String> {
    match value {
        None | Some("pmc") => Ok(Lang::Pmc),
        Some("pma") => Ok(Lang::Pma),
        Some(_) => Err(format!("`--lang` takes pmc or pma\n\n{FMT_USAGE}")),
    }
}

/// Format one source through the language-appropriate formatter,
/// collapsing the two distinct error types (`CompileError` for `.pmc`,
/// `AsmError` for `.pma`) into one shape both call sites below render
/// identically through [`render_fatal`].
fn format_by_lang(source: &str, lang: Lang) -> Result<String, (Span, String, &'static str)> {
    match lang {
        Lang::Pmc => format_source(source).map_err(|e| (e.span, e.kind.to_string(), e.kind.code())),
        Lang::Pma => format_asm(source).map_err(|e| (e.span, e.kind.to_string(), e.kind.code())),
    }
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
        let path = Path::new(p);
        let found = collect_sources(path, &excludes, &mut files)?;
        if found == 0 {
            return Err(format!("{p}: no .pmc or .pma files found"));
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    // Writing a changed file (default mode) is still success (exit 0) —
    // only `--check` turns "would change" into a nonzero exit; a parse
    // fatal is nonzero regardless of `--check` (spec "Exit codes").
    let mut would_change = false;
    let mut had_error = false;
    for file in &files {
        let source =
            fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let lang = match file.extension().and_then(|x| x.to_str()) {
            Some("pmc") => Lang::Pmc,
            Some("pma") => Lang::Pma,
            _ => {
                // Only reachable for an explicitly listed file — the
                // directory walk (`collect_sources`) only ever collects
                // `.pmc`/`.pma` extensions (same shape as lint's route).
                had_error = true;
                let _ = writeln!(
                    stderr,
                    "{}: error: unknown source extension (expected .pmc or .pma)",
                    file.display()
                );
                continue;
            }
        };
        match format_by_lang(&source, lang) {
            Ok(formatted) => {
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
            Err((span, kind, code)) => {
                // Per-file fatal: report, keep going (batch model, same as lint).
                had_error = true;
                render_fatal(&mut stderr, file, span, &kind, code);
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
/// pmc), format it, write to stdout (or, under `--check`, write nothing
/// and only signal via the exit code). A lex/parse error is a whole-tool
/// error (single input, no batch to continue) — mirrors
/// `cli/build.rs::compile`'s single-file fatal.
fn fmt_stdin(check: bool, lang: Lang) -> Result<CliOutput, String> {
    let mut source = String::new();
    std::io::stdin()
        .read_to_string(&mut source)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    match format_by_lang(&source, lang) {
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
