//! Link warnings on the CLI: printed always, suppressed by `--allow`,
//! promoted by `-Werror` (docs/tmt/cli.md (link warnings)).

use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A fresh scratch directory under `CARGO_TARGET_TMPDIR`, unique per call
/// (process id + an atomic counter), mirroring `mode_equivalence.rs`'s own
/// helper of the same name — this crate has no shared test-support module.
fn scratch(name: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
#[ignore = "raised by the plain-site check task"]
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
#[ignore = "raised by the plain-site check task"]
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
#[ignore = "raised by the plain-site check task"]
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
#[ignore = "raised by the plain-site check task"]
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
