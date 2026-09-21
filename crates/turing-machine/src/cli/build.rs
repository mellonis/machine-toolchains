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
    CompileOptions, CompileReport, Declarations, Origin, compile as compile_source,
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

/// `--extern` files, read through the SAME shared fixpoint `tmt build`
/// uses for its own siblings and libraries
/// (`crate::header::resolve_declarations`, docs/tmt/project.md
/// (Declaration derivation)): each file is read against every OTHER
/// `--extern` file already read clean, then the embedded standard
/// library (unless `nostdlib`), iterated until no more progress is made
/// — so one `--extern` file may itself depend on another, in either
/// command-line order. The FINAL table is still assembled in
/// command-line order, then the embedded standard library last unless
/// `nostdlib` — the exact push order [`crate::footprint::
/// find_external`]'s first-match lookup relies on, so a user's own
/// `--extern std.tmh` shadows the built-in `std` when both are given;
/// the fixpoint's OWN read context now pushes stdlib last too, for the
/// identical reason (`crate::header::resolve_declarations`'s own doc
/// comment). Every file that never reads clean is reported together,
/// through [`render_unresolved`], never just the first in command-line
/// order. Shared with `tmt interface`'s own `--extern`/`--nostdlib`
/// (`cli/interface.rs`), which takes exactly this same meaning —
/// `tmt build` derives its own siblings' and libraries' declarations
/// independently (`cli/driver.rs`) and does NOT call this function,
/// though it does share [`render_unresolved`] with it.
pub(super) fn read_externals(paths: &[String], nostdlib: bool) -> Result<Declarations, String> {
    let mut sources = Vec::with_capacity(paths.len());
    for raw in paths {
        let path = Path::new(raw);
        let text =
            fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        sources.push(crate::header::DeclarationSource {
            origin: Origin::Extern(path.to_path_buf()),
            text: crate::header::DeclarationText::Source {
                path: path.to_path_buf(),
                text,
            },
        });
    }
    let results = crate::header::resolve_declarations(&sources, !nostdlib);
    if results.iter().any(Result::is_err) {
        let failures: Vec<crate::header::UnresolvedSource> =
            results.into_iter().filter_map(Result::err).collect();
        return Err(render_unresolved(&failures));
    }

    let mut externals = Declarations::none();
    for (raw, result) in paths.iter().zip(results) {
        externals.push(
            Origin::Extern(PathBuf::from(raw)),
            result.expect("checked above"),
        );
    }
    if !nostdlib {
        externals.push_stdlib();
    }
    Ok(externals)
}

/// Every diagnostic code that means a source's OWN read failed only
/// because some OTHER source's declarations never resolved, never
/// because of a defect the source itself owns (`docs/tmt/cli.md`'s
/// matching error-code rows carry the identical "declarations were/are
/// not given" wording).
const DECLARATIONS_MISSING_CODES: [&str; 4] = [
    "unresolved-alphabet",
    "undefined-graph",
    "undefined-map",
    "state-args-need-declarations",
];

