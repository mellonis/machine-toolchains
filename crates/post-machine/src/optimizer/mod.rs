//! `-O1` pass driver. One module per pass; a pass is either per-function,
//! `fn(&mut IrFunction) -> u32` (PIPELINE), or program-level,
//! `fn(&mut IrProgram) -> u32` (PROGRAM_PIPELINE — currently `inline`).
//!
//! # The equivalence contract (internal — read before touching a pass)
//!
//! Every pass returns its change count and MUST preserve: the final tape
//! contents, the termination kind (`stp` / `hlt` / which trap), and every
//! branch decision that depends on the match flag. Two things are
//! explicitly excluded from this contract and MAY change: resource-limit
//! outcomes (inlining and tail-call change stack depth, so a
//! `StackOverflow` at `-O0` legally becomes a `StepLimit` trap at `-O1`
//! once a self-recursive tail call becomes an in-place loop — resource
//! traps are a quality-of-implementation outcome, not a semantic one),
//! and step counts/intermediate states — EXCEPT at an un-stripped `brk`,
//! which is an observability barrier: no motion or elimination may cross
//! it, so a debugger attached at `-O1` still sees honest state there.
//! (The user-facing summary of this same contract is docs/pmt/language.md
//! (optimization); this header is the binding version for pass authors —
//! it is a contract between passes, not with users, so it stays here.)
//!
//! A volatile program (docs/pmt/language.md (volatile programs)) generalizes the
//! `brk` barrier from a point to a standing, whole-run rule: every tape
//! access is externally observable, and the outside world may change the
//! cell under the head between accesses. No pass may assume a value
//! written to the tape reads back, and no pass may change the tape's
//! access sequence — no dropping idempotent or dead writes, no fusing
//! write+move shapes, no deciding a branch from a value the program only
//! wrote. Such programs compile in a second, GATED column that runs this
//! same pipeline with [`gated_pass_names`] disabled; the passes outside
//! that set keep running, so a volatile build is optimized, just not on
//! any assumption about what the tape will answer.
//!
//! Passes also MUST preserve the closed-terminator-targets invariant
//! (every terminator's target is a block id that still exists in the
//! function), checked in debug builds after every application.

use std::collections::HashSet;

use crate::ir::{IrFunction, IrProgram};

pub mod branch_fold;
pub mod cell_state;
pub mod check_fold;
pub mod dataflow;
pub mod dce;
pub mod fuse_tape_ops;
pub mod inline;
pub mod jump_threading;
pub mod move_elim;
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
/// tail-merge: return-chaining rewrites `Return` into `FallThrough`, which
/// would destroy tail-call's precondition (a trailing call in a `Return`
/// block) before it gets a chance to apply — this ordering constraint is
/// load-bearing, not a mere preference. Statically the two are a tie
/// (each drops one terminal byte); tail-call's decisive win is at
/// RUNTIME — no stack-slot growth and no return trip — whenever both
/// apply to the same block.
const PIPELINE: &[(&str, PassFn)] = &[
    ("check-fold", check_fold::run),
    ("jump-threading", jump_threading::run),
    ("cell-state", cell_state::run),
    ("branch-fold", branch_fold::run),
    ("tail-call", tail_call::run),
    ("tail-merge", tail_merge::run),
    ("dce", dce::run),
    ("move-elim", move_elim::run),
    ("fuse-tape-ops", fuse_tape_ops::run),
];

type ProgramPassFn = fn(&mut IrProgram) -> u32;

/// Program-level passes (cross-function), run at round start.
const PROGRAM_PIPELINE: &[(&str, ProgramPassFn)] = &[("inline", inline::run)];

const MAX_ROUNDS: u32 = 10;

/// The canonical `--fno-<pass>` / `--emit-ir=after:<pass>` names
/// (docs/pmt/language.md (optimization)), in pipeline order: the
/// program-level pass first, then the per-function pipeline. This is the
/// single source of truth other surfaces (shell-completion choices, the
/// drift guard that checks them) read from instead of retyping the list.
pub fn pass_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = PROGRAM_PIPELINE.iter().map(|(name, _)| *name).collect();
    names.extend(PIPELINE.iter().map(|(name, _)| *name));
    names
}

/// The passes a VOLATILE build disables, on top of whatever the user
/// already disabled (docs/pmt/optimizer.md (volatile builds)). Names come
/// from the same space as [`pass_names`] and reach the pipeline through
/// the ordinary `disabled` set, so the gate needs no second mechanism.
///
/// The dividing line is what a pass believes about the tape. The match
/// flag as a REGISTER — latched by an access the program actually
/// performed, then read again with no access in between — stays sound on
/// any tape, volatile or not. PREDICTING the flag (or the cell) from a
/// value the program merely wrote assumes the tape reads back what was
/// written, which a device does not owe anyone. The same line, drawn
/// around instructions rather than values: a pass may move jumps between
/// accesses, but may not drop, merge, split or reorder the accesses
/// themselves. Three passes cross it:
///
/// - `cell-state` — drops idempotent and dead writes. Both rules delete a
///   write the source asked for, and the idempotent rule additionally
///   reads its licence off a preceding write.
/// - `branch-fold` — decides a `check` from a known cell value. On a path
///   whose knowledge came from a `check` edge that is the latched flag and
///   sound; on a path whose knowledge came from a `wr` it is exactly the
///   write-read-back assumption. The gate is per PASS, not per path, so
///   the sound half is gated with the unsound one.
/// - `fuse-tape-ops` — folds `wr x` + move into `wrl`/`wrr`, which skips
///   the intermediate latch read of the written cell: two transactions
///   become one.
/// - `move-elim` — deletes an adjacent inverse move pair (`rgt; lft` or
///   `lft; rgt`). A move on a volatile band is itself an observable
///   access, so eliminating the pair drops two accesses — sound or not,
///   that is exactly what a volatile build must never do.
///
/// The remaining six only rewire control flow between accesses they leave
/// untouched (`check-fold`, `jump-threading`, `tail-call`, `tail-merge`,
/// `inline`) or delete code that never runs (`dce`), so they keep running
/// in the volatile column. Every verdict here, gated and clean alike, is
/// pinned by a test in tests/gated_passes.rs.
pub fn gated_pass_names() -> &'static [&'static str] {
    &["cell-state", "branch-fold", "fuse-tape-ops", "move-elim"]
}

/// Run the enabled pipeline to a change-fixpoint (round-capped). `-O0`
/// returns immediately: unoptimized output stays bit-identical to plain
/// codegen, with no optimizer artifact leaking in.
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
