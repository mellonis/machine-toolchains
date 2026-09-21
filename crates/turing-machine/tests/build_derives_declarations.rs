//! `tmt build` derives declarations from a target's sibling sources and
//! declared libraries (docs/tmt/project.md (declared source set)),
//! instead of every unit believing only the embedded standard library the
//! way it did before this file's tests were added. Manifest-mode fixtures
//! spawn the real `tmt` binary with `current_dir` set to the fixture
//! directory — discovery starts at the process cwd, so an in-process
//! `execute` call would race every other test in the same process
//! (`tests/build_driver.rs`'s own `tmt()` precedent, restated here since
//! this crate has no shared test-support module).

use std::path::{Path, PathBuf};
use std::process::Command;

use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter — copied verbatim from
/// `tests/mode_equivalence.rs::scratch`.
fn scratch(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    path
}

/// The real `tmt` binary, spawned as a subprocess — manifest mode's
/// discovery starts at the process cwd (`tests/build_driver.rs`'s `tmt()`
/// precedent).
fn tmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tmt"))
}

fn build_in(dir: &Path, extra: &[&str]) -> std::process::Output {
    tmt()
        .arg("build")
        .args(extra)
        .current_dir(dir)
        .output()
        .unwrap()
}

// ── sibling sources ─────────────────────────────────────────────────────

/// A library sibling exporting a routine with two `state` parameters —
/// `pick`'s callers can leave only through `hit`/`miss`, which the
/// compiler can lower only when it knows `pick`'s own parameter order
/// (docs/tmt/language.md (routines)).
const LIB_PICK: &str = "\
namespace lib {
  alphabet ab { '_', '0', '1' }

  export routine pick(tape t: ab, state hit, state miss) {
    entry state s {
      ['_'] -> goto hit;
      [*]   -> goto miss;
    }
  }
}
";

/// Calls `lib::pick` with explicit `state` bindings — a shape that needs
/// `pick`'s declared parameter ORDER, not merely its name, to lower at
/// all (`StateArgsNeedDeclarations` otherwise).
const APP_CALLS_LIB_PICK: &str = "\
alphabet ab { '_', '0', '1' }

machine {
  tape d: ab;
  entry state go { [*] -> call lib::pick(t = d, hit = won, miss = lost) then done; }
  state won  { [*] -> write ['0'] stop; }
  state lost { [*] -> write ['1'] stop; }
  state done { [*] -> halt; }
}
";

/// Two `.tmc` in one target, one calling the other with a `state`-param
/// binding: builds only when `app.tmc` is compiled knowing `lib.tmc`'s
/// declarations. Mutation: dropping the sibling pass entirely — the
/// negative control below (`app.tmc` compiled ALONE, `lib.tmc` absent
/// from its declarations) is exactly that mutation's effect, and it fails
/// with the *declarations were not given* message
/// (`state-args-need-declarations`) at the exact call site the manifest
/// build compiles clean.
#[test]
fn a_sibling_source_supplies_declarations() {
    let dir = scratch("sibling_supplies");
    write(&dir, "lib.tmc", LIB_PICK);
    write(&dir, "app.tmc", APP_CALLS_LIB_PICK);
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "targets": { "app": {
            "sources": ["lib.tmc", "app.tmc"]
        } } } }"#,
    );

    let out = build_in(&dir, &[]);
    assert!(
        out.status.success(),
        "sibling declarations must reach app.tmc's compile: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The mutation itself, applied directly rather than inferred: `app.tmc`
/// compiled ALONE (`tmt compile`, no `--extern`, no sibling in sight) is
/// exactly what compiling it WOULD see if the sibling pass were dropped —
/// same source, same (missing) declarations. It fails with the message
/// naming `lib::pick`'s declarations as not given.
#[test]
fn dropping_the_sibling_pass_reproduces_as_a_bare_compile_failure() {
    let dir = scratch("sibling_mutation");
    let app = write(&dir, "app.tmc", APP_CALLS_LIB_PICK);
    let out = execute(&args(&[
        "compile",
        app.to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("app.tmo").to_str().unwrap(),
    ]))
    .expect_err("lib::pick's declarations were never given to this bare compile");
    assert!(out.contains("state-args-need-declarations"), "{out}");
    assert!(out.contains("lib::pick"), "{out}");
}

// ── libraries: the object half ──────────────────────────────────────────

const BITLIB: &str = "\
namespace bitlib {
  export alphabet ab { '_', '0', '1' }

  export routine flip(tape t: ab writes { '0', '1' }) {
    entry state s { [*] -> return; }
  }
}
";

