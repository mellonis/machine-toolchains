use std::fs;
use std::path::PathBuf;

use mtc_post_machine::cli::execute;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn no_args_prints_usage() {
    let out = execute(&[]).unwrap();
    assert!(out.stdout.contains("USAGE: pmt"));
    assert_eq!(out.code, 0);
}

#[test]
fn unknown_subcommand_errors() {
    assert!(execute(&args(&["bogus"])).is_err());
}

#[test]
fn version_reports_the_language_version() {
    let out = execute(&args(&["--version"])).unwrap();
    assert!(out.stdout.contains(&format!(
        "pmc language {}",
        mtc_post_machine::PMC_LANG_VERSION
    )));
    assert_eq!(mtc_post_machine::PMC_LANG_VERSION, "0.2");
}

const HELLO: &str = "main() { 1: mark; 2: right; 3: mark(!); }";

#[test]
fn compile_writes_an_object_and_link_writes_exe_and_map() {
    let dir = scratch("build_pipeline");
    let src = dir.join("hello.pmc");
    fs::write(&src, HELLO).unwrap();

    let out = execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    let obj = dir.join("hello.pmo");
    assert!(obj.exists());

    let exe = dir.join("hello.pmx");
    let out = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "-o",
        exe.to_str().unwrap(),
        "-v",
    ]))
    .unwrap();
    assert!(exe.exists());
    assert!(dir.join("hello.pmx.map").exists());
    assert!(out.stderr.contains("link:"));
}

#[test]
fn compile_dash_s_emits_pma_and_asm_accepts_it() {
    let dir = scratch("s_roundtrip");
    let src = dir.join("hello.pmc");
    fs::write(&src, HELLO).unwrap();
    execute(&args(&["compile", src.to_str().unwrap(), "-S"])).unwrap();
    let pma = dir.join("hello.pma");
    assert!(pma.exists());
    execute(&args(&["asm", pma.to_str().unwrap()])).unwrap();
    assert!(dir.join("hello.pmo").exists());
}

#[test]
fn emit_ir_stage_last_wins_and_validates() {
    let dir = scratch("emit_ir");
    let src = dir.join("hello.pmc");
    fs::write(&src, HELLO).unwrap();
    execute(&args(&[
        "compile",
        src.to_str().unwrap(),
        "-O1",
        "--emit-ir=lowered",
    ]))
    .unwrap();
    let ir = fs::read_to_string(dir.join("hello.ir.json")).unwrap();
    assert!(ir.contains("\"version\": 3"));
    let err = execute(&args(&[
        "compile",
        src.to_str().unwrap(),
        "--emit-ir=bogus",
    ]))
    .unwrap_err();
    assert!(err.contains("unknown IR stage"));
}

#[test]
fn werror_fails_on_warnings() {
    let dir = scratch("werror");
    let src = dir.join("warny.pmc");
    // an unused non-exported helper → unused-function warning
    fs::write(&src, "helper() { 1: right(!); }\nmain() { 1: mark(!); }").unwrap();
    let ok = execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    assert!(ok.stderr.contains("warning"));
    let err = execute(&args(&["compile", src.to_str().unwrap(), "-Werror"])).unwrap_err();
    assert!(err.contains("-Werror"));
}

