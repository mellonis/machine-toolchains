//! Cross-unit alphabet resolution: a qualified alphabet reference
//! (`tape d: std::binaryNumbersBare::symbols;`, or the same shape on a
//! signature tape parameter) and the `use`-import shape that closes the
//! asymmetry `call`/`graft`/`bind` targets always had over alphabets —
//! `use lib::bits;` then a bare `tape d: bits;` now resolves `bits`
//! against the compile's own `Declarations` table exactly as an external
//! routine target does (docs/tmt/language.md (declarations)). Every
//! assertion here goes through `mtc_turing_machine::cli::execute`, the
//! same crate-private-surface workaround `tests/extern_declarations.rs`
//! and `tests/header_roundtrip.rs` already use, since `Declarations` and
//! `Resolved` are crate-private.
//!
//! The `unresolved-alphabet` code stays ONE code for two distinct
//! messages (docs/tmt/cli.md (error codes)): a name nothing declares
//! anywhere says so plainly, while a name a `use` import or a qualified
//! path reaches — but whose own unit's declarations were never given —
//! names the remedy (`--extern`, or declare it locally) instead of
//! reading as a typo. The two message-splitting tests below assert the
//! rendered TEXT, not just the code, since the split is invisible at code
//! granularity.
//!
//! The drift pair (`a_drifted_import_is_refused_at_link` /
//! `a_matching_import_links`) is what proves a compiled object's
//! `Interface::imports` record is real rather than decorative: it is the
//! one fixture that exercises the linker's `AlphabetDrift` check
//! (`crates/core/src/linker/interface.rs`), dormant since it had nothing
//! populating `Interface::imports` to compare.
//!
//! **Symbolic emission** (docs/formats.md (bound calls)): a `call`/`bind`
//! that binds tapes into a routine outside this compilation unit compiles
//! to a SYMBOLIC binding record — the callee's parameter NAME where an
//! in-unit site writes a caller-tape position, and a bound map's
//! destination as a glyph LABEL rather than an index, because the callee's
//! own tape order and index space belong to the LINKER to resolve. When the
//! callee's declarations are known (a sibling, `--extern`, or the embedded
//! standard library), the entries print in the callee's OWN tape order and
//! every parameter is checked at compile time; when they are not, the site
//! keeps its source order and every entry is named, and the checks defer to
//! the linker. An omitted map emits NO pairs — index identity, the
//! linker's `glyph-mismatch` warning stands guard — and `with map { }`
//! emits a WRITTEN empty map, which silences that warning on purpose.

use std::path::{Path, PathBuf};

use mtc_core::formats::object::{ImportedAlphabet, ObjectFile};
use mtc_turing_machine::cli::{CliOutput, execute};

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter — copied verbatim from
/// `tests/mode_equivalence.rs::scratch` (temp paths must not collide
/// across concurrent `cargo test`/`cargo nextest` processes sharing one
/// `CARGO_TARGET_TMPDIR`).
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
    std::fs::write(&path, content).unwrap();
    path
}

/// `tmt compile INPUT -o out.tmo [FLAGS]`, run in-process through the
/// public CLI entry, mirroring `tests/extern_declarations.rs::compile`.
fn compile(dir: &Path, input: &Path, out_name: &str, flags: &[&str]) -> Result<CliOutput, String> {
    let out = dir.join(out_name);
    let mut argv: Vec<String> = vec!["compile".into(), input.to_str().unwrap().into()];
    argv.extend(flags.iter().map(|s| s.to_string()));
    argv.push("-o".into());
    argv.push(out.to_str().unwrap().into());
    execute(&argv)
}

/// `tmt link OBJECT... -o out.tmx [FLAGS]`.
fn link(
    dir: &Path,
    objects: &[&Path],
    out_name: &str,
    flags: &[&str],
) -> Result<CliOutput, String> {
    let out = dir.join(out_name);
    let mut argv: Vec<String> = vec!["link".into()];
    argv.extend(objects.iter().map(|p| p.to_str().unwrap().to_string()));
    argv.extend(flags.iter().map(|s| s.to_string()));
    argv.push("-o".into());
    argv.push(out.to_str().unwrap().into());
    execute(&argv)
}

// -- the two grammar positions -----------------------------------------

const QUALIFIED_TAPE: &str = "\
namespace ns {
  export alphabet bits { '_', '0', '1' }
}

machine {
  tape d: ns::bits;
  entry state s { [*] -> stop; }
}
";

