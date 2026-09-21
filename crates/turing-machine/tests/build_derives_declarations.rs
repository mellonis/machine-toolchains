//! `tmt build` derives declarations from a target's sibling sources and
//! declared libraries (docs/tmt/project.md (Declaration derivation)),
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

/// The header is the declaration source when BOTH `<name>.tmh` and
/// `<name>.tmo` exist, at the BUILD level: `glib::mark`'s full graph body
/// exists only on the header — if the object's (graph-less) declarations
/// were preferred instead, the graft would be unreachable and the build
/// would fail with `undefined-graph`, exactly as the object-alone
/// negative control above does. Both files sit in the same directory
/// here; the routine-signature case (`find_library_for_build_returns_
/// both_when_both_files_exist`, `crates/turing-machine/src/cli/build.rs`)
/// cannot discriminate this preference by itself, since a plain
/// signature is content BOTH arms can carry identically.
///
/// Mutation, applied by hand and verified, then reverted: in
/// `find_library_for_build` (`crates/turing-machine/src/cli/build.rs`),
/// swapping the header/object preference so `has_tmo` is tried FIRST
/// when both exist — the object's declarations then win, `glib::mark`'s
/// body never reaches the table, and this build fails with
/// `undefined-graph` instead of succeeding.
#[test]
fn a_library_header_wins_over_its_object_when_both_exist() {
    let dir = scratch("library_header_wins_over_object");
    write(&dir, "app.tmc", GRAFTS_GLIB_MARK);
    write(&dir, "tmt.json", TMT_JSON_LIBGLIB);
    std::fs::create_dir_all(dir.join("libs")).unwrap();
    let glib_src = write(&dir, "glib.tmc", GLIB);
    let header_out = execute(&args(&[
        "interface",
        glib_src.to_str().unwrap(),
        "-o",
        dir.join("libs/glib.tmh").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("interface glib.tmc: {e}"));
    assert_eq!(header_out.code, 0, "{}", header_out.stderr);
    let object_out = execute(&args(&[
        "compile",
        glib_src.to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("libs/glib.tmo").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile glib.tmc: {e}"));
    assert_eq!(object_out.code, 0, "{}", object_out.stderr);
    assert!(dir.join("libs/glib.tmh").exists());
    assert!(dir.join("libs/glib.tmo").exists());

    let built = build_in(&dir, &[]);
    assert!(
        built.status.success(),
        "the header must win over the object beside it: {}",
        String::from_utf8_lossy(&built.stderr)
    );
}

/// A header-only library declaring a ROUTINE (not a graph) `app.tmc`
/// calls transparently — no object exists anywhere on the search path.
const LIBCALL: &str = "\
namespace libcall {
  export alphabet ab { '_', '0', '1' }

  export routine touch(tape t: ab writes { '0' }) {
    entry state s { [*] -> write ['0'] return; }
  }
}
";

const CALLS_LIBCALL_TOUCH: &str = "\
alphabet ab { '_', '0', '1' }

machine {
  tape d: ab;
  entry state go { [*] -> call libcall::touch() then done; }
  state done { [*] -> halt; }
}
";

const TMT_JSON_LIBCALL: &str = "\
{ \"project\": { \"targets\": { \"app\": {
    \"sources\": [\"app.tmc\"],
    \"libraries\": { \"dirs\": [\"libs\"], \"link\": [\"libcall\"] }
} } } }
";

