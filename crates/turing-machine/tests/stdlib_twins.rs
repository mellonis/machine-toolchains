//! The embedded standard library's volatile twin namespaces, proven against
//! the plain namespaces they mirror.
//!
//! `std::binaryNumbersVolatile` and `std::binaryNumbersBareVolatile` exist so
//! that the CALLEE side of a `std::` call can carry the volatile mark. The
//! library arrives at link time already compiled, and nothing a caller
//! declares about its own band reaches the routine it calls, so a program
//! whose tape is a device calls the twin and the mark rides the signature the
//! compiled routine was built from. Their bodies are deliberately the same
//! bodies — the graph-backed facades graft the SAME shared graphs, and the
//! two composition routines mirror their chains with every call retargeted to
//! the twin of its callee.
//!
//! Two claims are proven here, and they are different in kind:
//!
//! - **Functional equivalence** — the twin and its plain counterpart, run on
//!   the same seed across the full `-O0`/`-O1` × mono/frames/hybrid matrix,
//!   produce the same outcome and the same final tape. This is the durable
//!   claim. The hand-derived expectations stay in `stdlib_golden.rs`, which is
//!   where the plain side's correctness is earned; here the plain run IS the
//!   reference, so the twins can never silently diverge from it.
//! - **Byte identity** — today the twin's compiled code blob is byte-identical
//!   to its counterpart's, because no optimizer pass yet reasons about values
//!   on a non-volatile band. That one is explicitly temporary; see the
//!   obligation comment on the test.
//!
//! The structural mirror (names, signatures, contract clauses, shared graphs,
//! retargeted calls) is guarded in-crate, in `stdlib`'s own test module, where
//! the resolved module is reachable.
//!
//! Alphabets (index = position), as in `stdlib_golden.rs`:
//!   delimited `std::binaryNumbers::symbols`     `_`=0 `^`=1 `$`=2 `0`=3 `1`=4
//!   bare      `std::binaryNumbersBare::symbols` `_`=0 `0`=1 `1`=2

use std::sync::OnceLock;

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::{ObjectFile, SymbolDef};
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;
use mtc_turing_machine::stdlib;

const DELIM: &str = "alphabet a { '_', '^', '$', '0', '1' }";
const BARE: &str = "alphabet a { '_', '0', '1' }";

/// The two namespace pairs, plain first, with the alphabet declaration and
/// tape width a consumer of either side needs.
const PAIRS: [(&str, &str, &str, u32); 2] = [
    ("std::binaryNumbers", "std::binaryNumbersVolatile", DELIM, 5),
    (
        "std::binaryNumbersBare",
        "std::binaryNumbersBareVolatile",
        BARE,
        3,
    ),
];

/// Every routine of the delimited representation, with the seed
/// `stdlib_golden.rs` derives that routine's observable from.
const DELIM_CASES: [(&str, &[u8], i64, i64); 10] = [
    ("goToNumber", &[1, 4, 4, 2], 0, 0),
    ("goToNumbersStart", &[1, 4, 4, 2], 0, 3),
    ("goToNextNumber", &[1, 4, 2, 0, 1, 4, 3, 2], 0, 2),
    ("goToPreviousNumber", &[1, 4, 2, 0, 1, 4, 3, 2], 0, 7),
    ("deleteNumber", &[1, 4, 2], 0, 0),
    ("normalizeNumber", &[1, 3, 4, 3, 4, 2], 0, 0),
    ("plusOne", &[1, 4, 4, 4, 2], 0, 0),
    ("minusOneFast", &[1, 4, 3, 2], 0, 0),
    ("invertNumber", &[1, 4, 3, 4, 2], 0, 0),
    ("minusOne", &[1, 4, 3, 2], 0, 0),
];

/// Every routine of the bare representation, same discipline.
const BARE_CASES: [(&str, &[u8], i64, i64); 4] = [
    ("plusOne", &[2, 2, 2], 0, 0),
    ("minusOne", &[2, 1], 0, 0),
    ("invertNumber", &[1, 2], 0, 0),
    ("normalizeNumber", &[1, 1], 0, 0),
];

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

/// A consumer machine over `alphabet` (a full `alphabet a { … }` line) that
/// transparently calls `qualified` on its one tape and stops — the compiled
/// stdlib's consumption path, identical for a plain routine and its twin.
fn consumer(alphabet: &str, qualified: &str) -> String {
    format!(
        "{alphabet}\n\
         machine {{\n\
           tape num: a;\n\
           entry state s {{ [*] -> call {qualified}() then done; }}\n\
           state done {{ [*] -> stop; }}\n\
         }}\n"
    )
}

/// The stdlib object at `level`, compiled once per process per level. `-O1`
/// is what `stdlib::object()` caches; `-O0` is compiled here (both
/// `brk`-stripped, though the stdlib carries no `brk`).
fn stdlib_object(level: OptLevel) -> &'static ObjectFile {
    static O0: OnceLock<ObjectFile> = OnceLock::new();
    match level {
        OptLevel::O1 => stdlib::object(),
        OptLevel::O0 => O0.get_or_init(|| {
            compile(
                stdlib::SOURCE,
                CompileOptions {
                    opt_level: OptLevel::O0,
                    strip_debugger: true,
                    ..Default::default()
                },
            )
            .expect("the stdlib compiles at -O0")
            .object
        }),
    }
}

