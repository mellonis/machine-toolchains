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

/// Two `--extern` headers declaring one standard-library name with
/// different contracts conflict, in either command-line order; the first
/// named is the other origin of the second. Mutation: skip the
/// own-definition-versus-table call in `resolve_program` — both compiles
/// return to first-wins.
#[test]
fn two_extern_headers_declaring_one_name_differently_conflict_in_either_order() {
    let dir = scratch("extern_order");
    let a = write(&dir, "order_a.tmh", ORDER_A);
    let b = write(&dir, "order_b.tmh", ORDER_B);
    let caller = write(&dir, "caller.tmc", STD_SHADOW_CALLER);
    for (first, second) in [(&a, &b), (&b, &a)] {
        let err = compile(
            &dir,
            &caller,
            &[
                "--nostdlib",
                "--extern",
                first.to_str().unwrap(),
                "--extern",
                second.to_str().unwrap(),
            ],
        )
        .unwrap_err();
        assert!(err.contains("[conflicting-declaration]"), "{err}");
        assert!(
            err.contains("std::binaryNumbers::goToNumbersStart"),
            "{err}"
        );
        assert!(err.contains(&format!("`{}`", first.display())), "{err}");
    }
}

const L1_TMH: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab writes { 'a' });
}
";

const L2_TMH: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab writes { 'a' }, state done);
}
";

const C8: &str = "\
use lib::ab;
use lib::r;
machine {
  tape t: ab;
  entry state s { [*] -> call r(t = t) then stop; }
}
";

const OWN: &str = "\
namespace lib {
  export alphabet ab { '_', 'a' }
  export routine r(tape t: ab writes { 'a' }, state done) {
    entry state s { [*] -> goto done; }
  }
}
machine {
  tape t: lib::ab;
  entry state s { [*] -> call lib::r(t = t, done = stop) then stop; }
}
";

const OWNSTD: &str = "\
namespace std {
  namespace binaryNumbers {
    export routine goToNumber(tape num: std::binaryNumbers::symbols, state done) {
      entry state s { [*] -> goto done; }
    }
  }
}
machine {
  tape num: std::binaryNumbers::symbols;
  entry state s { [*] -> call std::binaryNumbers::goToNumber(num = num, done = stop) then stop; }
}
";

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

/// Two headers declaring `lib::r` with different signatures: the one read
/// second is refused against the first, in either order. Mutation: skip
/// the `check_own_against_table` call — the compile returns to first-wins
/// (Ok one way, `missing-arg` the other).
#[test]
fn two_headers_declaring_one_routine_differently_conflict() {
    let dir = scratch("conflict_l1_l2");
    let l1 = write(&dir, "l1.tmh", L1_TMH);
    let l2 = write(&dir, "l2.tmh", L2_TMH);
    let c8 = write(&dir, "c8.tmc", C8);
    for (first, second) in [(&l1, &l2), (&l2, &l1)] {
        let err = compile(
            &dir,
            &c8,
            &[
                "--nostdlib",
                "--extern",
                first.to_str().unwrap(),
                "--extern",
                second.to_str().unwrap(),
            ],
        )
        .unwrap_err();
        assert!(
            err.contains(&format!("{}:3:18: error:", second.display())),
            "{err}"
        );
        assert!(
            err.contains(&format!(
                "`lib::r` is declared differently by this unit and by `{}`",
                first.display()
            )),
            "{err}"
        );
        assert!(err.contains("[conflicting-declaration]"), "{err}");
    }
}

/// One unit declared twice — the same file given twice, or a byte copy —
/// is one declaration. Mutation: compare by origin instead of by form.
#[test]
fn one_unit_declared_twice_merges() {
    let dir = scratch("conflict_merge");
    let l1 = write(&dir, "l1.tmh", L1_TMH);
    let copy = write(&dir, "l1copy.tmh", L1_TMH);
    let c8 = write(&dir, "c8.tmc", C8);
    for other in [&l1, &copy] {
        let out = compile(
            &dir,
            &c8,
            &[
                "--nostdlib",
                "--extern",
                l1.to_str().unwrap(),
                "--extern",
                other.to_str().unwrap(),
            ],
        );
        assert!(out.is_ok(), "{:?}", out.err());
    }
}

