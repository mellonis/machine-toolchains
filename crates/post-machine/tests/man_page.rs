//! External validation of `pmt man` against a real `mandoc`: the page must
//! lint clean at mandoc's strictest level, and the formatted output must
//! carry the sections a reader looks for. Each check notes and returns
//! when no `mandoc` is on `PATH`; the content-level unit tests in
//! `src/man.rs` are what runs everywhere.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use mtc_post_machine::cli::execute;

/// mandoc has no version flag, so "available" means it could be spawned
/// at all; its exit status on an empty document is irrelevant here.
fn mandoc_available() -> bool {
    Command::new("mandoc")
        .args(["-T", "lint"])
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok()
}

fn rendered_page() -> String {
    execute(&["man".to_string()])
        .expect("pmt man should succeed")
        .stdout
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pmt_{name}_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Drop the backspace-overstrike sequences mandoc's ascii output uses for
/// bold and underline (`t\x08t`, `_\x08t`), leaving plain text.
fn plain(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch == '\u{8}' {
            out.pop();
        } else {
            out.push(ch);
        }
    }
    out
}

#[test]
fn man_page_lints_clean_under_mandoc() {
    if !mandoc_available() {
        eprintln!("skipping man_page_lints_clean_under_mandoc: no mandoc on PATH");
        return;
    }
    let dir = scratch("man_lint");
    let file = dir.join("pmt.1");
    fs::write(&file, rendered_page()).unwrap();
    let output = Command::new("mandoc")
        .args(["-T", "lint", "-W", "all"])
        .arg(&file)
        .output()
        .expect("failed to run `mandoc -T lint`");
    assert!(
        output.status.success() && output.stderr.is_empty() && output.stdout.is_empty(),
        "mandoc -T lint -W all reported:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn formatted_page_carries_every_section_a_reader_looks_for() {
    if !mandoc_available() {
        eprintln!(
            "skipping formatted_page_carries_every_section_a_reader_looks_for: no mandoc on PATH"
        );
        return;
    }
    let dir = scratch("man_ascii");
    let file = dir.join("pmt.1");
    fs::write(&file, rendered_page()).unwrap();
    let output = Command::new("mandoc")
        .args(["-T", "ascii"])
        .arg(&file)
        .output()
        .expect("failed to run `mandoc -T ascii`");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = plain(&String::from_utf8_lossy(&output.stdout));
    for needle in [
        "PMT(1)",
        "NAME",
        "SYNOPSIS",
        "SUBCOMMANDS",
        "pmt compile",
        "USAGE: pmt compile INPUT.pmc",
        "pmt tape-block",
        "EXIT STATUS",
        "SEE ALSO",
        "tmt(1)",
    ] {
        assert!(text.contains(needle), "{needle:?} missing from:\n{text}");
    }
}
