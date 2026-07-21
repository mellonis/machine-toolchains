//! Derivation-first goldens for the brainfuck UTM
//! (docs/examples/brainfuck-utm.tma). A tiny independent brainfuck reference
//! interpreter derives the final four-tape state; the UTM's run must
//! reproduce it, and the committed `.tmt` goldens are byte-identical to the
//! derived snapshots. Mirrors the PM-1 `golden_programs.rs` discipline: the
//! goldens are regenerated FROM the derivation, never from run output.
//!
//! Run choice: the library `run_tapes` entry point on `WideTape` devices we
//! build directly (not the `tmt` CLI) — it hands back the raw tape snapshots
//! for byte-level comparison without the block/alphabet round-trip a CLI run
//! would impose.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, Trap, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{assemble, link};
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;

fn example_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/brainfuck-utm.tma")
}

/// The `.tmc` port of the same UTM — the high-level source for the identical
/// algorithm (docs/examples/brainfuck-utm.tmc).
fn tmc_example_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/brainfuck-utm.tmc")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// The UTM's per-tape alphabet cardinalities, from the example's `.routine`:
/// prog 9 symbols, data/out 127 each, cnt 2.
const WIDTHS: [u32; 4] = [9, 127, 127, 2];

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

/// The independent derivation: a brainfuck reference interpreter. Cells wrap
/// mod 127 to mirror the UTM's `%127` arithmetic; the data tape is unbounded
/// in both directions. Returns only the non-blank data cells (matching the
/// device's sparse storage), the final data pointer, and the output bytes.
struct BfRun {
    data: BTreeMap<i64, u8>,
    data_head: i64,
    out: Vec<u8>,
}

fn interpret(program: &str) -> BfRun {
    let ops: Vec<char> = program.chars().collect();
    let mut data: BTreeMap<i64, u8> = BTreeMap::new();
    let mut ptr: i64 = 0;
    let mut out = Vec::new();
    let mut ip = 0usize;
    while ip < ops.len() {
        match ops[ip] {
            // Wrap on a 0..=126 ring: +1 and +126 (== -1) both stay in `u8`.
            '+' => {
                let cell = data.entry(ptr).or_insert(0);
                *cell = (*cell + 1) % 127;
            }
            '-' => {
                let cell = data.entry(ptr).or_insert(0);
                *cell = (*cell + 126) % 127;
            }
            '<' => ptr -= 1,
            '>' => ptr += 1,
            '.' => out.push(data.get(&ptr).copied().unwrap_or(0)),
            '[' => {
                if data.get(&ptr).copied().unwrap_or(0) == 0 {
                    let mut depth = 1;
                    while depth > 0 {
                        ip += 1;
                        match ops[ip] {
                            '[' => depth += 1,
                            ']' => depth -= 1,
                            _ => {}
                        }
                    }
                }
            }
            ']' => {
                if data.get(&ptr).copied().unwrap_or(0) != 0 {
                    let mut depth = 1;
                    while depth > 0 {
                        ip -= 1;
                        match ops[ip] {
                            ']' => depth += 1,
                            '[' => depth -= 1,
                            _ => {}
                        }
                    }
                }
            }
            other => panic!("unsupported bf char {other:?}"),
        }
        ip += 1;
    }
    // Blank cells (value 0) are never stored on a tape — drop them so the
    // derived snapshot's span matches the device's.
    data.retain(|_, &mut v| v != 0);
    BfRun {
        data,
        data_head: ptr,
        out,
    }
}

/// The documented dense-window normalization (docs/formats.md (tape-block
/// snapshot)): span from `min(marks ∪ head)` to `max(marks ∪ head)`, blank
/// cells rendered as index 0. An independent re-implementation of the
/// device's `to_snapshot`, so the derived golden never routes through the
/// code under test.
fn snapshot(marks: &BTreeMap<i64, u8>, head: i64) -> TapeSnapshot {
    let lo = marks.keys().min().copied().unwrap_or(head).min(head);
    let hi = marks.keys().max().copied().unwrap_or(head).max(head);
    let cells = (lo..=hi)
        .map(|c| marks.get(&c).copied().unwrap_or(0))
        .collect();
    TapeSnapshot {
        origin: lo,
        cells,
        head,
        alphabet: None,
    }
}

