//! Inspection subcommands: dis, tape. Mirrors the PM-1 `pmt` shapes with
//! `.tmo`/`.tmx`/`.tmt` extensions. There is deliberately no `tape build`:
//! that PM-1 subcommand is glyph-pattern sugar (`" * *"`) tied to the fixed
//! two-symbol PM-1 alphabet; TM-1 tapes carry per-tape alphabets, so their
//! cells are set through `tape set --cells` against a template minted by
//! `tape new --from`.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::formats::{ContainerKind, sniff};
use mtc_core::linker::MapFile;

use crate::ir::IrProgram;

use super::{Args, CliOutput, render_tape};

const DIS_USAGE: &str = "\
USAGE: tmt dis FILE.tmo|FILE.tmx [--listing] [--map FILE.tmx.map]

Objects disassemble with real names from the symbol table. Executables
use the .tmx.map sidecar when present (FILE.tmx.map or --map), else
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
        Some(ContainerKind::TapeBlock) => Err("that is a tape block — use `tmt tape show`".into()),
        None => Err(format!("{}: not a toolchain container", path.display())),
    }
}

/// Sidecar discovery only: `FILE.tmx.map` next to the executable, ignored
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
USAGE: tmt tape new --from APP.tmx [-o OUT.tmt]
       tmt tape set IN.tmt (-o OUT.tmt | --in-place)
                    [--tape N] [--cells PATTERN] [--origin N] [--head N]
       tmt tape show FILE.tmt

new: a blank template sized to the executable's tape count, each tape's
alphabet the decimal labels 0..card-1 from the image's per-tape
cardinalities. set: clone IN.tmt, applying edits to tape N (default 0);
--cells maps each character through tape N's effective alphabet. show:
renders any .tmt with its own alphabet.
";

pub(super) fn tape(raw: &[String]) -> Result<CliOutput, String> {
    match raw.first().map(String::as_str) {
        Some("new") => tape_new(&raw[1..]),
        Some("set") => tape_set(&raw[1..]),
        Some("show") => tape_show(&raw[1..]),
        _ => Ok(CliOutput::ok(TAPE_USAGE.into(), String::new())),
    }
}

