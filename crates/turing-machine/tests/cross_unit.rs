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

/// Mutation: accepting `::` in only the machine tape-declaration position
/// — this fixture alone goes red, since it exercises the SIGNATURE
/// tape-parameter grammar exclusively (`Parser::sig_param`).
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
