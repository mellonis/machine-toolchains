//! The Mermaid renderings of one IR function (`pmt ir graph`): the RAW
//! view, one edge per successor exactly as the CFG records it; the MERGED
//! view (the default); and the SHAPE view, one edge per pair of blocks
//! labelled with the number of raw edges it stands for
//! (docs/pmt/cli.md (pmt ir)).
//!
//! A block has one terminator, so the only parallel edges a CFG can carry
//! are a `check` whose two arms land on the same block. The merged view
//! draws that pair as one edge labelled `{MF,!MF}` — the two readings of
//! the match flag as a set, in the notation of docs/formats.md (graph
//! label notation) — and is otherwise identical to the raw view: there is
//! no vector here to compress.

use std::fmt::Write as _;

use crate::ir::{IrFunction, IrOp, IrTerm};

/// Which rendering `ir graph` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphView {
    /// Parallel edges merged: a `check` whose arms agree draws one edge.
    #[default]
    Merged,
    /// One edge per (source, target) pair, labelled with its edge count.
    Shape,
    /// One edge per successor, exactly as the IR lists them.
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

/// Render `function` in `view`. Node text: source labels, then ops, then a
/// terminal marker for block-ending terminators; edges carry the
/// check/goto semantics.
pub fn render(function: &IrFunction, view: GraphView) -> String {
    let mut out = String::from("flowchart TD\n");
    for block in &function.blocks {
        let mut lines: Vec<String> = Vec::new();
        for &label in &block.labels {
            lines.push(format!("{label}:"));
        }
        for op in &block.ops {
            lines.push(match op {
                IrOp::Lft { .. } => "lft".into(),
                IrOp::Rgt { .. } => "rgt".into(),
                IrOp::Wr { index, .. } => format!("wr {index}"),
                IrOp::WrLft { index, .. } => format!("wrl {index}"),
                IrOp::WrRgt { index, .. } => format!("wrr {index}"),
                IrOp::Brk { .. } => "brk".into(),
                IrOp::Call { name, .. } => format!("call @{name}"),
            });
        }
        match &block.term {
            IrTerm::Return => lines.push("ret".into()),
            IrTerm::Halt => lines.push("hlt".into()),
            IrTerm::TailCall { name } => lines.push(format!("jmp @{name}")),
            IrTerm::FallThrough { .. } | IrTerm::Goto { .. } | IrTerm::Check { .. } => {}
        }
        if lines.is_empty() {
            lines.push("(empty)".into());
        }
        let _ = writeln!(out, "    B{}[\"{}\"]", block.id, lines.join("<br/>"));
    }
    for block in &function.blocks {
        let id = block.id;
        match (&block.term, view) {
            (IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. }, _) => {}
            (IrTerm::FallThrough { to } | IrTerm::Goto { to }, GraphView::Shape) => {
                edge(&mut out, id, *to, Some(("1", true)));
            }
            (IrTerm::Check { marked, blank }, GraphView::Shape) => {
                if marked == blank {
                    edge(&mut out, id, *marked, Some(("2", true)));
                } else {
                    edge(&mut out, id, *marked, Some(("1", true)));
                    edge(&mut out, id, *blank, Some(("1", true)));
                }
            }
            (IrTerm::Check { marked, blank }, GraphView::Merged) if marked == blank => {
                edge(&mut out, id, *marked, Some(("{MF,!MF}", true)));
            }
            (IrTerm::FallThrough { to }, _) => edge(&mut out, id, *to, None),
            (IrTerm::Goto { to }, _) => edge(&mut out, id, *to, Some(("goto", false))),
            (IrTerm::Check { marked, blank }, _) => {
                edge(&mut out, id, *marked, Some(("MF", false)));
                edge(&mut out, id, *blank, Some(("!MF", false)));
            }
        }
    }
    out
}

/// One edge line: unlabelled, or labelled — quoted or bare — with the
/// label text through the shared escaping every `ir graph` edge label
/// takes (docs/formats.md (graph label notation)).
fn edge(out: &mut String, from: u32, to: u32, label: Option<(&str, bool)>) {
    let _ = match label {
        None => writeln!(out, "    B{from} --> B{to}"),
        Some((text, quoted)) => {
            let text = mtc_core::mermaid::edge_label(text);
            if quoted {
                writeln!(out, "    B{from} -->|\"{text}\"| B{to}")
            } else {
                writeln!(out, "    B{from} -->|{text}| B{to}")
            }
        }
    };
}
