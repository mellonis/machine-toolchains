//! `-O1` pass driver for the TM IR. One module per pass; a pass is either
//! per-world, `fn(&mut IrWorld) -> u32` (PIPELINE), or program-level,
//! `fn(&mut IrProgram) -> u32` (PROGRAM_PIPELINE — cross-world, run at round
//! start). Each pass returns its change count; the driver loops the pipelines
//! to a change-fixpoint, round-capped at [`MAX_ROUNDS`].
//!
//! The pipelines are the growth points: both consts are **empty here** and
//! each optimizer pass registers itself into one of them as it lands. The
//! driver — the fixpoint loop, the disabled-pass and default-off gating, the
//! per-pass invariant re-check, the snapshot capture, and [`pass_names`] — is
//! complete and does not change as passes join. Until a pass registers,
//! `-O1` runs one empty round and converges, so **`-O1` output is
//! byte-identical to `-O0`** (the do-no-harm floor the compiler locks with a
//! byte comparison).
//!
//! # The equivalence contract (internal — read before touching a pass)
//!
//! Every pass returns its change count and MUST preserve: the final tape
//! contents of every tape, the termination kind (`stp` / `hlt` / which trap
//! KIND), and every dispatch decision that depends on the match register.
//! Two things are explicitly excluded and MAY change: resource-limit
//! outcomes (inlining and tail-calling change the frame-stack depth, so a
//! stack-overflow trap at `-O0` may legally become a step-limit trap at
//! `-O1`), and step counts / intermediate states — EXCEPT across an
//! un-stripped `brk`, which is an observability barrier: no motion or
//! elimination may cross it, so a debugger attached at `-O1` still sees
//! honest state there. The optimizer runs BEFORE codegen strips `brk`, so
//! the barrier always holds when a debugger is attached.
//!
//! # Invariant re-check
//!
//! Codegen relies on the world invariants (dense ids `id == index`, in-bounds
//! indices, arity-wide rows, traps only on synthesized rows, every `Goto`
//! target an existing id). In debug builds the driver re-runs
//! [`crate::ir::validate_world`] after EVERY pass (not only ones that reported
//! a change), so a pass that renumbers or retargets incorrectly fails loudly
//! at the pass that broke it rather than deep in codegen.
//!
//! The public shape mirrors the PM-1 optimizer (`OptLevel` / `OptOptions` /
//! `OptReport` / `pass_names` / `optimize`), with `world` where PM-1 has
//! `function`, so the two crates grow passes into their drivers the same way.

use std::collections::HashSet;

use crate::ir::{IrProgram, IrWorld};

/// The optimization level a compile runs at. `-O0` (default) is plain
/// codegen; `-O1` runs the pass pipeline.
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
    /// Pass names to skip (`--fno-<pass>`). Checked against [`pass_names`]; an
    /// unknown name is a CLI error, never silent.
    pub disabled: HashSet<String>,
    /// Capture an IR snapshot after each pass that changed something
    /// (`--emit-ir`), labelled `after:<pass>`. `compile()` adds the pipeline
    /// bookends `lowered` / `final` around this call.
    pub capture: bool,
    /// Enable the default-OFF `outline` pass (`--foutline`). Every other pass
    /// is default-ON; `outline` is the inverse of `inline` (it hoists shared
    /// subgraphs into a routine rather than splicing), so it runs only when
    /// the caller opts in — otherwise it would fight `inline` for a fixpoint.
    pub outline: bool,
}

/// One pass's effect on one world in one round (`tmt -v` material).
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

type PassFn = fn(&mut IrWorld) -> u32;

/// Per-world passes, in per-round application order. Empty until the passes
/// land; each registers itself here as it ships. The order carries a
/// load-bearing constraint once populated — `tail_call` must precede
/// `tail_merge` (return-chaining would otherwise destroy tail-call's
/// precondition before it can apply).
const PIPELINE: &[(&str, PassFn)] = &[];

type ProgramPassFn = fn(&mut IrProgram) -> u32;

/// Program-level passes (cross-world), run at round start. Empty until the
/// passes land. `inline` splices small callees into their call sites;
/// `outline` (default-OFF, gated by [`OptOptions::outline`]) is its inverse.
const PROGRAM_PIPELINE: &[(&str, ProgramPassFn)] = &[];

const MAX_ROUNDS: u32 = 10;

/// The canonical `--fno-<pass>` / `--emit-ir=after:<pass>` names, in pipeline
/// order: the program-level passes first, then the per-world pipeline. The
/// single source of truth other surfaces (shell completion, the drift guard)
/// read instead of retyping the list. Empty until the passes register.
pub fn pass_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = PROGRAM_PIPELINE.iter().map(|(name, _)| *name).collect();
    names.extend(PIPELINE.iter().map(|(name, _)| *name));
    names
}

