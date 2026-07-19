//! End-to-end `tmt` CLI tests, in-process: assemble → link → tape new →
//! tape set → run, asserting exit codes and the tape-new alphabet upgrade.
//! Mirrors the shape of the PM-1 `pmt` cli_programs tests.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_turing_machine::cli::{execute, execute_with};
use mtc_turing_machine::ir::IrProgram;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A committed `.tmc` fixture under `tests/golden/` (the Appendix A set +
/// the nested-graft case), shared with `tmc_golden.rs`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

/// `compile FIXTURE.tmc -> stem.tmo`, `link -> stem.tmx` (default mech).
/// Returns the executable path — the shared prologue for the `.tmc`
/// pipeline tests below.
fn compile_and_link(dir: &Path, stem: &str, fixture_name: &str) -> PathBuf {
    let obj = dir.join(format!("{stem}.tmo"));
    execute(&args(&[
        "compile",
        fixture(fixture_name).to_str().unwrap(),
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
fn version_reports_tool_language_and_dialect() {
    let out = execute(&args(&["--version"])).unwrap();
    // Line order mirrors `pmt --version`: tool / language / dialect.
    assert_eq!(
        out.stdout,
        format!(
            "tmt {}\ntmc language {}\ntma dialect (tm-1) {}\n",
            env!("CARGO_PKG_VERSION"),
            mtc_turing_machine::TMC_LANG_VERSION,
            mtc_turing_machine::TM1_TMA_DIALECT_VERSION
        )
    );
    assert_eq!(mtc_turing_machine::TMC_LANG_VERSION, "0.1");
    assert_eq!(mtc_turing_machine::TM1_TMA_DIALECT_VERSION, "0.3");
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

/// Assemble `one_tape_program("stp")` into `stem.tmo`, returning the obj
/// path — the shared prologue for the `link` flag tests below.
fn asm_one_tape(dir: &Path, stem: &str) -> PathBuf {
    let src = dir.join(format!("{stem}.tma"));
    fs::write(&src, one_tape_program("stp")).unwrap();
    let obj = dir.join(format!("{stem}.tmo"));
    execute(&args(&[
        "asm",
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]))
    .unwrap();
    obj
}

#[test]
fn link_accepts_entry_and_call_mech_flags() {
    let dir = scratch("link_flags");
    let obj = asm_one_tape(&dir, "p");
    let exe = dir.join("p.tmx");
    // --call-mech is carried, not yet consumed: all three link identically;
    // --entry main is the default made explicit.
    for mech in ["mono", "frames", "hybrid"] {
        execute(&args(&[
            "link",
            obj.to_str().unwrap(),
            "-o",
            exe.to_str().unwrap(),
            "--call-mech",
            mech,
            "--entry",
            "main",
        ]))
        .unwrap_or_else(|e| panic!("links with --call-mech {mech}: {e}"));
        assert!(exe.exists());
    }
}

#[test]
fn link_rejects_an_unknown_call_mech_listing_the_three() {
    let dir = scratch("link_bad_mech");
    let obj = asm_one_tape(&dir, "p");
    let exe = dir.join("p.tmx");
    let err = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "-o",
        exe.to_str().unwrap(),
        "--call-mech",
        "bogus",
    ]))
    .unwrap_err();
    assert!(
        err.contains("mono") && err.contains("frames") && err.contains("hybrid"),
        "error should list the three mechanisms: {err}"
    );
}

#[test]
fn link_unknown_entry_is_reported_by_name() {
    let dir = scratch("link_bad_entry");
    let obj = asm_one_tape(&dir, "p");
    let exe = dir.join("p.tmx");
    let err = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "-o",
        exe.to_str().unwrap(),
        "--entry",
        "nope",
    ]))
    .unwrap_err();
    assert!(
        err.contains("nope"),
        "error should name the missing entry: {err}"
    );
}

#[test]
fn link_help_lists_the_new_flags() {
    let out = execute(&args(&["link", "--help"])).unwrap();
    assert!(out.stdout.contains("--entry"), "{}", out.stdout);
    assert!(out.stdout.contains("--call-mech"), "{}", out.stdout);
}

// ── .tmc compile → link → run pipeline (exit codes across A.1/A.4/A.5) ──────

#[test]
fn compile_link_run_a1_stops_with_exit_0() {
    let dir = scratch("tmc_a1");
    let exe = compile_and_link(&dir, "a1", "a1_replace_b.tmc");
    let tape = dir.join("a1.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    // ab card 3 → labels "0"/"1"/"2"; seed "bab" = indices [2,1,2], head 0.
    execute(&args(&[
        "tape",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "212",
    ]))
    .unwrap();
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0, "A.1 stops (exit 0):\n{}", out.stdout);
    assert!(out.stdout.contains("Stopped"), "{}", out.stdout);
}

#[test]
fn compile_link_run_a4_overflow_halts_with_exit_2() {
    let dir = scratch("tmc_a4");
    let exe = compile_and_link(&dir, "a4", "a4_byte_increment.tmc");
    // A.4's `bytes` alphabet is 127 wide; the overflow value 126 has the
    // multi-char glyph "126", which `tape set --cells` (one char per cell)
    // cannot spell — so build the one-cell seed block directly.
    let tape = dir.join("a4.tmt");
    let block = TapeBlockFile {
        alphabet: (0..127u32).map(|i| i.to_string()).collect(),
        tapes: vec![TapeSnapshot {
            origin: 0,
            cells: vec![126],
            head: 0,
            alphabet: None,
        }],
    };
    fs::write(&tape, block.to_bytes()).unwrap();
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 2, "A.4 overflow halts (exit 2):\n{}", out.stdout);
    assert!(out.stdout.contains("Halted"), "{}", out.stdout);
}