/// A header-only library's object is never handed to the linker
/// (docs/tmt/project.md (Declaration derivation)): `app.tmc` calls
/// `libcall::touch`, whose declarations reach the compiler from the
/// header alone (no "declarations were not given" — the compile stage
/// succeeds), but no `.tmo` exists anywhere on the search path, so the
/// LINK stage must fail with `unresolved symbols` naming
/// `libcall::touch` specifically — proof that nothing was actually
/// linked to satisfy the call, not merely that the build "still
/// succeeds" (an assertion a redundant or unrelated object handed to the
/// linker would not move, since the earlier `--keep-objects`-free
/// `-v` version of this test showed: `LinkReport` carries no per-object
/// list, and a duplicate/irrelevant object reaching the linker changes
/// neither the exit code nor the rendered warnings).
///
/// Mutation, applied by hand and verified, then reverted: replacing the
/// `if let Some(obj) = object { libraries.push(obj); }` guard in
/// `build_one_target`/`argv_mode` with an unconditional push of a
/// zeroed, empty placeholder `ObjectFile` (arch `0`) when `object` is
/// `None` — the shape of forgetting the guard, with the simplest
/// placeholder available. The linker's own arch-consistency check
/// (`resolve()`, `crates/core/src/linker/resolve.rs`) then refuses the
/// WHOLE link with `architecture mismatch`, not `unresolved symbols:
/// libcall::touch` — this exact assertion goes RED. A same-arch
/// placeholder that still does not define `libcall::touch` would still
/// be caught (a different symbol name resolves nothing), and one
/// fabricated specifically TO define it would require compiling the
/// header back into an object — the second converter this design
/// forbids — so it is not a realistic accidental mistake to guard
/// against here.
#[test]
fn a_header_only_library_is_never_handed_to_the_linker() {
    let dir = scratch("header_only_never_linked");
    write(&dir, "app.tmc", CALLS_LIBCALL_TOUCH);
    write(&dir, "tmt.json", TMT_JSON_LIBCALL);
    std::fs::create_dir_all(dir.join("libs")).unwrap();
    let lib_src = write(&dir, "libcall.tmc", LIBCALL);
    let header = dir.join("libs/libcall.tmh");
    let out = execute(&args(&[
        "interface",
        lib_src.to_str().unwrap(),
        "-o",
        header.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("interface libcall.tmc: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !dir.join("libs/libcall.tmo").exists(),
        "precondition: nothing on disk could have been linked for this library"
    );

    let built = build_in(&dir, &[]);
    assert!(
        !built.status.success(),
        "the call must reach link with nothing to satisfy it"
    );
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(
        !stderr.contains("declarations were not given"),
        "the header's own declarations must have reached the compile stage: {stderr}"
    );
    assert!(
        stderr.contains("unresolved symbols") && stderr.contains("libcall::touch"),
        "{stderr}"
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

// ── libraries: one depends on another ────────────────────────────────────

const PRODUCER: &str = "\
namespace producer {
  export alphabet bits { '_', '0', '1' }
}
";

/// `consumer`'s own header (built below with `tmt interface --extern`)
/// keeps a `use producer::bits;` line — the printed header is a
/// text file that ITSELF still depends on `producer`'s declarations
/// to be read back, exactly like a hand-written one would.
const CONSUMER: &str = "\
namespace consumer {
  use producer::bits;

  export routine widen(tape n: bits writes { '0' }) {
    entry state s { [*] -> write ['0'] return; }
  }
}
";

const TRIVIAL_APP: &str = "\
alphabet ab { '_', 'a' }

machine {
  tape t: ab;
  entry state s { [*] -> stop; }
}
";

/// Builds `producer.tmh` (plain) and `consumer.tmh` (via `tmt interface
/// --extern`, so its own `use producer::bits;` line survives the
/// round-trip) into `dir/libs`, and a trivial `app.tmc` + manifest
/// naming both libraries in `order`.
fn write_producer_consumer_fixture(dir: &Path, order: [&str; 2]) {
    write(dir, "app.tmc", TRIVIAL_APP);
    std::fs::create_dir_all(dir.join("libs")).unwrap();
    let producer_src = write(dir, "producer.tmc", PRODUCER);
    let producer_header = dir.join("libs/producer.tmh");
    let out = execute(&args(&[
        "interface",
        producer_src.to_str().unwrap(),
        "-o",
        producer_header.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("interface producer.tmc: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);

    let consumer_src = write(dir, "consumer.tmc", CONSUMER);
    let out = execute(&args(&[
        "interface",
        consumer_src.to_str().unwrap(),
        "--extern",
        producer_header.to_str().unwrap(),
        "-o",
        dir.join("libs/consumer.tmh").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("interface consumer.tmc: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);

    write(
        dir,
        "tmt.json",
        &format!(
            "{{ \"project\": {{ \"targets\": {{ \"app\": {{
                \"sources\": [\"app.tmc\"],
                \"libraries\": {{ \"dirs\": [\"libs\"], \"link\": [\"{}\", \"{}\"] }}
            }} }} }} }}",
            order[0], order[1]
        ),
    );
}

/// A library header that itself depends on ANOTHER library's declarations
/// (the shape `tmt interface --extern` produces): `consumer.tmh` carries
/// a `use producer::bits;` line, so reading it clean needs `producer`'s
/// declarations in hand — the same shared fixpoint every OTHER
/// declaration source in the build goes through, closing this dependency
/// regardless of `-l` order.
///
/// Mutation: reading a library's own `.tmh` against the embedded stdlib
/// alone (this task's own pre-fix shape) — `consumer.tmh`'s `use
/// producer::bits;` would then be unresolvable regardless of order, and
/// BOTH variants below would fail with "declarations were not given".
#[test]
fn a_library_header_depends_on_another_library_producer_first() {
    let dir = scratch("lib_depends_on_lib_producer_first");
    write_producer_consumer_fixture(&dir, ["producer", "consumer"]);

    let out = build_in(&dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The other `-l` order — the ORIGINAL finding (a library header
/// depending on another library) reproduced with `producer` declared
/// AFTER `consumer`: order must not matter for READABILITY (only for
/// final first-match precedence, irrelevant here since the two libraries
/// declare disjoint names).
#[test]
fn a_library_header_depends_on_another_library_consumer_first() {
    let dir = scratch("lib_depends_on_lib_consumer_first");
    write_producer_consumer_fixture(&dir, ["consumer", "producer"]);

    let out = build_in(&dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── a sibling dependency chain of depth 3, listed worst-first ───────────

const CHAIN_A: &str = "\
namespace a {
  export alphabet aa { '_', '0', '1' }

  export routine leaf(tape t: aa writes { '0' }) {
    entry state s { [*] -> write ['0'] return; }
  }
}
";

/// Needs `a`'s declarations for both its tape's alphabet (`a::aa`) and
/// its own write-contract check (`a::leaf`'s declared `writes { '0' }`,
/// believed transparently).
const CHAIN_B: &str = "\
namespace b {
  export routine mid(tape t: a::aa writes { '0' }) {
    entry state s { [*] -> call a::leaf() then done; }
    state done { [*] -> return; }
  }
}
";

/// Needs `b`'s declarations the same way `b` needs `a`'s — one level
/// removed — plus its own machine entry, calling `c::top` by a named
/// (in-unit) binding.
const CHAIN_C: &str = "\
namespace c {
  export routine top(tape t: a::aa writes { '0' }) {
    entry state s { [*] -> call b::mid() then done; }
    state done { [*] -> return; }
  }
}

machine {
  tape t: a::aa;
  entry state go { [*] -> call c::top(t = t) then done; }
  state done { [*] -> halt; }
}
";

/// A three-deep sibling chain (C needs B needs A), sources declared in
/// the WORST possible order — the dependent listed before each of its
/// dependencies, in turn. Two passes close a depth-2 chain (B needs A
/// alone) but leave C unresolved after pass 2, since C's own
/// declarations depend on B's, which pass 2 has only JUST supplied;
/// closing C needs a third attempt at C specifically — which is exactly
/// what "iterate until no pass makes progress" gives for free and a
/// fixed pass count does not.
///
/// Mutation: capping the fixpoint at two passes (this task's own
/// discarded first design) — verified by hand: `header::
/// resolve_declarations`'s loop, capped at 2 iterations, makes this
/// EXACT build fail with `c.tmc`'s own `writes-outside-contract` (C never
/// gets B's declarations in time), then reverted.
#[test]
fn a_depth_three_sibling_chain_in_worst_order_still_resolves() {
    let dir = scratch("depth_three_worst_order");
    write(&dir, "c.tmc", CHAIN_C);
    write(&dir, "b.tmc", CHAIN_B);
    write(&dir, "a.tmc", CHAIN_A);
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "targets": { "app": {
            "sources": ["c.tmc", "b.tmc", "a.tmc"]
        } } } }"#,
    );

    let out = build_in(&dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── a sibling broken at expansion still supplies its declarations ───────

/// `lib::pick` is genuinely usable (needs only resolution-stage
/// declarations); `lib::bad`'s fold goes negative only once EXPANDED —
/// declarations-only reading never expands, so `lib.tmc`'s own
/// extraction still succeeds despite `bad` being broken.
const EXPANSION_BROKEN_LIB: &str = "\
namespace lib {
  export alphabet a6 { 0..5 }

  export routine pick(tape t: a6, state hit, state miss) {
    entry state s { [0]  -> goto hit;
                    [*]  -> goto miss; }
  }

  export routine bad(tape t: a6) {
    entry state s { [0..5 as v] -> write [{(v-1)%6}] return; }
  }
}
";

const CALLS_LIB_PICK_OVER_A6: &str = "\
use lib::a6;

machine {
  tape d: a6;
  entry state go { [*] -> call lib::pick(t = d, hit = won, miss = lost) then done; }
  state won  { [*] -> write [0] stop; }
  state lost { [*] -> write [1] stop; }
  state done { [*] -> halt; }
}
";

/// A sibling whose declarations-only read succeeds (`lib::pick`'s
/// signature and `lib::a6`'s alphabet both resolve cleanly) but whose
/// REAL compile fails at EXPANSION (`lib::bad`'s fold goes negative) must
/// still supply its declarations to a dependent — `app.tmc`, listed
/// FIRST, must itself actually COMPILE (not merely avoid one particular
/// error message: `lib.tmc`'s own real compile fails with the SAME
/// `negative-remainder` text whether declarations-only reading expanded
/// it or not, so asserting on `stderr` content alone cannot tell the two
/// apart — see the mutation note). `--keep-objects` makes the compiled
/// unit observable directly: `app.tmo` existing on disk is proof
/// `app.tmc` was compiled — reached, actually run through the compiler
/// — before the build failed at `lib.tmc`'s own later turn.
///
/// Mutation, applied by hand and verified, then reverted: making
/// `header::read_declarations_with_mode` also call `expand::expand` (the
/// shape declarations-only reading must NOT take). `lib.tmc`'s
/// declarations-only read then fails too — the identical
/// `negative-remainder` text `stderr` already shows from the real
/// compile, so that assertion alone stays green — but decision "a source
/// unresolved after the fixpoint is reported before compiling any
/// dependent" then bails out of the WHOLE build before compiling
/// anything at all: `app.tmo` is never written.
#[test]
fn a_sibling_broken_at_expansion_still_supplies_declarations() {
    let dir = scratch("expansion_broken_sibling");
    write(&dir, "app.tmc", CALLS_LIB_PICK_OVER_A6);
    write(&dir, "lib.tmc", EXPANSION_BROKEN_LIB);
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "targets": { "app": {
            "sources": ["app.tmc", "lib.tmc"]
        } } } }"#,
    );

    let out = build_in(&dir, &["--keep-objects"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("negative-remainder"), "{stderr}");
    assert!(stderr.contains("lib.tmc"), "{stderr}");
    assert!(
        dir.join("app.tmo").exists(),
        "app.tmc must have been compiled before lib.tmc's own later turn failed"
    );
}

// ── the root cause is shown, not a symptom ───────────────────────────────

/// The same shape as `CHAIN_B`->`CHAIN_A`'s own dependency but with a
/// PARSE error (a missing `;`) instead of a resolvable one — genuinely
/// unreadable regardless of context.
const UNPARSEABLE_LIB: &str = "\
namespace lib {
  export alphabet a6 { 0..5 }
  export routine pick(tape t: a6, state hit, state miss) {
    entry state s { [0]  -> goto hit
                    [*]  -> goto miss; }
  }
}
";

