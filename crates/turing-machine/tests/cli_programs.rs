//! End-to-end `tmt` CLI tests, in-process: assemble → link → tape new →
//! tape set → run, asserting exit codes and the tape-new alphabet upgrade.
//! Mirrors the shape of the PM-1 `pmt` cli_programs tests.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::tapeblock::TapeBlockFile;
use mtc_turing_machine::cli::{execute, execute_with};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A one-tape program that reads its head, matches the single row `[1]`,
/// and dispatches to `done`. On a marked start cell MR=1 and `djmp` lands
/// on `done`; on a blank cell MR=0 and `djmp` traps (NoTransition). The
/// `done` mnemonic (stp / hlt) fixes the stopped-vs-halted outcome.
fn one_tape_program(terminal: &str) -> String {
    format!(
        "\
.routine main, tapes=1, alpha=(2)
.section tables
T0: .row [1]
D0: .targets done
.section code
.func main
        rd
        mtc  T0
        djmp D0
done:   {terminal}
"
    )
}

/// asm IN.tma → link → `IN.tmx`, returning the executable path.
fn asm_and_link(dir: &Path, stem: &str, source: &str) -> PathBuf {
    let src = dir.join(format!("{stem}.tma"));
    fs::write(&src, source).unwrap();
    let obj = dir.join(format!("{stem}.tmo"));
    execute(&args(&[
        "asm",
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]))
    .unwrap();
    let exe = dir.join(format!("{stem}.tmx"));
    execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "-o",
        exe.to_str().unwrap(),
    ]))
    .unwrap();
    exe
}

#[test]
fn version_reports_exactly_two_lines() {
    let out = execute(&args(&["--version"])).unwrap();
    assert_eq!(
        out.stdout,
        format!(
            "tmt {}\ntma dialect (tm-1) {}\n",
            env!("CARGO_PKG_VERSION"),
            mtc_turing_machine::TM1_TMA_DIALECT_VERSION
        )
    );
    assert_eq!(mtc_turing_machine::TM1_TMA_DIALECT_VERSION, "0.1");
    assert_eq!(out.code, 0);
}

#[test]
fn no_args_prints_usage() {
    let out = execute(&[]).unwrap();
    assert!(out.stdout.contains("USAGE: tmt"));
    assert_eq!(out.code, 0);
}

#[test]
fn unknown_subcommand_errors() {
    assert!(execute(&args(&["bogus"])).is_err());
}

#[test]
fn full_pipeline_marked_tape_stops_with_exit_0() {
    let dir = scratch("pipeline_stp");
    let exe = asm_and_link(&dir, "prog", &one_tape_program("stp"));

    // tape new --from mints a blank one-band template with a binary alphabet.
    let tape = dir.join("prog.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    // tape set marks the start cell so `rd; mtc T0` yields MR=1.
    execute(&args(&[
        "tape",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--tape",
        "0",
        "--cells",
        "1",
    ]))
    .unwrap();

    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0, "stopped program exits 0:\n{}", out.stdout);
    assert!(out.stdout.contains("Stopped"), "{}", out.stdout);
}

#[test]
fn halt_variant_exits_2() {
    let dir = scratch("pipeline_hlt");
    let exe = asm_and_link(&dir, "prog", &one_tape_program("hlt"));
    let tape = dir.join("prog.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "tape",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "1",
    ]))
    .unwrap();
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 2, "halted program exits 2:\n{}", out.stdout);
    assert!(out.stdout.contains("Halted"), "{}", out.stdout);
}