/// Combines EVERY source [`crate::header::resolve_declarations`] could
/// not read into one error, rather than choosing a single "most likely"
/// cause and leaving the rest unreported: `compile --extern`,
/// `interface --extern`, and `build` all render their fixpoint failures
/// through this one function (docs/tmt/project.md (Declaration
/// derivation)), so an unreadable `--extern` file and an unreadable
/// sibling show identically. A source whose code is NOT one of the four
/// declarations-missing codes prints first — a genuine defect, never a
/// symptom of another source's failure — in the order it was given;
/// the ones that ARE one of those four codes follow, same order.
/// Classification is by CODE only, never by scanning `message` text, so
/// a future reword of the rendered prose can never silently defeat it.
///
/// When every remaining failure's code is one of those four, or is
/// `writes-outside-contract`, a trailing note is appended. `writes-
/// outside-contract` earns the same treatment for one specific reason:
/// it is the shape two units that each need the OTHER's declarations
/// fail with when neither's own read ever sees the other — each treats
/// its absent peer as an unconstrained (opaque) callee, infers a wider
/// write footprint than its OWN declared contract allows, and reports a
/// contract violation that is really a missing-peer symptom wearing a
/// different code. Two independent sources that each carry their own
/// unrelated writes-outside-contract bug also trigger this note; that
/// is accepted imprecision, not a claim that every such pair is a real
/// cycle — the note only ever supplements the individual diagnostics
/// already printed above it, never replaces them.
pub(super) fn render_unresolved(failures: &[crate::header::UnresolvedSource]) -> String {
    let is_declarations_missing = |code: &str| DECLARATIONS_MISSING_CODES.contains(&code);
    let may_be_a_missing_peer_symptom =
        |code: &str| is_declarations_missing(code) || code == "writes-outside-contract";

    let mut ordered: Vec<&crate::header::UnresolvedSource> = failures
        .iter()
        .filter(|f| !is_declarations_missing(f.code))
        .collect();
    ordered.extend(failures.iter().filter(|f| is_declarations_missing(f.code)));

    let mut out = String::new();
    for (i, f) in ordered.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&f.message);
    }

    if failures.len() >= 2
        && failures
            .iter()
            .all(|f| may_be_a_missing_peer_symptom(f.code))
    {
        out.push_str(
            "\nnote: every source named above may be failing only because it needs one of \
             the others' declarations, and none of them ever supplied them to each other — \
             two units that depend on each other's declarations are not supported; break the \
             cycle by hand-writing a header for one of them and supplying that header in its \
             place",
        );
    }

    out
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
/// (Declaration derivation)): whatever of `<name>.tmo` (its interface
/// section) and `<name>.tmh` (graphs, maps, doc lines) exists in the
/// first search directory that has either. Returns the object to hand
/// the linker (`.tmo`, when present — `None` for a header-only library,
/// never handed to the linker) and the declaration TEXT this library's
/// own reading needs: the header, when one exists — it is the
/// declaration source whether or not an object exists beside it, since
/// it carries graphs, maps and doc lines an object cannot, and it is
/// trusted outright over the object rather than checked against it
/// (docs/tmt/project.md (Declaration derivation)) — or the object's own
/// rendered interface otherwise. The text is NOT read yet: that happens
/// in the shared fixpoint every declaration source in the build goes
/// through together (`crate::header::resolve_declarations`), since a
/// header can itself depend on another library or unit. Neither file
/// present, in any searched directory, is an error naming the library.
///
/// A SEPARATE function from [`find_library`], because the two now mean
/// different things: `tmt link`'s own `-l` (and the LSP overlay's own
/// fixture, `lsp/overlay.rs`) links an OBJECT and nothing else — it has
/// no declarations table to populate and no use for a header-only
/// library, which carries no object to link at all — while `tmt build`'s
/// `-l`/`-L` ALSO select compile-time declarations. Both functions share
/// the same directory search and the same `<name>.tmo` candidate path.
pub(crate) fn find_library_for_build(
    name: &str,
    dirs: &[String],
) -> Result<(Option<ObjectFile>, crate::header::DeclarationText), String> {
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
        let text = if has_tmh {
            let source = fs::read_to_string(&tmh)
                .map_err(|e| format!("cannot read {}: {e}", tmh.display()))?;
            crate::header::DeclarationText::Source {
                path: tmh,
                text: source,
            }
        } else {
            // `has_tmo` alone, checked above.
            let obj = object.clone().expect("has_tmo checked above");
            crate::header::DeclarationText::Object {
                path: tmo,
                object: Box::new(obj),
            }
        };
        return Ok((object, text));
    }
    Err(format!("library `{name}` not found on the -L search path"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Resolved;
    use mtc_core::linker::LinkDiagnostic;

    // ---- find_library_for_build -------------------------------------------
    //
    // The resolver's own decision point for `tmt build`'s library handling
    // (docs/tmt/project.md (Declaration derivation)): white-box tests here pin exactly
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
        let text = crate::header::from_source(FIND_LIBRARY_FIXTURE, &Declarations::stdlib())
            .unwrap_or_else(|e| panic!("interface: {e}"));
        fs::write(dir.join("lib.tmh"), text).unwrap();
    }

    /// Runs the ONE reader (`crate::header::resolve_declarations`) over a
    /// single, already-located `DeclarationText` — the same read
    /// `find_library_for_build`'s own caller performs as part of the
    /// shared fixpoint, done here alone so a unit test can inspect what a
    /// given text actually declares.
    fn read_alone(text: crate::header::DeclarationText) -> Resolved {
        let source = crate::header::DeclarationSource {
            origin: Origin::Library("lib".to_string()),
            text,
        };
        crate::header::resolve_declarations(&[source], true)
            .into_iter()
            .next()
            .expect("one source in, one result out")
            .unwrap_or_else(|e| panic!("read: {}", e.message))
    }

    fn declares_flip(resolved: &Resolved) -> bool {
        resolved.worlds.iter().any(|w| w.name == "lib::flip")
    }

    /// Mutation: preferring the object's own (reduced) declarations even
    /// when a header also exists — the returned TEXT would then be the
    /// object's, not the header's.
    #[test]
    fn find_library_for_build_returns_both_when_both_files_exist() {
        let dir = unique_tmp_dir("both");
        write_fixture_object(&dir);
        write_fixture_header(&dir);
        let dirs = vec![dir.to_string_lossy().into_owned()];

        let (object, text) = find_library_for_build("lib", &dirs).unwrap();
        assert!(
            object.is_some(),
            "the object must still be returned to link"
        );
        assert!(
            matches!(text, crate::header::DeclarationText::Source { .. }),
            "the header wins as the declaration source when both exist"
        );
        assert!(declares_flip(&read_alone(text)));
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

        let (object, text) = find_library_for_build("lib", &dirs).unwrap();
        assert!(object.is_some());
        assert!(matches!(
            text,
            crate::header::DeclarationText::Object { .. }
        ));
        assert!(declares_flip(&read_alone(text)));
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

        let (object, text) = find_library_for_build("lib", &dirs).unwrap();
        assert!(
            object.is_none(),
            "no .tmo exists for this library — nothing to hand the linker"
        );
        assert!(matches!(
            text,
            crate::header::DeclarationText::Source { .. }
        ));
        assert!(declares_flip(&read_alone(text)));
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

    const GRAPH_LIBRARY_FIXTURE: &str = "\
namespace glib {
  export alphabet marks { '_', '0', '1' }
  export graph mark(tape t: marks) {
    entry state s { [*] -> write ['1'] stop; }
  }
}
";

    fn write_graph_fixture_object(dir: &Path) {
        let out = compile_source(
            GRAPH_LIBRARY_FIXTURE,
            CompileOptions {
                externals: Declarations::none(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("compile: {e}"));
        fs::write(dir.join("glib.tmo"), out.object.to_bytes()).unwrap();
    }

    fn write_graph_fixture_header(dir: &Path) {
        let text = crate::header::from_source(GRAPH_LIBRARY_FIXTURE, &Declarations::stdlib())
            .unwrap_or_else(|e| panic!("interface: {e}"));
        fs::write(dir.join("glib.tmh"), text).unwrap();
    }

    /// The discriminating half of "the header wins when both exist": a
    /// routine signature (`find_library_for_build_returns_both_when_
    /// both_files_exist`, above) is content BOTH arms can carry, so that
    /// test alone would stay green even if the preference were inverted,
    /// as long as the same routine happens to be declared identically on
    /// both. An exported GRAPH is not — the object arm carries no graph
    /// body at all (docs/formats.md (routine interfaces)), so its
    /// presence in the read-back table is proof the HEADER, not the
    /// object, was the source.
    ///
    /// Mutation: preferring the object's declarations even when a header
    /// exists — `glib::mark` would then be MISSING from the table
    /// entirely, not merely narrower, which this assertion catches
    /// directly.
    #[test]
    fn find_library_for_build_prefers_the_header_for_a_graph_body_the_object_cannot_supply() {
        let dir = unique_tmp_dir("header-wins-graph");
        write_graph_fixture_object(&dir);
        write_graph_fixture_header(&dir);
        let dirs = vec![dir.to_string_lossy().into_owned()];

        let (object, text) = find_library_for_build("glib", &dirs).unwrap();
        assert!(
            object.is_some(),
            "the object must still be returned to link"
        );
        let resolved = read_alone(text);
        assert!(
            resolved.worlds.iter().any(|w| w.name == "glib::mark"),
            "the header's graph body must be what this table reads when both exist"
        );
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
