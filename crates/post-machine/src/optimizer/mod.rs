//! `-O1` pass driver (spec §8). One module per pass; each pass is
//! `fn(&mut IrFunction) -> u32` returning its change count and MUST
//! preserve the equivalence contract and the closed-terminator-targets
//! invariant (checked in debug builds after every application).

use std::collections::HashSet;

use crate::ir::{IrFunction, IrProgram};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    #[default]
    O0,
    O1,
}

#[derive(Debug, Clone, Default)]
pub struct OptOptions {
    pub level: OptLevel,
    /// Pass names to skip (`--fno-<pass>`).
    pub disabled: HashSet<String>,
    /// Capture an IR snapshot after each pass that changed something.
    pub capture: bool,
}

/// One pass's effect on one function in one round (`pmt -v` material).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassChange {
    pub pass: &'static str,
    pub function: String,
    pub changes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OptReport {
    pub rounds: u32,
    pub changes: Vec<PassChange>,
}

type PassFn = fn(&mut IrFunction) -> u32;

/// Fixed pipeline, in per-round application order. Tasks 2-5 extend it.
const PIPELINE: &[(&str, PassFn)] = &[];

const MAX_ROUNDS: u32 = 10;

/// Run the enabled pipeline to a change-fixpoint (round-capped). `-O0`
/// returns immediately: Plan 5 output stays bit-identical.
pub fn optimize(
    ir: &mut IrProgram,
    options: &OptOptions,
    snapshots: &mut Vec<(String, IrProgram)>,
) -> OptReport {
    let mut report = OptReport::default();
    if options.level == OptLevel::O0 {
        return report;
    }
    loop {
        report.rounds += 1;
        let mut round_changes = 0u32;
        for (name, pass) in PIPELINE {
            if options.disabled.contains(*name) {
                continue;
            }
            let mut pass_total = 0u32;
            for f in &mut ir.functions {
                let n = pass(f);
                #[cfg(debug_assertions)]
                if let Err(e) = crate::ir::validate_function(f) {
                    panic!("pass `{name}` broke IR invariants: {e}");
                }
                if n > 0 {
                    report.changes.push(PassChange {
                        pass: name,
                        function: f.name.clone(),
                        changes: n,
                    });
                }
                pass_total += n;
            }
            if options.capture && pass_total > 0 {
                snapshots.push((format!("after:{name}"), ir.clone()));
            }
            round_changes += pass_total;
        }
        if round_changes == 0 || report.rounds >= MAX_ROUNDS {
            return report;
        }
    }
}
