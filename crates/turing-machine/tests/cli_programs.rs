//! End-to-end `tmt` CLI tests, in-process: assemble → link → tape new →
//! tape set → run, asserting exit codes and the tape-new alphabet upgrade.
//! Mirrors the shape of the PM-1 `pmt` cli_programs tests.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::ARCH_PM1;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
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
fn dis_refuses_a_foreign_architecture_executable() {
    // A .tmx stamped with PM-1's arch byte: `run` already refuses this
    // (`unknown architecture 0x01`); `dis` must refuse it the same way
    // instead of decoding the code section against TM-1's opcode table.
    let dir = scratch("dis_foreign_exe");
    let exe = Executable::code_only(ARCH_PM1, 0, vec![0x0D, 0x02]);
    let path = dir.join("foreign.tmx");
    fs::write(&path, exe.to_bytes()).unwrap();
    let err = execute(&args(&["dis", path.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("unknown architecture 0x01"), "{err}");
}

#[test]
fn dis_refuses_a_foreign_architecture_object() {
    let dir = scratch("dis_foreign_obj");
    let obj = ObjectFile::v2(ARCH_PM1, Vec::new(), Vec::new(), Vec::new(), None);
    let path = dir.join("foreign.tmo");
    fs::write(&path, obj.to_bytes()).unwrap();
    let err = execute(&args(&["dis", path.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("unknown architecture 0x01"), "{err}");
}

/// The embedded standard library auto-links: a program that transparently
/// calls `std::binaryNumbersBare::plusOne` links WITHOUT any `-l` flag (the
/// linker pulls the reachable routine out of the auto-added stdlib object),
/// and runs to a stop. Passing `--nostdlib` removes the auto-link, so the
/// same object fails to link with the symbol unresolved.
///
/// The call is bindingless (transparent, same-shape tape): a cross-unit
/// call that BOUND a tape into a stdlib routine would need the routine's
/// signature at compile time and is rejected (`external-binding-unsupported`),
/// so identity/same-alphabet transparent calls are the compiled stdlib's
/// consumption path.
#[test]
fn stdlib_auto_links_and_nostdlib_opts_out() {
    let dir = scratch("stdlib_autolink");
    let src = dir.join("consumer.tmc");
    fs::write(
        &src,
        "alphabet bits { '_', '0', '1' }\n\
         machine {\n\
           tape num: bits;\n\
           entry state start { [*] -> call std::binaryNumbersBare::plusOne() then done; }\n\
           state done { [*] -> stop; }\n\
         }\n",
    )
    .unwrap();
    let obj = dir.join("consumer.tmo");
    execute(&args(&[
        "compile",
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]))
    .unwrap();

    // link WITHOUT -l → the stdlib auto-links and resolves plusOne.
    let exe = dir.join("consumer.tmx");
    execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "-o",
        exe.to_str().unwrap(),
    ]))
    .expect("stdlib auto-links plusOne without -l");

    // ...and the program runs: plusOne on "1" (leftmost digit at the head)
    // yields "10", stopping with exit 0.
    let tape = dir.join("consumer.tmt");
    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "tape-block",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "0='1'",
    ]))
    .unwrap();
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0, "the stdlib call stops:\n{}", out.stdout);
    assert!(out.stdout.contains("Stopped"), "{}", out.stdout);

    // link WITH --nostdlib → plusOne is unresolved.
    let err = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("consumer_nostd.tmx").to_str().unwrap(),
    ]))
    .expect_err("--nostdlib leaves plusOne unresolved");
    assert!(
        err.contains("unresolved") && err.contains("std::binaryNumbersBare::plusOne"),
        "unexpected error: {err}"
    );
}

