use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mtc_turing_machine::cli::execute;

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

/// A committed `.tmc` fixture under `tests/golden/`, shared with
/// `tmc_golden.rs`/`cli_programs.rs`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
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

/// `--entry` is one of the two fields TM's argv mode adds beyond PM's
/// baseline (PM's argv mode always takes `LinkOptions::default()` for it,
/// so there is no inherited test to lean on). `other.tmc` defines no
/// `main` symbol at all — only an exported `other` — so a bare build must
/// fail with the default-entry error, and only `--entry other` can make
/// it succeed. This proves `flags.entry` actually reaches
/// `LinkOptions.entry` rather than being parsed and dropped.
#[test]
fn argv_mode_entry_flag_selects_a_non_default_root() {
    let dir = scratch("argv_entry_flag");
    let src = dir.join("other.tmc");
    fs::write(
        &src,
        "alphabet ab { '_', 'a' }\n\
         export routine other(tape t: ab) {\n\
           entry state s { [*] -> stop; }\n\
         }\n",
    )
    .unwrap();

    let err = execute(&args(&["build", src.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("main"), "{err}");

    let out = execute(&args(&["build", "--entry", "other", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(dir.join("other.tmx").is_file());
}

/// `--call-mech` is the other field TM's argv mode adds beyond PM's
/// baseline. `a5_call_across_alphabets.tmc`'s cross-alphabet `with map`
/// call site genuinely composes differently under mono (a specialized
/// stamped copy) versus frames (the generic compose directory) — hand
/// verified: 182 bytes under `--call-mech mono`, 166 under
/// `--call-mech frames`, for the identical source. If
/// `call_mech: flags.call_mech.unwrap_or_default()` ever reverted to a
/// hardcoded default (or the field dropped from the `LinkOptions`
/// literal), both builds below would collapse to the same bytes.
#[test]
fn argv_mode_call_mech_flag_changes_the_linked_image() {
    let dir = scratch("argv_call_mech_flag");
    let src = fixture("a5_call_across_alphabets.tmc");
    let mono = dir.join("mono.tmx");
    let frames = dir.join("frames.tmx");

    let out = execute(&args(&[
        "build",
        "--call-mech",
        "mono",
        "-o",
        mono.to_str().unwrap(),
        src.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    let out = execute(&args(&[
        "build",
        "--call-mech",
        "frames",
        "-o",
        frames.to_str().unwrap(),
        src.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);

    assert_ne!(
        fs::read(&mono).unwrap(),
        fs::read(&frames).unwrap(),
        "mono and frames should compose the cross-alphabet call site differently"
    );
}

// ── manifest mode ────────────────────────────────────────────────────────
//
// Discovery starts at the process cwd, so these tests spawn the real `tmt`
// binary (`current_dir` on an in-process `execute` call would race every
// other test in the same process) — mirrors post-machine's
// `crates/post-machine/tests/build_driver.rs`.

fn tmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tmt"))
}

/// `crates/turing-machine/tests/golden/a1_replace_b.tmc`, pasted verbatim
/// rather than read at test time (the golden suite already pins it).
const A1_REPLACE_B: &str = "\
? Walk right; replace every 'b' with 'a'; stop at the first blank.

alphabet ab { '_', 'a', 'b' }

machine {
  tape main: ab;

  entry state scan {
    ['b'] -> write ['a'] move [>] goto scan;
    ['a'] ->            move [>] goto scan;
    ['_'] -> stop;
  }
}
";

/// `crates/turing-machine/tests/golden/a2_binary_plus_one.tmc`, pasted
/// verbatim.
const A2_BINARY_PLUS_ONE: &str = "\
alphabet bits { '_', '0', '1' }

machine {
  tape num: bits;                    // head on the least significant digit

  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;   // carry
    ['0'] -> write ['1'] stop;
    ['_'] -> write ['1'] stop;
  }
}
";

/// Three targets over the two real `.tmc` fixtures above. `app` carries a
/// `run` block with a `.tmt` tape; `notape` deliberately has none (a
/// future run-driver test's pointed no-run-block error needs exactly this
/// shape); `zmono` pins its own `call-mech` and a nested custom `output`
/// path, so both a non-default lowering and a non-default output land in
/// the same fixture the "build everything" test already exercises.
///
/// The bootstrap build (`tmt build app`) exists only to mint a
/// band-compatible `.tmt` tape via `tmt tape new --from`
/// (docs/tmt/cli.md (tape)); its `app.tmx`/`app.tmx.map` are removed
/// immediately after. Without that cleanup, every later test that builds
/// (or asserts the absence of) `app.tmx` would pass off the bootstrap's
/// own artifact regardless of what `manifest_mode` actually did — a
/// vacuous test that cannot fail.
fn write_project(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/app.tmc"), A1_REPLACE_B).unwrap();
    fs::write(dir.join("src/other.tmc"), A2_BINARY_PLUS_ONE).unwrap();
    fs::write(
        dir.join("tmt.json"),
        r#"{ "project": {
            "call-mech": "hybrid",
            "targets": {
                "app":    { "sources": ["src/app.tmc"],
                            "run": { "tape": "tapes/app-in.tmt", "max-steps": 100000 } },
                "notape": { "sources": ["src/other.tmc"] },
                "zmono":  { "sources": ["src/other.tmc"], "call-mech": "mono",
                            "output": "out/zmono.tmx" }
            }
        } }"#,
    )
    .unwrap();

    let bootstrap = tmt()
        .args(["build", "app"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        bootstrap.status.success(),
        "bootstrap build for the tape mint: {}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );
    fs::create_dir_all(dir.join("tapes")).unwrap();
    let out = tmt()
        .args(["tape", "new", "--from", "app.tmx", "-o", "tapes/app-in.tmt"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    fs::remove_file(dir.join("app.tmx")).unwrap();
    fs::remove_file(dir.join("app.tmx.map")).unwrap();
}

/// The rejection guard (docs/tmt/project.md (call-mech)): manifest mode
/// rejects `-o`/`-L`/`-l`/`--nostdlib` exactly as the PM-1 driver's does,
/// PLUS `--entry` — TM's argv mode has an `--entry` flag PM's doesn't,
/// and it contradicts a target's declared `entry` key the same way `-o`
/// contradicts a declared `output`. Failing mutation: dropping
/// `flags.entry.is_some()` from the guard's condition — `--entry other`
/// would then build successfully instead of erroring, so BOTH assertions
/// below catch it (`!out.status.success()` because the build no longer
/// fails at all, and the `"manifest"` substring check because a
/// successful build's stderr never contains it either). The substring
/// check alone would NOT catch a narrower mutation that reworded the
/// error message while still failing the build — that gap is accepted,
/// not closed, by this test.
#[test]
fn manifest_mode_rejects_declared_model_flags_including_entry() {
    let dir = scratch("tm_manifest_reject_flags");
    write_project(&dir);
    for flagset in [
        vec!["-o", "x.tmx"],
        vec!["-L", "libs"],
        vec!["-l", "x"],
        vec!["--nostdlib"],
        vec!["--entry", "other"],
    ] {
        let mut cmd = tmt();
        cmd.arg("build").args(&flagset).arg("app").current_dir(&dir);
        let out = cmd.output().unwrap();
        assert!(
            !out.status.success(),
            "{flagset:?} must be rejected in manifest mode"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("manifest"),
            "{flagset:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// `--call-mech` is the one flag manifest mode ACCEPTS as an override
/// (maintainer ruling: the manifest records the committed lowering, the
/// flag exists to experiment against it) — unlike
/// `-o`/`-L`/`-l`/`--nostdlib`/`--entry` above. Failing mutation: the
/// rejection guard growing a sixth `|| flags.call_mech.is_some()` arm —
/// this build would then fail instead of succeeding. This test only
/// proves the flag is ACCEPTED, not that it changes anything —
/// `a1_replace_b.tmc` has no call sites, so mono/frames/hybrid all
/// compose to the same bytes for `app`. The resolution-order test below
/// (`call_mech_flag_wins_over_the_declared_project_key`) proves the
/// override actually reaches the linker's lowering choice.
#[test]
fn call_mech_flag_overrides_the_manifest_declaration() {
    let dir = scratch("tm_manifest_call_mech");
    write_project(&dir);
    let out = tmt()
        .args(["build", "--call-mech", "frames", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join("app.tmx").is_file());
}

/// `crates/turing-machine/tests/golden/a5_call_across_alphabets.tmc`,
/// pasted verbatim — the one fixture already hand-verified (in
/// `argv_mode_call_mech_flag_changes_the_linked_image` above) to compose
/// its cross-alphabet `with map` call site differently under mono versus
/// frames, so it is the one fixture that can tell the two lowerings
/// apart by linked bytes.
const A5_CALL_ACROSS_ALPHABETS: &str = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b', '0', '1' }

namespace mylib {
  export routine plusOne(tape num: bits) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*]   -> write ['1'] return;
    }
  }
}

use mylib::plusOne;

machine {
  tape ctl:  bits;
  tape data: wide;

  entry state main {
    ['1', *] -> call plusOne(num = data with map { '0'->'0', '1'->'1' }) then done;
    [*, *]   -> move [>, .] goto main;
  }

  state done { [*, *] -> stop; }
}
";

/// The precedence chain `flags.call_mech.or_else(|| manifest.
/// effective_call_mech(target)).unwrap_or_default()` (docs/tmt/
/// project.md (call-mech)) has two links `call_mech_flag_overrides_the_
/// manifest_declaration` above cannot exercise (its fixture has no call
/// sites, so it only proves the flag is ACCEPTED): that the declared
/// project-level `call-mech` key actually reaches the linker at all, and
/// that an explicit `--call-mech` flag wins over it. One target, one
/// output path, `tmt.json` rewritten between builds so the ONLY thing
/// that varies is the declared key or the flag — never the source or the
/// output path. Failing mutations this catches: (a) dropping `.or_else`
/// entirely (or replacing it with `.unwrap_or_default()` alone) so the
/// declared project key is never read — `mono` and `frames` builds would
/// collapse to the same bytes; (b) reversing the `.or_else` to
/// `manifest.effective_call_mech(target).or(flags.call_mech)` so the
/// declared key wins instead of the flag — the final build would match
/// `MONO` instead of `FRAMES`.
#[test]
fn call_mech_flag_wins_over_the_declared_project_key() {
    let dir = scratch("tm_manifest_call_mech_order");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/app.tmc"), A5_CALL_ACROSS_ALPHABETS).unwrap();
    let manifest = |call_mech: &str| {
        format!(
            r#"{{ "project": {{
                "call-mech": "{call_mech}",
                "targets": {{ "app": {{ "sources": ["src/app.tmc"] }} }}
            }} }}"#
        )
    };

    fs::write(dir.join("tmt.json"), manifest("mono")).unwrap();
    let out = tmt().arg("build").current_dir(&dir).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mono = fs::read(dir.join("app.tmx")).unwrap();

    fs::write(dir.join("tmt.json"), manifest("frames")).unwrap();
    let out = tmt().arg("build").current_dir(&dir).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let frames = fs::read(dir.join("app.tmx")).unwrap();
    assert_ne!(
        mono, frames,
        "the declared project-level call-mech key must reach the linker"
    );

    // Declare mono again, but override it with the flag: the result must
    // match the FRAMES bytes above, not the declared MONO.
    fs::write(dir.join("tmt.json"), manifest("mono")).unwrap();
    let out = tmt()
        .args(["build", "--call-mech", "frames"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let flag_wins = fs::read(dir.join("app.tmx")).unwrap();
    assert_eq!(
        flag_wins, frames,
        "--call-mech must win over the declared project-level call-mech key"
    );
}

/// The PM-1 driver's `manifest_mode_bare_build_builds_all_targets_
/// alphabetically`, ported (`.pmx` -> `.tmx`): a bare `tmt build` with
/// no target named builds every target, including `zmono`'s non-default
/// nested `output` path and non-default `call-mech`. Also carries
/// `--keep-objects` (not in the PM-1 original) since this is the one
/// test that builds every target sharing `src/other.tmc` — the cheapest
/// place to prove `flags.keep_objects` threads through
/// `build_one_target`'s call into the shared `load_one_source` at all;
/// hardcoding `keep_objects: false` there would go unnoticed by every
/// other test in this file.
#[test]
fn manifest_mode_bare_build_builds_all_targets_alphabetically() {
    let dir = scratch("tm_manifest_all");
    write_project(&dir);
    let out = tmt()
        .args(["build", "--keep-objects"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.join("app.tmx").is_file(),
        "default output <name>.tmx next to manifest"
    );
    assert!(dir.join("app.tmx.map").is_file());
    assert!(dir.join("notape.tmx").is_file());
    assert!(
        dir.join("out/zmono.tmx").is_file(),
        "a target's own custom output path is honoured"
    );
    assert!(
        dir.join("src/other.tmo").is_file(),
        "--keep-objects must thread through manifest mode's build_one_target"
    );
}

/// The PM-1 driver's `manifest_mode_named_target_builds_only_it`, ported.
#[test]
fn manifest_mode_named_target_builds_only_it() {
    let dir = scratch("tm_manifest_named");
    write_project(&dir);
    let out = tmt()
        .args(["build", "notape"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join("notape.tmx").is_file());
    assert!(!dir.join("app.tmx").exists());
    assert!(!dir.join("out/zmono.tmx").exists());
}

/// The PM-1 driver's `manifest_mode_discovery_walks_up_from_a_
/// subdirectory`, ported. Builds `notape` (not `app`) from `src/` so the
/// assertion isn't riding the bootstrap's already-cleaned-up `app.tmx`.
#[test]
fn manifest_mode_discovery_walks_up_from_a_subdirectory() {
    let dir = scratch("tm_manifest_walkup");
    write_project(&dir);
    let out = tmt()
        .args(["build", "notape"])
        .current_dir(dir.join("src"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.join("notape.tmx").is_file(),
        "outputs resolve against the MANIFEST dir, not cwd"
    );
}

/// The PM-1 driver's `manifest_mode_unknown_target_and_missing_manifest_
/// error`, ported.
#[test]
fn manifest_mode_unknown_target_and_missing_manifest_error() {
    let dir = scratch("tm_manifest_unknown");
    write_project(&dir);
    let out = tmt()
        .args(["build", "nosuch"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nosuch"));

    let empty = scratch("tm_manifest_absent");
    let out = tmt().arg("build").current_dir(&empty).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("project"));
}

/// The PM-1 driver's `list_targets_prints_name_and_run_marker`, ported.
/// Unlike the PM-1 test's two-target fixture, `write_project` declares
/// three targets, and `app` (not the second alphabetically) is the one
/// carrying the `run` block, so the expected line order/markers differ
/// from the PM-1 test's literal string: `BTreeMap` order is `app`,
/// `notape`, `zmono`.
#[test]
fn list_targets_prints_name_and_run_marker() {
    let dir = scratch("tm_manifest_list");
    write_project(&dir);
    let out = tmt()
        .args(["build", "--list-targets"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "app\trun\nnotape\nzmono\n"
    );
}

/// The PM-1 driver's `release_flag_selects_the_release_profile`, ported —
/// but rewritten to a byte comparison instead of the ported version's
/// bare `.is_file()` check. `notape`'s own source (`A2_BINARY_PLUS_ONE`)
/// was hand-verified (`cmp -s` on a plain `-O0` vs `-O1` build, no
/// `--release`/`--foutline` involved) to compile byte-IDENTICAL at both
/// levels — a file-existence assertion on either profile can never fail
/// no matter what `manifest.profiles.resolve(flags.release_preset)`
/// does, so it is not reused here. `A5_CALL_ACROSS_ALPHABETS` (defined
/// below, already proven elsewhere in this file to diverge under
/// different `call-mech` choices) was separately hand-verified via the
/// same `cmp -s` check to also diverge between plain `-O0` and `-O1`
/// with NO `--call-mech`/`--foutline` involved — the O1-only optimizer
/// pipeline (inline/jump_threading/tail_merge/dce/etc., all default-ON,
/// `outline` itself excepted) changes the cross-alphabet call site's
/// codegen on its own. Uses its own scratch manifest (a single `app`
/// target, not `write_project`'s `notape`) so the divergence is
/// guaranteed by construction rather than riding a shared fixture that
/// might stop diverging if `notape`'s source ever changed. Failing
/// mutation: `manifest.profiles.resolve(flags.release_preset)` collapsing
/// to `resolve(false)` (i.e. `--release` never reaching profile
/// selection) — both builds would then use the debug profile and produce
/// identical bytes.
#[test]
fn release_flag_selects_the_release_profile() {
    let dir = scratch("tm_manifest_release");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/app.tmc"), A5_CALL_ACROSS_ALPHABETS).unwrap();
    fs::write(
        dir.join("tmt.json"),
        r#"{ "project": { "targets": { "app": { "sources": ["src/app.tmc"] } } } }"#,
    )
    .unwrap();

    let out = tmt()
        .args(["build", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let debug = fs::read(dir.join("app.tmx")).unwrap();

    let out = tmt()
        .args(["build", "--release", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let release = fs::read(dir.join("app.tmx")).unwrap();

    assert_ne!(
        debug, release,
        "--release must select the release profile (-O1) and reach build_one_target's CompileOptions"
    );
}

/// Two structurally-identical 7-state exit-free chains that only
/// `outline` folds into one shared routine at `-O1` — the same shape as
/// `opt_equivalence.rs`'s `OUTLINE_TWIN_CHAINS` (this crate's tests have
/// no shared support module, so it's a local copy, matching this file's
/// own existing convention of local fixture consts).
const OUTLINE_TWIN_CHAINS: &str = "\
alphabet ab { '_', 'a', 'b' }
machine {
  tape t: ab;
  entry state start {
    ['a'] -> goto a0;
    ['b'] -> goto b0;
    [*]   -> stop;
  }
  state a0 { [*] -> move [>] goto a1; }
  state a1 { [*] -> move [>] goto a2; }
  state a2 { [*] -> move [>] goto a3; }
  state a3 { [*] -> move [>] goto a4; }
  state a4 { [*] -> move [>] goto a5; }
  state a5 { [*] -> move [>] goto a6; }
  state a6 { [*] -> move [>] goto mid; }
  state b0 { [*] -> move [>] goto b1; }
  state b1 { [*] -> move [>] goto b2; }
  state b2 { [*] -> move [>] goto b3; }
  state b3 { [*] -> move [>] goto b4; }
  state b4 { [*] -> move [>] goto b5; }
  state b5 { [*] -> move [>] goto b6; }
  state b6 { [*] -> move [>] goto mid; }
  state mid { [*] -> stop; }
}";

/// `outline` (from `--foutline`) and `stamped_asm` are the flag-only
/// compile axes `build_one_target` carries beyond PM's `CompileOptions`
/// (docs/tmt/project.md (profiles)) — the schema has no key for either,
/// so both must come from `flags` on every call. `stamped_asm` has no
/// test here: the driver never exposes `--stamped-asm` at all, and the
/// `.rept` re-detection pass it would skip is self-check-proven
/// (`compress_asm_with_object`) to assemble the identical object either
/// way — so no build-observable divergence exists for a test to catch,
/// with or without the wiring. `outline` DOES reach the object, so it
/// gets a real test: build the SAME target twice into the manifest's one
/// fixed output path (`-o` is rejected in manifest mode, so the two runs
/// can't have distinct output paths) and compare the bytes in between.
/// `-O1` is passed explicitly since the debug profile defaults to `-O0`,
/// where the optimizer — and so `outline` — never runs. Failing mutation:
/// hardcoding `outline` to a fixed `bool` (or dropping the field, which
/// defaults to `false`) in `build_one_target`'s `CompileOptions` literal
/// — both builds would then produce byte-identical images.
#[test]
fn foutline_flag_reaches_manifest_mode_compile_options() {
    let dir = scratch("tm_manifest_foutline");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/app.tmc"), OUTLINE_TWIN_CHAINS).unwrap();
    fs::write(
        dir.join("tmt.json"),
        r#"{ "project": { "targets": { "app": { "sources": ["src/app.tmc"] } } } }"#,
    )
    .unwrap();

    let out = tmt()
        .args(["build", "-O1", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let without = fs::read(dir.join("app.tmx")).unwrap();

    let out = tmt()
        .args(["build", "-O1", "--foutline", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let with = fs::read(dir.join("app.tmx")).unwrap();

    assert_ne!(
        without, with,
        "--foutline must reach build_one_target's CompileOptions and change the linked image"
    );
}
