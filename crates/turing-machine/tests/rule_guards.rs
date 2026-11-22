//! Rule guards (#133's `where { … }`): the typing and refusal surface —
//! cone-local contexts, one common sequence per order comparison, and the
//! four refusals, each pinned by kind AND span
//! (docs/tmt/language.md (rule guards), docs/internal/specs/…design.md).

use mtc_core::diagnostics::Span;
use mtc_turing_machine::compiler::{CompileError, CompileErrorKind, CompileOptions, compile};

fn parse_ok(src: &str) {
    compile(src, CompileOptions::default()).expect("the fixture must compile");
}

fn refusal(src: &str) -> CompileError {
    compile(src, CompileOptions::default()).expect_err("the fixture must be refused")
}

/// The 1-based `Pos` of a needle's first character — the parser battery's
/// own helper, copied for one column basis across test files.
fn at(src: &str, needle: &str) -> mtc_core::diagnostics::Pos {
    for (i, line) in src.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            return mtc_core::diagnostics::Pos {
                line: i as u32 + 1,
                col: col as u32 + 1,
            };
        }
    }
    panic!("`{needle}` not in fixture");
}

/// The half-open span covering the first occurrence of `needle`.
fn cover(src: &str, needle: &str) -> Span {
    let p = at(src, needle);
    Span::new(p.line, p.col, p.line, p.col + needle.len() as u32)
}

const HEAD: &str = "\
alphabet ab { 'a', 'b', 'c' }
set pair { 'a', 'b' }
set trio { 'a', 'b', 'c' }
set foreign { 'q', 'r' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
";

#[test]
fn one_sided_narrowing_names_both_contexts() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in pair && x < y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardOrderContext {
            left: "pair".into(),
            right: "ab".into(),
        }
    );
    // The refusal spans the COMPARISON, not the whole guard.
    assert_eq!(e.span, cover(&src, "x < y"));
    // …and its message names both contexts.
    let msg = e.kind.to_string();
    assert!(msg.contains("pair") && msg.contains("ab"), "message: {msg}");
}

#[test]
fn a_second_narrowing_agrees_only_on_identity() {
    // `x` narrowed to two different sets: identity needs no sequence and
    // stays legal…
    parse_ok(&format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in pair && x in trio && x == y }} -> stop;\n  }}\n}}\n"
    ));
    // …but a position comparison has no single sequence to live in.
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in pair && x in trio && x < y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardOrderContext {
            left: "pair".into(),
            right: "trio".into(),
        }
    );
}

#[test]
fn an_unknown_guard_name_is_refused() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ zz == y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardUnknownBinding { name: "zz".into() }
    );
    // Span at the name itself.
    assert_eq!(e.span, cover(&src, "zz"));
}

#[test]
fn an_in_off_alphabet_is_refused() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in foreign && x == y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardSetOutsideAlphabet {
            set: "foreign".into(),
            alphabet: "ab".into(),
        }
    );
    // The span is the set name AS USED in the guard (HEAD itself declares
    // the set, so the needle anchors past `x in `).
    let p = at(&src, "x in foreign");
    assert_eq!(
        e.span,
        Span::new(
            p.line,
            p.col + 5,
            p.line,
            p.col + 5 + "foreign".len() as u32
        )
    );
}

#[test]
fn a_comparison_with_no_binding_is_refused() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ 'q' == 'r' }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(e.kind, CompileErrorKind::GuardConstantComparison);
    assert_eq!(e.span, cover(&src, "'q' == 'r'"));
}

#[test]
fn a_comparison_types_by_its_own_cone() {
    // The narrowed leg compares in `pair`'s order; the bare leg (a sibling
    // `||` branch, outside the narrowing's cone) is licensed by the shared
    // alphabet instead. Both legs legal, one span of one guard.
    parse_ok(&format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ (x in pair && y in pair && x < y) || x < y }} -> stop;\n  }}\n}}\n"
    ));
}

// ---------------------------------------------------------------------------
// Evaluation and the row filter: a guard drops rows at expansion, and what
// survives is the hand-writable product (docs/tmt/language.md
// (rule guards)). Derivation-first: the guarded fixture's rows equal a
// twin written out rule by rule in the test code itself.
// ---------------------------------------------------------------------------

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{disassemble_executable_with_map, link};
use mtc_turing_machine::lint::{LintOptions, lint};

