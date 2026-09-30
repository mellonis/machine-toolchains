//! The IR channel: the document `--emit-ir` writes for a source language,
//! and the Mermaid graph `ir graph` draws of it, one entry per world or
//! function (`docs/wasm.md (the IR and its graph)`). Assembly has no IR.

use super::Lang;
use super::diagnostics::{Diag, pm_fatal, tm_fatal};
use super::positions::Utf16Index;

/// Which IR stage to hand back — `--emit-ir=lowered` or `--emit-ir=final`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IrStage {
    /// The graph straight from the source, before any optimizer pass.
    #[default]
    Lowered,
    /// What codegen received: the optimizer's output at `-O1`, the lowered
    /// graph again at `-O0`.
    Final,
}

impl IrStage {
    pub fn parse(s: &str) -> Option<IrStage> {
        match s {
            "lowered" => Some(IrStage::Lowered),
            "final" => Some(IrStage::Final),
            _ => None,
        }
    }
}

/// Which view `ir graph` draws — the CLI's merged default, `--shape`, or
/// `--raw` (docs/formats.md (graph label notation)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphView {
    #[default]
    Merged,
    Shape,
    Raw,
}

impl GraphView {
    pub fn parse(s: &str) -> Option<GraphView> {
        match s {
            "merged" => Some(GraphView::Merged),
            "shape" => Some(GraphView::Shape),
            "raw" => Some(GraphView::Raw),
            _ => None,
        }
    }
}

/// Why no IR came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrError {
    /// `.pma`/`.tma`: the assembler has no IR.
    NoIr(Lang),
    /// The source does not compile; the fatal diagnostic `check` reports.
    Compile(Diag),
}

/// One program's IR at `stage`, in either crate's own type.
enum Document {
    Pm(mtc_post_machine::ir::IrProgram),
    Tm(mtc_turing_machine::ir::IrProgram),
}

/// Compile `source` the way `compile --emit-ir=STAGE` does and keep the
/// requested stage. `opt_level` is 0 or 1; anything else is treated as 1,
/// as `build` treats it. The `.pmc` document is the normal build column,
/// exactly as `--emit-ir` writes it for a volatile program too.
fn document(lang: Lang, source: &str, opt_level: u8, stage: IrStage) -> Result<Document, IrError> {
    let idx = Utf16Index::new(source);
    let lowered = stage == IrStage::Lowered;
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
                capture_ir: lowered,
                ..Default::default()
            };
            let out = compile(source, options).map_err(|e| IrError::Compile(pm_fatal(&idx, &e)))?;
            Ok(Document::Pm(if lowered {
                stage_of(out.ir_snapshots)
            } else {
                out.ir
            }))
        }
        Lang::Tmc => {
            use mtc_turing_machine::compiler::{CompileOptions, compile};
            use mtc_turing_machine::optimizer::OptLevel;
            let options = CompileOptions {
                opt_level: if opt_level == 0 {
                    OptLevel::O0
                } else {
                    OptLevel::O1
                },
                capture_ir: lowered,
                ..Default::default()
            };
            let out = compile(source, options).map_err(|e| IrError::Compile(tm_fatal(&idx, &e)))?;
            Ok(Document::Tm(if lowered {
                stage_of(out.ir_snapshots)
            } else {
                out.ir
            }))
        }
        Lang::Pma | Lang::Tma => Err(IrError::NoIr(lang)),
    }
}

/// The `lowered` snapshot a capturing compile always takes first.
fn stage_of<P>(snapshots: Vec<(String, P)>) -> P {
    snapshots
        .into_iter()
        .find(|(label, _)| label == "lowered")
        .map(|(_, program)| program)
        .expect("a capturing compile records the lowered stage")
}

/// The IR JSON document, byte for byte what `compile --emit-ir=STAGE`
/// writes for the same source and optimization level.
pub fn ir(lang: Lang, source: &str, opt_level: u8, stage: IrStage) -> Result<String, IrError> {
    Ok(match document(lang, source, opt_level, stage)? {
        Document::Pm(p) => p.to_json(),
        Document::Tm(p) => p.to_json(),
    })
}

/// One graph per world (`.tmc`) or function (`.pmc`), in document order:
/// its name and the flowchart `ir graph` prints under that name's `%%`
/// header.
pub fn ir_graph(
    lang: Lang,
    source: &str,
    opt_level: u8,
    stage: IrStage,
    view: GraphView,
) -> Result<Vec<(String, String)>, IrError> {
    Ok(match document(lang, source, opt_level, stage)? {
        Document::Pm(p) => {
            use mtc_post_machine::ir_graph::GraphView as V;
            let v = match view {
                GraphView::Merged => V::Merged,
                GraphView::Shape => V::Shape,
                GraphView::Raw => V::Raw,
            };
            p.functions
                .iter()
                .map(|f| (f.name.clone(), f.to_mermaid_view(v)))
                .collect()
        }
        Document::Tm(p) => {
            use mtc_turing_machine::ir_graph::GraphView as V;
            let v = match view {
                GraphView::Merged => V::Merged,
                GraphView::Shape => V::Shape,
                GraphView::Raw => V::Raw,
            };
            p.worlds
                .iter()
                .map(|w| (w.name.clone(), w.to_mermaid_view(v)))
                .collect()
        }
    })
}
