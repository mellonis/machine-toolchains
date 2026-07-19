//! `-O1` pass driver for the TM IR — the phase-6a STUB.
//!
//! Phase 6a ships the `.tmc` front end and the `-O0` canonical codegen; the
//! optimizer passes (`inline` / `jump_threading` / `tail_call` / `tail_merge`
//! / `dce` / `dead_rows` / `dispatch_select`, plus `outline`) land in phase
//! 6b. Until then this module is the empty scaffold the pipeline plugs into:
//! [`pass_names`] returns nothing, and [`optimize`] runs a zero-pass fixpoint,
//! so **`-O1` is byte-identical to `-O0`** — the compiler wires the level
//! through, the CLI accepts `-O0`/`-O1`/`--fno-<pass>`/`--emit-ir[=STAGE]`,
//! and no optimizer artifact leaks. The registry emptiness is load-bearing:
//! an unknown `--fno-<pass>` name checks against this (empty) set, and the
//! only `--emit-ir` stages that match are `lowered` / `final` (no
//! `after:<pass>` yet).
//!
//! The public shape mirrors the PM-1 optimizer (`OptLevel` / `OptOptions` /
//! `OptReport` / `pass_names` / `optimize`) so phase 6b can grow passes into
//! it exactly as the PM-1 crate does, without touching `compile()`.

use std::collections::HashSet;

use crate::ir::IrProgram;

/// The optimization level a compile runs at. `-O0` (default) is plain
/// codegen; `-O1` runs the pass pipeline — empty in 6a, so identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    #[default]
    O0,
    O1,
}

/// Optimizer inputs threaded from `CompileOptions`.
#[derive(Debug, Clone, Default)]
pub struct OptOptions {
    pub level: OptLevel,
    /// Pass names to skip (`--fno-<pass>`). Checked against [`pass_names`];
    /// an unknown name is a CLI error (phase 8 wiring), never silent.
    pub disabled: HashSet<String>,
    /// Capture an IR snapshot after each pass that changed something
    /// (`--emit-ir`). No pass changes anything in 6a, so only the pipeline's
    /// `lowered` / `final` bookends are ever captured (by `compile()`).
    pub capture: bool,
}

/// One pass's effect on one world in one round (`tmt -v` material). No pass
/// emits one in 6a; the type exists so 6b passes report exactly as PM-1's do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassChange {
    pub pass: &'static str,
    pub world: String,
    pub changes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OptReport {
    pub rounds: u32,
    pub changes: Vec<PassChange>,
}

const MAX_ROUNDS: u32 = 10;

/// The canonical `--fno-<pass>` / `--emit-ir=after:<pass>` names, in pipeline
/// order — **empty in 6a** (no passes yet). The single source of truth other
/// surfaces (shell completion, the drift guard) read instead of retyping.
pub fn pass_names() -> Vec<&'static str> {
    Vec::new()
}

/// Run the enabled pipeline to a change-fixpoint (round-capped). `-O0`
/// returns immediately (`rounds == 0`); `-O1` runs one no-op round and
/// converges (`rounds == 1`, no changes) — byte-identical output either way
/// until 6b adds passes. `snapshots` is left untouched here; `compile()`
/// pushes the `lowered` / `final` bookends around this call.
pub fn optimize(
    ir: &mut IrProgram,
    options: &OptOptions,
    snapshots: &mut Vec<(String, IrProgram)>,
) -> OptReport {
    // `ir` and `snapshots` are the 6b growth points; touching neither keeps
    // `-O1` a pure identity today.
    let _ = (&*ir, &*snapshots);
    let mut report = OptReport::default();
    if options.level == OptLevel::O0 {
        return report;
    }
    loop {
        report.rounds += 1;
        let round_changes = 0u32; // no passes registered in 6a
        if round_changes == 0 || report.rounds >= MAX_ROUNDS {
            return report;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_registry_is_empty_in_6a() {
        assert!(pass_names().is_empty());
    }

    #[test]
    fn o0_is_an_immediate_no_op() {
        let mut ir = IrProgram {
            version: crate::ir::TM_IR_VERSION,
            worlds: Vec::new(),
            entry_world: None,
        };
        let mut snaps = Vec::new();
        let report = optimize(&mut ir, &OptOptions::default(), &mut snaps);
        assert_eq!(report.rounds, 0);
        assert!(report.changes.is_empty());
        assert!(snaps.is_empty());
    }

    #[test]
    fn o1_converges_in_one_round_with_no_changes() {
        let mut ir = IrProgram {
            version: crate::ir::TM_IR_VERSION,
            worlds: Vec::new(),
            entry_world: None,
        };
        let mut snaps = Vec::new();
        let report = optimize(
            &mut ir,
            &OptOptions {
                level: OptLevel::O1,
                ..Default::default()
            },
            &mut snaps,
        );
        assert_eq!(report.rounds, 1);
        assert!(report.changes.is_empty());
    }
}