/// Mutation: skip the `check_own_against_table` call.
#[test]
fn a_unit_defining_a_name_its_table_declares_differently_is_refused() {
    let dir = scratch("conflict_own");
    let l1 = write(&dir, "l1.tmh", L1_TMH);
    let own = write(&dir, "own.tmc", OWN);
    let err = compile(
        &dir,
        &own,
        &["--nostdlib", "--extern", l1.to_str().unwrap()],
    )
    .unwrap_err();
    assert!(err.contains("own.tmc:3:18: error:"), "{err}");
    assert!(
        err.contains(&format!("by this unit and by `{}`", l1.display())),
        "{err}"
    );
    assert!(err.contains("[conflicting-declaration]"), "{err}");
}

/// A unit compiled beside its own up-to-date header is fine; beside a
/// stale one it is refused. Mutation: own forms computed without
/// `settle_declarations` — the first half fails on `r`'s `noreturn`.
#[test]
fn a_unit_beside_its_own_header_compiles() {
    let dir = scratch("conflict_own_header");
    let own = write(&dir, "own.tmc", OWN);
    let printed = execute(&[
        "interface".into(),
        own.to_str().unwrap().into(),
        "--nostdlib".into(),
    ])
    .expect("interface");
    let fresh = write(&dir, "own.tmh", &printed.stdout);
    let ok = compile(
        &dir,
        &own,
        &["--nostdlib", "--extern", fresh.to_str().unwrap()],
    );
    assert!(ok.is_ok(), "{:?}", ok.err());
    let stale = write(&dir, "own_stale.tmh", L1_TMH);
    let err = compile(
        &dir,
        &own,
        &["--nostdlib", "--extern", stale.to_str().unwrap()],
    )
    .unwrap_err();
    assert!(err.contains("[conflicting-declaration]"), "{err}");
}

/// Mutation: leave the embedded stdlib out of the table the own-definition
/// check reads.
#[test]
fn a_unit_redefining_a_std_routine_differently_is_refused() {
    let dir = scratch("conflict_ownstd");
    let src = write(&dir, "ownstd.tmc", OWNSTD);
    let err = compile(&dir, &src, &[]).unwrap_err();
    assert!(err.contains("ownstd.tmc:3:20: error:"), "{err}");
    assert!(
        err.contains("`std::binaryNumbers::goToNumber` is declared differently by this unit and by the standard library"),
        "{err}"
    );
}

/// The identical redefinition compiles (the link refuses it). Mutation:
/// refuse every own name the table declares without comparing forms.
#[test]
fn a_unit_redefining_a_std_routine_identically_compiles() {
    let dir = scratch("conflict_ownsame");
    let src = write(&dir, "ownsame.tmc", OWNSAME);
    let out = compile(&dir, &src, &[]);
    assert!(out.is_ok(), "{:?}", out.err());
}

