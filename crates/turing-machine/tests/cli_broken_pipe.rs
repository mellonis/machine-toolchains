//! Regression coverage for the `tmt` shell exiting quietly when its
//! stdout/stderr reader goes away mid-output, instead of panicking on a
//! broken pipe (`docs/tmt/cli.md`, the thin-renderer rule: every byte of
//! terminal output originates in `bin/tmt.rs` and `cli/`). Mirrors
//! `mtc-post-machine`'s `cli_broken_pipe.rs`.
//!
//! Rust binaries ignore `SIGPIPE` by default, so a write past a closed
//! pipe surfaces as `EPIPE`; the stdlib's `print!`/`println!` macros
//! `.expect()` that away into a panic. Both cases here build a fixture
//! large enough that its rendered output exceeds a pipe's kernel buffer
//! (commonly 64 KiB), spawn the real `tmt` binary, read a single byte from
//! the stream under test, then drop the read end while the child is still
//! writing — the only way to force a real `EPIPE` rather than a write that
//! happens to complete before the reader goes away.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::linker::LinkOptions;
use mtc_turing_machine::asm::{assemble, link};

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter (`dap_programs.rs`'s
/// `scratch` pattern) — two concurrently running `cargo test`/`nextest`
/// invocations must never resolve to the same directory.
fn scratch(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("broken-pipe-{name}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A one-tape `main` of `n` straight-line `nop`s — enough instructions
/// that `dis --listing` and `run --trace` each render well past a pipe's
/// kernel buffer.
fn big_tma_source(n: usize) -> String {
    let mut src = String::from(".section code\n.routine main, tapes=1, alpha=(1)\n.func main\n");
    for _ in 0..n {
        src.push_str("        nop\n");
    }
    src.push_str("        stp\n");
    src
}

/// Assembles+links `big_tma_source(n)` standalone and writes the
/// executable into `dir`.
fn write_big_tmx(dir: &Path, name: &str, n: usize) -> PathBuf {
    let object = assemble(&big_tma_source(n), false).unwrap();
    let linked = link(&[object], &[], LinkOptions::default()).unwrap();
    let path = dir.join(format!("{name}.tmx"));
    fs::write(&path, linked.executable.to_bytes()).unwrap();
    path
}

/// A single blank-cell tape block over the 1-symbol alphabet `big_tma_source`
/// declares — the minimum a `run` needs.
fn write_one_tape_block(dir: &Path, name: &str) -> PathBuf {
    let block = TapeBlockFile {
        alphabet: vec!["_".to_string()],
        tapes: vec![TapeSnapshot {
            origin: 0,
            cells: vec![0],
            head: 0,
            alphabet: None,
        }],
    };
    let path = dir.join(format!("{name}.tmt"));
    fs::write(&path, block.to_bytes().unwrap()).unwrap();
    path
}

#[test]
fn dis_listing_exits_quietly_on_broken_stdout_pipe() {
    let dir = scratch("dis");
    let tmx = write_big_tmx(&dir, "big", 8_000);

    let mut child = Command::new(env!("CARGO_BIN_EXE_tmt"))
        .args(["dis", "--listing", tmx.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Read exactly one byte, then close our end of the pipe while the
    // child is (almost certainly) still writing the rest of the listing —
    // the only way to force a real EPIPE deterministically.
    let mut stdout = child.stdout.take().unwrap();
    let mut first_byte = [0u8; 1];
    stdout.read_exact(&mut first_byte).unwrap();
    drop(stdout);

    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected a quiet exit 0, got {:?}; stderr:\n{stderr}",
        output.status.code()
    );
    assert_ne!(output.status.code(), Some(101), "stderr:\n{stderr}");
    assert!(
        !stderr.contains("panicked"),
        "stdout write should not panic on a broken pipe; stderr:\n{stderr}"
    );
}

#[test]
fn run_trace_exits_quietly_on_broken_stderr_pipe() {
    let dir = scratch("trace");
    let tmx = write_big_tmx(&dir, "big", 8_000);
    let tmt_block = write_one_tape_block(&dir, "big");

    let mut child = Command::new(env!("CARGO_BIN_EXE_tmt"))
        .args([
            "run",
            tmx.to_str().unwrap(),
            "--tape-block",
            tmt_block.to_str().unwrap(),
            "--trace",
            "--max-steps",
            "20000",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // `--trace` streams to stderr live (docs/tmt/cli.md); break that pipe
    // the same way, one byte in.
    let mut stderr = child.stderr.take().unwrap();
    let mut first_byte = [0u8; 1];
    stderr.read_exact(&mut first_byte).unwrap();
    drop(stderr);

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "expected a quiet exit 0, got {:?}; stdout:\n{stdout}",
        output.status.code()
    );
    assert_ne!(output.status.code(), Some(101), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("panicked"),
        "trace writes should not panic on a broken pipe; stdout:\n{stdout}"
    );
}