#[test]
fn full_pipeline_marked_tape_stops_with_exit_0() {
    let dir = scratch("pipeline_stp");
    let exe = asm_and_link(&dir, "prog", &one_tape_program("stp"));

    // tape new --from mints a blank one-band template with a binary alphabet.
    let tape = dir.join("prog.tmt");
    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    // tape set marks the start cell so `rd; mtc T0` yields MR=1.
    execute(&args(&[
        "tape-block",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "0='1'",
    ]))
    .unwrap();

    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
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
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "tape-block",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "0='1'",
    ]))
    .unwrap();
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
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
        "tape-block",
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
        "--tape-block",
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
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "tape-block",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "0='1'",
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
            "--tape-block",
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
        "tape-block",
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
        "--tape-block",
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
        "tape-block",
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
        "--tape-block",
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
        "tape-block",
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

/// A routine declaring an alphabet wider than the MT tape-block's
/// byte-sized glyph-count field (300 > 255) assembles and links cleanly
/// (the executable header stores cardinalities as `u32`); `tape new`
/// minting the per-band alphabet is where the width has to fit a wire
/// `u8`, so it must surface a normal CLI error naming both numbers —
/// never panic.
#[test]
fn tape_new_rejects_an_oversize_alphabet_without_panicking() {
    let dir = scratch("tape_new_oversize_alphabet");
    let src = "\
.routine main, tapes=1, alpha=(300)
.section code
.func main
        stp
";
    let exe = asm_and_link(&dir, "oversize", src);
    let tape = dir.join("oversize.tmt");
    let err = execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .expect_err("an oversize alphabet must be a typed CLI error, not a panic");
    assert!(err.contains("300"), "{err}");
    assert!(err.contains("255"), "{err}");
    assert!(!tape.exists(), "no partial .tmt should be written on error");
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
    // `--nostdlib` (T6) is a link flag: its usage row must be present.
    assert!(out.stdout.contains("--nostdlib"), "{}", out.stdout);
}

#[test]
fn compile_help_lists_foutline() {
    // `--foutline` (T1) is a compile flag; its usage row must be present in
    // `tmt compile --help`.
    let out = execute(&args(&["compile", "--help"])).unwrap();
    assert!(out.stdout.contains("--foutline"), "{}", out.stdout);
}

// ── .tmc compile → link → run pipeline (exit codes across A.1/A.4/A.5) ──────

#[test]
fn compile_link_run_a1_stops_with_exit_0() {
    let dir = scratch("tmc_a1");
    let exe = compile_and_link(&dir, "a1", "a1_replace_b.tmc");
    let tape = dir.join("a1.tmt");
    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    // ab card 3 → labels "0"/"1"/"2"; seed "bab" = indices [2,1,2], head 0.
    execute(&args(&[
        "tape-block",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "0='2','1','2'",
    ]))
    .unwrap();
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
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
    fs::write(&tape, block.to_bytes().unwrap()).unwrap();
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
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
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        tape.to_str().unwrap(),
    ]))
    .unwrap();
    // ctl (tape 0, card 3): index 2 = '1' triggers the call.
    execute(&args(&[
        "tape-block",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "0='2'",
    ]))
    .unwrap();
    // data (tape 1, card 5): index 1 = 'a', a holey wide symbol → unmapped-read.
    execute(&args(&[
        "tape-block",
        "set",
        tape.to_str().unwrap(),
        "--in-place",
        "--cells",
        "1='1'",
    ]))
    .unwrap();
    let out = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
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
fn compile_emit_ir_after_a_real_pass_writes_a_version_2_snapshot() {
    let dir = scratch("tmc_emit_ir_after");
    // A forwarder program: `scan` hops to the empty forwarder `hop`, which
    // `jump-threading` retargets away at -O1 — so `after:jump-threading` names
    // a snapshot that is actually captured (the pass fires). The snapshot must
    // parse back as version-2 IR JSON.
    let src = "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state scan {
    ['a'] -> move [>] goto scan;
    ['_'] -> goto hop;
  }
  state finish { [*] -> write ['a'] stop; }
  state hop    { [*] -> goto finish; }
}
";
    let srcpath = dir.join("fwd.tmc");
    fs::write(&srcpath, src).unwrap();
    let obj = dir.join("fwd.tmo");
    execute(&args(&[
        "compile",
        srcpath.to_str().unwrap(),
        "-O1",
        "--emit-ir=after:jump-threading",
        "-o",
        obj.to_str().unwrap(),
    ]))
    .unwrap();
    let ir_path = dir.join("fwd.ir.json");
    assert!(ir_path.exists(), "the after:<pass> IR sidecar is written");
    let text = fs::read_to_string(&ir_path).unwrap();
    let program = IrProgram::from_json(&text).expect("the after:<pass> sidecar parses as IR JSON");
    assert_eq!(program.version, 2, "IR version 2");
    assert!(program.worlds.iter().any(|w| w.name == "main"));
}