/// The four final tape snapshots the UTM must leave, derived independently of
/// the toolchain:
///   [0] prog  unchanged; the head ends on the `'H'` sentinel (last cell) —
///             the UTM advances the program head once per executed instruction
///   [1] data  the reference interpreter's cells + final pointer
///   [2] out   byte k at position k, head one past the last write (each `'.'`
///             writes then steps the output head right)
///   [3] cnt   always blank, head home: every bracket-skip push is matched by
///             a pop, so the nesting counter returns to empty at halt
fn derive(program: &str) -> [TapeSnapshot; 4] {
    let prog = encode(program);
    let run = interpret(program);

    let prog_marks: BTreeMap<i64, u8> = prog
        .iter()
        .enumerate()
        .filter(|&(_, &v)| v != 0)
        .map(|(i, &v)| (i as i64, v))
        .collect();
    let prog_snap = snapshot(&prog_marks, prog.len() as i64 - 1);

    let data_snap = snapshot(&run.data, run.data_head);

    let out_marks: BTreeMap<i64, u8> = run
        .out
        .iter()
        .enumerate()
        .filter(|&(_, &v)| v != 0)
        .map(|(i, &v)| (i as i64, v))
        .collect();
    let out_snap = snapshot(&out_marks, run.out.len() as i64);

    let cnt_snap = snapshot(&BTreeMap::new(), 0);

    [prog_snap, data_snap, out_snap, cnt_snap]
}

/// Assemble + link the flagship example once.
fn utm() -> Executable {
    let source = fs::read_to_string(example_path()).expect("example source present");
    let obj = assemble(&source, false).expect("the UTM assembles");
    link(&[obj], &[], LinkOptions::default())
        .expect("the UTM links")
        .executable
}

/// Compile + link the `.tmc` port of the UTM at `opt`, asserting it compiles
/// warning-free. Yields an executable interchangeable with `utm()`'s: same
/// four-tape band, same alphabets, same observable behaviour.
fn utm_from_tmc(opt: OptLevel) -> Executable {
    let source = fs::read_to_string(tmc_example_path()).expect("the .tmc port is present");
    let out = compile(
        &source,
        CompileOptions {
            opt_level: opt,
            ..Default::default()
        },
    )
    .expect("the .tmc port compiles");
    assert!(
        out.report.diagnostics.is_empty(),
        "the .tmc port must compile warning-free at {opt:?}, got {:?}",
        out.report.diagnostics
    );
    link(&[out.object], &[], LinkOptions::default())
        .expect("the .tmc port links")
        .executable
}

/// Both optimization levels — the port's equivalence must not depend on the
/// optimizer being on or off.
fn opt_levels() -> [OptLevel; 2] {
    [OptLevel::O0, OptLevel::O1]
}

