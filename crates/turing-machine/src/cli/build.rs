//! Build-side subcommands: compile, asm, link. Mirrors the PM-1 `pmt` shapes
//! with `.tmc`/`.tma`/`.tmo`/`.tmx` extensions. `link` auto-links the
//! embedded standard library (`std::binaryNumbers` / `std::binaryNumbersBare`)
//! lazily via reachability, with `--nostdlib` to opt out — the PM-1 `link`
//! wiring. `compile` has its own, narrower `--nostdlib`/`--extern`
//! (docs/tmt/cli.md (compile)): the declarations base the footprint/contract
//! check believes ([`read_externals`]), and the same base a tape's
//! alphabet reference resolves against when it names one through `use` or
//! a qualified path and nothing local defines it
//! (docs/tmt/language.md (declarations)). Resolving a `call`/`graft`/
//! `bind` TARGET itself stays a link-time question independent of this
//! table: the callee's footprint is believed through it, but the target
//! need not be defined anywhere this compile can see to compile clean.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::{CallMech, LinkOptions, LinkReport};

use crate::compiler::{
    CompileOptions, CompileReport, Declarations, Origin, Resolved, compile as compile_source,
};
use crate::optimizer::OptLevel;

use super::{Args, CliOutput};

pub(super) const COMPILE_USAGE: &str = "\
USAGE: tmt compile INPUT.tmc [-o OUT.tmo] [FLAGS]

FLAGS:
  -g                 record debug info (labels + .tmc lines)
  -O0 | -O1          optimization level (default -O0)
  --strip-debugger   drop `brk` at codegen
  --debug            preset: -g -O0
  --release          preset: -O1 --strip-debugger
  -S                 emit the generated .tma instead of an object
  --stamped-asm      emit raw stamped .tma (skip .rept re-detection)
  --emit-ir[=STAGE]  write the world-graph IR JSON next to the output
                     (STAGE: lowered | final | after:<pass> for a registered
                      pass; default final)
  --fno-<pass>       disable one optimizer pass (repeatable)
  --foutline         enable the default-off `outline` optimizer pass
  --extern FILE      read FILE's declarations (.tmh strict, .tmc lenient;
                     repeatable, in command-line order)
  --nostdlib         do not read the embedded standard library's declarations
  -Werror            treat warnings as errors
  -v                 render the compile report (passes, rounds)
";

/// `--extern` files in command-line order, then the embedded standard
/// library last unless `nostdlib` (docs/tmt/cli.md (compile)) — the exact
/// push order [`crate::footprint::find_external`]'s first-match lookup
/// relies on, so a user's own `--extern std.tmh` shadows the built-in
/// `std` when both are given. Each file's own declarations-only read is
/// [`crate::header::read_extern`] (STRICT on a `.tmh`, LENIENT on
/// anything else); a file that fails to read or parse is reported by ITS
/// OWN path, never the primary compile's input.
fn read_externals(paths: &[String], nostdlib: bool) -> Result<Declarations, String> {
    let mut externals = Declarations::none();
    for raw in paths {
        let path = Path::new(raw);
        let source =
            fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let resolved = crate::header::read_extern(path, &source).map_err(|e| {
            format!(
                "{}:{}:{}: error: {} [{}]",
                path.display(),
                e.span.start.line,
                e.span.start.col,
                e.kind,
                e.kind.code()
            )
        })?;
        externals.push(Origin::Extern(path.to_path_buf()), resolved);
    }
    if !nostdlib {
        externals.push_stdlib();
    }
    Ok(externals)
}

pub(super) fn render_warnings(stderr: &mut String, input: &Path, report: &CompileReport) {
    for d in &report.diagnostics {
        let _ = writeln!(
            stderr,
            "{}:{}:{}: warning: {}",
            input.display(),
            d.span.start.line,
            d.span.start.col,
            d.message
        );
    }
}