/// A unit's source and its header given to one compile are one
/// declaration seen twice. Mutation: skip the writes step of
/// `header::settle_declarations` — the source reading then publishes the
/// whole alphabet and conflicts with the header.
#[test]
fn a_units_source_and_its_header_together_merge() {
    let dir = scratch("conflict_src_and_header");
    let u = write(&dir, "u.tmc", U);
    let header = execute(&[
        "interface".into(),
        u.to_str().unwrap().into(),
        "--nostdlib".into(),
    ])
    .expect("interface");
    let u_tmh = write(&dir, "u.tmh", &header.stdout);
    let uc = write(&dir, "uc.tmc", UC);
    let out = compile(
        &dir,
        &uc,
        &[
            "--nostdlib",
            "--extern",
            u.to_str().unwrap(),
            "--extern",
            u_tmh.to_str().unwrap(),
        ],
    );
    assert!(out.is_ok(), "{:?}", out.err());
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

// ── a broken --extern is named alongside its dependent's symptom ────────

/// A genuine parse error — a namespace block missing its closing `}`.
const BROKEN_PRODUCER: &str = "\
namespace prod {
  export alphabet bits { '_', '0', '1' }
";

/// `use`s the (broken) producer's alphabet — read alone this depends on
/// `producer`'s declarations exactly as `CHAIN_LIB` depends on
/// `CHAIN_OTHER` above, except here the dependency can never resolve.
/// Same namespace/routine name as `CHAIN_LIB` so `CHAIN_CALLER` (below)
/// can call it unchanged.
const CONSUMER_DEPENDS_ON_PRODUCER: &str = "\
use prod::bits;

namespace lib {
  export routine widen(tape n: bits writes { '0' });
}
";

/// `tmt compile --extern` shares the SAME rendering `tmt build` uses for
/// its own siblings and libraries (`cli/build.rs::render_unresolved`):
/// every unreadable `--extern` file is named, not just the first one in
/// command-line order. `producer.tmh` carries a genuine parse error;
/// `consumer.tmh` only fails because it depends on `producer.tmh`'s
/// declarations — both must be named, `producer.tmh`'s own parse error
/// FIRST (its code is not one of the four declarations-missing codes),
/// regardless of which was given first on the command line.
#[test]
fn an_extern_files_own_parse_error_is_shown_alongside_its_dependents_symptom_producer_first() {
    let dir = scratch("extern_broken_chain_producer_first");
    let producer = write(&dir, "producer.tmh", BROKEN_PRODUCER);
    let consumer = write(&dir, "consumer.tmh", CONSUMER_DEPENDS_ON_PRODUCER);
    let caller = write(&dir, "caller.tmc", CHAIN_CALLER);

    let err = compile(
        &dir,
        &caller,
        &[
            "--extern",
            producer.to_str().unwrap(),
            "--extern",
            consumer.to_str().unwrap(),
        ],
    )
    .unwrap_err();
    assert!(
        err.contains("producer.tmh") && err.contains("unexpected-token"),
        "{err}"
    );
    assert!(
        err.contains("consumer.tmh") && err.contains("declarations were not given"),
        "{err}"
    );
    let producer_at = err.find("producer.tmh").expect("named above");
    let consumer_at = err.find("consumer.tmh").expect("named above");
    assert!(
        producer_at < consumer_at,
        "the genuine parse error must print before the derived symptom: {err}"
    );
}

/// The other `--extern` order, for the identical fixture — byte-identical
/// output, proving the report no longer depends on command-line order.
#[test]
fn an_extern_files_own_parse_error_is_shown_alongside_its_dependents_symptom_consumer_first() {
    let dir = scratch("extern_broken_chain_consumer_first");
    let producer = write(&dir, "producer.tmh", BROKEN_PRODUCER);
    let consumer = write(&dir, "consumer.tmh", CONSUMER_DEPENDS_ON_PRODUCER);
    let caller = write(&dir, "caller.tmc", CHAIN_CALLER);

    let err = compile(
        &dir,
        &caller,
        &[
            "--extern",
            consumer.to_str().unwrap(),
            "--extern",
            producer.to_str().unwrap(),
        ],
    )
    .unwrap_err();
    assert!(
        err.contains("producer.tmh") && err.contains("unexpected-token"),
        "{err}"
    );
    assert!(
        err.contains("consumer.tmh") && err.contains("declarations were not given"),
        "{err}"
    );
    let producer_at = err.find("producer.tmh").expect("named above");
    let consumer_at = err.find("consumer.tmh").expect("named above");
    assert!(producer_at < consumer_at, "{err}");
}

/// A compiled object handed to `--extern` is refused on its own magic,
/// naming the rule: the flag takes a declaration SOURCE, and an object
/// reaches the table as a library instead (docs/tmt/cli.md (--extern and
/// --nostdlib)). Both subcommands that take the flag go through one
/// reader, so `tmt interface` is held to the same refusal here.
///
/// Mutation: drop the magic check and let the UTF-8 decode refuse the
/// object — the message then reports a malformed text file and never
/// names `--extern`'s own rule, so both `contains` assertions fail.
#[test]
fn an_object_given_to_extern_is_refused_by_its_magic() {
    let dir = scratch("extern_object");
    let mylib = write(&dir, "mylib.tmc", MYLIB_TMC);
    let caller = write(&dir, "caller.tmc", CALLER);

    // Build the object the mistake would hand to `--extern`.
    let object = dir.join("mylib.tmo");
    let built = execute(&[
        "compile".to_string(),
        mylib.to_str().unwrap().to_string(),
        "-o".to_string(),
        object.to_str().unwrap().to_string(),
    ])
    .unwrap_or_else(|e| panic!("compile mylib: {e}"));
    assert_eq!(built.code, 0, "{}", built.stderr);

    let err = compile(&dir, &caller, &["--extern", object.to_str().unwrap()]).unwrap_err();
    assert!(err.contains("mylib.tmo"), "{err}");
    assert!(err.contains("--extern takes a declaration SOURCE"), "{err}");

    // The same reader backs `tmt interface`'s own `--extern`.
    let from_interface = execute(&[
        "interface".to_string(),
        caller.to_str().unwrap().to_string(),
        "--extern".to_string(),
        object.to_str().unwrap().to_string(),
    ])
    .unwrap_err();
    assert!(
        from_interface.contains("--extern takes a declaration SOURCE"),
        "{from_interface}"
    );

    // The near miss: a real header at the same path still reads, so the
    // refusal keys on the container's magic and not on anything about
    // the argument's shape or name.
    let header = write(&dir, "mylib.tmh", MYLIB_HEADER);
    let ok = compile(&dir, &caller, &["--extern", header.to_str().unwrap()]);
    assert!(ok.is_ok(), "{:?}", ok.err());
}

const U: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  export routine h2(tape t: marks) {
    entry state s { [*] -> write ['x'] return; }
  }
}
";