/// A sibling with a genuine PARSE error must be named in the build's own
/// error, regardless of where it sits in the declared source list — a
/// dependent listed BEFORE it (`app.tmc`, which itself cannot read
/// without `lib`'s declarations either) must not steal the report with
/// its own derived "declarations were not given" complaint.
///
/// Mutation: reporting the FIRST source the fixpoint left unresolved, in
/// declared order, rather than preferring a non-"declarations were not
/// given"-shaped failure (`looks_like_a_missing_declarations_error`,
/// `cli/driver.rs`) — verified by hand: with `app.tmc` listed first, the
/// error becomes `app.tmc`'s own "declarations were not given" for
/// `lib::a6` instead of `lib.tmc`'s own parse error, and `lib.tmc` is
/// never mentioned at all.
#[test]
fn a_sibling_parse_error_is_the_root_cause_shown_dependent_listed_first() {
    let dir = scratch("root_cause_dependent_first");
    write(&dir, "app.tmc", CALLS_LIB_PICK_OVER_A6);
    write(&dir, "lib.tmc", UNPARSEABLE_LIB);
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "targets": { "app": {
            "sources": ["app.tmc", "lib.tmc"]
        } } } }"#,
    );

    let out = build_in(&dir, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("lib.tmc") && stderr.contains("unexpected-token"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("declarations were not given"),
        "the derived failure must not be what's reported: {stderr}"
    );
}

