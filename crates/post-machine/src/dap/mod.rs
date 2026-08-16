//! `PmDapAdapter`: the PM-1 half of the Debug Adapter Protocol surface
//! (mtc_core::dap), serving `pmt dap`. This skeleton covers program-mode
//! launch, the v1 lifecycle commands, run control (`continue`/`pause`),
//! and termination; later tasks extend the same struct with breakpoints,
//! stepping granularity, and stack/scopes/variables.
//!
//! Program mode (the only launch shape this skeleton implements): the
//! `launch` request names a prebuilt `.pmx` executable (`"program"`) and
//! an optional `.pmt` tape snapshot (`"tape"`) — PM defaults to the empty
//! tape when none is given. Target-mode launch (build-in-process from a
//! project manifest target) is a later task.
//!
//! Lifetime shape: `Machine`/`DebugSession` borrow the `&dyn Arch` they
//! were built against, not the struct that holds them — so keeping the
//! one `Pm1` instance behind a `'static` reference (`PM1`, a bare unit
//! struct — trivially `Sync`, unlike a `Box<dyn Arch>`-holding
//! `ArchRegistry`, which cannot itself sit behind a `static`) lets every
//! launched session be `DebugSession<'static>`, an ordinary owned field
//! with no self-referential borrow back into this adapter. PM-1 images
//! are always the v1 code-only shape (no tables, single tape — `pm1_syntax`
//! never opts into the table/vector assembler capabilities), so
//! `Machine::with_arch` — checked against the executable's own `arch`
//! byte first — is exactly `Machine::from_executable`'s outcome for any
//! real `.pmx` this adapter is handed.

use std::fs;

use serde_json::{Value, json};

use mtc_core::dap::server::{AdapterEvent, DebugAdapter, RunState};
use mtc_core::formats::ARCH_PM1;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeBlockFile;
use mtc_core::linemap::LineIndex;
use mtc_core::linker::MapFile;
use mtc_core::vm::{
    DebugEvent, DebugSession, InfiniteTape, Machine, Outcome, PauseCause, RunOptions,
};

use crate::arch::Pm1;

/// Per-tick step slice `tick` drives the session through
/// (`session.run_steps(tape, BUDGET)`): sub-millisecond on any program
/// this toolchain compiles, so a queued `pause`/other request is noticed
/// promptly by the server loop's `try_recv`/`tick` alternation
/// (`mtc_core::dap::server::run`) instead of waiting for a long or
/// non-terminating run to yield on its own. Deliberately unbounded
/// otherwise — the CLI's step cap does not apply to an interactive
/// session with a human `pause` button.
const BUDGET: u64 = 10_000;

/// The one `Pm1` arch instance every launched machine borrows (see the
/// module doc for why this is a bare static rather than an `ArchRegistry`).
static PM1: Pm1 = Pm1;

/// The `launch` request's program-mode arguments, kept for the lifetime
/// of the session (`stopOnEntry` is consulted once, at
/// `configurationDone`; `program`/`tape` are carried for future tasks —
/// e.g. a termination summary naming the program).
struct LaunchOpts {
    #[allow(dead_code)] // carried for later tasks (e.g. re-launch, summaries)
    program: String,
    #[allow(dead_code)]
    tape: Option<String>,
    stop_on_entry: bool,
}

/// The PM-1 debug adapter. Nothing is populated before a successful
/// `launch`; the four launch/session fields are `None` until then and
/// `Some` for the rest of the session's life (program mode never
/// re-launches in place — a second `launch` simply overwrites them).
pub struct PmDapAdapter {
    session: Option<DebugSession<'static>>,
    tape: Option<InfiniteTape>,
    #[allow(dead_code)] // resolved at launch; consumed starting with the breakpoints task
    line_index: Option<LineIndex>,
    launch_opts: Option<LaunchOpts>,
    /// The loop-facing run state (`DebugAdapter::run_state`): `Running`
    /// only between a `continue` and the next pause/finish, `Done` once
    /// termination events have been pushed or `disconnect` was handled.
    /// Distinct from the underlying `DebugSession`'s own `finished()` —
    /// a trap pause leaves the session privately finished one call before
    /// this adapter reports `Done` (see `tick`'s `Finished` arm).
    run_state: RunState,
}

impl Default for PmDapAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl PmDapAdapter {
    pub fn new() -> Self {
        PmDapAdapter {
            session: None,
            tape: None,
            line_index: None,
            launch_opts: None,
            run_state: RunState::Stopped,
        }
    }