/// Every non-allowed link warning, in the compile-warning format
/// (docs/tmt/cli.md (link warnings)). A link diagnostic has no path and
/// no column, so its location is the function and blob offset, or the
/// source line when the objects carried debug data. Returns how many
/// were printed, which is what `-Werror` counts.
pub(super) fn render_link_diagnostics(
    stderr: &mut String,
    report: &LinkReport,
    allow: &[String],
) -> usize {
    let mut n = 0;
    for d in &report.diagnostics {
        if allow.iter().any(|a| a == d.code) {
            continue;
        }
        n += 1;
        match d.line {
            Some(line) => {
                let _ = writeln!(
                    stderr,
                    "{}:{line}: warning: {} [{}]",
                    d.function, d.message, d.code
                );
            }
            None => {
                let _ = writeln!(
                    stderr,
                    "{}+0x{:04x}: warning: {} [{}]",
                    d.function, d.offset, d.message, d.code
                );
            }
        }
    }
    n
}

/// The link report's structural lines, `-v` only. Mirrors PM's renderer
/// of the same name so the two CLIs do not drift
/// (docs/core.md (the link report)).
pub(super) fn render_link_report(stderr: &mut String, prefix: &str, report: &LinkReport) {
    let _ = writeln!(
        stderr,
        "{prefix}link: dropped [{}]; {} site(s) relaxed short, {} far",
        report.dropped.join(", "),
        report.relaxed_calls,
        report.far_calls
    );
    if report.composites > 0 || report.instantiations > 0 {
        let _ = writeln!(
            stderr,
            "{prefix}frames: {} composite(s), {} stamp(s), {} B compose table; \
             {} deduped, {} trap row(s), {} expanded row(s)",
            report.composites,
            report.instantiations,
            report.compose_table_bytes,
            report.dedup_savings,
            report.synthesized_trap_rows,
            report.expanded_rows
        );
    }
    for fold in &report.folds {
        let _ = writeln!(
            stderr,
            "{prefix}fold: `{}` {} site(s), body {} B, descriptors {} B — {}",
            fold.routine,
            fold.sites,
            fold.body_bytes,
            fold.descriptor_bytes,
            if fold.shared { "shared" } else { "spliced" }
        );
    }
}

/// The one spelling of the strict-mode refusal, shared by `tmt link` and
/// both of `tmt build`'s modes (docs/tmt/cli.md (link warnings)).
pub(super) fn werror_message(stderr: &str, warned: usize) -> String {
    format!("{stderr}-Werror: {warned} link warning(s) treated as errors")
}

pub(super) fn render_opt_report(stderr: &mut String, report: &CompileReport) {
    let _ = writeln!(stderr, "opt: {} round(s)", report.opt.rounds);
    for change in &report.opt.changes {
        let _ = writeln!(
            stderr,
            "  {} {}: {} change(s)",
            change.pass, change.world, change.changes
        );
    }
}

