use std::fs;
use std::path::PathBuf;

use mtc_post_machine::cli::execute;

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
