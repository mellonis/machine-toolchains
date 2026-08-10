//! Inspection subcommands: dis, tape, ir.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::formats::{ARCH_PM1, ContainerKind, parse_glyph_list, parse_glyph_sequence, sniff};
use mtc_core::linker::MapFile;
use mtc_core::vm::LoadError;

use crate::arch::DEFAULT_GLYPHS;
use crate::ir::IrProgram;

use super::{Args, CliOutput, Delimit, parse_keyed, render_tape};

const DIS_USAGE: &str = "\
USAGE: pmt dis FILE.pmo|FILE.pmx [--listing] [--map FILE.pmx.map]

Objects disassemble with real names from the symbol table. Executables
use the .pmx.map sidecar when present (FILE.pmx.map or --map), else
recursive-descent discovery (func_XXXX). --listing prints the debugger
code view: addresses + raw bytes, not reassembleable.
";

pub(super) fn dis(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
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
            if obj.arch != ARCH_PM1 {
                return Err(LoadError::UnknownArch(obj.arch).to_string());
            }
            Ok(CliOutput::ok(
                crate::asm::disassemble_object(&obj),
                String::new(),
            ))
        }
        Some(ContainerKind::Executable) => {
            let exe = Executable::from_bytes(&bytes).map_err(|e| e.to_string())?;
            if exe.arch != ARCH_PM1 {
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
            Err("that is a tape block — use `pmt tape-block show`".into())
        }
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
USAGE: pmt tape-block build \" * * *\" [--head N] [-o OUT.pmt]
       pmt tape-block new [--from APP.pmx] [-o OUT.pmt] [EDITS]
       pmt tape-block set IN.pmt (-o OUT.pmt | --in-place) [EDITS]
       pmt tape-block show FILE.pmt [--dense | --separated]

EDITS (repeatable; KEY is a tape index):
  --alphabet KEY=GLYPHS   repin the block's glyphs (relabels; same cardinality)
  --cells    KEY=GLYPHS   set tape KEY's cells
  --head     KEY=N        set tape KEY's head
  --origin   KEY=N        set tape KEY's origin

build: cell characters are the PM-1 glyphs (space = blank, * = mark); the
leftmost character is cell 0. GLYPHS is alphabet notation: ' ','*'.
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

/// Resolve a tape key to its band index. On PM-1 a key is always an index:
/// tape names are a source-language construct and `pmt` has no source
/// provenance path (docs/pmt/cli.md (tape-block)). The `names` parameter
/// keeps the signature identical to the TM twin's.
fn resolve_key(key: &str, _names: &[String], tape_count: usize) -> Result<usize, String> {
    let Ok(index) = key.parse::<usize>() else {
        return Err(format!("tape key `{key}`: expected a tape index"));
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
/// band count (docs/pmt/cli.md (tape-block)).
fn freehand_bands(edits: &Edits) -> Result<Vec<Vec<String>>, String> {
    let mut bands: Vec<(usize, Vec<String>)> = Vec::new();
    for (key, text) in &edits.alphabets {
        let index = key
            .parse::<usize>()
            .map_err(|_| format!("--alphabet `{key}`: expected a tape index"))?;
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

pub(super) fn tape_block(raw: &[String]) -> Result<CliOutput, String> {
    match raw.first().map(String::as_str) {
        Some("build") => tape_build(&raw[1..]),
        Some("new") => tape_new(&raw[1..]),
        Some("set") => tape_set(&raw[1..]),
        Some("show") => tape_show(&raw[1..]),
        _ => Ok(CliOutput::ok(TAPE_USAGE.into(), String::new())),
    }
}

fn tape_build(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(TAPE_USAGE.into(), String::new()));
    }
    let head: i64 = match args.value("--head")? {
        Some(text) => text.parse().map_err(|_| format!("bad --head `{text}`"))?,
        None => 0,
    };
    let out = args.value("-o")?.unwrap_or_else(|| "tape.pmt".into());
    let inputs = args.positionals()?;
    let [pattern] = inputs.as_slice() else {
        return Err(format!(
            "tape-block build takes exactly one pattern\n\n{TAPE_USAGE}"
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
            alphabet: None,
        }],
    };
    let bytes = block.to_bytes().map_err(|e| e.to_string())?;
    fs::write(&out, bytes).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

/// `pmt tape-block new [--from APP.pmx] [-o OUT.pmt] [EDITS]` — mint a block
/// and apply this invocation's edits in one call.
///
/// With `--from`, the band count comes from the image header. Without it, the
/// `--alphabet` keys size the block, or — given none — a single empty band.
/// PM-1's alphabet is fixed at two glyphs, so bands default to the arch pair
/// rather than to index labels (docs/pmt/cli.md (tape-block)).
fn tape_new(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(TAPE_USAGE.into(), String::new()));
    }
    let from = args.value("--from")?;
    let out = args.value("-o")?.unwrap_or_else(|| "blank.pmt".into());
    let edits = collect_edits(&mut args)?;
    let extra = args.positionals()?;
    if !extra.is_empty() {
        return Err(format!(
            "tape-block new takes no positional arguments\n\n{TAPE_USAGE}"
        ));
    }

    let defaults: Vec<String> = DEFAULT_GLYPHS.iter().map(|g| g.to_string()).collect();

    let band_glyphs: Vec<Vec<String>> = match from.as_deref() {
        Some(path) => {
            let bytes = fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
            match sniff(&bytes) {
                Some(ContainerKind::Executable) => {}
                _ => return Err(format!("{path}: not an executable image (.pmx)")),
            }
            let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))?;
            vec![defaults.clone(); usize::from(exe.tape_count).max(1)]
        }
        None if edits.alphabets.is_empty() => vec![defaults.clone()],
        None => freehand_bands(&edits)?,
    };

    // Single-alphabet by construction: the block table holds the glyphs and
    // no band overrides it, which is what keeps the file at MT v1
    // (docs/formats.md (tape-block snapshot)).
    let mut block = TapeBlockFile {
        alphabet: band_glyphs[0].clone(),
        tapes: band_glyphs
            .iter()
            .map(|_| TapeSnapshot {
                origin: 0,
                cells: Vec::new(),
                head: 0,
                alphabet: None,
            })
            .collect(),
    };

    apply_edits(&mut block, &edits, &[], true)?;

    let bytes = block.to_bytes().map_err(|e| format!("{out}: {e}"))?;
    fs::write(&out, bytes).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

/// `pmt tape-block set IN.pmt (-o OUT.pmt | --in-place) [EDITS]` — clone
/// semantics: read `IN.pmt`, apply this invocation's edits, and write the
/// result out. The source is never mutated; the output goes to `-o` or, with
/// `--in-place`, back over the input. Any subset of edits may be given; none
/// is a plain copy (docs/pmt/cli.md (tape-block)).
fn tape_set(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(TAPE_USAGE.into(), String::new()));
    }
    let out = args.value("-o")?;
    let in_place = args.flag("--in-place");
    let edits = collect_edits(&mut args)?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "tape-block set takes exactly one file\n\n{TAPE_USAGE}"
        ));
    };

    // Output destination: exactly one of -o / --in-place. Refusing the
    // neither case is what keeps `set` from silently clobbering IN.pmt.
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
                "tape-block set needs -o OUT.pmt or --in-place\n\n{TAPE_USAGE}"
            ));
        }
    };

    let bytes = fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let mut block = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{input}: {e}"))?;

    // No names on PM: a tape key is always an index.
    apply_edits(&mut block, &edits, &[], true)?;

    let bytes = block.to_bytes().map_err(|e| format!("{dest}: {e}"))?;
    fs::write(&dest, bytes).map_err(|e| format!("cannot write {dest}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

fn tape_show(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(TAPE_USAGE.into(), String::new()));
    }
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
        // Each band renders through its own effective alphabet (its override
        // if present, else the block fallback). PM-1's own tools never write
        // per-tape tables, but `.pmt` and `.tmt` are one container, so a block
        // authored elsewhere can carry them
        // (docs/formats.md (per-tape glyph tables)).
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

const IR_USAGE: &str = "\
USAGE: pmt ir graph FILE.ir.json|FILE.pmc [--function NAME]
                    [--variant normal|volatile] [-O0|-O1]

Renders --emit-ir output as a Mermaid flowchart (one per function). A
.pmc input is compiled in memory first: --variant picks which build
column's CFG is rendered (default normal) and -O0/-O1 the optimization
level (default -O0, as in `pmt compile`). Both flags need a .pmc input —
a .ir.json file already holds exactly one column.
";

pub(super) fn ir(raw: &[String]) -> Result<CliOutput, String> {
    match raw.first().map(String::as_str) {
        Some("graph") => ir_graph(&raw[1..]),
        _ => Ok(CliOutput::ok(IR_USAGE.into(), String::new())),
    }
}

/// Which build column `pmt ir graph` renders from a `.pmc` input
/// (docs/pmt/cli.md (pmt ir)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IrColumn {
    Normal,
    Volatile,
}