fn object(src: &str) -> mtc_core::formats::object::ObjectFile {
    compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("the fixture must compile: {e}"))
        .object
}

/// The linked image's match rows, order-preserving.
fn rows_of(src: &str) -> Vec<String> {
    let out = link(
        std::slice::from_ref(&object(src)),
        &[],
        LinkOptions::default(),
    )
    .unwrap_or_else(|e| panic!("link failed: {e}"));
    let dis = disassemble_executable_with_map(&out.executable, &out.map);
    dis.lines()
        .filter_map(|l| {
            l.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .split_once(".row ")
                .map(|(_, op)| op.to_string())
        })
        .collect()
}

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

/// Compile, link and run `src` on one seed band per tape.
fn run_on(src: &str, seeds: &[(TapeSnapshot, u32)]) -> Outcome {
    let out =
        compile(src, CompileOptions::default()).unwrap_or_else(|e| panic!("must compile: {e}"));
    assert!(
        out.report.diagnostics.is_empty(),
        "expected no diagnostics, got {:?}",
        out.report.diagnostics
    );
    let exe: Executable = link(
        std::slice::from_ref(&out.object),
        &[],
        LinkOptions::default(),
    )
    .unwrap_or_else(|e| panic!("link failed: {e}"))
    .executable;
    let mut tapes: Vec<WideTape> = seeds
        .iter()
        .map(|(s, w)| WideTape::from_snapshot(s, *w).expect("seed fits its width"))
        .collect();
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");
    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(1_000_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    result.outcome
}

/// The issue's headline over a 27-label alphabet (`_` then `a`..`z`):
/// `x == y` keeps the 26-cell diagonal, `x == y - 1` the 25-cell
/// subdiagonal — 51 of the 2×26×26 product's 676 rows.
const HEADLINE: &str = "\
alphabet ab { '_', 'a'..'z' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    ['a'..'z' as x, 'a'..'z' as y] where { x == y || x == y - 1 } -> stop;
  }
}
";

/// The same rows spelled rule by rule — the derivation the guard must
/// equal. Generated here so the expected rows are never read off a run.
const HEADLINE_WRITTEN_RULES: &str = "\
alphabet ab { '_', 'a'..'z' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
%s  }
}
";

fn headline_written() -> String {
    let glyphs: Vec<char> = ('a'..='z').collect();
    let mut rules = String::new();
    for g in &glyphs {
        rules.push_str(&format!("    ['{g}', '{g}'] -> stop;\n"));
    }
    for pair in glyphs.windows(2) {
        rules.push_str(&format!("    ['{}', '{}'] -> stop;\n", pair[0], pair[1]));
    }
    HEADLINE_WRITTEN_RULES.replace("%s", &rules)
}

#[test]
fn a_guard_keeps_exactly_the_hand_written_product() {
    let rows = rows_of(HEADLINE);
    assert_eq!(
        rows,
        rows_of(&headline_written()),
        "the guard's rows are the hand-written diagonal + subdiagonal"
    );
    assert_eq!(rows.len(), 51, "26 diagonal + 25 subdiagonal, out of 676");
}

/// A pair fixture whose set order CONTRADICTS the alphabet's: `qq`'s
/// order is `z, a`, the alphabet's `a, z`. The narrowed leg of the guard
/// compares in `qq`'s order, the bare leg on the alphabet; runs witness
/// both orders distinguishably.
const CONTRAST: &str = "\
alphabet ab { '_', 'a', 'z' }
set qq { 'z', 'a' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    ['a'..'z' as x, 'a'..'z' as y] where { x in qq && y in qq && x < y || x == y } -> stop;
  }
}
";

