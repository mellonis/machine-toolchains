use std::fs;
use std::path::Path;

use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunLimits, RunOptions};
use mtc_post_machine::arch::{DEFAULT_GLYPHS, Pm1};
use mtc_post_machine::asm::link;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;
use mtc_post_machine::stdlib;

fn golden_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"))
}

fn build(pmc: &str, opt: OptLevel) -> mtc_core::formats::executable::Executable {
    let source = fs::read_to_string(golden_dir().join(pmc)).expect("golden source");
    let out = compile(
        &source,
        CompileOptions {
            opt_level: opt,
            ..Default::default()
        },
    )
    .expect("compiles");
    assert!(
        out.report.diagnostics.is_empty(),
        "{:?}",
        out.report.diagnostics
    );
    link(
        &[out.object],
        std::slice::from_ref(stdlib::object()),
        LinkOptions::default(),
    )
    .expect("links")
    .executable
}

fn run(exe: &mtc_core::formats::executable::Executable, cells: &[bool], head: i64) -> InfiniteTape {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells(cells.iter().copied(), 0, head);
    let result = machine.run(
        &mut tape,
        RunOptions {
            limits: RunLimits {
                max_steps: Some(1_000_000),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    assert_eq!(result.outcome, Outcome::Stopped);
    tape
}

fn block(snapshot: TapeSnapshot) -> TapeBlockFile {
    TapeBlockFile {
        alphabet: DEFAULT_GLYPHS.iter().map(|g| g.to_string()).collect(),
        tapes: vec![snapshot],
    }
}

/// (source file, golden file, input cells, head, DERIVED final snapshot)
fn cases() -> Vec<(&'static str, &'static str, Vec<bool>, i64, TapeSnapshot)> {
    vec![
        (
            "sum.pmc",
            "sum.expected.pmt",
            vec![true, true, true, false, true, true],
            0,
            TapeSnapshot {
                origin: 0,
                cells: vec![1, 1, 1, 1],
                head: 0,
                alphabet: None,
            },
        ),
        (
            "ty.pmc",
            "ty.expected.pmt",
            vec![true, true, true],
            0,
            TapeSnapshot {
                origin: 0,
                cells: vec![1, 1],
                head: 0,
                alphabet: None,
            },
        ),
        (
            "ty.pmc",
            "ty_empty.expected.pmt",
            vec![],
            0,
            TapeSnapshot {
                origin: 0,
                cells: vec![0],
                head: 0,
                alphabet: None,
            },
        ),
        // sum2.pmc is the same instruction sequence as sum.pmc in the
        // comma-group syntax — same input, same derivation, same golden
        // file; the case asserts the restyled port behaves identically.
        (
            "sum2.pmc",
            "sum.expected.pmt",
            vec![true, true, true, false, true, true],
            0,
            TapeSnapshot {
                origin: 0,
                cells: vec![1, 1, 1, 1],
                head: 0,
                alphabet: None,
            },
        ),
        // ty2: one step left off the marks, stop — the snapshot span
        // grows to cover the head resting on the blank at -1.
        (
            "ty2.pmc",
            "ty2.expected.pmt",
            vec![true, true, true],
            0,
            TapeSnapshot {
                origin: -1,
                cells: vec![0, 1, 1, 1],
                head: -1,
                alphabet: None,
            },
        ),
        // ex000001: unary increment of the n+1-marks number under the
        // head — [1,1,1] (two) becomes [1,1,1,1] (three), head returned
        // to the first mark.
        (
            "ex000001.pmc",
            "ex000001.expected.pmt",
            vec![true, true, true],
            0,
            TapeSnapshot {
                origin: 0,
                cells: vec![1, 1, 1, 1],
                head: 0,
                alphabet: None,
            },
        ),
        // ex000002: sum across an arbitrary gap — one (2 marks) plus two
        // (3 marks) across a 3-blank gap is three (4 marks), head on the
        // first mark. Exercises the walk-the-second-number-left loop
        // over a wider gap than sum.pmc's single blank.
        (
            "ex000002.pmc",
            "ex000002.expected.pmt",
            vec![true, true, false, false, false, true, true, true],
            0,
            TapeSnapshot {
                origin: 0,
                cells: vec![1, 1, 1, 1],
                head: 0,
                alphabet: None,
            },
        ),
    ]
}

/// test1.pmc (the 2007 codegen smoke test) deliberately never
/// terminates: left, right, jump back. The historic artifact's meaning
/// is the loop itself, so the port's contract is the step-limit trap
/// with the tape left untouched — at any cap the head sits on -1 or 0
/// depending on parity, so the head is not asserted.
#[test]
fn test1_loops_until_the_step_limit() {
    for opt in [OptLevel::O0, OptLevel::O1] {
        let mut registry = ArchRegistry::new();
        registry.register(Box::new(Pm1));
        let machine = Machine::from_executable(&build("test1.pmc", opt), &registry).expect("loads");
        let mut tape = InfiniteTape::from_cells([false; 0], 0, 0);
        let result = machine.run(
            &mut tape,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(1_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert_eq!(
            result.outcome,
            Outcome::Trapped(mtc_core::vm::Trap::StepLimit),
            "at {opt:?}"
        );
        assert!(tape.marked_cells().is_empty(), "at {opt:?}");
    }
}

#[test]
fn goldens_match_the_derived_snapshots_and_files() {
    for (pmc, golden, cells, head, expected) in cases() {
        for opt in [OptLevel::O0, OptLevel::O1] {
            let tape = run(&build(pmc, opt), &cells, head);
            assert_eq!(tape.to_snapshot(), expected, "{pmc} at {opt:?}");
        }
        // the committed .pmt is byte-for-byte the derived block
        let bytes = fs::read(golden_dir().join(golden)).expect("golden .pmt present");
        assert_eq!(
            bytes,
            block(expected).to_bytes().unwrap(),
            "{golden} drifted"
        );
    }
}

// NOTE: no O1-shrinks assertion here — sum/ty's only optimizable code is
// `main`, where tail-call is exempt (tail_call.rs) and std is always built
// -O1; O0 and O1 user objects may be byte-identical. Shrink assertions
// live in opt_equivalence.rs where shrinkage is derived.

/// Regenerates the golden .pmt files FROM THE DERIVED SNAPSHOTS above
/// (never from run output — derivation-first).
/// cargo test -p mtc-post-machine --test golden_programs regen -- --ignored
#[test]
#[ignore = "writes the golden files; run explicitly"]
fn regen_goldens() {
    for (_, golden, _, _, expected) in cases() {
        fs::write(
            golden_dir().join(golden),
            block(expected).to_bytes().unwrap(),
        )
        .unwrap();
    }
}