/// `--variant normal|volatile`, validated where it is read rather than
/// where it is used: a misspelled column then reports itself before a
/// missing input or an unreadable file can shadow it.
fn take_variant(args: &mut Args) -> Result<Option<IrColumn>, String> {
    let Some(raw) = args.value("--variant")? else {
        return Ok(None);
    };
    match raw.as_str() {
        "normal" => Ok(Some(IrColumn::Normal)),
        "volatile" => Ok(Some(IrColumn::Volatile)),
        other => Err(format!("unknown variant `{other}` (normal | volatile)")),
    }
}

/// Compiles a `.pmc` input for inspection and hands back the requested
/// column's CFG. Inspection is not the in-memory build path — nothing
/// here is linked, so there is no program bit to narrow the work with
/// and BOTH columns are built, whichever one is asked for
/// (docs/pmt/cli.md (pmt ir)).
fn compile_for_inspection(
    path: &Path,
    column: IrColumn,
    opt_level: crate::optimizer::OptLevel,
) -> Result<IrProgram, String> {
    let source =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let out = crate::compiler::compile(
        &source,
        crate::compiler::CompileOptions {
            opt_level,
            columns: crate::compiler::VariantColumns::Both,
            ..Default::default()
        },
    )
    .map_err(|e| {
        let mut stderr = String::new();
        super::lint::render_fatal(&mut stderr, path, e.span, &e.kind, e.kind.code());
        stderr.trim_end().to_string()
    })?;
    Ok(match column {
        IrColumn::Normal => out.ir,
        IrColumn::Volatile => out
            .ir_volatile
            .expect("a both-columns compile builds the volatile column"),
    })
}

