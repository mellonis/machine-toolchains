//! The build channel (compile or assemble → link against the embedded
//! stdlib) and the Program it yields: the executable, its map, the line
//! index over the map, and the per-band tape layouts the renderer needs.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::linemap::LineIndex;
use mtc_core::linker::{LinkOptions, MapFile};

use super::diagnostics::{Diag, Severity, asm_fatal, from_core, pm_fatal, tm_fatal};
use super::positions::Utf16Index;
use super::{Arch, Lang};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeLayout {
    pub name: String,
    pub glyphs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLoc {
    pub function: String,
    pub line: Option<u32>,
}

#[derive(Debug)]
pub struct Program {
    pub lang: Lang,
    pub arch: Arch,
    pub exe: Executable,
    pub map: MapFile,
    line_index: LineIndex,
    layouts: Vec<TapeLayout>,
}

/// A link failure has no source span; it is reported at the text start.
fn link_diag(message: String) -> Diag {
    Diag {
        code: "link".to_string(),
        severity: Severity::Error,
        from: 0,
        to: 0,
        message,
        fix: None,
    }
}

/// PM-1's fixed pair: one binary band, blank and mark.
fn pm_layouts() -> Vec<TapeLayout> {
    vec![TapeLayout {
        name: "tape".to_string(),
        glyphs: mtc_post_machine::arch::DEFAULT_GLYPHS
            .iter()
            .map(|g| g.to_string())
            .collect(),
    }]
}

/// A TM-1 image carries cardinalities and nothing else, so each band is
/// labelled with the decimal strings `0..card-1` — the convention
/// `tmt tape-block new --from app.tmx` uses (`docs/formats.md (glyph
/// tables)`).
fn image_layouts(exe: &Executable) -> Vec<TapeLayout> {
    let cards: Vec<u32> = if exe.alphabet_cardinalities.is_empty() {
        vec![2; usize::from(exe.tape_count).max(1)]
    } else {
        exe.alphabet_cardinalities.clone()
    };
    cards
        .iter()
        .enumerate()
        .map(|(i, &c)| TapeLayout {
            name: format!("tape{i}"),
            glyphs: (0..c).map(|s| s.to_string()).collect(),
        })
        .collect()
}

/// Link one unit against the embedded stdlib.
fn link(arch: Arch, object: ObjectFile) -> Result<mtc_core::linker::LinkOutput, Diag> {
    match arch {
        Arch::Pm1 => mtc_post_machine::asm::link(
            &[object],
            &[mtc_post_machine::stdlib::object().clone()],
            LinkOptions::default(),
        ),
        Arch::Tm1 => mtc_turing_machine::asm::link(
            &[object],
            &[mtc_turing_machine::stdlib::object().clone()],
            LinkOptions::default(),
        ),
    }
    .map_err(|e| link_diag(e.to_string()))
}

/// Compile — or, for `.pma`/`.tma`, assemble — with debug info (the
/// browser always wants the line table), link against the embedded
/// stdlib, and gather the compile channel's warnings. `opt_level` is 0 or
/// 1; anything else is treated as 1. The assembler has no optimizer, so
/// on an assembly language it is accepted and ignored.
pub fn build(lang: Lang, source: &str, opt_level: u8) -> Result<(Program, Vec<Diag>), Diag> {
    let idx = Utf16Index::new(source);
    let (object, warnings, layouts): (ObjectFile, Vec<Diag>, Option<Vec<TapeLayout>>) = match lang {
        Lang::Pmc => {
            use mtc_post_machine::compiler::{CompileOptions, compile};
            use mtc_post_machine::optimizer::OptLevel;
            let options = CompileOptions {
                opt_level: if opt_level == 0 {
                    OptLevel::O0
                } else {
                    OptLevel::O1
                },
                debug_info: true,
                ..Default::default()
            };
            let out = compile(source, options).map_err(|e| pm_fatal(&idx, &e))?;
            let warnings = out
                .report
                .diagnostics
                .iter()
                .map(|d| from_core(&idx, d, Severity::Warning))
                .collect();
            (out.object, warnings, Some(pm_layouts()))
        }
        Lang::Tmc => {
            use mtc_turing_machine::compiler::{CompileOptions, compile, machine_tape_layout};
            use mtc_turing_machine::optimizer::OptLevel;
            let options = CompileOptions {
                opt_level: if opt_level == 0 {
                    OptLevel::O0
                } else {
                    OptLevel::O1
                },
                debug_info: true,
                ..Default::default()
            };
            let out = compile(source, options).map_err(|e| tm_fatal(&idx, &e))?;
            let warnings = out
                .report
                .diagnostics
                .iter()
                .map(|d| from_core(&idx, d, Severity::Warning))
                .collect();
            let layouts = machine_tape_layout(source)
                .map_err(|e| tm_fatal(&idx, &e))?
                .map(|tapes| {
                    tapes
                        .into_iter()
                        .map(|t| TapeLayout {
                            name: t.name,
                            glyphs: t.glyphs,
                        })
                        .collect()
                });
            (out.object, warnings, layouts)
        }
        Lang::Pma => {
            let object =
                mtc_post_machine::asm::assemble(source, true).map_err(|e| asm_fatal(&idx, &e))?;
            (object, Vec::new(), Some(pm_layouts()))
        }
        Lang::Tma => {
            let object =
                mtc_turing_machine::asm::assemble(source, true).map_err(|e| asm_fatal(&idx, &e))?;
            (object, Vec::new(), None)
        }
    };
    let linked = link(lang.arch(), object)?;
    let layouts = layouts.unwrap_or_else(|| image_layouts(&linked.executable));
    Ok((
        Program::new(lang, linked.executable, linked.map, layouts),
        warnings,
    ))
}

impl Program {
    fn new(lang: Lang, exe: Executable, map: MapFile, layouts: Vec<TapeLayout>) -> Program {
        let line_index = LineIndex::new(&map);
        Program {
            lang,
            arch: lang.arch(),
            exe,
            map,
            line_index,
            layouts,
        }
    }

    pub fn tapes(&self) -> &[TapeLayout] {
        &self.layouts
    }

    pub fn line_of(&self, addr: u32) -> Option<SourceLoc> {
        self.line_index.resolve(addr).map(|loc| SourceLoc {
            function: loc.function.to_string(),
            line: loc.line,
        })
    }

    /// Where a breakpoint on `line` lands. A single source in the browser,
    /// so the per-file filter is not used.
    pub fn address_for_line(&self, line: u32) -> Option<u32> {
        self.line_index.address_for_line(line, None)
    }

    /// The reassembleable `.pma`/`.tma` text, names from the map.
    pub fn disassembly(&self) -> String {
        match self.arch {
            Arch::Pm1 => {
                mtc_post_machine::asm::disassemble_executable_with_map(&self.exe, &self.map)
            }
            Arch::Tm1 => {
                mtc_turing_machine::asm::disassemble_executable_with_map(&self.exe, &self.map)
            }
        }
    }

    /// The MX image, as `pmt build -o` would write it.
    pub fn bytes(&self) -> Vec<u8> {
        self.exe.to_bytes()
    }

    /// The `.map` sidecar text.
    pub fn map_json(&self) -> String {
        self.map.to_json()
    }
}