/// Mutation: accepting `::` in only the signature-parameter position (the
/// likely half-fix) — this fixture alone goes red, since it exercises the
/// MACHINE tape-declaration grammar exclusively.
#[test]
fn a_qualified_alphabet_names_a_tape() {
    let dir = scratch("qualified_tape");
    let input = write(&dir, "caller.tmc", QUALIFIED_TAPE);
    let out = compile(&dir, &input, "out.tmo", &[]);
    assert!(out.is_ok(), "{:?}", out.err());
}

const QUALIFIED_SIG_PARAM: &str = "\
namespace ns {
  export alphabet bits { '_', '0', '1' }
}

routine r(tape t: ns::bits writes { '0' }) {
  entry state s { [*] -> write ['0'] return; }
}

machine {
  tape d: ns::bits;
  entry state s { [*] -> call r(t = d) then stop; }
}
";

/// Mutation: accepting `::` only in the machine tape-declaration position
/// — `r`'s own signature parameter (`Parser::sig_param`) is what this
/// fixture pins; its `machine` block ALSO writes a qualified tape
/// declaration (needed to bind `t = d` at the `call`), so this fixture is
/// not exclusive to the signature position the way
/// `a_qualified_alphabet_names_a_tape` is exclusive to the machine one —
/// it still goes red under the mutation, on the signature parameter
/// specifically, one call frame further in.
#[test]
fn a_qualified_alphabet_names_a_signature_parameter() {
    let dir = scratch("qualified_sig_param");
    let input = write(&dir, "caller.tmc", QUALIFIED_SIG_PARAM);
    let out = compile(&dir, &input, "out.tmo", &[]);
    assert!(out.is_ok(), "{:?}", out.err());
}

// -- the four-fixture message split --------------------------------------

const LIB_HEADER: &str = "\
namespace lib {
  export alphabet bits { '_', '1', '0' }
}
";

const IMPORT_CALLER: &str = "\
use lib::bits;

machine {
  tape d: bits;
  entry state s { [*] -> stop; }
}
";

/// Mutation: collapsing the two `AlphabetMiss` arms back into one message
/// — this test and `an_alphabet_no_scope_declares_says_no_such_alphabet`
/// would then assert the same string, and one of them goes red (this one
/// requires "declarations were not given" and requires the OTHER
/// message's "unknown alphabet" phrase be ABSENT).
#[test]
fn an_imported_alphabet_without_declarations_says_so() {
    let dir = scratch("import_no_extern");
    let input = write(&dir, "caller.tmc", IMPORT_CALLER);
    let err = compile(&dir, &input, "out.tmo", &[]).unwrap_err();
    assert!(err.contains("[unresolved-alphabet]"), "{err}");
    assert!(err.contains("declarations were not given"), "{err}");
    assert!(!err.contains("unknown alphabet"), "{err}");
}

/// The near-miss half of the same pair: the identical source, with the
/// missing declarations now supplied.
#[test]
fn the_same_import_with_extern_resolves() {
    let dir = scratch("import_with_extern");
    let header = write(&dir, "lib.tmh", LIB_HEADER);
    let input = write(&dir, "caller.tmc", IMPORT_CALLER);
    let out = compile(
        &dir,
        &input,
        "out.tmo",
        &["--extern", header.to_str().unwrap()],
    );
    assert!(out.is_ok(), "{:?}", out.err());
}

const NO_SCOPE_CALLER: &str = "\
machine {
  tape d: nosuch;
  entry state s { [*] -> stop; }
}
";

/// Mutation: collapsing the two `AlphabetMiss` arms back into one message
/// — see `an_imported_alphabet_without_declarations_says_so`'s own note;
/// this test requires "unknown alphabet" and requires the OTHER message's
/// "declarations were not given" phrase be ABSENT.
#[test]
fn an_alphabet_no_scope_declares_says_no_such_alphabet() {
    let dir = scratch("no_scope");
    let input = write(&dir, "caller.tmc", NO_SCOPE_CALLER);
    let err = compile(&dir, &input, "out.tmo", &[]).unwrap_err();
    assert!(err.contains("[unresolved-alphabet]"), "{err}");
    assert!(err.contains("unknown alphabet"), "{err}");
    assert!(!err.contains("declarations were not given"), "{err}");
}

const LOCAL_SHADOWS_IMPORT_CALLER: &str = "\
use lib::bits;
alphabet bits { '_', '0', '1' }

machine {
  tape d: bits;
  entry state s { [*] -> stop; }
}
";

/// The near-miss half of the no-scope pair: a LOCAL declaration of the
/// same bare name a `use` also binds. `Scopes::resolve` walks a scope's
/// own definitions before its import bindings, so the local declaration
/// wins with no `--extern` needed — no code change this task makes is
/// exercised by this fixture; it pins the PRE-EXISTING precedence that
/// makes the split's "declared locally" remedy true.
#[test]
fn a_locally_declared_alphabet_resolves() {
    let dir = scratch("local_shadows_import");
    let input = write(&dir, "caller.tmc", LOCAL_SHADOWS_IMPORT_CALLER);
    let out = compile(&dir, &input, "out.tmo", &[]);
    assert!(out.is_ok(), "{:?}", out.err());
}

