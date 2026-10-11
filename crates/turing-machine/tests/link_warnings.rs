//! Link warnings on the CLI: printed always, suppressed by `--allow`,
//! promoted by `-Werror` (docs/tmt/cli.md (link warnings)).

use std::process::Command;

use mtc_core::formats::object::{ObjectFile, SymbolDef};
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
fn scratch(name: &str) -> ScratchDir {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
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

// --- undeclared-exit -----------------------------------------------------

/// Assemble `src`, then set `routine`'s declared exit count to `exits` in
/// the object's interface — the one way a body returning through an
/// undeclared exit reaches the linker now that the assembler refuses it.
fn declaring_exits(src: &str, routine: &str, exits: u8) -> ObjectFile {
    let mut obj = mtc_turing_machine::asm::assemble(src, false).unwrap();
    let blob = obj
        .symbols
        .iter()
        .find_map(|s| match s.def {
            SymbolDef::Defined { blob } | SymbolDef::Local { blob } if s.name == routine => {
                Some(blob)
            }
            _ => None,
        })
        .expect("the routine is defined");
    obj.interface
        .as_mut()
        .expect("the object has an interface")
        .routines[blob as usize]
        .exits = exits;
    obj
}

/// A hand-written callee declaring two exits whose second branch returns
/// through exit 2 — what `tmt compile -S` prints for a two-exit routine,
/// with its `retx #1` edited to `retx #2`.
const UNDECLARED_EXIT: &str = "\
.section tables
T0:     .row    [0]
        .row    [*]
D0:     .targets s__0, s__1
.section code
.routine sub, tapes=1, alpha=(3), exits=2, noreturn
.param t, ('_', '0', '1'), opaque
.func sub local
        rd
        mtc     T0
        djmp    D0
s__0:
        retx    #0
s__1:
        retx    #2
.routine main, tapes=2, alpha=(3, 3)
.param d, ('_', '0', '1')
.param out, ('_', '0', '1')
.func main
        call    sub [0] exits=(won, lost)
        jmp     done
won:
        wrmv    [-, 1], [., .]
        stp
lost:
        wrmv    [-, 2], [., .]
        stp
done:
        hlt
";

/// The assembler refuses the body above: its routine declares two exits and
/// the second branch returns through exit 2.
///
/// Mutation it catches: drop the check in the core assembler and the file
/// assembles.
#[test]
fn the_assembler_refuses_a_return_through_an_undeclared_exit() {
    let dir = scratch("link_warnings_assembler_refuses");
    let src = dir.join("fire.tma");
    std::fs::write(&src, UNDECLARED_EXIT).unwrap();
    let err = execute(&args(&[
        "asm",
        src.to_str().unwrap(),
        "-o",
        dir.join("fire.tmo").to_str().unwrap(),
    ]))
    .expect_err("a return through an undeclared exit does not assemble");
    let text = err.to_string();
    assert!(
        text.contains(
            "15:9: error: `sub` returns through exit 2, but its signature declares 2 exit(s) [undeclared-exit]"
        ),
        "{text}"
    );
}

/// The linker refuses an object whose routine returns through an exit its
/// interface does not declare, under every mechanism. The assembler
/// refuses this body, so the object is built with a third declared exit
/// and lowered back to the two its call site binds.
///
/// Mutation it catches: drop `stamp::check_declared_exits` from `link` —
/// frames links again and the copying mechanisms say `bad binding`
/// instead.
#[test]
fn the_linker_refuses_an_object_carrying_one_under_every_mechanism() {
    let dir = scratch("undeclared_exit_link");
    let src = UNDECLARED_EXIT.replace("exits=2, noreturn", "exits=3, noreturn");
    let obj = declaring_exits(&src, "sub", 2);
    let path = dir.join("fire.tmo");
    std::fs::write(&path, obj.to_bytes()).unwrap();
    for mech in ["mono", "frames", "hybrid"] {
        let err = execute(&args(&[
            "link",
            path.to_str().unwrap(),
            "--nostdlib",
            "--call-mech",
            mech,
            "-o",
            dir.join(format!("a_{mech}.tmx")).to_str().unwrap(),
        ]))
        .expect_err("an undeclared exit stops the link");
        assert!(
            err.to_string()
                .contains("`sub` returns through exit 2, but its signature declares 2 exit(s)"),
            "{mech}: {err}"
        );
    }
}

/// An unsigned entry making a plain call.
const MAIN_PLAIN: &str = "\
.func main
        call    sub
        stp
";

/// Declares three exits so it assembles; the test lowers the count to two.
const SUB_OVER: &str = "\
.routine sub, tapes=1, alpha=(3), exits=3
.param p, ('_', '0', '1')
.func sub
        retx    #2
";

/// The check does not need the entry to carry a signature: this link used
/// to stop on the entry's missing signature, never reaching the body.
///
/// Mutation it catches: keep the scan inside the engine, which an
/// unsigned entry never reaches — the link then reports the missing
/// signature instead.
#[test]
fn the_link_check_does_not_need_a_signed_entry() {
    let dir = scratch("undeclared_exit_unsigned_entry");
    let main = mtc_turing_machine::asm::assemble(MAIN_PLAIN, false).unwrap();
    let sub = declaring_exits(SUB_OVER, "sub", 2);
    std::fs::write(dir.join("main_plain.tmo"), main.to_bytes()).unwrap();
    std::fs::write(dir.join("sub.tmo"), sub.to_bytes()).unwrap();
    let err = execute(&args(&[
        "link",
        dir.join("main_plain.tmo").to_str().unwrap(),
        dir.join("sub.tmo").to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]))
    .expect_err("an undeclared exit stops the link");
    assert!(
        err.to_string()
            .contains("`sub` returns through exit 2, but its signature declares 2 exit(s)"),
        "{err}"
    );
}

/// `undeclared-exit` is no longer a link-warning code, so `--allow` names
/// an unknown rule exactly as it does for any other unknown code. The
/// object is the legal near miss (`retx #1` of two), so nothing else
/// could stop the link.
///
/// Mutation it catches: leave the `undeclared-exit` row in
/// `DIAGNOSTIC_CODES` and the flag is accepted.
#[test]
fn allowing_undeclared_exit_is_an_unknown_code() {
    let dir = scratch("undeclared_exit_allow");
    let obj = mtc_turing_machine::asm::assemble(
        &UNDECLARED_EXIT.replace("retx    #2", "retx    #1"),
        false,
    )
    .unwrap();
    let path = dir.join("fire_ok.tmo");
    std::fs::write(&path, obj.to_bytes()).unwrap();
    let err = execute(&args(&[
        "link",
        path.to_str().unwrap(),
        "--nostdlib",
        "--allow",
        "undeclared-exit",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]))
    .expect_err("an unknown code is refused");
    assert!(
        err.to_string()
            .contains("unknown lint rule `undeclared-exit`"),
        "{err}"
    );
}

/// A manifest's `lint.allow` is validated against the same table.
///
/// Mutation it catches: the manifest loader validating against a list of
/// its own that still holds the old warning code.
#[test]
fn a_manifest_allowing_undeclared_exit_is_refused() {
    let dir = scratch("undeclared_exit_manifest_allow");
    std::fs::write(
        dir.join("tmt.json"),
        r#"{ "lint": { "allow": ["undeclared-exit"] }, "project": { "targets": { "app": { "sources": ["a.tmc"] } } } }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("a.tmc"),
        "alphabet a { '_' }\nmachine { tape t: a; entry state s { [*] -> move [>] stop; } }\n",
    )
    .unwrap();
    let out = tmt().args(["build"]).current_dir(&dir).output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("unknown lint rule `undeclared-exit` in lint.allow"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every `.tmc` and `.tma` the repository ships, recursively, under the
/// three roots that hold real programs — the same roots
/// `plain_site_sweep.rs` measures.
fn corpus() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut out = Vec::new();
    for dir in [
        root.join("docs/examples"),
        root.join("crates/turing-machine/tests/golden"),
        root.join("crates/turing-machine/src/stdlib"),
    ] {
        collect(&dir, &mut out);
    }
    out.sort();
    out
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if matches!(
            p.extension().and_then(|s| s.to_str()),
            Some("tmc") | Some("tma")
        ) {
            out.push(p);
        }
    }
}

/// The control the corpus sweep carries: a routine with two `state`
/// parameters, which the compiler lowers to `retx #0` and `retx #1`.
/// No shipped program declares an exit-bearing ROUTINE today — every
/// shipped `state` parameter belongs to a graph, which is spliced into
/// its host and returns through nothing — so without this control the
/// sweep would never reach a body the scan grades.
const COMPILED_TWO_EXITS: &str = "\
alphabet ab { '_', '0', '1' }

routine sub(tape t: ab, state hit, state miss) {
  entry state s {
    ['_'] -> goto hit;
    [*]   -> goto miss;
  }
}

machine {
  tape d: ab;
  tape out: ab;
  entry state go { [*, *] -> call sub(t = d, hit = won, miss = lost) then done; }
  state won  { [*, *] -> write [-, '0'] stop; }
  state lost { [*, *] -> write [-, '1'] stop; }
  state done { [*, *] -> halt; }
}
";

/// Every shipped program that builds standalone, plus the compiled
/// control above, links against the embedded stdlib with no
/// `undeclared-exit` finding: the compiler lowers a state parameter to
/// the exit its own position names, so an honest body never trips the
/// check. An assembly refusal and a link refusal both carry that code
/// and both count as findings, never as a skip. The check runs once,
/// before any mechanism lowers anything, so one mechanism covers it.
/// Programs that do not build standalone (the stdlib itself, a
/// multi-unit target's pieces) are skipped; the floor keeps the sweep
/// from passing vacuously.
///
/// Mutation it catches: refuse every multi-exit return regardless of its
/// index, and the control is refused and becomes a finding — the shipped
/// corpus alone would not, since none of it reaches a `retx`. And let the control
/// stop compiling or linking (it would be skipped like any other
/// program), and the named check on it turns this red, where the floor
/// alone would not once one more shipped program links.
#[test]
fn no_shipped_program_returns_through_an_undeclared_exit() {
    use mtc_core::linker::{CallMech, LinkError, LinkOptions, link};
    use mtc_turing_machine::asm::{assemble, tm1_syntax};
    use mtc_turing_machine::compiler::{CompileOptions, compile};

    let stdlib = mtc_turing_machine::stdlib::object().clone();
    let syntax = tm1_syntax();
    let mut linked = 0usize;
    let mut control_linked = false;
    let mut findings = Vec::new();
    let control = std::path::PathBuf::from("<compiled two-exit control>.tmc");
    for path in corpus().into_iter().chain([control.clone()]) {
        let src = if path == control {
            COMPILED_TWO_EXITS.to_string()
        } else {
            std::fs::read_to_string(&path).unwrap()
        };
        let obj = if path.extension().and_then(|s| s.to_str()) == Some("tma") {
            match assemble(&src, false) {
                Ok(o) => o,
                Err(e) if e.kind.code() == "undeclared-exit" => {
                    findings.push(format!("{}: {}", path.display(), e.kind));
                    continue;
                }
                Err(_) => continue,
            }
        } else {
            match compile(&src, CompileOptions::default()) {
                Ok(out) => out.object,
                Err(_) => continue,
            }
        };
        match link(
            &syntax,
            std::slice::from_ref(&obj),
            std::slice::from_ref(&stdlib),
            LinkOptions {
                call_mech: CallMech::Frames,
                ..Default::default()
            },
        ) {
            Ok(_) => {
                linked += 1;
                control_linked |= path == control;
            }
            Err(e @ LinkError::UndeclaredExit { .. }) => {
                findings.push(format!("{}: {e}", path.display()));
            }
            Err(_) => continue,
        }
    }
    assert!(
        linked >= 16,
        "the corpus sweep linked only {linked} programs"
    );
    assert!(
        control_linked,
        "the compiled two-exit control did not link, so the sweep reached no `retx`"
    );
    assert!(findings.is_empty(), "{findings:#?}");
}