const UC: &str = "\
use lib::marks;
use lib::h2;
routine c(tape t: marks writes { 'x' }) {
  entry state s { [*] -> call h2(t = t) then done; }
  state done { [*] -> return; }
}
";

const UC_NONE: &str = "\
use lib::marks;
use lib::h2;
routine c(tape t: marks writes {}) {
  entry state s { [*] -> call h2(t = t) then done; }
  state done { [*] -> return; }
}
";

/// A `.tmc` read for its declarations believes the inferred write set its
/// generated header prints, so a caller whose contract matches that set
/// compiles through either route. Mutation: skip the writes step of
/// `header::settle_declarations` — the `.tmc` route believes the whole
/// alphabet again and fails `writes-outside-contract`.
#[test]
fn a_source_read_believes_the_write_set_its_header_prints() {
    let dir = scratch("source_read_writes");
    let u = write(&dir, "u.tmc", U);
    let header = execute(&[
        "interface".into(),
        u.to_str().unwrap().into(),
        "--nostdlib".into(),
    ])
    .expect("interface");
    let u_tmh = write(&dir, "u.tmh", &header.stdout);
    let uc = write(&dir, "uc.tmc", UC);
    for route in [&u_tmh, &u] {
        let out = compile(
            &dir,
            &uc,
            &["--nostdlib", "--extern", route.to_str().unwrap()],
        );
        assert!(out.is_ok(), "{}: {:?}", route.display(), out.err());
    }
}

/// The near miss: a caller promising to write nothing is refused through
/// both routes. Mutation: publish the EMPTY set for an uncontracted tape —
/// the `.tmc` route then accepts it.
#[test]
fn a_source_read_still_refuses_what_the_header_refuses() {
    let dir = scratch("source_read_refuses");
    let u = write(&dir, "u.tmc", U);
    let header = execute(&[
        "interface".into(),
        u.to_str().unwrap().into(),
        "--nostdlib".into(),
    ])
    .expect("interface");
    let u_tmh = write(&dir, "u.tmh", &header.stdout);
    let uc = write(&dir, "uc.tmc", UC_NONE);
    for route in [&u_tmh, &u] {
        let err = compile(
            &dir,
            &uc,
            &["--nostdlib", "--extern", route.to_str().unwrap()],
        )
        .unwrap_err();
        assert!(
            err.contains("[writes-outside-contract]"),
            "{}: {err}",
            route.display()
        );
    }
}

/// A header printing a PRIVATE `graph lib::walker` — the body its exported
/// `outerA` nests — which writes `'y'`.
const PRIV_LA: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  graph walker(tape t: marks writes { 'y' }, state done) {
    entry state w {
      ['x'] -> write ['y'] goto done;
      [*]   -> goto done;
    }
  }
  export graph outerA(tape a: marks writes { 'y' }, state done) {
    entry graft walker(t = a, done = done) as inner;
  }
}
";