pub(super) fn compile(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(COMPILE_USAGE.into(), String::new()));
    }
    let debug_preset = args.flag("--debug");
    let release_preset = args.flag("--release");
    let mut options = CompileOptions {
        debug_info: debug_preset || args.flag("-g"),
        strip_debugger: release_preset || args.flag("--strip-debugger"),
        stamped_asm: args.flag("--stamped-asm"),
        opt_level: if release_preset {
            OptLevel::O1
        } else {
            OptLevel::O0
        },
        ..Default::default()
    };
    if args.flag("-O0") {
        options.opt_level = OptLevel::O0;
    }
    if args.flag("-O1") {
        options.opt_level = OptLevel::O1;
    }
    // `--foutline` enables the default-off `outline` pass; it takes effect
    // only at `-O1` (the optimizer runs nowhere else).
    options.outline = args.flag("--foutline");
    let emit_asm = args.flag("-S");
    let werror = args.flag("-Werror");
    let verbose = args.flag("-v");
    let emit_ir = take_emit_ir(&mut args)?;
    take_disabled_passes(&mut args, &mut options.disabled_passes);
    options.capture_ir = matches!(emit_ir, Some(Some(_)));
    // `--extern`/`--nostdlib` pick the declarations base the footprint/
    // contract check believes (docs/tmt/cli.md (compile)) — read BEFORE
    // the primary input so a broken `--extern` file's error surfaces
    // first, naming ITS OWN path (`read_externals`), never the input's.
    let extern_paths = args.values("--extern")?;
    let nostdlib = args.flag("--nostdlib");
    options.externals = read_externals(&extern_paths, nostdlib)?;
    let explicit_out = args.value("-o")?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "compile takes exactly one input\n\n{COMPILE_USAGE}"
        ));
    };
    let input = Path::new(input);

    let source =
        fs::read_to_string(input).map_err(|e| format!("cannot read {}: {e}", input.display()))?;
    let out = compile_source(&source, options).map_err(|e| {
        format!(
            "{}:{}:{}: error: {} [{}]",
            input.display(),
            e.span.start.line,
            e.span.start.col,
            e.kind,
            e.kind.code()
        )
    })?;

    let mut stderr = String::new();
    render_warnings(&mut stderr, input, &out.report);
    if verbose {
        render_opt_report(&mut stderr, &out.report);
    }
    if werror && !out.report.diagnostics.is_empty() {
        return Err(format!(
            "{stderr}-Werror: {} warning(s) treated as errors",
            out.report.diagnostics.len()
        ));
    }

    let target = out_path(input, explicit_out, if emit_asm { "tma" } else { "tmo" });
    if emit_asm {
        fs::write(&target, &out.tma)
            .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    } else {
        fs::write(&target, out.object.to_bytes())
            .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    }

    if let Some(stage) = emit_ir {
        let ir_path = target.with_extension("ir.json");
        let json = match stage.as_deref() {
            None | Some("final") => out.ir.to_json(),
            Some(label) => out
                .ir_snapshots
                .iter()
                .rev() // repeated stages resolve last-wins
                .find(|(l, _)| l == label)
                .map(|(_, program)| program.to_json())
                .ok_or_else(|| format!("no IR snapshot labeled `{label}` was captured"))?,
        };
        fs::write(&ir_path, json)
            .map_err(|e| format!("cannot write {}: {e}", ir_path.display()))?;
    }

    Ok(CliOutput::ok(String::new(), stderr))
}

/// `--emit-ir` → `Some(None)`; `--emit-ir=STAGE` → `Some(Some(stage))`.
/// The stage is validated HERE against the optimizer's pass registry rather
/// than at write time (pmt's approach): the resolvable stages are the pipeline
/// bookends `lowered` / `final` plus `after:<pass>` for any registered pass, so
/// an unknown stage fails early with an error naming what exists, not late with
/// a "snapshot not captured".
fn take_emit_ir(args: &mut Args) -> Result<Option<Option<String>>, String> {
    if args.flag("--emit-ir") {
        return Ok(Some(None));
    }
    for slot in &mut args.tokens {
        if let Some(tok) = slot.as_deref()
            && let Some(stage) = tok.strip_prefix("--emit-ir=")
        {
            let stage = stage.to_string();
            *slot = None;
            if !stage_is_known(&stage) {
                return Err(format!("unknown IR stage `{stage}` ({})", known_stages()));
            }
            return Ok(Some(Some(stage)));
        }
    }
    Ok(None)
}

/// Whether `stage` names an IR snapshot the pipeline can produce: the
/// bookends `lowered` / `final`, plus `after:<pass>` for a registered pass.
fn stage_is_known(stage: &str) -> bool {
    if stage == "lowered" || stage == "final" {
        return true;
    }
    stage
        .strip_prefix("after:")
        .is_some_and(|pass| crate::optimizer::pass_names().contains(&pass))
}

/// The `--emit-ir=STAGE` stages that resolve today, for the error message.
fn known_stages() -> String {
    let mut stages = vec!["lowered".to_string(), "final".to_string()];
    for p in crate::optimizer::pass_names() {
        stages.push(format!("after:{p}"));
    }
    stages.join(" | ")
}

pub(super) fn take_disabled_passes(args: &mut Args, disabled: &mut Vec<String>) {
    for slot in &mut args.tokens {
        if let Some(tok) = slot.as_deref()
            && let Some(pass) = tok.strip_prefix("--fno-")
        {
            disabled.push(pass.to_string());
            *slot = None;
        }
    }
}

