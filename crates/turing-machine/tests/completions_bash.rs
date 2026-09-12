//! External validation of the rendered bash completion script against a
//! real `bash`: does it parse (`bash -n`), does sourcing it register the
//! completion (`complete -p tmt`), and — unlike zsh, where `_arguments`
//! refuses to run outside the widget — does calling the completion
//! function headlessly produce the right candidates? bash lets a test
//! set `COMP_LINE`/`COMP_POINT`, call `_tmt`, and read `COMPREPLY`, so
//! the candidate assertions zsh had to leave to a by-hand pty session
//! are automated here. `/bin/bash` on macOS is 3.2, the oldest bash
//! anyone plausibly completes in, so passing there is the portability
//! bar. Each check notes and returns when no `bash` is on `PATH`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn bash_available() -> bool {
    Command::new("bash")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn rendered_script() -> String {
    execute(&args(&["completions", "bash"]))
        .expect("tmt completions bash should succeed")
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

/// Candidates `_tmt` produces for `line` (the command line up to the
/// cursor), sorted and deduplicated. `cwd` is where file completion
/// looks; the script itself is written there as `tmt.bash`, so a file
/// test sees it among the "any file" candidates.
fn candidates(cwd: &Path, line: &str) -> Vec<String> {
    let script = cwd.join("tmt.bash");
    fs::write(&script, rendered_script()).unwrap();
    let program = format!(
        "source '{}'\n\
         COMP_LINE={line:?}\n\
         COMP_POINT=${{#COMP_LINE}}\n\
         COMP_WORDBREAKS=$' \\t\\n\"\\'><=;|&(:'\n\
         _tmt\n\
         printf '%s\\n' \"${{COMPREPLY[@]}}\"\n",
        script.display()
    );
    let output = Command::new("bash")
        .arg("--noprofile")
        .arg("--norc")
        .arg("-c")
        .arg(&program)
        .current_dir(cwd)
        .output()
        .expect("failed to run bash");
    assert!(
        output.status.success(),
        "bash failed on {line:?}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut out: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn bash_completions_parse_cleanly_under_bash_dash_n() {
    if !bash_available() {
        eprintln!("skipping bash_completions_parse_cleanly_under_bash_dash_n: no bash on PATH");
        return;
    }
    let dir = scratch("bash_syntax");
    let file = dir.join("tmt.bash");
    fs::write(&file, rendered_script()).unwrap();
    let output = Command::new("bash")
        .arg("-n")
        .arg(&file)
        .output()
        .expect("failed to run `bash -n`");
    assert!(
        output.status.success(),
        "bash -n reported a syntax error:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sourcing_registers_the_completion_function() {
    if !bash_available() {
        eprintln!("skipping sourcing_registers_the_completion_function: no bash on PATH");
        return;
    }
    let dir = scratch("bash_register");
    let file = dir.join("tmt.bash");
    fs::write(&file, rendered_script()).unwrap();
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-c"])
        .arg(format!("source '{}' && complete -p tmt", file.display()))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("-F _tmt") && stdout.contains("-o filenames"),
        "complete -p tmt: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn root_offers_every_subcommand() {
    if !bash_available() {
        return;
    }
    let dir = scratch("bash_root");
    let got = candidates(&dir, "tmt ");
    for name in [
        "compile",
        "link",
        "build",
        "tape-block",
        "ir",
        "lint",
        "fmt",
        "lsp",
        "dap",
        "completions",
    ] {
        assert!(got.contains(&name.to_string()), "{name} missing: {got:?}");
    }
    assert_eq!(candidates(&dir, "tmt li"), vec!["link", "lint"]);
    assert_eq!(candidates(&dir, "tmt --"), vec!["--help", "--version"]);
}

#[test]
fn a_group_offers_its_actions_then_the_action_flags() {
    if !bash_available() {
        return;
    }
    let dir = scratch("bash_group");
    assert_eq!(
        candidates(&dir, "tmt tape-block "),
        vec!["new", "set", "show"]
    );
    let got = candidates(&dir, "tmt tape-block show --");
    assert_eq!(got, vec!["--dense", "--help", "--separated"]);
    // `--dense` on the line removes its group-mate.
    let got = candidates(&dir, "tmt tape-block show --dense --");
    assert_eq!(got, vec!["--help"]);
}

#[test]
fn compile_flags_include_families_and_honour_exclusive_groups() {
    if !bash_available() {
        return;
    }
    let dir = scratch("bash_compile_flags");
    let got = candidates(&dir, "tmt compile -");
    assert!(
        got.contains(&"-O0".to_string()) && got.contains(&"-O1".to_string()),
        "{got:?}"
    );
    assert!(got.contains(&"--fno-inline".to_string()), "{got:?}");
    assert!(got.contains(&"--emit-ir".to_string()), "{got:?}");
    assert!(got.contains(&"--emit-ir=final".to_string()), "{got:?}");
    assert!(!got.contains(&"--fno-".to_string()), "{got:?}");
    let got = candidates(&dir, "tmt compile -O1 -O");
    assert_eq!(
        got,
        Vec::<String>::new(),
        "-O0 is withheld once -O1 is on the line"
    );
}

#[test]
fn equals_and_colon_joined_values_are_trimmed_to_bash_word_breaks() {
    if !bash_available() {
        return;
    }
    let dir = scratch("bash_wordbreaks");
    // `=` is a word break, so bash replaces only the text after it.
    assert_eq!(candidates(&dir, "tmt compile --emit-ir=fin"), vec!["final"]);
    // `:` is one too: after `after:` only the pass name is replaced.
    let got = candidates(&dir, "tmt compile --emit-ir=after:in");
    assert_eq!(got, vec!["inline"]);
    assert_eq!(candidates(&dir, "tmt link --call-mech=fr"), vec!["frames"]);
}

#[test]
fn value_flags_complete_their_values() {
    if !bash_available() {
        return;
    }
    let dir = scratch("bash_values");
    assert_eq!(
        candidates(&dir, "tmt link --call-mech "),
        vec!["frames", "hybrid", "mono"]
    );
    assert_eq!(candidates(&dir, "tmt fmt --lang t"), vec!["tma", "tmc"]);
    assert_eq!(candidates(&dir, "tmt link --entry "), Vec::<String>::new());
}

#[test]
fn positionals_offer_matching_files_and_directories() {
    if !bash_available() {
        return;
    }
    let dir = scratch("bash_files");
    fs::write(dir.join("a.tmc"), "").unwrap();
    fs::write(dir.join("b.tma"), "").unwrap();
    fs::write(dir.join("c.txt"), "").unwrap();
    fs::write(dir.join("d.tmx"), "").unwrap();
    fs::create_dir_all(dir.join("sub")).unwrap();
    assert_eq!(candidates(&dir, "tmt compile "), vec!["a.tmc", "sub"]);
    assert_eq!(candidates(&dir, "tmt lint "), vec!["a.tmc", "b.tma", "sub"]);
    assert_eq!(candidates(&dir, "tmt run "), vec!["d.tmx", "sub"]);
    // `-o` takes any path.
    assert_eq!(
        candidates(&dir, "tmt compile -o "),
        vec!["a.tmc", "b.tma", "c.txt", "d.tmx", "sub", "tmt.bash"]
    );
    // `-L` takes directories only.
    assert_eq!(candidates(&dir, "tmt link -L "), vec!["sub"]);
}
