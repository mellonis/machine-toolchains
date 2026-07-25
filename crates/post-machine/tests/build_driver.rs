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

/// The flags-win contract (docs/pmt/cli.md (build)): a manifest profile
/// key is a default, not a floor — an individual invocation flag must
/// still override it. The manifest declares `werror: false` (matching
/// the debug profile's own base, so this isn't exercising a base-vs-
/// override difference by accident). `dead()` is exported but never
/// called from `main`, so it is unreachable and the linker drops it
/// (docs/core.md (linker)) — its genuinely-unresolvable `@missing()`
/// call never becomes a link error, only the compile-time
/// undeclared-external warning, which only `-Werror` turns fatal.
#[test]
fn werror_flag_overrides_a_manifest_profile_that_disables_it() {
    let dir = scratch("manifest_werror_flag_wins");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/orphan.pmc"),
        "main() { mark; }\nexport dead() { @missing(); }",
    )
    .unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "profiles": { "debug": { "werror": false } },
            "targets": { "orphan": { "sources": ["src/orphan.pmc"] } }
        } }"#,
    )
    .unwrap();

    let out = pmt()
        .args(["build", "orphan"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "manifest profile disables werror: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = pmt()
        .args(["build", "-Werror", "orphan"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "-Werror flag must override the manifest profile's werror: false"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("treated as errors"));
}

/// `--run` needs exactly one selected target; with the two-target
/// fixture and no target named, the gate must fire BEFORE either
/// target builds (docs/pmt/cli.md (build)) — no `.pmx` written.
#[test]
fn run_flag_needs_exactly_one_target_and_fails_before_building() {
    let dir = scratch("manifest_run_needs_one");
    write_project(&dir);
    let out = pmt()
        .args(["build", "--run"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("exactly one target"));
    assert!(
        !dir.join("app.pmx").exists(),
        "the --run gate must fire before any target builds"
    );
    assert!(!dir.join("bench.pmx").exists());
}

/// `app.pmc`'s bare `@util()` is undeclared per-file but resolved by
/// the manifest-level `sources` (`shared.pmc`) once the effective
/// source list is compiled together — the undeclared-external
/// refinement (docs/pmt/cli.md (build)) must drop that warning in
/// manifest mode exactly as it does in argv mode.
#[test]
fn manifest_mode_refines_undeclared_external_resolved_by_shared_sources() {
    let dir = scratch("manifest_refine_undeclared");
    write_project(&dir);
    let out = pmt()
        .args(["build", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("undeclared"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A target's `libraries.dirs` must resolve against the manifest's
/// directory, not the process cwd — proven by building from a
/// subdirectory (`src/`) while the declared `libs` dir sits next to
/// the manifest.
#[test]
fn library_dirs_resolve_against_the_manifest_directory_not_cwd() {
    let dir = scratch("manifest_library_dirs");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("libs")).unwrap();
    let lib_src = dir.join("libs/libutil.pmc");
    fs::write(&lib_src, "export from_lib() { mark; }").unwrap();
    let lib_obj = dir.join("libs/libutil.pmo");
    let compiled = pmt()
        .args([
            "compile",
            lib_src.to_str().unwrap(),
            "-o",
            lib_obj.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    fs::write(dir.join("src/uselib.pmc"), "main() { @from_lib(); }").unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "targets": { "uselib": {
                "sources": ["src/uselib.pmc"],
                "libraries": { "dirs": ["libs"], "link": ["libutil"] }
            } }
        } }"#,
    )
    .unwrap();

    let out = pmt()
        .args(["build", "uselib"])
        .current_dir(dir.join("src"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join("uselib.pmx").is_file());
}

/// Finding 1's fix: `--keep-objects` must write the intermediate
/// `.pmo` for a `.pma` manifest source exactly as it does for `.pmc`
/// sources (docs/pmt/cli.md (build)) — this fixture is the first to
/// give the `.pma` arm of `build_one_target` any coverage.
#[test]
fn manifest_mode_keep_objects_writes_pmo_for_pma_sources_too() {
    let dir = scratch("manifest_keep_objects_pma");
    fs::create_dir_all(dir.join("src")).unwrap();
    let pmc = dir.join("src/asmtarget.pmc");
    fs::write(&pmc, "main() { mark; }").unwrap();
    let compiled = pmt()
        .args(["compile", pmc.to_str().unwrap(), "-S"])
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    assert!(dir.join("src/asmtarget.pma").is_file());

    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "targets": { "asmtarget": { "sources": ["src/asmtarget.pma"] } }
        } }"#,
    )
    .unwrap();

    let out = pmt()
        .args(["build", "--keep-objects", "asmtarget"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.join("src/asmtarget.pmo").is_file(),
        "--keep-objects must write the .pma source's intermediate .pmo too"
    );
}

/// `-g`'s effect is observable in the `.pmx.map` sidecar: `MapFunction`
/// lines are only recorded from `-g` objects (docs/formats.md); a
/// manifest profile that declares `debug-info: false` — the opposite of
/// the debug preset's own default — must still lose to an explicit
/// `-g` flag.
#[test]
fn debug_info_flag_overrides_a_manifest_profile_that_disables_it() {
    let dir = scratch("manifest_debug_info_flag_wins");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/prog.pmc"), "main() { mark; }").unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "profiles": { "debug": { "debug-info": false } },
            "targets": { "prog": { "sources": ["src/prog.pmc"] } }
        } }"#,
    )
    .unwrap();

    let out = pmt()
        .args(["build", "prog"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let map = mtc_core::linker::MapFile::from_json(
        &fs::read_to_string(dir.join("prog.pmx.map")).unwrap(),
    )
    .unwrap();
    assert!(
        map.functions.iter().all(|f| f.lines.is_empty()),
        "manifest profile disables debug info: {map:?}"
    );

    let out = pmt()
        .args(["build", "-g", "prog"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let map = mtc_core::linker::MapFile::from_json(
        &fs::read_to_string(dir.join("prog.pmx.map")).unwrap(),
    )
    .unwrap();
    assert!(
        map.functions.iter().any(|f| !f.lines.is_empty()),
        "-g flag must override the manifest profile's debug-info: false: {map:?}"
    );
}

/// `-O0`'s effect is observable in the `-v` opt report: `optimize`
/// returns immediately at `-O0` with `rounds == 0`, while `-O1` always
/// runs at least one round even when it finds nothing to change
/// (docs/pmt/language.md (optimization)) — so the rendered
/// `"opt: N round(s)"` line differs deterministically between the two
/// levels for any program. A manifest profile that declares `opt: O1`
/// must still lose to an explicit `-O0` flag.
#[test]
fn o0_flag_overrides_a_manifest_profile_that_declares_o1() {
    let dir = scratch("manifest_o0_flag_wins");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/prog.pmc"), "main() { mark; }").unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "profiles": { "debug": { "opt": "O1" } },
            "targets": { "prog": { "sources": ["src/prog.pmc"] } }
        } }"#,
    )
    .unwrap();

    let out = pmt()
        .args(["build", "-v", "prog"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("opt: 1 round(s)"),
        "manifest profile declares opt: O1: {stderr}"
    );

    let out = pmt()
        .args(["build", "-v", "-O0", "prog"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("opt: 0 round(s)"),
        "-O0 flag must override the manifest profile's opt: O1: {stderr}"
    );
}

/// The `-O1` counterpart of the test above: a manifest profile that
/// declares `opt: O0` — matching the debug preset's own default, made
/// explicit — must still lose to an explicit `-O1` flag. Covered as its
/// own test because `flags.o0` and `flags.o1` are independent `if`
/// branches in `build_one_target`; an inverted condition in either one
/// would only be caught by exercising both directions.
#[test]
fn o1_flag_overrides_a_manifest_profile_that_declares_o0() {
    let dir = scratch("manifest_o1_flag_wins");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/prog.pmc"), "main() { mark; }").unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "profiles": { "debug": { "opt": "O0" } },
            "targets": { "prog": { "sources": ["src/prog.pmc"] } }
        } }"#,
    )
    .unwrap();

    let out = pmt()
        .args(["build", "-v", "prog"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("opt: 0 round(s)"),
        "manifest profile declares opt: O0: {stderr}"
    );

    let out = pmt()
        .args(["build", "-v", "-O1", "prog"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("opt: 1 round(s)"),
        "-O1 flag must override the manifest profile's opt: O0: {stderr}"
    );
}

/// `--strip-debugger`'s effect is observable via `pmt dis`: a kept
/// `debugger;` statement disassembles as `brk`, a stripped one doesn't
/// (docs/pmt/isa.md). A manifest profile that declares
/// `strip-debugger: false` — the opposite of the debug preset's own
/// default — must still lose to an explicit `--strip-debugger` flag.
#[test]
fn strip_debugger_flag_overrides_a_manifest_profile_that_keeps_it() {
    let dir = scratch("manifest_strip_debugger_flag_wins");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/prog.pmc"), "main() { mark; debugger; mark; }").unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "profiles": { "debug": { "strip-debugger": false } },
            "targets": { "prog": { "sources": ["src/prog.pmc"] } }
        } }"#,
    )
    .unwrap();

    let out = pmt()
        .args(["build", "prog"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dis = pmt()
        .args(["dis", dir.join("prog.pmx").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        dis.status.success(),
        "{}",
        String::from_utf8_lossy(&dis.stderr)
    );
    assert!(
        String::from_utf8_lossy(&dis.stdout).contains("brk"),
        "manifest profile keeps the debugger statement: {}",
        String::from_utf8_lossy(&dis.stdout)
    );

    let out = pmt()
        .args(["build", "--strip-debugger", "prog"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dis = pmt()
        .args(["dis", dir.join("prog.pmx").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        dis.status.success(),
        "{}",
        String::from_utf8_lossy(&dis.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&dis.stdout).contains("brk"),
        "--strip-debugger flag must override the manifest profile's strip-debugger: false: {}",
        String::from_utf8_lossy(&dis.stdout)
    );
}
