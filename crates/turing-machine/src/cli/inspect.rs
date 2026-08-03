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
use mtc_core::formats::{ARCH_TM1, ContainerKind, parse_glyph_list, parse_glyph_sequence, sniff};
use mtc_core::linker::MapFile;
use mtc_core::vm::LoadError;

use crate::ir::IrProgram;

use super::{Args, CliOutput, Delimit, parse_keyed, render_tape};

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
            if obj.arch != ARCH_TM1 {
                return Err(LoadError::UnknownArch(obj.arch).to_string());
            }
            Ok(CliOutput::ok(
                crate::asm::disassemble_object(&obj),
                String::new(),
            ))
        }
        Some(ContainerKind::Executable) => {
            let exe = Executable::from_bytes(&bytes).map_err(|e| e.to_string())?;
            if exe.arch != ARCH_TM1 {
                return Err(LoadError::UnknownArch(exe.arch).to_string());
            }
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
        Some(ContainerKind::TapeBlock) => {
            Err("that is a tape block — use `tmt tape-block show`".into())
        }
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
USAGE: tmt tape-block new [--from APP.tmx | --from APP.tmc] [-o OUT.tmt] [EDITS]
       tmt tape-block set IN.tmt (-o OUT.tmt | --in-place)
                    [--from APP.tmc] [EDITS]
       tmt tape-block show FILE.tmt [--dense | --separated]

EDITS (repeatable; KEY is a tape index, or a tape name with --from a .tmc):
  --alphabet KEY=GLYPHS   repin tape KEY's glyphs (relabels; same cardinality)
  --cells    KEY=GLYPHS   set tape KEY's cells
  --head     KEY=N        set tape KEY's head
  --origin   KEY=N        set tape KEY's origin

GLYPHS is alphabet notation: ' ','s','1' or '0'..'9'. --alphabet applies
before --cells, so cells resolve against the glyphs just pinned.
";

/// The keyed edits one invocation carries, in flag order.
pub(super) struct Edits {
    pub alphabets: Vec<(String, String)>,
    pub cells: Vec<(String, String)>,
    pub heads: Vec<(String, String)>,
    pub origins: Vec<(String, String)>,
}

pub(super) fn collect_edits(args: &mut Args) -> Result<Edits, String> {
    Ok(Edits {
        alphabets: parse_keyed("--alphabet", &args.values("--alphabet")?)?,
        cells: parse_keyed("--cells", &args.values("--cells")?)?,
        heads: parse_keyed("--head", &args.values("--head")?)?,
        origins: parse_keyed("--origin", &args.values("--origin")?)?,
    })
}

/// Resolve a tape key to its band index. A key is an index, or — when `names`
/// is non-empty, i.e. a `.tmc` source supplied them — a declared tape name
/// (docs/tmt/cli.md (tape-block)).
fn resolve_key(key: &str, names: &[String], tape_count: usize) -> Result<usize, String> {
    if let Some(i) = names.iter().position(|n| n == key) {
        return Ok(i);
    }
    let Ok(index) = key.parse::<usize>() else {
        return if names.is_empty() {
            Err(format!(
                "tape key `{key}`: expected an index — tape names need `--from` a .tmc source"
            ))
        } else {
            Err(format!(
                "tape key `{key}`: no such tape (declared: {})",
                names.join(", ")
            ))
        };
    };
    if index >= tape_count {
        return Err(format!(
            "tape {index}: out of range (block has {tape_count} tape(s))"
        ));
    }
    Ok(index)
}

/// Apply every edit to `block`. `--alphabet` runs first for each tape so
/// `--cells` in the same invocation resolves against the newly pinned glyphs.
///
/// `pm_block_alphabet` writes a repin to the BLOCK alphabet instead of a
/// per-tape override — PM-1 blocks are single-tape and single-alphabet, and
/// keeping the override unset keeps them at MT v1
/// (docs/formats.md (tape-block snapshot)). TM always writes per-tape tables.
pub(super) fn apply_edits(
    block: &mut TapeBlockFile,
    edits: &Edits,
    names: &[String],
    pm_block_alphabet: bool,
) -> Result<(), String> {
    let tape_count = block.tapes.len();

    for (key, text) in &edits.alphabets {
        let index = resolve_key(key, names, tape_count)?;
        let glyphs = parse_glyph_list(text).map_err(|e| format!("--alphabet `{key}`: {e}"))?;
        // A repin relabels; it never resizes. Measured against the TAPE's
        // effective width, which on a multi-band block differs from the block
        // fallback's (docs/formats.md (per-tape glyph tables)).
        let current = block.tapes[index]
            .alphabet
            .as_deref()
            .unwrap_or(&block.alphabet)
            .len();
        if glyphs.len() != current {
            return Err(format!(
                "--alphabet `{key}`: tape {index} has cardinality {current}, \
                 the given alphabet has {} glyphs",
                glyphs.len()
            ));
        }
        if pm_block_alphabet {
            block.alphabet = glyphs;
            block.tapes[index].alphabet = None;
        } else {
            block.tapes[index].alphabet = Some(glyphs);
        }
    }

    for (key, text) in &edits.cells {
        let index = resolve_key(key, names, tape_count)?;
        let effective: Vec<String> = block.tapes[index]
            .alphabet
            .clone()
            .unwrap_or_else(|| block.alphabet.clone());
        let cells = if text.trim().is_empty() {
            Vec::new()
        } else {
            let glyphs = parse_glyph_sequence(text).map_err(|e| format!("--cells `{key}`: {e}"))?;
            glyphs
                .iter()
                .map(|g| {
                    effective
                        .iter()
                        .position(|e| e == g)
                        .map(|i| i as u8)
                        .ok_or_else(|| {
                            format!("--cells `{key}`: glyph `{g}` is not in {effective:?}")
                        })
                })
                .collect::<Result<Vec<u8>, String>>()?
        };
        block.tapes[index].cells = cells;
    }

    for (key, text) in &edits.heads {
        let index = resolve_key(key, names, tape_count)?;
        block.tapes[index].head = text
            .parse()
            .map_err(|_| format!("--head `{key}`: bad value `{text}`"))?;
    }

    for (key, text) in &edits.origins {
        let index = resolve_key(key, names, tape_count)?;
        block.tapes[index].origin = text
            .parse()
            .map_err(|_| format!("--origin `{key}`: bad value `{text}`"))?;
    }

    Ok(())
}

/// Without `--from`, the `--alphabet` flags define the block. Their keys must
/// be indices contiguous from 0, so a mistyped key cannot silently inflate the
/// band count (docs/tmt/cli.md (tape-block)).
fn freehand_bands(edits: &Edits) -> Result<Vec<Vec<String>>, String> {
    let mut bands: Vec<(usize, Vec<String>)> = Vec::new();
    for (key, text) in &edits.alphabets {
        let index = key.parse::<usize>().map_err(|_| {
            format!(
                "--alphabet `{key}`: expected an index — tape names need `--from` a .tmc source"
            )
        })?;
        let glyphs = parse_glyph_list(text).map_err(|e| format!("--alphabet `{key}`: {e}"))?;
        bands.push((index, glyphs));
    }
    bands.sort_by_key(|(index, _)| *index);
    if bands.is_empty() || bands.iter().enumerate().any(|(i, (index, _))| i != *index) {
        return Err(format!(
            "tape-block new without --from needs one --alphabet per tape, \
             keyed contiguously from 0\n\n{TAPE_USAGE}"
        ));
    }
    Ok(bands.into_iter().map(|(_, glyphs)| glyphs).collect())
}

/// `--from` dispatches on the container magic, never on the extension
/// (docs/formats.md (shared conventions)). Returns each band's glyphs and,
/// when the source supplied them, the tape names.
fn from_source_or_image(path: &str) -> Result<(Vec<Vec<String>>, Vec<String>), String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    match sniff(&bytes) {
        Some(ContainerKind::Executable) => {
            let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))?;
            let cards: Vec<u32> = if exe.alphabet_cardinalities.is_empty() {
                vec![2; usize::from(exe.tape_count).max(1)]
            } else {
                exe.alphabet_cardinalities.clone()
            };
            // An image carries cardinalities and nothing else, so each band is
            // labelled with the decimal strings `0..card-1` — the author repins
            // them with `--alphabet` (docs/formats.md (glyph tables)).
            let glyphs = cards
                .iter()
                .map(|&c| (0..c).map(|i| i.to_string()).collect())
                .collect();
            Ok((glyphs, Vec::new()))
        }
        Some(_) => Err(format!("{path}: not an executable image (.tmx)")),
        // Not a container: treat it as source text. A `.tmc` supplies both the
        // glyphs and the tape names, so the common case needs no --alphabet at
        // all (docs/tmt/cli.md (tape-block)).
        None => {
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("{path}: not an executable image and not UTF-8 source"))?;
            let layout = crate::compiler::machine_tape_layout(&text)
                .map_err(|e| format!("{path}: {e:?}"))?
                .ok_or_else(|| {
                    format!("{path}: declares no `machine` block, so it has no tape band")
                })?;
            let glyphs = layout.iter().map(|t| t.glyphs.clone()).collect();
            let names = layout.iter().map(|t| t.name.clone()).collect();
            Ok((glyphs, names))
        }
    }
}