#[test]
fn a_runs_witness_the_set_order_versus_the_alphabet_s() {
    // Tape t carries the first cell, tape u the second; the alphabet has
    // 3 symbols (`_`=0, `a`=1, `z`=2) and the set `qq` orders them z < a.
    let pair = |x: u8, y: u8| vec![(snap(0, &[x], 0), 3), (snap(0, &[y], 0), 3)];
    // The narrowed leg: `x < y` is SET order, so (z, a) — the pair the
    // alphabet would REVERSE — stops…
    assert_eq!(
        run_on(CONTRAST, &pair(2, 1)),
        Outcome::Stopped,
        "the set's order keeps (z, a)"
    );
    // …and the identity leg keeps equals, never (a, z).
    assert_eq!(
        run_on(CONTRAST, &pair(1, 1)),
        Outcome::Stopped,
        "the identity leg keeps (a, a)"
    );
    assert_eq!(
        run_on(CONTRAST, &pair(2, 2)),
        Outcome::Stopped,
        "the identity leg keeps (z, z)"
    );
}

#[test]
fn a_mismatched_pair_is_rejected_by_the_guard() {
    // t='a', u='z': neither leg (identity false; set-order `x < y` false
    // because set order puts z first) — no row, so the state has no
    // transition and the machine traps.
    let outcome = run_on(CONTRAST, &[(snap(0, &[1], 0), 3), (snap(0, &[2], 0), 3)]);
    assert!(
        matches!(outcome, Outcome::Trapped(_)),
        "no leg keeps (a, z): {outcome:?}"
    );
}

/// A routine body's guarded rule folds to the rows of its hand-written
/// twin, exactly as the machine's does.
const ROUTINE_GUARDED: &str = "\
alphabet ab { '_', 'a'..'z' }

routine one(tape n: ab) {
  entry state s {
    ['a'..'z' as x] where { x == 'q' } -> return;
    ['a'..'z' as x] where { x == 'q' - 1 } -> return;
  }
}

machine {
  tape t: ab;
  entry state go {
    ['q'] -> call one(n = t) then fin;
    [*] -> stop;
  }
  state fin { [*] -> stop; }
}
";

const ROUTINE_WRITTEN: &str = "\
alphabet ab { '_', 'a'..'z' }

routine one(tape n: ab) {
  entry state s {
    ['q'] -> return;
    ['p'] -> return;
  }
}

machine {
  tape t: ab;
  entry state go {
    ['q'] -> call one(n = t) then fin;
    [*] -> stop;
  }
  state fin { [*] -> stop; }
}
";

#[test]
fn a_routine_body_folds_to_the_same_rows() {
    assert_eq!(rows_of(ROUTINE_GUARDED), rows_of(ROUTINE_WRITTEN));
}

#[test]
fn a_guard_that_keeps_no_rows_warns_empty_expansion() {
    let out = compile(
        "\
alphabet ab { '_', 'a'..'z' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    ['a'..'z' as x, 'a'..'z' as y] where { x == y && x != y } -> stop;
  }
}
",
        CompileOptions::default(),
    )
    .expect("compiles");
    let codes: Vec<&str> = out.report.diagnostics.iter().map(|d| d.code).collect();
    assert!(
        codes.contains(&"empty-expansion"),
        "a guard keeping nothing warns: {codes:?}"
    );
}

#[test]
fn the_product_threshold_counts_before_the_filter() {
    // 25×25 = 625 rows of product; the guard keeps 25. The cost warning
    // still fires — a guard does not make expansion cheaper.
    let src = "\
alphabet big { 0 .. 24 }

machine {
  tape a: big;
  tape b: big;
  entry state s {
    [0..24 as x, 0..24 as y] where { x == y } -> stop;
    [*, *] -> stop;
  }
}
";
    let out = compile(src, CompileOptions::default()).expect("compiles");
    let codes: Vec<&str> = out.report.diagnostics.iter().map(|d| d.code).collect();
    assert!(
        codes.contains(&"expansion-threshold"),
        "pre-filter product is the cost: {codes:?}"
    );
    let products = lint(src, LintOptions::default())
        .expect("lints")
        .diagnostics
        .into_iter()
        .filter(|d| d.code == "binding-product-threshold")
        .count();
    assert_eq!(products, 1, "the lint recounts the same pre-filter product");
}

