//! The one-definition rule's first step: a link error that concerns two
//! inputs names both, by the label the driver gave each (docs/core.md
//! (linking)). Every assertion goes through the public CLI entry.

use std::path::{Path, PathBuf};

use mtc_turing_machine::cli::{CliOutput, execute};

/// `R1` and `R2` declare `lib::r` identically (the same explicit `writes`
/// contract) and differ only in their bodies, so the compile-time
/// one-declaration rule (docs/tmt/language.md (declarations)) lets both
/// through and it is the LINK that refuses the second definition.
const R1: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab writes { '_', 'a' }) {
    entry state s { [*] -> write ['a'] return; }
  }
}
";

const R2: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab writes { '_', 'a' }) {
    entry state s { [*] -> write ['_'] return; }
  }
}
";

/// Calls `lib::r`; compiled with `--extern r1.tmc` so it also records an
/// import of `lib::ab`.
const APP: &str = "\
use lib::ab;
machine {
  tape t: ab;
  entry state s { [*] -> call lib::r(t = t) then stop; }
}
";

const G1: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export graph walk(tape t: ab, state done) {
    entry state w { [*] -> write ['a'] goto done; }
  }
}
";

const G2: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export graph walk(tape t: ab, state done) {
    entry state w { [*] -> write ['_'] goto done; }
  }
}
";

/// Grafts `lib::walk`; compiled with `--extern g1.tmc`, so it records
/// the digest of g1's graph.
const GAPP: &str = "\
use lib::ab;
machine {
  tape t: ab;
  entry graft lib::walk(t = t, done = stop) as w;
}
";

