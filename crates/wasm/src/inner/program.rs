//! The build channel (compile → link against the embedded stdlib) and the
//! Program it yields: the executable, its map, the line index over the
//! map, and the per-band tape layouts the renderer needs.

use mtc_core::formats::executable::Executable;
use mtc_core::linemap::LineIndex;
use mtc_core::linker::{LinkOptions, MapFile};

use super::Lang;
use super::diagnostics::{Diag, Severity, from_core, pm_fatal, tm_fatal};
use super::positions::Utf16Index;

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

/// Compile with debug info (the browser always wants the line table),
/// link against the embedded stdlib, and gather the compile channel's
/// warnings. `opt_level` is 0 or 1; anything else is treated as 1.
pub fn build(lang: Lang, source: &str, opt_level: u8) -> Result<(Program, Vec<Diag>), Diag> {
    let idx = Utf16Index::new(source);
    match lang {
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
            let linked = mtc_post_machine::asm::link(
                &[out.object],
                &[mtc_post_machine::stdlib::object().clone()],
                LinkOptions::default(),
            )
            .map_err(|e| link_diag(e.to_string()))?;
            let layouts = vec![TapeLayout {
                name: "tape".to_string(),
                glyphs: mtc_post_machine::arch::DEFAULT_GLYPHS
                    .iter()
                    .map(|g| g.to_string())
                    .collect(),
            }];
            Ok((
                Program::new(lang, linked.executable, linked.map, layouts),
                warnings,
            ))
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
            let linked = mtc_turing_machine::asm::link(
                &[out.object],
                &[mtc_turing_machine::stdlib::object().clone()],
                LinkOptions::default(),
            )
            .map_err(|e| link_diag(e.to_string()))?;
            let layouts = machine_tape_layout(source)
                .map_err(|e| tm_fatal(&idx, &e))?
                .unwrap_or_default()
                .into_iter()
                .map(|t| TapeLayout {
                    name: t.name,
                    glyphs: t.glyphs,
                })
                .collect();
            Ok((
                Program::new(lang, linked.executable, linked.map, layouts),
                warnings,
            ))
        }
    }
}

impl Program {
    fn new(lang: Lang, exe: Executable, map: MapFile, layouts: Vec<TapeLayout>) -> Program {
        let line_index = LineIndex::new(&map);
        Program {
            lang,
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
        match self.lang {
            Lang::Pmc => {
                mtc_post_machine::asm::disassemble_executable_with_map(&self.exe, &self.map)
            }
            Lang::Tmc => {
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
