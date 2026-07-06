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

    // Verify the CLI result matches the last snapshot (ruling R4: last-wins)
    assert_eq!(
        cli_ir, last_snapshot,
        "CLI --emit-ir result should match the LAST occurrence of {} per ruling R4",
        repeating_label
    );
}
