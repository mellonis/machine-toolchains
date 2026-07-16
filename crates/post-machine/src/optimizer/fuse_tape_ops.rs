//! fuse-tape-ops: fuse an adjacent write-then-move pair into the single
//! fused instruction — `[wr i, lft]` → `wrl i`, `[wr i, rgt]` → `wrr i`.
//! Part of the `-O1` pipeline (optimizer/mod.rs); runs LAST so earlier
//! passes have already settled the op stream.
//!
//! Purely local and syntactic: for each block, scan `ops` left to right
//! and replace only IMMEDIATELY adjacent pairs, never looking past any
//! other op. The fused instruction inherits the WRITE's `line` (it maps
//! to the source line that wrote), latching MF from the landed cell — the
//! same observable a separate `wr` + move produces. A `brk` between the
//! write and the move breaks adjacency by construction, so the
//! observability-barrier contract holds trivially. Idempotent: a fused
//! `WrLft`/`WrRgt` never re-matches, so one linear scan reaches a
//! fixpoint.

use crate::ir::{IrFunction, IrOp};

pub fn run(f: &mut IrFunction) -> u32 {
    let mut fusions = 0u32;
    for b in &mut f.blocks {
        let ops = std::mem::take(&mut b.ops);
        let mut fused: Vec<IrOp> = Vec::with_capacity(ops.len());
        let mut i = 0usize;
        while i < ops.len() {
            if let IrOp::Wr { index, line } = ops[i]
                && i + 1 < ops.len()
            {
                match ops[i + 1] {
                    IrOp::Lft { .. } => {
                        fused.push(IrOp::WrLft { index, line });
                        fusions += 1;
                        i += 2;
                        continue;
                    }
                    IrOp::Rgt { .. } => {
                        fused.push(IrOp::WrRgt { index, line });
                        fusions += 1;
                        i += 2;
                        continue;
                    }
                    _ => {}
                }
            }
            fused.push(ops[i].clone());
            i += 1;
        }
        b.ops = fused;
    }
    fusions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrBlock, IrOp, IrTerm};

    fn block(ops: Vec<IrOp>) -> IrBlock {
        IrBlock {
            id: 0,
            labels: vec![],
            line: 1,
            ops,
            term: IrTerm::Return,
            term_line: 1,
        }
    }

    fn func(ops: Vec<IrOp>) -> IrFunction {
        IrFunction {
            name: "f".into(),
            line: 1,
            blocks: vec![block(ops)],
            local: false,
        }
    }

    #[test]
    fn fuses_adjacent_write_move_pairs() {
        // Distinct lines on the write and the move: the fused op MUST carry
        // the write's line, not the move's — a count-only check can't see
        // that, so assert the exact resulting ops.
        let mut f = func(vec![
            IrOp::Wr { index: 1, line: 5 },
            IrOp::Lft { line: 9 },
            IrOp::Wr { index: 0, line: 6 },
            IrOp::Rgt { line: 10 },
        ]);
        assert_eq!(run(&mut f), 2);
        assert_eq!(
            f.blocks[0].ops,
            vec![
                IrOp::WrLft { index: 1, line: 5 },
                IrOp::WrRgt { index: 0, line: 6 },
            ]
        );
        // Idempotent: a second scan re-fuses nothing.
        assert_eq!(run(&mut f), 0);
    }

    #[test]
    fn brk_between_blocks_fusion() {
        // A `brk` between the write and the move breaks adjacency: no fuse,
        // and the op stream is left exactly as-is.
        let ops = vec![
            IrOp::Wr { index: 1, line: 1 },
            IrOp::Brk { line: 1 },
            IrOp::Lft { line: 1 },
        ];
        let mut f = func(ops.clone());
        assert_eq!(run(&mut f), 0);
        assert_eq!(f.blocks[0].ops, ops);
    }

    #[test]
    fn lone_ops_untouched() {
        // A lone write, a lone move, and a move BEFORE a write (wrong order)
        // all stay put.
        for ops in [
            vec![IrOp::Wr { index: 1, line: 1 }],
            vec![IrOp::Lft { line: 1 }],
            vec![IrOp::Rgt { line: 1 }, IrOp::Wr { index: 1, line: 1 }],
        ] {
            let mut f = func(ops.clone());
            assert_eq!(run(&mut f), 0);
            assert_eq!(f.blocks[0].ops, ops);
        }
    }

    #[test]
    fn fused_ops_render_in_mermaid() {
        // Controller check: after fusing, the CFG mermaid carries the fused
        // mnemonic `wrl` (mirrors ir.rs's to_mermaid assertions).
        let mut f = func(vec![IrOp::Wr { index: 1, line: 1 }, IrOp::Lft { line: 1 }]);
        assert_eq!(run(&mut f), 1);
        let mermaid = f.to_mermaid();
        assert!(mermaid.contains("wrl 1"), "{mermaid}");
    }
}
