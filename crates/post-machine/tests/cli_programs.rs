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
    assert_eq!(mtc_post_machine::PMC_LANG_VERSION, "0.3");
}

#[test]
fn version_reports_exactly_three_lines() {
    let out = execute(&args(&["--version"])).unwrap();
    assert_eq!(
        out.stdout,
        format!(
            "pmt {}\npmc language {}\npma dialect (pm-1) {}\n",
            env!("CARGO_PKG_VERSION"),
            mtc_post_machine::PMC_LANG_VERSION,
            mtc_post_machine::PM1_PMA_DIALECT_VERSION
        )
    );
    assert_eq!(mtc_post_machine::PM1_PMA_DIALECT_VERSION, "0.3");
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
    assert!(ir.contains("\"version\": 4"));
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
    assert!(ir.contains("\"version\": 4"));
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

    // Verify the CLI result matches the last snapshot (docs/pmt/cli.md: repeated
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
fn tape_new_from_pmx_creates_blank_template() {
    let dir = scratch("tape_new");
    let src = dir.join("h.pmc");
    fs::write(&src, HELLO).unwrap();
    execute(&args(&["compile", src.to_str().unwrap()])).unwrap();
    execute(&args(&["link", dir.join("h.pmo").to_str().unwrap()])).unwrap();

    let blank = dir.join("t.pmt");
    execute(&args(&[
        "tape",
        "new",
        "--from",
        dir.join("h.pmx").to_str().unwrap(),
        "-o",
        blank.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(blank.exists());

    let shown = execute(&args(&["tape", "show", blank.to_str().unwrap()])).unwrap();
    // A v1 image has one tape; the template is empty with the head at 0.
    assert!(shown.stdout.contains("tape 0:"), "{}", shown.stdout);
    assert!(shown.stdout.contains("head 0"), "{}", shown.stdout);
    assert!(!shown.stdout.contains("tape 1:"), "{}", shown.stdout);
    // Empty cells render as the bare bordered span `||`.
    assert!(shown.stdout.contains("||"), "{}", shown.stdout);
}

#[test]
fn tape_set_clones_with_edits() {
    let dir = scratch("tape_set");
    let a = dir.join("a.pmt");
    let b = dir.join("b.pmt");
    // a: cells [blank, mark], head 1.
    execute(&args(&[
        "tape",
        "build",
        " *",
        "--head",
        "1",
        "-o",
        a.to_str().unwrap(),
    ]))
    .unwrap();
    // Clone a into b with both cells marked and the head moved to 0.
    execute(&args(&[
        "tape",
        "set",
        a.to_str().unwrap(),
        "-o",
        b.to_str().unwrap(),
        "--cells",
        "**",
        "--head",
        "0",
    ]))
    .unwrap();

    let shown_b = execute(&args(&["tape", "show", b.to_str().unwrap()])).unwrap();
    assert!(shown_b.stdout.contains("|**|"), "{}", shown_b.stdout);
    assert!(shown_b.stdout.contains("head 0"), "{}", shown_b.stdout);

    // Clone semantics: the source is untouched.
    let shown_a = execute(&args(&["tape", "show", a.to_str().unwrap()])).unwrap();
    assert!(shown_a.stdout.contains("| *|"), "{}", shown_a.stdout);
    assert!(shown_a.stdout.contains("head 1"), "{}", shown_a.stdout);
}

#[test]
fn tape_set_requires_output() {
    let dir = scratch("tape_set_no_out");
    let a = dir.join("a.pmt");
    execute(&args(&["tape", "build", " *", "-o", a.to_str().unwrap()])).unwrap();
    // Neither -o nor --in-place: refuse rather than silently overwrite.
    let err = execute(&args(&["tape", "set", a.to_str().unwrap(), "--head", "2"])).unwrap_err();
    assert!(err.contains("--in-place"), "{err}");
    assert!(err.contains("USAGE: pmt tape"), "{err}");
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
    assert!(err.contains("no .pmc or .pma files found"), "{err}");
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

#[test]
fn lint_pmt_json_allow_suppresses_a_finding() {
    let dir = scratch("lint_config_allow");
    std::fs::write(
        dir.join("pmt.json"),
        r#"{"lint":{"allow":["unused-label"]}}"#,
    )
    .unwrap();
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() {\n5: right;\n}\n").unwrap();

    let out = execute(&args(&["lint", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "pmt.json's allow-list suppresses the finding");
    assert!(out.stdout.is_empty());
}

#[test]
fn lint_pmt_json_allow_unions_with_allow_flag() {
    let dir = scratch("lint_config_union");
    std::fs::write(
        dir.join("pmt.json"),
        r#"{"lint":{"allow":["unused-label"]}}"#,
    )
    .unwrap();
    let src = dir.join("prog.pmc");
    // Two distinct findings: unused-label (file-suppressed) and
    // leading-zeros (flag-suppressed) — both must disappear together.
    std::fs::write(&src, "main() {\n5: right;\n007: left;\n    goto 007;\n}\n").unwrap();

    let out = execute(&args(&[
        "lint",
        src.to_str().unwrap(),
        "--allow",
        "leading-zeros",
    ]))
    .unwrap();
    assert_eq!(
        out.code, 0,
        "file allow ∪ flag allow must suppress both findings"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn lint_no_config_flag_ignores_pmt_json() {
    let dir = scratch("lint_config_no_config");
    std::fs::write(
        dir.join("pmt.json"),
        r#"{"lint":{"allow":["unused-label"]}}"#,
    )
    .unwrap();
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() {\n5: right;\n}\n").unwrap();

    let out = execute(&args(&["lint", src.to_str().unwrap(), "--no-config"])).unwrap();
    assert_eq!(out.code, 1, "--no-config ignores pmt.json entirely");
    assert!(out.stdout.contains("label 5 is never referenced"));
}

#[test]
fn lint_invalid_pmt_json_is_a_per_file_error_and_batch_continues() {
    let dir = scratch("lint_config_invalid");
    std::fs::create_dir_all(dir.join("bad")).unwrap();
    std::fs::create_dir_all(dir.join("good")).unwrap();
    // A typo'd top-level key: `lints` instead of `lint`.
    std::fs::write(dir.join("bad/pmt.json"), r#"{"lints":{}}"#).unwrap();
    std::fs::write(dir.join("bad/skip.pmc"), "main() { right; }\n").unwrap();
    // No pmt.json under good/ — this file lints on its own, proving the
    // batch continues past the bad config rather than aborting whole-tool.
    std::fs::write(dir.join("good/prog.pmc"), "main() {\n9: right;\n}\n").unwrap();

    let out = execute(&args(&[
        "lint",
        dir.join("bad/skip.pmc").to_str().unwrap(),
        dir.join("good/prog.pmc").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    let bad_config = dir.join("bad/pmt.json");
    assert!(
        out.stderr.contains(bad_config.to_str().unwrap()),
        "stderr names the pmt.json path: {}",
        out.stderr
    );
    assert!(out.stderr.contains("error:"));
    assert!(out.stderr.contains("unknown key `lints`"));
    // The file the bad config would have configured is skipped entirely.
    assert!(!out.stdout.contains("skip.pmc"));
    // The batch continues: good/prog.pmc (no config of its own) still
    // lints, and its finding shows up.
    assert!(out.stdout.contains("label 9 is never referenced"));
}

#[test]
fn fix_applies_safe_tier_only_and_force_unlocks_deletions() {
    let dir = scratch("lint_fix");
    let src = dir.join("prog.pmc");
    let original = "main() {\n    goto 007;\n5: right;\n7: left;\n}\n";
    std::fs::write(&src, original).unwrap();

    // Plain --fix: leading-zeros applied, unused-label deletion left.
    let out = execute(&args(&["lint", src.to_str().unwrap(), "--fix"])).unwrap();
    let fixed = std::fs::read_to_string(&src).unwrap();
    assert!(fixed.contains("goto 7;"), "safe tier applied");
    assert!(fixed.contains("5: right;"), "gated deletion NOT applied");
    assert_eq!(out.code, 1, "the unused label remains a finding");

    // --fix --force: the unused-label prefix goes too; re-run is clean.
    let out = execute(&args(&["lint", src.to_str().unwrap(), "--fix", "--force"])).unwrap();
    assert_eq!(out.code, 0);
    let fixed = std::fs::read_to_string(&src).unwrap();
    assert!(!fixed.contains("5:"));

    // Idempotence: a second forced run changes nothing and stays clean.
    let before = std::fs::read_to_string(&src).unwrap();
    let out = execute(&args(&["lint", src.to_str().unwrap(), "--fix", "--force"])).unwrap();
    assert_eq!(out.code, 0);
    assert_eq!(std::fs::read_to_string(&src).unwrap(), before);
}

#[test]
fn force_without_fix_errors_and_fatal_files_are_never_written() {
    let dir = scratch("lint_force");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() { right; }\n").unwrap();
    let err = execute(&args(&["lint", src.to_str().unwrap(), "--force"])).unwrap_err();
    assert!(err.contains("--force requires --fix"));

    let bad = dir.join("bad.pmc");
    let broken = "main( {\n";
    std::fs::write(&bad, broken).unwrap();
    let out = execute(&args(&["lint", bad.to_str().unwrap(), "--fix", "--force"])).unwrap();
    assert_eq!(out.code, 1);
    assert_eq!(
        std::fs::read_to_string(&bad).unwrap(),
        broken,
        "never written"
    );
}

#[test]
fn lint_exclude_is_component_based_not_string_prefix() {
    let dir = scratch("lint_exclude_prefix");
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    std::fs::create_dir_all(dir.join("vendored")).unwrap();
    std::fs::write(dir.join("vendor/a.pmc"), "main() {\n5: right;\n}\n").unwrap();
    std::fs::write(dir.join("vendored/b.pmc"), "main() {\n6: right;\n}\n").unwrap();
    let out = execute(&args(&[
        "lint",
        dir.to_str().unwrap(),
        "--exclude",
        dir.join("vendor").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    // `vendor/` is pruned; the sibling `vendored/` (a superstring, not a
    // path-component prefix) is NOT pruned and its findings appear.
    assert!(out.stdout.contains("vendored"));
    assert!(out.stdout.contains("b.pmc"));
    assert!(!out.stdout.contains("vendor/a.pmc") && !out.stdout.contains("vendor\\a.pmc"));
}

// --- `pmt lint` routes `.pma` (plan 2, task 5): extension routing over
// the union of the pmc and asm rule tables. ---

#[test]
fn lint_mixed_dir_lints_both_pmc_and_pma() {
    let dir = scratch("lint_mixed_dir");
    std::fs::write(dir.join("prog.pmc"), "main() {\n5: right;\n}\n").unwrap();
    std::fs::write(dir.join("prog.pma"), ".func f\nUNUSED: nop\n        stp\n").unwrap();

    let out = execute(&args(&["lint", dir.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(
        out.stdout.contains("label 5 is never referenced"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("label `UNUSED` is never referenced (function `f`)"),
        "{}",
        out.stdout
    );
}

#[test]
fn lint_explicit_unknown_extension_is_a_per_file_error_batch_continues() {
    let dir = scratch("lint_unknown_ext");
    let bad = dir.join("foo.txt");
    std::fs::write(&bad, "not a source file\n").unwrap();
    let good = dir.join("good.pmc");
    std::fs::write(&good, "main() {\n5: right;\n}\n").unwrap();

    let out = execute(&args(&[
        "lint",
        bad.to_str().unwrap(),
        good.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains(&format!(
            "{}: error: unknown source extension (expected .pmc or .pma)",
            bad.display()
        )),
        "{}",
        out.stderr
    );
    // The batch continues past the bad extension: good.pmc still lints.
    assert!(out.stdout.contains("label 5 is never referenced"));
}

#[test]
fn lint_bad_allow_aborts_up_front_even_for_an_all_unknown_extension_batch() {
    // Regression: the unknown-extension route never calls
    // `validate_allow` itself (nothing there is language-specific to
    // check against). Without an up-front validation ahead of the file
    // loop, a batch made entirely of unknown-extension files would never
    // reach either route's per-file `validate_allow` call, and a bad
    // `--allow` would go unnoticed instead of aborting the run.
    let dir = scratch("lint_bad_allow_unknown_ext");
    std::fs::write(dir.join("foo.txt"), "not a source file\n").unwrap();

    let err = execute(&args(&[
        "lint",
        dir.join("foo.txt").to_str().unwrap(),
        "--allow",
        "nonsense",
    ]))
    .unwrap_err();
    assert!(err.contains("nonsense"));
}

#[test]
fn lint_pma_fix_removes_unused_label_and_reruns_clean() {
    let dir = scratch("lint_pma_fix");
    let src = dir.join("prog.pma");
    std::fs::write(&src, ".func f\nUNUSED: nop\n        stp\n").unwrap();

    // unused-label is machine-applicable on the asm side too — plain
    // `--fix` (no `--force`) removes it, same tier as pmc's.
    let out = execute(&args(&["lint", src.to_str().unwrap(), "--fix"])).unwrap();
    assert_eq!(out.code, 0, "{}", out.stdout);
    let fixed = std::fs::read_to_string(&src).unwrap();
    assert!(!fixed.contains("UNUSED"), "{fixed}");

    // A genuinely separate re-run confirms the file on disk is clean,
    // not just the in-flight re-lint inside the --fix invocation.
    let out = execute(&args(&["lint", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.is_empty());
}

#[test]
fn lint_pma_fix_leaves_a_gated_leftover_debugger_and_force_removes_it() {
    // `leftover-debugger` is the gated (MaybeIncorrect) `.pma` rule —
    // mirrors `fix_applies_safe_tier_only_and_force_unlocks_deletions`
    // (the `.pmc` side's safe-vs-gated split) for the asm route's own
    // gated fix, through the same `--fix`/`--fix --force` batch loop.
    let dir = scratch("lint_pma_leftover_debugger");
    let src = dir.join("prog.pma");
    let original = ".func f\n        brk\n        stp\n";
    std::fs::write(&src, original).unwrap();

    // Plain `--fix`: the `brk` is gated, left in place, still reported
    // with the force hint.
    let out = execute(&args(&["lint", src.to_str().unwrap(), "--fix"])).unwrap();
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert_eq!(
        std::fs::read_to_string(&src).unwrap(),
        original,
        "gated fix NOT applied"
    );
    assert!(
        out.stdout
            .contains("fix (requires --force): remove the debugger break"),
        "{}",
        out.stdout
    );

    // `--fix --force`: the `brk` goes; re-lint (a separate run) is clean.
    let out = execute(&args(&["lint", src.to_str().unwrap(), "--fix", "--force"])).unwrap();
    assert_eq!(out.code, 0, "{}", out.stdout);
    let fixed = std::fs::read_to_string(&src).unwrap();
    assert!(!fixed.contains("brk"), "{fixed}");

    let out = execute(&args(&["lint", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.is_empty());
}

#[test]
fn lint_allow_unreachable_code_is_accepted_and_suppresses_on_pma() {
    let dir = scratch("lint_pma_allow_union");
    let src = dir.join("prog.pma");
    std::fs::write(&src, ".func f\n        stp\n        nop\n").unwrap();

    let out = execute(&args(&["lint", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("unreachable code"), "{}", out.stdout);

    let out = execute(&args(&[
        "lint",
        src.to_str().unwrap(),
        "--allow",
        "unreachable-code",
    ]))
    .unwrap();
    assert_eq!(out.code, 0, "{}", out.stdout);
    assert!(out.stdout.is_empty());
}

#[test]
fn lint_allow_pmc_only_code_is_accepted_on_a_pma_only_run() {
    // "non-camel-case" names no asm rule — it's pmc-only. The union
    // validator must still accept it even when the batch is all `.pma`.
    let dir = scratch("lint_pma_allow_pmc_code");
    let src = dir.join("prog.pma");
    std::fs::write(&src, ".func f\n        stp\n").unwrap();

    let out = execute(&args(&[
        "lint",
        src.to_str().unwrap(),
        "--allow",
        "non-camel-case",
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.is_empty());
}

#[test]
fn lint_allow_nonsense_still_aborts_the_whole_run() {
    let dir = scratch("lint_pma_allow_nonsense");
    let src = dir.join("prog.pma");
    std::fs::write(&src, ".func f\n        stp\n").unwrap();

    let err = execute(&args(&[
        "lint",
        src.to_str().unwrap(),
        "--allow",
        "nonsense",
    ]))
    .unwrap_err();
    assert!(err.contains("nonsense"));
}

#[test]
fn lint_pma_raw_line_is_a_per_file_error_on_stderr() {
    let dir = scratch("lint_pma_raw_line");
    let bad = dir.join("listing.pma");
    std::fs::write(&bad, "<goToEnd>\n").unwrap();
    let good = dir.join("good.pma");
    std::fs::write(&good, ".func f\n        stp\n").unwrap();

    let out = execute(&args(&[
        "lint",
        bad.to_str().unwrap(),
        good.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains(&format!(
            "{}:1:1: error: not assembly text [raw-line]",
            bad.display()
        )),
        "{}",
        out.stderr
    );
}

#[test]
fn lint_pmt_json_allow_merges_for_pma_identically_to_pmc() {
    let dir = scratch("lint_pma_config_allow");
    std::fs::write(
        dir.join("pmt.json"),
        r#"{"lint":{"allow":["unused-label"]}}"#,
    )
    .unwrap();
    let src = dir.join("prog.pma");
    std::fs::write(&src, ".func f\nUNUSED: nop\n        stp\n").unwrap();

    let out = execute(&args(&["lint", src.to_str().unwrap()])).unwrap();
    assert_eq!(
        out.code, 0,
        "pmt.json's allow-list suppresses the finding on .pma exactly as on .pmc"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn lsp_help_prints_usage_without_touching_stdio() {
    let out = execute(&args(&["lsp", "--help"])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("USAGE: pmt lsp"), "{}", out.stdout);
    assert!(
        out.stdout
            .contains("Exit code: 0 after shutdown/exit, 1 on exit without shutdown."),
        "{}",
        out.stdout
    );
}

#[test]
fn lsp_rejects_extra_arguments() {
    // Bare `pmt lsp` (no args) would hand real stdio to the server loop
    // and block this test process waiting for a client — never invoke
    // it here. An extra positional is rejected before that happens.
    let err = execute(&args(&["lsp", "extra-arg"])).unwrap_err();
    assert!(err.contains("lsp takes no arguments"), "{err}");
}

#[test]
fn top_level_help_lists_lsp() {
    let out = execute(&[]).unwrap();
    assert!(
        out.stdout
            .contains("  lsp          run the LSP server on stdio")
    );
}

#[test]
fn completions_zsh_emits_a_compdef_script() {
    let out = execute(&args(&["completions", "zsh"])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.starts_with("#compdef pmt"), "{}", out.stdout);
}

#[test]
fn completions_bash_and_fish_are_recognized_but_not_yet_implemented() {
    let bash_err = execute(&args(&["completions", "bash"])).unwrap_err();
    assert!(bash_err.contains("not implemented"), "{bash_err}");
    let fish_err = execute(&args(&["completions", "fish"])).unwrap_err();
    assert!(fish_err.contains("not implemented"), "{fish_err}");
}

#[test]
fn completions_unknown_shell_is_a_clear_error() {
    let err = execute(&args(&["completions", "powershell"])).unwrap_err();
    assert!(err.contains("unknown shell"), "{err}");
}

#[test]
fn completions_help_and_missing_shell_name() {
    let help = execute(&args(&["completions", "--help"])).unwrap();
    assert!(help.stdout.contains("USAGE: pmt completions"));
    let err = execute(&args(&["completions"])).unwrap_err();
    assert!(
        err.contains("completions takes exactly one shell name"),
        "{err}"
    );
}

// --- `pmt fmt` -------------------------------------------------------

/// Spawns the real `pmt` binary with piped stdio: the only way to feed
/// `pmt fmt -` a controlled stdin from an in-process test (the `-`
/// variants read the real process stdin, which `execute()`'s in-process
/// calls above never touch).
fn run_pmt_stdin(fmt_args: &[&str], stdin_data: &str) -> std::process::Output {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut child = Command::new(env!("CARGO_BIN_EXE_pmt"))
        .args(fmt_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn pmt");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_data.as_bytes())
        .unwrap();
    child.wait_with_output().expect("failed to wait on pmt")
}

#[test]
fn fmt_writes_in_place_only_when_changed() {
    let dir = scratch("fmt_inplace");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() { right; }").unwrap();

    let out = execute(&args(&["fmt", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    let formatted = std::fs::read_to_string(&src).unwrap();
    assert_eq!(formatted, "main() {\n    right;\n}\n");

    // Idempotence: a second run on an already-formatted file must not
    // touch it at all (no spurious mtime churn).
    let before = std::fs::metadata(&src).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let out = execute(&args(&["fmt", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    let after = std::fs::metadata(&src).unwrap().modified().unwrap();
    assert_eq!(
        before, after,
        "already-formatted file must not be rewritten"
    );
    assert_eq!(std::fs::read_to_string(&src).unwrap(), formatted);
}

#[test]
fn fmt_check_reports_and_writes_nothing() {
    let dir = scratch("fmt_check");
    let dirty = dir.join("dirty.pmc");
    let original = "main() { right; }";
    std::fs::write(&dirty, original).unwrap();

    let out = execute(&args(&["fmt", "--check", dirty.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains(dirty.to_str().unwrap()));
    assert_eq!(
        std::fs::read_to_string(&dirty).unwrap(),
        original,
        "never written"
    );

    let clean = dir.join("clean.pmc");
    std::fs::write(&clean, "main() {\n    right;\n}\n").unwrap();
    let out = execute(&args(&["fmt", "--check", clean.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.is_empty());
}

#[test]
fn fmt_dash_reads_stdin_and_writes_stdout() {
    let out = run_pmt_stdin(&["fmt", "-"], "main() { right; }");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "main() {\n    right;\n}\n"
    );
}

#[test]
fn fmt_dash_check_exits_0_or_1_with_no_output() {
    let clean = run_pmt_stdin(&["fmt", "-", "--check"], "main() {\n    right;\n}\n");
    assert!(clean.status.success());
    assert!(clean.stdout.is_empty());

    let dirty = run_pmt_stdin(&["fmt", "-", "--check"], "main() { right; }");
    assert_eq!(dirty.status.code(), Some(1));
    assert!(dirty.stdout.is_empty());
}

#[test]
fn fmt_dash_combined_with_path_is_an_error() {
    let dir = scratch("fmt_dash_combo");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() { right; }").unwrap();
    let err = execute(&args(&["fmt", "-", src.to_str().unwrap()])).unwrap_err();
    assert!(err.contains('-'), "{err}");
}

#[test]
fn fmt_dash_parse_error_goes_to_stderr_and_nothing_to_stdout() {
    let out = run_pmt_stdin(&["fmt", "-"], "main() { 1: right; 1: left; }");
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(!out.stderr.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));
}

#[test]
fn fmt_walks_directories_sorted_skips_dot_dirs_and_excludes() {
    let dir = scratch("fmt_walk");
    std::fs::create_dir_all(dir.join("src/nested")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    std::fs::write(dir.join("src/b.pmc"), "main() { right; }").unwrap();
    std::fs::write(dir.join("src/a.pmc"), "main() { left; }").unwrap();
    std::fs::write(dir.join("src/nested/c.pmc"), "main() { mark; }").unwrap();
    std::fs::write(dir.join(".hidden/d.pmc"), "main() { right; }").unwrap();
    std::fs::write(dir.join("vendor/e.pmc"), "main() { right; }").unwrap();

    let out = execute(&args(&[
        "fmt",
        "--check",
        dir.to_str().unwrap(),
        "--exclude",
        dir.join("vendor").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    let a = out.stdout.find("a.pmc").unwrap();
    let b = out.stdout.find("b.pmc").unwrap();
    let c = out.stdout.find("c.pmc").unwrap();
    assert!(a < b && b < c);
    assert!(!out.stdout.contains(".hidden"));
    assert!(!out.stdout.contains("vendor"));
}

#[test]
fn fmt_zero_match_path_is_an_error() {
    let dir = scratch("fmt_zero");
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    let err = execute(&args(&["fmt", dir.join("empty").to_str().unwrap()])).unwrap_err();
    assert!(err.contains("no .pmc or .pma files found"), "{err}");
}

#[test]
fn fmt_batch_survives_a_fatal_file_and_still_fails() {
    let dir = scratch("fmt_fatal");
    std::fs::write(dir.join("bad.pmc"), "main( {\n").unwrap();
    std::fs::write(dir.join("good.pmc"), "main() { right; }").unwrap();
    let out = execute(&args(&[
        "fmt",
        dir.join("bad.pmc").to_str().unwrap(),
        dir.join("good.pmc").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("error:"));
    assert!(out.stderr.contains("bad.pmc"));
    assert_eq!(
        std::fs::read_to_string(dir.join("bad.pmc")).unwrap(),
        "main( {\n",
        "a fatal file is never written"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("good.pmc")).unwrap(),
        "main() {\n    right;\n}\n",
        "the batch continues past the fatal file"
    );
}

#[test]
fn fmt_exclude_prunes_a_subtree_and_an_explicit_file() {
    let dir = scratch("fmt_exclude");
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    std::fs::write(dir.join("vendor/a.pmc"), "main() { right; }").unwrap();
    std::fs::write(dir.join("keep.pmc"), "main() { left; }").unwrap();
    std::fs::write(dir.join("skip.pmc"), "main() { mark; }").unwrap();

    let out = execute(&args(&[
        "fmt",
        "--check",
        dir.to_str().unwrap(),
        "--exclude",
        dir.join("vendor").to_str().unwrap(),
        "--exclude",
        dir.join("skip.pmc").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("keep.pmc"));
    assert!(!out.stdout.contains("vendor"));
    assert!(!out.stdout.contains("skip.pmc"));
}

// --- `pmt fmt` routes `.pma` (plan 2, task 6): extension routing, the
// `.pma` in-place writer, stdin `--lang`, and the per-file
// unknown-extension error — the fmt-side mirror of the lint battery
// above. ---

#[test]
fn fmt_mixed_dir_formats_both_pmc_and_pma() {
    // Regression test for the live bug this task fixes: `collect_sources`
    // (shared with `pmt lint`) already walked `.pma` files, but fmt's
    // per-file loop used to run every collected file through the `.pmc`
    // parser regardless of extension — a `.pma` in the batch failed with
    // a `.pmc` lex error. Routing by extension fixes it; this dir mixes
    // both languages to prove the fix and double as the regression net.
    let dir = scratch("fmt_mixed_dir");
    std::fs::write(dir.join("prog.pmc"), "main() { right; }").unwrap();
    std::fs::write(dir.join("prog.pma"), ".func f\nL1 :  rgt\n jm L1\n").unwrap();

    let out = execute(&args(&["fmt", dir.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        std::fs::read_to_string(dir.join("prog.pmc")).unwrap(),
        "main() {\n    right;\n}\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("prog.pma")).unwrap(),
        ".func f\nL1:     rgt\n        jm      L1\n"
    );
}

#[test]
fn fmt_pma_only_dir_is_not_a_zero_match_error() {
    // The extension-routed walk makes an all-`.pma` directory a valid
    // fmt target on its own, with no `.pmc` file needed alongside.
    let dir = scratch("fmt_pma_only_dir");
    std::fs::write(dir.join("prog.pma"), ".func f\n        rgt\n").unwrap();

    let out = execute(&args(&["fmt", dir.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn fmt_pma_writes_in_place_only_when_changed() {
    let dir = scratch("fmt_pma_inplace");
    let src = dir.join("prog.pma");
    std::fs::write(&src, ".func f\nrgt\n").unwrap();

    let out = execute(&args(&["fmt", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    let formatted = std::fs::read_to_string(&src).unwrap();
    assert_eq!(formatted, ".func f\n        rgt\n");

    // Idempotence: a second run on the now-canonical file must not touch
    // it at all (no spurious mtime churn).
    let before = std::fs::metadata(&src).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let out = execute(&args(&["fmt", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    let after = std::fs::metadata(&src).unwrap().modified().unwrap();
    assert_eq!(
        before, after,
        "already-formatted .pma file must not be rewritten"
    );
}

#[test]
fn fmt_pma_check_reports_and_writes_nothing() {
    let dir = scratch("fmt_pma_check");
    let dirty = dir.join("dirty.pma");
    let original = ".func f\nrgt\n";
    std::fs::write(&dirty, original).unwrap();

    let out = execute(&args(&["fmt", "--check", dirty.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains(dirty.to_str().unwrap()));
    assert_eq!(
        std::fs::read_to_string(&dirty).unwrap(),
        original,
        "never written"
    );

    let clean = dir.join("clean.pma");
    std::fs::write(&clean, ".func f\n        rgt\n").unwrap();
    let out = execute(&args(&["fmt", "--check", clean.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.is_empty());
}

#[test]
fn fmt_dash_lang_pma_grids_a_scrambled_snippet() {
    let out = run_pmt_stdin(
        &["fmt", "-", "--lang", "pma"],
        ".func f\nL1 :  rgt\n jm L1\n",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        ".func f\nL1:     rgt\n        jm      L1\n"
    );
}

#[test]
fn fmt_dash_lang_defaults_to_pmc() {
    // No `--lang` at all still formats stdin as `.pmc` — the default.
    let out = run_pmt_stdin(&["fmt", "-", "--lang", "pmc"], "main() { right; }");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "main() {\n    right;\n}\n"
    );
}

#[test]
fn fmt_dash_lang_rejects_an_unknown_value() {
    let out = run_pmt_stdin(&["fmt", "-", "--lang", "bogus"], "main() { right; }");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("`--lang` takes pmc or pma"));
}

#[test]
fn fmt_lang_with_paths_is_rejected() {
    let dir = scratch("fmt_lang_with_paths");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() { right; }").unwrap();
    let err = execute(&args(&["fmt", "--lang", "pmc", src.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("--lang applies to stdin (-) only"), "{err}");
}

#[test]
fn fmt_explicit_unknown_extension_is_a_per_file_error_batch_continues() {
    let dir = scratch("fmt_unknown_ext");
    let bad = dir.join("foo.txt");
    std::fs::write(&bad, "not a source file\n").unwrap();
    let good = dir.join("good.pmc");
    std::fs::write(&good, "main() { right; }").unwrap();

    let out = execute(&args(&[
        "fmt",
        bad.to_str().unwrap(),
        good.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains(&format!(
            "{}: error: unknown source extension (expected .pmc or .pma)",
            bad.display()
        )),
        "{}",
        out.stderr
    );
    // The batch continues past the bad extension: good.pmc still formats.
    assert_eq!(
        std::fs::read_to_string(&good).unwrap(),
        "main() {\n    right;\n}\n"
    );
}

#[test]
fn fmt_pma_raw_line_is_a_per_file_error_on_stderr() {
    let dir = scratch("fmt_pma_raw_line");
    let bad = dir.join("listing.pma");
    std::fs::write(&bad, "<goToEnd>\n").unwrap();
    let good = dir.join("good.pma");
    std::fs::write(&good, ".func f\nrgt\n").unwrap();

    let out = execute(&args(&[
        "fmt",
        bad.to_str().unwrap(),
        good.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains(&format!(
            "{}:1:1: error: not assembly text [raw-line]",
            bad.display()
        )),
        "{}",
        out.stderr
    );
    // The batch continues past the fatal file: good.pma still formats.
    assert_eq!(
        std::fs::read_to_string(&good).unwrap(),
        ".func f\n        rgt\n"
    );
}