#[test]
fn compile_emit_ir_after_pass_errors_naming_valid_stages() {
    let dir = scratch("tmc_emit_ir_bad");
    let obj = dir.join("a1.tmo");
    // `after:<pass>` resolves for a REGISTERED pass now; an unregistered name
    // fails early with an error listing the stages that do exist — the bookends
    // plus every registered `after:<pass>` (so `after:inline`/`after:outline`
    // appear in the list, and the bogus `after:nowhere` is rejected).
    let err = execute(&args(&[
        "compile",
        fixture("a1_replace_b.tmc").to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
        "--emit-ir=after:nowhere",
    ]))
    .unwrap_err();
    assert!(err.contains("lowered") && err.contains("final"), "{err}");
    assert!(err.contains("after:inline"), "{err}");
    assert!(
        err.contains("after:nowhere"),
        "the bogus stage is named: {err}"
    );
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

// --- tape-block: whole-block authoring (docs/tmt/cli.md (tape-block)) -------

/// Three tapes with cardinalities 5 / 2 / 2 — the shape that makes the
/// block fallback (sized to the widest band) differ from each band's own
/// width, which is what the repin cardinality check must measure against.
const POW2_SRC: &str = "\
alphabet mainAlpha { ' ', 's', 'b', 'k', '1' }
alphabet workAlpha { ' ', '1' }

machine {
  tape main: mainAlpha;
  tape cnt:  workAlpha;
  tape tmp:  workAlpha;

  entry state s { ['b', *, *] -> stop; }
}
";

fn compile_and_link_pow2(dir: &Path) -> PathBuf {
    let src = dir.join("pow2.tmc");
    fs::write(&src, POW2_SRC).unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("pow2.tmo").to_str().unwrap()])).unwrap();
    dir.join("pow2.tmx")
}

#[test]
fn tape_block_new_from_an_image_pins_glyphs_and_cells_in_one_call() {
    let dir = scratch("tb_new_image");
    let exe = compile_and_link_pow2(&dir);
    let out_path = dir.join("in.tmt");

    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "--alphabet",
        "0=' ','s','b','k','1'",
        "--alphabet",
        "1=' ','1'",
        "--alphabet",
        "2=' ','1'",
        "--cells",
        "0='s','b','1','1','1','k'",
        "-o",
        out_path.to_str().unwrap(),
    ]))
    .expect("new succeeds");

    let shown = execute(&args(&["tape-block", "show", out_path.to_str().unwrap()])).unwrap();
    assert!(shown.stdout.contains("|sb111k|"), "got:\n{}", shown.stdout);
}

#[test]
fn tape_block_new_without_from_sizes_the_block_from_the_alphabet_flags() {
    let dir = scratch("tb_new_freehand");
    let out_path = dir.join("in.tmt");

    execute(&args(&[
        "tape-block",
        "new",
        "--alphabet",
        "0=' ','a'",
        "--alphabet",
        "1=' ','1'",
        "--cells",
        "0='a','a'",
        "-o",
        out_path.to_str().unwrap(),
    ]))
    .expect("new succeeds");

    let shown = execute(&args(&["tape-block", "show", out_path.to_str().unwrap()])).unwrap();
    assert!(shown.stdout.contains("tape 0"), "got:\n{}", shown.stdout);
    assert!(shown.stdout.contains("tape 1"), "got:\n{}", shown.stdout);
    assert!(!shown.stdout.contains("tape 2"), "got:\n{}", shown.stdout);
    assert!(shown.stdout.contains("|aa|"), "got:\n{}", shown.stdout);
}

