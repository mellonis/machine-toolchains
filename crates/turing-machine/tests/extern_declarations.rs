//! `tmt compile --extern`/`--nostdlib`: external declaration modules
//! reaching the compile's own `Declarations` table (docs/tmt/cli.md
//! (--extern and --nostdlib)). `Declarations` and its `Resolved` payload
//! are crate-private (`declarations.rs`, `compiler::Resolved`), so —
//! exactly as `tests/header_roundtrip.rs` does for the equally
//! crate-private `header` printer — every assertion here goes through
//! `mtc_turing_machine::cli::execute`, observing the two things outside
//! the crate that consult the table: the footprint/contract check
//! (`compiler::check_contracts` -> `footprint::find_external`), which
//! every fixture below exercises, and cross-unit alphabet resolution
//! (`compiler::resolve_tape_alphabet` -> `find_external_alphabet`), which
//! `tests/cross_unit.rs` exercises instead — a `use lib::bits;` plus a
//! `tape d: bits;` reaching `bits`'s declarations only when `--extern`
//! gave them. A callee found in the table is believed at its DECLARED
//! effective write set; a callee found nowhere is opaque and assumed to
//! write the whole alphabet (docs/tmt/language.md (contract clauses)) —
//! so a caller with a narrow `writes {}` contract on a call into a
//! declared-but-external routine compiles cleanly iff that routine's
//! declarations reached the table.
//!
//! Every fixture below declares its alphabet LOCALLY and calls the
//! external routine TRANSPARENTLY (an empty-arg qualified `call`, binding
//! by tape index — the one external-call shape that already worked before
//! cross-unit alphabet resolution landed, independent of `Declarations`,
//! exactly as `crates/turing-machine/src/footprint.rs`'s own `STD_CALLER`
//! fixture demonstrates), so a compile's success or failure here turns
//! only on whether the CALLEE's declarations reached the table — not on
//! the alphabet resolution `cross_unit.rs` covers on its own.

use std::path::{Path, PathBuf};

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
/// public CLI entry (`header_roundtrip.rs`'s own precedent for testing a
/// crate-private surface).
fn compile(dir: &Path, input: &Path, flags: &[&str]) -> Result<CliOutput, String> {
    let out = dir.join("out.tmo");
    let mut argv: Vec<String> = vec!["compile".into(), input.to_str().unwrap().into()];
    argv.extend(flags.iter().map(|s| s.to_string()));
    argv.push("-o".into());
    argv.push(out.to_str().unwrap().into());
    execute(&argv)
}

const MYLIB_HEADER: &str = "\
namespace mylib {
  export alphabet bits { '_', '0', '1' }
  export routine widen(tape n: bits writes { '0' });
}
";

const MYLIB_TMC: &str = "\
namespace mylib {
  export alphabet bits { '_', '0', '1' }

  export routine widen(tape n: bits writes { '0' }) {
    entry state s { [*] -> write ['0'] return; }
  }
}
";

const BROKEN_HEADER: &str = "\
namespace mylib {
  export alphabet bits { '_', '0', '1' }
  export routine widen(tape n: bits writes { '0' }) {
    entry state s { [*] -> return; }
  }
}
";

const CALLER: &str = "\
alphabet bits { '_', '0', '1' }

routine caller(tape num: bits writes { '0' }) {
  entry state s { [*] -> call mylib::widen() then done; }
  state done { [*] -> return; }
}
";

/// Mutation: dropping the push — the table stays empty regardless of
/// `--extern`, so `mylib::widen` is never found, the callee stays opaque
/// (whole alphabet), and `caller`'s narrow `writes { '0' }` contract is
/// violated: the compile that must succeed here fails instead.
#[test]
fn an_extern_header_reaches_the_declarations_table() {
    let dir = scratch("extern_header");
    let mylib = write(&dir, "mylib.tmh", MYLIB_HEADER);
    let caller = write(&dir, "caller.tmc", CALLER);

    // Negative control (tool-verified): without --extern, mylib::widen is
    // opaque and the narrow contract on `num` is violated.
    let without = compile(&dir, &caller, &[]).unwrap_err();
    assert!(without.contains("writes-outside-contract"), "{without}");

    // With --extern, mylib.tmh's declared `writes { '0' }` is believed —
    // the table now holds mylib::widen's exported declaration.
    let with = compile(&dir, &caller, &["--extern", mylib.to_str().unwrap()]);
    assert!(with.is_ok(), "{:?}", with.err());
}

/// Mutation: requiring a `.tmh` extension (which would also violate the
/// never-dispatch-on-extension rule) — a `.tmc` extern would then be
/// rejected or silently skipped, `mylib::widen` stays unfound, and the
/// compile that must succeed here fails.
#[test]
fn an_extern_tmc_is_read_the_same_way() {
    let dir = scratch("extern_tmc");
    let mylib = write(&dir, "mylib.tmc", MYLIB_TMC);
    let caller = write(&dir, "caller.tmc", CALLER);

    let with = compile(&dir, &caller, &["--extern", mylib.to_str().unwrap()]);
    assert!(with.is_ok(), "{:?}", with.err());
}

