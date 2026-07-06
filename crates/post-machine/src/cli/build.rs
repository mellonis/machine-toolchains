//! Build-side subcommands: compile, asm, link.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::LinkOptions;

use crate::compiler::{CompileOptions, CompileReport, compile as compile_source};
use crate::optimizer::OptLevel;
use crate::stdlib;

use super::{Args, CliOutput};

const COMPILE_USAGE: &str = "\
USAGE: pmt compile INPUT.pmc [-o OUT.pmo] [FLAGS]

FLAGS:
  -g                 record debug info (labels + .pmc lines)
  -O0 | -O1          optimization level (default -O0)
  --strip-debugger   drop `brk` at codegen
  --debug            preset: -g -O0
  --release          preset: -O1 --strip-debugger
  -S                 emit the generated .pma instead of an object
  --emit-ir[=STAGE]  write the CFG IR JSON next to the output
                     (STAGE: lowered | after:<pass> | final; default final;
                      repeated stages resolve last-wins)
  --fno-<pass>       disable one optimizer pass (repeatable)
  -Werror            treat warnings as errors
  -v                 render the compile report (passes, rounds)
";

fn out_path(input: &Path, explicit: Option<String>, extension: &str) -> PathBuf {
    match explicit {
        Some(path) => PathBuf::from(path),
        None => input.with_extension(extension),
    }
}

fn render_warnings(stderr: &mut String, input: &Path, report: &CompileReport) {
    for w in &report.warnings {
        let _ = writeln!(
            stderr,
            "{}:{}: warning: {}",
            input.display(),
            w.line,
            w.message
        );
    }
}

fn render_opt_report(stderr: &mut String, report: &CompileReport) {
    let _ = writeln!(stderr, "opt: {} round(s)", report.opt.rounds);
    for change in &report.opt.changes {
        let _ = writeln!(
            stderr,
            "  {} {}: {} change(s)",
            change.pass, change.function, change.changes
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
            "{}:{}:{}: error: {}",
            input.display(),
            e.line,
            e.col,
            e.kind
        )
    })?;

    let mut stderr = String::new();
    render_warnings(&mut stderr, input, &out.report);
    if verbose {
        render_opt_report(&mut stderr, &out.report);
    }
    if werror && !out.report.warnings.is_empty() {
        return Err(format!(
            "{stderr}-Werror: {} warning(s) treated as errors",
            out.report.warnings.len()
        ));
    }

    let target = out_path(input, explicit_out, if emit_asm { "pma" } else { "pmo" });
    if emit_asm {
        fs::write(&target, &out.pma)
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
                .rev() // repeated stages resolve last-wins (docs/cli.md)
                .find(|(l, _)| l == label)
                .map(|(_, program)| program.to_json())
                .ok_or_else(|| format!("no IR snapshot labeled `{label}` was captured"))?,
        };
        fs::write(&ir_path, json)
            .map_err(|e| format!("cannot write {}: {e}", ir_path.display()))?;
    }

    Ok(CliOutput::ok(String::new(), stderr))
}

/// `--emit-ir` → Some(None); `--emit-ir=STAGE` → Some(Some(stage)).
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
            let known = stage == "lowered" || stage == "final" || stage.starts_with("after:");
            if !known {
                return Err(format!(
                    "unknown IR stage `{stage}` (lowered | after:<pass> | final)"
                ));
            }
            return Ok(Some(Some(stage)));
        }
    }
    Ok(None)
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

const ASM_USAGE: &str = "\
USAGE: pmt asm INPUT.pma [-o OUT.pmo] [-g]
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
    let object = crate::asm::assemble(&source, with_debug)
        .map_err(|e| format!("{}: {e}", input.display()))?;
    let target = out_path(input, explicit_out, "pmo");
    fs::write(&target, object.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

const LINK_USAGE: &str = "\
USAGE: pmt link INPUT.pmo... [-o OUT.pmx] [FLAGS]

FLAGS:
  --no-relax    keep every symbol site in far form
  --nostdlib    do not link the built-in std
  -L DIR        add a library search directory (repeatable, in order)
  -l NAME       link NAME.pmo from the search path (repeatable)
  -v            render the link report (dropped functions, relaxation)

Writes OUT.pmx and the OUT.pmx.map sidecar (function ranges; label/line
info when the objects carry -g debug data).
";

pub(super) fn link(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LINK_USAGE.into(), String::new()));
    }
    let relax = !args.flag("--no-relax");
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
    if !nostdlib {
        libraries.push(stdlib::object().clone());
    }

    let linked =
        crate::asm::link(&objects, &libraries, LinkOptions { relax }).map_err(|e| e.to_string())?;

    let target = out_path(Path::new(&inputs[0]), explicit_out, "pmx");
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
    }
    Ok(CliOutput::ok(String::new(), stderr))
}

/// `app.pmx` → `app.pmx.map` (docs/cli.md; docs/formats.md: the sidecar
/// keeps the full executable name).
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
        let candidate = Path::new(dir).join(format!("{name}.pmo"));
        if candidate.exists() {
            return read_object(&candidate);
        }
    }
    Err(format!("library `{name}` not found on the -L search path"))
}
