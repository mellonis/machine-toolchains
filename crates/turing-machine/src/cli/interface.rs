//! `tmt interface`: the fifteenth subcommand, a thin renderer over
//! `crate::header` (docs/tmt/cli.md (interface)). Mirrors `dis`'s shape in
//! `cli/inspect.rs` — sniff the input, dispatch on the container kind,
//! print the result. `--check` (docs/tmt/cli.md (tmt interface)) is the
//! same render compared against a committed file instead of written
//! anywhere, so a stale header — one a project committed and never
//! regenerated — fails a CI/pre-commit gate instead of silently drifting
//! from the source or object it was taken from.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::{ARCH_TM1, ContainerKind, sniff};
use mtc_core::vm::LoadError;

use super::{Args, CliOutput};

pub(super) const INTERFACE_USAGE: &str = "\
USAGE: tmt interface INPUT [-o OUT.tmh | --check FILE] [FLAGS]

INPUT is told apart by its container magic, never by its extension: a
.tmc source or a compiled .tmo object. A .tmh extension (case-insensitive)
additionally selects declarations-only reading of a text INPUT: a
`machine` block or a routine body is rejected, and a bodiless routine
signature is required instead. Prints the unit's exported
declarations — alphabets and routine signatures with their EFFECTIVE
write contracts either way; neither arm ever prints `volatile` (it
leaves no trace past source and is never checked at a call site). From
source the header is complete: it also carries exported graph bodies in
full and every `?` doc line. From an object it carries signatures and
alphabets only — no graph body, no map, no doc line, since none of those
exist on the wire. Without -o the header goes to stdout.

FLAGS:
  --check FILE       compare FILE, byte for byte, against this render;
                     exit 1 naming the first differing line on a
                     mismatch, and write nothing (exclusive with -o)
  -v                 with --check, also list the differing lines

FLAGS (text INPUT only — rejected on a .tmo object, which carries no
external references of its own left to resolve):
  --extern FILE      read FILE's declarations (.tmh strict, .tmc lenient;
                     repeatable, in command-line order)
  --nostdlib         do not read the embedded standard library's declarations
";

pub(super) fn interface(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(INTERFACE_USAGE.into(), String::new()));
    }
    let explicit_out = args.value("-o")?;
    let check = args.value("--check")?;
    if explicit_out.is_some() && check.is_some() {
        return Err(format!(
            "interface: -o and --check are mutually exclusive\n\n{INTERFACE_USAGE}"
        ));
    }
    let verbose = args.flag("-v");
    // Read BEFORE the input, exactly as `tmt compile` does (`cli/build.rs`
    // (compile)): a broken `--extern` file's error then names ITS OWN
    // path, never the primary input's.
    let extern_paths = args.values("--extern")?;
    let nostdlib = args.flag("--nostdlib");
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "interface takes exactly one input\n\n{INTERFACE_USAGE}"
        ));
    };
    let path = Path::new(input);
    let text = render_header(path, &extern_paths, nostdlib)?;

    if let Some(check) = check {
        return check_header(Path::new(&check), path, &text, verbose);
    }
    if let Some(out) = explicit_out {
        fs::write(&out, &text).map_err(|e| format!("cannot write {out}: {e}"))?;
        Ok(CliOutput::ok(String::new(), String::new()))
    } else {
        Ok(CliOutput::ok(text, String::new()))
    }
}

/// Renders `input`'s header exactly as `tmt interface INPUT` prints it —
/// the one text both `-o` and `--check` compare against, so neither can
/// drift from what the other sees (docs/tmt/cli.md (tmt interface)).
fn render_header(path: &Path, extern_paths: &[String], nostdlib: bool) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    match sniff(&bytes) {
        Some(ContainerKind::Object) => {
            // An object's own header is read straight off its interface
            // section — nothing left to resolve against another unit's
            // declarations, so `--extern`/`--nostdlib` are a usage error
            // here rather than silently ignored (docs/tmt/cli.md
            // (interface)).
            if !extern_paths.is_empty() || nostdlib {
                return Err(format!(
                    "{}: --extern/--nostdlib apply only to a text INPUT — a .tmo object's \
                     header is read straight off its own interface section, with no \
                     declarations left to resolve",
                    path.display()
                ));
            }
            let obj = ObjectFile::from_bytes(&bytes).map_err(|e| e.to_string())?;
            if obj.arch != ARCH_TM1 {
                return Err(LoadError::UnknownArch(obj.arch).to_string());
            }
            crate::header::from_object(&obj).map_err(|e| format!("{}: {e}", path.display()))
        }
        Some(_) => Err(format!(
            "{}: not a .tmc source or a .tmo object",
            path.display()
        )),
        None => {
            let source = String::from_utf8(bytes).map_err(|_| {
                format!("{}: not a .tmo object and not UTF-8 source", path.display())
            })?;
            // A `.tmh` extension (case-insensitive, matching the repo's
            // standing extension-routing rule) selects declarations-only
            // reading (docs/tmt/language.md (headers)) — text has no
            // container magic to sniff a header from a full source by, so
            // this is the one place the extension itself IS the signal.
            let is_header = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("tmh"));
            // The same declarations table `tmt compile` builds from
            // `--extern`/`--nostdlib` (`cli/build.rs::read_externals`): a
            // library whose exported routine, graph or map reaches
            // another unit's alphabet needs it to header at all
            // (docs/tmt/cli.md (interface)).
            let externals = super::build::read_externals(extern_paths, nostdlib)?;
            let text = if is_header {
                crate::header::from_declarations(&source, &externals)
            } else {
                crate::header::from_source(&source, &externals)
            };
            text.map_err(|e| {
                format!(
                    "{}:{}:{}: error: {} [{}]",
                    path.display(),
                    e.span.start.line,
                    e.span.start.col,
                    e.kind,
                    e.kind.code()
                )
            })
        }
    }
}

