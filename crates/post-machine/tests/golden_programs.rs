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
    ]
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
