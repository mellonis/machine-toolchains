//! `tmt build`: the cc-style driver (docs/tmt/cli.md (build)). Two modes
//! by positional shape — file inputs (argv mode, manifest never read) or
//! target names/none (manifest mode, docs/tmt/project.md). Both compose
//! the same internals `compile`/`asm`/`link` expose; objects stay in
//! memory unless --keep-objects.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::object::SymbolDef;
use mtc_core::linker::{CallMech, LinkOptions};

use crate::compiler::{CompileOptions, CompileReport, compile as compile_source};
use crate::optimizer::OptLevel;
use crate::stdlib;

use super::build::{
    find_library, out_path, parse_call_mech, read_object, render_opt_report, render_warnings,
    sidecar_path, take_disabled_passes,
};
use super::lint::render_fatal;
use super::{Args, CliOutput};

const BUILD_USAGE: &str = "\
USAGE: tmt build [INPUT.tmc|.tma|.tmo ...] [-o OUT.tmx] [FLAGS]   (argv mode)
       tmt build [TARGET ...] [FLAGS]                             (manifest mode)

Argv mode compiles/assembles/loads every input in memory, links with
the stdlib, and writes OUT.tmx (+ .tmx.map). Manifest mode discovers
the nearest tmt.json with a `project` section from the current
directory and builds its targets (all of them when none is named).

COMPILE FLAGS (argv mode; manifest mode: override the profile):
  --debug | --release   presets (manifest mode: profile selection)
  -O0 | -O1             optimization level
  -g                    record debug info
  --strip-debugger      drop `brk` at codegen
  --fno-<pass>          disable one optimizer pass (repeatable)
  --foutline            enable the default-off `outline` pass
  -Werror               treat (post-refinement) warnings as errors

LINK FLAGS (argv mode only; the manifest declares these):
  --nostdlib            do not link the built-in std
  -L DIR / -l NAME      library search dir / library (repeatable)
  --entry NAME          link NAME as the program entry (default: main)
  -o OUT.tmx            output path

COMMON:
  --no-relax            keep every symbol site in far form
  --call-mech MECH      bound-call lowering: mono | frames | hybrid
  --keep-objects        write each intermediate .tmo next to its source
  --run [TARGET]        manifest mode: build, then run the target's run block
  --list-targets        manifest mode: print `NAME[\\trun]` per target
  -v                    render the build report
";

struct Flags {
    debug_preset: bool,
    release_preset: bool,
    o0: bool,
    o1: bool,
    debug_info: bool,
    strip_debugger: bool,
    outline: bool,
    werror: bool,
    disabled_passes: Vec<String>,
    no_relax: bool,
    nostdlib: bool,
    keep_objects: bool,
    search_dirs: Vec<String>,
    lib_names: Vec<String>,
    out: Option<String>,
    entry: Option<String>,
    call_mech: Option<CallMech>,
    run: bool,
    list_targets: bool,
    verbose: bool,
}

pub(super) fn build(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(BUILD_USAGE.into(), String::new()));
    }
    let mut disabled_passes = Vec::new();
    take_disabled_passes(&mut args, &mut disabled_passes);
    let flags = Flags {
        debug_preset: args.flag("--debug"),
        release_preset: args.flag("--release"),
        o0: args.flag("-O0"),
        o1: args.flag("-O1"),
        debug_info: args.flag("-g"),
        strip_debugger: args.flag("--strip-debugger"),
        outline: args.flag("--foutline"),
        werror: args.flag("-Werror"),
        disabled_passes,
        no_relax: args.flag("--no-relax"),
        nostdlib: args.flag("--nostdlib"),
        keep_objects: args.flag("--keep-objects"),
        search_dirs: args.values("-L")?,
        lib_names: args.values("-l")?,
        out: args.value("-o")?,
        entry: args.value("--entry")?,
        // `--call-mech` is COMMON, not argv-only: manifest mode accepts it
        // as an override of the declared lowering (docs/tmt/project.md
        // (call-mech)). Parsed with the same function `tmt link` uses.
        call_mech: {
            let raw = args.value("--call-mech")?;
            match raw {
                Some(_) => Some(parse_call_mech(raw)?),
                None => None,
            }
        },
        run: args.flag("--run"),
        list_targets: args.flag("--list-targets"),
        verbose: args.flag("-v"),
    };
    let positionals = args.positionals()?;

    let is_file = |s: &str| s.ends_with(".tmc") || s.ends_with(".tma") || s.ends_with(".tmo");
    let (files, targets): (Vec<String>, Vec<String>) =
        positionals.into_iter().partition(|p| is_file(p));
    if !files.is_empty() && !targets.is_empty() {
        return Err(format!(
            "tmt build takes file inputs or target names, not both\n\n{BUILD_USAGE}"
        ));
    }
    if files.is_empty() {
        manifest_mode(&targets, &flags)
    } else {
        argv_mode(&files, &flags)
    }
}

