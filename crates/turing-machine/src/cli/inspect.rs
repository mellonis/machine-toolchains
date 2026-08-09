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
                    [--from APP.tmc] [SHAPE] [EDITS]
       tmt tape-block show FILE.tmt [--dense | --separated]

SHAPE (set only; applied remove -> add -> reorder, before EDITS; flag
order never matters — remove keys name the INPUT block, add positions
count after removals, --reorder and EDITS address the result):
  --add-tape [KEY=]ALPHABET   insert a band at position KEY, or append
  --remove-tape KEY           drop a band
  --reorder K1,K2,...         permute bands (every band exactly once)

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

/// The shape edits one `set` invocation carries. Phase order is fixed —
/// removals (keys name INPUT bands), then adds in flag order, then the
/// permutation; the content `Edits` address the result. Flag position on
/// the command line never matters (docs/tmt/cli.md (tape-block)).
pub(super) struct ShapeEdits {
    pub removes: Vec<String>,
    pub adds: Vec<(Option<usize>, Vec<String>)>,
    pub reorder: Option<Vec<String>>,
}

pub(super) fn collect_shape_edits(args: &mut Args) -> Result<ShapeEdits, String> {
    let removes = args.values("--remove-tape")?;
    let adds = args
        .values("--add-tape")?
        .iter()
        .map(|text| parse_add(text))
        .collect::<Result<Vec<_>, String>>()?;
    let reorders = args.values("--reorder")?;
    if reorders.len() > 1 {
        return Err(
            "--reorder: at most one per invocation — two permutations have no \
             composition order a reader can see"
                .to_string(),
        );
    }
    let reorder = reorders
        .into_iter()
        .next()
        .map(|text| text.split(',').map(|k| k.trim().to_string()).collect());
    Ok(ShapeEdits {
        removes,
        adds,
        reorder,
    })
}

/// `--add-tape [KEY=]ALPHABET`. Keyed iff the text before the first `=` is
/// all digits — a glyph list may itself contain `=` (`'='`), so the split
/// is by prefix shape, not by `=` presence. A tape NAME in the key slot is
/// a dedicated error: a name names a band, not a gap.
fn parse_add(text: &str) -> Result<(Option<usize>, Vec<String>), String> {
    if let Some((prefix, rest)) = text.split_once('=') {
        if !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_digit()) {
            let pos: usize = prefix
                .parse()
                .map_err(|_| format!("--add-tape `{text}`: bad position `{prefix}`"))?;
            let glyphs = parse_glyph_list(rest).map_err(|e| format!("--add-tape `{text}`: {e}"))?;
            return Ok((Some(pos), glyphs));
        }
        if !prefix.is_empty()
            && prefix.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !prefix.starts_with(|c: char| c.is_ascii_digit())
        {
            return Err(format!(
                "--add-tape `{prefix}`: a name names a band, not a gap — \
                 insert positions are numeric"
            ));
        }
    }
    let glyphs = parse_glyph_list(text).map_err(|e| format!("--add-tape `{text}`: {e}"))?;
    Ok((None, glyphs))
}

