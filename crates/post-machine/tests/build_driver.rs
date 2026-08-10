use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mtc_core::formats::object::{BlobVariant, ObjectFile, SymbolDef};
use mtc_post_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    // CARGO_TARGET_TMPDIR persists across `cargo test` runs, so a stale
    // artifact from an earlier pass could satisfy a file-existence
    // assertion after the code that writes it has broken. Start clean.
    let _ = fs::remove_dir_all(&dir);
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

/// On the PLAIN path (no `-Werror`) the compile stage's rendered warning
/// must reach the user even though a later stage — the link, here — fails:
/// the same `lone.pmc` shape warns at compile (undeclared external) and
/// then fails to link (the external stays genuinely unresolved), and both
/// must survive to the caller. Failing mutation: an early-returning `?` on
/// the link (or a write after it) that drops the warnings already rendered
/// into the driver's local buffer.
#[test]
fn argv_mode_flushes_rendered_warnings_when_the_link_fails() {
    let dir = scratch("argv_warn_then_link_fail");
    let lone = dir.join("lone.pmc");
    fs::write(&lone, "main() { @missing(); }").unwrap();
    let err = execute(&args(&["build", lone.to_str().unwrap()])).unwrap_err();
    assert!(
        err.contains("undeclared"),
        "the compile-stage warning must survive a failing link: {err}"
    );
    assert!(
        err.contains("unresolved symbols"),
        "the link error itself must still be reported: {err}"
    );
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

/// This exact byte format has a SECOND consumer that is not visible from
/// here: the VS Code extension's task provider splits these lines on TAB
/// to build its per-target tasks. Tidying the output would break the
/// editors silently, so this assertion is a contract, not a convenience.
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

/// Rewritten to a byte comparison instead of a bare `.is_file()` check.
/// `write_project`'s own `app` source (`main() { @util(); }`, calling the
/// exported `util() { mark; }`) was hand-verified to compile
/// byte-IDENTICAL at both `-O0` and `-O1` in manifest mode (27 bytes
/// either way) — a file-existence assertion built on it can never fail no
/// matter what `manifest.profiles.resolve(flags.release_preset)` does, so
/// it is not reused here. `main() { right; check(5, 5); 5: mark; }` — the
/// `check(A, A)` self-fold shape from `opt_equivalence.rs`'s
/// `check_fold_shrinks_and_preserves` (docs/pmt/language.md
/// (optimization)) — was separately hand-verified via the same debug-vs-
/// `--release` manifest-mode pair to diverge: 26 bytes plain, 24 bytes
/// under `--release`. Uses its own scratch manifest (a single `app`
/// target, not `write_project`'s) so the divergence is guaranteed by
/// construction rather than riding a shared fixture that might stop
/// diverging if `app`'s source ever changed. Failing mutation:
/// `manifest.profiles.resolve(flags.release_preset)` collapsing to
/// `resolve(false)` (i.e. `--release` never reaching profile selection) —
/// both builds would then use the debug profile and produce identical
/// bytes.
#[test]
fn release_flag_selects_the_release_profile() {
    let dir = scratch("manifest_release");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/app.pmc"),
        "main() { right; check(5, 5); 5: mark; }",
    )
    .unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": { "targets": { "app": { "sources": ["src/app.pmc"] } } } }"#,
    )
    .unwrap();

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
    let debug = fs::read(dir.join("app.pmx")).unwrap();

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
    let release = fs::read(dir.join("app.pmx")).unwrap();

    assert_ne!(
        debug, release,
        "--release must select the release profile (-O1) and reach build_one_target's CompileOptions"
    );
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

/// The manifest-mode twin of `argv_mode_flushes_rendered_warnings_when_
/// the_link_fails`: `build_one_target` has its OWN early-return sites
/// around the link and the writes after it, separate from argv mode's, so
/// each path needs its own proof. `lone.pmc`'s bare `@missing()` is
/// reachable from `main`, warns at compile, and stays genuinely
/// unresolved at link — on the plain path both must reach the user.
#[test]
fn manifest_mode_flushes_rendered_warnings_when_the_link_fails() {
    let dir = scratch("manifest_warn_then_link_fail");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lone.pmc"), "main() { @missing(); }").unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "targets": { "lone": { "sources": ["src/lone.pmc"] } }
        } }"#,
    )
    .unwrap();

    let out = pmt()
        .args(["build", "lone"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("undeclared"),
        "the compile-stage warning must survive a failing link: {stderr}"
    );
    assert!(
        stderr.contains("unresolved symbols"),
        "the link error itself must still be reported: {stderr}"
    );
}

