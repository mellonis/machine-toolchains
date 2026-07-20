//! `tmt lint` end-to-end, in-process: extension dispatch, exit codes, the
//! `--allow`/`--warn` flags, `tmt.json` union, and the batch model.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("lint-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    p
}

/// A `.tmc` with a leftover `debugger` marker — one lint finding.
const DIRTY: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> debugger goto s;
    ['_'] -> stop;
  }
}
";

const CLEAN: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { [*] -> stop; }
}
";

#[test]
fn a_dirty_tmc_file_reports_and_exits_one() {
    let dir = scratch("dirty");
    let f = write(&dir, "m.tmc", DIRTY);
    let out = execute(&args(&["lint", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("lint: leftover 'debugger' marker"), "{}", out.stdout);
}

#[test]
fn a_clean_tmc_file_is_silent_and_exits_zero() {
    let dir = scratch("clean");
    let f = write(&dir, "m.tmc", CLEAN);
    let out = execute(&args(&["lint", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
}

#[test]
fn allow_flag_suppresses_the_finding() {
    let dir = scratch("allow");
    let f = write(&dir, "m.tmc", DIRTY);
    let out = execute(&args(&[
        "lint",
        f.to_str().unwrap(),
        "--allow",
        "leftover-debugger",
    ]))
    .unwrap();
    assert_eq!(out.code, 0, "stdout: {} stderr: {}", out.stdout, out.stderr);
}

#[test]
fn an_unknown_allow_code_aborts_the_whole_run() {
    let dir = scratch("badallow");
    let f = write(&dir, "m.tmc", CLEAN);
    let err = execute(&args(&["lint", f.to_str().unwrap(), "--allow", "no-such"])).unwrap_err();
    assert!(err.contains("no-such"), "{err}");
}

#[test]
fn a_tma_path_is_recognized_but_not_yet_implemented() {
    let dir = scratch("tma");
    let f = write(&dir, "a.tma", "; placeholder\n");
    let out = execute(&args(&["lint", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains("not yet implemented"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn a_directory_walks_both_extensions_and_keeps_going() {
    // A dirty .tmc (finding) and a .tma (not-yet-implemented) under one dir —
    // both are visited; exit 1.
    let dir = scratch("walk");
    write(&dir, "m.tmc", DIRTY);
    write(&dir, "a.tma", "; placeholder\n");
    let out = execute(&args(&["lint", dir.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("leftover 'debugger'"), "{}", out.stdout);
    assert!(out.stderr.contains("not yet implemented"), "{}", out.stderr);
}

#[test]
fn a_parse_fatal_is_a_per_file_error_and_the_batch_continues() {
    let dir = scratch("fatal");
    write(&dir, "a_broken.tmc", "machine {");
    write(&dir, "b_dirty.tmc", DIRTY);
    let out = execute(&args(&[
        "lint",
        dir.join("a_broken.tmc").to_str().unwrap(),
        dir.join("b_dirty.tmc").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("error:"), "stderr: {}", out.stderr);
    assert!(out.stdout.contains("leftover 'debugger'"), "stdout: {}", out.stdout);
}

#[test]
fn tmt_json_allow_unions_with_the_flag() {
    let dir = scratch("config");
    let f = write(&dir, "m.tmc", DIRTY);
    write(&dir, "tmt.json", r#"{"lint":{"allow":["leftover-debugger"]}}"#);
    // No --allow flag: the config alone suppresses.
    let out = execute(&args(&["lint", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "stdout: {} stderr: {}", out.stdout, out.stderr);
    // --no-config ignores it: the finding returns.
    let out = execute(&args(&["lint", f.to_str().unwrap(), "--no-config"])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("leftover 'debugger'"), "{}", out.stdout);
}

#[test]
fn a_bad_tmt_json_is_a_per_file_error() {
    let dir = scratch("badconfig");
    let f = write(&dir, "m.tmc", CLEAN);
    write(&dir, "tmt.json", r#"{"lint":{"allow":["no-such-rule"]}}"#);
    let out = execute(&args(&["lint", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("no-such-rule"), "stderr: {}", out.stderr);
}

#[test]
fn warn_enables_the_opt_in_state_may_trap_rule() {
    // A partial state ('_' unmatched, no catch-all): silent by default, flagged
    // under --warn.
    let partial = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { ['1'] -> stop; }
}
";
    let dir = scratch("warn");
    let f = write(&dir, "m.tmc", partial);

    let quiet = execute(&args(&["lint", f.to_str().unwrap()])).unwrap();
    assert_eq!(quiet.code, 0, "stdout: {} stderr: {}", quiet.stdout, quiet.stderr);

    let warned = execute(&args(&["lint", f.to_str().unwrap(), "--warn", "state-may-trap"])).unwrap();
    assert_eq!(warned.code, 1);
    assert!(warned.stdout.contains("may trap"), "{}", warned.stdout);
}

#[test]
fn no_positionals_is_an_error() {
    let err = execute(&args(&["lint"])).unwrap_err();
    assert!(err.contains("at least one PATH"), "{err}");
}

#[test]
fn help_prints_usage() {
    let out = execute(&args(&["lint", "--help"])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("USAGE: tmt lint"), "{}", out.stdout);
}