pub(super) fn tape_block(raw: &[String]) -> Result<CliOutput, String> {
    match raw.first().map(String::as_str) {
        Some("new") => tape_new(&raw[1..]),
        Some("set") => tape_set(&raw[1..]),
        Some("show") => tape_show(&raw[1..]),
        _ => Ok(CliOutput::ok(TAPE_USAGE.into(), String::new())),
    }
}

/// `tmt tape-block new [--from APP.tmx] [-o OUT.tmt] [EDITS]` — mint a block
/// and apply this invocation's edits to it in one call.
///
/// With `--from` an executable, the band count and each band's cardinality
/// come from the image header, and glyphs default to the decimal labels
/// `0..card-1`. Without `--from`, the `--alphabet` flags define the block:
/// their keys must be contiguous from 0 (docs/tmt/cli.md (tape-block)).
fn tape_new(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let from = args.value("--from")?;
    let out = args.value("-o")?.unwrap_or_else(|| "blank.tmt".into());
    let edits = collect_edits(&mut args)?;
    let extra = args.positionals()?;
    if !extra.is_empty() {
        return Err(format!(
            "tape-block new takes no positional arguments\n\n{TAPE_USAGE}"
        ));
    }

    let (band_glyphs, names): (Vec<Vec<String>>, Vec<String>) = match from.as_deref() {
        Some(path) => from_source_or_image(path)?,
        None => (freehand_bands(&edits)?, Vec::new()),
    };

    let widest = band_glyphs.iter().map(Vec::len).max().unwrap_or(2);
    let mut block = TapeBlockFile {
        // The block-level alphabet is a fallback only (every band overrides
        // it); size it to the widest band so `tape-block show` renders sanely
        // if a band ever drops its override.
        alphabet: (0..widest).map(|i| i.to_string()).collect(),
        tapes: band_glyphs
            .iter()
            .map(|glyphs| TapeSnapshot {
                origin: 0,
                cells: Vec::new(),
                head: 0,
                alphabet: Some(glyphs.clone()),
            })
            .collect(),
    };

    apply_edits(&mut block, &edits, &names, false)?;

    let bytes = block.to_bytes().map_err(|e| format!("{out}: {e}"))?;
    fs::write(&out, bytes).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

/// `tmt tape-block set IN.tmt (-o OUT.tmt | --in-place) [--from APP.tmc]
/// [EDITS]` — clone semantics: read `IN.tmt`, apply this invocation's edits,
/// and write the result out. The source is never mutated; the output goes to
/// `-o` or, with `--in-place`, back over the input. Any subset of edits may
/// be given; none is a plain copy.
///
/// `--from APP.tmc` supplies tape NAMES only, so an edit may be keyed by name
/// instead of index; it never reshapes the block
/// (docs/tmt/cli.md (tape-block)).
fn tape_set(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let out = args.value("-o")?;
    let in_place = args.flag("--in-place");
    let from = args.value("--from")?;
    let edits = collect_edits(&mut args)?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "tape-block set takes exactly one file\n\n{TAPE_USAGE}"
        ));
    };

    // Output destination: exactly one of -o / --in-place. Refusing the
    // neither case is what keeps `set` from silently clobbering IN.tmt.
    let dest: String = match (out, in_place) {
        (Some(_), true) => {
            return Err(format!(
                "tape-block set: -o and --in-place are mutually exclusive\n\n{TAPE_USAGE}"
            ));
        }
        (Some(path), false) => path,
        (None, true) => input.clone(),
        (None, false) => {
            return Err(format!(
                "tape-block set needs -o OUT.tmt or --in-place\n\n{TAPE_USAGE}"
            ));
        }
    };

    let bytes = fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let mut block = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{input}: {e}"))?;

    let names: Vec<String> = match from.as_deref() {
        Some(path) => from_source_or_image(path)?.1,
        None => Vec::new(),
    };

    apply_edits(&mut block, &edits, &names, false)?;

    let bytes = block.to_bytes().map_err(|e| format!("{dest}: {e}"))?;
    fs::write(&dest, bytes).map_err(|e| format!("cannot write {dest}: {e}"))?;
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
    let mut args = Args::new(raw);
    let dense = args.flag("--dense");
    let separated = args.flag("--separated");
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "tape-block show takes exactly one file\n\n{TAPE_USAGE}"
        ));
    };
    let delimit = match (dense, separated) {
        (true, true) => {
            return Err("tape-block show: --dense and --separated are mutually exclusive".into());
        }
        (true, false) => Delimit::Dense,
        (false, true) => Delimit::Separated,
        (false, false) => Delimit::Auto,
    };
    let bytes = fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let block = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{input}: {e}"))?;
    let mut out = String::new();
    for (i, tape) in block.tapes.iter().enumerate() {
        // Each band renders through its own effective alphabet, and PRINTS
        // it: which glyphs a band actually uses is not derivable from the
        // block fallback, which is only a default for bands that carry no
        // table of their own (docs/formats.md (per-tape glyph tables)).
        let effective: &[String] = tape.alphabet.as_deref().unwrap_or(&block.alphabet);
        let rendered = render_tape(tape, effective, delimit);
        let (head_line, rest) = rendered
            .split_once('\n')
            .expect("render_tape emits a head line then the span");
        out.push_str(&format!(
            "tape {i}: {head_line}, alphabet {effective:?}\n{rest}"
        ));
    }
    Ok(CliOutput::ok(out, String::new()))
}
