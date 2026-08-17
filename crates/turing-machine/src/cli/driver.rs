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
    let (manifest_path, manifest) = discover_project(&cwd)?;
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

/// Discovers the nearest ancestor `tmt.json` with a `project` section
/// starting from `start` (docs/tmt/project.md (discovery)) — shared by
/// `manifest_mode` (CLI, always the process's own `current_dir`) and
/// [`build_target_for_launch`] (the DAP seam, an explicit override or
/// that same fallback), so the two callers can never drift on the
/// "no manifest found" wording.
fn discover_project(start: &Path) -> Result<(PathBuf, crate::project::Manifest), String> {
    crate::project::discover_manifest(start)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "no tmt.json with a `project` section found from the current directory upward".into()
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
/// each line into its own `Output` event), and the target's own `run`
/// block's resolved tape PATH (root-relative, docs/tmt/project.md (run
/// block), through the exact rule `tmt build --run` uses — `run_once`,
/// this file).
///
/// Unlike PM's `DapTargetBuild` (`crates/post-machine/src/cli/driver.rs`),
/// the tape is NOT loaded here — only its path is resolved. TM-1's tape
/// validation (band count, then per-tape alphabet cardinality) needs the
/// launched executable's OWN tape header, which exists only once
/// `crate::dap`'s `finish_launch` has read the `.tmx` this function just
/// wrote — so loading happens there, through the exact same `build_tapes`
/// helper program mode already uses (`dap/mod.rs`), never a second tape-
/// loading copy.
#[derive(Debug)]
pub(crate) struct DapTargetBuild {
    pub output: PathBuf,
    pub diagnostics: Vec<String>,
    pub tape_path: String,
}

/// tmt dap target-mode launch (`crate::dap`): builds ONE named manifest
/// target IN PROCESS — the same manifest-mode path `tmt build TARGET`
/// already runs (`build_one_target`, shared, not duplicated), carved out
/// from behind `manifest_mode`'s cwd-only discovery and its declared flag
/// surface, neither of which a launch request has any use for. Mirrors
/// PM's `build_target_for_launch` closely; see [`DapTargetBuild`] for the
/// one shape difference (a resolved tape PATH, not a loaded tape).
///
/// `project_dir` substitutes for the CLI's own `current_dir()` as
/// [`discover_project`]'s starting point — `None` falls back to it,
/// matching every other `discover_manifest` call site in this crate.
/// `force_debug_info` is the caller's decision, not this function's
/// default: the DAP adapter always passes `true` (a debug session
/// without line maps is crippled), but the seam itself stays a faithful
/// `-g`-optional build so a same-crate test can prove the forcing
/// actually overrides the manifest's own profile rather than merely
/// agreeing with it by coincidence.
///
/// Scope deliberately narrower than a full `tmt build TARGET` run: only
/// `-g` is force-overridable here — opt level, `--strip-debugger`,
/// `-Werror`, and `--call-mech` still come from the resolved profile /
/// manifest declaration as declared (`build_one_target`'s own
/// `flags.call_mech.or_else(|| manifest.effective_call_mech(target))`
/// chain applies unchanged — this seam never sets `flags.call_mech`,
/// so the target's own committed lowering wins, exactly as `tmt build
/// TARGET` resolves it), since a launch request has no flag surface for
/// any of them. The target's `run` block's `max-steps`/`max-tacts` are
/// deliberately NOT adopted, mirroring PM: those bound a batch `tmt run`,
/// whereas a debug session runs interactively under its own per-tick
/// budget.
///
/// TM-specific contract — the tape-only run-block rule
/// (docs/tmt/project.md (run block)): unlike PM, a TM-1 launch has no
/// empty-tape default (`tmt run` itself requires `--tape-block`), so a
/// target with no `run` block, or a `run` block that declares no `tape`,
/// cannot be launched. Enforced through [`run_block_tape_path`] — the
/// SAME function [`run_once`] calls for `tmt build --run` — so a target
/// that CLI itself refuses to run fails a dap launch through the exact
/// same guard conditions, not a second copy of them.
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
        .expect("tmt.json has a parent")
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
        outline: false,
        werror: false,
        disabled_passes: Vec::new(),
        no_relax: false,
        nostdlib: false,
        keep_objects: false,
        search_dirs: Vec::new(),
        lib_names: Vec::new(),
        out: None,
        entry: None,
        call_mech: None,
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

    // The tape-only run-block rule (this function's own doc comment):
    // delegates to the shared `run_block_tape_path` helper — see its own
    // doc comment.
    let (tape_path, _spec) =
        run_block_tape_path(&root, target_name, target.run.as_ref(), "a dap launch")?;

    Ok(DapTargetBuild {
        output,
        diagnostics,
        tape_path,
    })
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
    let mut unit_sources: Vec<Option<PathBuf>> = Vec::new();
    for raw in manifest.effective_sources(target) {
        let path = resolve(&raw)?;
        load_one_source(
            &path,
            &options,
            flags.keep_objects,
            &read_err_prefix,
            &mut objects,
            &mut reports,
            &mut unit_sources,
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
    let output = crate::project::normalize_rel(&manifest.output_of(name, target))
        .map(|rel| root.join(rel))
        .map_err(|e| format!("{stderr}{e}"))?;
    let tail = link_and_write(
        manifest,
        name,
        target,
        &objects,
        &libraries,
        flags,
        &output,
        &unit_sources,
    )
    .map_err(|e| format!("{stderr}{e}"))?;
    stderr.push_str(&tail);
    Ok((output, stderr))
}

/// Links a target's compiled units, writes the executable (+ sidecar) to
/// the caller-resolved output path, and renders the `-v` link line — the
/// tail of `build_one_target` past the point its compile-stage warnings
/// are already rendered, factored out so that whole sequence is one
/// fallible unit its caller can prefix with those warnings at a single
/// site (docs/tmt/cli.md (build)). Returns the (possibly empty) `-v`
/// chunk; the caller owns concatenating it onto its own accumulated
/// stderr.
#[allow(clippy::too_many_arguments)]
fn link_and_write(
    manifest: &crate::project::Manifest,
    name: &str,
    target: &crate::project::Target,
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    flags: &Flags,
    output: &Path,
    unit_sources: &[Option<PathBuf>],
) -> Result<String, String> {
    // The sidecar path anchors the provenance strings, so it is resolved
    // BEFORE the link (docs/formats.md (map sidecar)).
    let map_path = sidecar_path(output);
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
            sources: sidecar_sources(&map_path, unit_sources),
        },
    )
    .map_err(|e| format!("target `{name}`: {e}"))?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::write(output, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
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
    Ok(tail)
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
    let (tape_path, spec) = run_block_tape_path(root, name, run, "tmt build --run")?;
    let settings = super::run::RunSettings {
        tape: Some(tape_path),
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

/// The tape-only run-block rule (docs/tmt/project.md (run block)):
/// TM-1 has no empty-tape default, so a target either declares no `run`
/// block, or one that declares no `tape`, cannot proceed — both are hard
/// errors naming the target. Shared by `run_once` (`tmt build --run`)
/// and `build_target_for_launch` (the DAP target-mode seam,
/// `dap/mod.rs`) so the two callers can never drift on the guard
/// CONDITIONS; `caller` supplies only the per-invocation phrase naming
/// who needed the tape (`"tmt build --run"` / `"a dap launch"`), so the
/// two callers' messages read naturally without a second copy of the
/// guard logic itself. On success, resolves the declared `tape` path
/// manifest-relative (root-relative, not the process cwd) and hands back
/// the resolved `&RunSpec` too, so a caller needing its other fields
/// (`run_once`'s own `no_step_limit`/`max_steps`/`max_tacts`) doesn't
/// have to re-derive `Some(spec)` from `run` a second time.
fn run_block_tape_path<'a>(
    root: &Path,
    target_name: &str,
    run: Option<&'a crate::project::RunSpec>,
    caller: &str,
) -> Result<(String, &'a crate::project::RunSpec), String> {
    let Some(spec) = run else {
        return Err(format!(
            "target `{target_name}` declares no `run` block: {caller} needs one with a `tape`"
        ));
    };
    let Some(raw_tape) = spec.tape.clone() else {
        return Err(format!(
            "target `{target_name}`'s run block declares no `tape`: {caller} needs a .tmt snapshot"
        ));
    };
    let tape_path = root
        .join(crate::project::normalize_rel(&raw_tape)?)
        .to_string_lossy()
        .into_owned();
    Ok((tape_path, spec))
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
    let mut unit_sources: Vec<Option<PathBuf>> = Vec::new();
    for file in files {
        let path = Path::new(file);
        load_one_source(
            path,
            &options,
            flags.keep_objects,
            "",
            &mut objects,
            &mut reports,
            &mut unit_sources,
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
    let tail = link_and_write_argv(&objects, &libraries, flags, &files[0], &unit_sources)
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
    unit_sources: &[Option<PathBuf>],
) -> Result<String, String> {
    let target = out_path(Path::new(first_file), flags.out.clone(), "tmx");
    let map_path = sidecar_path(&target);
    // Argv mode threads relax / entry / call_mech explicitly — there is
    // no default to lean on for `call_mech` once the flag exists (TM's
    // own `tmt link` does the same); `sources` carries the units' sidecar
    // provenance (docs/formats.md (map sidecar)).
    let linked = crate::asm::link(
        objects,
        libraries,
        LinkOptions {
            relax: !flags.no_relax,
            entry: flags.entry.clone(),
            call_mech: flags.call_mech.unwrap_or_default(),
            sources: sidecar_sources(&map_path, unit_sources),
        },
    )
    .map_err(|e| e.to_string())?;

    fs::write(&target, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
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

/// The map sidecar's per-unit provenance strings (docs/formats.md (map
/// sidecar)): each compiled unit's source path re-expressed relative to
/// the sidecar's own directory, so a build tree stays relocatable —
/// falling back to the absolute path when the two share no common root.
/// Purely lexical, like the resolution a debug adapter performs on the
/// way back (docs/lsp.md (known caveats)). Entries stay parallel to the
/// unit objects; the libraries after them carry no provenance and are
/// simply not covered.
fn sidecar_sources(map_path: &Path, unit_sources: &[Option<PathBuf>]) -> Vec<Option<String>> {
    use mtc_core::source_path::{lexical_absolute, relative_to};
    let cwd = std::env::current_dir().unwrap_or_default();
    let map_dir = lexical_absolute(&cwd, map_path.parent().unwrap_or(Path::new(".")));
    unit_sources
        .iter()
        .map(|source| {
            source.as_ref().map(|path| {
                let abs = lexical_absolute(&cwd, path);
                relative_to(&map_dir, &abs)
                    .unwrap_or(abs)
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .collect()
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
    unit_sources: &mut Vec<Option<PathBuf>>,
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
            unit_sources.push(Some(path.to_path_buf()));
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
            unit_sources.push(Some(path.to_path_buf()));
            objects.push(object);
        }
        // A prebuilt `.tmo` is not a source a debugger could open — no
        // provenance (docs/formats.md (map sidecar)).
        _ => {
            unit_sources.push(None);
            objects.push(read_object(path)?);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
    use mtc_core::linemap::LineIndex;
    use mtc_core::linker::MapFile;

    // ---- build_target_for_launch (the DAP target-mode seam) ------------

    /// A fresh, per-call scratch directory under the OS temp dir, unique
    /// by process id + an atomic counter — this is an in-crate `#[cfg(test)]`
    /// module, not a `tests/*.rs` integration binary, so `CARGO_TARGET_TMPDIR`
    /// is not set here; mirrors `project.rs`'s own `unique_tmp_dir` for the
    /// same reason (and PM's identical driver.rs test helper).
    fn unique_tmp_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tmt-driver-test-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal single-tape fixture: one mapped instruction line (line 7,
    /// the `write`/`stop` rule) for `LineIndex::address_for_line` to probe.
    const FIXTURE_TMC: &str = "\
alphabet ab { '_', 'a' }

machine {
  tape main: ab;

  entry state scan {
    [*] -> write ['a'] stop;
  }
}
";

    fn write_fixture_tape(dir: &Path, name: &str) {
        let block = TapeBlockFile {
            alphabet: vec!["_".to_string(), "a".to_string()],
            tapes: vec![TapeSnapshot {
                origin: 0,
                cells: vec![0],
                head: 0,
                alphabet: None,
            }],
        };
        fs::write(dir.join(format!("{name}.tmt")), block.to_bytes().unwrap()).unwrap();
    }

    /// The negative control the forcing claim needs (mirrors PM's own
    /// `force_debug_info_overrides_a_manifest_profile_that_disables_it`):
    /// a test that merely asserts "launching with `force_debug_info: true`
    /// produces a line map" would pass even if the parameter were entirely
    /// ignored and `-g` came from elsewhere. Pinning `false` first, against
    /// a manifest whose OWN debug profile disables `-g`, proves the seam is
    /// a faithful pass-through by default — and only then does flipping to
    /// `true` on the exact same fixture prove the flag is what closes the
    /// gap, not a coincidence of the default profile.
    #[test]
    fn force_debug_info_overrides_a_manifest_profile_that_disables_it() {
        let dir = unique_tmp_dir("force-debug-info");
        fs::write(dir.join("app.tmc"), FIXTURE_TMC).unwrap();
        write_fixture_tape(&dir, "app");
        fs::write(
            dir.join("tmt.json"),
            r#"{ "project": {
                "profiles": { "debug": { "debug-info": false } },
                "targets": { "app": {
                    "sources": ["app.tmc"],
                    "run": { "tape": "app.tmt" }
                } }
            } }"#,
        )
        .unwrap();

        let line_is_mapped = |built: &DapTargetBuild| {
            let map_text = fs::read_to_string(sidecar_path(&built.output)).unwrap();
            let map = MapFile::from_json(&map_text).unwrap();
            LineIndex::new(&map).address_for_line(7, None).is_some()
        };

        let built = build_target_for_launch(Some(&dir), "app", false).unwrap();
        assert!(
            !line_is_mapped(&built),
            "force_debug_info: false must leave the manifest's own -g-less profile in force"
        );

        // Same fixture, same manifest — only the seam's own parameter
        // changes.
        let built = build_target_for_launch(Some(&dir), "app", true).unwrap();
        assert!(
            line_is_mapped(&built),
            "force_debug_info: true must inject -g even though the manifest profile disabled it"
        );
    }

    #[test]
    fn build_target_for_launch_reports_an_unknown_target_by_name() {
        let dir = unique_tmp_dir("unknown-target");
        fs::write(dir.join("app.tmc"), FIXTURE_TMC).unwrap();
        fs::write(
            dir.join("tmt.json"),
            r#"{ "project": { "targets": { "app": { "sources": ["app.tmc"] } } } }"#,
        )
        .unwrap();

        let err = build_target_for_launch(Some(&dir), "nosuch", true).unwrap_err();
        assert!(err.contains("nosuch"), "{err}");
        assert!(err.contains("app"), "{err}");
    }

    /// TM-specific: a target with no `run` block cannot launch — there is
    /// no empty-tape default (module doc on `build_target_for_launch`).
    #[test]
    fn build_target_for_launch_rejects_a_target_with_no_run_block() {
        let dir = unique_tmp_dir("no-run-block");
        fs::write(dir.join("app.tmc"), FIXTURE_TMC).unwrap();
        fs::write(
            dir.join("tmt.json"),
            r#"{ "project": { "targets": { "app": { "sources": ["app.tmc"] } } } }"#,
        )
        .unwrap();

        let err = build_target_for_launch(Some(&dir), "app", true).unwrap_err();
        assert!(err.contains("app"), "{err}");
        assert!(err.contains("run"), "{err}");
    }

    /// TM-specific: a target whose `run` block declares no `tape` is the
    /// other half of the same rule.
    #[test]
    fn build_target_for_launch_rejects_a_run_block_with_no_tape() {
        let dir = unique_tmp_dir("no-tape");
        fs::write(dir.join("app.tmc"), FIXTURE_TMC).unwrap();
        fs::write(
            dir.join("tmt.json"),
            r#"{ "project": { "targets": { "app": {
                "sources": ["app.tmc"],
                "run": { "max-steps": 10 }
            } } } }"#,
        )
        .unwrap();

        let err = build_target_for_launch(Some(&dir), "app", true).unwrap_err();
        assert!(err.contains("app"), "{err}");
        assert!(err.contains("tape"), "{err}");
    }
}
