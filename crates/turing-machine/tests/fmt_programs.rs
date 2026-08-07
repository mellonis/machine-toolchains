//! `tmt fmt`: the `.tma` library property (idempotence + lossless through the
//! canonical grid) over the frames/tables/rept surface, and the CLI for both
//! languages (extension dispatch, `--check`, stdin `-` with `--lang`, the
//! per-file fatal in a batch). The `.tma` formatter is core's
//! `format_asm_with` — these tests are the first to exercise its grid over the
//! TM-1-only constructs (`.frame`/`.map`/`.exits`, `.rept`, vector operands);
//! the `.tmc` side is the crate's own printer, whose whole-corpus properties
//! live in `fmt_tmc.rs` and whose layout fixtures live beside the module.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::asm::{AsmErrorKind, format_asm_with};
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_turing_machine::asm::{
    assemble, disassemble_executable_with_map, disassemble_object, link, tm1_syntax,
};
use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::stdlib;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("fmt-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    p
}

fn fmt_tma(src: &str) -> String {
    format_asm_with(src, tm1_syntax().caps).expect("formats")
}

/// Full 0.2 frames surface — a `.frame`/`.map`/`.exits` descriptor, `call.m`,
/// `trap`, `retx`, match table, vector operands — deliberately spaced
/// off-grid so formatting actually changes it.
const FRAMES: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.routine helper, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, 1]
  .row [*, *]
F0: .frame tapes=(1, 0)
  .map 0, rmap=(1->1, 3=>1)
  .exits done, other
.section code
.func main
   rd
   mtc T0
   trap #0
   call.m helper, F0
done: stp
other: hlt
.func helper
   wr [1, -]
   retx #1
";

/// Assert format is idempotent (`fmt∘fmt == fmt`) and lossless (the formatted
/// source assembles to byte-identical object code).
fn assert_idempotent_and_lossless(src: &str, label: &str) {
    let once = fmt_tma(src);
    let twice = fmt_tma(&once);
    assert_eq!(once, twice, "{label}: fmt is not idempotent");
    let a = assemble(src, false).unwrap_or_else(|e| panic!("{label}: source assembles: {e:?}"));
    let b = fmt_tma(src);
    let b = assemble(&b, false).unwrap_or_else(|e| panic!("{label}: formatted assembles: {e:?}"));
    assert_eq!(
        a.to_bytes(),
        b.to_bytes(),
        "{label}: fmt changed the object bytes"
    );
}

#[test]
fn frames_fixture_fmt_is_idempotent_and_lossless() {
    assert_idempotent_and_lossless(FRAMES, "frames");
    // The off-grid input really is reshaped (guards against a no-op formatter
    // trivially satisfying idempotence + lossless).
    assert_ne!(fmt_tma(FRAMES), FRAMES, "the off-grid input should reshape");
}

#[test]
fn brainfuck_fixture_fmt_is_idempotent_and_lossless() {
    // The flagship UTM: sections, an 8-row match table, `.rept` macros with
    // `{v}` substitution, dispatch tables. Read-only — never written back
    // (it is golden-backed).
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/brainfuck-utm.tma");
    let src = fs::read_to_string(&path).expect("read brainfuck-utm.tma");
    assert_idempotent_and_lossless(&src, "brainfuck");
}

/// The `.tma` dogfood lock, mirroring `fmt_tmc.rs`'s
/// `every_tmc_source_is_already_fmt_clean`: every `.tma` source the
/// repository ships must already be in canonical form, so formatting it
/// is a byte-for-byte no-op. Any future printer change that would
/// reformat a shipped source fails here first.
#[test]
fn every_tma_source_is_already_fmt_clean() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/brainfuck-utm.tma");
    let src = fs::read_to_string(&path).expect("read brainfuck-utm.tma");
    assert_eq!(fmt_tma(&src), src, "brainfuck-utm.tma is not fmt-clean");
}