fn scratch(name: &str) -> ScratchDir {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
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

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn run(argv: &[&str]) -> Result<CliOutput, String> {
    execute(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

/// `tmt compile SRC -o DIR/OUT --nostdlib [--extern E]…`, which must succeed.
fn compile_ok(dir: &Path, src: &Path, out: &str, externs: &[&Path]) -> PathBuf {
    let obj = dir.join(out);
    let mut argv = vec![
        "compile".to_string(),
        src.to_str().unwrap().into(),
        "--nostdlib".into(),
    ];
    for e in externs {
        argv.push("--extern".into());
        argv.push(e.to_str().unwrap().into());
    }
    argv.push("-o".into());
    argv.push(obj.to_str().unwrap().into());
    execute(&argv).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
    obj
}

/// The r1/r2/app trio, compiled into `dir`: returns (app, r1, r2) objects.
fn routine_trio(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let r1 = write(dir, "r1.tmc", R1);
    let r2 = write(dir, "r2.tmc", R2);
    let app = write(dir, "app.tmc", APP);
    (
        compile_ok(dir, &app, "app.tmo", &[&r1]),
        compile_ok(dir, &r1, "r1.tmo", &[]),
        compile_ok(dir, &r2, "r2.tmo", &[]),
    )
}

/// The g1/g2/gapp trio, compiled into `dir`: returns (gapp, g1, g2) objects.
fn graph_trio(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let g1 = write(dir, "g1.tmc", G1);
    let g2 = write(dir, "g2.tmc", G2);
    let gapp = write(dir, "gapp.tmc", GAPP);
    (
        compile_ok(dir, &gapp, "gapp.tmo", &[&g1]),
        compile_ok(dir, &g1, "g1.tmo", &[]),
        compile_ok(dir, &g2, "g2.tmo", &[]),
    )
}

/// Two user objects defining `lib::r`: the error names both by the paths
/// they were given as. Mutation: `tmt link` leaves `LinkOptions::labels`
/// empty — the message then says `input #1` and `input #2`.
#[test]
fn a_duplicate_symbol_between_two_objects_names_both() {
    let dir = scratch("one_def_user_user");
    let (app, r1, r2) = routine_trio(&dir);
    let out = dir.join("uu.tmx");
    let err = run(&[
        "link",
        app.to_str().unwrap(),
        r1.to_str().unwrap(),
        r2.to_str().unwrap(),
        "--nostdlib",
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(
        err.contains("duplicate symbol `lib::r`: defined by "),
        "{err}"
    );
    assert!(
        err.contains(&format!("{} and by {}", r1.display(), r2.display())),
        "{err}"
    );
}

/// A graft drift names the consumer by its path and the exporting library
/// by its `-l` name. Mutation: the drift check keeps naming inputs from
/// `sources` alone — `tmt link` passes none, so the message says `input #0`.
#[test]
fn a_graft_drift_names_the_consumer_and_the_library() {
    let dir = scratch("one_def_drift_names");
    let (gapp, _g1, _g2) = graph_trio(&dir);
    let out = dir.join("gd.tmx");
    let err = run(&[
        "link",
        gapp.to_str().unwrap(),
        "--nostdlib",
        "-L",
        dir.to_str().unwrap(),
        "-l",
        "g2",
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(
        err.contains(&format!(
            "`lib::walk` was grafted into {} from a header that does not match library `g2`",
            gapp.display()
        )),
        "{err}"
    );
}

/// `build` in argv mode labels its units by the paths it was given.
/// Mutation: the argv-mode `build` link leaves `LinkOptions::labels`
/// empty — the message then says `input #1` and `input #2`.
#[test]
fn an_argv_build_names_both_duplicate_definers() {
    let dir = scratch("one_def_build_argv");
    let app = write(&dir, "app.tmc", APP);
    let r1 = write(&dir, "r1.tmc", R1);
    let r2 = write(&dir, "r2.tmc", R2);
    let out = dir.join("b.tmx");
    let err = run(&[
        "build",
        app.to_str().unwrap(),
        r1.to_str().unwrap(),
        r2.to_str().unwrap(),
        "--nostdlib",
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(
        err.contains(&format!(
            "duplicate symbol `lib::r`: defined by {} and by {}",
            r1.display(),
            r2.display()
        )),
        "{err}"
    );
}

/// `build` in manifest mode labels its units by the raw source strings
/// the manifest declares. Mutation: the manifest-mode link leaves
/// `LinkOptions::labels` empty — the
/// sidecar-relative `sources` fallback then names `../src/r1.tmc`.
#[test]
fn a_manifest_build_names_both_duplicate_definers() {
    let dir = scratch("one_def_build_manifest");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    write(&dir, "src/app.tmc", APP);
    write(&dir, "src/r1.tmc", R1);
    write(&dir, "src/r2.tmc", R2);
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "stdlib": false, "targets": {
            "app": { "sources": ["src/app.tmc", "src/r1.tmc", "src/r2.tmc"],
                     "output": "out/app.tmx" } } } }"#,
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_tmt"))
        .args(["build", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("duplicate symbol `lib::r`: defined by src/r1.tmc and by src/r2.tmc"),
        "{err}"
    );
}

/// An alphabet drift names the consumer by its path and the exporting
/// library by its `-l` name. Mutation: `check_imported_alphabets` is
/// handed no names — the message says `input #0` and `input #1`.
#[test]
fn an_alphabet_drift_names_the_consumer_and_the_library() {
    let dir = scratch("one_def_alphabet_drift");
    let r1 = write(&dir, "r1.tmc", R1);
    let r3 = write(&dir, "r3.tmc", &R1.replace("'a'", "'b'"));
    let app = write(&dir, "app.tmc", APP);
    let app_o = compile_ok(&dir, &app, "app.tmo", &[&r1]);
    compile_ok(&dir, &r3, "r3.tmo", &[]);
    let out = dir.join("ad.tmx");
    let err = run(&[
        "link",
        app_o.to_str().unwrap(),
        "--nostdlib",
        "-L",
        dir.to_str().unwrap(),
        "-l",
        "r3",
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains(&app_o.display().to_string()), "{err}");
    assert!(err.contains("library `r3`"), "{err}");
}

/// Imports `lib::ab` and calls nothing, so a name two libraries share is
/// never reached.
const APP0: &str = "\
use lib::ab;
machine {
  tape t: ab;
  entry state s { [*] -> stop; }
}
";

/// The stdlib's own `goToNumber` signature, re-defined by a user unit:
/// `tmt interface` prints it identically to `std.tmh`'s line, so the
/// compile-time one-declaration rule lets it through and the link must
/// refuse it.
const OWNSAME: &str = "\
namespace std {
  namespace binaryNumbers {
    export routine goToNumber(tape num: std::binaryNumbers::symbols writes {} leaves { '$' }) {
      entry state walk {
        ['$'] -> return;
        [*]   -> move [>] goto walk;
      }
    }
  }
}

machine {
  tape num: std::binaryNumbers::symbols;
  entry state s { [*] -> call std::binaryNumbers::goToNumber(num = num) then stop; }
}
";

/// A user object and a `-l` library both defining a reached name: the
/// error labels the library as `library r2`. Mutation: `tmt link` does
/// not label `-l` libraries — the message says `input #1` for it.
#[test]
fn a_reached_name_an_object_and_a_library_both_define_is_refused() {
    let dir = scratch("one_def_obj_lib");
    let (app, r1, _r2) = routine_trio(&dir);
    let out = dir.join("ol.tmx");
    let err = run(&[
        "link",
        app.to_str().unwrap(),
        r1.to_str().unwrap(),
        "--nostdlib",
        "-L",
        dir.to_str().unwrap(),
        "-l",
        "r2",
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(
        err.contains(&format!(
            "duplicate symbol `lib::r`: defined by {} and by library `r2`",
            r1.display()
        )),
        "{err}"
    );
}

/// Two libraries both defining a reached name. Mutation: library against
/// library is left first-wins — the link succeeds and `unwrap_err` panics.
#[test]
fn a_reached_name_two_libraries_define_is_refused() {
    let dir = scratch("one_def_lib_lib");
    let (app, _r1, _r2) = routine_trio(&dir);
    let out = dir.join("ll.tmx");
    let err = run(&[
        "link",
        app.to_str().unwrap(),
        "--nostdlib",
        "-L",
        dir.to_str().unwrap(),
        "-l",
        "r1",
        "-l",
        "r2",
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(
        err.contains("duplicate symbol `lib::r`: defined by library `r1` and by library `r2`"),
        "{err}"
    );
}

/// The same two libraries, but the program never reaches the shared name.
/// Mutation: the check ignores reach — the link is refused and `unwrap`
/// panics.
#[test]
fn an_unreached_name_two_libraries_share_still_links() {
    let dir = scratch("one_def_unreached");
    let r1 = write(&dir, "r1.tmc", R1);
    let r2 = write(&dir, "r2.tmc", R2);
    let app0 = write(&dir, "app0.tmc", APP0);
    let app0_o = compile_ok(&dir, &app0, "app0.tmo", &[&r1]);
    compile_ok(&dir, &r1, "r1.tmo", &[]);
    compile_ok(&dir, &r2, "r2.tmo", &[]);
    let out = dir.join("un.tmx");
    let ok = run(&[
        "link",
        app0_o.to_str().unwrap(),
        "--nostdlib",
        "-L",
        dir.to_str().unwrap(),
        "-l",
        "r1",
        "-l",
        "r2",
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(ok.code, 0);
}

/// A user unit defining a `std::` routine the program calls: the error
/// labels the embedded library `the standard library`. Mutation: `tmt
/// build` does not label the stdlib — the message says `input #1`.
#[test]
fn a_user_definition_of_a_reached_std_routine_is_refused() {
    let dir = scratch("one_def_std");
    let path = write(&dir, "ownsame.tmc", OWNSAME);
    let out = dir.join("ownsame.tmx");
    let err = run(&["build", path.to_str().unwrap(), "-o", out.to_str().unwrap()]).unwrap_err();
    assert!(
        err.contains(&format!(
            "duplicate symbol `std::binaryNumbers::goToNumber`: defined by {} and by the standard library",
            path.display()
        )),
        "{err}"
    );
}

/// `lib::ab` with a third glyph, and a routine under another name, so a
/// link with `R1` contests only the alphabet export.
const R3: &str = "\
namespace lib {
  export alphabet ab { '_', 'a', 'b' }
  export routine q(tape t: ab) {
    entry state s { [*] -> write ['b'] return; }
  }
}
";

/// `tmt link OBJECTS… --nostdlib -L DIR -l LIB…`, returning the outcome.
fn link_with(dir: &Path, objects: &[&Path], libs: &[&str]) -> Result<CliOutput, String> {
    let out = dir.join("out.tmx");
    let mut argv: Vec<String> = vec!["link".into()];
    argv.extend(objects.iter().map(|o| o.to_str().unwrap().to_string()));
    argv.push("--nostdlib".into());
    if !libs.is_empty() {
        argv.push("-L".into());
        argv.push(dir.to_str().unwrap().into());
    }
    for lib in libs {
        argv.push("-l".into());
        argv.push((*lib).into());
    }
    argv.push("-o".into());
    argv.push(out.to_str().unwrap().into());
    execute(&argv)
}

/// Two libraries export `lib::walk` differently and the consumer grafted
/// the first one's. Mutation: keep the first-wins exporter map — the
/// consumer agrees with the first library and the link succeeds.
#[test]
fn two_libraries_exporting_a_grafted_graph_differently_are_refused() {
    let dir = scratch("one_def_graph_lib_lib");
    let (gapp, _g1, _g2) = graph_trio(&dir);
    let err = link_with(&dir, &[&gapp], &["g1", "g2"]).unwrap_err();
    assert!(
        err.contains(
            "the graph `lib::walk` is exported differently by library `g1` and by library `g2`"
        ),
        "{err}"
    );
}

/// The same pair in the other order: the consumer now disagrees with the
/// first library, and the conflict is still what the link reports.
/// Mutation: run the drift check before the conflict pass — the message
/// is then the graft drift against the first library.
#[test]
fn the_export_conflict_is_reported_before_drift() {
    let dir = scratch("one_def_graph_order");
    let (gapp, _g1, _g2) = graph_trio(&dir);
    let err = link_with(&dir, &[&gapp], &["g2", "g1"]).unwrap_err();
    assert!(
        err.contains(
            "the graph `lib::walk` is exported differently by library `g2` and by library `g1`"
        ),
        "{err}"
    );
}

/// A byte copy of a library is the same export twice. Mutation: compare
/// exporters by input instead of by record — the copy is a conflict.
#[test]
fn identical_graph_exports_are_one_export() {
    let dir = scratch("one_def_graph_copy");
    let (gapp, g1, _g2) = graph_trio(&dir);
    std::fs::copy(&g1, dir.join("g1copy.tmo")).unwrap();
    let ok = link_with(&dir, &[&gapp], &["g1", "g1copy"]).unwrap();
    assert_eq!(ok.code, 0);
}

/// Differing library exports of a graph nothing grafts still link.
/// Mutation: refuse every differing pair regardless of consumption.
#[test]
fn differing_library_graph_exports_nobody_grafts_link() {
    let dir = scratch("one_def_graph_unconsumed");
    let (app, r1, _r2) = routine_trio(&dir);
    graph_trio(&dir);
    let ok = link_with(&dir, &[&app, &r1], &["g1", "g2"]).unwrap();
    assert_eq!(ok.code, 0);
}

/// Two user objects that export one graph differently are refused even
/// though nothing grafts it. Mutation: drop the both-user-objects arm.
#[test]
fn two_user_objects_exporting_one_graph_differently_are_refused() {
    let dir = scratch("one_def_graph_user_user");
    let (app, r1, _r2) = routine_trio(&dir);
    let (_gapp, g1, g2) = graph_trio(&dir);
    let err = link_with(&dir, &[&app, &r1, &g1, &g2], &[]).unwrap_err();
    assert!(
        err.contains(&format!(
            "the graph `lib::walk` is exported differently by {} and by {}",
            g1.display(),
            g2.display()
        )),
        "{err}"
    );
}

/// Two libraries export `lib::ab` with different glyph lists and the
/// application imports it, in either order. Mutation: leave the alphabet
/// exporter map first-wins — the first order reports a drift against the
/// second library at best, or links, never the conflict.
#[test]
fn two_libraries_exporting_an_imported_alphabet_differently_are_refused() {
    let dir = scratch("one_def_alphabet_lib_lib");
    let (app, _r1, _r2) = routine_trio(&dir);
    let r3 = write(&dir, "r3.tmc", R3);
    compile_ok(&dir, &r3, "r3.tmo", &[]);
    let err = link_with(&dir, &[&app], &["r1", "r3"]).unwrap_err();
    assert!(
        err.contains(
            "the alphabet `lib::ab` is exported differently by library `r1` and by library `r3`"
        ),
        "{err}"
    );
    let err = link_with(&dir, &[&app], &["r3", "r1"]).unwrap_err();
    assert!(
        err.contains(
            "the alphabet `lib::ab` is exported differently by library `r3` and by library `r1`"
        ),
        "{err}"
    );
}