#[test]
fn tape_block_new_freehand_rejects_non_contiguous_keys() {
    let dir = scratch("tb_new_gap");
    let err = execute(&args(&[
        "tape-block",
        "new",
        "--alphabet",
        "0=' ','a'",
        "--alphabet",
        "5=' ','b'",
        "-o",
        dir.join("x.tmt").to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains("contiguously"), "got: {err}");
}

#[test]
fn tape_block_new_rejects_an_alphabet_of_the_wrong_cardinality() {
    let dir = scratch("tb_new_card");
    let exe = compile_and_link_pow2(&dir);
    let err = execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "--alphabet",
        "0=' ','x'", // tape 0 is 5 wide
        "-o",
        dir.join("x.tmt").to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains("cardinality 5"), "got: {err}");
    assert!(err.contains("2 glyphs"), "got: {err}");
}

#[test]
fn tape_block_cells_resolve_against_the_alphabet_pinned_in_the_same_call() {
    // `s` exists only in the NEW alphabet, never in the image's decimal
    // labels — so this only succeeds if --alphabet applied before --cells.
    let dir = scratch("tb_new_order");
    let exe = compile_and_link_pow2(&dir);
    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "--alphabet",
        "0=' ','s','b','k','1'",
        "--cells",
        "0='s'",
        "-o",
        dir.join("ok.tmt").to_str().unwrap(),
    ]))
    .expect("--alphabet must apply before --cells");
}

#[test]
fn tape_block_new_rejects_a_tape_index_past_the_block() {
    let dir = scratch("tb_new_range");
    let exe = compile_and_link_pow2(&dir);
    let err = execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "--cells",
        "7='1'",
        "-o",
        dir.join("x.tmt").to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains("out of range"), "got: {err}");
    assert!(err.contains("3 tape(s)"), "got: {err}");
}

#[test]
fn a_tape_name_without_a_source_is_a_clear_error() {
    let dir = scratch("tb_name_no_src");
    let exe = compile_and_link_pow2(&dir);
    let err = execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "--cells",
        "main='s'",
        "-o",
        dir.join("in.tmt").to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains(".tmc"), "got: {err}");
}

#[test]
fn tape_block_edit_flags_reject_a_repeated_key() {
    let dir = scratch("tb_dup_key");
    let err = execute(&args(&[
        "tape-block",
        "new",
        "--alphabet",
        "0=' ','a'",
        "--cells",
        "0='a'",
        "--cells",
        "0='a','a'",
        "-o",
        dir.join("x.tmt").to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains("twice"), "got: {err}");
}

#[test]
fn tape_block_new_from_source_takes_glyphs_and_names_from_the_program() {
    let dir = scratch("tb_new_src");
    let src = dir.join("pow2.tmc");
    fs::write(&src, POW2_SRC).unwrap();
    let out_path = dir.join("in.tmt");

    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        src.to_str().unwrap(),
        "--cells",
        "main='s','b','1','1','1','k'",
        "-o",
        out_path.to_str().unwrap(),
    ]))
    .expect("new from source succeeds");

    let shown = execute(&args(&["tape-block", "show", out_path.to_str().unwrap()])).unwrap();
    assert!(shown.stdout.contains("|sb111k|"), "got:\n{}", shown.stdout);
}

#[test]
fn tape_block_new_from_source_accepts_an_index_key_too() {
    let dir = scratch("tb_new_src_ix");
    let src = dir.join("pow2.tmc");
    fs::write(&src, POW2_SRC).unwrap();
    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        src.to_str().unwrap(),
        "--cells",
        "0='s'",
        "-o",
        dir.join("in.tmt").to_str().unwrap(),
    ]))
    .expect("index keys stay legal on the source path");
}