    fn handle_launch(&mut self, arguments: &Value) -> Result<Value, String> {
        let program = arguments
            .get("program")
            .and_then(Value::as_str)
            .ok_or_else(|| "launch requires a 'program' path".to_string())?
            .to_string();
        let tape_path = arguments
            .get("tape")
            .and_then(Value::as_str)
            .map(str::to_string);
        let stop_on_entry = arguments
            .get("stopOnEntry")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let bytes = fs::read(&program).map_err(|e| format!("cannot read {program}: {e}"))?;
        let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{program}: {e}"))?;
        if exe.arch != ARCH_PM1 {
            return Err(format!(
                "{program}: not a PM-1 executable (arch byte {:#04x})",
                exe.arch
            ));
        }
        let machine = Machine::with_arch(&PM1, exe.code.clone(), exe.entry)
            .map_err(|e| format!("{program}: {e}"))?;

        let tape = build_tape(tape_path.as_deref())?;
        let session = machine.debug(RunOptions::default());
        let line_index = sidecar_map(&program).map(|map| LineIndex::new(&map));

        self.session = Some(session);
        self.tape = Some(tape);
        self.line_index = line_index;
        self.launch_opts = Some(LaunchOpts {
            program,
            tape: tape_path,
            stop_on_entry,
        });
        self.run_state = RunState::Stopped;

        Ok(Value::Null)
    }

    /// `stopOnEntry` fires here, not at `launch`: the client is only
    /// guaranteed ready to receive events once configuration is done.
    /// Without it, `configurationDone` leaves the session paused,
    /// awaiting an explicit `continue`.
    fn handle_configuration_done(&mut self, out: &mut Vec<AdapterEvent>) -> Result<Value, String> {
        let launch_opts = self
            .launch_opts
            .as_ref()
            .ok_or_else(|| "configurationDone before launch".to_string())?;
        if launch_opts.stop_on_entry {
            out.push(AdapterEvent::Stopped {
                reason: "entry",
                description: None,
            });
        }
        Ok(Value::Null)
    }

    fn handle_continue(&mut self) -> Result<Value, String> {
        if self.session.is_none() {
            return Err("continue before launch".to_string());
        }
        if self.run_state == RunState::Done {
            return Err("cannot continue: the program has already finished".to_string());
        }
        self.run_state = RunState::Running;
        Ok(json!({"allThreadsContinued": true}))
    }

    /// Purely adapter-side: unlike `AsyncSession`, `DebugSession` has no
    /// external pause flag, so a client `pause` is honored between ticks
    /// by flipping `run_state` straight to `Stopped` — the server loop
    /// (`mtc_core::dap::server::run`) only calls `tick` again once this
    /// adapter reports `Running`, so no in-flight step is interrupted;
    /// the next `continue` resumes exactly where the last `tick` left off.
    fn handle_pause(&mut self, out: &mut Vec<AdapterEvent>) -> Result<Value, String> {
        if self.session.is_none() {
            return Err("pause before launch".to_string());
        }
        if self.run_state != RunState::Running {
            return Err("cannot pause: the program is not running".to_string());
        }
        self.run_state = RunState::Stopped;
        out.push(AdapterEvent::Stopped {
            reason: "pause",
            description: None,
        });
        Ok(Value::Null)
    }

    /// Termination path (docs/pmt/cli.md's `pmt run` exit-code mapping,
    /// mirrored here rather than cited from a DAP doc that doesn't exist
    /// yet): one summary `output` event carrying the same numbers `run`
    /// prints, then `terminated`, then `exited` with 0/2/3 for
    /// stopped/halted/trapped.
    fn finish(&mut self, outcome: Outcome, out: &mut Vec<AdapterEvent>) {
        let stats = self
            .session
            .as_ref()
            .expect("finish is only called right after this session produced Finished")
            .stats();
        let summary = format!(
            "outcome: {outcome:?}\nsteps {}, core tacts {}, stall tacts {} (total {})",
            stats.steps,
            stats.core_tacts,
            stats.stall_tacts,
            stats.total_tacts()
        );
        out.push(AdapterEvent::Output {
            category: "console",
            output: summary,
        });
        out.push(AdapterEvent::Terminated);
        let code = match outcome {
            Outcome::Stopped => 0,
            Outcome::Halted => 2,
            Outcome::Trapped(_) => 3,
        };
        out.push(AdapterEvent::Exited { code });
        self.run_state = RunState::Done;
    }
}