/// A header exporting a ROUTINE under the name `PRIV_LA` holds privately.
const PRIV_LR: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  export routine walker(tape t: marks writes {});
}
";

/// A header exporting a GRAPH of another body under that name.
const PRIV_LB: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  export graph walker(tape t: marks writes { 'x' }, state done) {
    entry state w {
      ['y'] -> write ['x'] goto done;
      [*]   -> goto done;
    }
  }
}
";

/// A SOURCE whose private `routine lib::r` writes `'y'`; its generated
/// header does not print it.
const PRIV_X: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  routine r(tape t: marks) {
    entry state s { [*] -> write ['y'] return; }
  }
  export routine other(tape t: marks) {
    entry state s { [*] -> call r(t = t) then d; }
    state d { [*] -> return; }
  }
}
";

/// Headers publishing `lib::r`: with another write set than `PRIV_X`'s
/// private routine has, and with the same one.
const PRIV_H_QUIET: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  export routine r(tape t: marks writes {});
}
";
const PRIV_H_SAME: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  export routine r(tape t: marks writes { 'y' });
}
";

/// Another library whose PRIVATE `lib::walker` has another body, with
/// nothing published under that name.
const PRIV_LZ: &str = "\
namespace lib {
  export alphabet marks { '_', 'x', 'y' }
  graph walker(tape t: marks writes { 'x' }, state done) {
    entry state w {
      ['y'] -> write ['x'] goto done;
      [*]   -> goto done;
    }
  }
  export graph outerZ(tape a: marks writes { 'x' }, state done) {
    entry graft walker(t = a, done = done) as inner;
  }
}
";

const PRIV_CALLS_WALKER: &str = "\
use lib::marks;
routine quiet(tape t: marks writes {}) {
  entry state s { [*] -> call lib::walker(t = t) then d; }
  state d { [*] -> return; }
}
machine {
  tape t: marks;
  entry state go { [*] -> call quiet(t = t) then fin; }
  state fin { [*] -> stop; }
}
";

const PRIV_GRAFTS_WALKER: &str = "\
use lib::marks;
machine {
  tape t: marks;
  entry graft lib::walker(t = t, done = fin) as r;
  state fin { [*] -> stop; }
}
";

const PRIV_GRAFTS_OUTER_A: &str = "\
use lib::marks;
use lib::outerA;
machine {
  tape t: marks;
  entry graft outerA(a = t, done = fin) as r;
  state fin { [*] -> stop; }
}
";

const PRIV_CALLS_R_QUIET: &str = "\
use lib::marks;
routine quiet(tape t: marks writes {}) {
  entry state s { [*] -> call lib::r(t = t) then d; }
  state d { [*] -> return; }
}
machine {
  tape t: marks;
  entry state go { [*] -> call quiet(t = t) then fin; }
  state fin { [*] -> stop; }
}
";

const PRIV_CALLS_R_LOUD: &str = "\
use lib::marks;
routine loud(tape t: marks writes { 'y' }) {
  entry state s { [*] -> call lib::r(t = t) then d; }
  state d { [*] -> return; }
}
machine {
  tape t: marks;
  entry state go { [*] -> call loud(t = t) then fin; }
  state fin { [*] -> stop; }
}
";

/// `tmt compile caller --nostdlib --extern FIRST --extern SECOND`, in both
/// orders of the two declaration files.
fn compile_in_both_orders(
    tag: &str,
    caller: &str,
    a: (&str, &str),
    b: (&str, &str),
) -> [Result<CliOutput, String>; 2] {
    let dir = scratch(tag);
    let caller = write(&dir, "caller.tmc", caller);
    let a = write(&dir, a.0, a.1);
    let b = write(&dir, b.0, b.1);
    [(&a, &b), (&b, &a)].map(|(first, second)| {
        compile(
            &dir,
            &caller,
            &[
                "--nostdlib",
                "--extern",
                first.to_str().unwrap(),
                "--extern",
                second.to_str().unwrap(),
            ],
        )
    })
}