/// `--check`: compares `text` (this invocation's render of `input`)
/// against `check`'s bytes on disk. `check` failing to read is a hard
/// error — never reported as "differs", so a CI gate cannot mistake a
/// missing header for a stale one. A byte match is exit 0 with no
/// output; a mismatch is exit 1, reporting the first differing line
/// (and, under `-v`, a `-`/`+` listing) without writing anything.
fn check_header(
    check: &Path,
    input: &Path,
    text: &str,
    verbose: bool,
) -> Result<CliOutput, String> {
    let actual_bytes =
        fs::read(check).map_err(|e| format!("{}: cannot read: {e}", check.display()))?;
    // A leading "generated, do not edit" notice is not part of the header
    // (docs/tmt/cli.md (interface)): skip it, remembering how many lines
    // it took so a reported line number stays the line in FILE.
    let (skipped_bytes, skipped_lines) = leading_notice(&actual_bytes);
    let actual_bytes = &actual_bytes[skipped_bytes..];
    if actual_bytes == text.as_bytes() {
        return Ok(CliOutput::ok(String::new(), String::new()));
    }
    // The committed file need not be UTF-8 to fail the comparison above,
    // but line-by-line reporting needs text; a non-UTF-8 FILE is already
    // known to differ, and lossy decoding only affects how the mismatch
    // is DESCRIBED, never whether one was found.
    let actual = String::from_utf8_lossy(actual_bytes);
    let line = skipped_lines + first_diff_line(text, &actual);
    let mut stderr = format!(
        "{}: differs from the header of {} (first difference at line {line})\n",
        check.display(),
        input.display()
    );
    if verbose {
        stderr.push_str(&diff_lines(text, &actual));
    }
    Ok(CliOutput {
        stdout: String::new(),
        stderr,
        code: 1,
    })
}

/// The length in bytes, and in lines, of the LEADING run of `//` line
/// comments and blank lines at the top of `bytes` — the notice a project
/// may stamp on a generated header. Only the very top counts: the printed
/// header itself never contains a comment, so a comment anywhere later is
/// an ordinary difference.
fn leading_notice(bytes: &[u8]) -> (usize, usize) {
    let mut offset = 0;
    let mut lines = 0;
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        let body = line.trim_ascii();
        if !(body.is_empty() || body.starts_with(b"//")) {
            break;
        }
        offset += line.len();
        lines += 1;
    }
    (offset, lines)
}

/// Splits `s` into lines that each KEEP their own terminator (`\n` or
/// `\r\n`; the final line carries none when `s` has no trailing newline).
/// `str::lines` would strip that terminator, and with it the very
/// difference a stale-header comparison most needs to catch: it folds
/// `"a\r\n"` and `"a\n"` into the identical line `"a"`, and folds a final
/// unterminated `"a"` into that same line again — hiding a CRLF-vs-LF
/// header and a missing trailing newline alike. Comparing terminators
/// makes the trailing-newline rule fall out of the ordinary per-line
/// comparison below rather than needing a special case: a header that
/// agrees on every line's CONTENT but is missing its own final `\n`
/// differs only at that unterminated last line, which the comparison
/// reaches on its own.
fn lines_with_terminators(s: &str) -> Vec<&str> {
    s.split_inclusive('\n').collect()
}

/// The 1-based line number of the first difference between `expected` and
/// `actual`. Only called once the caller has already confirmed the two
/// differ as bytes, so the comparison finds one (`split_inclusive` lines
/// concatenate back to their text) except for the lossy-decoding case
/// noted at the end.
fn first_diff_line(expected: &str, actual: &str) -> usize {
    let exp_lines = lines_with_terminators(expected);
    let act_lines = lines_with_terminators(actual);
    let max = exp_lines.len().max(act_lines.len());
    for i in 0..max {
        if exp_lines.get(i) != act_lines.get(i) {
            return i + 1;
        }
    }
    // Every line agreeing means byte-identical text, so this is reached
    // only when FILE's non-UTF-8 bytes were lossily decoded into text that
    // happens to equal the render; report the line past the last one.
    max + 1
}

/// A minimal unified-diff-style listing (`-`/`+` lines, no external tool)
/// of every line where `expected` and `actual` disagree — a naive
/// positional diff, not an LCS-minimal one: an inserted or deleted line
/// shifts everything after it, and each shifted line reports as its own
/// pair rather than being recognized as unchanged-but-moved. Each
/// printed line drops its own terminator (`lines_with_terminators` kept
/// it only so the comparison could see it); a CRLF-vs-LF or
/// missing-trailing-newline difference therefore prints two visually
/// identical `-`/`+` lines, which is the honest rendering of "the text is
/// the same, the line ending is not".
fn diff_lines(expected: &str, actual: &str) -> String {
    let exp_lines = lines_with_terminators(expected);
    let act_lines = lines_with_terminators(actual);
    let max = exp_lines.len().max(act_lines.len());
    let mut out = String::new();
    for i in 0..max {
        let e = exp_lines.get(i).copied();
        let a = act_lines.get(i).copied();
        if e != a {
            if let Some(e) = e {
                let _ = writeln!(out, "-{}", e.trim_end_matches(['\r', '\n']));
            }
            if let Some(a) = a {
                let _ = writeln!(out, "+{}", a.trim_end_matches(['\r', '\n']));
            }
        }
    }
    out
}