/// Resolve a tape key to its band index. A key is an index, or — when
/// `names` is non-empty, i.e. a `.tmc` source supplied them — a declared
/// tape name. A name whose band was dropped earlier in this invocation
/// gets a dedicated error instead of "no such tape"
/// (docs/tmt/cli.md (tape-block)).
fn resolve_key(
    key: &str,
    names: &[Option<String>],
    removed: &[String],
    tape_count: usize,
) -> Result<usize, String> {
    if let Some(i) = names.iter().position(|n| n.as_deref() == Some(key)) {
        return Ok(i);
    }
    if removed.iter().any(|n| n == key) {
        return Err(format!(
            "tape `{key}` was removed by --remove-tape in this invocation"
        ));
    }
    let Ok(index) = key.parse::<usize>() else {
        return if names.is_empty() {
            Err(format!(
                "tape key `{key}`: expected an index — tape names need `--from` a .tmc source"
            ))
        } else {
            let declared: Vec<&str> = names.iter().flatten().map(String::as_str).collect();
            Err(format!(
                "tape key `{key}`: no such tape (declared: {})",
                declared.join(", ")
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
    names: &[Option<String>],
    removed: &[String],
    pm_block_alphabet: bool,
) -> Result<(), String> {
    let tape_count = block.tapes.len();

    for (key, text) in &edits.alphabets {
        let index = resolve_key(key, names, removed, tape_count)?;
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
        let index = resolve_key(key, names, removed, tape_count)?;
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
        let index = resolve_key(key, names, removed, tape_count)?;
        block.tapes[index].head = text
            .parse()
            .map_err(|_| format!("--head `{key}`: bad value `{text}`"))?;
    }

    for (key, text) in &edits.origins {
        let index = resolve_key(key, names, removed, tape_count)?;
        block.tapes[index].origin = text
            .parse()
            .map_err(|_| format!("--origin `{key}`: bad value `{text}`"))?;
    }

    Ok(())
}

/// Apply the shape phases, keeping `names` aligned with the bands: a
/// removed band's name is dropped and remembered in `removed_names`, an
/// added band is unnamed, reorder moves names with their bands
/// (docs/tmt/cli.md (tape-block)).
fn reshape(
    block: &mut TapeBlockFile,
    shape: &ShapeEdits,
    names: &mut Vec<Option<String>>,
    removed_names: &mut Vec<String>,
) -> Result<(), String> {
    // Phase 1: removals — every key resolves against the INPUT block.
    let mut drop: Vec<usize> = Vec::new();
    for key in &shape.removes {
        let index = resolve_key(key, names, &[], block.tapes.len())
            .map_err(|e| format!("--remove-tape: {e}"))?;
        if drop.contains(&index) {
            return Err(format!("--remove-tape `{key}`: tape {index} removed twice"));
        }
        drop.push(index);
    }
    drop.sort_unstable();
    for &index in drop.iter().rev() {
        block.tapes.remove(index);
        if !names.is_empty()
            && let Some(name) = names.remove(index)
        {
            removed_names.push(name);
        }
    }

    // Phase 2: adds, in flag order; positions are in the block as the
    // previous phases left it.
    for (pos, glyphs) in &shape.adds {
        let at = pos.unwrap_or(block.tapes.len());
        if at > block.tapes.len() {
            return Err(format!(
                "--add-tape: position {at} out of range (block has {} tape(s) at this point)",
                block.tapes.len()
            ));
        }
        block.tapes.insert(
            at,
            TapeSnapshot {
                origin: 0,
                cells: Vec::new(),
                head: 0,
                alphabet: Some(glyphs.clone()),
            },
        );
        if !names.is_empty() {
            names.insert(at, None);
        }
    }

    // Phase 3: the permutation — complete, over the post-add block.
    if let Some(order) = &shape.reorder {
        let count = block.tapes.len();
        if order.len() != count {
            return Err(format!(
                "--reorder lists {} tape(s), the block has {count}",
                order.len()
            ));
        }
        let mut seen = vec![false; count];
        let mut new_tapes = Vec::with_capacity(count);
        let mut new_names = Vec::with_capacity(count);
        for key in order {
            let index = resolve_key(key, names, removed_names, count)
                .map_err(|e| format!("--reorder: {e}"))?;
            if seen[index] {
                return Err(format!("--reorder `{key}`: tape {index} listed twice"));
            }
            seen[index] = true;
            new_tapes.push(block.tapes[index].clone());
            if !names.is_empty() {
                new_names.push(names[index].clone());
            }
        }
        block.tapes = new_tapes;
        if !names.is_empty() {
            *names = new_names;
        }
    }

    if block.tapes.is_empty() {
        return Err("tape-block would have no tapes".to_string());
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

    let names: Vec<Option<String>> = names.into_iter().map(Some).collect();
    apply_edits(&mut block, &edits, &names, &[], false)?;

    let bytes = block.to_bytes().map_err(|e| format!("{out}: {e}"))?;
    fs::write(&out, bytes).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

/// `tmt tape-block set IN.tmt (-o OUT.tmt | --in-place) [--from APP.tmc]
/// [SHAPE] [EDITS]` — clone semantics: read `IN.tmt`, apply this
/// invocation's shape edits then its content edits, and write the result
/// out. The source is never mutated; the output goes to `-o` or, with
/// `--in-place`, back over the input. Any subset of edits may be given;
/// none is a plain copy.
///
/// `--from APP.tmc` supplies tape NAMES; a SHAPE edit that drops or adds a
/// band keeps `names` aligned so a later EDITS key can still resolve by
/// name (docs/tmt/cli.md (tape-block)).
fn tape_set(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let out = args.value("-o")?;
    let in_place = args.flag("--in-place");
    let from = args.value("--from")?;
    let shape = collect_shape_edits(&mut args)?;
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

    let from_path = from.as_deref();
    let mut names: Vec<Option<String>> = match from_path {
        Some(path) => from_source_or_image(path)?
            .1
            .into_iter()
            .map(Some)
            .collect(),
        None => Vec::new(),
    };

    // `.tmc` sources carry names; a `.tmx` image does not (an image's names
    // stay empty, which is the legitimate index-only mode every consumer
    // below already handles). Whenever a source DID supply names, they must
    // already align one-to-one with the block's bands — `resolve_key`'s
    // name arm and every `reshape` phase index into `names` on that
    // assumption, so this is the one place that establishes it before any
    // of them run (docs/tmt/cli.md (tape-block)).
    if !names.is_empty() && names.len() != block.tapes.len() {
        return Err(format!(
            "--from {}: declares {} tape(s), but {input} has {}",
            from_path.unwrap_or_default(),
            names.len(),
            block.tapes.len()
        ));
    }

    let mut removed_names: Vec<String> = Vec::new();
    reshape(&mut block, &shape, &mut names, &mut removed_names)?;
    apply_edits(&mut block, &edits, &names, &removed_names, false)?;

    let bytes = block.to_bytes().map_err(|e| format!("{dest}: {e}"))?;
    fs::write(&dest, bytes).map_err(|e| format!("cannot write {dest}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

const IR_USAGE: &str = "\
USAGE: tmt ir graph FILE.ir.json [--function NAME]
       tmt ir footprints FILE.ir.json [--function NAME]

`graph` renders --emit-ir output as a Mermaid flowchart (one per world).
`footprints` renders each world's inferred write footprint: per tape, the
symbol indices its body may ever write, out of the tape's cardinality. Both
share the `--function` flag (pmt's flag name, for cross-tool muscle memory);
a TM world IS the unit here (the `machine` block or a routine), so NAME is a
world name.
";

pub(super) fn ir(raw: &[String]) -> Result<CliOutput, String> {
    match raw.first().map(String::as_str) {
        Some("graph") => ir_graph(&raw[1..]),
        Some("footprints") => ir_footprints(&raw[1..]),
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

/// `tmt ir footprints FILE.ir.json [--function NAME]` — renders
/// [`crate::footprint::infer_ir`]'s per-world, per-tape write-set inference
/// over `--emit-ir` JSON. The IR is index-only by contract (no glyph table
/// travels in the sidecar), so the report prints symbol INDICES, one line
/// per tape, in the world's own tape order:
///
/// ```text
/// world std::binaryNumbersBare::invertNumber
///   tape 0 (num): writes {1, 2} of 3
/// ```
///
/// Worlds render in the IR's own program order (never the footprint
/// table's `HashMap` iteration order, which is unspecified) so the report
/// is stable across runs. `--function` filters to one world, mirroring
/// `ir graph`'s flag and error shape exactly, including the unknown-world
/// message (docs/tmt/cli.md (tmt ir)).
fn ir_footprints(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let filter = args.value("--function")?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "ir footprints takes exactly one file\n\n{IR_USAGE}"
        ));
    };
    let text = fs::read_to_string(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let program = IrProgram::from_json(&text).map_err(|e| format!("{input}: {e}"))?;
    let table = crate::footprint::infer_ir(&program);

    let mut blocks: Vec<String> = Vec::new();
    for world in &program.worlds {
        if filter.as_deref().is_some_and(|f| f != world.name) {
            continue;
        }
        // `infer_ir` computes one entry per world `program.worlds` lists, so
        // this always resolves — a lookup miss would mean the table and the
        // program it was built from disagree on which worlds exist.
        let footprint = table
            .worlds
            .get(&world.name)
            .expect("infer_ir covers every world its own input program lists");
        let mut block = format!("world {}\n", world.name);
        for (index, tape) in world.tapes.iter().enumerate() {
            // `FootprintTable.worlds` is keyed by name, so two worlds
            // sharing a name collapse to the LAST one's arity — a shape
            // `--emit-ir` itself never produces (its names are mangled
            // unique), but the file this leaf reads is untrusted input, not
            // a value this process just built, so a hand-edited or
            // corrupted `.ir.json` can still name-collide two
            // differently-sized worlds. Defaulting to the empty set on a
            // miss keeps that case a (conservative) report line rather
            // than a panic.
            let set = footprint.tapes.get(index).copied().unwrap_or_default();
            let members: Vec<String> = set.iter().map(|i| i.to_string()).collect();
            block.push_str(&format!(
                "  tape {index} ({}): writes {{{}}} of {}\n",
                tape.name,
                members.join(", "),
                tape.cardinality
            ));
        }
        blocks.push(block);
    }
    if blocks.is_empty() {
        return Err(match filter {
            Some(f) => format!("no world `{f}` in {input}"),
            None => format!("{input}: no worlds"),
        });
    }
    Ok(CliOutput::ok(blocks.join("\n"), String::new()))
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