// ---------------------------------------------------------------------------
// Cross-unit truth: a header carries the guard and the digest covers its
// sets, so a grafted guard filters in GRAPH space and drift stays loud
// (docs/tmt/language.md (rule guards), docs/formats.md (routine
// interfaces)).
// ---------------------------------------------------------------------------

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mtc_core::formats::object::{ObjectFile, SymbolDef};
use mtc_core::linker::LinkError;
use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::Declarations;

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter (the repo's collision-free
/// temp-path rule for parallel test runs).
fn scratch(name: &str) -> ScratchDir {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    ScratchDir::fresh(dir)
}

/// A scratch directory this test owns; `Drop` removes it again once the
/// test ends, passing or panicking.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn fresh(dir: PathBuf) -> Self {
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
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
/// stand-in.
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

/// `tmt interface SRC` to stdout — the header text itself.
fn interface_stdout(dir: &Path, name: &str, src: &str) -> String {
    let input = write_file(dir, name, src);
    let result = execute(&args(&["interface", input.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("interface {name}: {e}"));
    assert_eq!(result.code, 0, "{name}: {}", result.stderr);
    result.stdout
}

/// `tmt fmt --check HEADER` — the generated `.tmh` is already canonical,
/// its bytes the fixed point of the printer (docs/tmt/cli.md (interface)).
fn fmt_check(header: &Path) {
    let out = execute(&args(&["fmt", "--check", header.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("fmt --check {}: {e}", header.display()));
    assert_eq!(
        out.code, 0,
        "the generated header is not fmt-canonical: {}{}",
        out.stdout, out.stderr
    );
}

/// `tmt compile SRC --nostdlib --extern HEADER -o OUT`, through the CLI.
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

/// The blob bytes of the symbol named `name`.
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

/// Link `objects` and run on one seeded band per tape.
fn run_objects(objects: Vec<ObjectFile>, seeds: &[(TapeSnapshot, u32)]) -> Outcome {
    let exe = link(&objects, &[], LinkOptions::default())
        .unwrap_or_else(|e| panic!("link: {e}"))
        .executable;
    let mut tapes: Vec<WideTape> = seeds
        .iter()
        .map(|(s, w)| WideTape::from_snapshot(s, *w).expect("seed fits its width"))
        .collect();
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");
    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
    machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(1_000_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok")
        .outcome
}

/// The guarded library: `x < y` counts positions in ITS OWN `ab` (order
/// `_ < a < z`), so the only row its guard keeps is (`a`, `z`). The twin
/// is that row written out — the derivation, not a run's output.
const GUARDED_LIB: &str = "\
namespace lib {
  export alphabet ab { '_', 'a', 'z' }

  export graph pick(tape p: ab, tape q: ab, state done) {
    entry state s {
      ['a'..'z' as x, 'a'..'z' as y] where { x < y } -> goto done;
    }
  }
}
";

const TWIN_LIB: &str = "\
namespace lib {
  export alphabet ab { '_', 'a', 'z' }

  export graph pick(tape p: ab, tape q: ab, state done) {
    entry state s {
      ['a', 'z'] -> goto done;
    }
  }
}
";

/// The host: its own `hb` (`_ < p < q`) and an order-NON-preserving glyph
/// map at each binding (`'p' -> 'z'`, `'q' -> 'a'` reverses `ab`'s order),
/// so the surviving graph row (`a`, `z`) lands on the host as the
/// DESCENDING pair (q, p) — rows a host-side `x < y` reading over `hb`
/// could never produce. Filter-then-map, never host-space.
const HOST_PICK: &str = "\
alphabet hb { '_', 'p', 'q' }
use lib::pick;
machine {
  tape t: hb;
  tape u: hb;
  entry graft pick(
    p = t with map { '_' -> '_', 'p' -> 'z', 'q' -> 'a' },
    q = u with map { '_' -> '_', 'p' -> 'z', 'q' -> 'a' },
    done = fin
  ) as inst;
  state fin { [*, *] -> stop; }
}
";

/// A library whose set is named only by the guard's `in` — the one name
/// the digest's named-content appendix must see on the guard's account.
const DRIFT_LIB: &str = "\
namespace lib {
  export alphabet ab { '_', 'a', 'z' }
  export set qq { 'z', 'a' }

  export graph pick(tape p: ab, tape q: ab, state done) {
    entry state s {
      ['a'..'z' as x, 'a'..'z' as y] where { x in qq && y in qq && x < y } -> goto done;
    }
  }
}
";

const HOST_QQ: &str = "\
use lib::ab;
use lib::pick;
machine {
  tape t: ab;
  tape u: ab;
  entry graft pick(p = t, q = u, done = fin) as inst;
  state fin { [*, *] -> stop; }
}
";

/// A raw guard spelling per test below; every expected string is DERIVED
/// here in comments, never read off a run.
const CANON_PIPE: &str = "\
namespace twin {
  export alphabet ab { '_', 'a', 'z' }

  export graph pick(tape p: ab, tape q: ab, state done) {
    entry state s {
      ['a'..'z' as x, 'a'..'z' as y] where { (x == y) || (x == 'z') } -> goto done;
    }
  }
}
";

const CANON_PAREN: &str = "\
namespace twin {
  export alphabet ab { '_', 'a', 'z' }
  export set qq { 'z', 'a' }

  export graph pick(tape p: ab, tape q: ab, state done) {
    entry state s {
      ['a'..'z' as x, 'a'..'z' as y] where { x in qq && (x == y || x == 'z') } -> goto done;
    }
  }
}
";

/// The same graph, spelled with named vector cells — `name:` is pure
/// spelling, so both sources print ONE header (docs/tmt/language.md
/// (named vector cells)).
const TWIN_NAMED: &str = "\
namespace twin {
  export alphabet ab { '_', 'a', 'z' }

  export graph pick(tape t: ab, tape u: ab, state done) {
    entry state s {
      [t: 'a'..'z' as x, u: 'a'..'z' as y] where { x == y || x == y - 1 } -> goto done;
    }
  }
}
";

