//! The one-definition rule's first step: a link error that concerns two
//! inputs names both, by the label the driver gave each (docs/core.md
//! (linking)). Every assertion goes through the public CLI entry.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mtc_post_machine::cli::execute;

/// A fresh, per-call scratch directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter.
fn scratch(name: &str) -> ScratchDir {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("one-def-{name}-{}-{n}", std::process::id()));
    ScratchDir::fresh(dir)
}

/// A scratch directory this test owns. Its name can repeat across
/// processes — a later test process may reuse a pid, restarting the
/// counter — so `fresh` removes whatever an earlier run left under the
/// name before creating it, and `Drop` removes it again once the test
/// ends, passing or panicking.
struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn fresh(dir: std::path::PathBuf) -> Self {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("clearing stale scratch dir {}: {e}", dir.display()),
        }
        std::fs::create_dir_all(&dir).unwrap();
        ScratchDir(dir)
    }
}

impl std::ops::Deref for ScratchDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for ScratchDir {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn run(argv: &[&str]) -> Result<mtc_post_machine::cli::CliOutput, String> {
    execute(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

fn compile_ok(dir: &Path, name: &str, src: &str) -> PathBuf {
    let input = write(dir, name, src);
    let obj = input.with_extension("pmo");
    run(&[
        "compile",
        input.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ])
    .unwrap_or_else(|e| panic!("{name}: {e}"));
    obj
}

const PA: &str = "export f() { right; }\nmain() { @f(); }\n";
const PB: &str = "export f() { left; }\n";

/// Mutation: `pmt link` leaves `LinkOptions::labels` empty — `input #0/#1`.
#[test]
fn a_duplicate_symbol_between_two_objects_names_both() {
    let dir = scratch("user_user");
    let pa = compile_ok(&dir, "pa.pmc", PA);
    let pb = compile_ok(&dir, "pb.pmc", PB);
    let out = dir.join("pab.pmx");
    let err = run(&[
        "link",
        pa.to_str().unwrap(),
        pb.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(
        err.contains(&format!(
            "duplicate symbol `f`: defined by {} and by {}",
            pa.display(),
            pb.display()
        )),
        "{err}"
    );
}
