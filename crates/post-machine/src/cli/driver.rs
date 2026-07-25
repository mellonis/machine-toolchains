//! `pmt build`: the cc-style driver (docs/pmt/cli.md (build)). Two modes
//! by positional shape — file inputs (argv mode, manifest never read) or
//! target names/none (manifest mode, docs/pmt/project.md). Both compose
//! the same internals `compile`/`asm`/`link` expose; objects stay in
//! memory unless --keep-objects.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::object::SymbolDef;
use mtc_core::linker::LinkOptions;

use crate::compiler::{CompileOptions, CompileReport, compile as compile_source};
use crate::optimizer::OptLevel;
use crate::stdlib;

use super::build::{
    find_library, out_path, read_object, render_opt_report, render_warnings, sidecar_path,
    take_disabled_passes,
};
use super::lint::render_fatal;
use super::{Args, CliOutput};

const BUILD_USAGE: &str = "\
USAGE: pmt build [INPUT.pmc|.pma|.pmo ...] [-o OUT.pmx] [FLAGS]   (argv mode)
       pmt build [TARGET ...] [FLAGS]                             (manifest mode)

Argv mode compiles/assembles/loads every input in memory, links with
the stdlib, and writes OUT.pmx (+ .pmx.map). Manifest mode discovers
the nearest pmt.json with a `project` section from the current
directory and builds its targets (all of them when none is named).

COMPILE FLAGS (argv mode; manifest mode: override the profile):
  --debug | --release   presets (manifest mode: profile selection)
  -O0 | -O1             optimization level
  -g                    record debug info
  --strip-debugger      drop `brk` at codegen
  --fno-<pass>          disable one optimizer pass (repeatable)
  -Werror               treat (post-refinement) warnings as errors

LINK FLAGS (argv mode only; the manifest declares these):
  --nostdlib            do not link the built-in std
  -L DIR / -l NAME      library search dir / library (repeatable)
  -o OUT.pmx            output path

COMMON:
  --no-relax            keep every symbol site in far form
  --keep-objects        write each intermediate .pmo next to its source
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
    werror: bool,
    disabled_passes: Vec<String>,
    no_relax: bool,
    nostdlib: bool,
    keep_objects: bool,
    search_dirs: Vec<String>,
    lib_names: Vec<String>,
    out: Option<String>,
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
        werror: args.flag("-Werror"),
        disabled_passes,
        no_relax: args.flag("--no-relax"),
        nostdlib: args.flag("--nostdlib"),
        keep_objects: args.flag("--keep-objects"),
        search_dirs: args.values("-L")?,
        lib_names: args.values("-l")?,
        out: args.value("-o")?,
        run: args.flag("--run"),
        list_targets: args.flag("--list-targets"),
        verbose: args.flag("-v"),
    };
    let positionals = args.positionals()?;

    let is_file = |s: &str| s.ends_with(".pmc") || s.ends_with(".pma") || s.ends_with(".pmo");
    let (files, targets): (Vec<String>, Vec<String>) =
        positionals.into_iter().partition(|p| is_file(p));
    if !files.is_empty() && !targets.is_empty() {
        return Err(format!(
            "pmt build takes file inputs or target names, not both\n\n{BUILD_USAGE}"
        ));
    }
    if files.is_empty() {
        manifest_mode(&targets, &flags)
    } else {
        argv_mode(&files, &flags)
    }
}