const TWIN_POSITIONAL: &str = "\
namespace twin {
  export alphabet ab { '_', 'a', 'z' }

  export graph pick(tape t: ab, tape u: ab, state done) {
    entry state s {
      ['a'..'z' as x, 'a'..'z' as y] where { x == y || x == y - 1 } -> goto done;
    }
  }
}
";

/// The task's central claim: a header-grafted guarded graph keeps exactly
/// the rows its guard keeps in GRAPH space, mapped onto the host — the
/// emitted `main` equals the same host grafting the hand-written twin.
/// **Mutation:** the header dropping the guard — the twin keeps one row,
/// the guarded graft keeps four, and the blobs part.
#[test]
fn a_guarded_graph_body_splices_graph_space() {
    let dir = scratch("guard_graph_space");
    let lib_src = write_file(&dir, "lib.tmc", GUARDED_LIB);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let twin_src = write_file(&dir, "twin.tmc", TWIN_LIB);
    let twin_header = interface(&dir, &twin_src, "twin.tmh");

    let guarded = compile_extern(&dir, "guarded_app", HOST_PICK, &header);
    let twin = compile_extern(&dir, "twin_app", HOST_PICK, &twin_header);
    assert_eq!(
        blob_of(&guarded, "main"),
        blob_of(&twin, "main"),
        "the guard filtered anywhere but the graph's own row space"
    );

    // The surviving row lands as the host's DESCENDING pair (q, p)…
    let lib = compile_alone(GUARDED_LIB);
    assert_eq!(
        run_objects(
            vec![guarded, lib],
            &[(snap(0, &[2], 0), 3), (snap(0, &[1], 0), 3)]
        ),
        Outcome::Stopped,
        "the spliced survivor is the host pair (q, p)"
    );
    // …and the host-ascending pair (p, q) — what a host-side `x < y`
    // reading over `hb` would have kept — has no row and traps.
    let lib2 = compile_alone(GUARDED_LIB);
    let guarded2 = compile_extern(&dir, "guarded_app2", HOST_PICK, &header);
    assert!(matches!(
        run_objects(
            vec![guarded2, lib2],
            &[(snap(0, &[1], 0), 3), (snap(0, &[2], 0), 3)]
        ),
        Outcome::Trapped(_),
    ));
}