/// A declaration one `--extern` file carries WITHOUT exporting it, beside
/// another file's published declaration of the same name in another form,
/// is refused whichever file comes first, and both files are named — a
/// consumer's lookup would otherwise take whichever came first. Four
/// shapes: a caller of a name that is a private graph here and a
/// published routine there; a graft of a name that is a private graph
/// here and an exported graph of another body there; a consumer of one
/// library's export while the two libraries hold that pair; and a caller
/// of a name that is a private routine no header prints in a source and a
/// published routine in a header. Mutation: compare exported forms only
/// (`published::readings_conflict` requiring BOTH readings to be
/// published) — the orders then diverge: one compiles or reports a false
/// `writes-outside-contract`, the other conflicts.
#[test]
fn a_private_name_beside_a_published_one_of_another_form_is_refused_in_both_extern_orders() {
    let shapes = [
        (
            "priv_call",
            PRIV_CALLS_WALKER,
            ("la.tmh", PRIV_LA),
            ("lr.tmh", PRIV_LR),
            "lib::walker",
        ),
        (
            "priv_graft",
            PRIV_GRAFTS_WALKER,
            ("la.tmh", PRIV_LA),
            ("lb.tmh", PRIV_LB),
            "lib::walker",
        ),
        (
            "priv_nested",
            PRIV_GRAFTS_OUTER_A,
            ("la.tmh", PRIV_LA),
            ("lb.tmh", PRIV_LB),
            "lib::walker",
        ),
        (
            "priv_unprinted",
            PRIV_CALLS_R_QUIET,
            ("x.tmc", PRIV_X),
            ("h.tmh", PRIV_H_QUIET),
            "lib::r",
        ),
    ];
    for (tag, caller, a, b, name) in shapes {
        for (order, out) in compile_in_both_orders(tag, caller, a, b)
            .into_iter()
            .enumerate()
        {
            let err = out.expect_err("a private name beside a published one of another form");
            assert!(
                err.contains("[conflicting-declaration]"),
                "{tag}, order {order}: {err}"
            );
            assert!(
                err.contains(&format!("`{name}` is declared differently")),
                "{tag}, order {order}: {err}"
            );
            for file in [a.0, b.0] {
                assert!(err.contains(file), "{tag}, order {order}: {file}: {err}");
            }
            assert!(
                !err.contains("[writes-outside-contract]"),
                "{tag}, order {order}: {err}"
            );
        }
    }
}

/// The near miss: the private routine beside a header publishing the SAME
/// form (the set the routine really writes) merges in both orders, and
/// the caller is held to that one form. Mutation: refuse a private
/// declaration that shares a name with a published one whatever its form.
#[test]
fn a_private_name_beside_a_published_one_of_the_same_form_compiles_in_both_extern_orders() {
    for (order, out) in compile_in_both_orders(
        "priv_same",
        PRIV_CALLS_R_LOUD,
        ("x.tmc", PRIV_X),
        ("h.tmh", PRIV_H_SAME),
    )
    .into_iter()
    .enumerate()
    {
        assert!(out.is_ok(), "order {order}: {:?}", out.err());
    }
}

/// Two libraries' PRIVATE graphs of one name, with nothing published
/// under it, are not compared with each other: a consumer of one
/// library's export compiles in both orders. Mutation: compare every pair
/// of readings whatever their export status.
#[test]
fn two_private_graphs_of_one_name_compile_for_a_consumer_of_one_export_in_both_extern_orders() {
    for (order, out) in compile_in_both_orders(
        "priv_priv",
        PRIV_GRAFTS_OUTER_A,
        ("la.tmh", PRIV_LA),
        ("lz.tmh", PRIV_LZ),
    )
    .into_iter()
    .enumerate()
    {
        assert!(out.is_ok(), "order {order}: {:?}", out.err());
    }
}

// ── the order of `--extern` sources decides nothing ───────────────────────

/// `a::mid` declares no `writes` clause and calls `b::leaf`: the set it
/// publishes is inferred, and is what `b::leaf` writes only once `b`'s
/// declarations are known.
const EXT_ORDER_A: &str = "\
namespace a {
  export alphabet ab { '_', 'x', 'y' }
  export routine mid(tape t: ab) {
    entry state s { [*] -> call b::leaf(t = t) then d; }
    state d { [*] -> return; }
  }
}
";

