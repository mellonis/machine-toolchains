//! Inline-cap sweep harness (docs/tmt/optimizer.md (inline)) — an
//! explicit-run measurement instrument, not a correctness suite. Prints
//! one row per cap in {6, 12, 24}: total executed steps, total image
//! bytes, and total instruction count across the TM corpus (the
//! flagship UTM's `.tmc` port on the pinned `++[>+++<-]>.` run, plus the
//! embedded stdlib source — which carries both stdlib twins,
//! `std::binaryNumbers` and `std::binaryNumbersBare`, each with its
//! volatile mirror, in one file). Build helpers crib
//! `tests/golden_programs.rs`; the run-and-tally shape and the
//! instruction-count walk crib `tests/trampoline_gate.rs`.
//!
//! cargo test -p mtc-turing-machine --test sweep -- --ignored --nocapture
//!
//! Set SWEEP_CAP=6|12|24 to run a single cap — the house per-cap-split
//! escape hatch for a sweep that ever grows slow enough to risk the
//! silent-command ceiling; this corpus is small enough that the default
//! (all three caps in one run) is normally the right choice.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::asm::listing_line;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;
use mtc_turing_machine::stdlib;
use mtc_turing_machine::tm1_syntax;

fn tmc_example_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/brainfuck-utm.tmc")
}

/// The UTM's per-tape alphabet cardinalities (docs/examples/brainfuck-utm.tma
/// (routine)): prog 9 symbols, data/out 127 each, cnt 2.
const WIDTHS: [u32; 4] = [9, 127, 127, 2];

/// The sweep program pinned by the round's ruled inline-threshold decision
/// rule.
const PROGRAM: &str = "++[>+++<-]>.";

/// bf source → prog-tape symbol indices, plus the `'H'` sentinel (index 8)
/// the UTM halts on (docs/examples/brainfuck-utm.tma alphabet:
/// 0=' ' 1='+' 2='-' 3='<' 4='>' 5='.' 6='[' 7=']' 8='H').
fn encode(program: &str) -> Vec<u8> {
    let mut prog: Vec<u8> = program
        .chars()
        .map(|c| match c {
            '+' => 1,
            '-' => 2,
            '<' => 3,
            '>' => 4,
            '.' => 5,
            '[' => 6,
            ']' => 7,
            other => panic!("unsupported bf char {other:?}"),
        })
        .collect();
    prog.push(8); // the 'H' sentinel
    prog
}

/// Compile the `.tmc` port of the UTM at `-O1` with `inline_cap: Some(cap)`
/// and link it standalone (no external libraries — the flagship is
/// self-contained), asserting a warning-free compile.
fn build_flagship(cap: usize) -> Executable {
    let source = fs::read_to_string(tmc_example_path()).expect("the .tmc port is present");
    let out = compile(
        &source,
        CompileOptions {
            opt_level: OptLevel::O1,
            inline_cap: Some(cap),
            ..Default::default()
        },
    )
    .expect("the .tmc port compiles");
    assert!(
        out.report.diagnostics.is_empty(),
        "the .tmc port must compile warning-free, got {:?}",
        out.report.diagnostics
    );
    link(&[out.object], &[], LinkOptions::default())
        .expect("the .tmc port links")
        .executable
}

/// Total decoded instruction count across a code image, walking every
/// instruction exactly once through the public `listing_line` decoder.
fn instruction_count(code: &[u8]) -> u64 {
    let syntax = tm1_syntax();
    let resolve = |_: u32| -> Option<String> { None };
    let mut addr = 0u32;
    let mut count = 0u64;
    while (addr as usize) < code.len() {
        let (_, len) = listing_line(&syntax, code, addr, &resolve);
        addr += len;
        count += 1;
    }
    count
}

/// Run the flagship over a fresh four-tape band on the pinned program,
/// asserting a clean stop, and return the executed step count.
fn run_flagship_steps(exe: &Executable) -> u64 {
    let prog_snap = TapeSnapshot {
        origin: 0,
        cells: encode(PROGRAM),
        head: 0,
        alphabet: None,
    };
    let mut prog_tape = WideTape::from_snapshot(&prog_snap, WIDTHS[0]).expect("prog fits width 9");
    let mut data_tape = WideTape::new(WIDTHS[1]);
    let mut out_tape = WideTape::new(WIDTHS[2]);
    let mut cnt_tape = WideTape::new(WIDTHS[3]);

    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");

    let mut devices: Vec<&mut dyn Tape> =
        vec![&mut prog_tape, &mut data_tape, &mut out_tape, &mut cnt_tape];
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
    assert_eq!(result.outcome, Outcome::Stopped, "{PROGRAM} halts (stp)");
    result.stats.steps
}

#[derive(Debug, Default, Clone, Copy)]
struct Totals {
    steps: u64,
    bytes: u64,
    instructions: u64,
}

fn sweep_cap(cap: usize) -> Totals {
    let mut totals = Totals::default();

    let flagship = build_flagship(cap);
    totals.bytes += flagship.code.len() as u64;
    totals.instructions += instruction_count(&flagship.code);
    totals.steps += run_flagship_steps(&flagship);

    // The embedded stdlib SOURCE, carrying BOTH stdlib twins (the
    // delimited std::binaryNumbers and bare std::binaryNumbersBare
    // namespaces, each with its volatile mirror) in one file — compiled
    // directly through `compile()` with the cap (mirroring
    // `stdlib::object()`'s own build options: -O1, debugger-stripped).
    // No `machine` block, so bytes/instructions only.
    let stdlib_out = compile(
        stdlib::SOURCE,
        CompileOptions {
            opt_level: OptLevel::O1,
            strip_debugger: true,
            inline_cap: Some(cap),
            ..Default::default()
        },
    )
    .expect("the stdlib compiles");
    for blob in &stdlib_out.object.blobs {
        totals.bytes += blob.len() as u64;
        totals.instructions += instruction_count(blob);
    }

    totals
}

fn sweep_caps() -> Vec<usize> {
    match env::var("SWEEP_CAP") {
        Ok(v) => vec![v.parse().expect("SWEEP_CAP is 6, 12, or 24")],
        Err(_) => vec![6, 12, 24],
    }
}

/// The inline-cap sweep table (docs/tmt/optimizer.md (inline) — the
/// round's decision-rule input): one row per cap in {6, 12, 24},
/// corpus-wide totals. Not an assertion — a report the round's decision
/// rule (best step total, subject to a 5% image-growth ceiling over the
/// cap-6 baseline, ties toward the smaller cap) is applied to by hand.
/// cargo test -p mtc-turing-machine --test sweep -- --ignored --nocapture
#[test]
#[ignore = "prints the sweep's measurements table; run explicitly"]
fn sweep() {
    println!(
        "{:<6} {:>14} {:>14} {:>14}",
        "cap", "total steps", "total bytes", "total instrs"
    );
    for cap in sweep_caps() {
        let totals = sweep_cap(cap);
        println!(
            "{:<6} {:>14} {:>14} {:>14}",
            cap, totals.steps, totals.bytes, totals.instructions
        );
    }
}