/// Named and positional spellings of one guarded rule print one header,
/// and the header text really carries the guard — the twin comparison
/// alone also holds for two headers printing nothing at all.
#[test]
fn a_twin_header_is_still_one_header() {
    let dir = scratch("guard_twin_header");
    let named = interface(
        &dir,
        &write_file(&dir, "named.tmc", TWIN_NAMED),
        "named.tmh",
    );
    let positional = interface(
        &dir,
        &write_file(&dir, "positional.tmc", TWIN_POSITIONAL),
        "positional.tmh",
    );
    let named_text = std::fs::read_to_string(&named).unwrap();
    let positional_text = std::fs::read_to_string(&positional).unwrap();
    assert_eq!(named_text, positional_text);
    assert!(
        positional_text.contains(
            "['a'..'z' as x, 'a'..'z' as y] where { x == y || x == y - 1 } -> goto done;"
        ),
        "the header lost the guard (or the pattern): {positional_text}"
    );
}

/// A guard's `in S` set drifts between two units' declarations: the
/// digest appendix must see it, or `GraftDrift` goes quiet — which is
/// exactly what it must not do. The matching pair links (control).
/// **Mutation:** dropping the appendix's guard scan — the drift goes
/// silent and this test red on `expect_err`.
#[test]
fn guard_set_drift_is_still_graft_drift() {
    let dir = scratch("guard_drift");
    let lib_src = write_file(&dir, "lib.tmc", DRIFT_LIB);
    let header = interface(&dir, &lib_src, "lib.tmh");
    let consumer = compile_extern(&dir, "drift_app", HOST_QQ, &header);

    // A DIFFERENT member set under the SAME name — the guard's rows change
    // with it, and the appendix is the only thing naming those members.
    let drifted = DRIFT_LIB.replace("set qq { 'z', 'a' }", "set qq { '_', 'a' }");
    assert_ne!(drifted, DRIFT_LIB, "the replacement did not fire");
    let drifted_lib = compile_alone(&drifted);

    let err = link(&[consumer, drifted_lib], &[], LinkOptions::default())
        .expect_err("a drift in a guard's `in` set must be refused");
    assert!(
        matches!(err, LinkError::GraftDrift { ref graph, .. } if graph == "lib::pick"),
        "{err}"
    );

    let consumer2 = compile_extern(&dir, "match_app", HOST_QQ, &header);
    let lib = compile_alone(DRIFT_LIB);
    link(&[consumer2, lib], &[], LinkOptions::default())
        .unwrap_or_else(|e| panic!("the matching pair must link: {e}"));
}

/// Two truths must not split: `tmt interface` prints the guard a source
/// wrote, and the generated `.tmh` is its own `tmt fmt` fixed point and
/// reads back byte-identically through `tmt interface`.
#[test]
fn an_unfiltered_guard_never_reaches_a_header() {
    let dir = scratch("guard_header_roundtrip");
    let src = write_file(&dir, "lib.tmc", CANON_PAREN);
    let header = interface(&dir, &src, "lib.tmh");
    let text = std::fs::read_to_string(&header).unwrap();
    assert!(
        text.contains("where { x in qq && (x == y || x == 'z') } -> goto done;"),
        "the header dropped the guard: {text}"
    );
    fmt_check(&header);
    let again = interface(&dir, &header, "again.tmh");
    assert_eq!(
        std::fs::read_to_string(&again).unwrap(),
        text,
        "a generated header does not read back to itself"
    );
}

/// `guard_text` is one canonical spelling per tree: written parens are
/// dropped, semantic parens reinserted (a column under `||`), names and
/// literals as written, offsets spelled out.
#[test]
fn guard_text_canon_is_one_spelling() {
    let dir = scratch("guard_canon");
    // Written parens around top-level `||` operands vanish: the tree is
    // Or(Cmp, Cmp), printed bare.
    let pipes = interface_stdout(&dir, "pipes.tmc", CANON_PIPE);
    assert!(
        pipes.contains("where { x == y || x == 'z' } -> goto done;"),
        "{pipes}"
    );
    // A `||` column UNDER `&&` keeps its parentheses on the way out —
    // dropping them would re-parse into a different cone.
    let parens = interface_stdout(&dir, "parens.tmc", CANON_PAREN);
    assert!(
        parens.contains("where { x in qq && (x == y || x == 'z') } -> goto done;"),
        "{parens}"
    );
    // The headline shape: offsets spelled `y - 1`, no parens invented at
    // the top of an `||`.
    let head = interface_stdout(&dir, "head.tmc", TWIN_POSITIONAL);
    assert!(
        head.contains("where { x == y || x == y - 1 } -> goto done;"),
        "{head}"
    );
}