#[test]
fn compile_link_run_a5_holey_read_traps_with_exit_3() {
    let dir = scratch("tmc_a5");
    // Default mech (hybrid); the trap is mode-independent.
    let exe = compile_and_link(&dir, "a5", "a5_call_across_alphabets.tmc");
    let tape = dir.join("a5.tmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    // ctl (tape 0, card 3): index 2 = '1' triggers the call.
    execute(&args(&[
        "tape",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--tape",
        "0",
        "--cells",
        "2",
    ]))
    .unwrap();
    // data (tape 1, card 5): index 1 = 'a', a holey wide symbol → unmapped-read.
    execute(&args(&[
        "tape",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--tape",
        "1",
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
    assert_eq!(
        out.code, 3,
        "A.5 holey read traps (exit 3):\n{}",
        out.stdout
    );
    assert!(out.stdout.contains("Trapped"), "{}", out.stdout);
}

// ── compile flags: --emit-ir, -S, -Werror, ir graph ─────────────────────────

#[test]
fn compile_emit_ir_writes_a_version_2_sidecar() {
    let dir = scratch("tmc_emit_ir");
    let obj = dir.join("a1.tmo");
    execute(&args(&[
        "compile",
        fixture("a1_replace_b.tmc").to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
        "--emit-ir",
    ]))
    .unwrap();
    let ir_path = dir.join("a1.ir.json");
    assert!(ir_path.exists(), "the --emit-ir sidecar is written");
    let text = fs::read_to_string(&ir_path).unwrap();
    let program = IrProgram::from_json(&text).expect("the sidecar parses as IR JSON");
    assert_eq!(program.version, 2, "IR version 2");
    assert!(program.worlds.iter().any(|w| w.name == "main"));
}

#[test]
fn compile_emit_ir_after_pass_errors_naming_valid_stages() {
    let dir = scratch("tmc_emit_ir_bad");
    let obj = dir.join("a1.tmo");
    // No optimizer passes ship in 6a, so `after:<pass>` never matches the
    // (empty) registry; the error names the stages that do exist.
    let err = execute(&args(&[
        "compile",
        fixture("a1_replace_b.tmc").to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
        "--emit-ir=after:inline",
    ]))
    .unwrap_err();
    assert!(err.contains("lowered") && err.contains("final"), "{err}");
    assert!(err.contains("after:inline"), "{err}");
}

#[test]
fn compile_accepts_and_consumes_foutline() {
    // `--foutline` must be a recognised flag: the hand-rolled parser rejects
    // any leftover dashed token as an "unknown flag", so a clean compile here
    // proves the flag reached `CompileOptions` (was consumed) rather than
    // falling through to the positional check. Enabling outline is inert until
    // the pass registers, so the object still writes normally.
    let dir = scratch("tmc_foutline");
    let obj = dir.join("a1.tmo");
    execute(&args(&[
        "compile",
        fixture("a1_replace_b.tmc").to_str().unwrap(),
        "-O1",
        "--foutline",
        "-o",
        obj.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("--foutline must be accepted: {e}"));
    assert!(obj.exists(), "the object is written with --foutline set");
}

#[test]
fn compile_dash_s_emits_reassemblable_tma() {
    let dir = scratch("tmc_dash_s");
    let tma = dir.join("a1.tma");
    execute(&args(&[
        "compile",
        fixture("a1_replace_b.tmc").to_str().unwrap(),
        "-S",
        "-o",
        tma.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(tma.exists(), "the -S .tma text is written");
    // The emitted assembly re-assembles cleanly through `tmt asm`.
    let obj = dir.join("a1.tmo");
    execute(&args(&[
        "asm",
        tma.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("emitted .tma must re-assemble: {e}"));
    assert!(obj.exists());
}

#[test]
fn compile_werror_escalates_a_warning() {
    let dir = scratch("tmc_werror");
    // A local (unexported), uncalled routine draws an `unused-routine` warning.
    let src = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
";
    let srcpath = dir.join("warn.tmc");
    fs::write(&srcpath, src).unwrap();
    let obj = dir.join("warn.tmo");
    // Plain compile: succeeds, the warning renders on stderr.
    let out = execute(&args(&[
        "compile",
        srcpath.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        out.stderr.contains("warning:") && out.stderr.contains("helper"),
        "plain compile warns: {}",
        out.stderr
    );
    // -Werror: the same warning is now fatal.
    let err = execute(&args(&[
        "compile",
        srcpath.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
        "-Werror",
    ]))
    .unwrap_err();
    assert!(
        err.contains("treated as errors"),
        "-Werror escalates: {err}"
    );
}

#[test]
fn ir_graph_renders_mermaid_and_filters_by_world() {
    let dir = scratch("tmc_ir_graph");
    let obj = dir.join("a1.tmo");
    execute(&args(&[
        "compile",
        fixture("a1_replace_b.tmc").to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
        "--emit-ir",
    ]))
    .unwrap();
    let ir_path = dir.join("a1.ir.json");
    let out = execute(&args(&["ir", "graph", ir_path.to_str().unwrap()])).unwrap();
    assert!(out.stdout.contains("flowchart TD"), "{}", out.stdout);
    assert!(out.stdout.contains("%% main"), "{}", out.stdout);
    // `--function` (pmt's flag name) filters by world name; a miss is by name.
    let err = execute(&args(&[
        "ir",
        "graph",
        ir_path.to_str().unwrap(),
        "--function",
        "nope",
    ]))
    .unwrap_err();
    assert!(err.contains("nope"), "{err}");
}
