//! `-O1` pass driver (spec §8). One module per pass; a pass is either
//! per-function, `fn(&mut IrFunction) -> u32` (PIPELINE), or program-level,
//! `fn(&mut IrProgram) -> u32` (PROGRAM_PIPELINE — currently `inline`).
//! Every pass returns its change count and MUST preserve the equivalence
//! contract and the closed-terminator-targets invariant (checked in debug
//! builds after every application).

use std::collections::HashSet;

use crate::ir::{IrFunction, IrProgram};

pub mod branch_fold;
pub mod cell_state;
pub mod check_fold;
pub mod dataflow;
pub mod dce;
pub mod inline;
pub mod jump_threading;
pub mod tail_call;
pub mod tail_merge;

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

/// Fixed pipeline, in per-round application order. tail-call runs BEFORE
/// tail-merge: return-chaining rewrites `Return` into `FallThrough` and
/// would otherwise destroy the tail-call precondition (Task-6 finding) —
/// this ordering constraint is load-bearing, not a mere preference.
/// Statically the two are a tie (each drops one terminal byte); tail-call's
/// decisive win is at RUNTIME — no stack-slot growth and no return trip —
/// whenever both apply to the same block.
const PIPELINE: &[(&str, PassFn)] = &[
    ("check-fold", check_fold::run),
    ("jump-threading", jump_threading::run),
    ("cell-state", cell_state::run),
    ("branch-fold", branch_fold::run),
    ("tail-call", tail_call::run),
    ("tail-merge", tail_merge::run),
    ("dce", dce::run),
];

type ProgramPassFn = fn(&mut IrProgram) -> u32;

/// Program-level passes (cross-function), run at round start.
const PROGRAM_PIPELINE: &[(&str, ProgramPassFn)] = &[("inline", inline::run)];

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
        for (name, pass) in PROGRAM_PIPELINE {
            if options.disabled.contains(*name) {
                continue;
            }
            let n = pass(ir);
            #[cfg(debug_assertions)]
            for f in &ir.functions {
                if let Err(e) = crate::ir::validate_function(f) {
                    panic!("pass `{name}` broke IR invariants: {e}");
                }
            }
            if n > 0 {
                report.changes.push(PassChange {
                    pass: name,
                    function: "(module)".to_string(),
                    changes: n,
                });
                if options.capture {
                    snapshots.push((format!("after:{name}"), ir.clone()));
                }
            }
            round_changes += n;
        }
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
