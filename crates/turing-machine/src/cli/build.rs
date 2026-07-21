//! Build-side subcommands: compile, asm, link. Mirrors the PM-1 `pmt` shapes
//! with `.tmc`/`.tma`/`.tmo`/`.tmx` extensions. `link` auto-links the
//! embedded standard library (`std::binaryNumbers` / `std::binaryNumbersBare`)
//! lazily via reachability, with `--nostdlib` to opt out — the PM-1 `link`
//! wiring. `compile` needs no such flag: the stdlib is a link-time input, not
//! a compile-time one.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::{CallMech, LinkOptions};

use crate::compiler::{CompileOptions, CompileReport, compile as compile_source};
use crate::optimizer::OptLevel;

use super::{Args, CliOutput};

const COMPILE_USAGE: &str = "\
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
  -Werror            treat warnings as errors
  -v                 render the compile report (passes, rounds)
";

fn render_warnings(stderr: &mut String, input: &Path, report: &CompileReport) {
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

fn render_opt_report(stderr: &mut String, report: &CompileReport) {
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
    if args.flag("--help") {
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

fn take_disabled_passes(args: &mut Args, disabled: &mut Vec<String>) {
    for slot in &mut args.tokens {
        if let Some(tok) = slot.as_deref()
            && let Some(pass) = tok.strip_prefix("--fno-")
        {
            disabled.push(pass.to_string());
            *slot = None;
        }
    }
}

fn out_path(input: &Path, explicit: Option<String>, extension: &str) -> PathBuf {
    match explicit {
        Some(path) => PathBuf::from(path),
        None => input.with_extension(extension),
    }
}

const ASM_USAGE: &str = "\
USAGE: tmt asm INPUT.tma [-o OUT.tmo] [-g]
";

pub(super) fn asm(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
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

const LINK_USAGE: &str = "\
USAGE: tmt link INPUT.tmo... [-o OUT.tmx] [FLAGS]

FLAGS:
  --no-relax        keep every call site in far form
  --entry NAME      link NAME as the program entry (default: main)
  --call-mech MECH  bound-call lowering: mono | frames | hybrid (default: hybrid)
  --nostdlib        do not auto-link the embedded standard library
  -L DIR            add a library search directory (repeatable, in order)
  -l NAME           link NAME.tmo from the search path (repeatable)
  -v                render the link report (dropped functions, relaxation)

Writes OUT.tmx and the OUT.tmx.map sidecar (function ranges + table
section info; label/line info when the objects carry -g debug data).
";

/// Parse `--call-mech` (case-sensitive lowercase); absent selects the
/// default `Hybrid`.
fn parse_call_mech(raw: Option<String>) -> Result<CallMech, String> {
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
    if args.flag("--help") {
        return Ok(CliOutput::ok(LINK_USAGE.into(), String::new()));
    }
    let relax = !args.flag("--no-relax");
    let entry = args.value("--entry")?;
    let call_mech = parse_call_mech(args.value("--call-mech")?)?;
    let nostdlib = args.flag("--nostdlib");
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
        },
    )
    .map_err(|e| e.to_string())?;

    let target = out_path(Path::new(&inputs[0]), explicit_out, "tmx");
    fs::write(&target, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    let map_path = sidecar_path(&target);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    let mut stderr = String::new();
    if verbose {
        let r = &linked.report;
        let _ = writeln!(
            stderr,
            "link: dropped [{}]; {} site(s) relaxed short, {} far",
            r.dropped.join(", "),
            r.relaxed_calls,
            r.far_calls
        );
        // The composition-engine counters follow only when the image carries
        // frames content, so a frameless link keeps the single-line report
        // (docs/core.md (the link report)).
        if r.composites > 0 || r.instantiations > 0 {
            let _ = writeln!(
                stderr,
                "frames: {} composite(s), {} stamp(s), {} B compose table; \
                 {} deduped, {} trap row(s), {} expanded row(s)",
                r.composites,
                r.instantiations,
                r.compose_table_bytes,
                r.dedup_savings,
                r.synthesized_trap_rows,
                r.expanded_rows
            );
        }
    }
    Ok(CliOutput::ok(String::new(), stderr))
}

/// `app.tmx` → `app.tmx.map` (the sidecar keeps the full executable name).
fn sidecar_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".map");
    PathBuf::from(s)
}

fn read_object(path: &Path) -> Result<ObjectFile, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    ObjectFile::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn find_library(name: &str, dirs: &[String]) -> Result<ObjectFile, String> {
    for dir in dirs {
        let candidate = Path::new(dir).join(format!("{name}.tmo"));
        if candidate.exists() {
            return read_object(&candidate);
        }
    }
    Err(format!("library `{name}` not found on the -L search path"))
}
