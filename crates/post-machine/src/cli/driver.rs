//! `pmt build`: the cc-style driver (docs/pmt/cli.md (build)). Two modes
//! by positional shape — file inputs (argv mode, manifest never read) or
//! target names/none (manifest mode, docs/pmt/project.md). Both compose
//! the same internals `compile`/`asm`/`link` expose; objects stay in
//! memory unless --keep-objects.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::object::SymbolDef;
use mtc_core::linker::{DEFAULT_ENTRY, LinkOptions};
use mtc_core::vm::InfiniteTape;

use crate::compiler::{CompileOptions, CompileReport, VariantColumns, compile as compile_source};
use crate::optimizer::OptLevel;
use crate::stdlib;

use super::build::{
    find_library, out_path, read_object, render_link_report, render_opt_report, render_warnings,
    sidecar_path, take_disabled_passes,
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
    let (manifest_path, manifest) = discover_project(&cwd)?;
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
            return Err(unknown_target_error(&manifest_path, &manifest, name));
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
        // failing link (docs/pmt/cli.md (build)).
        let (output, chunk) = build_one_target(&root, &manifest, name, target, flags)
            .map_err(|e| format!("{stderr}{e}"))?;
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

/// Discovers the nearest ancestor `pmt.json` with a `project` section
/// starting from `start` (docs/pmt/project.md (discovery)) — shared by
/// `manifest_mode` (CLI, always the process's own `current_dir`) and
/// [`build_target_for_launch`] (the DAP seam, an explicit override or
/// that same fallback), so the two callers can never drift on the
/// "no manifest found" wording.
fn discover_project(start: &Path) -> Result<(PathBuf, crate::project::Manifest), String> {
    crate::project::discover_manifest(start)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "no pmt.json with a `project` section found from the current directory upward".into()
        })
}

