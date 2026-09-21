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

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::{ObjectFile, SymbolDef};
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{LinkError, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{assemble, link};
use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::{CompileOptions, Declarations, compile};
use mtc_turing_machine::stdlib;

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

/// [`compile_extern`]'s `-S` twin: the generated assembly TEXT, for tests
/// that need to tell two spliced instances of the same mangled graph name
/// apart by their own emitted code shape rather than by isolating bytes
/// inside one shared blob.
fn compile_extern_text(dir: &Path, name: &str, src: &str, header: &Path) -> String {
    let input = write_file(dir, name, src);
    let out = dir.join(format!("{name}.tma"));
    let result = execute(&args(&[
        "compile",
        input.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-S",
        "-o",
        out.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile -S {name}: {e}"));
    assert_eq!(result.code, 0, "{}: {}", name, result.stderr);
    std::fs::read_to_string(&out).unwrap()
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

/// An exporting object's own `Interface::graphs` digest for `name`.
fn graph_digest_of(object: &ObjectFile, name: &str) -> u32 {
    object
        .interface
        .as_ref()
        .expect("the library carries an interface section")
        .graphs
        .iter()
        .find(|g| g.name == name)
        .unwrap_or_else(|| panic!("no exported digest for {name} in {:?}", object.interface))
        .digest
}

/// A consuming object's own `ObjectFile::grafts` digest for `name` — the
/// body it spliced.
fn graft_digest_of(object: &ObjectFile, name: &str) -> u32 {
    object
        .grafts
        .iter()
        .find(|g| g.graph == name)
        .unwrap_or_else(|| panic!("no graft record for {name} in {:?}", object.grafts))
        .digest
}

/// Link `objects` (`--nostdlib`-shaped: no library search path) and, on
/// success, run the executable on ONE seeded tape (index-coded cells —
/// `cells` must discriminate, never a blank run) and return the resulting
/// snapshot.
fn link_and_run(objects: Vec<ObjectFile>, width: u32, cells: Vec<u8>) -> (Outcome, TapeSnapshot) {
    let exe = link(&objects, &[], LinkOptions::default())
        .unwrap_or_else(|e| panic!("link: {e}"))
        .executable;
    run_seeded(&exe, width, cells)
}

/// Run an already-linked executable on ONE seeded tape (index-coded cells —
/// `cells` must discriminate, never a blank run) and return the resulting
/// snapshot. [`link_and_run`]'s own second half, factored out so a program
/// linked by the CLI (`tmt build`, read back from disk) can be run the same
/// way as one linked in-process.
fn run_seeded(exe: &Executable, width: u32, cells: Vec<u8>) -> (Outcome, TapeSnapshot) {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
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
/// unsupported` must still refuse when read from a header. `other` +
/// `bad` exist only so a CONSUMER can bind `walker`'s `t` parameter
/// through a map declared over the WRONG target alphabet — `bad`'s own
/// declaration is legal (its two alphabets are equal-size and its pairs
/// identity-complete injectively), the mismatch is only ever at a BINDING
/// SITE naming it against a parameter typed `marks`, not `other`.
const LIB_TMC: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  export alphabet other { '_', 'y', 'x' }
  export map flip: marks -> marks { 'x' -> 'y', 'y' -> 'x' }
  export map bad: marks -> other { 'x' -> 'y', 'y' -> 'x' }

  ? walks the tape once
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

/// The SAME shape as [`CONSUMER_WALKER`], except `lib::marks` is declared
/// LOCALLY (a permuted glyph order) instead of imported — a consumer may
/// legally mix its own local declarations into a namespace it otherwise
/// only imports from. The HOST tape names the SAME mangled alphabet
/// (`lib::marks`), so it correctly reads the CONSUMER's own declaration —
/// but `walker`'s OWN parameter, over the SAME mangled name, correctly
/// resolves in the LIBRARY's own scope instead. The two are now genuinely
/// DIFFERENT alphabets sharing one name, so the omitted graft map
/// (identity, which needs glyph-for-glyph equal tapes) must be refused —
/// the collision is diagnosed, never silently captured either way.
const CONSUMER_WALKER_COLLIDING_ALPHABET: &str = "\
namespace lib {
  alphabet marks { '_', 'y', 'x' }
}
use lib::walker;
machine {
  tape t: lib::marks;
  entry graft walker(t = t, done = fin) as walk;
  state fin { [*] -> stop; }
}
";

/// An UNRELATED local `lib::marks` (wider than the library's own 3-symbol
/// alphabet) alongside a graft whose host tape is over the library's real
/// shape under a DIFFERENT local name (`h`) — an omitted map needs
/// glyph-for-glyph equal tapes, which `h` and the library's `marks`
/// genuinely are; only a consumer-scope-first lookup of `walker`'s OWN
/// `t: marks` parameter could ever disagree.
const CONSUMER_WALKER_WIDER_COLLIDING_ALPHABET: &str = "\
namespace lib {
  alphabet marks { '_', 'x', 'y', 'z' }
}
alphabet h { '_', 'x', 'y' }
use lib::walker;
machine {
  tape t: h;
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

/// [`CONSUMER_WALKER`]'s own graft, but inside a ROUTINE (`facade`) rather
/// than directly in the machine — the byte-identity claim pinned for a
/// SECOND world besides `main`.
const CONSUMER_WALKER_IN_ROUTINE: &str = "\
use lib::marks;
use lib::walker;
routine facade(tape t: marks) {
  entry graft walker(t = t, done = return) as walk;
}
machine {
  tape t: marks;
  entry state s { [*] -> call facade(t = t) then done; }
  state done { [*] -> stop; }
}
";

/// [`CONSUMER_WALKER_IN_ROUTINE`], with `walker` defined and grafted
/// LOCALLY — the in-unit control for the second-world byte-identity claim.
const CONSUMER_WALKER_IN_ROUTINE_LOCAL: &str = "\
alphabet marks { '_', 'x', 'y' }
graph walker(tape t: marks, state done) {
  entry state w {
    ['x'] -> write ['y'] goto done;
    [*]   -> goto done;
  }
}
routine facade(tape t: marks) {
  entry graft walker(t = t, done = return) as walk;
}
machine {
  tape t: marks;
  entry state s { [*] -> call facade(t = t) then done; }
  state done { [*] -> stop; }
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

/// Binds `walker`'s `t` parameter (alphabet `lib::marks`) through `lib::bad`
/// — declared `marks -> other` — so the map's declared TARGET does not
/// match the callee parameter's alphabet. `named-map-target-mismatch`
/// in-unit; this is the header-grafted shape.
const CONSUMER_MISMATCHED_MAP: &str = "\
use lib::marks;
use lib::walker;
use lib::bad;
machine {
  tape t: marks;
  entry graft walker(t = t with map bad, done = fin) as walk;
  state fin { [*] -> stop; }
}
";

/// A consumer's own `namespace lib { graph walker … }` — a DIFFERENT body
/// (unconditional move-left, no write) than the library's own `walker`
/// (`['x'] -> write ['y'] goto done; [*] -> goto done;`) — grafted
/// DIRECTLY, alongside the library's own `relay` (whose body nests a graft
/// of the library's OWN `walker` `with map flip`). Both graft targets
/// share the identical mangled name `lib::walker`, resolved from TWO
/// DIFFERENT owning modules — this unit's own for the direct graft,
/// the library's own for `relay`'s nested one. The direct graft is
/// declared FIRST in source order, so it is the first of the two
/// expanded.
const CONSUMER_MEMO_COLLISION_LOCAL_FIRST: &str = "\
namespace lib {
  graph walker(tape t: marks, state done) {
    entry state w { [*] -> move [<] goto done; }
  }
}
use lib::marks;
use lib::relay;
machine {
  tape t1: marks;
  tape t2: marks;
  entry state start { [*, *] -> goto walk; }
  graft lib::walker(t = t1, done = after_walk) as walk;
  state after_walk { [*, *] -> goto rel; }
  graft relay(a = t2, done = after_rel) as rel;
  state after_rel { [*, *] -> stop; }
}
";

/// The identical declarations as [`CONSUMER_MEMO_COLLISION_LOCAL_FIRST`],
/// with the two grafts in the OPPOSITE source order (`relay` first) — the
/// other direction a shared, name-only expansion cache can get wrong,
/// since which body wins depends on which graft is expanded first.
const CONSUMER_MEMO_COLLISION_LIBRARY_FIRST: &str = "\
namespace lib {
  graph walker(tape t: marks, state done) {
    entry state w { [*] -> move [<] goto done; }
  }
}
use lib::marks;
use lib::relay;
machine {
  tape t1: marks;
  tape t2: marks;
  entry state start { [*, *] -> goto rel; }
  graft relay(a = t2, done = after_rel) as rel;
  state after_rel { [*, *] -> goto walk; }
  graft lib::walker(t = t1, done = after_walk) as walk;
  state after_walk { [*, *] -> stop; }
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

/// Grafts the embedded standard library's OWN exported graph
/// `std::binaryNumbers::goToNumberGraph` — a plain `entry graft`, no
/// `--extern`/`--nostdlib` needed, since the stdlib's declarations are the
/// default. `goToNumberGraph` walks right until the first `'$'`, leaving
/// the tape unchanged (`std.tmc`'s own body: `['$'] -> done; [*] -> move
/// [>] goto walk;`).
const STDLIB_GRAFT_CONSUMER: &str = "\
use std::binaryNumbers::symbols;
use std::binaryNumbers::goToNumberGraph;
machine {
  tape num: symbols;
  entry graft goToNumberGraph(num = num, done = fin) as walk;
  state fin { [*] -> stop; }
}
";

/// `symbols { '_', '^', '$', '0', '1' }` seeded as a bracketed one-digit
/// number `\"^1$\"`, head at 0 — `goToNumberGraph` must walk the head to
/// index 2 (the `'$'`) and leave every cell unchanged.
const STDLIB_GRAFT_SEED: [u8; 3] = [1, 4, 2]; // '^', '1', '$'

/// A HAND-WRITTEN header (never generated by `tmt interface`) — a
/// documented capability (`docs/tmt/cli.md (--extern)`) — whose exported
/// graph's own body grafts a QUALIFIED stdlib graph by name. Reading this
/// header always believes the embedded stdlib (`header::read_extern`'s own
/// rule), so the nested target resolves fine AT READ TIME regardless of
/// what the PRIMARY compile's own `--nostdlib`/`--extern` set turns out to
/// be — the gap this fixture exercises is downstream, at splice time.
const NESTED_STDLIB_HEADER: &str = "\
namespace lib3 {
  export alphabet symbols { '_', '^', '$', '0', '1' }
  export graph wrap(tape num: symbols, state done) {
    entry graft std::binaryNumbers::goToNumberGraph(num = num, done = done) as inner;
  }
}
";

const NESTED_STDLIB_CONSUMER: &str = "\
use lib3::symbols;
use lib3::wrap;
machine {
  tape num: symbols;
  entry graft wrap(num = num, done = fin) as w;
  state fin { [*] -> stop; }
}
";

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

    // The same claim for a SECOND world besides `main`: the identical
    // graft, spliced into a ROUTINE instead of the machine block.
    let header_routine = compile_extern(
        &dir,
        "header_routine_app",
        CONSUMER_WALKER_IN_ROUTINE,
        &header,
    );
    let local_routine = compile_alone(CONSUMER_WALKER_IN_ROUTINE_LOCAL);
    assert_eq!(
        blob_of(&header_routine, "facade"),
        blob_of(&local_routine, "facade"),
        "the header-grafted routine's code differs from the in-unit splice"
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

    assert_eq!(
        graph_digest_of(&lib, "lib::walker"),
        graft_digest_of(&consumer, "lib::walker"),
        "the exporter's and the consumer's digests for lib::walker disagree"
    );
}

/// `walker` carries a `?` doc line (`"? walks the tape once"`). Rewriting
/// that doc line to a different, longer sentence must not move the
/// exported digest, the consumer's spliced digest, or whether the two
/// still link clean — the digest is signature-and-body only, doc lines
/// excluded (`header::graph_body_lines`), the same insensitivity already
/// proven for comments and whitespace. **Mutation:** folding the `?` doc
/// lines into `header::graph_digest`'s own hashed text (`graph_lines`
/// instead of `graph_body_lines`) — this test goes RED, since it is the
/// one fixture in this file that actually edits a doc line.
#[test]
fn a_doc_line_edit_does_not_move_the_digest() {
    let dir = scratch("lib_graft_doc_line");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let baseline_consumer = compile_extern(&dir, "baseline_app", CONSUMER_WALKER, &header);
    let baseline_lib = compile_alone(LIB_TMC);
    let baseline_digest = graph_digest_of(&baseline_lib, "lib::walker");
    assert_eq!(
        graft_digest_of(&baseline_consumer, "lib::walker"),
        baseline_digest
    );

    let edited = LIB_TMC.replace(
        "? walks the tape once",
        "? a longer, differently worded description of the same walk over the tape",
    );
    assert_ne!(edited, LIB_TMC, "the doc-line replacement did not fire");
    let edited_src = write_file(&dir, "lib2.tmc", &edited);
    let edited_header = interface(&dir, &edited_src, "lib2.tmh");
    let edited_consumer = compile_extern(&dir, "edited_app", CONSUMER_WALKER, &edited_header);
    let edited_lib = compile_alone(&edited);

    assert_eq!(
        graph_digest_of(&edited_lib, "lib::walker"),
        baseline_digest,
        "a doc-line-only edit moved the exporter's own digest"
    );
    assert_eq!(
        graft_digest_of(&edited_consumer, "lib::walker"),
        baseline_digest,
        "a doc-line-only edit moved the consumer's spliced digest"
    );

    // Not just two equal numbers: linking the ORIGINAL consumer against
    // the EDITED library's object must still succeed — a real GraftDrift
    // check passing across an actual doc-only edit.
    link(
        &[baseline_consumer, edited_lib],
        &[],
        LinkOptions::default(),
    )
    .unwrap_or_else(|e| panic!("a doc-only edit must not trip GraftDrift: {e}"));
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

/// A program grafting one of the embedded standard library's OWN exported
/// graphs compiles through `tmt compile` — the CLI subcommand specifically,
/// not just the `compile()` library function, since that is exactly where
/// the declarations table `tmt compile` builds for the implicit stdlib
/// disagreed with every other producer of one. Links against the compiled
/// stdlib object and runs on a seeded tape. **Mutation:** `cli/build.rs::
/// read_externals` pushing `stdlib::resolved()` (unstamped: no graph
/// digests) instead of `Declarations::push_stdlib()`'s `stdlib::header()` —
/// the graft then panics inside `compiler::compile` instead of compiling.
#[test]
fn a_stdlib_graph_grafts_through_tmt_compile() {
    let dir = scratch("stdlib_graft_cli_compile");
    let input = write_file(&dir, "app.tmc", STDLIB_GRAFT_CONSUMER);
    let out = dir.join("app.tmo");
    let result = execute(&args(&[
        "compile",
        input.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile app: {e}"));
    assert_eq!(result.code, 0, "{}", result.stderr);
    let consumer = ObjectFile::from_bytes(&std::fs::read(&out).unwrap()).unwrap();

    let (outcome, snap) = link_and_run(
        vec![consumer, stdlib::object().clone()],
        5,
        STDLIB_GRAFT_SEED.to_vec(),
    );
    assert_eq!(outcome, Outcome::Stopped, "{outcome:?}");
    assert_eq!(
        snap.head, 2,
        "goToNumberGraph did not walk the head to the '$': {snap:?}"
    );
    assert_eq!(
        snap.cells,
        STDLIB_GRAFT_SEED.to_vec(),
        "goToNumberGraph must leave the tape unchanged: {snap:?}"
    );
}

/// The same graft, through `tmt build` — argv mode compiles AND links (with
/// the embedded stdlib) in one call, so this exercises the OTHER CLI entry
/// point end to end: it already used `Declarations::stdlib()` before this
/// fix and must keep doing so. **Mutation:** the same `read_externals`
/// reversion above; `tmt build` would then disagree with `tmt compile`
/// about whether the feature exists at all.
#[test]
fn a_stdlib_graph_grafts_through_tmt_build() {
    let dir = scratch("stdlib_graft_cli_build");
    let input = write_file(&dir, "app.tmc", STDLIB_GRAFT_CONSUMER);
    let out = dir.join("app.tmx");
    let result = execute(&args(&[
        "build",
        input.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("build app: {e}"));
    assert_eq!(result.code, 0, "{}", result.stderr);
    let exe = Executable::from_bytes(&std::fs::read(&out).unwrap()).expect("valid executable");

    let (outcome, snap) = run_seeded(&exe, 5, STDLIB_GRAFT_SEED.to_vec());
    assert_eq!(outcome, Outcome::Stopped, "{outcome:?}");
    assert_eq!(
        snap.head, 2,
        "goToNumberGraph did not walk the head to the '$': {snap:?}"
    );
}

/// A `with map NAME` binding into a header-grafted library graph is
/// checked against that graph's KNOWN parameter alphabet exactly as an
/// in-unit target already is — `named-map-target-mismatch`, not a silent
/// accept. **Near miss:** the matching map (`CONSUMER_RELAY`'s own `with
/// map flip`, declared `marks -> marks`, matching `walker`'s `t: marks`)
/// compiles. **Mutation:** `expand_named_maps` hardcoding `external:
/// false` for every graft — the mismatch is then accepted at rc 0 and the
/// emitted code differs from the correctly-bound shape.
#[test]
fn a_mismatched_with_map_is_refused_on_a_header_grafted_target() {
    let dir = scratch("lib_graft_map_mismatch");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");

    let err = compile_extern_err(&dir, "mismatch_app", CONSUMER_MISMATCHED_MAP, &header);
    assert!(err.contains("[named-map-target-mismatch]"), "{err}");

    // Near miss: the matching map compiles.
    let _ = compile_extern(&dir, "matching_map_app", CONSUMER_RELAY, &header);
}

/// A consumer's own LOCAL declaration of `lib::marks` (the SAME mangled
/// name the library exports, a permuted glyph order) must not be
/// silently captured into a header-grafted library graph's own tape
/// frame — every name inside a spliced foreign body resolves in the
/// scope of the unit that DECLARED it, never the consumer's. Here the
/// host's own tape and `walker`'s own parameter share one mangled name
/// but now resolve in two DIFFERENT scopes (the consumer's own
/// declaration for the host, the library's for the graph), so they are
/// genuinely different alphabets and the omitted (identity) graft map is
/// correctly refused — a clean diagnostic, never a silent re-index.
/// **Near miss:** the identical graft against the library's REAL
/// `marks`, reached by import rather than a colliding local declaration,
/// compiles. **Mutation:** resolving a spliced graph's own tape alphabet
/// against the consumer's `resolved.alphabets` before the declaring
/// module's — the two tables then coincide (both read the consumer's own
/// permuted declaration) and this compiles cleanly instead of being
/// refused.
#[test]
fn a_colliding_local_alphabet_is_diagnosed_not_silently_captured() {
    let dir = scratch("lib_graft_name_capture");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");

    let err = compile_extern_err(
        &dir,
        "colliding_app",
        CONSUMER_WALKER_COLLIDING_ALPHABET,
        &header,
    );
    assert!(err.contains("[identity-glyph-mismatch]"), "{err}");

    // Near miss: the same graft against the library's real `marks`
    // (imported, not locally re-declared) compiles.
    let _ = compile_extern(&dir, "control_app", CONSUMER_WALKER, &header);
}

/// An UNRELATED local `lib::marks` declaration (wider than the library's
/// own alphabet) must not refuse an otherwise-valid graft whose host tape
/// is over a DIFFERENT local alphabet that genuinely IS glyph-for-glyph
/// equal to the library's own. **Mutation:** the same consumer-scope-first
/// lookup — this compiles under the fix, and is wrongly refused with
/// `identity-glyph-mismatch` under the mutation.
#[test]
fn an_unrelated_colliding_local_alphabet_does_not_refuse_a_valid_graft() {
    let dir = scratch("lib_graft_name_capture_wider");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");

    let _ = compile_extern(
        &dir,
        "wider_app",
        CONSUMER_WALKER_WIDER_COLLIDING_ALPHABET,
        &header,
    );
}

/// A `.graph`/`.grafted`-bearing `-S` text reassembles to a byte-identical
/// object (docs/formats.md (routine interfaces)) — for one EXPORTER
/// (`LIB_TMC`, whose object carries `.graph` lines for its exported
/// graphs) and one CONSUMER (a header-grafted program, whose object
/// carries a `.grafted` line). **Mutation:** any `.graph`/`.grafted`
/// spelling the assembler cannot read back identically — the reassembled
/// object would then disagree with the one `compile()`/`tmt compile`
/// itself produced.
#[test]
fn graph_and_grafted_lines_reassemble_byte_identically() {
    // A NON-exported alphabet: `Interface.alphabets` (which lists only
    // EXPORTED alphabets, filled in post-assembly since an alphabet has no
    // directive of its own) then stays empty either way, so it cannot
    // confound this test with the separate, already-documented
    // text-expressibility exception for exported/imported alphabets
    // (docs/formats.md (routine interfaces)) — the one thing a `.graph`/
    // `.grafted`-bearing object CAN legitimately fail to reassemble.
    const REASSEMBLE_LIB: &str = "\
namespace lib {
  alphabet marks { '_', 'x', 'y' }
  export graph walker(tape t: marks, state done) {
    entry state w {
      ['x'] -> write ['y'] goto done;
      [*]   -> goto done;
    }
  }
}
";
    // A LOCAL alphabet, never `use lib::marks;`: an imported alphabet is
    // the SAME kind of no-directive exception as an exported one
    // (docs/formats.md (routine interfaces)), so avoiding it here too (an
    // omitted graft map needs only glyph-for-glyph equality, not identical
    // provenance) keeps this test on the one property it targets.
    const REASSEMBLE_CONSUMER: &str = "\
alphabet localMarks { '_', 'x', 'y' }
use lib::walker;
machine {
  tape t: localMarks;
  entry graft walker(t = t, done = fin) as walk;
  state fin { [*] -> stop; }
}
";
    let dir = scratch("lib_graft_reassemble");

    // Exporter half.
    let lib_out = compile(
        REASSEMBLE_LIB,
        CompileOptions {
            externals: Declarations::none(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile lib: {e}"));
    assert!(lib_out.tma.contains(".graph "), "{}", lib_out.tma);
    let reassembled_lib =
        assemble(&lib_out.tma, false).unwrap_or_else(|e| panic!("reassemble lib: {e}"));
    assert_eq!(reassembled_lib, lib_out.object);

    // Consumer half, through the CLI (needs `--extern`).
    let lib_src = write_file(&dir, "lib.tmc", REASSEMBLE_LIB);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let input = write_file(&dir, "app.tmc", REASSEMBLE_CONSUMER);

    let tma_path = dir.join("app.tma");
    let result = execute(&args(&[
        "compile",
        input.to_str().unwrap(),
        "--nostdlib",
        "--extern",
        header.to_str().unwrap(),
        "-S",
        "-o",
        tma_path.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile -S app: {e}"));
    assert_eq!(result.code, 0, "{}", result.stderr);
    let tma_text = std::fs::read_to_string(&tma_path).unwrap();
    assert!(tma_text.contains(".grafted "), "{tma_text}");
    let reassembled_app =
        assemble(&tma_text, false).unwrap_or_else(|e| panic!("reassemble app: {e}"));

    let app_obj = compile_extern(&dir, "app_obj", REASSEMBLE_CONSUMER, &header);
    assert_eq!(reassembled_app, app_obj);
}

/// A consumer's own local graph must not leak into a LIBRARY graph's own
/// nested graft of the identical mangled name, and the object's `.grafted`
/// record must keep naming the library's REAL body — the direct graft
/// declared FIRST in source order (the shape a bare-name expansion cache
/// gets wrong: the library's `relay`, expanded second, would otherwise
/// reuse the consumer's already-memoized `lib::walker`). **Mutation:**
/// dropping the owning module from the expansion memo's key (bare name
/// only) — both this test and its sibling below go RED.
#[test]
fn a_consumers_own_graph_does_not_leak_into_a_librarys_nested_graft() {
    let dir = scratch("lib_graft_memo_local_first");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");

    let text = compile_extern_text(
        &dir,
        "memo3_text",
        CONSUMER_MEMO_COLLISION_LOCAL_FIRST,
        &header,
    );
    // `walk`'s own instance: the CONSUMER's own body — an unconditional
    // keep-and-move-left on tape 0, the other tape untouched.
    assert!(
        text.contains("wrmv    [-, -], [<, .]"),
        "the direct graft did not use the consumer's own body:\n{text}"
    );
    // `rel`'s own nested instance: the LIBRARY's own body — a real
    // conditional dispatch (`rd`/`mtc`/`djmp`), never a bare move.
    assert!(
        text.contains("rd\n        mtc     T0\n        djmp    D0"),
        "relay's own nested walker did not use the library's own dispatch:\n{text}"
    );

    let consumer = compile_extern(
        &dir,
        "memo3_obj",
        CONSUMER_MEMO_COLLISION_LOCAL_FIRST,
        &header,
    );
    let lib = compile_alone(LIB_TMC);
    assert_eq!(
        consumer.grafts.len(),
        1,
        "the purely local `lib::walker` must not be recorded: {:?}",
        consumer.grafts
    );
    assert_eq!(
        graft_digest_of(&consumer, "lib::relay"),
        graph_digest_of(&lib, "lib::relay"),
        "the graft record must name the LIBRARY's own digest for relay"
    );

    // A real drift check still fires: linking against the matching library
    // succeeds, against a drifted one it is refused.
    link(&[consumer.clone(), lib], &[], LinkOptions::default())
        .unwrap_or_else(|e| panic!("the matching library must link: {e}"));

    let drifted_lib_tmc = LIB_TMC.replace(
        "['x'] -> write ['y'] goto done;\n      [*]   -> goto done;",
        "[*] -> goto done;",
    );
    assert_ne!(drifted_lib_tmc, LIB_TMC, "the replacement did not fire");
    let drifted_lib = compile_alone(&drifted_lib_tmc);
    let err = link(&[consumer, drifted_lib], &[], LinkOptions::default())
        .expect_err("a drifted library must still be refused");
    assert!(
        matches!(err, LinkError::GraftDrift { ref graph, .. } if graph == "lib::relay"),
        "{err}"
    );
}

/// The same collision, with the grafts in the OPPOSITE source order (the
/// library's `relay` expanded FIRST) — a library graph's own nested
/// reference must not leak into a consumer's PURELY LOCAL graft either.
/// **Mutation:** the same bare-name memo key as above.
#[test]
fn a_librarys_nested_graft_does_not_leak_into_a_consumers_own_graph() {
    let dir = scratch("lib_graft_memo_library_first");
    let lib_src = write_file(&dir, "lib.tmc", LIB_TMC);
    let header = interface(&dir, &lib_src, "lib.tmh");

    let text = compile_extern_text(
        &dir,
        "memo2_text",
        CONSUMER_MEMO_COLLISION_LIBRARY_FIRST,
        &header,
    );
    assert!(
        text.contains("wrmv    [-, -], [<, .]"),
        "the local graft did not keep its own body: {text}"
    );
    assert!(
        text.contains("rd\n        mtc     T0\n        djmp    D0"),
        "relay's own nested walker did not use the library's own dispatch:\n{text}"
    );

    let consumer = compile_extern(
        &dir,
        "memo2_obj",
        CONSUMER_MEMO_COLLISION_LIBRARY_FIRST,
        &header,
    );
    assert_eq!(
        consumer.grafts.len(),
        1,
        "the purely local `lib::walker` must not be recorded: {:?}",
        consumer.grafts
    );
    assert_eq!(consumer.grafts[0].graph, "lib::relay");
}

/// A declared graph's own NESTED graft target can resolve fine when its
/// header is READ (always against the embedded stdlib) and still be
/// unreachable when the primary compile actually SPLICES it, if that
/// compile's own declarations do not carry it (`--nostdlib` here). This
/// must be a clean `undefined-graph`, never a panic — a hand-written
/// header naming a target the CONSUMER cannot see is ordinary input, not
/// a compiler bug. **Mutation:** reverting the nested lookup to `.expect`
/// — a process abort (exit 101) instead of a typed fatal.
#[test]
fn a_declared_graphs_unreachable_nested_target_is_undefined_graph_not_a_panic() {
    let dir = scratch("lib_graft_nested_unreachable");
    let header = write_file(&dir, "lib3.tmh", NESTED_STDLIB_HEADER);
    let err = compile_extern_err(&dir, "nostd_app", NESTED_STDLIB_CONSUMER, &header);
    assert!(err.contains("[undefined-graph]"), "{err}");
    assert!(err.contains("declarations were not given"), "{err}");
    assert!(
        err.contains("std::binaryNumbers::goToNumberGraph"),
        "the diagnostic must name the missing graph: {err}"
    );
}

/// Near miss: the identical header and consumer, WITH the embedded
/// stdlib's declarations available (no `--nostdlib`) — the nested target
/// resolves, both the outer and the inner graft's own digests are
/// recorded, and the program links against the REAL compiled stdlib and
/// runs.
#[test]
fn a_declared_graphs_nested_target_resolves_when_available() {
    let dir = scratch("lib_graft_nested_available");
    let header = write_file(&dir, "lib3.tmh", NESTED_STDLIB_HEADER);
    let input = write_file(&dir, "std_app.tmc", NESTED_STDLIB_CONSUMER);
    let out = dir.join("std_app.tmo");
    let result = execute(&args(&[
        "compile",
        input.to_str().unwrap(),
        "--extern",
        header.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile std_app: {e}"));
    assert_eq!(result.code, 0, "{}", result.stderr);
    let consumer = ObjectFile::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(consumer.grafts.len(), 2, "{:?}", consumer.grafts);
    assert!(
        consumer
            .grafts
            .iter()
            .any(|g| g.graph == "std::binaryNumbers::goToNumberGraph"),
        "the nested target's own provenance was not recorded: {:?}",
        consumer.grafts
    );

    let (outcome, snap) = link_and_run(
        vec![consumer, stdlib::object().clone()],
        5,
        STDLIB_GRAFT_SEED.to_vec(),
    );
    assert_eq!(outcome, Outcome::Stopped, "{outcome:?}");
    assert_eq!(
        snap.head, 2,
        "the nested goToNumberGraph did not walk the head to the '$': {snap:?}"
    );
}