// -- the interface section -------------------------------------------------

/// Mutation: recording the imported alphabet's NAME without its glyphs
/// (or with the wrong glyphs) — `AlphabetDrift` would then have nothing
/// correct to compare against, which is exactly what
/// `a_drifted_import_is_refused_at_link` catches from the other side;
/// this test pins the record's CONTENT directly.
#[test]
fn an_imported_alphabet_is_recorded_in_the_interface_section() {
    let dir = scratch("interface_imports");
    let header = write(&dir, "lib.tmh", LIB_HEADER);
    let input = write(&dir, "caller.tmc", IMPORT_CALLER);
    let out = compile(
        &dir,
        &input,
        "out.tmo",
        &["--extern", header.to_str().unwrap()],
    );
    assert!(out.is_ok(), "{:?}", out.err());
    let bytes = std::fs::read(dir.join("out.tmo")).unwrap();
    let object = ObjectFile::from_bytes(&bytes).expect("a compiled object decodes");
    let interface = object
        .interface
        .expect("a compiled object carries an interface section");
    assert_eq!(
        interface.imports,
        vec![ImportedAlphabet {
            name: "lib::bits".to_string(),
            glyphs: vec!["_".to_string(), "1".to_string(), "0".to_string()],
        }],
        "{:?}",
        interface.imports
    );
}

// -- the drift pair, through compile --extern then link -------------------

const LIB_HEADER_MATCHING: &str = "\
namespace lib {
  export alphabet bits { '_', '1', '0' }
}
";

const LIB_HEADER_DRIFTED: &str = "\
namespace lib {
  export alphabet bits { '_', '0', '1' }
}
";

/// The library unit itself, exporting `bits` in a fixed glyph order —
/// `_, 1, 0` — that one header matches and the other does not.
const LIB_TMC: &str = "\
namespace lib {
  export alphabet bits { '_', '1', '0' }

  export routine touch(tape t: bits writes {}) {
    entry state s { [*] -> return; }
  }
}
";

/// Mutation (verified by hand — see the task report): filling
/// `Interface::imports` with the LOCAL declaration's glyphs (dropping a
/// name with no local declaration) instead of the header's — the caller
/// here declares no local `bits`, so `imports` would come out empty and
/// this refusal would wrongly go green. Asserts the specific
/// `AlphabetDrift` text, not mere failure — a `--extern`/compile mistake
/// elsewhere could also make this link fail for an unrelated reason.
#[test]
fn a_drifted_import_is_refused_at_link() {
    let dir = scratch("drift_refused");
    let lib_src = write(&dir, "lib.tmc", LIB_TMC);
    let lib_obj = compile(&dir, &lib_src, "lib.tmo", &["--nostdlib"]).expect("lib compiles");
    assert_eq!(lib_obj.code, 0);

    let drifted_header = write(&dir, "lib_drift.tmh", LIB_HEADER_DRIFTED);
    let caller_src = write(&dir, "caller.tmc", IMPORT_CALLER);
    let caller_obj = compile(
        &dir,
        &caller_src,
        "caller.tmo",
        &["--extern", drifted_header.to_str().unwrap(), "--nostdlib"],
    );
    assert!(caller_obj.is_ok(), "{:?}", caller_obj.err());

    let err = link(
        &dir,
        &[&dir.join("caller.tmo"), &dir.join("lib.tmo")],
        "out.tmx",
        &["--nostdlib"],
    )
    .unwrap_err();
    assert!(err.contains("lib::bits"), "{err}");
    assert!(err.contains("differs from"), "{err}");
}

/// The near-miss half of the drift pair: the same shapes, with the header
/// declaring `bits` in the SAME glyph order the library object exports.
#[test]
fn a_matching_import_links() {
    let dir = scratch("drift_matching");
    let lib_src = write(&dir, "lib.tmc", LIB_TMC);
    let lib_obj = compile(&dir, &lib_src, "lib.tmo", &["--nostdlib"]).expect("lib compiles");
    assert_eq!(lib_obj.code, 0);

    let matching_header = write(&dir, "lib_match.tmh", LIB_HEADER_MATCHING);
    let caller_src = write(&dir, "caller.tmc", IMPORT_CALLER);
    let caller_obj = compile(
        &dir,
        &caller_src,
        "caller.tmo",
        &["--extern", matching_header.to_str().unwrap(), "--nostdlib"],
    );
    assert!(caller_obj.is_ok(), "{:?}", caller_obj.err());

    let out = link(
        &dir,
        &[&dir.join("caller.tmo"), &dir.join("lib.tmo")],
        "out.tmx",
        &["--nostdlib"],
    );
    assert!(out.is_ok(), "{:?}", out.err());
    assert!(dir.join("out.tmx").exists());
}

