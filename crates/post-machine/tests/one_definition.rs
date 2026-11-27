//! The one-definition rule at link (docs/core.md (linking)): a name two
//! inputs define is refused once the program reaches it — user objects,
//! libraries and the embedded standard library alike — and a link error
//! that concerns two inputs names both, by the label the driver gave
//! each. Every assertion goes through the public CLI entry.

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

/// `build` in argv mode labels its units by the paths it was given.
/// Mutation: the argv-mode `build` link leaves `LinkOptions::labels`
/// empty — `input #0` and `input #1`.
#[test]
fn an_argv_build_names_both_duplicate_definers() {
    let dir = scratch("build_argv");
    let pa = write(&dir, "pa.pmc", PA);
    let pb = write(&dir, "pb.pmc", PB);
    let out = dir.join("b.pmx");
    let err = run(&[
        "build",
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

/// `build` in manifest mode labels its units by the raw source strings
/// the manifest declares. Mutation: the manifest-mode link leaves
/// `LinkOptions::labels` empty — the
/// sidecar-relative `sources` fallback then names `../src/pa.pmc`.
#[test]
fn a_manifest_build_names_both_duplicate_definers() {
    let dir = scratch("build_manifest");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    write(&dir, "src/pa.pmc", PA);
    write(&dir, "src/pb.pmc", PB);
    write(
        &dir,
        "pmt.json",
        r#"{ "project": { "targets": { "app": {
            "sources": ["src/pa.pmc", "src/pb.pmc"], "output": "out/app.pmx" } } } }"#,
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_pmt"))
        .args(["build", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("duplicate symbol `f`: defined by src/pa.pmc and by src/pb.pmc"),
        "{err}"
    );
}

const OVERRIDE: &str = "\
namespace std { export goToEnd() { 1: left(!); } }
use std::goToEnd;
main() { @goToEnd(!); }
";

const PNEAR: &str = "\
namespace std { export goToEnd() { 1: left(!); } }
main() { right; }
";

/// A user definition of a `std::` routine the program calls: the error
/// labels the embedded library `the standard library`. Mutation: the PM
/// driver does not label the stdlib — the message says `input #1`.
#[test]
fn a_user_definition_of_a_reached_std_routine_is_refused() {
    let dir = scratch("override");
    let path = write(&dir, "override.pmc", OVERRIDE);
    let out = dir.join("ov.pmx");
    let err = run(&["build", path.to_str().unwrap(), "-o", out.to_str().unwrap()]).unwrap_err();
    assert!(
        err.contains(&format!(
            "duplicate symbol `std::goToEnd`: defined by {} and by the standard library",
            path.display()
        )),
        "{err}"
    );
}

/// The same definition, never called: the program links. Mutation: the
/// check ignores reach — the build is refused and `unwrap` panics.
#[test]
fn an_unreached_user_definition_of_a_std_name_still_links() {
    let dir = scratch("pnear");
    let path = write(&dir, "pnear.pmc", PNEAR);
    let out = dir.join("pn.pmx");
    run(&["build", path.to_str().unwrap(), "-o", out.to_str().unwrap()]).unwrap();
}

/// A user object and a `-l` library both defining a reached name.
/// Mutation: `pmt link` does not label `-l` libraries — `input #1`.
#[test]
fn a_reached_name_an_object_and_a_library_both_define_is_refused() {
    let dir = scratch("obj_lib");
    let pa = compile_ok(&dir, "pa.pmc", PA);
    compile_ok(&dir, "pb.pmc", PB);
    let out = dir.join("pl.pmx");
    let err = run(&[
        "link",
        pa.to_str().unwrap(),
        "-L",
        dir.to_str().unwrap(),
        "-l",
        "pb",
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(
        err.contains(&format!(
            "duplicate symbol `f`: defined by {} and by library `pb`",
            pa.display()
        )),
        "{err}"
    );
}