/// The "no such target" error text, shared by `manifest_mode`'s
/// per-request validation loop and [`build_target_for_launch`]'s single
/// lookup — one wording, never two.
fn unknown_target_error(
    manifest_path: &Path,
    manifest: &crate::project::Manifest,
    name: &str,
) -> String {
    format!(
        "no target `{name}` in {} (targets: {})",
        manifest_path.display(),
        manifest
            .targets
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// [`build_target_for_launch`]'s answer: everything a DAP `launch`
/// handler needs to start a session against a manifest target, without
/// reaching back into `crate::project` itself — the built executable's
/// absolute path (already written, sidecar `.map` included), its
/// rendered compile warnings ONE STRING PER DIAGNOSTIC (this crate's
/// thin-renderer rule preserved: data, not printing — the caller turns
/// each line into its own `Output` event), and the initial tape +
/// alphabet + strict-cells bit resolved from the target's OWN `run`
/// settings (docs/pmt/project.md (run block)) through the exact rules
/// `pmt build --run` uses (`run_once`, this file) — never a second copy
/// of that resolution.
#[derive(Debug)]
pub(crate) struct DapTargetBuild {
    pub output: PathBuf,
    pub diagnostics: Vec<String>,
    pub tape: InfiniteTape,
    pub alphabet: Vec<String>,
    pub strict_cells: bool,
}

/// pmt dap target-mode launch (`crate::dap`): builds ONE named manifest
/// target IN PROCESS — the same manifest-mode path `pmt build TARGET`
/// already runs (`build_one_target`, shared, not duplicated), carved out
/// from behind `manifest_mode`'s cwd-only discovery and its declared
/// flag surface, neither of which a launch request has any use for.
///
/// `project_dir` substitutes for the CLI's own `current_dir()` as
/// [`discover_project`]'s starting point — `None` falls back to it,
/// matching every other `discover_manifest` call site in this crate; a
/// directory or the `pmt.json` file itself both work (the discovery walk
/// tolerates either — see its own doc comment). `force_debug_info` is
/// the caller's decision, not this function's default: the DAP adapter
/// always passes `true` (a debug session without line maps is crippled),
/// but the seam itself stays a faithful `-g`-optional build so a
/// same-crate test can prove the forcing actually overrides the
/// manifest's own profile rather than merely agreeing with it by
/// coincidence.
///
/// Scope deliberately narrower than a full `pmt build TARGET` run: only
/// `-g` is force-overridable here — opt level, `--strip-debugger`, and
/// `-Werror` still come from the resolved (always non-release) profile
/// as declared, since a launch request has no flag surface for them. The
/// target's `run` block's `max-steps`/`max-tacts`/`tact-profile` are
/// deliberately NOT adopted — those bound a batch `pmt run`, whereas a
/// debug session runs interactively under its own per-tick budget
/// (`dap`'s module doc); only the tape/tape-block/head/strict-cells
/// corner of `run` applies to a launched session.
pub(crate) fn build_target_for_launch(
    project_dir: Option<&Path>,
    target_name: &str,
    force_debug_info: bool,
) -> Result<DapTargetBuild, String> {
    let cwd;
    let start: &Path = match project_dir {
        Some(p) => p,
        None => {
            cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            &cwd
        }
    };
    let (manifest_path, manifest) = discover_project(start)?;
    let root = manifest_path
        .parent()
        .expect("pmt.json has a parent")
        .to_path_buf();
    let target = manifest
        .targets
        .get(target_name)
        .ok_or_else(|| unknown_target_error(&manifest_path, &manifest, target_name))?;

    let flags = Flags {
        debug_preset: false,
        release_preset: false,
        o0: false,
        o1: false,
        debug_info: force_debug_info,
        strip_debugger: false,
        werror: false,
        disabled_passes: Vec::new(),
        no_relax: false,
        nostdlib: false,
        keep_objects: false,
        search_dirs: Vec::new(),
        lib_names: Vec::new(),
        out: None,
        run: false,
        list_targets: false,
        verbose: false,
    };
    let (output, diagnostics) = build_one_target(&root, &manifest, target_name, target, &flags)?;
    // One line per diagnostic is `render_warnings`' own contract (every
    // diagnostic is exactly one `writeln!`, `verbose: false` above keeps
    // `render_opt_report`'s multi-line entries out of this string
    // entirely) — splitting on newlines here recovers that structure
    // without a second, Vec-returning rendering path in `build.rs`.
    let diagnostics: Vec<String> = diagnostics
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();

    let spec = target.run.clone().unwrap_or_default();
    let tape_block = spec
        .tape_block
        .map(|raw| -> Result<String, String> {
            Ok(root
                .join(crate::project::normalize_rel(&raw)?)
                .to_string_lossy()
                .into_owned())
        })
        .transpose()?;
    let (tape, alphabet) = super::run::initial_tape(
        tape_block.as_deref(),
        spec.tape.as_deref(),
        spec.head.unwrap_or(0),
    )?;

    Ok(DapTargetBuild {
        output,
        diagnostics,
        tape,
        alphabet,
        strict_cells: spec.strict_cells,
    })
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
        // Settled below, once the inputs have been scanned for the
        // program's volatile bit.
        columns: VariantColumns::Both,
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
    let paths: Vec<PathBuf> = manifest
        .effective_sources(target)
        .iter()
        .map(|raw| resolve(raw))
        .collect::<Result<_, _>>()?;
    let units = load_units(&paths, &options, flags.keep_objects, &read_err_prefix)?;

    // The libraries are resolved BEFORE anything is compiled, because the
    // entry symbol may come from one of them and its object's bit then
    // decides every unit's build column.
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

    // The same entry name this target links with (`LinkOptions::entry`
    // below), so the column decision reads the bit off the object the
    // linker will actually resolve the entry from.
    let entry = target.entry.as_deref().unwrap_or(DEFAULT_ENTRY);
    options.columns = columns_for(
        entry_owner_is_volatile(&units, &libraries, entry),
        flags.keep_objects,
    );
    compile_units(
        units,
        &options,
        flags.keep_objects,
        &mut objects,
        &mut reports,
    )?;

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
    // them on an early `?` return (docs/pmt/cli.md (build)).
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
/// (docs/pmt/cli.md (build)). Returns the absolute output path and the
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
    // `entry` threads the manifest's per-target key into the linker's
    // BFS root; `call_mech` has no PM-1 analogue, so it stays default.
    let linked = crate::asm::link(
        objects,
        libraries,
        LinkOptions {
            relax: !flags.no_relax,
            entry: target.entry.clone(),
            ..Default::default()
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
        render_link_report(&mut tail, &format!("{name}: "), &linked.report);
    }
    Ok((output, tail))
}

/// Maps a manifest `run` block onto `run.rs`'s `RunSettings` and runs
/// the just-built executable (docs/pmt/project.md (run block)): an
/// absent block runs `pmt run`'s own defaults (empty tape, head 0, no
/// limits) rather than erroring. The build's stderr chunk is prefixed
/// onto the run's so `--run`'s combined output reads as one build+run
/// invocation — on success by concatenation, and on failure by prefixing
/// `run_once`'s error, the same fallible-tail-then-prefix shape
/// `link_and_write` uses for the build's own link-then-write tail
/// (docs/pmt/project.md (run block)).
fn run_target(
    root: &Path,
    output: &Path,
    run: Option<&crate::project::RunSpec>,
    build_stderr: String,
) -> Result<CliOutput, String> {
    let mut run_out = run_once(root, output, run).map_err(|e| format!("{build_stderr}{e}"))?;
    run_out.stderr = format!("{build_stderr}{}", run_out.stderr);
    Ok(run_out)
}

/// The fallible tail of `run_target`: resolves the run block's tape path
/// and drives the just-built executable. Factored out so a failure
/// anywhere in it — resolving the path, or the run itself — is one
/// `Result` its caller can prefix with the build's warnings at a single
/// site, rather than trusting every `?` inside to remember.
fn run_once(
    root: &Path,
    output: &Path,
    run: Option<&crate::project::RunSpec>,
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
    super::run::execute_run(output, &settings, &mut std::io::sink())
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
        // Settled by `columns_for` once the inputs have been scanned.
        columns: VariantColumns::Both,
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
    let mut options = argv_compile_options(flags);

    let mut objects: Vec<ObjectFile> = Vec::new();
    let mut reports: Vec<(PathBuf, CompileReport)> = Vec::new();
    let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let units = load_units(&paths, &options, flags.keep_objects, "")?;

    // Resolved before compiling: `-l` can supply the entry symbol, and
    // then its object's bit is the program's.
    let mut libraries = Vec::new();
    for name in &flags.lib_names {
        libraries.push(find_library(name, &flags.search_dirs)?);
    }
    if !flags.nostdlib {
        libraries.push(stdlib::object().clone());
    }

    // Argv mode links with `LinkOptions::default()`, i.e. the default
    // entry symbol.
    options.columns = columns_for(
        entry_owner_is_volatile(&units, &libraries, DEFAULT_ENTRY),
        flags.keep_objects,
    );
    compile_units(
        units,
        &options,
        flags.keep_objects,
        &mut objects,
        &mut reports,
    )?;

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
    // (docs/pmt/cli.md (build)).
    let tail = link_and_write_argv(&objects, &libraries, flags, &files[0])
        .map_err(|e| format!("{stderr}{e}"))?;
    stderr.push_str(&tail);
    Ok(CliOutput::ok(String::new(), stderr))
}

/// The link + write tail of argv-mode `build`, factored out for the same
/// reason as manifest mode's [`link_and_write`]: one fallible unit whose
/// error a single call site can prefix with the already-rendered warnings
/// (docs/pmt/cli.md (build)). Returns the (possibly empty) `-v` chunk; the
/// output path itself is not needed past this point in argv mode.
fn link_and_write_argv(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    flags: &Flags,
    first_file: &str,
) -> Result<String, String> {
    // LinkOptions has three fields (relax / entry / call_mech); argv mode
    // takes the defaults for the latter two, exactly as `pmt link` does.
    let linked = crate::asm::link(
        objects,
        libraries,
        LinkOptions {
            relax: !flags.no_relax,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;

    let target = out_path(Path::new(first_file), flags.out.clone(), "pmx");
    fs::write(&target, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    let map_path = sidecar_path(&target);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    let mut tail = String::new();
    if flags.verbose {
        render_link_report(&mut tail, "", &linked.report);
    }
    Ok(tail)
}

/// One build input, carried as far as the build-column decision allows.
/// `.pma` and `.pmo` inputs are already objects and their build column
/// is settled; a `.pmc` source still has to be compiled, and cannot be
/// until the program's kind is known — so it travels with the facts a
/// parse-only pass could establish about it.
enum Unit {
    Source {
        path: PathBuf,
        text: String,
        scan: SourceScan,
    },
    Object(ObjectFile),
}

/// What one parse-only pass over a `.pmc` source tells the driver: no
/// flatten, no IR, no optimizer, no codegen. A source that does not even
/// parse yields the empty scan and gets its real diagnostic moments later
/// when it is compiled.
#[derive(Default)]
struct SourceScan {
    /// A top-level `volatile main`, i.e. the program bit this unit's
    /// object will carry — the same thing the compiler derives it from.
    volatile: bool,
    /// The symbol names this unit will define for cross-object resolution
    /// (`export`ed top-level functions plus `main`, namespace-qualified as
    /// the compiler mangles them). Locals are omitted: they never enter
    /// the linker's namespace, so they can never own the entry.
    exports: Vec<String>,
}

fn scan_source(text: &str) -> SourceScan {
    let Some(program) = crate::lexer::lex(text)
        .ok()
        .and_then(|tokens| crate::parser::parse(&tokens).ok())
    else {
        return SourceScan::default();
    };
    SourceScan {
        volatile: program.functions.iter().any(|f| f.volatile),
        exports: program
            .functions
            .iter()
            .filter(|f| f.exported)
            .map(|f| crate::compiler::full_name(&f.ns, &f.name))
            .collect(),
    }
}

impl Unit {
    /// The program bit this unit's object carries.
    fn volatile(&self) -> bool {
        match self {
            Unit::Source { scan, .. } => scan.volatile,
            Unit::Object(object) => object.program_volatile,
        }
    }

    /// Whether this unit defines `name` the way the linker's namespace
    /// sees definitions — `SymbolDef::Defined` for an object, an export
    /// for a source.
    fn defines(&self, name: &str) -> bool {
        match self {
            Unit::Source { scan, .. } => scan.exports.iter().any(|e| e == name),
            Unit::Object(object) => defines_symbol(object, name),
        }
    }
}

/// Phase one of the load (docs/pmt/cli.md (build)): every input in argv
/// order, doing the work that does not depend on the build column —
/// `.pma` assembles, anything that is not `.pmc`/`.pma` loads as a
/// `.pmo` object, `.pmc` is only read. `read_err_prefix` lets a caller
/// name context a bare `cannot read` message can't (manifest mode passes
/// `` target `NAME`:  ``; argv mode passes `""`).
///
/// One dispatch shared by argv mode and manifest mode's per-target loop,
/// so a fix (like `--keep-objects` covering `.pma`) lands once instead of
/// drifting between two copies.
fn load_units(
    paths: &[PathBuf],
    options: &CompileOptions,
    keep_objects: bool,
    read_err_prefix: &str,
) -> Result<Vec<Unit>, String> {
    let mut units = Vec::new();
    for path in paths {
        match path.extension().and_then(|e| e.to_str()) {
            Some("pmc") => {
                let text = fs::read_to_string(path)
                    .map_err(|e| format!("{read_err_prefix}cannot read {}: {e}", path.display()))?;
                let scan = scan_source(&text);
                units.push(Unit::Source {
                    path: path.clone(),
                    text,
                    scan,
                });
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
                    write_object(path, &object)?;
                }
                units.push(Unit::Object(object));
            }
            _ => units.push(Unit::Object(read_object(path)?)),
        }
    }
    Ok(units)
}

/// Phase two: compile every `.pmc` unit with the settled column and
/// collect the objects IN INPUT ORDER — the linker's namespace is
/// first-wins, so the order an input was written in is part of the
/// build's meaning and survives the two-phase split.
fn compile_units(
    units: Vec<Unit>,
    options: &CompileOptions,
    keep_objects: bool,
    objects: &mut Vec<ObjectFile>,
    reports: &mut Vec<(PathBuf, CompileReport)>,
) -> Result<(), String> {
    for unit in units {
        match unit {
            Unit::Object(object) => objects.push(object),
            Unit::Source { path, text, .. } => {
                let out = compile_source(&text, options.clone()).map_err(|e| {
                    let mut stderr = String::new();
                    render_fatal(&mut stderr, &path, e.span, &e.kind, e.kind.code());
                    stderr.trim_end().to_string()
                })?;
                if keep_objects {
                    write_object(&path, &out.object)?;
                }
                reports.push((path, out.report));
                objects.push(out.object);
            }
        }
    }
    Ok(())
}

fn write_object(source: &Path, object: &ObjectFile) -> Result<(), String> {
    let pmo = source.with_extension("pmo");
    fs::write(&pmo, object.to_bytes()).map_err(|e| format!("cannot write {}: {e}", pmo.display()))
}

/// The program's volatile bit as the driver can know it BEFORE compiling
/// anything (docs/pmt/cli.md (build)), reproducing the rule the linker
/// applies (docs/core.md (linking)): the bit belongs to the ONE object
/// that defines the entry symbol, not to the inputs collectively. A
/// union over every input would disagree — a non-entry input carrying
/// the bit would flip the columns of a program the linker then resolves
/// as normal, and the two builds would emit different code.
///
/// So: walk the inputs in the order the linker's namespace is built —
/// the units first, then the libraries — take the first that defines
/// `entry`, and read its bit alone. First-in-order matches first-wins,
/// and a library only ever owns the entry when no unit does, exactly as
/// `resolve` fills its namespace with `or_insert`. Two units defining
/// `entry` is a duplicate-symbol link error whichever bit is chosen, so
/// the tie is broken deterministically and not diagnosed here; nothing
/// defining it at all is a `NoEntrySymbol` link error, and the column is
/// moot — normal, which is also the linker's assumption.
///
/// The libraries have to be in it: `pmt build util.pmc -L lib -l entry`
/// is a link whose entry comes from a library, and reading the bit off
/// the units alone there compiles the wrong column for the whole build.
/// The embedded stdlib is the harmless case that motivated the wrong
/// shortcut — every std name is `std::`-qualified, so it can never own an
/// unqualified entry, and its own bit is clear regardless.
fn entry_owner_is_volatile(units: &[Unit], libraries: &[ObjectFile], entry: &str) -> bool {
    if let Some(unit) = units.iter().find(|unit| unit.defines(entry)) {
        return unit.volatile();
    }
    libraries
        .iter()
        .find(|library| defines_symbol(library, entry))
        .is_some_and(|library| library.program_volatile)
}

/// Whether an object defines `name` the way the linker's namespace sees
/// definitions: `Defined` only, never a `Local` (a local is invisible
/// there, so it can never own the entry).
fn defines_symbol(object: &ObjectFile, name: &str) -> bool {
    object
        .symbols
        .iter()
        .any(|s| s.name == name && matches!(s.def, SymbolDef::Defined { .. }))
}

/// The column rule (docs/pmt/cli.md (build)). An in-memory object dies
/// inside a link whose program kind is already known, so only the needed
/// column is built and the other one is provably dead. Anything that
/// lands on disk carries both, because a `.pmo` outlives the invocation
/// that made it: `--keep-objects` therefore switches the whole build back
/// to two columns rather than writing out a half object. Either way the
/// linker selects one column, so the executable is the same bytes.
fn columns_for(volatile: bool, keep_objects: bool) -> VariantColumns {
    match (keep_objects, volatile) {
        (true, _) => VariantColumns::Both,
        (false, true) => VariantColumns::VolatileOnly,
        (false, false) => VariantColumns::NormalOnly,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Unit {
        Unit::Source {
            path: PathBuf::from("unit.pmc"),
            text: text.to_string(),
            scan: scan_source(text),
        }
    }

    fn object(pma: &str) -> Unit {
        Unit::Object(crate::asm::assemble(pma, false).expect("the fixture assembles"))
    }

    /// The two single-column arms are what this driver exists to choose;
    /// nothing downstream can tell them from a both-columns build, because
    /// the linker selects one column either way and the image is identical
    /// — so only a direct test can stop the rule from silently reverting
    /// to compiling twice the work.
    #[test]
    fn the_column_rule_builds_one_column_in_memory_and_both_on_disk() {
        assert_eq!(columns_for(true, false), VariantColumns::VolatileOnly);
        assert_eq!(columns_for(false, false), VariantColumns::NormalOnly);
        assert_eq!(columns_for(true, true), VariantColumns::Both);
        assert_eq!(columns_for(false, true), VariantColumns::Both);
    }

    #[test]
    fn the_entry_owners_bit_decides_not_a_union_over_the_inputs() {
        // The reproducing shape: a non-entry input carries the bit while
        // the entry unit does not. A union would say volatile; the linker
        // resolves the entry from `app.pmc`, so the answer is normal.
        let units = vec![
            source("main() { @util(); }"),
            object(".volatile\n.func util\n        wr      1\n        ret\n"),
        ];
        assert!(!entry_owner_is_volatile(&units, &[], DEFAULT_ENTRY));

        // The inverse: the entry unit declares itself, a plain sibling
        // does not drag it back to normal.
        let units = vec![
            source("volatile main() { @util(); }"),
            object(".func util\n        wr      1\n        ret\n"),
        ];
        assert!(entry_owner_is_volatile(&units, &[], DEFAULT_ENTRY));
    }

    #[test]
    fn the_entry_owner_may_be_an_object_and_is_the_first_definer() {
        let volatile_main = ".volatile\n.func main\n.volatile\n        stp\n";
        let units = vec![source("export util() { mark; }"), object(volatile_main)];
        assert!(entry_owner_is_volatile(&units, &[], DEFAULT_ENTRY));

        // Two definers is a duplicate-symbol link error whichever bit is
        // read; the tie resolves to the first in input order, silently.
        let units = vec![object(".func main\n        stp\n"), object(volatile_main)];
        assert!(!entry_owner_is_volatile(&units, &[], DEFAULT_ENTRY));
    }

    #[test]
    fn a_custom_entry_name_is_looked_up_by_that_name() {
        // A `.pmc` unit's bit is "does it contain a volatile main", which
        // is independent of which of its exports the link enters through:
        // the linker reads the OBJECT's bit once it knows which object
        // owns the entry.
        let units = vec![
            source("export other() { mark; }"),
            source("volatile main() { mark; }\nexport start() { mark; }"),
        ];
        assert!(entry_owner_is_volatile(&units, &[], "start"));
        assert!(entry_owner_is_volatile(&units, &[], DEFAULT_ENTRY));
        assert!(!entry_owner_is_volatile(&units, &[], "other"));
        // A name no unit defines: the link will fail, the column is moot.
        assert!(!entry_owner_is_volatile(&units, &[], "nowhere"));
    }

    #[test]
    fn a_local_definition_never_owns_the_entry() {
        // `main` is always exported, a bare sibling never is — and a local
        // is invisible to the linker's namespace, so it cannot be reached
        // by name at all.
        let scan = scan_source("volatile main() { @helper(); }\nhelper() { mark; }");
        assert_eq!(scan.exports, vec!["main".to_string()]);
        assert!(scan.volatile);

        // A namespaced `main` is `ns::main`, not the entry.
        let scan = scan_source("namespace ns { export main() { mark; } }");
        assert_eq!(scan.exports, vec!["ns::main".to_string()]);
        assert!(!scan.volatile);
    }

    /// `pmt build util.pmc -L lib -l entry`: the entry symbol comes from a
    /// LIBRARY, so its object carries the program bit and decides every
    /// unit's column. Scanning the units alone reads no bit at all and
    /// compiles the whole build against the wrong column.
    #[test]
    fn a_library_may_own_the_entry_and_supply_the_bit() {
        let units = vec![source("export util() { mark; }")];
        let volatile_entry =
            crate::asm::assemble(".volatile\n.func main\n.volatile\n        stp\n", false)
                .expect("the fixture assembles");
        let plain_entry =
            crate::asm::assemble(".func main\n        stp\n", false).expect("assembles");

        assert!(entry_owner_is_volatile(
            &units,
            std::slice::from_ref(&volatile_entry),
            DEFAULT_ENTRY
        ));
        assert!(!entry_owner_is_volatile(
            &units,
            std::slice::from_ref(&plain_entry),
            DEFAULT_ENTRY
        ));

        // A unit that owns the entry outranks any library: the linker's
        // namespace takes user objects first, and a library copy is
        // silently shadowed.
        let owning_units = vec![source("main() { @util(); }")];
        assert!(!entry_owner_is_volatile(
            &owning_units,
            std::slice::from_ref(&volatile_entry),
            DEFAULT_ENTRY
        ));

        // Between libraries it is first-wins in link order, which is why
        // the stdlib riding last can never displace a `-l` library.
        let both = vec![volatile_entry.clone(), plain_entry.clone()];
        assert!(entry_owner_is_volatile(&[], &both, DEFAULT_ENTRY));
        let reversed = vec![plain_entry, volatile_entry];
        assert!(!entry_owner_is_volatile(&[], &reversed, DEFAULT_ENTRY));
    }

    /// The embedded stdlib qualifies every export under `std::`, so it can
    /// never own an unqualified entry however it is ordered.
    #[test]
    fn the_stdlib_cannot_own_the_default_entry() {
        let std_object = crate::stdlib::object().clone();
        assert!(!defines_symbol(&std_object, DEFAULT_ENTRY));
        assert!(!std_object.program_volatile);
    }

    #[test]
    fn an_unparsable_source_scans_empty_instead_of_guessing() {
        let scan = scan_source("volatile main() { this is not a program");
        assert!(!scan.volatile);
        assert!(scan.exports.is_empty());
    }

    // ---- build_target_for_launch (the DAP target-mode seam) ------------

    /// A fresh, per-call scratch directory under the OS temp dir, unique
    /// by process id + an atomic counter — this is an in-crate `#[cfg(test)]`
    /// module, not a `tests/*.rs` integration binary, so `CARGO_TARGET_TMPDIR`
    /// is not set here; mirrors `project.rs`'s own `unique_tmp_dir` for the
    /// same reason.
    fn unique_tmp_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pmt-driver-test-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The negative control the forcing claim needs: without it, a test
    /// that merely asserts "launching with `force_debug_info: true`
    /// produces a line map" would pass even if the parameter were
    /// entirely ignored and `-g` came from elsewhere (the resolved
    /// profile happens to default to `debug-info: true` for the
    /// non-release profile `build_target_for_launch` always resolves).
    /// Pinning `false` first, against a manifest whose OWN debug profile
    /// disables `-g`, proves the seam is a faithful pass-through by
    /// default — and only then does flipping to `true` on the exact same
    /// fixture prove the flag is what closes the gap, not a coincidence
    /// of the default profile.
    #[test]
    fn force_debug_info_overrides_a_manifest_profile_that_disables_it() {
        let dir = unique_tmp_dir("force-debug-info");
        fs::write(dir.join("app.pmc"), "main() {\n    mark;\n    halt;\n}\n").unwrap();
        fs::write(
            dir.join("pmt.json"),
            r#"{ "project": {
                "profiles": { "debug": { "debug-info": false } },
                "targets": { "app": { "sources": ["app.pmc"] } }
            } }"#,
        )
        .unwrap();

        let line_is_mapped = |built: &DapTargetBuild| {
            let map_text = fs::read_to_string(sidecar_path(&built.output)).unwrap();
            let map = mtc_core::linker::MapFile::from_json(&map_text).unwrap();
            mtc_core::linemap::LineIndex::new(&map)
                .address_for_line(2)
                .is_some()
        };

        let built = build_target_for_launch(Some(&dir), "app", false).unwrap();
        assert!(
            !line_is_mapped(&built),
            "force_debug_info: false must leave the manifest's own -g-less profile in force"
        );

        // Same fixture, same manifest, same profile — only the seam's own
        // parameter changes.
        let built = build_target_for_launch(Some(&dir), "app", true).unwrap();
        assert!(
            line_is_mapped(&built),
            "force_debug_info: true must inject -g even though the manifest profile disabled it"
        );
    }

    #[test]
    fn build_target_for_launch_reports_an_unknown_target_by_name() {
        let dir = unique_tmp_dir("unknown-target");
        fs::write(dir.join("app.pmc"), "main() { halt; }").unwrap();
        fs::write(
            dir.join("pmt.json"),
            r#"{ "project": { "targets": { "app": { "sources": ["app.pmc"] } } } }"#,
        )
        .unwrap();

        let err = build_target_for_launch(Some(&dir), "nosuch", true).unwrap_err();
        assert!(err.contains("nosuch"), "{err}");
        assert!(err.contains("app"), "{err}");
    }
}