// -- an end-to-end stdlib import, through `tmt build` ----------------------

/// A user program that imports a STANDARD-LIBRARY alphabet by `use` (a
/// tape declared over it) AND makes a plain `call` into a standard-library
/// routine, built through `tmt build` — compile, THEN link against the
/// embedded stdlib the way an ordinary program does, with no `--extern`
/// and no `-L`/`-l` at all (the embedded stdlib is pushed into the
/// compile-time declarations table by default, and auto-linked by
/// reachability at link time — both already-existing behaviors this
/// fixture is the first to exercise TOGETHER with a cross-unit alphabet
/// reference). Must link clean: no `AlphabetDrift` (the embedded stdlib's
/// declarations and its compiled object agree on `symbols`'s glyph order
/// by construction, but nothing before this test proved that for a
/// genuinely cross-unit-imported alphabet specifically) and no
/// `glyph-mismatch` warning on the `call`. Mutation: `Interface::imports`
/// recording the WRONG glyph order for `symbols` (the same class of bug
/// the synthetic drift pair catches) would turn this build's exit code
/// non-zero here too, against the real embedded stdlib rather than a
/// hand-written fixture.
#[test]
fn a_stdlib_alphabet_imported_by_use_builds_and_links_clean() {
    const STDLIB_IMPORT_CALLER: &str = "\
use std::binaryNumbers::symbols;

machine {
  tape num: symbols;
  entry state s { [*] -> call std::binaryNumbers::goToNumbersStart() then stop; }
}
";
    let dir = scratch("stdlib_import_build");
    let input = write(&dir, "caller.tmc", STDLIB_IMPORT_CALLER);
    let out_path = dir.join("out.tmx");
    let out = execute(&[
        "build".to_string(),
        input.to_str().unwrap().to_string(),
        "-o".to_string(),
        out_path.to_str().unwrap().to_string(),
    ]);
    assert!(out.is_ok(), "{:?}", out.err());
    let out = out.unwrap();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !out.stderr.contains("AlphabetDrift") && !out.stderr.contains("glyph-mismatch"),
        "{}",
        out.stderr
    );
    assert!(out_path.exists());
}

// -- symbolic emission: a bound call into another compilation unit --------

/// `mylib`'s declared interface: one exported alphabet and one exported
/// routine with a single `bits`-typed tape parameter. `--extern`-ed as a
/// `.tmh` wherever the caller's declarations are known.
const MYLIB_HEADER: &str = "\
namespace mylib {
  export alphabet bits { '_', '0', '1' }
  export routine plusOne(tape num: bits writes { '0', '1' });
}
";

/// The caller side of the symbolic-emission fixture: a 2-tape machine
/// (`ctl: bits`, `data: wide`) binding `data` into `mylib::plusOne`'s one
/// tape parameter through an inline map — the `.tmc` pair that must produce
/// the plan's own tool-verified `.tma` target text (`docs/formats.md` (bound
/// calls)): `call    mylib::plusOne [num: 1{3->'0', 4->'1'}]`.
fn app_src(map: &str) -> String {
    format!(
        "\
use mylib::bits;

alphabet wide {{ '_', '^', '$', '0', '1' }}

machine {{
  tape ctl: bits;
  tape data: wide;
  entry state go {{ [*, *] -> call mylib::plusOne(num = data{map}) then done; }}
  state done {{ [*, *] -> stop; }}
}}
"
    )
}

/// **[tool-verified]** against the plan's own D1/D2 `.tma` pair
/// (`.superpowers/plan3a-fixture-check.md`): the exact same operand,
/// reached from `.tmc` source through `--extern` rather than hand-authored
/// assembly. Mutation (verified by hand — see the task report): emitting
/// `dst` as an index instead of a label (`IrMapDst::Index(src)` in place
/// of `IrMapDst::Label(glyph_label(&p.dst))`); the byte compare here goes
/// red. `the_pair_links_and_runs` below assembles the D1/D2 `.tma` pair
/// directly rather than compiling it, so this particular mutation does not
/// reach it — the two tests are independent proofs (compiler emission
/// here, linker resolution there), not one continuous pipeline.
#[test]
fn an_external_bound_call_emits_the_symbolic_operand() {
    let dir = scratch("symbolic_emission");
    let header = write(&dir, "mylib.tmh", MYLIB_HEADER);
    let input = write(
        &dir,
        "app.tmc",
        &app_src(" with map { '0' -> '0', '1' -> '1' }"),
    );
    let out = compile(
        &dir,
        &input,
        "app.tma",
        &["-S", "--extern", header.to_str().unwrap()],
    );
    assert!(out.is_ok(), "{:?}", out.err());
    let tma = std::fs::read_to_string(dir.join("app.tma")).unwrap();
    let call_line = tma
        .lines()
        .find(|l| l.contains("mylib::plusOne"))
        .unwrap_or_else(|| panic!("no call line in:\n{tma}"))
        .trim();
    assert_eq!(
        call_line, "call    mylib::plusOne [num: 1{3->'0', 4->'1'}]",
        "{tma}"
    );
}