/// Mutation: reporting the primary input's path instead of the broken
/// extern's own.
#[test]
fn a_broken_extern_names_its_own_file() {
    let dir = scratch("extern_broken");
    let broken = write(&dir, "broken.tmh", BROKEN_HEADER);
    let caller = write(&dir, "caller.tmc", CALLER);

    let err = compile(&dir, &caller, &["--extern", broken.to_str().unwrap()]).unwrap_err();
    assert!(err.contains("broken.tmh"), "{err}");
    assert!(!err.contains("caller.tmc"), "{err}");
}

const NOSTDLIB_CALLER: &str = "\
alphabet bin { '_', '^', '$', '0', '1' }

routine caller(tape n: bin writes {}) {
  entry state s { [*] -> call std::binaryNumbers::goToNumbersStart() then return; }
}
";

/// Mutation: ignoring `--nostdlib` — the embedded stdlib stays in the
/// table, `goToNumbersStart`'s declared `writes {}` is still believed,
/// and the compile that must fail here (tool-verified) succeeds instead.
#[test]
fn nostdlib_empties_the_table() {
    let dir = scratch("nostdlib_empty");
    let caller = write(&dir, "caller.tmc", NOSTDLIB_CALLER);

    let err = compile(&dir, &caller, &["--nostdlib"]).unwrap_err();
    assert!(err.contains("writes-outside-contract"), "{err}");
}

/// Mutation: dropping the stdlib from the table even without
/// `--nostdlib` — the compile that must succeed here fails instead.
#[test]
fn without_nostdlib_the_stdlib_is_present() {
    let dir = scratch("nostdlib_present");
    let caller = write(&dir, "caller.tmc", NOSTDLIB_CALLER);

    let out = compile(&dir, &caller, &[]);
    assert!(out.is_ok(), "{:?}", out.err());
}

/// `tmt build`'s argv mode has no `--extern` of its own, but reuses its
/// existing (link-scoped) `--nostdlib` for the compile-time declarations
/// base too (`cli/driver.rs::argv_compile_options`) — tool-verified:
/// without `--nostdlib`, `tmt build` on this source fails only at LINK
/// (no `main` entry, unrelated to this task); with it, it fails earlier,
/// at COMPILE, on the same `writes-outside-contract` `tmt compile
/// --nostdlib` produces. Mutation: `--nostdlib` reaching only the link
/// step as before the fix — the compile step would keep believing the
/// stdlib and this build would never surface a compile-side error.
#[test]
fn argv_build_nostdlib_reaches_the_compile_step_too() {
    let dir = scratch("build_nostdlib");
    let input = write(&dir, "caller.tmc", NOSTDLIB_CALLER);
    let out = dir.join("out.tmx");
    let err = execute(&[
        "build".to_string(),
        input.to_str().unwrap().to_string(),
        "--nostdlib".to_string(),
        "-o".to_string(),
        out.to_str().unwrap().to_string(),
    ])
    .unwrap_err();
    assert!(err.contains("writes-outside-contract"), "{err}");
}

const MY_STD_HEADER: &str = "\
namespace std {
  namespace binaryNumbers {
    export alphabet symbols { '_', '^', '$', '0', '1' }
    export routine goToNumbersStart(tape num: symbols writes { '^' });
  }
}
";

const STD_SHADOW_CALLER: &str = "\
alphabet bin { '_', '^', '$', '0', '1' }

routine caller(tape n: bin writes { '^' }) {
  entry state s { [*] -> call std::binaryNumbers::goToNumbersStart() then return; }
}
";

const BARE_CALLER: &str = "\
alphabet bareBits { '_', '0', '1' }

routine bareCaller(tape n: bareBits writes { '0', '1' }) {
  entry state s { [*] -> call std::binaryNumbersBare::plusOne() then return; }
}
";

/// Mutation: re-adding the embedded stdlib when any `--extern` is given
/// (i.e. ignoring `--nostdlib` once `--extern` is non-empty). A check on
/// a name `my_std.tmh` ITSELF declares cannot catch this — first-match
/// always finds the `--extern` entry before a redundantly-pushed stdlib
/// one declaring the SAME name, extern-before-stdlib ordering holding
/// either way. So this test's second half calls a real-stdlib-only
/// routine `my_std.tmh` never declares: under the mutation the embedded
/// library's own (matching) contract is found and the compile succeeds;
/// correctly, the callee is opaque and it fails.
#[test]
fn an_explicit_std_header_under_nostdlib_shadows_nothing_and_is_used() {
    let dir = scratch("nostdlib_shadow");
    let my_std = write(&dir, "my_std.tmh", MY_STD_HEADER);
    let extern_arg = my_std.to_str().unwrap().to_string();
    let flags = ["--nostdlib", "--extern", extern_arg.as_str()];

    // my_std.tmh's own declaration is used: its narrower `writes { '^' }`
    // is what this compile believes.
    let shadow_caller = write(&dir, "shadow_caller.tmc", STD_SHADOW_CALLER);
    let shadowed = compile(&dir, &shadow_caller, &flags);
    assert!(shadowed.is_ok(), "{:?}", shadowed.err());

    // The embedded stdlib is NOT also present: a call to a real-
    // stdlib-only routine `my_std.tmh` never declares is opaque, so a
    // caller whose contract matches only the REAL routine's own declared
    // set (tool-verified: `std::binaryNumbersBare::plusOne` declares
    // exactly `writes { '0', '1' }`) fails.
    let bare_caller = write(&dir, "bare_caller.tmc", BARE_CALLER);
    let unshadowed = compile(&dir, &bare_caller, &flags);
    let err = unshadowed.unwrap_err();
    assert!(err.contains("writes-outside-contract"), "{err}");
}

