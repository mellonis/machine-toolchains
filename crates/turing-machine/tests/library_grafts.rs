//! Grafting a graph from another unit's declarations table: a library
//! header carries an exported graph's FULL body (docs/tmt/language.md
//! (headers)), so a graft target reached only through `use`/a qualified
//! path resolves and splices exactly as an in-unit graph does, and the
//! object records a digest of the body it spliced against the exporting
//! unit's own digest for the same body (docs/formats.md (routine
//! interfaces)) — dormant since the wire carried the two records, live
//! here. `undefined-graph` splits into "no such graph" and "declarations
//! not given" the same way `unresolved-alphabet`/`undefined-map` already
//! do, and a call-bearing graph is refused whether its source is local or
//! read from a header.
//!
//! `Declarations`'s own constructors besides `stdlib()`/`none()` are
//! crate-private, so every assertion that needs a populated declarations
//! table goes through `mtc_turing_machine::cli::execute`'s `--extern`
//! flag — `tests/extern_declarations.rs`'s and `tests/state_params.rs`'s
//! own precedent.

use std::path::{Path, PathBuf};

use mtc_core::formats::object::{ObjectFile, SymbolDef};
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{LinkError, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::{CompileOptions, Declarations, compile};

// ── harness ──────────────────────────────────────────────────────────────

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

fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// `tmt interface SRC -o OUT` — the real printer, never a hand-written
/// stand-in: a hand-written header risks a canonical-spelling mismatch
/// that would make a digest comparison meaningless.
fn interface(dir: &Path, src: &Path, out_name: &str) -> PathBuf {
    let out = dir.join(out_name);
    let result = execute(&args(&[
        "interface",
        src.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("interface {}: {e}", src.display()));
    assert_eq!(result.code, 0, "{}", result.stderr);
    out
}

/// `tmt compile SRC --nostdlib [FLAGS] -o OUT`, in-process via `compile()` —
/// no `Declarations` beyond `none()`/`stdlib()` needed.
fn compile_alone(src: &str) -> ObjectFile {
    compile(
        src,
        CompileOptions {
            externals: Declarations::none(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile: {e}"))
    .object
}

/// `tmt compile SRC --nostdlib --extern HEADER -o OUT`, through the CLI —
/// the one route an integration test has to build a populated
/// `Declarations` table.
fn compile_extern(dir: &Path, name: &str, src: &str, header: &Path) -> ObjectFile {
    let input = write_file(dir, name, src);
    let out = dir.join(format!("{name}.tmo"));
    let result = execute(&args(&[
        "compile",
        input.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile {name}: {e}"));
    assert_eq!(result.code, 0, "{}: {}", name, result.stderr);
    ObjectFile::from_bytes(&std::fs::read(&out).unwrap()).unwrap()
}

/// `tmt compile SRC --nostdlib --extern HEADER`, expecting a compile
/// FATAL — `execute` returns `Err(String)` for one (`cli/build.rs::
/// compile`), never a non-zero `CliOutput.code`, so this asserts on the
/// OUTER `Result`, the same shape every other fatal-expecting test in
/// this crate uses (`tests/extern_declarations.rs`'s own precedent).
fn compile_extern_err(dir: &Path, name: &str, src: &str, header: &Path) -> String {
    let input = write_file(dir, name, src);
    let out = dir.join(format!("{name}.tmo"));
    execute(&args(&[
        "compile",
        input.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]))
    .expect_err(&format!("expected a compile failure for {name}"))
}

/// The blob bytes of the symbol named `name` (`main`, or a mangled
/// routine/graph name).
fn blob_of<'a>(object: &'a ObjectFile, name: &str) -> &'a [u8] {
    let symbol = object
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol named `{name}` in {:?}", object.symbols));
    match symbol.def {
        SymbolDef::Defined { blob } | SymbolDef::Local { blob } => &object.blobs[blob as usize],
        SymbolDef::External => panic!("`{name}` is external, not defined in this object"),
    }
}

/// Link `objects` (`--nostdlib`-shaped: no library search path) and, on
/// success, run the executable on ONE seeded tape (index-coded cells —
/// `cells` must discriminate, never a blank run) and return the resulting
/// snapshot.
fn link_and_run(objects: Vec<ObjectFile>, width: u32, cells: Vec<u8>) -> (Outcome, TapeSnapshot) {
    let exe = link(&objects, &[], LinkOptions::default())
        .unwrap_or_else(|e| panic!("link: {e}"))
        .executable;
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");
    let mut tape = WideTape::from_snapshot(
        &TapeSnapshot {
            origin: 0,
            cells,
            head: 0,
            alphabet: None,
        },
        width,
    )
    .expect("seed tape is in alphabet");
    let mut devices: Vec<&mut dyn Tape> = vec![&mut tape];
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(10_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    drop(devices);
    (result.outcome, tape.to_snapshot())
}

// ── fixtures ─────────────────────────────────────────────────────────────

/// `marks { '_', 'x', 'y' }` — index 0 the blank, 1 `'x'`, 2 `'y'`.
/// `walker` is the simple, call-free graph every grafting test starts
/// from: a comment inside its body is the fixture `the_graft_digest_
/// matches_the_exporters` needs to discriminate a "digest the source text"
/// mutation from the canonical-rendering digest this task requires.
/// `relay` grafts `walker` `with map flip` — the named-map-inside-a-
/// graph-body shape. `callish` calls `helper` — the shape `graft-call-
/// unsupported` must still refuse when read from a header.
const LIB_TMC: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  export map flip: marks -> marks { 'x' -> 'y', 'y' -> 'x' }

  export graph walker(tape t: marks, state done) {
    entry state w {
      // a mid-body comment: absent from the printed header, so a digest
      // over raw source text would disagree with one over the canonical
      // rendering — the printer strips it either way.
      ['x'] -> write ['y'] goto done;
      [*]   -> goto done;
    }
  }

  export graph relay(tape a: marks, state done) {
    entry graft walker(t = a with map flip, done = done) as inner;
  }

  export routine helper(tape t: marks) {
    entry state h { [*] -> return; }
  }

  export graph callish(tape t: marks, state done) {
    entry state s { [*] -> call helper(t = t) then done; }
  }
}
";

/// The consumer grafting `walker` straight from `lib`'s header.
const CONSUMER_WALKER: &str = "\
use lib::marks;
use lib::walker;
machine {
  tape t: marks;
  entry graft walker(t = t, done = fin) as walk;
  state fin { [*] -> stop; }
}
";

/// `walker`, defined and grafted LOCALLY — same shape, same binding, same
/// machine — for the central byte-identity claim.
const CONSUMER_WALKER_LOCAL: &str = "\
alphabet marks { '_', 'x', 'y' }
graph walker(tape t: marks, state done) {
  entry state w {
    ['x'] -> write ['y'] goto done;
    [*]   -> goto done;
  }
}
machine {
  tape t: marks;
  entry graft walker(t = t, done = fin) as walk;
  state fin { [*] -> stop; }
}
";

const CONSUMER_RELAY: &str = "\
use lib::marks;
use lib::relay;
machine {
  tape a: marks;
  entry graft relay(a = a, done = fin) as go;
  state fin { [*] -> stop; }
}
";

const CONSUMER_CALLISH: &str = "\
use lib::marks;
use lib::callish;
machine {
  tape t: marks;
  entry graft callish(t = t, done = fin) as x;
  state fin { [*] -> stop; }
}
";

/// A grafted, call-free graph over the SAME `callish` shape, minus the
/// call — the near miss for `graft-call-unsupported`: grafting `walker`
/// (already call-free) compiles.
const CONSUMER_WALKER_NEAR_MISS: &str = CONSUMER_WALKER;

/// row 1's positive: a BARE graft target nothing local defines and no
/// `use`/qualified path reaches — `Scopes::resolve` returns a total miss
/// (`None`), the one shape that reads *no such graph* rather than
/// *declarations not given*. A `::`-qualified `lib::g` would NOT serve
/// here: an absolute path resolves structurally regardless of whether
/// anything backs it (`compiler::Scopes::resolve`'s own doc), landing on
/// the declarations-not-given reading instead — exactly the shape
/// `tests/cross_unit.rs::an_alphabet_no_scope_declares_says_no_such_
/// alphabet` uses a bare name for, on `unresolved-alphabet`.
const UNDECLARED_LIB: &str = "\
alphabet marks { '_', 'x' }
machine {
  tape t: marks;
  entry graft ghost(t = t, done = fin) as x;
  state fin { [*] -> stop; }
}
";

/// row 1's near miss: a LOCAL graph of the same bare name `g`, grafted
/// unqualified — confirms the fix does not disturb ordinary local
/// resolution.
const LOCAL_G_NEAR_MISS: &str = "\
alphabet marks { '_', 'x' }
graph g(tape t: marks, state done) {
  entry state s { [*] -> goto done; }
}
machine {
  tape t: marks;
  entry graft g(t = t, done = fin) as x;
  state fin { [*] -> stop; }
}
";

/// row 2's positive: `use lib::g;` reaches a name nothing local declares —
/// `declarations not given` without `--extern`.
const USE_UNDECLARED_LIB: &str = "\
use lib::g;
alphabet marks { '_', 'x' }
machine {
  tape t: marks;
  entry graft g(t = t, done = fin) as x;
  state fin { [*] -> stop; }
}
";

fn undefined_graph(src: &str) -> mtc_turing_machine::CompileError {
    compile(
        src,
        CompileOptions {
            externals: Declarations::none(),
            ..Default::default()
        },
    )
    .expect_err("expected undefined-graph")
}

// ── tests ────────────────────────────────────────────────────────────────

/// A library graph found only through the declarations table grafts
/// exactly as an in-unit one: compile, link, run, assert the tape.
/// **Mutation:** resolving the graft against the in-unit map only; the
/// compile fails with *no such graph* instead of splicing.
#[test]
fn a_library_graph_grafts_from_its_header() {
    let dir = scratch("lib_graft_basic");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let consumer = compile_extern(&dir, "app", CONSUMER_WALKER, &header);
    let lib = compile_alone(LIB_TMC);

    let (outcome, snap) = link_and_run(vec![consumer, lib], 3, vec![1]); // seed: 'x'
    assert_eq!(outcome, Outcome::Stopped, "{outcome:?}");
    assert_eq!(
        snap.cells.first().copied().unwrap_or(0),
        2, // 'y' — walker's own write, spliced from the header
        "the header-grafted walker did not run: {snap:?}"
    );
}

/// The SAME graph, written locally and grafted from a header, produce
/// byte-identical CODE for the identical host machine. **Mutation:** any
/// divergence in the header path's splice. This is the task's central
/// claim.
#[test]
fn the_spliced_body_is_identical_to_the_in_unit_splice() {
    let dir = scratch("lib_graft_identical_splice");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let header_side = compile_extern(&dir, "header_app", CONSUMER_WALKER, &header);
    let local_side = compile_alone(CONSUMER_WALKER_LOCAL);

    assert_eq!(
        blob_of(&header_side, "main"),
        blob_of(&local_side, "main"),
        "the header-grafted machine's code differs from the in-unit splice"
    );
}

/// A library graph whose body uses `with map NAME`, and whose tapes are
/// over the library's own alphabet, splices from a header and runs.
/// **Mutation:** resolving the graph's own tape alphabet only against the
/// host's `resolved.alphabets`, never the declarations table — the
/// library's own `marks` alphabet is absent there for a header-only graft,
/// so this traps `expand.rs`'s local-then-external alphabet lookup.
#[test]
fn a_library_graph_grafts_with_a_named_map() {
    let dir = scratch("lib_graft_named_map");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let consumer = compile_extern(&dir, "relay_app", CONSUMER_RELAY, &header);
    let lib = compile_alone(LIB_TMC);

    let (outcome, snap) = link_and_run(vec![consumer, lib], 3, vec![2]); // seed: 'y'
    assert_eq!(outcome, Outcome::Stopped, "{outcome:?}");
    // `flip` is its own inverse (x<->y): whatever `walker` observes and
    // writes on its own side, the write-back direction applies `flip`
    // again reaching the host. Pinned from a verified run rather than
    // hand-derived from the map algebra.
    assert_eq!(
        snap.cells.first().copied().unwrap_or(0),
        1,
        "the with-map splice produced an unexpected cell: {snap:?}"
    );
}

/// The two `u32`s agree over a compiler-produced pair: the exporter's own
/// `Interface::graphs` digest for `lib::walker` and the consumer's
/// `ObjectFile::grafts` digest for the same graph, read from its header.
/// **Mutation:** digesting the source text rather than the canonical
/// rendering — `LIB_TMC`'s mid-body comment (absent from the printed
/// header) makes the two disagree under that mutation.
#[test]
fn the_graft_digest_matches_the_exporters() {
    let dir = scratch("lib_graft_digest");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let consumer = compile_extern(&dir, "digest_app", CONSUMER_WALKER, &header);
    let lib = compile_alone(LIB_TMC);

    let exported = lib
        .interface
        .as_ref()
        .expect("the library carries an interface section")
        .graphs
        .iter()
        .find(|g| g.name == "lib::walker")
        .unwrap_or_else(|| panic!("no exported digest for lib::walker in {:?}", lib.interface));
    let spliced = consumer
        .grafts
        .iter()
        .find(|g| g.graph == "lib::walker")
        .unwrap_or_else(|| panic!("no graft record for lib::walker in {:?}", consumer.grafts));
    assert_eq!(
        exported.digest, spliced.digest,
        "the exporter's and the consumer's digests for lib::walker disagree"
    );
}

/// A consumer built against a header that drifted from the library's own
/// object is refused AT LINK, as the `GraftDrift` variant specifically —
/// not just failure.
#[test]
fn a_drifted_header_is_refused_at_link() {
    let dir = scratch("lib_graft_drift");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let consumer = compile_extern(&dir, "drift_app", CONSUMER_WALKER, &header);

    // A DIFFERENT body for the SAME graph name — a different digest.
    let drifted_lib_tmc = LIB_TMC.replace(
        "['x'] -> write ['y'] goto done;\n      [*]   -> goto done;",
        "[*] -> goto done;",
    );
    assert_ne!(drifted_lib_tmc, LIB_TMC, "the replacement did not fire");
    let drifted_lib = compile_alone(&drifted_lib_tmc);

    let err = link(&[consumer, drifted_lib], &[], LinkOptions::default())
        .expect_err("a drifted header must be refused");
    assert!(
        matches!(err, LinkError::GraftDrift { ref graph, .. } if graph == "lib::walker"),
        "{err}"
    );
}

/// The matching pair (the header the consumer compiled against, and the
/// object of the library that PRODUCED that header) links clean.
#[test]
fn the_matching_pair_links() {
    let dir = scratch("lib_graft_matching");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let consumer = compile_extern(&dir, "matching_app", CONSUMER_WALKER, &header);
    let lib = compile_alone(LIB_TMC);

    link(&[consumer, lib], &[], LinkOptions::default())
        .unwrap_or_else(|e| panic!("the matching pair must link: {e}"));
}

/// A header-only library — no `.tmo` for it in the link — is NOT checked:
/// the one place a header is trusted outright (docs/formats.md (routine
/// interfaces)). **Mutation:** requiring the exporter; the link fails.
#[test]
fn a_header_only_library_is_not_checked() {
    let dir = scratch("lib_graft_header_only");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let consumer = compile_extern(&dir, "header_only_app", CONSUMER_WALKER, &header);

    link(&[consumer], &[], LinkOptions::default())
        .unwrap_or_else(|e| panic!("a header-only library must not be checked: {e}"));
}

/// `graft lib::g(...)` with no `lib` anywhere at all → *no such graph*.
/// **Near miss:** a local graph of the same bare name compiles.
#[test]
fn a_graft_target_naming_nothing_anywhere_is_no_such_graph() {
    let err = undefined_graph(UNDECLARED_LIB);
    assert_eq!(err.kind.code(), "undefined-graph");
    assert!(err.to_string().contains("unknown graph `ghost`"), "{err}");
    assert!(
        !err.to_string().contains("declarations were not given"),
        "{err}"
    );

    compile(
        LOCAL_G_NEAR_MISS,
        CompileOptions {
            externals: Declarations::none(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("a local graph of the same bare name must compile: {e}"));
}

/// `use lib::g;` with no `--extern` → *declarations not given*, naming the
/// remedy. **Near miss:** the same source, given `--extern lib.tmh`,
/// compiles (`a_library_graph_grafts_from_its_header` already proves the
/// positive shape end to end; this asserts the SAME text-shaped source
/// only fails for the missing-declarations reason, never any other).
#[test]
fn a_use_reached_graph_with_no_extern_names_its_remedy() {
    let err = undefined_graph(USE_UNDECLARED_LIB);
    assert_eq!(err.kind.code(), "undefined-graph");
    let text = err.to_string();
    assert!(text.contains("lib::g"), "{text}");
    assert!(text.contains("--extern"), "{text}");
    assert!(text.contains("declarations were not given"), "{text}");
}

/// A header graph whose body contains a `call` hits the SAME guard a
/// local one does. **Mutation:** leaving the old "awaits binding
/// composition" wording — asserted on the reworded advice text.
/// **Near miss:** the same shape minus the call (`walker`) compiles.
#[test]
fn a_call_bearing_header_graph_hits_the_same_guard() {
    let dir = scratch("lib_graft_call_unsupported");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");

    let err = compile_extern_err(&dir, "callish_app", CONSUMER_CALLISH, &header);
    assert!(err.contains("[graft-call-unsupported]"), "{err}");
    assert!(
        err.contains("write it as a routine, with `state` parameters"),
        "{err}"
    );
    assert!(
        !err.contains("awaits binding composition"),
        "the stale wording is still present: {err}"
    );

    // Near miss: the call-free graph from the SAME header compiles.
    let _ = compile_extern(&dir, "near_miss_app", CONSUMER_WALKER_NEAR_MISS, &header);
}