/// A bound call into a routine whose declarations are NOT given (no
/// `--extern`, no sibling, no library — a bare `tmt compile` with no
/// knowledge of `mylib` at all) still compiles: the site keeps its source
/// order and every entry is named, which is exactly what the linker's own
/// `reorder_named` exists to fix up once it has the callee's real
/// signature. Links clean against the real library object. Mutation:
/// requiring declarations before emitting a binding at all — compilation
/// would fail here, contradicting the "a bare `tmt compile` with no
/// knowledge of any library still produces a linkable object" claim.
#[test]
fn an_external_bound_call_without_declarations_still_compiles_and_links() {
    let dir = scratch("symbolic_no_decl");
    // No `use mylib::bits;` — that would itself need `mylib`'s declarations
    // (`unresolved-alphabet`), which this test deliberately withholds. The
    // caller declares its own local alphabet instead.
    let app = "\
alphabet wide { '_', '^', '$', '0', '1' }

machine {
  tape data: wide;
  entry state go { [*] -> call mylib::plusOne(num = data with map { '0' -> '0', '1' -> '1' }) then done; }
  state done { [*] -> stop; }
}
";
    let input = write(&dir, "app.tmc", app);
    let app_obj = compile(&dir, &input, "app.tmo", &["--nostdlib"]);
    assert!(app_obj.is_ok(), "{:?}", app_obj.err());

    let lib_src = write(
        &dir,
        "mylib.tmc",
        "\
namespace mylib {
  export alphabet bits { '_', '0', '1' }
  export routine plusOne(tape num: bits writes { '0', '1' }) {
    entry state s { [*] -> write ['1'] return; }
  }
}
",
    );
    let lib_obj = compile(&dir, &lib_src, "mylib.tmo", &["--nostdlib"]);
    assert!(lib_obj.is_ok(), "{:?}", lib_obj.err());

    let out = link(
        &dir,
        &[&dir.join("app.tmo"), &dir.join("mylib.tmo")],
        "app.tmx",
        &["--nostdlib", "--call-mech", "frames"],
    );
    assert!(out.is_ok(), "{:?}", out.err());
    assert!(dir.join("app.tmx").exists());
}

/// A caller tape whose alphabet is the SAME SIZE as `bits` but declares its
/// two non-blank glyphs in the OPPOSITE order — the one shape that triggers
/// `glyph-mismatch` specifically (a cardinality difference would trigger
/// `narrow-alphabet` instead, checked first and returning early — established
/// fact G in `.superpowers/plan3a-fixture-check.md`, the shipped corpus's own
/// alphabets being position-identical with the stdlib's).
fn reordered_app_src(map: &str) -> String {
    format!(
        "\
alphabet swapped {{ '_', '1', '0' }}

machine {{
  tape data: swapped;
  entry state go {{ [*] -> call mylib::plusOne(num = data{map}) then done; }}
  state done {{ [*] -> stop; }}
}}
"
    )
}

/// An OMITTED map (no `with map` at all) emits NO pairs — index identity —
/// so the linker's `glyph-mismatch` warning still fires when the caller and
/// callee alphabets are equal-size but declare their glyphs in a different
/// order. Mutation: "helpfully" expanding the omitted map into identity
/// pairs; the warning stops firing, the silent-failure shape the arc exists
/// to raise.
#[test]
fn an_omitted_map_emits_no_pairs() {
    let dir = scratch("symbolic_omitted_map");
    let header = write(&dir, "mylib.tmh", MYLIB_HEADER);
    let input = write(&dir, "app.tmc", &reordered_app_src(""));
    let out = compile(
        &dir,
        &input,
        "app.tmo",
        &["--extern", header.to_str().unwrap(), "--nostdlib"],
    );
    assert!(out.is_ok(), "{:?}", out.err());

    let lib_src = write(
        &dir,
        "mylib.tmc",
        "\
namespace mylib {
  export alphabet bits { '_', '0', '1' }
  export routine plusOne(tape num: bits writes { '0', '1' }) {
    entry state s { [*] -> return; }
  }
}
",
    );
    let lib_obj = compile(&dir, &lib_src, "mylib.tmo", &["--nostdlib"]);
    assert!(lib_obj.is_ok(), "{:?}", lib_obj.err());

    let out = link(
        &dir,
        &[&dir.join("app.tmo"), &dir.join("mylib.tmo")],
        "app.tmx",
        &["--nostdlib", "--call-mech", "frames", "-v"],
    );
    assert!(out.is_ok(), "{:?}", out.err());
    assert!(
        out.unwrap().stderr.contains("glyph-mismatch"),
        "an omitted map over a reordered, equal-size alphabet must warn"
    );
}