/// Run the UTM over a fresh four-tape band: the program encoded onto the prog
/// tape, the other three blank. Returns the outcome and the four final
/// snapshots.
fn run_utm(exe: &Executable, program: &str) -> (Outcome, [TapeSnapshot; 4]) {
    let prog = encode(program);
    let prog_snap = TapeSnapshot {
        origin: 0,
        cells: prog,
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
    drop(devices);

    let snaps = [
        prog_tape.to_snapshot(),
        data_tape.to_snapshot(),
        out_tape.to_snapshot(),
        cnt_tape.to_snapshot(),
    ];
    (result.outcome, snaps)
}

/// Wrap the four derived snapshots into an MT block for byte comparison. A
/// shared numeric alphabet spans every tape's symbols (the widest is 127);
/// the snapshots carry no per-tape override, so the block serializes in the
/// v1 shared-alphabet shape.
fn block(snaps: &[TapeSnapshot; 4]) -> TapeBlockFile {
    TapeBlockFile {
        alphabet: (0..127u32).map(|i| i.to_string()).collect(),
        tapes: snaps.to_vec(),
    }
}

/// (bf source, golden file, expected first output byte). The output byte is
/// spelled out so the test pins the reference interpreter to a hand-computed
/// value, not merely to itself.
///
/// Coverage note: the first two cases never enter a loop with a ZERO control
/// cell, so neither one exercises the skip-FORWARD scanner — `[` always finds
/// a non-zero cell and falls straight into the body. `bf_skip` closes that
/// hole with a loop that is skipped before it ever runs, and nests a second
/// loop inside the skipped body so the nesting counter is pushed twice and
/// popped twice: the pop-check is taken on both of its branches (still
/// nested, then empty). Without this case the whole forward-scan path —
/// five of the machine's states — is dead under test on both the assembly
/// and the `.tmc` port.
fn cases() -> Vec<(&'static str, &'static str, u8)> {
    vec![
        ("+++.", "bf_add.expected.tmt", 3),
        ("++[>+++<-]>.", "bf_loop.expected.tmt", 6),
        ("[[+]+]++.", "bf_skip.expected.tmt", 2),
    ]
}

#[test]
fn goldens_match_the_derived_snapshots_and_files() {
    let exe = utm();
    for (program, golden, expected_out) in cases() {
        let derived = derive(program);
        // The reference derivation itself yields the intended output byte.
        assert_eq!(
            derived[2].cells.first().copied(),
            Some(expected_out),
            "{program} should output {expected_out}"
        );

        let (outcome, actual) = run_utm(&exe, program);
        assert_eq!(
            outcome,
            Outcome::Stopped,
            "{program} halts via the 'H' sentinel (stp)"
        );
        // The UTM reproduces the independent derivation on every tape — the
        // prog tape unchanged with its head on 'H', the data + out tapes
        // matching the reference interpreter, the cnt tape blank.
        assert_eq!(
            actual, derived,
            "{program}: UTM tapes must match the reference derivation"
        );

        // The committed .tmt is byte-for-byte the derived block.
        let bytes = fs::read(golden_dir().join(golden)).expect("golden .tmt present");
        assert_eq!(
            bytes,
            block(&derived).to_bytes().unwrap(),
            "{golden} drifted"
        );
    }
}

/// The `.tmc` port reproduces the hand-written UTM exactly: same derivation,
/// same goldens, two independent implementations of one algorithm. The
/// assembly file is authored against the ISA; the `.tmc` file is authored
/// against the language and compiled down. Nothing here weakens or duplicates
/// the assembly path's own assertions above — this reruns the SAME cases and
/// the SAME committed `.tmt` bytes through the compiled executable.
#[test]
fn the_tmc_port_matches_the_same_goldens() {
    for opt in opt_levels() {
        let exe = utm_from_tmc(opt);
        for (program, golden, expected_out) in cases() {
            let derived = derive(program);
            assert_eq!(
                derived[2].cells.first().copied(),
                Some(expected_out),
                "{program} should output {expected_out}"
            );

            let (outcome, actual) = run_utm(&exe, program);
            assert_eq!(
                outcome,
                Outcome::Stopped,
                "{program} halts via the 'H' sentinel (stp) on the .tmc port at {opt:?}"
            );
            assert_eq!(
                actual, derived,
                "{program} at {opt:?}: the .tmc port's tapes must match the reference derivation"
            );

            // The very bytes the hand-written UTM is pinned to.
            let bytes = fs::read(golden_dir().join(golden)).expect("golden .tmt present");
            assert_eq!(
                bytes,
                block(&derived).to_bytes().unwrap(),
                "{golden} drifted under the .tmc port"
            );
        }
    }
}

/// The two implementations agree tape-for-tape on every case, asserted
/// directly against each other rather than only against the derivation.
#[test]
fn the_tmc_port_and_the_assembly_agree_tape_for_tape() {
    let asm_exe = utm();
    for opt in opt_levels() {
        let tmc_exe = utm_from_tmc(opt);
        for (program, _, _) in cases() {
            let (asm_outcome, asm_snaps) = run_utm(&asm_exe, program);
            let (tmc_outcome, tmc_snaps) = run_utm(&tmc_exe, program);
            assert_eq!(
                asm_outcome, tmc_outcome,
                "{program} at {opt:?}: outcomes differ"
            );
            assert_eq!(
                asm_snaps, tmc_snaps,
                "{program} at {opt:?}: final tapes differ"
            );
        }
    }
}

/// The free "invalid opcode" fault survives the port: `fetch` has no
/// catch-all rule, so the blank program symbol matches nothing and the
/// machine traps exactly as the hand-written dispatch table does.
#[test]
fn the_tmc_port_traps_on_a_program_symbol_with_no_rule() {
    for opt in opt_levels() {
        let exe = utm_from_tmc(opt);
        let mut registry = ArchRegistry::new();
        registry.register(Box::new(Tm1::new(exe.tape_count)));
        let machine = Machine::from_executable(&exe, &registry).expect("loads");

        let mut prog = WideTape::new(WIDTHS[0]);
        let mut data = WideTape::new(WIDTHS[1]);
        let mut out = WideTape::new(WIDTHS[2]);
        let mut cnt = WideTape::new(WIDTHS[3]);
        let mut devices: Vec<&mut dyn Tape> = vec![&mut prog, &mut data, &mut out, &mut cnt];
        let result = machine
            .run_tapes(
                &mut devices,
                RunOptions {
                    limits: RunLimits {
                        max_steps: Some(1_000),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .expect("run set-up ok");
        assert!(
            matches!(result.outcome, Outcome::Trapped(Trap::NoTransition { .. })),
            "the .tmc port traps on an unmatched program symbol at {opt:?}: {:?}",
            result.outcome
        );
    }
}

#[test]
fn a_program_symbol_with_no_dispatch_row_traps() {
    // A blank program tape reads symbol 0 (' ') at the first fetch: Tfetch has
    // rows for 1..=8 only, and no catch-all, so `mtc` leaves MR=0 and `djmp`
    // faults — the UTM's free "invalid opcode" trap, now proven.
    let exe = utm();
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");

    let mut prog = WideTape::new(WIDTHS[0]);
    let mut data = WideTape::new(WIDTHS[1]);
    let mut out = WideTape::new(WIDTHS[2]);
    let mut cnt = WideTape::new(WIDTHS[3]);
    let mut devices: Vec<&mut dyn Tape> = vec![&mut prog, &mut data, &mut out, &mut cnt];
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(1_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    assert!(
        matches!(result.outcome, Outcome::Trapped(Trap::NoTransition { .. })),
        "a blank program symbol traps with no applicable transition: {:?}",
        result.outcome
    );
}

/// The `+`/`-` handlers are single MODULAR fold rules — the ring wrap rides
/// in the `%`, so one rule covers all 127 values with no separate boundary
/// rule — and the compiled `-S` artifact folds the stamped 127-way arithmetic
/// tables back into `.rept` families so the generated assembly stays compact.
///
/// The source assertions match FULL write vectors, which only ever appear in a
/// rule body: the teaching prose quotes bare `{...}` folds inline, so a
/// substring like `{(v+1)%127}` alone would hit a comment and never go red.
#[test]
fn the_tmc_port_uses_modular_folds_and_compiles_compact() {
    let source = fs::read_to_string(tmc_example_path()).expect("the .tmc port is present");

    // The two arithmetic handlers are single modular-fold rules: increment
    // wraps in `(v+1)%127`, decrement in `(v+126)%127` (the non-negative
    // remainder, so a wrapping decrement adds 126 rather than subtracting one).
    assert!(
        source.contains("write [-, {(v+1)%127}, -, -]"),
        "'+' must be a single modular increment fold"
    );
    assert!(
        source.contains("write [-, {(v+126)%127}, -, -]"),
        "'-' must be a single modular decrement fold"
    );
    // ...and NOT the retired non-wrapping range + hand-named boundary rule.
    assert!(
        !source.contains("write [-, {v+1}, -, -]"),
        "the non-wrapping '+' range rule must be gone"
    );
    assert!(
        !source.contains("write [-, {v-1}, -, -]"),
        "the non-wrapping '-' range rule must be gone"
    );

    // The `-S` artifact folds the stamped arithmetic families back into `.rept`
    // loops, so the generated assembly stays compact: the hand-written `.tma`
    // is ~212 lines, unfolded codegen ~1659. Bounds, not exact counts — the
    // source fragments above already guard against a workaround revert.
    let out = compile(&source, CompileOptions::default()).expect("the .tmc port compiles");
    let rept_headers = out
        .tma
        .lines()
        .filter(|l| l.trim_start().starts_with(".rept"))
        .count();
    let total_lines = out.tma.lines().count();
    assert!(
        rept_headers >= 3,
        "expected >= 3 .rept families in -S, got {rept_headers}:\n{}",
        out.tma
    );
    assert!(
        total_lines < 400,
        "expected < 400 lines in -S, got {total_lines}"
    );
}

/// Regenerates the golden `.tmt` files FROM THE DERIVED SNAPSHOTS (never from
/// run output — derivation-first). Mirrors pmt's regen convention.
/// cargo test -p mtc-turing-machine --test golden_programs regen -- --ignored
#[test]
#[ignore = "writes the golden files; run explicitly"]
fn regen_goldens() {
    for (program, golden, _) in cases() {
        let derived = derive(program);
        fs::write(
            golden_dir().join(golden),
            block(&derived).to_bytes().unwrap(),
        )
        .unwrap();
    }
}
