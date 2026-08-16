//! Spec R1: at -O1 the flagship executes ZERO post-dispatch
//! trampolines — an executed `jmp` reached as the target of `djmp`,
//! or of `jm` when taken (docs/tmt/optimizer.md (dispatch-target
//! threading)). A `jmp` merely following a NOT-taken `jm` (i.e. plain
//! fall-through into a `jmp` that happens to sit right after it) is not
//! a trampoline.
//!
//! Flagship build + tape seeding are cribbed from `tests/golden_programs.rs`;
//! the trace-walking session loop mirrors `cli/run.rs::drive_traced`
//! (`crates/turing-machine/src/cli/run.rs`).

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::asm::listing_line;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{
    ArchRegistry, DebugEvent, Machine, Outcome, PauseCause, RunLimits, RunOptions, Tape, WideTape,
};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{assemble, link, tm1_syntax};
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;

fn example_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/brainfuck-utm.tma")
}

fn tmc_example_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/brainfuck-utm.tmc")
}

/// The UTM's per-tape alphabet cardinalities (docs/examples/brainfuck-utm.tma
/// (routine)): prog 9 symbols, data/out 127 each, cnt 2.
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

/// Assemble + link the hand-written UTM once — the twin `build_flagship`
/// is compared against in the measurements table.
fn utm() -> Executable {
    let source = fs::read_to_string(example_path()).expect("example source present");
    let obj = assemble(&source, false).expect("the UTM assembles");
    link(&[obj], &[], LinkOptions::default())
        .expect("the UTM links")
        .executable
}

