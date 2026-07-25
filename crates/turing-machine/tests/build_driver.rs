use std::fs;
use std::path::PathBuf;

use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A one-state, one-tape program with no external references — used
/// wherever a fixture just needs to compile cleanly.
const TRIVIAL_TMC: &str = "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state s { [*] -> stop; }
}
";

/// A bare (unqualified, undeclared) call to `util` — the same shape as
/// PM-1's `@util()`: undeclared per-file, resolved once `util.tmc` is
/// compiled alongside it (docs/tmt/cli.md (build)).
const MAIN_CALLS_UTIL: &str = "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { [*] -> call util() then done; }
  state done { [*] -> stop; }
}
";
const UTIL_EXPORTED: &str = "\
alphabet ab { '_', 'a' }
export routine util(tape t: ab) {
  entry state s { [*] -> return; }
}
";

#[test]
fn argv_mode_compiles_and_links_multiple_tmc_inputs_in_memory() {
    let dir = scratch("argv_two_tmc");
    let main = dir.join("main.tmc");
    let util = dir.join("util.tmc");
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
        dir.join("main.tmx").is_file(),
        "default output = first input's stem + .tmx"
    );
    assert!(dir.join("main.tmx.map").is_file(), "sidecar rides along");
    assert!(
        !dir.join("main.tmo").exists(),
        "no disk intermediates by default"
    );
}

#[test]
fn argv_mode_keep_objects_writes_tmo_next_to_each_source() {
    let dir = scratch("argv_keep_objects");
    let main = dir.join("main.tmc");
    let util = dir.join("util.tmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();
    fs::write(&util, UTIL_EXPORTED).unwrap();

    execute(&args(&[
        "build",
        "--keep-objects",
        main.to_str().unwrap(),
        util.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(dir.join("main.tmo").is_file());
    assert!(dir.join("util.tmo").is_file());
}

#[test]
fn argv_mode_accepts_mixed_tmc_and_tmo_inputs() {
    let dir = scratch("argv_mixed");
    let util = dir.join("util.tmc");
    fs::write(&util, UTIL_EXPORTED).unwrap();
    execute(&args(&["compile", util.to_str().unwrap()])).unwrap();
    let main = dir.join("main.tmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();

    let out = execute(&args(&[
        "build",
        main.to_str().unwrap(),
        dir.join("util.tmo").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(dir.join("main.tmx").is_file());
}

/// PM-1's finding-1 fix (`--keep-objects` must cover `.pma` too, not only
/// `.pmc`) has a TM analogue: the shared `load_one_source` dispatch must
/// write the intermediate `.tmo` for a `.tma` source exactly as it does
/// for `.tmc` (docs/tmt/cli.md (build)). Failing mutation: dropping the
/// `keep_objects` write from the `"tma"` arm of `load_one_source`.
#[test]
fn argv_mode_keep_objects_writes_tmo_for_a_tma_source_too() {
    let dir = scratch("argv_keep_objects_tma");
    let main = dir.join("main.tmc");
    fs::write(&main, TRIVIAL_TMC).unwrap();
    let compiled = execute(&args(&["compile", main.to_str().unwrap(), "-S"])).unwrap();
    assert_eq!(compiled.code, 0);
    let tma = dir.join("main.tma");
    assert!(tma.is_file());

    execute(&args(&["build", "--keep-objects", tma.to_str().unwrap()])).unwrap();
    assert!(
        dir.join("main.tmo").is_file(),
        "--keep-objects must write the .tma source's intermediate .tmo too"
    );
}

#[test]
fn argv_mode_refines_undeclared_external_resolved_by_a_sibling() {
    let dir = scratch("argv_refine");
    let main = dir.join("main.tmc");
    let util = dir.join("util.tmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();
    fs::write(&util, UTIL_EXPORTED).unwrap();

    // `call util()` in main.tmc is a bare undeclared external per-file, but
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
    let lone = dir.join("lone.tmc");
    fs::write(
        &lone,
        "alphabet ab { '_', 'a' }\n\
         machine {\n\
           tape t: ab;\n\
           entry state go { [*] -> call missing() then done; }\n\
           state done { [*] -> stop; }\n\
         }\n",
    )
    .unwrap();
    let err = execute(&args(&["build", "-Werror", lone.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("treated as errors"), "{err}");
}

#[test]
fn mixing_files_and_target_names_is_an_error() {
    let dir = scratch("argv_mixing");
    let main = dir.join("main.tmc");
    fs::write(&main, TRIVIAL_TMC).unwrap();
    let err = execute(&args(&["build", main.to_str().unwrap(), "sometarget"])).unwrap_err();
    assert!(err.contains("not both"), "{err}");
}

#[test]
fn argv_mode_rejects_s_emit_ir_and_stamped_asm() {
    let dir = scratch("argv_no_inspect_flags");
    let main = dir.join("main.tmc");
    fs::write(&main, TRIVIAL_TMC).unwrap();
    for flag in ["-S", "--emit-ir", "--stamped-asm"] {
        let err = execute(&args(&["build", flag, main.to_str().unwrap()])).unwrap_err();
        assert!(err.contains("unknown flag"), "{flag}: {err}");
    }
}