/// Calls `bitlib::flip` transparently (bare, positional binding) under a
/// contract that only holds if `flip`'s declared `writes { '0', '1' }` is
/// believed — an opaque (undeclared) callee would contribute the WHOLE
/// alphabet instead, including `'_'`, violating `caller`'s own contract.
const CALLS_BITLIB_FLIP: &str = "\
alphabet ab { '_', '0', '1' }

routine caller(tape n: ab writes { '0', '1' }) {
  entry state s { [*] -> call bitlib::flip() then done; }
  state done { [*] -> return; }
}

machine {
  tape d: ab;
  entry state go { [*] -> call caller(n = d) then done; }
  state done { [*] -> halt; }
}
";

/// A library with only `<name>.tmo` on the search path (its interface
/// section) supplies `bitlib::flip`'s declared write contract to the
/// compiler. Mutation: `find_library_for_build` falling back to the plain
/// `find_library` behavior (object bytes only, no declarations derived
/// from its interface) — the callee stays opaque, `caller`'s narrow
/// contract is violated by the whole-alphabet assumption, and this build
/// fails instead of succeeding.
#[test]
fn a_library_object_supplies_its_interface() {
    let dir = scratch("library_object");
    write(&dir, "caller.tmc", CALLS_BITLIB_FLIP);
    write(&dir, "tmt.json", TMT_JSON_LIBBITLIB);
    std::fs::create_dir_all(dir.join("libs")).unwrap();
    let bitlib_src = write(&dir, "bitlib.tmc", BITLIB);
    let out = execute(&args(&[
        "compile",
        bitlib_src.to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("libs/bitlib.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile bitlib.tmc: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);

    let built = build_in(&dir, &[]);
    assert!(
        built.status.success(),
        "the library object's own interface must supply bitlib::flip's contract: {}",
        String::from_utf8_lossy(&built.stderr)
    );
}

const TMT_JSON_LIBBITLIB: &str = "\
{ \"project\": { \"targets\": { \"app\": {
    \"sources\": [\"caller.tmc\"],
    \"libraries\": { \"dirs\": [\"libs\"], \"link\": [\"bitlib\"] }
} } } }
";

// ── libraries: the header half (and the header-only case) ──────────────

/// A graph-exporting library: `mark`'s FULL BODY exists only in source
/// form (docs/tmt/language.md (headers)) — the object arm carries no
/// graph body at all, so only a `.tmh` can supply enough for a consumer
/// to graft it.
const GLIB: &str = "\
namespace glib {
  export alphabet marks { '_', '0', '1' }

  export graph mark(tape t: marks) {
    entry state s { [*] -> write ['1'] stop; }
  }
}
";

/// Grafts `glib::mark` — a `use`-only reference to a graph whose body
/// must be spliced at compile time.
const GRAFTS_GLIB_MARK: &str = "\
use glib::marks;
use glib::mark;

machine {
  tape t: marks;
  entry graft mark(t = t) as m;
}
";

const TMT_JSON_LIBGLIB: &str = "\
{ \"project\": { \"targets\": { \"app\": {
    \"sources\": [\"app.tmc\"],
    \"libraries\": { \"dirs\": [\"libs\"], \"link\": [\"glib\"] }
} } } }
";