pub(super) fn out_path(input: &Path, explicit: Option<String>, extension: &str) -> PathBuf {
    match explicit {
        Some(path) => PathBuf::from(path),
        None => input.with_extension(extension),
    }
}

pub(super) const ASM_USAGE: &str = "\
USAGE: tmt asm INPUT.tma [-o OUT.tmo] [-g]
";

pub(super) fn asm(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(ASM_USAGE.into(), String::new()));
    }
    let with_debug = args.flag("-g");
    let explicit_out = args.value("-o")?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("asm takes exactly one input\n\n{ASM_USAGE}"));
    };
    let input = Path::new(input);
    let source =
        fs::read_to_string(input).map_err(|e| format!("cannot read {}: {e}", input.display()))?;
    let object = crate::asm::assemble(&source, with_debug).map_err(|e| {
        format!(
            "{}:{}:{}: error: {} [{}]",
            input.display(),
            e.span.start.line,
            e.span.start.col,
            e.kind,
            e.kind.code()
        )
    })?;
    let target = out_path(input, explicit_out, "tmo");
    fs::write(&target, object.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

pub(super) const LINK_USAGE: &str = "\
USAGE: tmt link INPUT.tmo... [-o OUT.tmx] [FLAGS]

FLAGS:
  --no-relax        keep every call site in far form
  --entry NAME      link NAME as the program entry (default: main)
  --call-mech MECH  bound-call lowering: mono | frames | hybrid (default: hybrid)
  --nostdlib        do not auto-link the embedded standard library
  --allow CODE      suppress a link warning code (repeatable)
  -Werror           treat link warnings as errors
  -L DIR            add a library search directory (repeatable, in order)
  -l NAME           link NAME.tmo from the search path (repeatable)
  -v                render the link report (dropped functions, relaxation)

Writes OUT.tmx and the OUT.tmx.map sidecar (function ranges + table
section info; label/line info when the objects carry -g debug data).
";

/// Parse `--call-mech` (case-sensitive lowercase); absent selects the
/// default `Hybrid`.
pub(super) fn parse_call_mech(raw: Option<String>) -> Result<CallMech, String> {
    match raw.as_deref() {
        None => Ok(CallMech::Hybrid),
        Some("mono") => Ok(CallMech::Mono),
        Some("frames") => Ok(CallMech::Frames),
        Some("hybrid") => Ok(CallMech::Hybrid),
        Some(other) => Err(format!(
            "unknown --call-mech `{other}` (expected one of: mono, frames, hybrid)"
        )),
    }
}

pub(super) fn link(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(LINK_USAGE.into(), String::new()));
    }
    let relax = !args.flag("--no-relax");
    let entry = args.value("--entry")?;
    let call_mech = parse_call_mech(args.value("--call-mech")?)?;
    let nostdlib = args.flag("--nostdlib");
    let allow = args.values("--allow")?;
    crate::lint::validate_allow(&allow).map_err(|e| e.to_string())?;
    let werror = args.flag("-Werror");
    let verbose = args.flag("-v");
    let search_dirs = args.values("-L")?;
    let lib_names = args.values("-l")?;
    let explicit_out = args.value("-o")?;
    let inputs = args.positionals()?;
    if inputs.is_empty() {
        return Err(format!("link needs at least one object\n\n{LINK_USAGE}"));
    }

    let mut objects = Vec::new();
    for path in &inputs {
        objects.push(read_object(Path::new(path))?);
    }
    let mut libraries = Vec::new();
    for name in &lib_names {
        libraries.push(find_library(name, &search_dirs)?);
    }
    // The embedded stdlib links last (first-wins means -l objects and the
    // command-line inputs shadow it), lazily via reachability, unless opted
    // out. Mirrors the pmt `link` wiring.
    if !nostdlib {
        libraries.push(crate::stdlib::object().clone());
    }

    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions {
            relax,
            entry,
            call_mech,
            // `link` consumes prebuilt objects, which carry no source to
            // name — sidecar provenance is a build-driver concern
            // (docs/formats.md (map sidecar)).
            sources: Vec::new(),
        },
    )
    .map_err(|e| e.to_string())?;

    let mut stderr = String::new();
    // A link warning prints always, in the compile-warning format — the
    // report's structural lines stay behind `-v`
    // (docs/tmt/cli.md (link warnings)).
    let warned = render_link_diagnostics(&mut stderr, &linked.report, &allow);
    if verbose {
        render_link_report(&mut stderr, "", &linked.report);
    }
    if werror && warned > 0 {
        return Err(werror_message(&stderr, warned));
    }

    let target = out_path(Path::new(&inputs[0]), explicit_out, "tmx");
    fs::write(&target, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    let map_path = sidecar_path(&target);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    Ok(CliOutput::ok(String::new(), stderr))
}