const EXT_ORDER_B: &str = "\
namespace b {
  export alphabet ab { '_', 'x', 'y' }
  export routine leaf(tape t: ab) {
    entry state s { [*] -> write ['x'] return; }
  }
}
";

/// `b::leaf` one level deeper: it calls `d::leaf`, which does the write.
const EXT_ORDER_B_RELAY: &str = "\
namespace b {
  export alphabet ab { '_', 'x', 'y' }
  export routine leaf(tape t: ab) {
    entry state s { [*] -> call d::leaf(t = t) then e; }
    state e { [*] -> return; }
  }
}
";

const EXT_ORDER_D: &str = "\
namespace d {
  export alphabet ab { '_', 'x', 'y' }
  export routine leaf(tape t: ab) {
    entry state s { [*] -> write ['x'] return; }
  }
}
";

const EXT_ORDER_C: &str = "\
use a::ab;
routine top(tape t: ab writes { 'x' }) {
  entry state s { [*] -> call a::mid(t = t) then d; }
  state d { [*] -> return; }
}
machine {
  tape t: ab;
  entry state go { [*] -> call top(t = t) then fin; }
  state fin { [*] -> stop; }
}
";

/// Every ordering of `items`.
fn orderings<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut all = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(i);
        for mut tail in orderings(&rest) {
            tail.insert(0, head.clone());
            all.push(tail);
        }
    }
    all
}

/// `tmt compile c.tmc --nostdlib --extern …` with the externs in the
/// given order.
fn compile_against(dir: &Path, unit: &Path, externs: &[PathBuf]) -> Result<CliOutput, String> {
    let mut flags: Vec<&str> = vec!["--nostdlib"];
    for e in externs {
        flags.push("--extern");
        flags.push(e.to_str().unwrap());
    }
    compile(dir, unit, &flags)
}

/// Whether a unit compiles does not depend on the order of its `--extern`
/// sources: with both known, `a::mid` writes only what `b::leaf` writes,
/// so the caller's `writes { 'x' }` holds whichever is given first — and
/// in every order of the chain one level deeper. Mutation: read each
/// source once (in `header::resolve_declarations`, mark a peer due a read
/// only while it has no reading) — `--extern a.tmc --extern b.tmc` then
/// fails `writes-outside-contract`.
#[test]
fn a_compile_does_not_depend_on_the_order_of_its_extern_sources() {
    let dir = scratch("extern_source_order");
    let c = write(&dir, "c.tmc", EXT_ORDER_C);
    let a = write(&dir, "a.tmc", EXT_ORDER_A);
    let b = write(&dir, "b.tmc", EXT_ORDER_B);
    for order in orderings(&[a.clone(), b]) {
        let out = compile_against(&dir, &c, &order);
        assert!(out.is_ok(), "{order:?}: {:?}", out.err());
    }
    let b = write(&dir, "b.tmc", EXT_ORDER_B_RELAY);
    let d = write(&dir, "d.tmc", EXT_ORDER_D);
    for order in orderings(&[a, b, d]) {
        let out = compile_against(&dir, &c, &order);
        assert!(out.is_ok(), "{order:?}: {:?}", out.err());
    }
}

/// The near miss: a caller promising to write only `'y'` is refused in
/// every order, for the glyph the chain really writes. Mutation: let the
/// compile of the unit itself defer the peer-dependent checks
/// (`compiler::analyze_with_provenance` passing `PeerChecks::Deferred`) —
/// deferring is for a declarations read against peers still settling, and
/// here every order would compile.
#[test]
fn a_compile_breaking_a_contract_is_refused_in_every_order_of_its_extern_sources() {
    let dir = scratch("extern_source_order_violation");
    let c = write(
        &dir,
        "c.tmc",
        &EXT_ORDER_C.replace("writes { 'x' }", "writes { 'y' }"),
    );
    let a = write(&dir, "a.tmc", EXT_ORDER_A);
    let b = write(&dir, "b.tmc", EXT_ORDER_B);
    for order in orderings(&[a, b]) {
        let err = compile_against(&dir, &c, &order).expect_err("the contract is broken");
        assert!(
            err.contains("`top` may write 'x' on tape `t`")
                && err.contains("[writes-outside-contract]"),
            "{order:?}: {err}"
        );
    }
}