/// The other declared order, for the identical fixture: `lib.tmc` listed
/// first already names itself correctly under the OLD (order-dependent)
/// design too — kept as the positive control this file's own mutation
/// note above describes.
#[test]
fn a_sibling_parse_error_is_the_root_cause_shown_dependency_listed_first() {
    let dir = scratch("root_cause_dependency_first");
    write(&dir, "app.tmc", CALLS_LIB_PICK_OVER_A6);
    write(&dir, "lib.tmc", UNPARSEABLE_LIB);
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "targets": { "app": {
            "sources": ["lib.tmc", "app.tmc"]
        } } } }"#,
    );

    let out = build_in(&dir, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("lib.tmc") && stderr.contains("unexpected-token"),
        "{stderr}"
    );
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

/// `stdlib: false` must ALSO reach a LIBRARY's own declarations read, not
/// only a sibling's: `sl.tmh` declares a routine whose tape is
/// `std::binaryNumbers::symbols` — under `stdlib: false` that name is
/// external and unresolvable from WITHIN the library header's own
/// reading, the same "declarations were not given" refusal. Without
/// `stdlib: false`, the identical header resolves clean (the header
/// itself was generated with the real stdlib present, via `tmt
/// interface`, which is unrelated to what `tmt build` is later given).
#[test]
fn stdlib_false_reaches_a_library_headers_own_read() {
    let dir = scratch("stdlib_false_library");
    write(&dir, "app.tmc", TRIVIAL_APP);
    std::fs::create_dir_all(dir.join("libs")).unwrap();
    let sl_src = write(
        &dir,
        "sl.tmc",
        "\
namespace sl {
  export routine touch(tape n: std::binaryNumbers::symbols writes { '^' }) {
    entry state s { [*] -> return; }
  }
}
",
    );
    let out = execute(&args(&[
        "interface",
        sl_src.to_str().unwrap(),
        "-o",
        dir.join("libs/sl.tmh").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("interface sl.tmc: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
    write(
        &dir,
        "tmt.json",
        r#"{ "project": { "stdlib": false, "targets": { "app": {
            "sources": ["app.tmc"],
            "libraries": { "dirs": ["libs"], "link": ["sl"] }
        } } } }"#,
    );

    let built = build_in(&dir, &[]);
    assert!(
        !built.status.success(),
        "stdlib: false must reach the library's own read"
    );
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(stderr.contains("declarations were not given"), "{stderr}");
    assert!(stderr.contains("sl.tmh"), "{stderr}");
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