/// Compile + link the `.tmc` port of the UTM at `opt`, asserting it
/// compiles warning-free — the round's optimizer must never introduce a
/// diagnostic on the flagship.
fn build_flagship(opt: OptLevel) -> Executable {
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

/// A fresh four-tape band for `program`: the program encoded onto the prog
/// tape, the other three blank (mirrors `tests/golden_programs.rs::run_utm`'s
/// set-up).
fn seed_tapes(program: &str) -> (WideTape, WideTape, WideTape, WideTape) {
    let prog = encode(program);
    let prog_snap = TapeSnapshot {
        origin: 0,
        cells: prog,
        head: 0,
        alphabet: None,
    };
    let prog_tape = WideTape::from_snapshot(&prog_snap, WIDTHS[0]).expect("prog fits width 9");
    let data_tape = WideTape::new(WIDTHS[1]);
    let out_tape = WideTape::new(WIDTHS[2]);
    let cnt_tape = WideTape::new(WIDTHS[3]);
    (prog_tape, data_tape, out_tape, cnt_tape)
}

/// Per-run execution counters: how many `jmp`/`djmp`/`jm` instructions
/// actually retired, how many of the executed `jmp`s were post-dispatch
/// trampolines, and the total instruction count.
#[derive(Debug, Default, Clone, Copy)]
struct ExecStats {
    steps: u64,
    trampolines: u64,
    jmp: u64,
    djmp: u64,
    jm: u64,
}

/// The mnemonic token out of a `listing_line` rendering, given the decoded
/// instruction's byte length `len` (also returned by `listing_line`). The
/// rendered line is `"  {addr:04x}:  {bytes_hex:<15} {mnemonic:<8}{operand}"`
/// (trimmed at the end): token 0 is the address column, the next `len`
/// tokens are the hex byte pairs, and the token right after those is
/// exactly the mnemonic — precise by construction, unlike guessing from
/// token shape (a mnemonic can coincidentally look like a hex byte pair,
/// e.g. `jm`, but never at that fixed position).
fn mnemonic_at(line: &str, len: u32) -> &str {
    line.split_whitespace()
        .nth(1 + len as usize)
        .expect("a listing line always carries a mnemonic token")
}

/// Drive `exe` over a fresh tape band for `program` via a multi-tape debug
/// session, exactly as `cli/run.rs::drive_traced` does (`machine.debug_tapes`
/// / `session.ip()` / `session.step_in_tapes` / `listing_line`), tallying
/// per-mnemonic execution counts and post-dispatch trampolines along the
/// way. Asserts the run halts cleanly (`Outcome::Stopped`) — an unexpected
/// trap or resource-limit finish is a set-up bug, not a result to report.
fn exec_stats(exe: &Executable, program: &str) -> ExecStats {
    let syntax = tm1_syntax();
    let resolve = |_: u32| -> Option<String> { None };

    let (mut prog_tape, mut data_tape, mut out_tape, mut cnt_tape) = seed_tapes(program);

    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");

    let mut devices: Vec<&mut dyn Tape> =
        vec![&mut prog_tape, &mut data_tape, &mut out_tape, &mut cnt_tape];

    let mut session = machine.debug_tapes(RunOptions {
        limits: RunLimits {
            max_steps: Some(1_000_000),
            ..Default::default()
        },
        ..Default::default()
    });

    let mut stats = ExecStats::default();
    // (mnemonic, fetch address, instruction byte length) of the
    // previously executed instruction.
    let mut prev: Option<(String, u32, u32)> = None;

    loop {
        let ip = session.ip();
        assert!(
            (ip as usize) < exe.code.len(),
            "{program}: fetch address {ip:#06x} ran off the code image"
        );
        let (line, len) = listing_line(&syntax, &exe.code, ip, &resolve);
        let mnemonic = mnemonic_at(&line, len);

        match mnemonic {
            "jmp" => stats.jmp += 1,
            "djmp" => stats.djmp += 1,
            "jm" => stats.jm += 1,
            _ => {}
        }

        let is_trampoline = mnemonic == "jmp"
            && prev
                .as_ref()
                .is_some_and(|(pm, pip, plen)| pm == "djmp" || (pm == "jm" && ip != pip + plen));
        if is_trampoline {
            stats.trampolines += 1;
        }
        prev = Some((mnemonic.to_string(), ip, len));

        let event = session.step_in_tapes(&mut devices);
        stats.steps += 1;
        match event {
            DebugEvent::Paused(PauseCause::Trap(trap)) => {
                panic!(
                    "{program}: unexpected trap {trap:?} after {} steps",
                    stats.steps
                );
            }
            DebugEvent::Paused(_) => {}
            DebugEvent::Finished(outcome) => {
                assert_eq!(
                    outcome,
                    Outcome::Stopped,
                    "{program}: expected a clean stop, got {outcome:?}"
                );
                break;
            }
        }
    }

    stats
}

/// (trampoline count, total steps) — the shape the two acceptance tests
/// read.
fn trampolines(exe: &Executable, program: &str) -> (u64, u64) {
    let stats = exec_stats(exe, program);
    (stats.trampolines, stats.steps)
}

/// Total decoded instruction count across the whole code image (static —
/// how many instructions the image holds, not how many execute), walking
/// every instruction exactly once through the public `listing_line`
/// decoder.
fn instruction_count(exe: &Executable) -> u64 {
    let syntax = tm1_syntax();
    let resolve = |_: u32| -> Option<String> { None };
    let mut addr = 0u32;
    let mut count = 0u64;
    while (addr as usize) < exe.code.len() {
        let (_, len) = listing_line(&syntax, &exe.code, addr, &resolve);
        addr += len;
        count += 1;
    }
    count
}

#[test]
fn o1_flagship_executes_zero_trampolines() {
    let (count, _steps) = trampolines(&build_flagship(OptLevel::O1), "++[>+++<-]>.");
    assert_eq!(count, 0);
}

#[test]
fn o0_flagship_still_pays_them() {
    // The honest unoptimized baseline: strictly more than zero.
    let (count, _steps) = trampolines(&build_flagship(OptLevel::O0), "++[>+++<-]>.");
    assert!(count > 0);
}

/// The round's #32 measurements table: executed `jmp`/`djmp`/`jm`,
/// trampolines, steps, image bytes, and instruction count at `-O0` and
/// `-O1`, alongside the hand-written twin built the way
/// `tests/golden_programs.rs` builds it. Not an assertion — a report.
/// `cargo test -p mtc-turing-machine --test trampoline_gate measurements -- --ignored --nocapture`
#[test]
#[ignore = "prints the round's measurements table; run explicitly"]
fn measurements() {
    let program = "++[>+++<-]>.";

    let o0 = build_flagship(OptLevel::O0);
    let o1 = build_flagship(OptLevel::O1);
    let hand = utm();

    let o0_stats = exec_stats(&o0, program);
    let o1_stats = exec_stats(&o1, program);

    let row = |label: &str, o0: String, o1: String| {
        println!("{label:<28} {o0:>12} {o1:>12}");
    };

    println!("{:<28} {:>12} {:>12}", "metric", "-O0", "-O1");
    row(
        "executed jmp",
        o0_stats.jmp.to_string(),
        o1_stats.jmp.to_string(),
    );
    row(
        "executed djmp",
        o0_stats.djmp.to_string(),
        o1_stats.djmp.to_string(),
    );
    row(
        "executed jm",
        o0_stats.jm.to_string(),
        o1_stats.jm.to_string(),
    );
    row(
        "trampolines",
        o0_stats.trampolines.to_string(),
        o1_stats.trampolines.to_string(),
    );
    row(
        "steps",
        o0_stats.steps.to_string(),
        o1_stats.steps.to_string(),
    );
    row(
        "image bytes",
        o0.code.len().to_string(),
        o1.code.len().to_string(),
    );
    row(
        "static instruction count",
        instruction_count(&o0).to_string(),
        instruction_count(&o1).to_string(),
    );
    println!(
        "{:<28} {:>12} {:>12}",
        "hand-written twin",
        format!("{}B", hand.code.len()),
        format!("{}i", instruction_count(&hand))
    );
}