/// `app.tmx` → `app.tmx.map` (the sidecar keeps the full executable name).
pub(super) fn sidecar_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".map");
    PathBuf::from(s)
}

pub(super) fn read_object(path: &Path) -> Result<ObjectFile, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    ObjectFile::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) fn find_library(name: &str, dirs: &[String]) -> Result<ObjectFile, String> {
    for dir in dirs {
        let candidate = Path::new(dir).join(format!("{name}.tmo"));
        if candidate.exists() {
            return read_object(&candidate);
        }
    }
    Err(format!("library `{name}` not found on the -L search path"))
}

/// `tmt build`'s own library resolution (docs/tmt/project.md
/// (libraries)): whatever of `<name>.tmo` (its interface section) and
/// `<name>.tmh` (graphs, maps, doc lines) exists in the first search
/// directory that has either. When the header exists it is the
/// declaration source — it carries graphs, maps and doc lines the object
/// cannot — and the object, if it also exists, is what gets LINKED (the
/// returned `Option<ObjectFile>`). A header-only library (no `.tmo` next
/// to it) contributes declarations and no object at all — the caller
/// never hands it to the linker. Neither file present, in any searched
/// directory, is an error naming the library, matching [`find_library`]'s
/// own wording.
///
/// Deliberately a SEPARATE function from [`find_library`]: `tmt link`'s
/// own `-l` and the LSP overlay's own fixture (`lsp/overlay.rs`) both need
/// exactly an object or a "not found" error — widening `find_library`'s
/// return shape would force those two callers to unwrap a declarations
/// field they have no use for. Both functions share the same directory
/// search and the same `<name>.tmo` candidate path.
pub(crate) fn find_library_for_build(
    name: &str,
    dirs: &[String],
) -> Result<(Option<ObjectFile>, Resolved), String> {
    for dir in dirs {
        let tmo = Path::new(dir).join(format!("{name}.tmo"));
        let tmh = Path::new(dir).join(format!("{name}.tmh"));
        let has_tmo = tmo.exists();
        let has_tmh = tmh.exists();
        if !has_tmo && !has_tmh {
            continue;
        }
        let object = if has_tmo {
            Some(read_object(&tmo)?)
        } else {
            None
        };
        let declarations = if has_tmh {
            let source = fs::read_to_string(&tmh)
                .map_err(|e| format!("cannot read {}: {e}", tmh.display()))?;
            crate::header::read_extern(&tmh, &source).map_err(|e| {
                format!(
                    "{}:{}:{}: error: {} [{}]",
                    tmh.display(),
                    e.span.start.line,
                    e.span.start.col,
                    e.kind,
                    e.kind.code()
                )
            })?
        } else {
            // `has_tmo` alone, checked above: derive declarations from the
            // object's own interface — the ONE object→declarations path
            // (`crate::header::declarations_from_object`).
            let obj = object.as_ref().expect("has_tmo checked above");
            crate::header::declarations_from_object(obj)
                .map_err(|e| format!("{}: {e}", tmo.display()))?
        };
        return Ok((object, declarations));
    }
    Err(format!("library `{name}` not found on the -L search path"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtc_core::linker::LinkDiagnostic;

    // ---- find_library_for_build -------------------------------------------
    //
    // The resolver's own decision point for `tmt build`'s library handling
    // (docs/tmt/project.md (libraries)): white-box tests here pin exactly
    // what `find_library_for_build` returns for each of the four
    // presence combinations, since a `LinkReport` carries no per-object
    // list an end-to-end test could assert on directly — this IS the
    // check whose `Option<ObjectFile>` decides whether a library's object
    // ever reaches `build_one_target`'s own `libraries` list.

    /// A fresh, per-call scratch directory under the OS temp dir, unique
    /// by process id + an atomic counter — mirrors `cli/driver.rs`'s own
    /// `unique_tmp_dir` test helper of the same shape (this crate has no
    /// shared test-support module).
    fn unique_tmp_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tmt-find-library-test-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const FIND_LIBRARY_FIXTURE: &str = "\
namespace lib {
  export alphabet ab { '_', '0', '1' }
  export routine flip(tape t: ab writes { '0', '1' }) {
    entry state s { [*] -> return; }
  }
}
";

    fn write_fixture_object(dir: &Path) {
        let out = compile_source(
            FIND_LIBRARY_FIXTURE,
            CompileOptions {
                externals: Declarations::none(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("compile: {e}"));
        fs::write(dir.join("lib.tmo"), out.object.to_bytes()).unwrap();
    }

    fn write_fixture_header(dir: &Path) {
        let text = crate::header::from_source(FIND_LIBRARY_FIXTURE)
            .unwrap_or_else(|e| panic!("interface: {e}"));
        fs::write(dir.join("lib.tmh"), text).unwrap();
    }

    fn declares_flip(resolved: &Resolved) -> bool {
        resolved.worlds.iter().any(|w| w.name == "lib::flip")
    }

    /// Mutation: preferring the object's own (reduced) declarations even
    /// when a header also exists — this pins that the HEADER wins as the
    /// declaration source when both are present, per the requirement
    /// ("when both files exist the header is the declaration source"),
    /// while the object is STILL what gets returned to link.
    #[test]
    fn find_library_for_build_returns_both_when_both_files_exist() {
        let dir = unique_tmp_dir("both");
        write_fixture_object(&dir);
        write_fixture_header(&dir);
        let dirs = vec![dir.to_string_lossy().into_owned()];

        let (object, declarations) = find_library_for_build("lib", &dirs).unwrap();
        assert!(
            object.is_some(),
            "the object must still be returned to link"
        );
        assert!(declares_flip(&declarations));
    }

    /// Mutation: `find_library_for_build` requiring a `.tmh` unconditionally
    /// (today's `find_library` behavior, inverted) — an object-only
    /// library would then error instead of deriving declarations from its
    /// interface section.
    #[test]
    fn find_library_for_build_object_only_derives_declarations_from_the_object() {
        let dir = unique_tmp_dir("object-only");
        write_fixture_object(&dir);
        let dirs = vec![dir.to_string_lossy().into_owned()];

        let (object, declarations) = find_library_for_build("lib", &dirs).unwrap();
        assert!(object.is_some());
        assert!(declares_flip(&declarations));
    }

    /// The structural half of "a header-only library is never linked":
    /// with no `.tmo` on the search path at all, the resolver returns
    /// `None` for the object — the value `build_one_target`'s `if let
    /// Some(obj) = object { libraries.push(obj); }` guard reads to decide
    /// whether the library ever reaches the linker's own input list.
    ///
    /// Mutation: fabricating a placeholder `ObjectFile` instead of
    /// returning `None` when no `.tmo` exists — a header-only library
    /// would then be silently handed to the linker.
    #[test]
    fn find_library_for_build_header_only_returns_no_object() {
        let dir = unique_tmp_dir("header-only");
        write_fixture_header(&dir);
        let dirs = vec![dir.to_string_lossy().into_owned()];

        let (object, declarations) = find_library_for_build("lib", &dirs).unwrap();
        assert!(
            object.is_none(),
            "no .tmo exists for this library — nothing to hand the linker"
        );
        assert!(declares_flip(&declarations));
    }

    /// Mutation: falling back to an empty, silent `Declarations` instead
    /// of erroring when neither file exists — the library would then
    /// look declared-but-empty rather than visibly missing, and the
    /// message would no longer name it.
    #[test]
    fn find_library_for_build_errors_when_neither_exists() {
        let dir = unique_tmp_dir("neither");
        let dirs = vec![dir.to_string_lossy().into_owned()];

        let err = find_library_for_build("lib", &dirs).unwrap_err();
        assert!(err.contains("lib"), "{err}");
    }

    /// A `LinkReport` carrying only `diagnostics`, every other counter at
    /// its zero/empty value — the renderer under test reads only
    /// `diagnostics`, so the rest need not vary.
    fn blank_report(diagnostics: Vec<LinkDiagnostic>) -> LinkReport {
        LinkReport {
            dropped: Vec::new(),
            relaxed_calls: 0,
            far_calls: 0,
            instantiations: 0,
            composites: 0,
            compose_table_bytes: 0,
            dedup_savings: 0,
            synthesized_trap_rows: 0,
            expanded_rows: 0,
            variant_fallbacks: Vec::new(),
            folds: Vec::new(),
            diagnostics,
            program_volatile: false,
        }
    }

    /// Mutation it catches: swap the `line` branch for the offset branch
    /// (or drop it) and a line-numbered diagnostic stops naming its
    /// source line.
    #[test]
    fn render_link_diagnostics_with_a_line_renders_the_source_line() {
        let report = blank_report(vec![LinkDiagnostic {
            code: "narrow-alphabet",
            message: "`sub` reads a narrower alphabet".to_string(),
            function: "main".to_string(),
            offset: 4,
            line: Some(7),
        }]);
        let mut out = String::new();
        let n = render_link_diagnostics(&mut out, &report, &[]);
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "main:7: warning: `sub` reads a narrower alphabet [narrow-alphabet]\n"
        );
    }

    /// Mutation it catches: drop the offset branch (or print it in
    /// decimal) and a debug-less diagnostic stops naming its `+0xNNNN`
    /// blob offset.
    #[test]
    fn render_link_diagnostics_without_a_line_renders_the_offset() {
        let report = blank_report(vec![LinkDiagnostic {
            code: "glyph-mismatch",
            message: "`sub` spells different glyphs".to_string(),
            function: "main".to_string(),
            offset: 0x2a,
            line: None,
        }]);
        let mut out = String::new();
        let n = render_link_diagnostics(&mut out, &report, &[]);
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "main+0x002a: warning: `sub` spells different glyphs [glyph-mismatch]\n"
        );
    }

    /// Mutation it catches: ignore the allow list and an allowed code
    /// still prints, or still counts toward what `-Werror` promotes.
    #[test]
    fn render_link_diagnostics_skips_an_allowed_code_and_excludes_it_from_the_count() {
        let report = blank_report(vec![
            LinkDiagnostic {
                code: "narrow-alphabet",
                message: "a".to_string(),
                function: "f".to_string(),
                offset: 0,
                line: None,
            },
            LinkDiagnostic {
                code: "glyph-mismatch",
                message: "b".to_string(),
                function: "f".to_string(),
                offset: 1,
                line: None,
            },
        ]);
        let mut out = String::new();
        let n = render_link_diagnostics(&mut out, &report, &["narrow-alphabet".to_string()]);
        assert_eq!(n, 1, "one of two diagnostics is allowed");
        assert!(!out.contains("narrow-alphabet"));
        assert!(out.contains("glyph-mismatch"));
    }

    /// Mutation it catches: change the wording, drop the count, or lose
    /// the caller's already-rendered `stderr` prefix, and a promoted
    /// strict-mode refusal drifts between `tmt link` and `tmt build`.
    #[test]
    fn werror_message_appends_the_trailer_to_the_given_stderr() {
        let msg = werror_message("main+0x0000: warning: x [y]\n", 2);
        assert_eq!(
            msg,
            "main+0x0000: warning: x [y]\n-Werror: 2 link warning(s) treated as errors"
        );
    }
}