fn ir_graph(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(IR_USAGE.into(), String::new()));
    }
    let filter = args.value("--function")?;
    let variant = take_variant(&mut args)?;
    // -O0 then -O1, exactly as `pmt compile` resolves them: the later
    // check wins when both are written.
    let o0 = args.flag("-O0");
    let o1 = args.flag("-O1");
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!("ir graph takes exactly one file\n\n{IR_USAGE}"));
    };
    let program = if input.ends_with(".pmc") {
        let mut opt_level = crate::optimizer::OptLevel::O0;
        if o0 {
            opt_level = crate::optimizer::OptLevel::O0;
        }
        if o1 {
            opt_level = crate::optimizer::OptLevel::O1;
        }
        compile_for_inspection(
            Path::new(input),
            variant.unwrap_or(IrColumn::Normal),
            opt_level,
        )?
    } else {
        // A rendered artifact carries one column at one optimization
        // level already; silently ignoring a flag that cannot apply
        // would be worse than saying so.
        if variant.is_some() || o0 || o1 {
            return Err(format!(
                "--variant and -O0/-O1 apply to a .pmc input; {input} already holds one column\n\n{IR_USAGE}"
            ));
        }
        let text = fs::read_to_string(input).map_err(|e| format!("cannot read {input}: {e}"))?;
        IrProgram::from_json(&text).map_err(|e| format!("{input}: {e}"))?
    };
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
