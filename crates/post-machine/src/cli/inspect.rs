//! Inspection subcommands: dis, tape, ir.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::formats::{ContainerKind, sniff};
use mtc_core::linker::MapFile;

use crate::arch::DEFAULT_GLYPHS;
use crate::ir::IrProgram;

use super::{Args, CliOutput, render_tape};

const DIS_USAGE: &str = "\
USAGE: pmt dis FILE.pmo|FILE.pmx [--listing] [--map FILE.pmx.map]

Objects disassemble with real names from the symbol table. Executables
use the .pmx.map sidecar when present (FILE.pmx.map or --map), else
recursive-descent discovery (func_XXXX). --listing prints the debugger
code view: addresses + raw bytes, not reassembleable.
";

pub(super) fn dis(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(DIS_USAGE.into(), String::new()));
    }
    let listing = args.flag("--listing");
    let map_path = args.value("--map")?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("dis takes exactly one input\n\n{DIS_USAGE}"));
    };
    let path = Path::new(input);
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    match sniff(&bytes) {
        Some(ContainerKind::Object) => {
            if listing {
                return Err("--listing applies to executables only".into());
            }
            let obj = ObjectFile::from_bytes(&bytes).map_err(|e| e.to_string())?;
            Ok(CliOutput::ok(
                crate::asm::disassemble_object(&obj),
                String::new(),
            ))
        }
        Some(ContainerKind::Executable) => {
            let exe = Executable::from_bytes(&bytes).map_err(|e| e.to_string())?;
            let map = load_map(path, map_path)?;
            let text = if listing {
                crate::asm::listing_executable(&exe, map.as_ref())
            } else {
                match &map {
                    Some(m) => crate::asm::disassemble_executable_with_map(&exe, m),
                    None => crate::asm::disassemble_executable(&exe),
                }
            };
            Ok(CliOutput::ok(text, String::new()))
        }
        Some(ContainerKind::TapeBlock) => Err("that is a tape block — use `pmt tape show`".into()),
        None => Err(format!("{}: not a toolchain container", path.display())),
    }
}

/// Sidecar discovery only: `FILE.pmx.map` next to the executable, ignored
/// silently if missing or unparsable (a stale sidecar must not break
/// plain `dis`/`run`). Shared by [`load_map`] and `run::run`.
pub(super) fn sidecar_map(exe_path: &Path) -> Option<MapFile> {
    let mut sidecar = exe_path.as_os_str().to_owned();
    sidecar.push(".map");
    let sidecar = PathBuf::from(sidecar);
    fs::read_to_string(&sidecar)
        .ok()
        .and_then(|text| MapFile::from_json(&text).ok())
}

/// Explicit --map wins; else the sidecar. A present but unparsable
/// explicit map is an error, an unparsable sidecar is silently ignored.
fn load_map(exe_path: &Path, explicit: Option<String>) -> Result<Option<MapFile>, String> {
    if let Some(p) = explicit {
        let text = fs::read_to_string(&p).map_err(|e| format!("cannot read {p}: {e}"))?;
        return MapFile::from_json(&text)
            .map(Some)
            .map_err(|e| format!("{p}: {e}"));
    }
    Ok(sidecar_map(exe_path))
}

const TAPE_USAGE: &str = "\
USAGE: pmt tape build \" * * *\" [--head N] [-o OUT.pmt]
       pmt tape show FILE.pmt

build: cell characters are the PM-1 glyphs (space = blank, * = mark);
the leftmost character is cell 0. show: renders any .pmt with its own
alphabet.
";

pub(super) fn tape(raw: &[String]) -> Result<CliOutput, String> {
    match raw.first().map(String::as_str) {
        Some("build") => tape_build(&raw[1..]),
        Some("show") => tape_show(&raw[1..]),
        _ => Ok(CliOutput::ok(TAPE_USAGE.into(), String::new())),
    }
}

fn tape_build(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let head: i64 = match args.value("--head")? {
        Some(text) => text.parse().map_err(|_| format!("bad --head `{text}`"))?,
        None => 0,
    };
    let out = args.value("-o")?.unwrap_or_else(|| "tape.pmt".into());
    let inputs = args.positionals()?;
    let [pattern] = inputs.as_slice() else {
        return Err(format!(
            "tape build takes exactly one pattern\n\n{TAPE_USAGE}"
        ));
    };
    let cells: Vec<u8> = pattern
        .chars()
        .map(|c| match c {
            ' ' => Ok(0),
            '*' => Ok(1),
            other => Err(format!("bad cell character `{other}` (space or *)")),
        })
        .collect::<Result<_, _>>()?;
    let block = TapeBlockFile {
        alphabet: DEFAULT_GLYPHS.iter().map(|g| g.to_string()).collect(),
        tapes: vec![TapeSnapshot {
            origin: 0,
            cells,
            head,
        }],
    };
    fs::write(&out, block.to_bytes()).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

fn tape_show(raw: &[String]) -> Result<CliOutput, String> {
    let args = Args::new(raw);
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("tape show takes exactly one file\n\n{TAPE_USAGE}"));
    };
    let bytes = fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let block = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{input}: {e}"))?;
    let mut out = format!("alphabet: {:?}\n", block.alphabet);
    for (i, tape) in block.tapes.iter().enumerate() {
        out.push_str(&format!("tape {i}: {}", render_tape(tape, &block.alphabet)));
    }
    Ok(CliOutput::ok(out, String::new()))
}

const IR_USAGE: &str = "\
USAGE: pmt ir graph FILE.ir.json [--function NAME]

Renders --emit-ir output as a Mermaid flowchart (one per function).
";

pub(super) fn ir(raw: &[String]) -> Result<CliOutput, String> {
    match raw.first().map(String::as_str) {
        Some("graph") => ir_graph(&raw[1..]),
        _ => Ok(CliOutput::ok(IR_USAGE.into(), String::new())),
    }
}

fn ir_graph(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let filter = args.value("--function")?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("ir graph takes exactly one file\n\n{IR_USAGE}"));
    };
    let text = fs::read_to_string(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let program = IrProgram::from_json(&text).map_err(|e| format!("{input}: {e}"))?;
    let mut out = String::new();
    for function in &program.functions {
        if filter.as_deref().is_some_and(|f| f != function.name) {
            continue;
        }
        out.push_str(&format!(
            "%% {}\n{}\n",
            function.name,
            function.to_mermaid()
        ));
    }
    if out.is_empty() {
        return Err(match filter {
            Some(f) => format!("no function `{f}` in {input}"),
            None => format!("{input}: no functions"),
        });
    }
    Ok(CliOutput::ok(out, String::new()))
}
