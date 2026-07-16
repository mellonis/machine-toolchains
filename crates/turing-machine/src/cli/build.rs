//! Build-side subcommands: asm, link. Mirrors the PM-1 `pmt` shapes with
//! `.tma`/`.tmo`/`.tmx` extensions. TM-1 has no embedded stdlib yet, so
//! `link` drops PM-1's `--nostdlib` flag and stdlib auto-link — every
//! object comes from the command line or the `-L` search path.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::LinkOptions;

use super::{Args, CliOutput};

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
  --no-relax    keep every call site in far form
  -L DIR        add a library search directory (repeatable, in order)
  -l NAME       link NAME.tmo from the search path (repeatable)
  -v            render the link report (dropped functions, relaxation)

Writes OUT.tmx and the OUT.tmx.map sidecar (function ranges + table
section info; label/line info when the objects carry -g debug data).
";

pub(super) fn link(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LINK_USAGE.into(), String::new()));
    }
    let relax = !args.flag("--no-relax");
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

    let linked =
        crate::asm::link(&objects, &libraries, LinkOptions { relax }).map_err(|e| e.to_string())?;

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