fn manifest_mode(_targets: &[String], _flags: &Flags) -> Result<CliOutput, String> {
    Err("manifest mode lands in the next task".to_string()) // Task 12 replaces this
}

/// Compile options for argv mode: exactly `tmt compile`'s preset/flag
/// logic (cli/build.rs::compile), minus -S/--emit-ir/--stamped-asm which
/// stay compile-only inspection artifacts.
fn argv_compile_options(flags: &Flags) -> CompileOptions {
    let mut options = CompileOptions {
        debug_info: flags.debug_preset || flags.debug_info,
        strip_debugger: flags.release_preset || flags.strip_debugger,
        opt_level: if flags.release_preset {
            OptLevel::O1
        } else {
            OptLevel::O0
        },
        disabled_passes: flags.disabled_passes.clone(),
        capture_ir: false,
        outline: flags.outline,
        stamped_asm: false, // --stamped-asm is a compile-only emit knob
    };
    if flags.o0 {
        options.opt_level = OptLevel::O0;
    }
    if flags.o1 {
        options.opt_level = OptLevel::O1;
    }
    options
}

fn argv_mode(files: &[String], flags: &Flags) -> Result<CliOutput, String> {
    if flags.run || flags.list_targets {
        return Err(format!(
            "--run and --list-targets are manifest-mode flags\n\n{BUILD_USAGE}"
        ));
    }
    let options = argv_compile_options(flags);

    let mut objects: Vec<ObjectFile> = Vec::new();
    let mut reports: Vec<(PathBuf, CompileReport)> = Vec::new();
    for file in files {
        let path = Path::new(file);
        load_one_source(
            path,
            &options,
            flags.keep_objects,
            "",
            &mut objects,
            &mut reports,
        )?;
    }

    let mut libraries = Vec::new();
    for name in &flags.lib_names {
        libraries.push(find_library(name, &flags.search_dirs)?);
    }
    if !flags.nostdlib {
        libraries.push(stdlib::object().clone());
    }

    refine_reports(&mut reports, &defined_names(&objects, &libraries));

    let mut stderr = String::new();
    let mut warning_count = 0usize;
    for (path, report) in &reports {
        warning_count += report.diagnostics.len();
        render_warnings(&mut stderr, path, report);
        if flags.verbose {
            render_opt_report(&mut stderr, report);
        }
    }
    if flags.werror && warning_count > 0 {
        return Err(format!(
            "{stderr}-Werror: {warning_count} warning(s) treated as errors"
        ));
    }

    // `LinkOptions` has three fields (relax / entry / call_mech); argv mode
    // threads all three explicitly — there is no default to lean on for
    // `call_mech` once the flag exists (TM's own `tmt link` does the same).
    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions {
            relax: !flags.no_relax,
            entry: flags.entry.clone(),
            call_mech: flags.call_mech.unwrap_or_default(),
        },
    )
    .map_err(|e| e.to_string())?;

    let target = out_path(Path::new(&files[0]), flags.out.clone(), "tmx");
    fs::write(&target, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    let map_path = sidecar_path(&target);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    if flags.verbose {
        let r = &linked.report;
        let _ = writeln!(
            stderr,
            "link: dropped [{}]; {} site(s) relaxed short, {} far",
            r.dropped.join(", "),
            r.relaxed_calls,
            r.far_calls
        );
    }
    Ok(CliOutput::ok(String::new(), stderr))
}