/// `with map { }` — a WRITTEN empty map — silences the same warning on
/// purpose: it tells the linker "bind by index, deliberately," the
/// documented distinction from an omitted map. Same reordered-alphabet
/// fixture as `an_omitted_map_emits_no_pairs`, the ONE difference between
/// the two tests being the `{ }`. Mutation: treating `{}` as an omitted map
/// (`map_written` collapsed to `false`); the warning returns, making this
/// test fail exactly where its sibling passes.
#[test]
fn with_map_empty_silences_the_warning() {
    let dir = scratch("symbolic_empty_map");
    let header = write(&dir, "mylib.tmh", MYLIB_HEADER);
    let input = write(&dir, "app.tmc", &reordered_app_src(" with map { }"));
    let out = compile(
        &dir,
        &input,
        "app.tmo",
        &["--extern", header.to_str().unwrap(), "--nostdlib"],
    );
    assert!(out.is_ok(), "{:?}", out.err());

    let lib_src = write(
        &dir,
        "mylib.tmc",
        "\
namespace mylib {
  export alphabet bits { '_', '0', '1' }
  export routine plusOne(tape num: bits writes { '0', '1' }) {
    entry state s { [*] -> return; }
  }
}
",
    );
    let lib_obj = compile(&dir, &lib_src, "mylib.tmo", &["--nostdlib"]);
    assert!(lib_obj.is_ok(), "{:?}", lib_obj.err());

    let out = link(
        &dir,
        &[&dir.join("app.tmo"), &dir.join("mylib.tmo")],
        "app.tmx",
        &["--nostdlib", "--call-mech", "frames", "-v"],
    );
    assert!(out.is_ok(), "{:?}", out.err());
    assert!(
        !out.unwrap().stderr.contains("glyph-mismatch"),
        "a written empty map silences the warning on purpose"
    );
}

/// The in-unit regression gate: a local bound call, byte-compared against
/// the fixture's own record of what it printed BEFORE this task (the
/// existing corpus's own binary-plus-one call, `cli_programs.rs`'s
/// `A2_BINARY_PLUS_ONE`-shaped site — reconstructed here rather than
/// imported, since fixtures are per-file by house convention). Mutation:
/// taking the symbolic (named, out-of-unit) path for an in-unit callee —
/// the operand would gain a `name: ` prefix that never printed before.
#[test]
fn an_in_unit_bound_call_is_byte_identical_to_before() {
    let dir = scratch("symbolic_in_unit_gate");
    let src = "\
alphabet wide { '_', '^', '$', '0', '1' }
alphabet bits { '_', '0', '1' }

routine plusOne(tape num: bits writes { '0', '1' }) {
  entry state s { [*] -> write ['1'] return; }
}

machine {
  tape ctl: bits;
  tape data: wide;
  entry state go { [*, *] -> call plusOne(num = data with map { '0' -> '0', '1' -> '1' }) then done; }
  state done { [*, *] -> stop; }
}
";
    let input = write(&dir, "app.tmc", src);
    let out = compile(&dir, &input, "app.tma", &["-S"]);
    assert!(out.is_ok(), "{:?}", out.err());
    let tma = std::fs::read_to_string(dir.join("app.tma")).unwrap();
    let call_line = tma
        .lines()
        .find(|l| l.contains("call    plusOne"))
        .unwrap_or_else(|| panic!("no call line in:\n{tma}"))
        .trim();
    assert_eq!(
        call_line, "call    plusOne [1{3->1, 4->2}]",
        "an in-unit entry stays positional and index-only: {tma}"
    );
}