impl DebugAdapter for PmDapAdapter {
    fn handle(
        &mut self,
        command: &str,
        arguments: &Value,
        out: &mut Vec<AdapterEvent>,
    ) -> Result<Value, String> {
        match command {
            // `supportsConfigurationDoneRequest` is the only capability
            // honestly true at this skeleton stage; `supportsSetVariable`
            // / `supportsSteppingGranularity` / `supportsDisassembleRequest`
            // / `supportsInstructionBreakpoints` join once the features
            // they name land in later tasks.
            "initialize" => Ok(json!({"supportsConfigurationDoneRequest": true})),
            "launch" => self.handle_launch(arguments),
            "configurationDone" => self.handle_configuration_done(out),
            "threads" => Ok(json!({"threads": [{"id": 1, "name": "machine"}]})),
            "continue" => self.handle_continue(),
            "pause" => self.handle_pause(out),
            "disconnect" => {
                self.run_state = RunState::Done;
                Ok(Value::Null)
            }
            other => Err(mtc_core::dap::server::unsupported_command(other)),
        }
    }

    fn tick(&mut self, out: &mut Vec<AdapterEvent>) -> RunState {
        let event = match (&mut self.session, &mut self.tape) {
            (Some(session), Some(tape)) => session.run_steps(tape, BUDGET),
            _ => {
                self.run_state = RunState::Done;
                return RunState::Done;
            }
        };
        match event {
            // Budget exhaustion, DebugSession's only `Manual` cause
            // (docs/core.md's `PauseCause` doc): invisible to the
            // client per the design's run-loop rule — stay Running.
            DebugEvent::Paused(PauseCause::Manual) => {}
            DebugEvent::Paused(PauseCause::Trap(trap)) => {
                out.push(AdapterEvent::Stopped {
                    reason: "exception",
                    description: Some(trap.to_string()),
                });
                self.run_state = RunState::Stopped;
            }
            DebugEvent::Paused(PauseCause::Breakpoint(_)) => {
                out.push(AdapterEvent::Stopped {
                    reason: "breakpoint",
                    description: None,
                });
                self.run_state = RunState::Stopped;
            }
            DebugEvent::Paused(PauseCause::Brk) => {
                out.push(AdapterEvent::Stopped {
                    reason: "breakpoint",
                    description: Some("debugger statement".to_string()),
                });
                self.run_state = RunState::Stopped;
            }
            DebugEvent::Paused(PauseCause::Step) => {
                // run_steps's own loop never surfaces a bare Step (only
                // budget exhaustion, a breakpoint, a brk, or a trap end
                // one of its iterations) — kept for exhaustiveness against
                // future PauseCause growth, mapped the same as a manual
                // step would be.
                out.push(AdapterEvent::Stopped {
                    reason: "step",
                    description: None,
                });
                self.run_state = RunState::Stopped;
            }
            DebugEvent::Finished(outcome) => self.finish(outcome, out),
        }
        self.run_state
    }

    fn run_state(&self) -> RunState {
        self.run_state
    }
}

/// Program-mode tape resolution: PM defaults to the empty tape when no
/// `"tape"` argument is given; otherwise loads a `.pmt` block the same
/// way `pmt run --tape-block` does (a per-tape glyph table wins over the
/// block's shared fallback).
fn build_tape(tape_path: Option<&str>) -> Result<InfiniteTape, String> {
    let Some(path) = tape_path else {
        return Ok(InfiniteTape::new());
    };
    let bytes = fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let file = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let [snapshot] = file.tapes.as_slice() else {
        return Err(format!("{path}: PM-1 blocks hold exactly one tape"));
    };
    InfiniteTape::from_snapshot(snapshot).map_err(|e| format!("{path}: {e:?}"))
}

/// `<program>.map` sidecar discovery — a standalone copy of
/// `cli::inspect::sidecar_map`'s logic (that function is private to
/// `cli`); a missing or unparsable sidecar degrades to no line info
/// rather than failing the launch — program mode is meant to run a
/// prebuilt artifact as-is, `-g` debug info or not.
fn sidecar_map(program: &str) -> Option<MapFile> {
    let mut sidecar = std::ffi::OsString::from(program);
    sidecar.push(".map");
    fs::read_to_string(sidecar)
        .ok()
        .and_then(|text| MapFile::from_json(&text).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tape_defaults_to_empty_when_no_tape_path_is_given() {
        let tape = build_tape(None).unwrap();
        assert_eq!(tape.head(), 0);
    }

    #[test]
    fn sidecar_map_is_none_for_a_missing_file() {
        assert!(sidecar_map("/definitely/not/a/real/path.pmx").is_none());
    }
}