fn manifest_mode(requested: &[String], flags: &Flags) -> Result<CliOutput, String> {
    if flags.out.is_some()
        || !flags.search_dirs.is_empty()
        || !flags.lib_names.is_empty()
        || flags.nostdlib
    {
        return Err(format!(
            "-o/-L/-l/--nostdlib contradict the manifest — it declares outputs and libraries\n\n{BUILD_USAGE}"
        ));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let Some((manifest_path, manifest)) =
        crate::project::discover_manifest(&cwd).map_err(|e| e.to_string())?
    else {
        return Err(
            "no pmt.json with a `project` section found from the current directory upward".into(),
        );
    };
    let root = manifest_path
        .parent()
        .expect("pmt.json has a parent")
        .to_path_buf();

    if flags.list_targets {
        let mut stdout = String::new();
        for (name, target) in &manifest.targets {
            stdout.push_str(name);
            if target.run.is_some() {
                stdout.push_str("\trun");
            }
            stdout.push('\n');
        }
        return Ok(CliOutput::ok(stdout, String::new()));
    }

    for name in requested {
        if !manifest.targets.contains_key(name) {
            return Err(format!(
                "no target `{name}` in {} (targets: {})",
                manifest_path.display(),
                manifest
                    .targets
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let selected: Vec<&str> = if requested.is_empty() {
        manifest.targets.keys().map(String::as_str).collect() // BTreeMap: alphabetical
    } else {
        requested.iter().map(String::as_str).collect()
    };

    if flags.run && selected.len() != 1 {
        return Err(format!(
            "--run needs exactly one target (have {}): name it\n\n{BUILD_USAGE}",
            selected.len()
        ));
    }

    let mut stderr = String::new();
    let mut built: Vec<(String, PathBuf)> = Vec::new();
    for name in &selected {
        let target = &manifest.targets[*name];
        let (output, chunk) = build_one_target(&root, &manifest, name, target, flags)?;
        stderr.push_str(&chunk);
        built.push((name.to_string(), output));
    }

    if flags.run {
        let (name, output) = &built[0];
        let target = &manifest.targets[name.as_str()];
        return run_target(&root, output, target.run.as_ref(), stderr);
    }
    Ok(CliOutput::ok(String::new(), stderr))
}

/// Builds one target: compile/assemble/load its effective sources with
/// the resolved profile (+ flag overrides), refine warnings against the
/// declared set, link with the declared libraries + entry, write the
/// output (+ sidecar) relative to the manifest dir. Returns the
/// absolute output path and the stderr chunk.
fn build_one_target(
    root: &Path,
    manifest: &crate::project::Manifest,
    name: &str,
    target: &crate::project::Target,
    flags: &Flags,
) -> Result<(PathBuf, String), String> {
    // In manifest mode --debug/--release are PURE profile selectors
    // (docs/pmt/cli.md (build)): only the individual flags (-g, -O*,
    // --strip-debugger, -Werror) override the resolved profile's keys.
    let profile = manifest.profiles.resolve(flags.release_preset);
    let mut options = CompileOptions {
        debug_info: if flags.debug_info {
            true
        } else {
            profile.debug_info
        },
        strip_debugger: if flags.strip_debugger {
            true
        } else {
            profile.strip_debugger
        },
        opt_level: profile.opt_level,
        disabled_passes: flags.disabled_passes.clone(),
        capture_ir: false,
    };
    if flags.o0 {
        options.opt_level = OptLevel::O0;
    }
    if flags.o1 {
        options.opt_level = OptLevel::O1;
    }
    let werror = profile.werror || flags.werror;

    let resolve = |raw: &str| -> Result<PathBuf, String> {
        Ok(root.join(crate::project::normalize_rel(raw)?))
    };

    let read_err_prefix = format!("target `{name}`: ");
    let mut objects: Vec<ObjectFile> = Vec::new();
    let mut reports: Vec<(PathBuf, CompileReport)> = Vec::new();
    for raw in manifest.effective_sources(target) {
        let path = resolve(&raw)?;
        load_one_source(
            &path,
            &options,
            flags.keep_objects,
            &read_err_prefix,
            &mut objects,
            &mut reports,
        )?;
    }

    let libs = manifest.effective_libraries(target);
    let dirs: Vec<String> = libs
        .dirs
        .iter()
        .map(|d| resolve(d).map(|p| p.to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    let mut libraries = Vec::new();
    for lib in &libs.link {
        libraries.push(find_library(lib, &dirs)?);
    }
    if manifest.stdlib {
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
    if werror && warning_count > 0 {
        return Err(format!(
            "{stderr}-Werror: {warning_count} warning(s) treated as errors"
        ));
    }

    // `entry` threads the manifest's per-target key into the linker's
    // BFS root; `call_mech` has no PM-1 analogue, so it stays default.
    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions {
            relax: !flags.no_relax,
            entry: target.entry.clone(),
            ..Default::default()
        },
    )
    .map_err(|e| format!("target `{name}`: {e}"))?;

    let output = resolve(&manifest.output_of(name, target))?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::write(&output, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    let map_path = sidecar_path(&output);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    if flags.verbose {
        let r = &linked.report;
        let _ = writeln!(
            stderr,
            "{name}: link: dropped [{}]; {} site(s) relaxed short, {} far",
            r.dropped.join(", "),
            r.relaxed_calls,
            r.far_calls
        );
    }
    Ok((output, stderr))
}

/// Maps a manifest `run` block onto `run.rs`'s `RunSettings` and runs
/// the just-built executable (docs/pmt/project.md (run block)): an
/// absent block runs `pmt run`'s own defaults (empty tape, head 0, no
/// limits) rather than erroring. The build's stderr chunk is prefixed
/// onto the run's so `--run`'s combined output reads as one build+run
/// invocation.
fn run_target(
    root: &Path,
    output: &Path,
    run: Option<&crate::project::RunSpec>,
    build_stderr: String,
) -> Result<CliOutput, String> {
    use mtc_core::vm::TactProfile;
    let spec = run.cloned().unwrap_or_default();
    let settings = super::run::RunSettings {
        tape_block: spec
            .tape_block
            .map(|raw| -> Result<String, String> {
                Ok(root
                    .join(crate::project::normalize_rel(&raw)?)
                    .to_string_lossy()
                    .into_owned())
            })
            .transpose()?,
        tape_inline: spec.tape,
        head: spec.head.unwrap_or(0),
        save: None,
        strict: spec.strict_cells,
        no_step_limit: false,
        max_steps: spec.max_steps,
        max_tacts: spec.max_tacts,
        // TactProfile has five fields since the TM-1 arc (table_read_cost,
        // frame_load_cost); the manifest declares only the three PM-1 ones,
        // so the rest come from the ELECTRONIC base.
        profile: spec
            .tact_profile
            .map_or(TactProfile::ELECTRONIC, |[m, r, w]| TactProfile {
                move_cost: m,
                read_cost: r,
                write_cost: w,
                ..TactProfile::ELECTRONIC
            }),
        trace: false,
    };
    let mut run_out = super::run::execute_run(output, &settings, &mut std::io::sink())?;
    run_out.stderr = format!("{build_stderr}{}", run_out.stderr);
    Ok(run_out)
}

/// Compile options for argv mode: exactly `pmt compile`'s preset/flag
/// logic (cli/build.rs::compile), minus -S/--emit-ir which stay
/// compile-only inspection artifacts.
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

    // LinkOptions has three fields (relax / entry / call_mech); argv mode
    // takes the defaults for the latter two, exactly as `pmt link` does.
    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions {
            relax: !flags.no_relax,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;

    let target = out_path(Path::new(&files[0]), flags.out.clone(), "pmx");
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
/// (docs/pmt/cli.md (build)): `.pmc` compiles, `.pma` assembles,
/// anything else loads as a `.pmo` object — the one dispatch shared by
/// argv mode and manifest mode's per-target loop, so a fix (like
/// `--keep-objects` covering `.pma`) lands once instead of drifting
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
        Some("pmc") => {
            let source = fs::read_to_string(path)
                .map_err(|e| format!("{read_err_prefix}cannot read {}: {e}", path.display()))?;
            let out = compile_source(&source, options.clone()).map_err(|e| {
                let mut stderr = String::new();
                render_fatal(&mut stderr, path, e.span, &e.kind, e.kind.code());
                stderr.trim_end().to_string()
            })?;
            if keep_objects {
                let pmo = path.with_extension("pmo");
                fs::write(&pmo, out.object.to_bytes())
                    .map_err(|e| format!("cannot write {}: {e}", pmo.display()))?;
            }
            reports.push((path.to_path_buf(), out.report));
            objects.push(out.object);
        }
        Some("pma") => {
            let source = fs::read_to_string(path)
                .map_err(|e| format!("{read_err_prefix}cannot read {}: {e}", path.display()))?;
            let object = crate::asm::assemble(&source, options.debug_info).map_err(|e| {
                let mut stderr = String::new();
                render_fatal(&mut stderr, path, e.span, &e.kind, e.kind.code());
                stderr.trim_end().to_string()
            })?;
            if keep_objects {
                let pmo = path.with_extension("pmo");
                fs::write(&pmo, object.to_bytes())
                    .map_err(|e| format!("cannot write {}: {e}", pmo.display()))?;
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

/// The undeclared-external refinement (docs/pmt/cli.md (build)): a bare
/// call that is undeclared per-file but resolved by the declared set is
/// not a defect of the BUILD — drop its warning. Runs before -Werror
/// counting so -Werror judges the post-filter set. The retain predicate
/// itself lives in `compiler.rs`, next to the warning it refines
/// (docs/pmt/cli.md (undeclared-external)) — this just walks every
/// report in the build.
fn refine_reports(reports: &mut [(PathBuf, CompileReport)], defined: &HashSet<String>) {
    for (_, report) in reports.iter_mut() {
        crate::compiler::refine_undeclared(&mut report.diagnostics, defined);
    }
}