/// `mylib::combine`'s TWO tape parameters — a single-parameter callee
/// cannot exercise "missing" at all (a call binding zero of its args is a
/// PLAIN call, not a partial binding). Declared both as a `.tmh` header
/// (the compile-error half) and as a real compiled object (the link-error
/// half, `MYLIB_COMBINE_TMC`).
const MYLIB_COMBINE_HEADER: &str = "\
namespace mylib {
  export alphabet bits { '_', '0', '1' }
  export routine combine(tape a: bits writes { '0', '1' }, tape b: bits writes { '0', '1' });
}
";
const MYLIB_COMBINE_TMC: &str = "\
namespace mylib {
  export alphabet bits { '_', '0', '1' }
  export routine combine(tape a: bits writes { '0', '1' }, tape b: bits writes { '0', '1' }) {
    entry state s { [*, *] -> return; }
  }
}
";

/// The missing-parameter pair: with the callee's declarations given, an
/// unbound parameter is a COMPILE error naming it — checked here exactly as
/// a local signature's arity is. With no declarations at all, the same
/// shape compiles (every entry named, source order) and the arity gap
/// surfaces only at LINK time instead. Mutation: requiring declarations for
/// the compile-error half to fire, or checking arity even with no
/// declarations — either collapses the pair into one behavior.
#[test]
fn a_missing_parameter_is_a_link_error_not_a_compile_error() {
    // With declarations: `combine` takes TWO tape parameters (`a`, `b`),
    // and this call binds only `a` — a compile error naming `b`.
    let dir = scratch("symbolic_missing_arg_compile");
    let header = write(&dir, "mylib.tmh", MYLIB_COMBINE_HEADER);
    let input = write(
        &dir,
        "app.tmc",
        "\
use mylib::bits;
machine {
  tape data: bits;
  entry state go { [*] -> call mylib::combine(a = data) then done; }
  state done { [*] -> stop; }
}
",
    );
    let err = compile(
        &dir,
        &input,
        "app.tmo",
        &["--extern", header.to_str().unwrap()],
    )
    .unwrap_err();
    assert!(err.contains("[missing-arg]"), "{err}");
    assert!(err.contains('b'), "{err}");

    // Without declarations: the identical shape compiles (nothing here to
    // check it against) and links against the REAL library object, whose
    // second parameter the call never binds — an arity mismatch the
    // LINKER's own interface pre-pass catches instead
    // (`crates/core/src/linker/interface.rs::reorder_named`).
    let dir2 = scratch("symbolic_missing_arg_link");
    let app2 = "\
alphabet bits2 { '_', '0', '1' }

machine {
  tape data: bits2;
  entry state go { [*] -> call mylib::combine(a = data) then done; }
  state done { [*] -> stop; }
}
";
    let input2 = write(&dir2, "app.tmc", app2);
    let app_obj = compile(&dir2, &input2, "app.tmo", &["--nostdlib"]);
    assert!(app_obj.is_ok(), "{:?}", app_obj.err());

    let lib_src = write(&dir2, "mylib.tmc", MYLIB_COMBINE_TMC);
    let lib_obj = compile(&dir2, &lib_src, "mylib.tmo", &["--nostdlib"]);
    assert!(lib_obj.is_ok(), "{:?}", lib_obj.err());

    let link_err = link(
        &dir2,
        &[&dir2.join("app.tmo"), &dir2.join("mylib.tmo")],
        "app.tmx",
        &["--nostdlib", "--call-mech", "frames"],
    )
    .unwrap_err();
    assert!(link_err.contains("does not bind parameter"), "{link_err}");
    assert!(link_err.contains('b'), "{link_err}");
}

/// A binding arg naming something `combine` does NOT declare: `order_by_
/// callee_tapes` alone only walks the FORWARD direction (a declared tape
/// with no matching arg), so an extra, unrecognized arg name is silently
/// dropped rather than reported unless checked separately — the exact
/// silent-failure shape this arc exists to raise, and invisible in the
/// missing-parameter test above because there the omission always left
/// some declared parameter unbound too. This fixture binds every real
/// parameter correctly and adds one bogus name, isolating the reverse
/// direction. Mutation: checking only that every DECLARED parameter has an
/// arg (the forward direction already covered by
/// `a_missing_parameter_is_a_link_error_not_a_compile_error`), never that
/// every ARG names something declared; the compile would wrongly succeed
/// and `xyz` would vanish from the emitted operand.
#[test]
fn an_unrecognized_argument_name_is_a_compile_error_with_declarations() {
    let dir = scratch("symbolic_unknown_arg");
    let header = write(&dir, "mylib.tmh", MYLIB_COMBINE_HEADER);
    let input = write(
        &dir,
        "app.tmc",
        "\
alphabet bits2 { '_', '0', '1' }

machine {
  tape data: bits2;
  tape data2: bits2;
  entry state go { [*, *] -> call mylib::combine(a = data, b = data2, xyz = data) then done; }
  state done { [*, *] -> stop; }
}
",
    );
    let err = compile(
        &dir,
        &input,
        "app.tmo",
        &["--extern", header.to_str().unwrap()],
    )
    .unwrap_err();
    assert!(err.contains("[unknown-arg]"), "{err}");
    assert!(err.contains("xyz"), "{err}");
}

