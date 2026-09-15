//! Link warnings on the CLI: printed always, suppressed by `--allow`,
//! promoted by `-Werror` (docs/tmt/cli.md (link warnings)).

use std::process::Command;

use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A fresh scratch directory under `CARGO_TARGET_TMPDIR`, unique per call
/// (process id + an atomic counter), mirroring `mode_equivalence.rs`'s own
/// helper of the same name — this crate has no shared test-support module.
/// `remove_dir_all` first (the `build_driver.rs` idiom): `CARGO_TARGET_TMPDIR`
/// persists across runs and pids recycle, so a stale artifact from an
/// earlier pass could otherwise satisfy a file-EXISTENCE assertion (or, as
/// this file's manifest-mode tests need, a file-ABSENCE one) after the code
/// that writes it has broken.
fn scratch(name: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The `tmt` binary under test, spawned as a real process — manifest mode
/// is driven through argv/cwd exactly as a user would invoke it, mirroring
/// `build_driver.rs`'s own `tmt()` helper (this crate has no shared
/// test-support module, hence the duplicate).
fn tmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tmt"))
}

/// A caller whose 5-symbol band plainly calls a 3-symbol callee: the
/// `narrow-alphabet` warning's minimal shape.
const NARROW: &str = "\
.routine main, tapes=1, alpha=(5)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

fn build_object(dir: &std::path::Path) -> std::path::PathBuf {
    let src = dir.join("narrow.tma");
    std::fs::write(&src, NARROW).unwrap();
    let obj = dir.join("narrow.tmo");
    execute(&args(&[
        "asm",
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]))
    .expect("assembles");
    obj
}

/// Mutation it catches: render diagnostics only under `-v` and this
/// assertion fails, because no `-v` is passed.
#[test]
fn a_link_warning_prints_without_v() {
    let dir = scratch("link_warnings_plain");
    let obj = build_object(&dir);
    let out = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]))
    .expect("links");
    assert_eq!(out.code, 0, "a warning does not fail the link");
    assert!(
        out.stderr.contains("warning:") && out.stderr.contains("[narrow-alphabet]"),
        "{}",
        out.stderr
    );
}

/// Mutation it catches: ignore the allow list and the warning still
/// prints.
#[test]
fn allow_suppresses_a_link_warning() {
    let dir = scratch("link_warnings_allow");
    let obj = build_object(&dir);
    let out = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "--nostdlib",
        "--allow",
        "narrow-alphabet",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]))
    .expect("links");
    assert_eq!(out.code, 0);
    assert!(!out.stderr.contains("narrow-alphabet"), "{}", out.stderr);
}

/// Mutation it catches: leave `-Werror` off the link stage and this link
/// succeeds.
#[test]
fn werror_promotes_a_link_warning() {
    let dir = scratch("link_warnings_werror");
    let obj = build_object(&dir);
    let err = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "--nostdlib",
        "-Werror",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]));
    assert!(err.is_err(), "-Werror must fail the link: {err:?}");
}

/// An unknown code is a typo, caught up front like a lint `--allow`.
/// Mutation it catches: skip validation and a typo'd `--allow` silently
/// suppresses nothing.
#[test]
fn an_unknown_allow_code_is_rejected() {
    let dir = scratch("link_warnings_typo");
    let obj = build_object(&dir);
    let err = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "--nostdlib",
        "--allow",
        "narow-alphabet",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]));
    assert!(err.is_err(), "a typo'd code must be rejected: {err:?}");
}

// --- manifest-mode `tmt build` coverage --------------------------------
//
// The four tests above exercise `tmt link` directly (argv mode);
// `tmt build`'s manifest mode has its OWN `--allow` union (with the
// manifest's `lint.allow`, no second discovery walk) and its own
// `profile.werror || flags.werror` resolution feeding the link stage,
// separate code paths from `tmt link`'s. Each of the three tests below
// writes its own `tmt.json` over the same `narrow.tma` (so the ONLY thing
// that varies between them is the manifest), and drives the real `tmt`
// binary from the manifest's directory, mirroring `build_driver.rs`'s own
// idiom.

/// Writes `narrow.tma` (the same `NARROW` fixture the argv-mode tests
/// above use) into `dir`, ready for a manifest's `"sources"` to name it.
fn write_narrow_source(dir: &std::path::Path) {
    std::fs::write(dir.join("narrow.tma"), NARROW).unwrap();
}

/// `lint.allow` in the manifest silences the link-stage `narrow-alphabet`
/// warning exactly as `tmt link --allow` does, over the SAME union
/// namespace `--allow` and `lint.allow` share. Mutation it catches: union
/// only the compile-warning allow list (drop the link-diagnostic codes
/// from the manifest's `lint.allow` before checking them against
/// `LinkReport.diagnostics`) and this build still prints the warning.
#[test]
fn manifest_mode_lint_allow_silences_a_link_warning() {
    let dir = scratch("link_warnings_manifest_allow");
    write_narrow_source(&dir);
    std::fs::write(
        dir.join("tmt.json"),
        r#"{
            "lint": { "allow": ["narrow-alphabet"] },
            "project": { "targets": { "app": { "sources": ["narrow.tma"] } } }
        }"#,
    )
    .unwrap();

    let out = tmt().args(["build"]).current_dir(&dir).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("narrow-alphabet"),
        "manifest lint.allow must silence the link warning: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.join("app.tmx").is_file(),
        "an allowed warning must not block the build"
    );
}

/// A manifest profile's `werror: true` promotes the link warning to a
/// build failure, and — because the promotion decision runs before the
/// executable and its `.map` sidecar are written — a strict build that
/// fails leaves NEITHER artifact behind. Mutation it catches: resolve
/// `profile.werror` for the compile stage only, never folding it into the
/// link stage's promotion decision, and this build succeeds with both
/// files on disk.
#[test]
fn manifest_mode_profile_werror_fails_the_build_and_leaves_no_artifact() {
    let dir = scratch("link_warnings_manifest_werror");
    write_narrow_source(&dir);
    std::fs::write(
        dir.join("tmt.json"),
        r#"{
            "project": {
                "profiles": { "debug": { "werror": true } },
                "targets": { "app": { "sources": ["narrow.tma"] } }
            }
        }"#,
    )
    .unwrap();

    let out = tmt().args(["build"]).current_dir(&dir).output().unwrap();
    assert!(
        !out.status.success(),
        "the profile's werror must fail the build"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("treated as errors"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !dir.join("app.tmx").exists(),
        "a failed strict build must not leave the executable behind"
    );
    assert!(
        !dir.join("app.tmx.map").exists(),
        "a failed strict build must not leave the .map sidecar behind"
    );
}

/// The control: neither `lint.allow` nor a `werror` profile in play, so
/// the link warning prints and the build still succeeds — the baseline
/// the other two manifest-mode tests each vary exactly one thing away
/// from. Mutation it catches: swallow link diagnostics in manifest mode
/// entirely (render nothing, ever) and this assertion — the ONLY one of
/// the three checking the warning DOES print — goes unnoticed by the
/// other two, which only check its absence or a failure.
#[test]
fn manifest_mode_prints_the_warning_and_still_succeeds_by_default() {
    let dir = scratch("link_warnings_manifest_plain");
    write_narrow_source(&dir);
    std::fs::write(
        dir.join("tmt.json"),
        r#"{
            "project": { "targets": { "app": { "sources": ["narrow.tma"] } } }
        }"#,
    )
    .unwrap();

    let out = tmt().args(["build"]).current_dir(&dir).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("[narrow-alphabet]"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join("app.tmx").is_file());
    assert!(dir.join("app.tmx.map").is_file());
}