#[test]
fn tape_block_new_from_source_rejects_an_unknown_tape_name() {
    let dir = scratch("tb_new_src_bad");
    let src = dir.join("pow2.tmc");
    fs::write(&src, POW2_SRC).unwrap();
    let err = execute(&args(&[
        "tape-block",
        "new",
        "--from",
        src.to_str().unwrap(),
        "--cells",
        "nope='s'",
        "-o",
        dir.join("in.tmt").to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains("no such tape"), "got: {err}");
    assert!(err.contains("main"), "got: {err}");
}

/// A library resolves fine but has no single band to describe, so the CLI —
/// not the compiler — is the one that refuses.
#[test]
fn tape_block_new_from_a_library_source_is_a_clear_error() {
    let dir = scratch("tb_new_src_lib");
    let src = dir.join("lib.tmc");
    fs::write(
        &src,
        "alphabet a { ' ', '1' }\nroutine r(tape t: a) { entry state s { [*] -> stop; } }\n",
    )
    .unwrap();
    let err = execute(&args(&[
        "tape-block",
        "new",
        "--from",
        src.to_str().unwrap(),
        "-o",
        dir.join("x.tmt").to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains("no `machine` block"), "got: {err}");
}

/// Cell indices of one band, decoded from a written block — the repin
/// invariant asserts on these directly.
fn cells_of(path: &Path, tape: usize) -> Vec<u8> {
    let bytes = fs::read(path).unwrap();
    TapeBlockFile::from_bytes(&bytes).unwrap().tapes[tape]
        .cells
        .clone()
}

#[test]
fn tape_block_set_repins_glyphs_without_moving_a_cell() {
    let dir = scratch("tb_set_repin");
    let exe = compile_and_link_pow2(&dir);
    let path = dir.join("in.tmt");
    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "--cells",
        "0='1','2','4','4','4','3'", // the image's decimal labels
        "-o",
        path.to_str().unwrap(),
    ]))
    .unwrap();
    let before = cells_of(&path, 0);

    execute(&args(&[
        "tape-block",
        "set",
        path.to_str().unwrap(),
        "--in-place",
        "--alphabet",
        "0=' ','s','b','k','1'",
    ]))
    .expect("repin succeeds");

    let shown = execute(&args(&["tape-block", "show", path.to_str().unwrap()])).unwrap();
    assert!(shown.stdout.contains("|sb111k|"), "got:\n{}", shown.stdout);

    // Relabel, never re-map: the cell INDICES are untouched, only the
    // glyph table they are read through changed.
    assert_eq!(cells_of(&path, 0), before, "a repin must not move cells");
}

#[test]
fn tape_block_set_rejects_a_repin_of_the_wrong_cardinality() {
    let dir = scratch("tb_set_card");
    let exe = compile_and_link_pow2(&dir);
    let path = dir.join("in.tmt");
    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        exe.to_str().unwrap(),
        "-o",
        path.to_str().unwrap(),
    ]))
    .unwrap();
    let err = execute(&args(&[
        "tape-block",
        "set",
        path.to_str().unwrap(),
        "--in-place",
        "--alphabet",
        "1=' ','a','b'", // tape 1 is 2 wide
    ]))
    .unwrap_err();
    assert!(err.contains("cardinality 2"), "got: {err}");
}

#[test]
fn tape_block_set_takes_names_from_a_source() {
    let dir = scratch("tb_set_names");
    let src = dir.join("pow2.tmc");
    fs::write(&src, POW2_SRC).unwrap();
    let path = dir.join("in.tmt");
    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        src.to_str().unwrap(),
        "-o",
        path.to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "tape-block",
        "set",
        path.to_str().unwrap(),
        "--in-place",
        "--from",
        src.to_str().unwrap(),
        "--cells",
        "cnt='1'",
    ]))
    .expect("name keys resolve on set");
}

