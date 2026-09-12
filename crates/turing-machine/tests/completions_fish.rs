//! External validation of the rendered fish completion script against a
//! real `fish`: does it parse (`fish --no-execute`), and does sourcing
//! it then asking `complete -C '<line>'` — fish's headless completion
//! query, which needs no interactive session — produce the right
//! candidates? Each check notes and returns when no `fish` is on `PATH`;
//! the content-level unit tests in `src/completions/fish.rs` are what
//! runs everywhere.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn fish_available() -> bool {
    Command::new("fish")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn rendered_script() -> String {
    execute(&args(&["completions", "fish"]))
        .expect("tmt completions fish should succeed")
        .stdout
}

/// `CARGO_TARGET_TMPDIR` is shared across the workspace's crates, so the
/// directory carries the crate and the PID: a file test lists what it
/// finds, and must not find the sibling crate's fixtures.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("tmt_{name}_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The candidate WORDS `complete -C` produces for `line`, sorted and
/// deduplicated (fish prints `word<TAB>description` per line; the
/// description is dropped). `cwd` is where file completion looks.
fn candidates(cwd: &Path, line: &str) -> Vec<String> {
    let script = cwd.join("tmt.fish");
    fs::write(&script, rendered_script()).unwrap();
    let program = format!("source '{}'; complete -C {line:?}", script.display());
    let output = Command::new("fish")
        .arg("--no-config")
        .arg("-c")
        .arg(&program)
        .current_dir(cwd)
        .output()
        .expect("failed to run fish");
    assert!(
        output.status.success(),
        "fish failed on {line:?}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut out: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').next().unwrap().to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn fish_completions_parse_cleanly_under_no_execute() {
    if !fish_available() {
        eprintln!("skipping fish_completions_parse_cleanly_under_no_execute: no fish on PATH");
        return;
    }
    let dir = scratch("fish_syntax");
    let file = dir.join("tmt.fish");
    fs::write(&file, rendered_script()).unwrap();
    let output = Command::new("fish")
        .arg("--no-execute")
        .arg(&file)
        .output()
        .expect("failed to run `fish --no-execute`");
    assert!(
        output.status.success(),
        "fish --no-execute reported a syntax error:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn root_offers_every_subcommand() {
    if !fish_available() {
        return;
    }
    let dir = scratch("fish_root");
    let got = candidates(&dir, "tmt ");
    for name in [
        "compile",
        "link",
        "build",
        "tape-block",
        "ir",
        "lint",
        "fmt",
        "completions",
    ] {
        assert!(got.contains(&name.to_string()), "{name} missing: {got:?}");
    }
    assert_eq!(candidates(&dir, "tmt li"), vec!["link", "lint"]);
}

#[test]
fn a_group_offers_its_actions_then_the_action_flags() {
    if !fish_available() {
        return;
    }
    let dir = scratch("fish_group");
    assert_eq!(
        candidates(&dir, "tmt tape-block "),
        vec!["new", "set", "show"]
    );
    assert_eq!(
        candidates(&dir, "tmt tape-block show --"),
        vec!["--dense", "--help", "--separated"]
    );
    assert_eq!(
        candidates(&dir, "tmt tape-block show --dense --"),
        vec!["--help"]
    );
}

#[test]
fn compile_flags_include_families_and_honour_exclusive_groups() {
    if !fish_available() {
        return;
    }
    let dir = scratch("fish_compile_flags");
    let got = candidates(&dir, "tmt compile -");
    assert!(
        got.contains(&"-O0".to_string()) && got.contains(&"-O1".to_string()),
        "{got:?}"
    );
    assert!(got.contains(&"--fno-inline".to_string()), "{got:?}");
    assert!(got.contains(&"--emit-ir".to_string()), "{got:?}");
    assert!(
        !got.contains(&"--emit-ir=final".to_string()),
        "joined only after the head: {got:?}"
    );
    assert_eq!(
        candidates(&dir, "tmt compile --emit-ir=fin"),
        vec!["--emit-ir=final"]
    );
    let got = candidates(&dir, "tmt compile -O1 -O");
    assert!(
        !got.contains(&"-O0".to_string()),
        "-O0 is withheld once -O1 is on the line: {got:?}"
    );
}

#[test]
fn value_flags_complete_their_values() {
    if !fish_available() {
        return;
    }
    let dir = scratch("fish_values");
    assert_eq!(
        candidates(&dir, "tmt link --call-mech "),
        vec!["frames", "hybrid", "mono"]
    );
    assert_eq!(candidates(&dir, "tmt fmt --lang t"), vec!["tma", "tmc"]);
}

#[test]
fn positionals_offer_matching_files_and_directories() {
    if !fish_available() {
        return;
    }
    let dir = scratch("fish_files");
    fs::write(dir.join("a.tmc"), "").unwrap();
    fs::write(dir.join("b.tma"), "").unwrap();
    fs::write(dir.join("c.txt"), "").unwrap();
    fs::create_dir_all(dir.join("sub")).unwrap();
    assert_eq!(candidates(&dir, "tmt compile "), vec!["a.tmc", "sub/"]);
    assert_eq!(
        candidates(&dir, "tmt lint "),
        vec!["a.tmc", "b.tma", "sub/"]
    );
    assert_eq!(candidates(&dir, "tmt link -L "), vec!["sub/"]);
    // A value flag with several extensions offers all of them.
    fs::write(dir.join("d.tmx"), "").unwrap();
    assert_eq!(
        candidates(&dir, "tmt tape-block new --from "),
        vec!["a.tmc", "d.tmx", "sub/"]
    );
}