/// Whether a program-level pass should run under `options`: skipped when
/// `--fno-<pass>` names it, and skipped when it is default-OFF and not
/// explicitly enabled (`outline` without `--foutline`).
fn program_pass_enabled(name: &str, options: &OptOptions) -> bool {
    if options.disabled.contains(name) {
        return false;
    }
    // `outline` is the one default-OFF program pass: it runs only when the
    // caller opts in via `--foutline`. Every other pass is default-ON.
    if name == "outline" && !options.outline {
        return false;
    }
    true
}

/// Run the enabled pipeline to a change-fixpoint (round-capped). `-O0`
/// returns immediately: unoptimized output stays bit-identical to plain
/// codegen, with no optimizer artifact leaking in. `snapshots` receives an
/// `after:<pass>` entry for each changed pass when `options.capture` is set;
/// `compile()` brackets the whole run with `lowered` / `final`.
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
            if !program_pass_enabled(name, options) {
                continue;
            }
            let n = pass(ir);
            // Re-check the world invariants after EVERY pass, not only when it
            // reported a change: a pass that returns 0 but still corrupted the
            // graph must fail here, at the pass that broke it, rather than deep
            // in codegen. The `.pmc` driver validates the same way.
            #[cfg(debug_assertions)]
            for w in &ir.worlds {
                if let Err(e) = crate::ir::validate_world(w) {
                    panic!(
                        "pass `{name}` broke IR invariants in world `{}`: {e}",
                        w.name
                    );
                }
            }
            if n > 0 {
                report.changes.push(PassChange {
                    pass: name,
                    world: "(module)".to_string(),
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
            for w in &mut ir.worlds {
                let n = pass(w);
                // Validate after every pass regardless of the change count (see
                // the program-pass loop above).
                #[cfg(debug_assertions)]
                if let Err(e) = crate::ir::validate_world(w) {
                    panic!(
                        "pass `{name}` broke IR invariants in world `{}`: {e}",
                        w.name
                    );
                }
                if n > 0 {
                    report.changes.push(PassChange {
                        pass: name,
                        world: w.name.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_ir() -> IrProgram {
        IrProgram {
            version: crate::ir::TM_IR_VERSION,
            worlds: Vec::new(),
            entry_world: None,
        }
    }

    #[test]
    fn pass_registry_is_empty_until_passes_register() {
        // Both pipelines are empty scaffolding; `pass_names` is their only
        // reader and reflects it. Filled per task as passes land.
        assert!(pass_names().is_empty());
    }

    #[test]
    fn o0_returns_immediately() {
        let mut ir = empty_ir();
        let mut snaps = Vec::new();
        let report = optimize(&mut ir, &OptOptions::default(), &mut snaps);
        assert_eq!(report.rounds, 0);
        assert!(report.changes.is_empty());
        assert!(snaps.is_empty());
    }

    #[test]
    fn o1_converges_in_one_empty_round() {
        let mut ir = empty_ir();
        let mut snaps = Vec::new();
        let report = optimize(
            &mut ir,
            &OptOptions {
                level: OptLevel::O1,
                ..Default::default()
            },
            &mut snaps,
        );
        // One round runs, finds nothing to do (empty pipelines), converges.
        assert_eq!(report.rounds, 1);
        assert!(report.changes.is_empty());
        assert!(snaps.is_empty());
    }

    #[test]
    fn outline_is_the_only_default_off_program_pass() {
        // Default-ON passes run unless `--fno-<pass>` disables them.
        let on = OptOptions {
            level: OptLevel::O1,
            ..Default::default()
        };
        assert!(program_pass_enabled("inline", &on), "inline is default-ON");
        assert!(
            !program_pass_enabled("outline", &on),
            "outline is default-OFF without --foutline"
        );

        // `--foutline` turns outline on; it does not turn anything else on.
        let with_outline = OptOptions {
            outline: true,
            ..on.clone()
        };
        assert!(program_pass_enabled("outline", &with_outline));
        assert!(program_pass_enabled("inline", &with_outline));

        // `--fno-<pass>` wins even over an explicit `--foutline`.
        let mut disabled = HashSet::new();
        disabled.insert("outline".to_string());
        disabled.insert("inline".to_string());
        let vetoed = OptOptions {
            outline: true,
            disabled,
            ..on
        };
        assert!(!program_pass_enabled("outline", &vetoed));
        assert!(!program_pass_enabled("inline", &vetoed));
    }
}