#[test]
fn tape_block_show_prints_each_bands_effective_alphabet() {
    let dir = scratch("tb_show_alpha");
    let src = dir.join("pow2.tmc");
    fs::write(&src, POW2_SRC).unwrap();
    let path = dir.join("in.tmt");
    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        src.to_str().unwrap(),
        "-o",
        path.to_str().unwrap(),
    ]))
    .unwrap();
    let shown = execute(&args(&["tape-block", "show", path.to_str().unwrap()])).unwrap();
    assert!(
        shown
            .stdout
            .contains("tape 0: origin 0, head 0 reads ' ', alphabet"),
        "got:\n{}",
        shown.stdout
    );
    // Band 0 is mainAlpha (5 glyphs), band 1 workAlpha (2) — a single
    // header line could not have shown both.
    assert!(shown.stdout.contains("\"k\""), "got:\n{}", shown.stdout);
    let band1 = shown
        .stdout
        .lines()
        .find(|l| l.starts_with("tape 1:"))
        .unwrap();
    assert!(band1.contains("[\" \", \"1\"]"), "got: {band1}");
}

#[test]
fn tape_block_show_honours_the_delimit_flags() {
    let dir = scratch("tb_show_delim");
    let path = dir.join("in.tmt");
    execute(&args(&[
        "tape-block",
        "new",
        "--alphabet",
        "0=' ','a','b'",
        "--cells",
        "0='a','b'",
        "-o",
        path.to_str().unwrap(),
    ]))
    .unwrap();
    let dense = execute(&args(&["tape-block", "show", path.to_str().unwrap()])).unwrap();
    assert!(dense.stdout.contains("|ab|"), "got:\n{}", dense.stdout);
    let sep = execute(&args(&[
        "tape-block",
        "show",
        path.to_str().unwrap(),
        "--separated",
    ]))
    .unwrap();
    assert!(sep.stdout.contains("|a|b|"), "got:\n{}", sep.stdout);
}

#[test]
fn run_save_tape_block_preserves_every_bands_glyphs() {
    let dir = scratch("tm_save");
    let src = dir.join("pow2.tmc");
    fs::write(&src, POW2_SRC).unwrap();
    let exe = compile_and_link_pow2(&dir);
    let input = dir.join("in.tmt");
    let saved = dir.join("out.tmt");

    execute(&args(&[
        "tape-block",
        "new",
        "--from",
        src.to_str().unwrap(),
        "--cells",
        "main='b'",
        "-o",
        input.to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
        input.to_str().unwrap(),
        "--save-tape-block",
        saved.to_str().unwrap(),
    ]))
    .expect("run succeeds");

    let shown = execute(&args(&["tape-block", "show", saved.to_str().unwrap()])).unwrap();
    // Band 0 is mainAlpha (5 glyphs), band 1 workAlpha (2). A save that
    // collapsed to one block alphabet could not express both.
    assert!(
        shown.stdout.contains("\"k\""),
        "band 0 lost its glyphs:\n{}",
        shown.stdout
    );
    let band1 = shown
        .stdout
        .lines()
        .find(|l| l.starts_with("tape 1:"))
        .unwrap();
    assert!(
        band1.contains("[\" \", \"1\"]"),
        "band 1 lost its glyphs: {band1}"
    );
}

#[test]
fn run_rejects_a_block_whose_cardinality_disagrees_with_the_image() {
    let dir = scratch("tm_card");
    let exe = compile_and_link_pow2(&dir); // tape 0 is 5 wide
    let bad = dir.join("bad.tmt");
    execute(&args(&[
        "tape-block",
        "new",
        "--alphabet",
        "0=' ','x'", // 2 wide
        "--alphabet",
        "1=' ','1'",
        "--alphabet",
        "2=' ','1'",
        "-o",
        bad.to_str().unwrap(),
    ]))
    .unwrap();

    let err = execute(&args(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
        bad.to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains("tape 0"), "got: {err}");
    assert!(err.contains("2 glyph(s)"), "got: {err}");
    assert!(err.contains("expects 5"), "got: {err}");
}
