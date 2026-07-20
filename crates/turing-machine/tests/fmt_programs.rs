//! `tmt fmt` for `.tma`: the library property (idempotence + lossless through
//! the canonical grid) over the frames/tables/rept surface, and the CLI
//! (extension dispatch, `--check`, stdin `-` with `--lang`, the `.tmc`
//! not-yet-implemented route). The `.tma` formatter is core's
//! `format_asm_with`; these tests are the first to exercise its grid over the
//! TM-1-only constructs (`.frame`/`.map`/`.exits`, `.rept`, vector operands).

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::asm::format_asm_with;
use mtc_turing_machine::asm::{assemble, tm1_syntax};
use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("fmt-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    p
}

fn fmt_tma(src: &str) -> String {
    format_asm_with(src, tm1_syntax().caps).expect("formats")
}

/// Full 0.2 frames surface — a `.frame`/`.map`/`.exits` descriptor, `call.m`,
/// `trap`, `retx`, match table, vector operands — deliberately spaced
/// off-grid so formatting actually changes it.
const FRAMES: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.routine helper, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, 1]
  .row [*, *]
F0: .frame tapes=(1, 0)
  .map 0, rmap=(1->1, 3=>1)
  .exits done, other
.section code
.func main
   rd
   mtc T0
   trap #0
   call.m helper, F0
done: stp
other: hlt
.func helper
   wr [1, -]
   retx #1
";

/// Assert format is idempotent (`fmt∘fmt == fmt`) and lossless (the formatted
/// source assembles to byte-identical object code).
fn assert_idempotent_and_lossless(src: &str, label: &str) {
    let once = fmt_tma(src);
    let twice = fmt_tma(&once);
    assert_eq!(once, twice, "{label}: fmt is not idempotent");
    let a = assemble(src, false).unwrap_or_else(|e| panic!("{label}: source assembles: {e:?}"));
    let b = fmt_tma(src);
    let b = assemble(&b, false).unwrap_or_else(|e| panic!("{label}: formatted assembles: {e:?}"));
    assert_eq!(
        a.to_bytes(),
        b.to_bytes(),
        "{label}: fmt changed the object bytes"
    );
}

#[test]
fn frames_fixture_fmt_is_idempotent_and_lossless() {
    assert_idempotent_and_lossless(FRAMES, "frames");
    // The off-grid input really is reshaped (guards against a no-op formatter
    // trivially satisfying idempotence + lossless).
    assert_ne!(fmt_tma(FRAMES), FRAMES, "the off-grid input should reshape");
}

#[test]
fn brainfuck_fixture_fmt_is_idempotent_and_lossless() {
    // The flagship UTM: sections, an 8-row match table, `.rept` macros with
    // `{v}` substitution, dispatch tables. Read-only — never written back
    // (it is golden-backed).
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/brainfuck-utm.tma");
    let src = fs::read_to_string(&path).expect("read brainfuck-utm.tma");
    assert_idempotent_and_lossless(&src, "brainfuck");
}

#[test]
fn fmt_check_on_canonical_tma_is_silent_and_exits_zero() {
    let dir = scratch("check-clean");
    let canonical = fmt_tma(FRAMES);
    let f = write(&dir, "a.tma", &canonical);
    let out = execute(&args(&["fmt", "--check", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "stdout: {} stderr: {}", out.stdout, out.stderr);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
}

#[test]
fn fmt_check_on_offgrid_tma_lists_it_and_exits_one_without_writing() {
    let dir = scratch("check-dirty");
    let f = write(&dir, "a.tma", FRAMES);
    let out = execute(&args(&["fmt", "--check", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("a.tma"), "{}", out.stdout);
    // --check must not have rewritten the file.
    assert_eq!(fs::read_to_string(&f).unwrap(), FRAMES);
}

#[test]
fn fmt_write_reformats_the_tma_in_place_and_exits_zero() {
    let dir = scratch("write");
    let f = write(&dir, "a.tma", FRAMES);
    let out = execute(&args(&["fmt", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let written = fs::read_to_string(&f).unwrap();
    assert_eq!(written, fmt_tma(FRAMES), "file left in canonical form");
    // Idempotent: a second run is a no-op.
    let out2 = execute(&args(&["fmt", f.to_str().unwrap()])).unwrap();
    assert_eq!(out2.code, 0);
    assert_eq!(fs::read_to_string(&f).unwrap(), written);
}

#[test]
fn fmt_lang_alongside_a_path_is_an_error() {
    // `--lang` selects stdin's language; alongside a PATH it is a misuse (a
    // file's language comes from its extension). The stdin happy path itself
    // reads the process's real stdin, so it is not driven through the
    // in-process `execute`.
    let dir = scratch("lang-misuse");
    let f = write(&dir, "a.tma", FRAMES);
    let err = execute(&args(&["fmt", "--lang", "tma", f.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("--lang applies to stdin"), "{err}");
}

#[test]
fn fmt_rejects_an_unknown_lang() {
    let err = execute(&args(&["fmt", "-", "--lang", "cobol"])).unwrap_err();
    assert!(err.contains("takes tmc or tma"), "{err}");
}

#[test]
fn fmt_tmc_is_reported_as_not_yet_implemented() {
    let dir = scratch("tmc");
    let f = write(&dir, "m.tmc", "machine { }\n");
    let out = execute(&args(&["fmt", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains("not yet implemented"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn fmt_directory_walks_both_extensions() {
    // A .tma (reformatted) and a .tmc (not-yet-implemented) under one dir:
    // the .tma is handled, the .tmc reported; exit 1 (the .tmc error).
    let dir = scratch("walk");
    write(&dir, "a.tma", FRAMES);
    write(&dir, "m.tmc", "machine { }\n");
    let out = execute(&args(&["fmt", "--check", dir.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("a.tma"), "stdout: {}", out.stdout);
    assert!(
        out.stderr.contains("not yet implemented"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn fmt_help_prints_usage() {
    let out = execute(&args(&["fmt", "--help"])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("USAGE: tmt fmt"), "{}", out.stdout);
}