const ORDER_A: &str = "\
namespace std {
  namespace binaryNumbers {
    export alphabet symbols { '_', '^', '$', '0', '1' }
    export routine goToNumbersStart(tape num: symbols writes { '^' });
  }
}
";

const ORDER_B: &str = "\
namespace std {
  namespace binaryNumbers {
    export alphabet symbols { '_', '^', '$', '0', '1' }
    export routine goToNumbersStart(tape num: symbols writes { '$' });
  }
}
";

/// Mutation: reversing extern order, or pushing the embedded stdlib
/// FIRST instead of last. Reversing swaps which of `order_a`/`order_b`
/// wins in BOTH orderings below, flipping both assertions. Pushing the
/// stdlib first makes the embedded library's own `writes {}` win over
/// EITHER extern (the empty set is a subset of any declared contract),
/// turning the second, expected-failing compile into a success too —
/// this is the mutation the orchestrator verified by hand (see the task
/// report).
#[test]
fn extern_order_is_command_line_order_then_stdlib() {
    let dir = scratch("extern_order");
    let a = write(&dir, "order_a.tmh", ORDER_A);
    let b = write(&dir, "order_b.tmh", ORDER_B);
    let caller = write(&dir, "caller.tmc", STD_SHADOW_CALLER);
    let a_str = a.to_str().unwrap().to_string();
    let b_str = b.to_str().unwrap().to_string();

    // order_a first: its `writes { '^' }` matches the caller's contract.
    let a_first = compile(
        &dir,
        &caller,
        &["--extern", a_str.as_str(), "--extern", b_str.as_str()],
    );
    assert!(a_first.is_ok(), "{:?}", a_first.err());

    // order_b first: its `writes { '$' }` does not.
    let b_first = compile(
        &dir,
        &caller,
        &["--extern", b_str.as_str(), "--extern", a_str.as_str()],
    );
    let err = b_first.unwrap_err();
    assert!(err.contains("writes-outside-contract"), "{err}");
}

const CHAIN_OTHER: &str = "\
namespace other {
  export alphabet bits { '_', '0', '1' }
}
";

/// `lib.tmc`'s own `use other::bits;` line means READING `lib.tmc` itself
/// (as an `--extern` file, LENIENT — its declarations are all this reads,
/// the body along for the ride) needs `other.tmc`'s declarations —
/// `--extern` files may depend on EACH OTHER, closed by the same shared
/// fixpoint every declaration source in the crate goes through
/// (docs/tmt/cli.md (--extern and --nostdlib)), regardless of which one
/// is given first.
const CHAIN_LIB: &str = "\
use other::bits;

namespace lib {
  export routine widen(tape n: bits writes { '0' }) {
    entry state s { [*] -> write ['0'] return; }
  }
}
";

const CHAIN_CALLER: &str = "\
alphabet ab { '_', '0', '1' }

routine caller(tape n: ab writes { '0' }) {
  entry state s { [*] -> call lib::widen() then done; }
  state done { [*] -> return; }
}
";

/// Mutation: reading each `--extern` file against the embedded stdlib
/// alone (this crate's own pre-fix shape for library/sibling reading) —
/// `lib.tmc`'s `use other::bits;` would then be unresolvable regardless
/// of order, and BOTH variants below would fail with "declarations were
/// not given" naming `other::bits`.
#[test]
fn an_extern_file_may_depend_on_another_extern_file_other_first() {
    let dir = scratch("extern_chain_other_first");
    let other = write(&dir, "other.tmc", CHAIN_OTHER);
    let lib = write(&dir, "lib.tmc", CHAIN_LIB);
    let caller = write(&dir, "caller.tmc", CHAIN_CALLER);

    let out = compile(
        &dir,
        &caller,
        &[
            "--extern",
            other.to_str().unwrap(),
            "--extern",
            lib.to_str().unwrap(),
        ],
    );
    assert!(out.is_ok(), "{:?}", out.err());
}

/// The other `--extern` order, for the identical fixture.
#[test]
fn an_extern_file_may_depend_on_another_extern_file_lib_first() {
    let dir = scratch("extern_chain_lib_first");
    let other = write(&dir, "other.tmc", CHAIN_OTHER);
    let lib = write(&dir, "lib.tmc", CHAIN_LIB);
    let caller = write(&dir, "caller.tmc", CHAIN_CALLER);

    let out = compile(
        &dir,
        &caller,
        &[
            "--extern",
            lib.to_str().unwrap(),
            "--extern",
            other.to_str().unwrap(),
        ],
    );
    assert!(out.is_ok(), "{:?}", out.err());
}
