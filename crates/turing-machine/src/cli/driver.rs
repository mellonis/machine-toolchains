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
    if args.help() {
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

/// Discovers the nearest `tmt.json` with a `project` section from the
/// current directory upward, selects targets, and builds them
/// (docs/tmt/project.md (discovery)). Flags that contradict the declared
/// model (`-o`/`-L`/`-l`/`--nostdlib`/`--entry` — the manifest already
/// declares outputs, libraries and entries) are rejected up front.
/// `--call-mech` is deliberately NOT in that list: the manifest records
/// the committed lowering, and the flag exists to experiment against it
/// (docs/tmt/project.md (call-mech)) — resolved flag-first, then target
/// key, then project key, then the linker's own default.
fn manifest_mode(requested: &[String], flags: &Flags) -> Result<CliOutput, String> {
    if flags.out.is_some()
        || !flags.search_dirs.is_empty()
        || !flags.lib_names.is_empty()
        || flags.nostdlib
        || flags.entry.is_some()
    {
        return Err(format!(
            "-o/-L/-l/--nostdlib/--entry contradict the manifest — it declares outputs, libraries and entries\n\n{BUILD_USAGE}"
        ));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let Some((manifest_path, manifest)) =
        crate::project::discover_manifest(&cwd).map_err(|e| e.to_string())?
    else {
        return Err(
            "no tmt.json with a `project` section found from the current directory upward".into(),
        );
    };
    let root = manifest_path
        .parent()
        .expect("tmt.json has a parent")
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
        // A prior target's already-rendered warnings live in `stderr`
        // before this call — prefixed onto a failure here for the same
        // reason `build_one_target` prefixes its OWN warnings onto a
        // failing link (docs/tmt/cli.md (build)).
        let (output, chunk) = build_one_target(&root, &manifest, name, target, flags)
            .map_err(|e| format!("{stderr}{e}"))?;
        stderr.push_str(&chunk);
        built.push((name.to_string(), output));
    }

    if flags.run {
        let (name, output) = &built[0];
        let target = &manifest.targets[name.as_str()];
        return run_target(&root, output, name, target.run.as_ref(), stderr);
    }
    Ok(CliOutput::ok(String::new(), stderr))
}

/// Builds one target: compile/assemble/load its effective sources with
/// the resolved profile (+ flag overrides), refine warnings against the
/// declared set, link with the declared libraries + entry + resolved
/// call-mech, write the output (+ sidecar) relative to the manifest
/// directory. Returns the absolute output path and the stderr chunk.
fn build_one_target(
    root: &Path,
    manifest: &crate::project::Manifest,
    name: &str,
    target: &crate::project::Target,
    flags: &Flags,
) -> Result<(PathBuf, String), String> {
    // In manifest mode --debug/--release are PURE profile selectors
    // (docs/tmt/cli.md (build)): only the individual flags (-g, -O*,
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
        // Flag-only axes: never manifest keys (docs/tmt/project.md
        // (profiles)) — the schema has no field for either. `outline`
        // still reads `--foutline` exactly as argv mode does.
        // `stamped_asm` stays unconditionally false: the driver never
        // exposes `--stamped-asm`, and the `.rept` re-detection pass it
        // would skip is self-check-proven to assemble the identical
        // object either way, so there is nothing a manifest key could
        // control even if one existed.
        outline: flags.outline,
        stamped_asm: false,
        inline_cap: None,
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

    // Link and write are one fallible unit below this point precisely so a
    // failure anywhere in it — the link itself, or one of the writes after
    // — carries the warnings already rendered above rather than dropping
    // them on an early `?` return (docs/tmt/cli.md (build)).
    let (output, tail) = link_and_write(root, manifest, name, target, &objects, &libraries, flags)
        .map_err(|e| format!("{stderr}{e}"))?;
    stderr.push_str(&tail);
    Ok((output, stderr))
}

/// Links a target's compiled units, writes the executable (+ sidecar) to
/// its resolved output path, and renders the `-v` link line — the tail of
/// `build_one_target` past the point its compile-stage warnings are
/// already rendered, factored out so that whole sequence is one fallible
/// unit its caller can prefix with those warnings at a single site
/// (docs/tmt/cli.md (build)). Returns the absolute output path and the
/// (possibly empty) `-v` chunk; the caller owns concatenating it onto its
/// own accumulated stderr.
fn link_and_write(
    root: &Path,
    manifest: &crate::project::Manifest,
    name: &str,
    target: &crate::project::Target,
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    flags: &Flags,
) -> Result<(PathBuf, String), String> {
    let linked = crate::asm::link(
        objects,
        libraries,
        LinkOptions {
            relax: !flags.no_relax,
            entry: target.entry.clone(),
            // Flags win over the declared lowering, exactly as the
            // profile flags win over profile keys.
            call_mech: flags
                .call_mech
                .or_else(|| manifest.effective_call_mech(target))
                .unwrap_or_default(),
        },
    )
    .map_err(|e| format!("target `{name}`: {e}"))?;

    let output = root.join(crate::project::normalize_rel(
        &manifest.output_of(name, target),
    )?);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::write(&output, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    let map_path = sidecar_path(&output);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    let mut tail = String::new();
    if flags.verbose {
        let r = &linked.report;
        let _ = writeln!(
            tail,
            "{name}: link: dropped [{}]; {} site(s) relaxed short, {} far",
            r.dropped.join(", "),
            r.relaxed_calls,
            r.far_calls
        );
    }
    Ok((output, tail))
}

/// Runs a just-built target under `--run` (docs/tmt/cli.md (run)): the
/// manifest `run` block split against a `RunSettings`-shaped driver, the
/// way `build_one_target` splits `CompileOptions`/`LinkOptions`. Unlike
/// PM's `run_target`, `tmt run` always drives a whole multi-tape band
/// loaded from a `.tmt` snapshot — there is no empty-tape default to
/// fall back on — so a target without a declared `run` block, or one
/// whose block declares no `tape`, cannot be `--run`; both cases name
/// the target in a pointed error instead of inventing a run. The build's
/// stderr chunk is prefixed onto the run's — on success by concatenation,
/// and on failure by prefixing `run_once`'s error, the same
/// fallible-tail-then-prefix shape `link_and_write` uses for the build's
/// own link-then-write tail (docs/tmt/cli.md (run)).
fn run_target(
    root: &Path,
    output: &Path,
    name: &str,
    run: Option<&crate::project::RunSpec>,
    build_stderr: String,
) -> Result<CliOutput, String> {
    let mut run_out =
        run_once(root, output, name, run).map_err(|e| format!("{build_stderr}{e}"))?;
    run_out.stderr = format!("{build_stderr}{}", run_out.stderr);
    Ok(run_out)
}

/// The fallible tail of `run_target`: the two pointed pre-attempt guards
/// (no `run` block; a `run` block with no `tape`), resolving the tape
/// path, and driving the just-built executable. Factored out so a
/// failure anywhere in it is one `Result` its caller can prefix with the
/// build's warnings at a single site, rather than trusting every early
/// return inside to remember.
fn run_once(
    root: &Path,
    output: &Path,
    name: &str,
    run: Option<&crate::project::RunSpec>,
) -> Result<CliOutput, String> {
    let Some(spec) = run else {
        return Err(format!(
            "target `{name}` declares no `run` block: --run needs one with a `tape`"
        ));
    };
    let Some(raw_tape) = spec.tape.clone() else {
        return Err(format!(
            "target `{name}`'s run block declares no `tape`: tmt run needs a .tmt snapshot"
        ));
    };
    let settings = super::run::RunSettings {
        tape: Some(
            root.join(crate::project::normalize_rel(&raw_tape)?)
                .to_string_lossy()
                .into_owned(),
        ),
        // The manifest `run` block declares no save target, matching PM's
        // driver: `--save-tape-block` is a `tmt run` flag, not a target key.
        save: None,
        no_step_limit: spec.no_step_limit,
        max_steps: spec.max_steps,
        max_tacts: spec.max_tacts,
        trace: false,
    };
    super::run::execute_run(output, &settings, &mut std::io::sink())
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
        inline_cap: None,
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

    // Link and write are one fallible unit below this point precisely so a
    // failure anywhere in it carries the warnings already rendered above
    // rather than dropping them on an early `?` return
    // (docs/tmt/cli.md (build)).
    let tail = link_and_write_argv(&objects, &libraries, flags, &files[0])
        .map_err(|e| format!("{stderr}{e}"))?;
    stderr.push_str(&tail);
    Ok(CliOutput::ok(String::new(), stderr))
}

/// The link + write tail of argv-mode `build`, factored out for the same
/// reason as manifest mode's [`link_and_write`]: one fallible unit whose
/// error a single call site can prefix with the already-rendered warnings
/// (docs/tmt/cli.md (build)). Returns the (possibly empty) `-v` chunk; the
/// output path itself is not needed past this point in argv mode.
fn link_and_write_argv(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    flags: &Flags,
    first_file: &str,
) -> Result<String, String> {
    // `LinkOptions` has three fields (relax / entry / call_mech); argv mode
    // threads all three explicitly — there is no default to lean on for
    // `call_mech` once the flag exists (TM's own `tmt link` does the same).
    let linked = crate::asm::link(
        objects,
        libraries,
        LinkOptions {
            relax: !flags.no_relax,
            entry: flags.entry.clone(),
            call_mech: flags.call_mech.unwrap_or_default(),
        },
    )
    .map_err(|e| e.to_string())?;

    let target = out_path(Path::new(first_file), flags.out.clone(), "tmx");
    fs::write(&target, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    let map_path = sidecar_path(&target);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    let mut tail = String::new();
    if flags.verbose {
        let r = &linked.report;
        let _ = writeln!(
            tail,
            "link: dropped [{}]; {} site(s) relaxed short, {} far",
            r.dropped.join(", "),
            r.relaxed_calls,
            r.far_calls
        );
    }
    Ok(tail)
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

/// The undeclared-external refinement (docs/tmt/cli.md (build)): a bare
/// call that is undeclared per-file but resolved by the declared set is
/// not a defect of the BUILD — drop its warning. Runs before -Werror
/// counting so -Werror judges the post-filter set. The retain predicate
/// itself lives in `compiler.rs`, next to the warning it refines
/// (docs/tmt/cli.md (undeclared-external)) — this just walks every
/// report in the build.
fn refine_reports(reports: &mut [(PathBuf, CompileReport)], defined: &HashSet<String>) {
    for (_, report) in reports.iter_mut() {
        crate::compiler::refine_undeclared(&mut report.diagnostics, defined);
    }
}
