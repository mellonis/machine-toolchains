//! `pmt fmt` (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`,
//! "CLI: pmt fmt" — the durable `docs/cli.md` section lands with the
//! docs task): thin renderer over the fmt library. The library
//! [`format`](crate::fmt::format) stays print-free (`crate::fmt`'s own
//! doc comment) — this module is the ONLY place that prints or touches
//! the filesystem for the fmt surface, same discipline as
//! [`super::lint`]. Batch model (`PATH...`) is IDENTICAL to `pmt lint`'s,
//! so it shares [`super::lint::collect_pmc`] rather than duplicating the
//! walk.

use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::fmt::format as format_source;

use super::lint::collect_sources;
use super::{Args, CliOutput};

const FMT_USAGE: &str = "\
USAGE: pmt fmt PATH... [--exclude PATH]... [--check]
       pmt fmt - [--check]

PATH is a .pmc file or a directory; directories are walked recursively
for *.pmc (sorted order, symlinks not followed, dot-entries skipped).
`-` reads one .pmc from stdin and writes the result to stdout; it
cannot be combined with PATH arguments.

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs)
  --check         do not write; with PATH..., list files that would be
                  reformatted and exit 1 if any would change; with -,
                  exit 1 if stdin would change (CI mode)
";

pub(super) fn fmt(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(FMT_USAGE.into(), String::new()));
    }
    let check = args.flag("--check");
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
        return fmt_stdin(check);
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
            return Err(format!("{p}: no .pmc files found"));
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
        match format_source(&source) {
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
            Err(e) => {
                // Per-file fatal: report, keep going (batch model, same as lint).
                had_error = true;
                let _ = writeln!(
                    stderr,
                    "{}:{}:{}: error: {} [{}]",
                    file.display(),
                    e.span.start.line,
                    e.span.start.col,
                    e.kind,
                    e.kind.code()
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

/// `-`: read one `.pmc` from stdin, format it, write to stdout (or, under
/// `--check`, write nothing and only signal via the exit code). A
/// lex/parse error is a whole-tool error (single input, no batch to
/// continue) — mirrors `cli/build.rs::compile`'s single-file fatal.
fn fmt_stdin(check: bool) -> Result<CliOutput, String> {
    let mut source = String::new();
    std::io::stdin()
        .read_to_string(&mut source)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    match format_source(&source) {
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
        Err(e) => Err(format!(
            "<stdin>:{}:{}: error: {} [{}]",
            e.span.start.line,
            e.span.start.col,
            e.kind,
            e.kind.code()
        )),
    }
}