/// Loads one already-resolved source path per its extension
/// (docs/tmt/cli.md (build)): `.tmc` compiles, `.tma` assembles,
/// anything else loads as a `.tmo` object — the one dispatch shared by
/// argv mode and manifest mode's per-target loop, so a fix (like
/// `--keep-objects` covering `.tma`) lands once instead of drifting
/// between two copies. `read_err_prefix` lets a caller name context a
/// bare `cannot read` message can't (manifest mode passes
/// `` target `NAME`:  ``; argv mode passes `""`); everything past the
/// read — compile/assemble, `render_fatal` on failure, the
/// `--keep-objects` write, and appending to `objects`/`reports` — is
/// identical between callers.
fn load_one_source(
    path: &Path,
    options: &CompileOptions,
    keep_objects: bool,
    read_err_prefix: &str,
    objects: &mut Vec<ObjectFile>,
    reports: &mut Vec<(PathBuf, CompileReport)>,
) -> Result<(), String> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("tmc") => {
            let source = fs::read_to_string(path)
                .map_err(|e| format!("{read_err_prefix}cannot read {}: {e}", path.display()))?;
            let out = compile_source(&source, options.clone()).map_err(|e| {
                let mut stderr = String::new();
                render_fatal(&mut stderr, path, e.span, &e.kind, e.kind.code());
                stderr.trim_end().to_string()
            })?;
            if keep_objects {
                let tmo = path.with_extension("tmo");
                fs::write(&tmo, out.object.to_bytes())
                    .map_err(|e| format!("cannot write {}: {e}", tmo.display()))?;
            }
            reports.push((path.to_path_buf(), out.report));
            objects.push(out.object);
        }
        Some("tma") => {
            let source = fs::read_to_string(path)
                .map_err(|e| format!("{read_err_prefix}cannot read {}: {e}", path.display()))?;
            let object = crate::asm::assemble(&source, options.debug_info).map_err(|e| {
                let mut stderr = String::new();
                render_fatal(&mut stderr, path, e.span, &e.kind, e.kind.code());
                stderr.trim_end().to_string()
            })?;
            if keep_objects {
                let tmo = path.with_extension("tmo");
                fs::write(&tmo, object.to_bytes())
                    .map_err(|e| format!("cannot write {}: {e}", tmo.display()))?;
            }
            objects.push(object);
        }
        _ => objects.push(read_object(path)?),
    }
    Ok(())
}

/// Every symbol name the declared set defines FOR CROSS-OBJECT
/// resolution — `SymbolDef::Defined` only, exactly the set
/// `linker::resolve` builds its namespace from (`Local` is invisible
/// there too).
fn defined_names(objects: &[ObjectFile], libraries: &[ObjectFile]) -> HashSet<String> {
    objects
        .iter()
        .chain(libraries.iter())
        .flat_map(|o| &o.symbols)
        .filter(|s| matches!(s.def, SymbolDef::Defined { .. }))
        .map(|s| s.name.clone())
        .collect()
}

/// The name inside the first backtick pair of an `undeclared-external`
/// message — the compiler's fixed format ("reference to undeclared
/// external `NAME` — declare it with `use NAME;`"), pinned by
/// `refinement_name_extraction_matches_the_compiler_format` below.
fn undeclared_name(message: &str) -> Option<&str> {
    let start = message.find('`')? + 1;
    let rest = &message[start..];
    Some(&rest[..rest.find('`')?])
}

/// The undeclared-external refinement (docs/tmt/cli.md (build)): a bare
/// call that is undeclared per-file but resolved by the declared set is
/// not a defect of the BUILD — drop its warning. Runs before -Werror
/// counting so -Werror judges the post-filter set.
fn refine_reports(reports: &mut [(PathBuf, CompileReport)], defined: &HashSet<String>) {
    for (_, report) in reports.iter_mut() {
        report.diagnostics.retain(|d| {
            !(d.code == "undeclared-external"
                && undeclared_name(&d.message).is_some_and(|n| defined.contains(n)))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the extraction against the compiler's REAL warning format —
    /// if the message ever changes shape, this fails here rather than
    /// silently breaking the refinement.
    #[test]
    fn refinement_name_extraction_matches_the_compiler_format() {
        let src = "alphabet ab { '_', 'a' }\nmachine { tape t: ab; entry state s { [*] -> call go() then s; } }";
        let out = compile_source(src, CompileOptions::default()).unwrap();
        let diag = out
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == "undeclared-external")
            .expect("bare call go() warns");
        assert_eq!(undeclared_name(&diag.message), Some("go"));
    }
}