/// Compile the consumer at `level`, link it against the stdlib (also at
/// `level`) under `mech`.
fn build(src: &str, level: OptLevel, mech: CallMech) -> Executable {
    let consumer = compile(
        src,
        CompileOptions {
            opt_level: level,
            ..Default::default()
        },
    )
    .expect("the consumer compiles")
    .object;
    link(
        &[consumer],
        std::slice::from_ref(stdlib_object(level)),
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .expect("the consumer links against the stdlib")
    .executable
}

/// Run `exe` on one tape band, returning outcome and the final snapshot.
fn run_one(exe: &Executable, seed: &TapeSnapshot, width: u32) -> (Outcome, TapeSnapshot) {
    let mut tape = WideTape::from_snapshot(seed, width).expect("seed fits width");
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut devices: Vec<&mut dyn Tape> = vec![&mut tape as &mut dyn Tape];
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
    drop(devices);
    (result.outcome, tape.to_snapshot())
}

/// Every (plain, twin) qualified-name pair, with the seed and consumer shape
/// each needs — the fourteen routines, in source order.
fn cases() -> Vec<(String, String, &'static str, u32, TapeSnapshot)> {
    let mut out = Vec::new();
    for ((plain_ns, twin_ns, alphabet, width), roster) in PAIRS
        .iter()
        .zip([DELIM_CASES.as_slice(), BARE_CASES.as_slice()])
    {
        for (local, cells, origin, head) in roster {
            out.push((
                format!("{plain_ns}::{local}"),
                format!("{twin_ns}::{local}"),
                *alphabet,
                *width,
                snap(*origin, cells, *head),
            ));
        }
    }
    out
}

/// Family 2 — functional equivalence. Every twin, run on its counterpart's
/// golden seed, reproduces that counterpart's outcome and final tape across
/// the full `-O0`/`-O1` × mono/frames/hybrid matrix.
///
/// The plain run is the reference rather than a re-derived expectation:
/// `stdlib_golden.rs` already derives every one of these observables by hand
/// from the routine's contract, so restating them here would duplicate the
/// derivation without strengthening anything. What this adds is the pairing —
/// a twin that diverged from its counterpart in ANY of the six lowerings
/// fails, whatever the plain side happens to compute.
///
/// Tacts are deliberately not compared. Once a pass assumes values on a
/// non-volatile band, the twin is expected to cost more; the tape and the
/// outcome are what must never differ.
#[test]
fn every_twin_reproduces_its_counterparts_observables() {
    let cases = cases();
    assert_eq!(cases.len(), 14, "every exported routine has a twin case");

    for (plain, twin, alphabet, width, seed) in cases {
        for level in [OptLevel::O0, OptLevel::O1] {
            for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
                let plain_run = run_one(
                    &build(&consumer(alphabet, &plain), level, mech),
                    &seed,
                    width,
                );
                let twin_run = run_one(
                    &build(&consumer(alphabet, &twin), level, mech),
                    &seed,
                    width,
                );
                // Non-vacuity: a pair that trapped identically would other-
                // wise "agree" without either routine having run.
                assert_eq!(
                    plain_run.0,
                    Outcome::Stopped,
                    "{plain} ({level:?}/{mech:?}) runs to a stop"
                );
                assert_eq!(
                    plain_run, twin_run,
                    "{twin} diverges from {plain} ({level:?}/{mech:?})"
                );
            }
        }
    }
}

/// Family 3 — the byte-identity pin. Each twin's compiled code blob is
/// byte-identical to its plain counterpart's, at both optimization levels.
///
/// This holds because a twin IS its counterpart's body: the graph-backed
/// facades graft the same graphs, the two composition routines emit the same
/// instruction sequence with their call operands left as holes, and the
/// routine names live in the object's symbol table rather than in any blob.
/// The volatile mark changes what the compiler is ALLOWED to assume, and
/// today no pass assumes any of it.
#[test]
fn each_twin_blob_is_byte_identical_to_its_counterparts() {
    // TODAY-ONLY: the first optimizer pass that assumes values on non-volatile tapes MUST flip this test to functional-equivalence-only (family 2 stays; this byte pin goes) and say so in its release notes.
    for level in [OptLevel::O0, OptLevel::O1] {
        let object = stdlib_object(level);
        let blob_index = |name: &str| -> usize {
            let symbol = object
                .symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("{name} is an exported stdlib symbol"));
            match symbol.def {
                SymbolDef::Defined { blob } => blob as usize,
                _ => panic!("{name} is defined and exported by the stdlib object"),
            }
        };

        for (plain, twin, _, _, _) in cases() {
            let (p, t) = (blob_index(&plain), blob_index(&twin));
            // Non-vacuity: two symbols sharing one blob would make the
            // comparison below a blob against itself.
            assert_ne!(p, t, "{plain} and {twin} own distinct blobs ({level:?})");
            assert_eq!(
                object.blobs[p], object.blobs[t],
                "{twin}'s code blob differs from {plain}'s ({level:?})"
            );
        }
    }
}
