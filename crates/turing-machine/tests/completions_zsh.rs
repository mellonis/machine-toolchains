//! External validation of the rendered zsh completion script: does it
//! parse as valid zsh, and does `compinit` accept it as a `#compdef`
//! file without erroring? This shells out to a real `zsh` binary — if
//! none is on `PATH`, each check notes that and returns rather than
//! failing an environment that simply doesn't have zsh installed (the
//! acceptance bar — "loads cleanly under `compinit`" — inherently needs
//! an actual zsh to check against).
//!
//! NOT covered here, and not practically coverable in an automated,
//! headless test: that pressing Tab in a real interactive session shows
//! the right candidates. `_arguments`/`_describe` refuse to run outside
//! the real completion-widget machinery ("can only be called from
//! completion function" when invoked directly), so exercising that needs
//! a pty feeding actual keystrokes through an interactive shell — done
//! manually during development, not automated here. The content-level
//! assertions in `src/completions/zsh.rs`'s own unit tests (every
//! subcommand, every flag, the right extension filters) are what stands
//! in for that.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn zsh_available() -> bool {
    Command::new("zsh")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn rendered_script() -> String {
    execute(&args(&["completions", "zsh"]))
        .expect("tmt completions zsh should succeed")
        .stdout
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn zsh_completions_output_looks_like_a_compdef_script() {
    let script = rendered_script();
    assert!(script.starts_with("#compdef tmt"), "{script}");
    assert!(script.contains("_tmt() {"), "{script}");
    assert!(script.trim_end().ends_with("_tmt \"$@\""), "{script}");
}

#[test]
fn zsh_completions_parse_cleanly_under_zsh_dash_n() {
    if !zsh_available() {
        eprintln!("skipping zsh_completions_parse_cleanly_under_zsh_dash_n: no zsh on PATH");
        return;
    }
    let dir = scratch("zsh_syntax");
    let file = dir.join("_tmt");
    fs::write(&file, rendered_script()).unwrap();

    let output = Command::new("zsh")
        .arg("-n") // parse only, don't execute
        .arg(&file)
        .output()
        .expect("failed to run `zsh -n`");
    assert!(
        output.status.success(),
        "zsh -n reported a syntax error:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn zsh_completions_load_under_compinit_without_errors() {
    if !zsh_available() {
        eprintln!("skipping zsh_completions_load_under_compinit_without_errors: no zsh on PATH");
        return;
    }
    let dir = scratch("zsh_compinit");
    let zfunc = dir.join("zfunc");
    fs::create_dir_all(&zfunc).unwrap();
    fs::write(zfunc.join("_tmt"), rendered_script()).unwrap();
    let dump = dir.join("zcompdump");

    let script = format!(
        "fpath=('{}' $fpath)\n\
         autoload -Uz compinit\n\
         compinit -u -d '{}'\n\
         whence -w _tmt\n",
        zfunc.display(),
        dump.display()
    );

    let output = Command::new("zsh")
        .arg("-f") // hermetic: no rc files
        .arg("-c")
        .arg(&script)
        .output()
        .expect("failed to run `zsh -f -c`");
    assert!(
        output.status.success(),
        "compinit rejected the generated script:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("_tmt: function") || stdout.contains("_tmt: autoload"),
        "compinit did not register `_tmt` as a completion function: {stdout}"
    );
}
