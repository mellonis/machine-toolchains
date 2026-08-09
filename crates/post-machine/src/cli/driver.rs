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
use mtc_core::linker::LinkOptions;

use crate::compiler::{CompileOptions, CompileReport, VariantColumns, compile as compile_source};
use crate::optimizer::OptLevel;
use crate::stdlib;

use super::build::{
    find_library, out_path, read_object, render_link_report, render_opt_report, render_warnings,
    sidecar_path, take_disabled_passes,
};
use super::lint::render_fatal;
use super::{Args, CliOutput};

/// The entry symbol a link resolves from when nothing overrides it —
/// `LinkOptions::entry`'s `None` case (docs/core.md (linking)). The
/// driver needs it by name to find the unit that owns the entry before
/// anything is linked.
const DEFAULT_ENTRY: &str = "main";

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
        // Settled below, once the inputs have been scanned for the
        // program's volatile bit.
        columns: VariantColumns::Both,
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
    // The same entry name this target links with (`LinkOptions::entry`
    // below), so the column decision reads the bit off the object the
    // linker will actually resolve the entry from.
    let entry = target.entry.as_deref().unwrap_or(DEFAULT_ENTRY);
    options.columns = columns_for(entry_owner_is_volatile(&units, entry), flags.keep_objects);
    compile_units(
        units,
        &options,
        flags.keep_objects,
        &mut objects,
        &mut reports,
    )?;

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
        render_link_report(&mut stderr, &format!("{name}: "), &linked.report);
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
        // Settled by `columns_for` once the inputs have been scanned.
        columns: VariantColumns::Both,
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
    // Argv mode links with `LinkOptions::default()`, i.e. the default
    // entry symbol.
    options.columns = columns_for(
        entry_owner_is_volatile(&units, DEFAULT_ENTRY),
        flags.keep_objects,
    );
    compile_units(
        units,
        &options,
        flags.keep_objects,
        &mut objects,
        &mut reports,
    )?;

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
        render_link_report(&mut stderr, "", &linked.report);
    }
    Ok(CliOutput::ok(String::new(), stderr))
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
            Unit::Object(object) => object.symbols.iter().any(|s| {
                s.name == name
                    && matches!(s.def, mtc_core::formats::object::SymbolDef::Defined { .. })
            }),
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
/// So: walk the units in input order, take the first that defines
/// `entry`, and read its bit alone. First-in-order matches the namespace
/// the linker builds, which is first-wins over the user objects. Two
/// units defining `entry` is a duplicate-symbol link error whichever bit
/// is chosen, so the tie is broken deterministically and not diagnosed
/// here; no unit defining it is a `NoEntrySymbol` link error, and the
/// column is moot — normal, which is also the linker's assumption.
///
/// Boundary, stated rather than glossed: only the build's own units are
/// scanned, never its libraries. The embedded stdlib qualifies every name
/// under `std::`, so it cannot own an unqualified entry; a `-l` library
/// that defines the entry AND sets the bit would be read as normal here.
fn entry_owner_is_volatile(units: &[Unit], entry: &str) -> bool {
    units
        .iter()
        .find(|unit| unit.defines(entry))
        .is_some_and(Unit::volatile)
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
        assert!(!entry_owner_is_volatile(&units, DEFAULT_ENTRY));

        // The inverse: the entry unit declares itself, a plain sibling
        // does not drag it back to normal.
        let units = vec![
            source("volatile main() { @util(); }"),
            object(".func util\n        wr      1\n        ret\n"),
        ];
        assert!(entry_owner_is_volatile(&units, DEFAULT_ENTRY));
    }

    #[test]
    fn the_entry_owner_may_be_an_object_and_is_the_first_definer() {
        let volatile_main = ".volatile\n.func main\n.volatile\n        stp\n";
        let units = vec![source("export util() { mark; }"), object(volatile_main)];
        assert!(entry_owner_is_volatile(&units, DEFAULT_ENTRY));

        // Two definers is a duplicate-symbol link error whichever bit is
        // read; the tie resolves to the first in input order, silently.
        let units = vec![object(".func main\n        stp\n"), object(volatile_main)];
        assert!(!entry_owner_is_volatile(&units, DEFAULT_ENTRY));
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
        assert!(entry_owner_is_volatile(&units, "start"));
        assert!(entry_owner_is_volatile(&units, DEFAULT_ENTRY));
        assert!(!entry_owner_is_volatile(&units, "other"));
        // A name no unit defines: the link will fail, the column is moot.
        assert!(!entry_owner_is_volatile(&units, "nowhere"));
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

    #[test]
    fn an_unparsable_source_scans_empty_instead_of_guessing() {
        let scan = scan_source("volatile main() { this is not a program");
        assert!(!scan.volatile);
        assert!(scan.exports.is_empty());
    }
}
