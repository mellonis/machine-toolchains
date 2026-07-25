use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mtc_post_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

const MAIN_CALLS_UTIL: &str = "main() { @util(); }";
const UTIL_EXPORTED: &str = "export util() { mark; }";

#[test]
fn argv_mode_compiles_and_links_multiple_pmc_inputs_in_memory() {
    let dir = scratch("argv_two_pmc");
    let main = dir.join("main.pmc");
    let util = dir.join("util.pmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();
    fs::write(&util, UTIL_EXPORTED).unwrap();

    let out = execute(&args(&[
        "build",
        main.to_str().unwrap(),
        util.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(
        dir.join("main.pmx").is_file(),
        "default output = first input's stem + .pmx"
    );
    assert!(dir.join("main.pmx.map").is_file(), "sidecar rides along");
    assert!(
        !dir.join("main.pmo").exists(),
        "no disk intermediates by default"
    );
}

#[test]
fn argv_mode_keep_objects_writes_pmo_next_to_each_source() {
    let dir = scratch("argv_keep_objects");
    let main = dir.join("main.pmc");
    let util = dir.join("util.pmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();
    fs::write(&util, UTIL_EXPORTED).unwrap();

    execute(&args(&[
        "build",
        "--keep-objects",
        main.to_str().unwrap(),
        util.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(dir.join("main.pmo").is_file());
    assert!(dir.join("util.pmo").is_file());
}

#[test]
fn argv_mode_accepts_mixed_pmc_and_pmo_inputs() {
    let dir = scratch("argv_mixed");
    let util = dir.join("util.pmc");
    fs::write(&util, UTIL_EXPORTED).unwrap();
    execute(&args(&["compile", util.to_str().unwrap()])).unwrap();
    let main = dir.join("main.pmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();

    let out = execute(&args(&[
        "build",
        main.to_str().unwrap(),
        dir.join("util.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(dir.join("main.pmx").is_file());
}

#[test]
fn argv_mode_refines_undeclared_external_resolved_by_a_sibling() {
    let dir = scratch("argv_refine");
    let main = dir.join("main.pmc");
    let util = dir.join("util.pmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();
    fs::write(&util, UTIL_EXPORTED).unwrap();

    // `@util()` in main.pmc is a bare undeclared external per-file, but
    // the declared set (both files) resolves it — no warning survives,
    // so -Werror over the POST-filter set succeeds.
    let out = execute(&args(&[
        "build",
        "-Werror",
        main.to_str().unwrap(),
        util.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(!out.stderr.contains("undeclared"), "{}", out.stderr);

    // A genuinely unresolvable bare external still warns and -Werror fails.
    let lone = dir.join("lone.pmc");
    fs::write(&lone, "main() { @missing(); }").unwrap();
    let err = execute(&args(&["build", "-Werror", lone.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("treated as errors"), "{err}");
}

#[test]
fn mixing_files_and_target_names_is_an_error() {
    let dir = scratch("argv_mixing");
    let main = dir.join("main.pmc");
    fs::write(&main, "main() { mark; }").unwrap();
    let err = execute(&args(&["build", main.to_str().unwrap(), "sometarget"])).unwrap_err();
    assert!(err.contains("not both"), "{err}");
}

#[test]
fn argv_mode_rejects_s_and_emit_ir() {
    let dir = scratch("argv_no_inspect_flags");
    let main = dir.join("main.pmc");
    fs::write(&main, "main() { mark; }").unwrap();
    for flag in ["-S", "--emit-ir"] {
        let err = execute(&args(&["build", flag, main.to_str().unwrap()])).unwrap_err();
        assert!(err.contains("unknown flag"), "{flag}: {err}");
    }
}

fn pmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pmt"))
}

fn write_project(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/shared.pmc"), "export util() { mark; }").unwrap();
    fs::write(dir.join("src/app.pmc"), "main() { @util(); }").unwrap();
    fs::write(
        dir.join("src/bench.pmc"),
        "export start() { @util(); halt; }",
    )
    .unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "sources": ["src/shared.pmc"],
            "targets": {
                "app":   { "sources": ["src/app.pmc"] },
                "bench": { "sources": ["src/bench.pmc"], "entry": "start",
                           "run": { "tape": " *" } }
            }
        } }"#,
    )
    .unwrap();
}

#[test]
fn manifest_mode_bare_build_builds_all_targets_alphabetically() {
    let dir = scratch("manifest_all");
    write_project(&dir);
    let out = pmt().arg("build").current_dir(&dir).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.join("app.pmx").is_file(),
        "default output <name>.pmx next to manifest"
    );
    assert!(dir.join("app.pmx.map").is_file());
    assert!(dir.join("bench.pmx").is_file());
}

#[test]
fn manifest_mode_named_target_builds_only_it() {
    let dir = scratch("manifest_named");
    write_project(&dir);
    let out = pmt()
        .args(["build", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(dir.join("app.pmx").is_file());
    assert!(!dir.join("bench.pmx").exists());
}

#[test]
fn manifest_mode_discovery_walks_up_from_a_subdirectory() {
    let dir = scratch("manifest_walkup");
    write_project(&dir);
    let out = pmt()
        .args(["build", "app"])
        .current_dir(dir.join("src"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.join("app.pmx").is_file(),
        "outputs resolve against the MANIFEST dir, not cwd"
    );
}

#[test]
fn manifest_mode_rejects_declared_model_flags() {
    let dir = scratch("manifest_reject_flags");
    write_project(&dir);
    for flagset in [
        vec!["-o", "x.pmx"],
        vec!["-L", "libs"],
        vec!["-l", "x"],
        vec!["--nostdlib"],
    ] {
        let mut cmd = pmt();
        cmd.arg("build").args(&flagset).arg("app").current_dir(&dir);
        let out = cmd.output().unwrap();
        assert!(
            !out.status.success(),
            "{flagset:?} must be rejected in manifest mode"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("manifest"), "{flagset:?}: {stderr}");
    }
}

#[test]
fn manifest_mode_unknown_target_and_missing_manifest_error() {
    let dir = scratch("manifest_unknown");
    write_project(&dir);
    let out = pmt()
        .args(["build", "nosuch"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nosuch"));

    let empty = scratch("manifest_absent");
    let out = pmt().arg("build").current_dir(&empty).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("project"));
}

#[test]
fn list_targets_prints_name_and_run_marker() {
    let dir = scratch("manifest_list");
    write_project(&dir);
    let out = pmt()
        .args(["build", "--list-targets"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "app\nbench\trun\n");
}

#[test]
fn release_flag_selects_the_release_profile() {
    let dir = scratch("manifest_release");
    write_project(&dir);
    let out = pmt()
        .args(["build", "--release", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join("app.pmx").is_file());
}