/// The same reverse-direction gap, on a repeated name instead of an
/// unrecognized one: `order_by_callee_tapes`'s forward walk finds the
/// FIRST arg matching each declared tape and never notices a second one
/// naming it again, so a duplicate silently overwrote its first binding
/// rather than being reported. Binds `num` twice into a single-parameter
/// callee — the second binding simply vanishes without this check.
/// Mutation: checking only unknown names, not repeats; the compile would
/// wrongly succeed, keeping whichever binding source order happens to
/// list first and silently dropping the other.
#[test]
fn a_duplicate_argument_name_is_a_compile_error_with_declarations() {
    let dir = scratch("symbolic_duplicate_arg");
    let header = write(&dir, "mylib.tmh", MYLIB_HEADER);
    let input = write(
        &dir,
        "app.tmc",
        "\
use mylib::bits;

machine {
  tape data: bits;
  tape data2: bits;
  entry state go { [*, *] -> call mylib::plusOne(num = data, num = data2) then done; }
  state done { [*, *] -> stop; }
}
",
    );
    let err = compile(
        &dir,
        &input,
        "app.tmo",
        &["--extern", header.to_str().unwrap()],
    )
    .unwrap_err();
    assert!(err.contains("[duplicate-arg]"), "{err}");
    assert!(err.contains("num"), "{err}");
}

/// D1/D2's own `.tma` pair, assembled, linked and RUN — the end-to-end
/// proof that a symbolic site the compiler emits is exactly what the
/// linker's already-landed pre-pass expects. `plusOne` unconditionally
/// writes `bits` symbol 1 (`'0'`) to its one tape; the two-way pair set
/// `{3->'0', 4->'1'}` decides which CALLER symbol that write-back becomes.
/// `data` is SEEDED with `'$'` before the call (neither candidate answer),
/// so the assertion proves the call actually wrote, not merely that the
/// tape stayed at its initial value. Mutation: swapping the two pairs'
/// `dst` labels (`{3->'1', 4->'0'}`) — the callee's write-back now resolves
/// through the OTHER pair, landing `'1'` (wide index 4) instead of `'0'`
/// (index 3); verified by hand (see the task report) rather than committed
/// as a second test, since the two branches are mutually exclusive
/// assertions on the same cell.
#[test]
fn the_pair_links_and_runs() {
    use mtc_core::formats::tapeblock::TapeSnapshot;
    use mtc_core::linker::{CallMech, LinkOptions};
    use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
    use mtc_turing_machine::arch::Tm1;
    use mtc_turing_machine::asm::{assemble, link};

    const MYLIB_TMA: &str = "\
.routine mylib::plusOne, tapes=1, alpha=(3)
.param num, ('_','0','1'), writes=('0','1')
.section code
.func mylib::plusOne
        wr      [1]
        ret
";
    const APP_TMA: &str = "\
.routine main, tapes=2, alpha=(3, 5)
.param ctl, ('_','0','1')
.param data, ('_','^','$','0','1')
.section code
.func main
        call    mylib::plusOne [num: 1{3->'0', 4->'1'}]
        stp
";

    let app_obj = assemble(APP_TMA, false).expect("app.tma assembles");
    let lib_obj = assemble(MYLIB_TMA, false).expect("mylib.tma assembles");
    let opts = LinkOptions {
        call_mech: CallMech::Frames,
        ..Default::default()
    };
    let exe = link(&[app_obj, lib_obj], &[], opts)
        .expect("the composition engine links the symbolic site")
        .executable;

    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");
    let mut ctl = WideTape::new(3);
    // Seed `data` with '$' (wide index 2) — neither candidate answer ('0'
    // at 3, '1' at 4) — so the assertion below proves the call wrote,
    // rather than merely that the tape held its initial value.
    let mut data = WideTape::from_snapshot(
        &TapeSnapshot {
            origin: 0,
            cells: vec![2],
            head: 0,
            alphabet: None,
        },
        5,
    )
    .expect("seeds");
    let mut devices: Vec<&mut dyn Tape> = vec![&mut ctl, &mut data];
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(100_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    assert_eq!(result.outcome, Outcome::Stopped);
    drop(devices);
    let snap = data.to_snapshot();
    let idx = (0 - snap.origin) as usize;
    assert_eq!(
        snap.cells.get(idx).copied().unwrap_or(0),
        3,
        "the callee's write-back resolves through the FIRST pair (3->'0'): {snap:?}"
    );
}