/// `tmt tape new --from APP.tmx [-o OUT.tmt]` — a blank tape template sized
/// to the executable's tape count, one band per tape. Each band carries
/// its own alphabet: the decimal-string labels `0..card-1` taken from the
/// image's per-tape `alphabet_cardinalities` (MT v2 per-tape alphabets).
/// A v1 code-only image (no cardinalities) falls back to a binary `["0",
/// "1"]` alphabet per its single band.
fn tape_new(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let from = args.value("--from")?;
    let out = args.value("-o")?.unwrap_or_else(|| "blank.tmt".into());
    let extra = args.positionals()?;
    if !extra.is_empty() {
        return Err(format!(
            "tape new takes no positional arguments\n\n{TAPE_USAGE}"
        ));
    }
    let Some(from) = from else {
        return Err(format!("tape new needs --from APP.tmx\n\n{TAPE_USAGE}"));
    };
    let bytes = fs::read(&from).map_err(|e| format!("cannot read {from}: {e}"))?;
    match sniff(&bytes) {
        Some(ContainerKind::Executable) => {}
        _ => return Err(format!("{from}: not an executable image (.tmx)")),
    }
    let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{from}: {e}"))?;

    // Per-tape cardinalities drive per-band alphabets. A v1 code-only
    // image carries none, so fall back to one binary band per tape.
    let cardinalities: Vec<u32> = if exe.alphabet_cardinalities.is_empty() {
        vec![2; usize::from(exe.tape_count).max(1)]
    } else {
        exe.alphabet_cardinalities.clone()
    };
    let labels = |card: u32| -> Vec<String> { (0..card).map(|i| i.to_string()).collect() };

    let block = TapeBlockFile {
        // The block-level alphabet is a fallback only (every band overrides
        // it); size it to the widest band so `tape show` renders sanely if a
        // band ever drops its override.
        alphabet: labels(cardinalities.iter().copied().max().unwrap_or(2)),
        tapes: cardinalities
            .iter()
            .map(|&card| TapeSnapshot {
                origin: 0,
                cells: Vec::new(),
                head: 0,
                alphabet: Some(labels(card)),
            })
            .collect(),
    };
    fs::write(&out, block.to_bytes()).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

/// `tmt tape set IN.tmt (-o OUT.tmt | --in-place) [--tape N] [--cells P]
/// [--origin N] [--head N]` — clone semantics: read `IN.tmt`, apply the
/// given edits to tape N (default 0), and write the result out. The source
/// is never mutated; the output goes to `-o` or, with `--in-place`, back
/// over the input. Any subset of edits may be given; none is a plain copy.
/// `--cells` maps each character of the pattern through tape N's effective
/// alphabet (its own if present, else the block's) by glyph.
fn tape_set(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let out = args.value("-o")?;
    let in_place = args.flag("--in-place");
    let tape_index: usize = match args.value("--tape")? {
        Some(text) => text.parse().map_err(|_| format!("bad --tape `{text}`"))?,
        None => 0,
    };
    let cells = args.value("--cells")?;
    let origin: Option<i64> = match args.value("--origin")? {
        Some(text) => Some(text.parse().map_err(|_| format!("bad --origin `{text}`"))?),
        None => None,
    };
    let head: Option<i64> = match args.value("--head")? {
        Some(text) => Some(text.parse().map_err(|_| format!("bad --head `{text}`"))?),
        None => None,
    };
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("tape set takes exactly one file\n\n{TAPE_USAGE}"));
    };

    // Output destination: exactly one of -o / --in-place. Refusing the
    // neither case is what keeps `set` from silently clobbering IN.tmt.
    let dest: String = match (out, in_place) {
        (Some(_), true) => {
            return Err(format!(
                "tape set: -o and --in-place are mutually exclusive\n\n{TAPE_USAGE}"
            ));
        }
        (Some(path), false) => path,
        (None, true) => input.clone(),
        (None, false) => {
            return Err(format!(
                "tape set needs -o OUT.tmt or --in-place\n\n{TAPE_USAGE}"
            ));
        }
    };

    let bytes = fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let mut block = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{input}: {e}"))?;
    let tape_count = block.tapes.len();
    if tape_index >= tape_count {
        return Err(format!(
            "--tape {tape_index}: out of range (block has {tape_count} tape(s))"
        ));
    }

    // Map the pattern under two shared borrows (the tape's own alphabet
    // and the block fallback) BEFORE taking the mutable tape borrow.
    let new_cells: Option<Vec<u8>> = match cells {
        Some(pattern) => {
            let effective: &[String] = block.tapes[tape_index]
                .alphabet
                .as_deref()
                .unwrap_or(&block.alphabet);
            let mapped = pattern
                .chars()
                .map(|c| {
                    let glyph = c.to_string();
                    effective
                        .iter()
                        .position(|g| *g == glyph)
                        .map(|i| i as u8)
                        .ok_or_else(|| {
                            format!("bad cell character `{c}` (alphabet: {effective:?})")
                        })
                })
                .collect::<Result<Vec<u8>, _>>()?;
            Some(mapped)
        }
        None => None,
    };

    let tape = &mut block.tapes[tape_index];
    if let Some(cells) = new_cells {
        tape.cells = cells;
    }
    if let Some(origin) = origin {
        tape.origin = origin;
    }
    if let Some(head) = head {
        tape.head = head;
    }

    fs::write(&dest, block.to_bytes()).map_err(|e| format!("cannot write {dest}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

const IR_USAGE: &str = "\
USAGE: tmt ir graph FILE.ir.json [--function NAME]

Renders --emit-ir output as a Mermaid flowchart (one per world). The filter
flag keeps pmt's `--function` name for cross-tool muscle memory; a TM world
IS the unit here (the `machine` block or a routine), so NAME is a world name.
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
    for world in &program.worlds {
        if filter.as_deref().is_some_and(|f| f != world.name) {
            continue;
        }
        out.push_str(&format!("%% {}\n{}\n", world.name, world.to_mermaid()));
    }
    if out.is_empty() {
        return Err(match filter {
            Some(f) => format!("no world `{f}` in {input}"),
            None => format!("{input}: no worlds"),
        });
    }
    Ok(CliOutput::ok(out, String::new()))
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
        // Each band renders through its own effective alphabet (its
        // override if present, else the block fallback).
        let effective: &[String] = tape.alphabet.as_deref().unwrap_or(&block.alphabet);
        out.push_str(&format!("tape {i}: {}", render_tape(tape, effective)));
    }
    Ok(CliOutput::ok(out, String::new()))
}