#[test]
fn nostdlib_makes_std_calls_unresolved() {
    let dir = scratch("nostdlib");
    let src = dir.join("uses_std.pmc");
    fs::write(&src, "use std::goToEnd; main() { @goToEnd(!); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    let obj = dir.join("uses_std.pmo");
    // with std (default): links
    execute(&args(&["link", obj.to_str().unwrap()])).unwrap();
    // without: unresolved
    let err = execute(&args(&["link", obj.to_str().unwrap(), "--nostdlib"])).unwrap_err();
    assert!(err.to_lowercase().contains("unresolved"));
}

#[test]
fn compile_errors_render_one_position_prefix() {
    let dir = scratch("err_prefix");
    let src = dir.join("bad.pmc");
    fs::write(&src, "main() { 1: flip; }").unwrap();
    let err = execute(&args(&["compile", src.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("bad.pmc:1:"), "{err}");
    assert!(err.contains("error:"), "{err}");
    assert!(!err.contains("line 1"), "doubled prefix: {err}");
}

#[test]
fn bare_emit_ir_before_the_positional_does_not_eat_it() {
    let dir = scratch("emit_ir_bare");
    let src = dir.join("hello.pmc");
    fs::write(&src, HELLO).unwrap();
    // the flag PRECEDES the input — scan-based parsing must not consume it
    let out = execute(&args(&["compile", "--emit-ir", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(dir.join("hello.pmo").exists());
    let ir = fs::read_to_string(dir.join("hello.ir.json")).unwrap();
    assert!(ir.contains("\"version\": 3"));
}

#[test]
fn emit_ir_duplicate_stage_labels_resolve_last_wins() {
    let src_text = "walk() { 1: right; 2: check(1, !); }\n\
                    hop() { 1: @walk(!); }\n\
                    main() { 1: @hop(); 2: mark(!); }";

    // Compile via library to identify which passes repeat
    let lib_result = compile(
        src_text,
        CompileOptions {
            opt_level: OptLevel::O1,
            capture_ir: true,
            ..Default::default()
        },
    )
    .unwrap();

    // Find a label that appears multiple times
    let mut label_counts = std::collections::HashMap::new();
    for (label, _) in &lib_result.ir_snapshots {
        *label_counts.entry(label.clone()).or_insert(0) += 1;
    }

    // Pick a repeating label (after:inline appears at least twice)
    let repeating_label = label_counts
        .iter()
        .find(|(_, count)| **count > 1)
        .map(|(label, _)| label.clone())
        .expect("should have a repeating pass label");

    // Get the LAST (rightmost) occurrence of this label
    let last_snapshot = lib_result
        .ir_snapshots
        .iter()
        .rev()
        .find(|(l, _)| l == &repeating_label)
        .expect("should find last occurrence")
        .1
        .to_json();

    // Now compile via CLI with --emit-ir=after:<pass>
    let dir = scratch("emit_ir_last_wins");
    let src = dir.join("multi.pmc");
    fs::write(&src, src_text).unwrap();

    let out = execute(&args(&[
        "compile",
        src.to_str().unwrap(),
        "-O1",
        &format!("--emit-ir={}", repeating_label),
    ]))
    .unwrap();
    assert_eq!(out.code, 0, "compile failed: {}", out.stderr);

    let cli_ir = fs::read_to_string(dir.join("multi.ir.json")).unwrap();

    // Verify the CLI result matches the last snapshot (docs/cli.md: repeated
    // stages resolve last-wins).
    assert_eq!(
        cli_ir, last_snapshot,
        "CLI --emit-ir result should match the LAST occurrence of {} (last-wins)",
        repeating_label
    );
}

#[test]
fn full_pipeline_reproduces_the_sum_golden() {
    let dir = scratch("pipeline_sum");
    let golden_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let src = dir.join("sum.pmc");
    fs::copy(golden_dir.join("sum.pmc"), &src).unwrap();

    execute(&args(&["compile", src.to_str().unwrap(), "-O1"])).unwrap();
    execute(&args(&["link", dir.join("sum.pmo").to_str().unwrap()])).unwrap();
    execute(&args(&[
        "tape",
        "build",
        "*** **",
        "-o",
        dir.join("in.pmt").to_str().unwrap(),
    ]))
    .unwrap();
    let out = execute(&args(&[
        "run",
        dir.join("sum.pmx").to_str().unwrap(),
        "--tape-block",
        dir.join("in.pmt").to_str().unwrap(),
        "--save-tape-block",
        dir.join("out.pmt").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("outcome: Stopped"));
    assert_eq!(
        fs::read(dir.join("out.pmt")).unwrap(),
        fs::read(golden_dir.join("sum.expected.pmt")).unwrap(),
    );
}

#[test]
fn exit_codes_distinguish_halt_and_trap() {
    let dir = scratch("exit_codes");
    let src = dir.join("h.pmc");
    fs::write(&src, "main() { 1: halt; }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("h.pmo").to_str().unwrap()])).unwrap();
    let out = execute(&args(&["run", dir.join("h.pmx").to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 2);

    // step-limit trap
    let src = dir.join("spin.pmc");
    fs::write(&src, "main() { 1: right(1); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("spin.pmo").to_str().unwrap()])).unwrap();
    let out = execute(&args(&[
        "run",
        dir.join("spin.pmx").to_str().unwrap(),
        "--max-steps",
        "100",
    ]))
    .unwrap();
    assert_eq!(out.code, 3);
    assert!(out.stdout.contains("StepLimit"));
}

#[test]
fn strict_cells_traps_double_mark() {
    let dir = scratch("strict");
    let src = dir.join("dbl.pmc");
    fs::write(&src, "main() { 1: mark; 2: mark(!); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("dbl.pmo").to_str().unwrap()])).unwrap();
    let ok = execute(&args(&["run", dir.join("dbl.pmx").to_str().unwrap()])).unwrap();
    assert_eq!(ok.code, 0); // permissive default
    let strict = execute(&args(&[
        "run",
        dir.join("dbl.pmx").to_str().unwrap(),
        "--strict-cells",
    ]))
    .unwrap();
    assert_eq!(strict.code, 3);
}

#[test]
fn trace_streams_lines_with_post_state_into_the_writer() {
    use mtc_post_machine::cli::execute_with;
    let dir = scratch("trace");
    let src = dir.join("t.pmc");
    fs::write(&src, "main() { 1: mark; 2: right(!); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("t.pmo").to_str().unwrap()])).unwrap();
    let mut trace = Vec::new();
    let out = execute_with(
        &args(&["run", dir.join("t.pmx").to_str().unwrap(), "--trace"]),
        &mut trace,
    )
    .unwrap();
    let text = String::from_utf8(trace).unwrap();
    // ent, wr, rgt, stp — one line each; blank tape latches MF=0 at load
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(
        lines[0].contains("ent") && lines[0].ends_with("; MF=0 head=0"),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].contains("wr") && lines[1].ends_with("; MF=1 head=0"),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].contains("rgt") && lines[2].ends_with("; MF=0 head=1"),
        "{}",
        lines[2]
    );
    assert!(
        lines[3].contains("stp") && lines[3].ends_with("; MF=0 head=1"),
        "{}",
        lines[3]
    );
    assert!(out.stderr.is_empty(), "trace must stream, not buffer");
}

#[test]
fn dis_listing_and_tape_show_render() {
    let dir = scratch("dis_listing");
    let src = dir.join("d.pmc");
    fs::write(&src, "main() { 1: mark(!); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap(), "-g"])).unwrap();
    execute(&args(&["link", dir.join("d.pmo").to_str().unwrap()])).unwrap();
    let out = execute(&args(&[
        "dis",
        dir.join("d.pmx").to_str().unwrap(),
        "--listing",
    ]))
    .unwrap();
    assert!(out.stdout.starts_with("main:"), "{}", out.stdout);
    assert!(out.stdout.contains("0000:"));

    execute(&args(&[
        "tape",
        "build",
        " **",
        "-o",
        dir.join("s.pmt").to_str().unwrap(),
    ]))
    .unwrap();
    let shown = execute(&args(&[
        "tape",
        "show",
        dir.join("s.pmt").to_str().unwrap(),
    ]))
    .unwrap();
    assert!(shown.stdout.contains("| **|"), "{}", shown.stdout);
}

#[test]
fn ir_graph_renders_mermaid() {
    let dir = scratch("ir_graph");
    let src = dir.join("g.pmc");
    fs::write(&src, "main() { 1: right; 2: check(1, !); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap(), "--emit-ir"])).unwrap();
    let out = execute(&args(&[
        "ir",
        "graph",
        dir.join("g.ir.json").to_str().unwrap(),
    ]))
    .unwrap();
    assert!(out.stdout.contains("flowchart TD"));
    assert!(out.stdout.contains("-->|MF|"));
}

#[test]
fn traced_trap_prints_the_faulting_line_exactly_once() {
    use mtc_post_machine::cli::execute_with;
    let dir = scratch("trace_trap");
    let src = dir.join("spin.pmc");
    fs::write(&src, "main() { 1: right(1); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("spin.pmo").to_str().unwrap()])).unwrap();
    let mut trace = Vec::new();
    let out = execute_with(
        &args(&[
            "run",
            dir.join("spin.pmx").to_str().unwrap(),
            "--max-steps",
            "3",
            "--trace",
        ]),
        &mut trace,
    )
    .unwrap();
    assert_eq!(out.code, 3);
    let text = String::from_utf8(trace).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    // ent, rgt, rgt(StepLimit at step 3) — the faulting line once, not twice
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_ne!(lines[1], lines[2], "faulting line duplicated: {lines:?}");
}

#[test]
fn run_accepts_dash_v() {
    let dir = scratch("run_v");
    let src = dir.join("ok.pmc");
    fs::write(&src, "main() { 1: mark(!); }").unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("ok.pmo").to_str().unwrap()])).unwrap();
    let out = execute(&args(&["run", dir.join("ok.pmx").to_str().unwrap(), "-v"])).unwrap();
    assert_eq!(out.code, 0);
}

#[test]
fn traced_run_off_the_code_end_matches_untraced() {
    use mtc_post_machine::cli::execute_with;
    let dir = scratch("trace_off_end");
    let pma = dir.join("off.pma");
    fs::write(&pma, ".func main\n        rgt\n").unwrap();
    execute(&args(&["asm", pma.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("off.pmo").to_str().unwrap()])).unwrap();
    let exe = dir.join("off.pmx");
    // untraced: clean trap exit
    let plain = execute(&args(&["run", exe.to_str().unwrap()])).unwrap();
    assert_eq!(plain.code, 3);
    // traced: same outcome, same stats, no panic; last line is synthetic
    let mut trace = Vec::new();
    let traced = execute_with(
        &args(&["run", exe.to_str().unwrap(), "--trace"]),
        &mut trace,
    )
    .unwrap();
    assert_eq!(traced.code, 3);
    assert_eq!(
        traced.stdout, plain.stdout,
        "traced and untraced runs must be identical"
    );
    let text = String::from_utf8(trace).unwrap();
    assert!(
        text.lines().last().unwrap().contains("<beyond code image>"),
        "{text}"
    );
}

#[test]
fn lint_reports_findings_with_exit_1_and_fix_hints() {
    let dir = scratch("lint_single");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() {\n5: right;\n007: left;\n    goto 007;\n}\n").unwrap();
    let out = execute(&args(&["lint", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    // unused-label on `5:` — gated fix hint.
    assert!(out.stdout.contains("lint: label 5 is never referenced"));
    assert!(
        out.stdout
            .contains("fix (requires --force): remove the label prefix '5:'")
    );
    // leading-zeros — safe-tier fix hint.
    assert!(out.stdout.contains("has leading zeros"));
    assert!(out.stdout.contains("  fix: rewrite '007' as '7'"));
}

#[test]
fn lint_clean_file_exits_0_and_allow_suppresses() {
    let dir = scratch("lint_clean");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() { right; }\n").unwrap();
    let out = execute(&args(&["lint", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.is_empty());

    let dirty = dir.join("dirty.pmc");
    std::fs::write(&dirty, "main() {\n5: right;\n}\n").unwrap();
    let out = execute(&args(&[
        "lint",
        dirty.to_str().unwrap(),
        "--allow",
        "unused-label",
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
}

#[test]
fn lint_unknown_allow_code_is_a_tool_error() {
    let dir = scratch("lint_badallow");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() { right; }\n").unwrap();
    let err = execute(&args(&[
        "lint",
        src.to_str().unwrap(),
        "--allow",
        "no-such-rule",
    ]))
    .unwrap_err();
    assert!(err.contains("no-such-rule"));
}

#[test]
fn lint_walks_directories_sorted_skips_dot_dirs_and_excludes() {
    let dir = scratch("lint_walk");
    std::fs::create_dir_all(dir.join("src/nested")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    // b before a alphabetically reversed on disk creation order.
    std::fs::write(dir.join("src/b.pmc"), "main() {\n5: right;\n}\n").unwrap();
    std::fs::write(
        dir.join("src/a.pmc"),
        "a() {\n6: right;\n}\nmain() { @a(); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/nested/c.pmc"),
        "c() {\n7: right;\n}\nmain() { @c(); }\n",
    )
    .unwrap();
    std::fs::write(dir.join(".hidden/d.pmc"), "main() {\n8: right;\n}\n").unwrap();
    std::fs::write(dir.join("vendor/e.pmc"), "main() {\n9: right;\n}\n").unwrap();

    let out = execute(&args(&[
        "lint",
        dir.to_str().unwrap(),
        "--exclude",
        dir.join("vendor").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    // Sorted walk: a.pmc findings before b.pmc, nested/c.pmc last.
    let a = out.stdout.find("a.pmc").unwrap();
    let b = out.stdout.find("b.pmc").unwrap();
    let c = out.stdout.find("c.pmc").unwrap();
    assert!(a < b && b < c);
    // Dot-dir and excluded subtree never appear.
    assert!(!out.stdout.contains(".hidden"));
    assert!(!out.stdout.contains("vendor"));
}

#[test]
fn lint_zero_match_path_is_an_error() {
    let dir = scratch("lint_zero");
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    let err = execute(&args(&["lint", dir.join("empty").to_str().unwrap()])).unwrap_err();
    assert!(err.contains("no .pmc files"));
}

#[test]
fn lint_batch_survives_a_fatal_file_and_still_fails() {
    let dir = scratch("lint_fatal");
    std::fs::write(dir.join("bad.pmc"), "main( {\n").unwrap();
    std::fs::write(dir.join("good.pmc"), "main() { right; }\n").unwrap();
    let out = execute(&args(&[
        "lint",
        dir.join("bad.pmc").to_str().unwrap(),
        dir.join("good.pmc").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("error:"));
    assert!(out.stderr.contains("bad.pmc"));
}
