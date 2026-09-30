//! Compile warnings on the CLI: every one prints with its bracketed code,
//! `--allow CODE` suppresses one by that code on `tmt compile` and on
//! `tmt build`'s compile stage, and `tmt.json`'s `lint.allow` suppresses
//! one for `tmt build` in manifest mode (docs/tmt/cli.md (compile
//! warnings)). The last test holds the front end's emitters to the
//! published registry the renderer, the allow namespace and the docs
//! table all read.

use std::path::{Path, PathBuf};
use std::process::Command;

use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::WARNING_CODES;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A fresh scratch directory under `CARGO_TARGET_TMPDIR`, unique per call
/// (process id + an atomic counter), cleared first so a stale artifact
/// from an earlier run can never satisfy an assertion — the idiom
/// `link_warnings.rs` uses (this crate has no shared test-support module).
fn scratch(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The `tmt` binary under test, spawned as a real process so manifest
/// mode is driven through cwd exactly as a user would invoke it.
fn tmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tmt"))
}

/// Two partial rules with the same pattern: the later one (line 8,
/// column 5) can never fire.
const SHADOWED: &str = "\
alphabet ab { '_', 'K' }

machine {
  tape a: ab;
  tape b: ab;
  entry state s {
    ['K', *] -> stop;
    ['K', *] -> halt;
    [*, *] -> stop;
  }
}
";

/// A second all-wildcard rule (line 7, column 5).
const SECOND_CATCH_ALL: &str = "\
alphabet ab { '_', 'K' }

machine {
  tape a: ab;
  entry state s {
    [*] -> stop;
    [*] -> halt;
  }
}
";

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

fn warning_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.contains(": warning: "))
        .collect()
}

/// `tmt compile SRC -o OUT [extra…]`.
fn compile(
    src: &Path,
    dir: &Path,
    extra: &[&str],
) -> Result<mtc_turing_machine::cli::CliOutput, String> {
    let out = dir.join("out.tmo");
    let mut argv = vec![
        "compile",
        src.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ];
    argv.extend_from_slice(extra);
    execute(&args(&argv))
}

/// The issue's shape: a partial rule written twice gives exactly one
/// warning, located at the LATER rule and tagged with its code.
///
/// Mutation it catches: render the warning without its `[code]` suffix
/// and the line no longer ends with `[shadowed-rule]`.
#[test]
fn a_duplicate_partial_rule_prints_one_tagged_warning() {
    let dir = scratch("compile_warnings_shadowed");
    let src = write(&dir, "shadow.tmc", SHADOWED);
    let out = compile(&src, &dir, &[]).expect("a warning does not fail the compile");
    assert_eq!(out.code, 0);
    let lines = warning_lines(&out.stderr);
    assert_eq!(lines.len(), 1, "{}", out.stderr);
    assert_eq!(
        lines[0],
        format!(
            "{}:8:5: warning: this rule is unreachable — an earlier rule has the same \
             pattern [K, *] [shadowed-rule]",
            src.display()
        )
    );
}

/// Mutation it catches: the `[code]` suffix dropped from the compile
/// renderer, for this code as for the other.
#[test]
fn a_second_catch_all_prints_its_own_code() {
    let dir = scratch("compile_warnings_catch_all");
    let src = write(&dir, "catch.tmc", SECOND_CATCH_ALL);
    let out = compile(&src, &dir, &[]).expect("a warning does not fail the compile");
    let lines = warning_lines(&out.stderr);
    assert_eq!(lines.len(), 1, "{}", out.stderr);
    assert!(
        lines[0].starts_with(&format!("{}:7:5: warning: ", src.display()))
            && lines[0].ends_with(" [unreachable-rule]"),
        "{}",
        lines[0]
    );
}

/// `--allow` suppresses the warning, and `-Werror` counts only what
/// survives it; without the allow, `-Werror` still fails.
///
/// Mutation it catches: the allow list read but not consulted by the
/// renderer (or `-Werror` counting the unfiltered report) — the first or
/// second run then prints or fails.
#[test]
fn allow_suppresses_a_compile_warning_and_werror_counts_what_survives() {
    let dir = scratch("compile_warnings_allow");
    let src = write(&dir, "shadow.tmc", SHADOWED);

    let allowed = compile(&src, &dir, &["--allow", "shadowed-rule"]).expect("compiles");
    assert_eq!(allowed.code, 0);
    assert!(allowed.stderr.is_empty(), "{}", allowed.stderr);

    let strict_allowed = compile(&src, &dir, &["--allow", "shadowed-rule", "-Werror"])
        .expect("nothing survives the allow, so -Werror has nothing to promote");
    assert_eq!(strict_allowed.code, 0);
    assert!(
        strict_allowed.stderr.is_empty(),
        "{}",
        strict_allowed.stderr
    );

    let strict = compile(&src, &dir, &["-Werror"]).expect_err("-Werror promotes the warning");
    assert!(
        strict.contains("[shadowed-rule]")
            && strict.ends_with("-Werror: 1 warning(s) treated as errors"),
        "{strict}"
    );
}

/// An allow naming another code leaves this warning alone — the filter is
/// by code, not a blanket mute.
///
/// Mutation it catches: any non-empty allow list silencing every warning.
#[test]
fn allowing_another_code_keeps_the_warning() {
    let dir = scratch("compile_warnings_other_code");
    let src = write(&dir, "shadow.tmc", SHADOWED);
    let out = compile(&src, &dir, &["--allow", "unreachable-rule"]).expect("compiles");
    assert!(out.stderr.contains("[shadowed-rule]"), "{}", out.stderr);
}