/// A library with only `<name>.tmh` on the search path (no `<name>.tmo`
/// at all) supplies `glib::mark`'s full graph body. Mutation: reading
/// only `.tmo` (today's `find_library`) — the graft target is
/// unreachable (no declarations at all), and this build fails with
/// `undefined-graph`'s "declarations were not given" instead of
/// succeeding. A companion negative control proves the CONVERSE: an
/// object alone (compiled from the SAME source, `.tmh` deleted) cannot
/// supply the graph — object-arm headers carry no graph bodies at all
/// (docs/tmt/cli.md (interface)).
#[test]
fn a_library_header_supplies_its_graphs() {
    let dir = scratch("library_header");
    write(&dir, "app.tmc", GRAFTS_GLIB_MARK);
    write(&dir, "tmt.json", TMT_JSON_LIBGLIB);
    std::fs::create_dir_all(dir.join("libs")).unwrap();
    let glib_src = write(&dir, "glib.tmc", GLIB);
    let header = dir.join("libs/glib.tmh");
    let out = execute(&args(&[
        "interface",
        glib_src.to_str().unwrap(),
        "-o",
        header.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("interface glib.tmc: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !dir.join("libs/glib.tmo").exists(),
        "precondition: no object exists for this header-only library"
    );

    let built = build_in(&dir, &[]);
    assert!(
        built.status.success(),
        "the header's own graph body must reach the compiler: {}",
        String::from_utf8_lossy(&built.stderr)
    );
}

/// The converse of the header half: an OBJECT alone (no `.tmh`), compiled
/// from the identical `glib.tmc`, cannot supply `mark`'s body — the
/// object arm has no graph-body field on the wire at all
/// (docs/formats.md (routine interfaces)).
#[test]
fn a_library_object_alone_cannot_supply_a_graph_body() {
    let dir = scratch("library_object_no_graph");
    write(&dir, "app.tmc", GRAFTS_GLIB_MARK);
    write(&dir, "tmt.json", TMT_JSON_LIBGLIB);
    std::fs::create_dir_all(dir.join("libs")).unwrap();
    let glib_src = write(&dir, "glib.tmc", GLIB);
    let out = execute(&args(&[
        "compile",
        glib_src.to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("libs/glib.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile glib.tmc: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);

    let built = build_in(&dir, &[]);
    assert!(
        !built.status.success(),
        "an object with no interface graph body must not satisfy the graft"
    );
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(stderr.contains("undefined-graph"), "{stderr}");
}

/// A header-only library's object is never handed to the linker
/// (docs/tmt/project.md (libraries)): the SAME fixture as
/// `a_library_header_supplies_its_graphs`, asserted from the resolver's
/// own decision point rather than a `LinkReport` field — `LinkReport`
/// carries no per-object list, so the thing that actually decides
/// whether a library's object reaches the linker is `find_library_for_
/// build`'s returned `Option<ObjectFile>`: `None` for a header-only
/// library is what keeps `build_one_target`'s own `if let Some(obj) =
/// object { libraries.push(obj); }` guard from ever adding one. Proven
/// end to end here by using `--keep-objects`-free `-v` build output and
/// confirming it succeeds with NO `.tmo` on disk for this library at
/// all — the precondition already asserted above makes plain there was
/// nothing to hand the linker; `crates/turing-machine/src/cli/build.rs`'s
/// own unit tests pin the `Option` directly.
///
/// Mutation: dropping the `has_tmo` guard in `find_library_for_build` and
/// unconditionally trying to `read_object` the `.tmo` candidate — the
/// build fails with a "cannot read" error, since no such file exists for
/// a header-only library, rather than succeeding.
#[test]
fn a_header_only_library_builds_and_is_never_linked() {
    let dir = scratch("header_only_never_linked");
    write(&dir, "app.tmc", GRAFTS_GLIB_MARK);
    write(&dir, "tmt.json", TMT_JSON_LIBGLIB);
    std::fs::create_dir_all(dir.join("libs")).unwrap();
    let glib_src = write(&dir, "glib.tmc", GLIB);
    let header = dir.join("libs/glib.tmh");
    let out = execute(&args(&[
        "interface",
        glib_src.to_str().unwrap(),
        "-o",
        header.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("interface glib.tmc: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !dir.join("libs/glib.tmo").exists(),
        "nothing on disk could have been linked for this library"
    );

    let built = build_in(&dir, &["-v"]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
}

// ── libraries: neither file ──────────────────────────────────────────────

/// Neither `<name>.tmo` nor `<name>.tmh` exists anywhere on the search
/// path: an error naming the library. Mutation: falling back to silence
/// (an empty declarations table, no object) — the build would succeed
/// past this point and then fail confusingly at LINK (undefined symbol),
/// rather than failing HERE with a message that names the library by
/// name.
#[test]
fn a_library_with_neither_is_an_error() {
    let dir = scratch("library_neither");
    write(
        &dir,
        "app.tmc",
        "\
alphabet ab { '_', '0', '1' }

machine {
  tape d: ab;
  entry state go { [*] -> stop; }
}
",
    );
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "targets": { "app": {
            "sources": ["app.tmc"],
            "libraries": { "dirs": ["libs"], "link": ["nosuch"] }
        } } } }"#,
    );
    std::fs::create_dir_all(dir.join("libs")).unwrap();

    let out = build_in(&dir, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nosuch"), "{stderr}");
}

// ── stdlib: false reaches the compile stage too ──────────────────────────

/// A `use`-qualified reference to a real embedded-stdlib alphabet: with
/// `stdlib: false`, `std::binaryNumbers::symbols` is external and
/// unresolvable — its declarations were never given, matching
/// `AlphabetMiss::DeclarationsNotGiven`'s own wording. Without `stdlib:
/// false` (the sibling test below), the identical source compiles clean.
///
/// Mutation (applied and verified by hand, then reverted): passing
/// `manifest.stdlib` to the LINK stage only, which was this driver's
/// behavior before the compile stage also read it. Under that mutation
/// `unit_declarations`'s compile-stage `stdlib` argument is effectively
/// always `true`, so this source resolves `symbols` from the
/// (always-present) embedded stdlib and the build that must FAIL here
/// succeeds instead — RED before the fix, GREEN after, which is what
/// makes this a real test rather than a restated description.
#[test]
fn stdlib_false_reaches_the_compile_stage() {
    let dir = scratch("stdlib_false_compile");
    write(
        &dir,
        "app.tmc",
        "\
use std::binaryNumbers::symbols;

machine {
  tape d: symbols;
  entry state go { [*] -> stop; }
}
",
    );
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "stdlib": false, "targets": { "app": {
            "sources": ["app.tmc"]
        } } } }"#,
    );

    let out = build_in(&dir, &[]);
    assert!(!out.status.success(), "stdlib: false must reach compile");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("declarations were not given"), "{stderr}");
}

/// The positive control for the test above: the SAME source, WITHOUT
/// `stdlib: false` (the manifest default, `stdlib: true`) — the embedded
/// standard library resolves `symbols` and the build succeeds. Proves the
/// failure above is really about the `stdlib` KEY, not a broken fixture.
#[test]
fn stdlib_true_by_default_still_resolves_the_same_source() {
    let dir = scratch("stdlib_true_compile");
    write(
        &dir,
        "app.tmc",
        "\
use std::binaryNumbers::symbols;

machine {
  tape d: symbols;
  entry state go { [*] -> stop; }
}
",
    );
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "targets": { "app": {
            "sources": ["app.tmc"]
        } } } }"#,
    );

    let out = build_in(&dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── `build` does not accept `--extern` ────────────────────────────────────

/// `--extern` belongs to `compile`; `build` derives its own declarations
/// and never accepts it, in EITHER mode — `build`'s flag parser consumes
/// every OTHER flag before `Args::positionals` runs, so an unconsumed
/// `--extern` always falls through to the SAME "unknown flag" refusal
/// `positionals` gives any dashed token neither mode recognizes. No new
/// rejection code was added for this: this is the PRE-EXISTING message,
/// recorded (tool-verified) against the unchanged binary before this
/// task's own change and asserted verbatim here.
///
/// Mutation: `build` growing its own `--extern` handling — this exact
/// invocation would then either succeed or fail with a DIFFERENT message,
/// and this assertion would catch either.
#[test]
fn build_rejects_extern() {
    let dir = scratch("build_rejects_extern");
    let app = write(
        &dir,
        "app.tmc",
        "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state s { [*] -> stop; }
}
",
    );
    let err = execute(&args(&[
        "build",
        "--extern",
        "x.tmh",
        app.to_str().unwrap(),
    ]))
    .expect_err("--extern is not a build flag in either mode");
    assert_eq!(err, "unknown flag `--extern`");
}