/// The Appendix A + nested-graft `.tmc` fixtures `tmc_golden.rs` runs
/// derivation-first goldens over — read fresh here (per-file-helper
/// convention: this file's sweep over every debug mode and call
/// mechanism is its own concern, not that file's run assertions).
const CORPUS: &[&str] = &[
    "a1_replace_b",
    "a2_binary_plus_one",
    "a3_two_tape_copy",
    "a4_byte_increment",
    "a5_call_across_alphabets",
    "a6_graph_graft_multi_exit",
    "nested_graft",
];

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// `tmt dis`'s permanent dogfood gate over the golden corpus — the PM-1
/// sibling this mirrors (`fmt_pma.rs`'s `dis_output_is_already_canonical`)
/// asserts format identity alone, because a `.pma` object never carries
/// tables and so never has an assemblability defect to catch; a `.tma`
/// one can. For every fixture, every debug mode, and (for the linked
/// half) every call mechanism, both `disassemble_object`'s and
/// `disassemble_executable`'s output must (1) already be fmt-canonical
/// and (2) reassemble — the pair Task 5's plan scoped to objects only,
/// because the executable path's naming, wrapping, and signature defects
/// were known at the time; both are fixed now, so this gate covers both
/// renderers with no scoping note.
///
/// One documented exception, asserted rather than silently excluded: a
/// `--call-mech=mono` link names a stamped specialized routine copy with
/// a digest suffix (`name$<hex>`, `docs/tmt/isa.md (call mechanisms)`),
/// which is not a legal `.tma` identifier — the disassembled name does
/// not re-lex, so reassembly fails regardless of anything the disassembler
/// prints. `a5_call_across_alphabets` is the only fixture in this corpus
/// whose binding is holey enough to force a mono stamp, so it is the only
/// combination this gate expects to fail reassembly — and it asserts
/// that specific failure rather than skipping the case: if a future fix
/// makes it reassemble, this assertion goes red and the exception must
/// be removed.
#[test]
fn dis_output_of_the_golden_corpus_assembles_and_is_fmt_clean() {
    for &fixture in CORPUS {
        let src = fs::read_to_string(golden_dir().join(format!("{fixture}.tmc")))
            .unwrap_or_else(|e| panic!("{fixture}: read fixture: {e}"));
        for debug in [false, true] {
            let obj = compile(
                &src,
                CompileOptions {
                    debug_info: debug,
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{fixture} (-g={debug}): compile failed: {e}"))
            .object;

            // Object disassembly: no executable-only naming/signature
            // surface, so no exception applies here.
            let obj_dis = disassemble_object(&obj);
            assert_eq!(
                fmt_tma(&obj_dis),
                obj_dis,
                "{fixture} (-g={debug}) object dis is not fmt-clean:\n{obj_dis}"
            );
            assemble(&obj_dis, false).unwrap_or_else(|e| {
                panic!("{fixture} (-g={debug}) object dis does not reassemble: {e}\n{obj_dis}")
            });

            for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
                let out = link(
                    std::slice::from_ref(&obj),
                    std::slice::from_ref(stdlib::object()),
                    LinkOptions {
                        call_mech: mech,
                        ..Default::default()
                    },
                )
                .unwrap_or_else(|e| panic!("{fixture} (-g={debug}, {mech}): link failed: {e}"));
                let exe_dis = disassemble_executable_with_map(&out.executable, &out.map);
                let reassembled = assemble(&exe_dis, false);

                if fixture == "a5_call_across_alphabets" && mech == CallMech::Mono {
                    let e = reassembled.expect_err(
                        "a5_call_across_alphabets under mono should still hit the digest-name \
                         exception documented above — remove this exception if it now \
                         reassembles",
                    );
                    assert!(
                        matches!(e.kind, AsmErrorKind::RawLine),
                        "unexpected failure shape for the digest-name exception: {e}\n{exe_dis}"
                    );
                    continue;
                }
                reassembled.unwrap_or_else(|e| {
                    panic!(
                        "{fixture} (-g={debug}, {mech}) executable dis does not reassemble: \
                         {e}\n{exe_dis}"
                    )
                });
                assert_eq!(
                    fmt_tma(&exe_dis),
                    exe_dis,
                    "{fixture} (-g={debug}, {mech}) executable dis is not fmt-clean:\n{exe_dis}"
                );
            }
        }
    }
}

#[test]
fn fmt_check_on_canonical_tma_is_silent_and_exits_zero() {
    let dir = scratch("check-clean");
    let canonical = fmt_tma(FRAMES);
    let f = write(&dir, "a.tma", &canonical);
    let out = execute(&args(&["fmt", "--check", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "stdout: {} stderr: {}", out.stdout, out.stderr);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
}

#[test]
fn fmt_check_on_offgrid_tma_lists_it_and_exits_one_without_writing() {
    let dir = scratch("check-dirty");
    let f = write(&dir, "a.tma", FRAMES);
    let out = execute(&args(&["fmt", "--check", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("a.tma"), "{}", out.stdout);
    // --check must not have rewritten the file.
    assert_eq!(fs::read_to_string(&f).unwrap(), FRAMES);
}

#[test]
fn fmt_write_reformats_the_tma_in_place_and_exits_zero() {
    let dir = scratch("write");
    let f = write(&dir, "a.tma", FRAMES);
    let out = execute(&args(&["fmt", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let written = fs::read_to_string(&f).unwrap();
    assert_eq!(written, fmt_tma(FRAMES), "file left in canonical form");
    // Idempotent: a second run is a no-op.
    let out2 = execute(&args(&["fmt", f.to_str().unwrap()])).unwrap();
    assert_eq!(out2.code, 0);
    assert_eq!(fs::read_to_string(&f).unwrap(), written);
}

#[test]
fn fmt_lang_alongside_a_path_is_an_error() {
    // `--lang` selects stdin's language; alongside a PATH it is a misuse (a
    // file's language comes from its extension). The stdin happy path itself
    // reads the process's real stdin, so it is not driven through the
    // in-process `execute`.
    let dir = scratch("lang-misuse");
    let f = write(&dir, "a.tma", FRAMES);
    let err = execute(&args(&["fmt", "--lang", "tma", f.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("--lang applies to stdin"), "{err}");
}

#[test]
fn fmt_rejects_an_unknown_lang() {
    let err = execute(&args(&["fmt", "-", "--lang", "cobol"])).unwrap_err();
    assert!(err.contains("takes tmc or tma"), "{err}");
}

/// A small `.tmc` program in canonical form (its own crate-side battery
/// proves the whole corpus round-trips; here it only has to be a file the CLI
/// leaves alone).
const TMC_CANONICAL: &str = "\
alphabet ab { '_', 'a' }

machine {
  tape main: ab;

  entry state scan {
    ['a'] -> write ['_'] move [>] goto scan;
    [*]   -> stop;
  }
}
";

#[test]
fn fmt_rewrites_a_tmc_file() {
    let dir = scratch("tmc");
    let messy = "alphabet ab{'_','a'}\nmachine{\ntape main:ab;\nentry state scan{\n['a']->write['_'] move[>] goto scan;\n[*]->stop;\n}\n}\n";
    let f = write(&dir, "m.tmc", messy);
    let out = execute(&args(&["fmt", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let formatted = fs::read_to_string(&f).unwrap();
    assert!(
        formatted.contains("    ['a'] -> write ['_'] move [>] goto scan;"),
        "{formatted}"
    );
    // Second run is a no-op — the canonical form is a fixed point.
    let out = execute(&args(&["fmt", "--check", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "stdout: {}", out.stdout);
}

#[test]
fn fmt_check_leaves_a_canonical_tmc_file_alone() {
    let dir = scratch("tmc-canonical");
    let f = write(&dir, "m.tmc", TMC_CANONICAL);
    let out = execute(&args(&["fmt", "--check", f.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "stdout: {}", out.stdout);
    assert_eq!(fs::read_to_string(&f).unwrap(), TMC_CANONICAL);
}

#[test]
fn fmt_reports_a_tmc_parse_fatal_and_keeps_going() {
    // Batch model: a broken `.tmc` is one diagnostic, not an abort — the
    // `.tma` sibling in the same run is still formatted.
    let dir = scratch("tmc-fatal");
    write(&dir, "a.tma", FRAMES);
    let broken = write(&dir, "m.tmc", "machine {\n");
    let out = execute(&args(&["fmt", "--check", dir.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("a.tma"), "stdout: {}", out.stdout);
    assert!(
        out.stderr
            .contains(broken.file_name().unwrap().to_str().unwrap()),
        "stderr: {}",
        out.stderr
    );
    assert!(out.stderr.contains("error:"), "stderr: {}", out.stderr);
}

#[test]
fn fmt_directory_walks_both_extensions() {
    // A .tma and a .tmc under one dir, both off-grid: `--check` lists both
    // and exits 1.
    let dir = scratch("walk");
    write(&dir, "a.tma", FRAMES);
    write(&dir, "m.tmc", "machine{\ntape t:ab;\n}\n");
    let out = execute(&args(&["fmt", "--check", dir.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("a.tma"), "stdout: {}", out.stdout);
    assert!(out.stdout.contains("m.tmc"), "stdout: {}", out.stdout);
    assert!(out.stderr.is_empty(), "stderr: {}", out.stderr);
}

#[test]
fn fmt_help_prints_usage() {
    let out = execute(&args(&["fmt", "--help"])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("USAGE: tmt fmt"), "{}", out.stdout);
}