/// Mutation it catches: skip validation and a typo'd `--allow` silently
/// suppresses nothing.
#[test]
fn an_unknown_allow_code_is_rejected_on_compile() {
    let dir = scratch("compile_warnings_typo");
    let src = write(&dir, "shadow.tmc", SHADOWED);
    let err = compile(&src, &dir, &["--allow", "no-such-code"]).expect_err("a typo is refused");
    assert_eq!(err, "unknown lint rule `no-such-code`");
}

/// Argv-mode `tmt build` honours `--allow` for its compile stage exactly
/// as `tmt compile` does, and prints the tag without it.
///
/// Mutation it catches: the argv-mode compile stage rendering with an
/// empty allow list.
#[test]
fn argv_build_allow_suppresses_a_compile_warning() {
    let dir = scratch("compile_warnings_argv_build");
    let src = write(&dir, "shadow.tmc", SHADOWED);
    let exe = dir.join("app.tmx");
    let plain = execute(&args(&[
        "build",
        src.to_str().unwrap(),
        "-o",
        exe.to_str().unwrap(),
    ]))
    .expect("builds");
    assert!(plain.stderr.contains("[shadowed-rule]"), "{}", plain.stderr);

    let allowed = execute(&args(&[
        "build",
        src.to_str().unwrap(),
        "--allow",
        "shadowed-rule",
        "-Werror",
        "-o",
        exe.to_str().unwrap(),
    ]))
    .expect("builds: the only warning is allowed");
    assert!(allowed.stderr.is_empty(), "{}", allowed.stderr);
}

fn manifest(dir: &Path, lint: &str) {
    std::fs::write(
        dir.join("tmt.json"),
        format!(
            r#"{{
                {lint}
                "project": {{ "targets": {{ "app": {{ "sources": ["shadow.tmc"] }} }} }}
            }}"#
        ),
    )
    .unwrap();
}

/// Manifest mode: `lint.allow` in `tmt.json` silences the compile
/// warning, and `-Werror` then has nothing to promote. The control run
/// (same project, no `lint` section) prints the tagged warning.
///
/// Mutation it catches: the manifest's allow list threaded to the link
/// stage only — the compile warning then prints and `-Werror` fails.
#[test]
fn manifest_mode_lint_allow_silences_a_compile_warning() {
    let control = scratch("compile_warnings_manifest_control");
    write(&control, "shadow.tmc", SHADOWED);
    manifest(&control, "");
    let out = tmt()
        .args(["build"])
        .current_dir(&control)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("[shadowed-rule]"), "{stderr}");

    let dir = scratch("compile_warnings_manifest_allow");
    write(&dir, "shadow.tmc", SHADOWED);
    manifest(&dir, r#""lint": { "allow": ["shadowed-rule"] },"#);
    let out = tmt()
        .args(["build", "-Werror"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("shadowed-rule"), "{stderr}");
    assert!(dir.join("app.tmx").is_file());
}

/// Every `.rs` file under `dir`, recursively, skipping the subtrees named
/// in `skip` (relative to `root`).
fn rust_files(root: &Path, dir: &Path, skip: &[&str], out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if skip.iter().any(|s| rel == *s) {
            continue;
        }
        if path.is_dir() {
            rust_files(root, &path, skip, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// The code literal of every bare `Diagnostic { … code: "…" … }`
/// construction in `text`: a `Diagnostic {` not preceded by an identifier
/// character (so `LinkDiagnostic {` and `ServiceDiagnostic {` are not
/// matched), then the first `code: "` within the next few lines.
fn emitted_codes(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut codes = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let mut from = 0;
        while let Some(pos) = line[from..].find("Diagnostic {") {
            let at = from + pos;
            from = at + 1;
            let preceded_by_ident = line[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            if preceded_by_ident {
                continue;
            }
            for next in lines.iter().skip(i).take(6) {
                if let Some(start) = next.find("code: \"") {
                    let rest = &next[start + "code: \"".len()..];
                    let end = rest.find('"').expect("closing quote");
                    codes.push(rest[..end].to_string());
                    break;
                }
            }
        }
    }
    codes
}

/// The front end's emitters and the published registry name the same
/// codes, both ways. Core's `Diagnostic` carries its code as a plain
/// `&'static str`, so emission cannot be forced through the registry by
/// the type system; this is a SOURCE SCAN instead, over every file of the
/// crate except the lint catalog (`lint/`, guarded by its own rule
/// tables) and the language server (`lsp/`, which only re-publishes).
/// It sees only a code written as a literal in a `Diagnostic { … }`
/// construction — a code computed at run time would slip past it, and
/// none exists today. The floor keeps the scan from passing vacuously.
///
/// Mutation it catches: emit a code the registry lacks (or retire an
/// emitter and leave its row behind) and the sets differ.
#[test]
fn every_front_end_warning_code_is_registered() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &root, &["lint", "lsp"], &mut files);
    let mut emitted: Vec<String> = files
        .iter()
        .flat_map(|f| emitted_codes(&std::fs::read_to_string(f).unwrap()))
        .collect();
    emitted.sort();
    emitted.dedup();
    assert!(emitted.len() >= 8, "the scan found only {emitted:?}");
    let mut registry: Vec<String> = WARNING_CODES.iter().map(|(c, _)| c.to_string()).collect();
    registry.sort();
    assert_eq!(
        emitted, registry,
        "front-end emitters vs compiler::WARNING_CODES"
    );
}
