//! Inline-cap sweep harness (docs/pmt/optimizer.md (inline)) — an
//! explicit-run measurement instrument, not a correctness suite. Prints
//! one row per cap in {6, 12, 24}: total executed steps, total image
//! bytes, and total instruction count across the PM corpus (the embedded
//! stdlib source + the golden `.pmc` programs on their committed
//! inputs). Corpus and build helpers crib `tests/golden_programs.rs`;
//! the instruction-count walk mirrors
//! `crates/turing-machine/tests/trampoline_gate.rs::instruction_count`.
//!
//! cargo test -p mtc-post-machine --test sweep -- --ignored --nocapture
//!
//! Set SWEEP_CAP=6|12|24 to run a single cap — the house per-cap-split
//! escape hatch for a sweep that ever grows slow enough to risk the
//! silent-command ceiling; this corpus is small enough that the default
//! (all three caps in one run) is normally the right choice.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

use mtc_core::asm::listing_line;
use mtc_core::formats::executable::Executable;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunLimits, RunOptions, Trap};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::{link, pm1_syntax};
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;
use mtc_post_machine::stdlib;

fn golden_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"))
}

/// Compile `pmc` at `-O1` with `inline_cap: Some(cap)`, link against the
/// SHIPPED embedded stdlib object (cap-independent — Task 8's plumbing
/// note: `stdlib::object()` is cached and never sees this override, which
/// is why the stdlib SOURCE gets its own corpus entry below).
fn build(pmc: &str, cap: usize) -> Executable {
    let source = fs::read_to_string(golden_dir().join(pmc)).expect("golden source");
    let out = compile(
        &source,
        CompileOptions {
            opt_level: OptLevel::O1,
            inline_cap: Some(cap),
            ..Default::default()
        },
    )
    .expect("compiles");
    link(
        &[out.object],
        std::slice::from_ref(stdlib::object()),
        LinkOptions::default(),
    )
    .expect("links")
    .executable
}

/// Total decoded instruction count across a code image, walking every
/// instruction exactly once through the public `listing_line` decoder.
fn instruction_count(code: &[u8]) -> u64 {
    let syntax = pm1_syntax();
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

/// The unique golden `.pmc` source files (excluding `test1.pmc`, handled
/// separately below) — one build, and so one bytes/instructions
/// contribution, per file however many cases run it.
fn corpus_files() -> &'static [&'static str] {
    &[
        "sum.pmc",
        "ty.pmc",
        "sum2.pmc",
        "ty2.pmc",
        "ex000001.pmc",
        "ex000002.pmc",
    ]
}

/// (source file, input cells, head) — the run inputs, cribbed from
/// `golden_programs.rs::cases()`. `ty.pmc` appears twice (two distinct
/// inputs over the same build, matching upstream: with marks and empty).
fn run_cases() -> Vec<(&'static str, Vec<bool>, i64)> {
    vec![
        ("sum.pmc", vec![true, true, true, false, true, true], 0),
        ("ty.pmc", vec![true, true, true], 0),
        ("ty.pmc", vec![], 0),
        ("sum2.pmc", vec![true, true, true, false, true, true], 0),
        ("ty2.pmc", vec![true, true, true], 0),
        ("ex000001.pmc", vec![true, true, true], 0),
        (
            "ex000002.pmc",
            vec![true, true, false, false, false, true, true, true],
            0,
        ),
    ]
}

/// Run `exe` to a clean stop, returning the executed step count.
fn run_stopped(exe: &Executable, cells: &[bool], head: i64) -> u64 {
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
    result.stats.steps
}

/// `test1.pmc` (the 2007 codegen smoke test) never terminates: left,
/// right, jump back. It is run under the SAME fixed step limit at every
/// cap so its step contribution to the total stays constant across the
/// sweep, isolating the cap's effect on this entry to the bytes and
/// instruction columns only (docs/pmt/optimizer.md (inline) — the
/// upstream test of the same name documents the same non-termination).
const TEST1_MAX_STEPS: u64 = 1_000;

fn test1_steps(exe: &Executable) -> u64 {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells([false; 0], 0, 0);
    let result = machine.run(
        &mut tape,
        RunOptions {
            limits: RunLimits {
                max_steps: Some(TEST1_MAX_STEPS),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    assert_eq!(result.outcome, Outcome::Trapped(Trap::StepLimit));
    assert_eq!(result.stats.steps, TEST1_MAX_STEPS);
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

    // The golden .pmc corpus: one build per unique source file
    // (bytes + instructions), one run per case (steps).
    let mut built: HashMap<&str, Executable> = HashMap::new();
    for &pmc in corpus_files() {
        let exe = build(pmc, cap);
        totals.bytes += exe.code.len() as u64;
        totals.instructions += instruction_count(&exe.code);
        built.insert(pmc, exe);
    }
    for (pmc, cells, head) in run_cases() {
        let exe = built.get(pmc).expect("built above");
        totals.steps += run_stopped(exe, &cells, head);
    }

    // test1.pmc: its own build (bytes/instructions vary with the cap),
    // fixed step-limit trap (steps constant).
    let test1_exe = build("test1.pmc", cap);
    totals.bytes += test1_exe.code.len() as u64;
    totals.instructions += instruction_count(&test1_exe.code);
    totals.steps += test1_steps(&test1_exe);

    // The embedded stdlib SOURCE, compiled directly through `compile()`
    // with the cap (mirroring `stdlib::object()`'s own build options —
    // -O1, debugger-stripped). It carries no `main`, so it contributes
    // bytes/instructions only; per-blob decode sums across every routine
    // the library exports.
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

/// The inline-cap sweep table (docs/pmt/optimizer.md (inline) — the
/// round's decision-rule input): one row per cap in {6, 12, 24},
/// corpus-wide totals. Not an assertion — a report the round's decision
/// rule (best step total, subject to a 5% image-growth ceiling over the
/// cap-6 baseline, ties toward the smaller cap) is applied to by hand.
/// cargo test -p mtc-post-machine --test sweep -- --ignored --nocapture
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