/// `pmt build --run TARGET` builds then runs the target's `run` block,
/// adopting the machine's own exit code (docs/pmt/cli.md (build)):
/// `bench`'s `run: { tape: " *" }` reaches `halt` -> exit 2 and the
/// stdout carries the `run`-shaped `outcome:` report; `app` has no
/// `run` block at all, so `run_target` must fall back to `pmt run`'s
/// own defaults (empty tape) rather than erroring — its program stops
/// cleanly -> exit 0.
#[test]
fn build_run_adopts_the_machine_exit_code() {
    let dir = scratch("manifest_run");
    write_project(&dir);
    let out = pmt()
        .args(["build", "--run", "bench"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("outcome"));

    let out = pmt()
        .args(["build", "--run", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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
/// `strip-debugger: false` — the opposite of what `--strip-debugger`
/// asks for — must still lose to the explicit flag.
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

/// A bare `pmt lint` operates on the nearest manifest's declared source
/// set (docs/pmt/project.md (the declared source set)) — never a
/// directory scan. `src/stray.pmc` sits right next to the declared
/// files but is not named by any target's `sources`, so it must never
/// be read. The fixture is deliberately unparseable (rather than valid
/// but lint-clean code) so a regression that swaps the declared set for
/// a directory scan is guaranteed to surface: a compile fatal always
/// names its file on stderr, whereas valid PM-1 code can lint clean and
/// leave no trace either way — silently defeating the assertion below.
#[test]
fn bare_lint_uses_the_manifests_declared_source_set() {
    let dir = scratch("manifest_bare_lint");
    write_project(&dir);
    // A file NOT in the manifest must not be linted, even though it sits
    // in the same directory — the declared set is the set, never a scan.
    fs::write(dir.join("src/stray.pmc"), "@@@ not valid pmc syntax @@@").unwrap();

    let out = pmt().arg("lint").current_dir(&dir).output().unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !combined.contains("stray.pmc"),
        "undeclared file must not be linted: {combined}"
    );
}

/// With no ancestor `pmt.json` carrying a `project` section, a bare
/// `pmt lint` must error naming what it searched for — mirroring bare
/// `pmt build`'s discovery-failure message exactly.
#[test]
fn bare_lint_without_a_manifest_errors_naming_what_was_searched() {
    let empty = scratch("manifest_bare_lint_absent");
    let out = pmt().arg("lint").current_dir(&empty).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("pmt.json"), "{stderr}");
    assert!(stderr.contains("project"), "{stderr}");
}

/// `--no-config` cannot combine with a bare invocation — the manifest
/// IS the input, so skipping discovery would leave nothing to lint.
#[test]
fn bare_lint_rejects_no_config() {
    let dir = scratch("manifest_bare_lint_noconfig");
    write_project(&dir);
    let out = pmt()
        .args(["lint", "--no-config"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--no-config"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A bare `pmt fmt` formats exactly the manifest's declared source set:
/// the declared `src/app.pmc` is rewritten in place, but the undeclared
/// `src/stray.pmc` — sitting in the same directory, also unformatted —
/// is left untouched. This is the write-side twin of
/// `bare_lint_uses_the_manifests_declared_source_set`.
#[test]
fn bare_fmt_formats_exactly_the_declared_set() {
    let dir = scratch("manifest_bare_fmt");
    write_project(&dir);
    let stray = dir.join("src/stray.pmc");
    let unformatted = "main(){mark;}";
    fs::write(&stray, unformatted).unwrap();
    fs::write(dir.join("src/app.pmc"), "main(){@util();}").unwrap();

    let out = pmt().arg("fmt").current_dir(&dir).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&stray).unwrap(),
        unformatted,
        "an undeclared file must be left untouched"
    );
    assert_ne!(
        fs::read_to_string(dir.join("src/app.pmc")).unwrap(),
        "main(){@util();}",
        "a declared file is formatted in place"
    );
}

// --- Variant columns: the disk vs in-memory rule --------------------------

/// `mark; right;` fuses in the normal column and stays two transactions
/// in the gated one, so every routine here genuinely splits at `-O1` —
/// unlabelled, because a `.pmc` label starts a new block and
/// `fuse-tape-ops` only fuses within one.
const VOLATILE_MAIN: &str = "volatile main() {\n    mark;\n    right;\n    @util();\n}\n";
const NORMAL_MAIN: &str = "main() {\n    mark;\n    right;\n    @util();\n}\n";
const UTIL_FUSING: &str = "export util() {\n    mark;\n    right;\n}\n";

/// A hand-written, directive-free object: no variant records at all, i.e.
/// exactly what a pre-volatile toolchain emitted.
const LEGACY_UTIL_PMA: &str = ".func util\n        wr      1\n        rgt\n        ret\n";

/// A hand-written volatile program. The two `.volatile` lines are
/// different directives in different positions: before the first `.func`
/// it sets the object's PROGRAM bit, inside the block it tags that blob
/// as the volatile column. Both are needed — a program bit with an
/// untagged `main` is the legacy shape, and links `main` itself as a
/// counted fallback.
const VOLATILE_MAIN_PMA: &str =
    ".volatile\n.func main\n.volatile\n        call   util\n        stp\n";

fn variant_pairs(pmo: &Path, name: &str) -> Vec<BlobVariant> {
    let bytes = fs::read(pmo).unwrap();
    let obj = ObjectFile::from_bytes(&bytes).expect("reads back");
    let variants = obj
        .variants
        .as_deref()
        .expect("a compiled object is tagged");
    obj.symbols
        .iter()
        .filter(|s| s.name == name)
        .filter_map(|s| match s.def {
            SymbolDef::Defined { blob } | SymbolDef::Local { blob } => {
                Some(variants[blob as usize])
            }
            SymbolDef::External => None,
        })
        .collect()
}

#[test]
fn keep_objects_writes_both_variant_columns() {
    let dir = scratch("variant_keep_objects");
    let main = dir.join("main.pmc");
    let util = dir.join("util.pmc");
    fs::write(&main, VOLATILE_MAIN).unwrap();
    fs::write(&util, UTIL_FUSING).unwrap();

    execute(&args(&[
        "build",
        "--keep-objects",
        "-O1",
        main.to_str().unwrap(),
        util.to_str().unwrap(),
        "-o",
        dir.join("kept.pmx").to_str().unwrap(),
    ]))
    .unwrap();

    assert_eq!(
        variant_pairs(&dir.join("util.pmo"), "util"),
        vec![BlobVariant::Normal, BlobVariant::Volatile],
        "a kept object lands on disk, where it outlives the link that knew the program kind"
    );
    assert_eq!(
        variant_pairs(&dir.join("main.pmo"), "main"),
        vec![BlobVariant::Normal, BlobVariant::Volatile],
        "the entry unit is no exception — --keep-objects is the disk rule"
    );

    // Switching the whole build back to two columns is an intermediate
    // artifact decision, never an image decision: the linker selects one
    // column either way.
    execute(&args(&[
        "build",
        "-O1",
        main.to_str().unwrap(),
        util.to_str().unwrap(),
        "-o",
        dir.join("plain.pmx").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(
        fs::read(dir.join("kept.pmx")).unwrap(),
        fs::read(dir.join("plain.pmx")).unwrap(),
        "--keep-objects must not change the executable"
    );
}

/// The spec's gate: the in-memory driver compiles only the needed column,
/// and the `.pmx` it produces must equal the one built through on-disk
/// two-column objects. Runs in BOTH directions of the rule.
fn in_memory_equals_on_disk(tag: &str, main_source: &str) {
    let dir = scratch(tag);
    let main = dir.join("main.pmc");
    let util = dir.join("util.pmc");
    fs::write(&main, main_source).unwrap();
    fs::write(&util, UTIL_FUSING).unwrap();
    let mem = dir.join("mem.pmx");
    let disk = dir.join("disk.pmx");

    let built = execute(&args(&[
        "build",
        "-O1",
        "-v",
        main.to_str().unwrap(),
        util.to_str().unwrap(),
        "-o",
        mem.to_str().unwrap(),
    ]))
    .unwrap();

    execute(&args(&[
        "compile",
        "-O1",
        main.to_str().unwrap(),
        "-o",
        dir.join("main.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "compile",
        "-O1",
        util.to_str().unwrap(),
        "-o",
        dir.join("util.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    let linked = execute(&args(&[
        "link",
        "-v",
        dir.join("main.pmo").to_str().unwrap(),
        dir.join("util.pmo").to_str().unwrap(),
        "-o",
        disk.to_str().unwrap(),
    ]))
    .unwrap();

    assert_eq!(
        fs::read(&mem).unwrap(),
        fs::read(&disk).unwrap(),
        "{tag}: the in-memory column rule must not change the image"
    );
    assert_eq!(
        fs::read_to_string(dir.join("mem.pmx.map")).unwrap(),
        fs::read_to_string(dir.join("disk.pmx.map")).unwrap(),
        "{tag}: nor the debug sidecar"
    );
    for (which, out) in [("build", &built), ("link", &linked)] {
        assert!(
            !out.stderr.contains("volatile column"),
            "{tag}: {which} reported a fallback where every name offers both columns:\n{}",
            out.stderr
        );
    }
}

#[test]
fn in_memory_build_equals_the_on_disk_path_for_a_volatile_program() {
    in_memory_equals_on_disk("variant_inmem_volatile", VOLATILE_MAIN);
}

#[test]
fn in_memory_build_equals_the_on_disk_path_for_a_normal_program() {
    in_memory_equals_on_disk("variant_inmem_normal", NORMAL_MAIN);
}

/// The two paths run DIFFERENT compiler machinery, not just different
/// amounts of it: a single-column build tags every blob and dedups
/// nothing, while a both-columns build runs the transitive demotion
/// fixpoint — a function byte-identical across columns still splits when
/// a transitive callee splits. This chain is built to make that cascade
/// fire (`main` and `mid` are column-invariant on their own and demote
/// only through `leaf`, and every body stays over the inliner's limit in
/// both columns), so it is the shape where the volatile column's bytes
/// could differ between the two paths if the fixpoint leaked into them.
#[test]
fn in_memory_build_equals_the_on_disk_path_through_a_demotion_cascade() {
    const CASCADE: &str = "\
volatile main() {
    @mid();
    @mid();
    @mid();
    @mid();
    @mid();
    @mid();
    @mid();
}
mid() {
    @leaf();
    @leaf();
    @leaf();
    @leaf();
    @leaf();
    @leaf();
    @leaf();
}
leaf() {
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
    mark;
    right;
}
";
    let dir = scratch("variant_cascade");
    let src = dir.join("chain.pmc");
    fs::write(&src, CASCADE).unwrap();
    let mem = dir.join("mem.pmx");
    let disk = dir.join("disk.pmx");

    let built = execute(&args(&[
        "build",
        "-O1",
        "-v",
        src.to_str().unwrap(),
        "-o",
        mem.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        !built.stderr.contains("volatile column"),
        "every name in the chain offers the volatile column:\n{}",
        built.stderr
    );

    execute(&args(&[
        "compile",
        "-O1",
        src.to_str().unwrap(),
        "-o",
        dir.join("chain.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    // The kept object really does carry the cascade — otherwise the two
    // paths would be comparing the same work twice.
    assert_eq!(
        variant_pairs(&dir.join("chain.pmo"), "mid"),
        vec![BlobVariant::Normal, BlobVariant::Volatile],
        "mid must have demoted through leaf, or this fixture proves nothing"
    );
    execute(&args(&[
        "link",
        dir.join("chain.pmo").to_str().unwrap(),
        "-o",
        disk.to_str().unwrap(),
    ]))
    .unwrap();

    assert_eq!(
        fs::read(&mem).unwrap(),
        fs::read(&disk).unwrap(),
        "the demoted columns must be the same bytes the single-column build emits"
    );
}

/// The program bit can arrive on a `.pma` input (file-level `.volatile`),
/// not only from a `.pmc` `volatile main` — the driver's pre-scan must
/// see it, or every `.pmc` sibling compiles the wrong column.
#[test]
fn a_pma_program_bit_drives_the_in_memory_column() {
    let dir = scratch("variant_pma_bit");
    let app = dir.join("app.pma");
    let helper = dir.join("util.pmc");
    fs::write(&app, VOLATILE_MAIN_PMA).unwrap();
    fs::write(&helper, UTIL_FUSING).unwrap();
    let mem = dir.join("mem.pmx");
    let disk = dir.join("disk.pmx");

    let built = execute(&args(&[
        "build",
        "-O1",
        "-v",
        app.to_str().unwrap(),
        helper.to_str().unwrap(),
        "-o",
        mem.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        !built.stderr.contains("volatile column"),
        "the .pmc sibling must offer the volatile column:\n{}",
        built.stderr
    );

    execute(&args(&[
        "asm",
        app.to_str().unwrap(),
        "-o",
        dir.join("app.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "compile",
        "-O1",
        helper.to_str().unwrap(),
        "-o",
        dir.join("util.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "link",
        dir.join("app.pmo").to_str().unwrap(),
        dir.join("util.pmo").to_str().unwrap(),
        "-o",
        disk.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(
        fs::read(&mem).unwrap(),
        fs::read(&disk).unwrap(),
        "the in-memory path must agree with the on-disk one here too"
    );
}

#[test]
fn verbose_link_reports_the_variant_fallback_and_is_silent_at_zero() {
    let dir = scratch("variant_fallback_line");
    let legacy = dir.join("legacy.pma");
    fs::write(&legacy, LEGACY_UTIL_PMA).unwrap();

    let volatile = dir.join("volatile.pmc");
    fs::write(&volatile, "volatile main() {\n    @util();\n}\n").unwrap();
    let out = execute(&args(&[
        "build",
        "-O1",
        "-v",
        volatile.to_str().unwrap(),
        legacy.to_str().unwrap(),
        "-o",
        dir.join("v.pmx").to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        out.stderr
            .contains("link: 1 name(s) with no volatile column linked normal [util]"),
        "got:\n{}",
        out.stderr
    );

    let normal = dir.join("normal.pmc");
    fs::write(&normal, "main() {\n    @util();\n}\n").unwrap();
    let out = execute(&args(&[
        "build",
        "-O1",
        "-v",
        normal.to_str().unwrap(),
        legacy.to_str().unwrap(),
        "-o",
        dir.join("n.pmx").to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        !out.stderr.contains("volatile column"),
        "a normal program wants the normal column — nothing fell back:\n{}",
        out.stderr
    );
}

/// A `.pma`/`.pmo` sibling that declares itself volatile but defines no
/// `main`: the program bit belongs to the object that owns the ENTRY, so
/// this one must not flip a plain program's columns.
const VOLATILE_NON_ENTRY_PMA: &str =
    ".volatile\n.func util\n        wr      1\n        rgt\n        ret\n";

/// A hand-written pair: `util` in both columns, with genuinely different
/// bodies (the bare one fuses write+move, the tagged one does not) so the
/// linker's choice is visible in the image.
const UTIL_PAIR_PMA: &str = ".func util\n        wrr     1\n        ret\n\
                             .func util\n.volatile\n        wr      1\n        rgt\n        ret\n";

/// Critical case the single-kind fixtures structurally cannot see: the
/// bit is carried by an input that does NOT own the entry. The linker
/// reads the entry owner's bit alone, so the driver must too — a union
/// over the inputs compiles the wrong column here and the two paths
/// diverge at -O1.
#[test]
fn a_non_entry_inputs_program_bit_does_not_flip_the_columns() {
    let dir = scratch("variant_mixed_non_entry");
    let app = dir.join("app.pmc");
    let helper = dir.join("helper.pma");
    fs::write(&app, NORMAL_MAIN).unwrap();
    fs::write(&helper, VOLATILE_NON_ENTRY_PMA).unwrap();
    let mem = dir.join("mem.pmx");
    let disk = dir.join("disk.pmx");

    let built = execute(&args(&[
        "build",
        "-O1",
        "-v",
        app.to_str().unwrap(),
        helper.to_str().unwrap(),
        "-o",
        mem.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        !built.stderr.contains("column"),
        "the program is normal and every name offers the normal column:\n{}",
        built.stderr
    );

    execute(&args(&[
        "compile",
        "-O1",
        app.to_str().unwrap(),
        "-o",
        dir.join("app.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "asm",
        helper.to_str().unwrap(),
        "-o",
        dir.join("helper.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "link",
        dir.join("app.pmo").to_str().unwrap(),
        dir.join("helper.pmo").to_str().unwrap(),
        "-o",
        disk.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(
        fs::read(&mem).unwrap(),
        fs::read(&disk).unwrap(),
        "a bit on a non-entry input must not change the image"
    );
}

/// The inverse mix: the entry unit declares itself volatile and a
/// hand-written sibling offers both columns. The volatile bodies link,
/// nothing falls back, and the two paths agree.
#[test]
fn a_volatile_entry_links_a_hand_written_volatile_column_in_a_mixed_build() {
    let dir = scratch("variant_mixed_volatile_entry");
    let app = dir.join("app.pmc");
    let plain = dir.join("plain.pmc");
    let pair = dir.join("pair.pma");
    fs::write(&app, "volatile main() {\n    @util();\n}\n").unwrap();
    fs::write(&plain, "main() {\n    @util();\n}\n").unwrap();
    fs::write(&pair, UTIL_PAIR_PMA).unwrap();
    let mem = dir.join("mem.pmx");
    let disk = dir.join("disk.pmx");

    let built = execute(&args(&[
        "build",
        "-O1",
        "-v",
        app.to_str().unwrap(),
        pair.to_str().unwrap(),
        "-o",
        mem.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        !built.stderr.contains("column"),
        "the sibling offers both columns, so nothing falls back:\n{}",
        built.stderr
    );

    execute(&args(&[
        "compile",
        "-O1",
        app.to_str().unwrap(),
        "-o",
        dir.join("app.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "asm",
        pair.to_str().unwrap(),
        "-o",
        dir.join("pair.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "link",
        dir.join("app.pmo").to_str().unwrap(),
        dir.join("pair.pmo").to_str().unwrap(),
        "-o",
        disk.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(
        fs::read(&mem).unwrap(),
        fs::read(&disk).unwrap(),
        "the in-memory path must agree with the on-disk one"
    );

    // Non-vacuity: the same pair under a PLAIN main links the other body,
    // so the column choice demonstrably reaches the image.
    let plain_out = dir.join("plain.pmx");
    execute(&args(&[
        "build",
        "-O1",
        plain.to_str().unwrap(),
        pair.to_str().unwrap(),
        "-o",
        plain_out.to_str().unwrap(),
    ]))
    .unwrap();
    assert_ne!(
        fs::read(&mem).unwrap(),
        fs::read(&plain_out).unwrap(),
        "the two columns carry different code, so the images must differ"
    );
}

/// The `.pmo` leg of the pre-scan: an entry unit arriving as an
/// already-compiled object contributes its header bit like any other.
#[test]
fn a_pmo_entry_units_program_bit_drives_the_in_memory_column() {
    let dir = scratch("variant_pmo_bit");
    let app = dir.join("app.pmc");
    let util = dir.join("util.pmc");
    fs::write(&app, VOLATILE_MAIN).unwrap();
    fs::write(&util, UTIL_FUSING).unwrap();
    let app_pmo = dir.join("app.pmo");
    execute(&args(&[
        "compile",
        "-O1",
        app.to_str().unwrap(),
        "-o",
        app_pmo.to_str().unwrap(),
    ]))
    .unwrap();

    let mem = dir.join("mem.pmx");
    let built = execute(&args(&[
        "build",
        "-O1",
        "-v",
        app_pmo.to_str().unwrap(),
        util.to_str().unwrap(),
        "-o",
        mem.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        !built.stderr.contains("column"),
        "the .pmc sibling must offer the volatile column:\n{}",
        built.stderr
    );

    execute(&args(&[
        "compile",
        "-O1",
        util.to_str().unwrap(),
        "-o",
        dir.join("util.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    let disk = dir.join("disk.pmx");
    execute(&args(&[
        "link",
        app_pmo.to_str().unwrap(),
        dir.join("util.pmo").to_str().unwrap(),
        "-o",
        disk.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(
        fs::read(&mem).unwrap(),
        fs::read(&disk).unwrap(),
        "an object input's bit must drive the sibling's column"
    );
}

/// Column selection is symmetric, so the `-v` sentence must be: a NORMAL
/// program reaching a name that offers only the volatile column falls
/// back too, and the line has to name the column that was missing rather
/// than assume the volatile one.
#[test]
fn verbose_link_names_whichever_column_was_missing() {
    let dir = scratch("variant_fallback_direction");
    let volatile_only = dir.join("util.pma");
    fs::write(
        &volatile_only,
        ".func util\n.volatile\n        wr      1\n        rgt\n        ret\n",
    )
    .unwrap();
    let normal = dir.join("normal.pmc");
    fs::write(&normal, "main() {\n    @util();\n}\n").unwrap();

    let out = execute(&args(&[
        "build",
        "-O1",
        "-v",
        normal.to_str().unwrap(),
        volatile_only.to_str().unwrap(),
        "-o",
        dir.join("n.pmx").to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        out.stderr
            .contains("link: 1 name(s) with no normal column linked volatile [util]"),
        "got:\n{}",
        out.stderr
    );
}

/// A library can own the entry symbol (`pmt build util.pmc -L lib -l
/// entry`), and then ITS object carries the program bit for the whole
/// build. Both directions: bit set → the units compile the volatile
/// column; bit clear → normal. Either way the in-memory image must equal
/// the on-disk one and the `-v` line must stay silent.
fn library_entry_case(tag: &str, entry_pma: &str) {
    let dir = scratch(tag);
    let lib_dir = dir.join("lib");
    fs::create_dir_all(&lib_dir).unwrap();
    let entry_src = dir.join("entry.pma");
    fs::write(&entry_src, entry_pma).unwrap();
    execute(&args(&[
        "asm",
        entry_src.to_str().unwrap(),
        "-o",
        lib_dir.join("entry.pmo").to_str().unwrap(),
    ]))
    .unwrap();

    let util = dir.join("util.pmc");
    fs::write(&util, UTIL_FUSING).unwrap();
    let mem = dir.join("mem.pmx");
    let disk = dir.join("disk.pmx");

    let built = execute(&args(&[
        "build",
        "-O1",
        "-v",
        util.to_str().unwrap(),
        "-L",
        lib_dir.to_str().unwrap(),
        "-l",
        "entry",
        "-o",
        mem.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        !built.stderr.contains("column"),
        "{tag}: every reached name offers the column the entry selects:\n{}",
        built.stderr
    );

    execute(&args(&[
        "compile",
        "-O1",
        util.to_str().unwrap(),
        "-o",
        dir.join("util.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    execute(&args(&[
        "link",
        dir.join("util.pmo").to_str().unwrap(),
        "-L",
        lib_dir.to_str().unwrap(),
        "-l",
        "entry",
        "-o",
        disk.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(
        fs::read(&mem).unwrap(),
        fs::read(&disk).unwrap(),
        "{tag}: a library-owned entry must drive the in-memory column too"
    );
}

#[test]
fn a_library_owned_entry_with_the_bit_drives_the_in_memory_column() {
    library_entry_case(
        "variant_lib_entry_volatile",
        ".volatile\n.func main\n.volatile\n        call    util\n        stp\n",
    );
}

#[test]
fn a_library_owned_entry_without_the_bit_stays_normal() {
    library_entry_case(
        "variant_lib_entry_plain",
        ".func main\n        call    util\n        stp\n",
    );
}

/// The two library-entry cases must not produce the same image, or the
/// pair above would pass with the bit ignored entirely.
#[test]
fn the_two_library_entry_cases_select_different_code() {
    let dir = scratch("variant_lib_entry_differ");
    let lib_dir = dir.join("lib");
    fs::create_dir_all(&lib_dir).unwrap();
    let util = dir.join("util.pmc");
    fs::write(&util, UTIL_FUSING).unwrap();

    let mut images = Vec::new();
    for (name, pma) in [
        (
            "vol",
            ".volatile\n.func main\n.volatile\n        call    util\n        stp\n",
        ),
        ("plain", ".func main\n        call    util\n        stp\n"),
    ] {
        let src = dir.join(format!("{name}.pma"));
        fs::write(&src, pma).unwrap();
        execute(&args(&[
            "asm",
            src.to_str().unwrap(),
            "-o",
            lib_dir.join("entry.pmo").to_str().unwrap(),
        ]))
        .unwrap();
        let out = dir.join(format!("{name}.pmx"));
        execute(&args(&[
            "build",
            "-O1",
            util.to_str().unwrap(),
            "-L",
            lib_dir.to_str().unwrap(),
            "-l",
            "entry",
            "-o",
            out.to_str().unwrap(),
        ]))
        .unwrap();
        images.push(fs::read(&out).unwrap());
    }
    assert_ne!(
        images[0], images[1],
        "the volatile column of `util` is different code from its normal one"
    );
}