#[test]
fn blank_tape_mr0_djmp_traps_with_exit_3() {
    let dir = scratch("pipeline_trap");
    let exe = asm_and_link(&dir, "prog", &one_tape_program("stp"));
    // A blank tape reads 0, so `mtc T0` yields MR=0 and `djmp` traps.
    let tape = dir.join("prog.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 3, "trapped program exits 3:\n{}", out.stdout);
    assert!(out.stdout.contains("Trapped"), "{}", out.stdout);
}

#[test]
fn trace_streams_listing_lines_and_still_reports_the_outcome() {
    let dir = scratch("pipeline_trace");
    let exe = asm_and_link(&dir, "prog", &one_tape_program("stp"));
    let tape = dir.join("prog.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "tape",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "1",
    ]))
    .unwrap();

    // `--trace` streams per-instruction listing lines into the writer seam
    // (the bin passes stderr; here a Vec<u8>), while the CliOutput still
    // carries the outcome/stats/final tapes and the exit code.
    let mut trace = Vec::new();
    let out = execute_with(
        &args(&[
            "run",
            exe.to_str().unwrap(),
            "--tape",
            tape.to_str().unwrap(),
            "--trace",
        ]),
        &mut trace,
    )
    .unwrap();
    assert_eq!(out.code, 0, "traced run still exits 0:\n{}", out.stdout);
    let trace = String::from_utf8(trace).unwrap();
    assert!(trace.contains("rd"), "trace shows the read step:\n{trace}");
    assert!(
        trace.contains("MF=") && trace.contains("heads=["),
        "trace shows post-state:\n{trace}"
    );
    // Base-profile pin: the ` FR=<n>` suffix appears only under the frames
    // profile, so a base-profile image's trace must never carry it.
    assert!(
        !trace.contains("FR="),
        "base-profile trace must not carry the frames FR= suffix:\n{trace}"
    );
}

#[test]
fn tape_count_mismatch_is_a_tool_error_naming_both_numbers() {
    let dir = scratch("mismatch");
    // A one-tape image and a two-tape image; mint a two-band tape from the
    // latter, then run the former against it → the band count (2) does not
    // match the image's tape count (1).
    let one = asm_and_link(&dir, "one", &one_tape_program("stp"));
    let two_src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
        stp
";
    let two = asm_and_link(&dir, "two", two_src);
    let tape = dir.join("two.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        two.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();

    let err = execute(&args(&[
        "run",
        one.to_str().unwrap(),
        "--tape",
        tape.to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains('2'), "mismatch names the band count: {err}");
    assert!(err.contains('1'), "mismatch names the image count: {err}");
}

#[test]
fn wide_alphabet_tape_writes_a_symbol_beyond_binary_and_stops() {
    let dir = scratch("wide_alphabet");
    // A one-tape, 3-symbol program: write symbol 2 at the start cell, stop.
    // Under a physically two-symbol tape `wr [2]` would fault; the run builds
    // a width-3 `WideTape` from the band's effective alphabet, so it succeeds.
    let src = "\
.routine main, tapes=1, alpha=(3)
.section code
.func main
        wr   [2]
        stp
";
    let exe = asm_and_link(&dir, "prog", src);
    let tape = dir.join("prog.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();

    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(
        out.code, 0,
        "wide-alphabet write stops (exit 0):\n{}",
        out.stdout
    );
    assert!(out.stdout.contains("Stopped"), "{}", out.stdout);
    // The final tape shows the written symbol under its 3-glyph alphabet.
    assert!(
        out.stdout.contains("|2|"),
        "final tape carries the written symbol:\n{}",
        out.stdout
    );
}

#[test]
fn tape_new_sizes_per_tape_alphabets_from_cardinalities() {
    let dir = scratch("tape_new_alphabets");
    // A two-tape image with distinct cardinalities (2, 3); the minted MT
    // must carry a per-band alphabet sized to each.
    let src = "\
.routine main, tapes=2, alpha=(2, 3)
.section code
.func main
        stp
";
    let exe = asm_and_link(&dir, "prog", src);
    let tape = dir.join("prog.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();

    let block = TapeBlockFile::from_bytes(&fs::read(&tape).unwrap()).unwrap();
    assert_eq!(block.tapes.len(), 2);
    assert_eq!(
        block.tapes[0].alphabet.as_deref(),
        Some(["0", "1"].map(String::from).as_slice())
    );
    assert_eq!(
        block.tapes[1].alphabet.as_deref(),
        Some(["0", "1", "2"].map(String::from).as_slice())
    );
}
