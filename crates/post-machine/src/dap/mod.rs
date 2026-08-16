//! `PmDapAdapter`: the PM-1 half of the Debug Adapter Protocol surface
//! (mtc_core::dap), serving `pmt dap`. This covers program-mode launch,
//! the v1 lifecycle commands, run control (`continue`/`pause`),
//! termination, source/instruction breakpoints, and stepping
//! (`next`/`stepIn`/`stepOut` at line or instruction granularity); a
//! later task extends the same struct with stack/scopes/variables.
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

use std::collections::BTreeSet;
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

/// `handle_set_breakpoints`'s answer for a line with no mapped code —
/// either the program was built with no `-g` map at all, or the specific
/// line requested has none (past the end, a comment, a blank line): both
/// point at the same fix.
const UNMAPPED_BREAKPOINT_MESSAGE: &str =
    "no code at this line — build with -g and place the breakpoint on an executable line";

/// `next`'s underlying primitive steps OVER a call (runs it to completion
/// before reporting); `stepIn`'s steps INTO one (lands on the callee's
/// first instruction). `handle_step` is the one loop parameterized by this.
#[derive(Clone, Copy)]
enum StepKind {
    Over,
    Into,
}

/// What a stepping request settled on, once its underlying `DebugSession`
/// motion(s) are done: either report one `Stopped` event with this reason
/// (and stay `Stopped`), or the debuggee finished — hand off to `finish`.
enum StepOutcome {
    Stop(&'static str, Option<String>),
    Finished(Outcome),
}

/// Every `DebugEvent` shape that is NOT a bare `Step` pause converts to a
/// stepping outcome the same way regardless of which motion produced it —
/// this is shared between `handle_step`'s loop and `handle_step_out`'s
/// single call. `PauseCause::Step` is deliberately excluded (`None`): each
/// call site decides for itself whether a bare step is done stepping
/// (`handle_step_out`, always) or should keep going (`handle_step`'s line
/// granularity, until the mapped line changes).
fn nonstep_outcome(event: DebugEvent) -> Option<StepOutcome> {
    match event {
        DebugEvent::Paused(PauseCause::Step) => None,
        DebugEvent::Paused(PauseCause::Breakpoint(_)) => {
            Some(StepOutcome::Stop("breakpoint", None))
        }
        DebugEvent::Paused(PauseCause::Brk) => Some(StepOutcome::Stop(
            "breakpoint",
            Some("debugger statement".to_string()),
        )),
        DebugEvent::Paused(PauseCause::Trap(trap)) => {
            Some(StepOutcome::Stop("exception", Some(trap.to_string())))
        }
        // No budget is ever passed to the motions this feeds (`step_over`/
        // `step_in`/`step_out` all run unbounded), so `Manual` should be
        // unreachable here in practice — kept for exhaustiveness against
        // future `PauseCause` growth, mapped the same as a manual step
        // would be (mirrors `tick`'s own defensive `Step` arm).
        DebugEvent::Paused(PauseCause::Manual) => Some(StepOutcome::Stop("step", None)),
        DebugEvent::Finished(outcome) => Some(StepOutcome::Finished(outcome)),
    }
}

/// `"0x…"`/`"0X…"` (or bare hex, no prefix) -> address. The same shape
/// `handle_set_breakpoints` hands back via `instructionReference`, so a
/// round trip through this parser recovers exactly the address that was
/// planted.
fn parse_instruction_reference(reference: &str) -> Option<u32> {
    let digits = reference
        .strip_prefix("0x")
        .or_else(|| reference.strip_prefix("0X"))
        .unwrap_or(reference);
    u32::from_str_radix(digits, 16).ok()
}

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
    line_index: Option<LineIndex>,
    launch_opts: Option<LaunchOpts>,
    /// The loop-facing run state (`DebugAdapter::run_state`): `Running`
    /// between a `configurationDone` (or `continue`) and the next
    /// pause/finish, `Done` once termination events have been pushed or
    /// `disconnect` was handled. Distinct from the underlying
    /// `DebugSession`'s own `finished()` — a trap pause leaves the
    /// session privately finished one call before this adapter reports
    /// `Done` (see `tick`'s `Finished` arm).
    run_state: RunState,
    /// Addresses this adapter added to the session on behalf of
    /// `setBreakpoints`, kept separately from `instruction_breakpoints` so
    /// each request kind can REPLACE only its own list (DAP semantics —
    /// `setBreakpoints`/`setInstructionBreakpoints` are independent
    /// breakpoint kinds). Also consulted directly by the stepping loop
    /// (`handle_step`): `step_in`/`step_over`'s raw per-instruction path
    /// never checks `DebugSession`'s own breakpoint set (only the
    /// `continue`-shaped motions do — docs/core.md (DebugSession)), so a
    /// breakpoint hit mid-line has to be noticed here instead.
    source_breakpoints: BTreeSet<u32>,
    /// Addresses added on behalf of `setInstructionBreakpoints` — see
    /// `source_breakpoints` for why this is a separate set and why the
    /// stepping loop consults both.
    instruction_breakpoints: BTreeSet<u32>,
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
            source_breakpoints: BTreeSet::new(),
            instruction_breakpoints: BTreeSet::new(),
        }
    }

    /// `Initialized` fires here — not from `initialize` itself — because
    /// readiness-for-configuration is genuinely gated on a program having
    /// loaded: a failed `launch` returns `Err` before this point and
    /// never claims readiness, which an automatic post-`initialize`
    /// emission (fired before any program exists to configure against)
    /// could not distinguish from a successful one.
    fn handle_launch(
        &mut self,
        arguments: &Value,
        out: &mut Vec<AdapterEvent>,
    ) -> Result<Value, String> {
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
        // A re-launch overwrites the session (module doc), so any
        // addresses tracked from the OLD session are meaningless against
        // the new one — clearing here avoids a stale entry coincidentally
        // matching a new address and firing a phantom breakpoint.
        self.source_breakpoints.clear();
        self.instruction_breakpoints.clear();

        out.push(AdapterEvent::Initialized);
        Ok(Value::Null)
    }

    /// Mirrors the real client sequence (`launch` → `initialized` →
    /// configuration requests → `configurationDone` → run): without
    /// `stopOnEntry`, `configurationDone` is what starts the program —
    /// it moves straight to `Running`, no explicit `continue` needed (a
    /// later `continue` while already `Running` stays legal — see
    /// `handle_continue` — so a client that sends one anyway is not
    /// punished). With `stopOnEntry`, the session stays paused at the
    /// entry instruction instead, reporting `stopped("entry")`; an
    /// explicit `continue` is what starts it from there.
    fn handle_configuration_done(&mut self, out: &mut Vec<AdapterEvent>) -> Result<Value, String> {
        let launch_opts = self
            .launch_opts
            .as_ref()
            .ok_or_else(|| "configurationDone before launch".to_string())?;
        // Mirrors `handle_continue`'s guard: a repeat `configurationDone`
        // after the program has already finished must not re-run
        // `finish()`'s termination events a second time.
        if self.run_state == RunState::Done {
            return Err("cannot configure: the program has already finished".to_string());
        }
        if launch_opts.stop_on_entry {
            out.push(AdapterEvent::Stopped {
                reason: "entry",
                description: None,
            });
        } else {
            self.run_state = RunState::Running;
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

    /// Source (line-mapped) breakpoints — DAP REPLACE semantics: this
    /// request's list is the WHOLE new set for this kind, so every
    /// previously tracked source-breakpoint address is cleared first
    /// (`setInstructionBreakpoints` owns its own independent list, unaffected
    /// here). A line resolves via `LineIndex::address_for_line`; a line past
    /// every mapped line (or a program with no `-g` map at all) answers
    /// `verified: false` with a message pointing at the fix. A verified
    /// entry's `line` is the RESOLVED line (the snapping rule may move it
    /// later than requested), and its `instructionReference` names the
    /// planted address — both for a real client's UI and so this crate's own
    /// tests can recover an address without any extra introspection surface.
    fn handle_set_breakpoints(&mut self, arguments: &Value) -> Result<Value, String> {
        let Some(session) = self.session.as_mut() else {
            return Err("setBreakpoints before launch".to_string());
        };
        let requested = arguments
            .get("breakpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for addr in std::mem::take(&mut self.source_breakpoints) {
            if !self.instruction_breakpoints.contains(&addr) {
                session.remove_breakpoint(addr);
            }
        }

        let mut results = Vec::with_capacity(requested.len());
        for item in &requested {
            let Some(line) = item.get("line").and_then(Value::as_u64).map(|l| l as u32) else {
                results.push(json!({
                    "verified": false,
                    "message": "breakpoint request is missing a line number",
                }));
                continue;
            };
            match self
                .line_index
                .as_ref()
                .and_then(|idx| idx.address_for_line(line))
            {
                Some(addr) => {
                    session.add_breakpoint(addr);
                    self.source_breakpoints.insert(addr);
                    let resolved_line = self
                        .line_index
                        .as_ref()
                        .and_then(|idx| idx.resolve(addr))
                        .and_then(|(_, l)| l)
                        .unwrap_or(line);
                    results.push(json!({
                        "verified": true,
                        "line": resolved_line,
                        "instructionReference": format!("0x{addr:x}"),
                    }));
                }
                None => {
                    results.push(json!({
                        "verified": false,
                        "line": line,
                        "message": UNMAPPED_BREAKPOINT_MESSAGE,
                    }));
                }
            }
        }
        Ok(json!({"breakpoints": results}))
    }

    /// Instruction breakpoints — the address analog of `handle_set_breakpoints`:
    /// same REPLACE-this-kind-only semantics, own tracked set. Each entry's
    /// `instructionReference` is a hex address (`"0x…"`, case-insensitive
    /// prefix, no fixed width — the same shape `handle_set_breakpoints`
    /// hands back); anything that fails to parse answers `verified: false`
    /// rather than rejecting the whole request; a raw address needs no
    /// `-g` map to be legal, so unlike source breakpoints every parseable
    /// address is planted directly.
    fn handle_set_instruction_breakpoints(&mut self, arguments: &Value) -> Result<Value, String> {
        let Some(session) = self.session.as_mut() else {
            return Err("setInstructionBreakpoints before launch".to_string());
        };
        let requested = arguments
            .get("breakpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for addr in std::mem::take(&mut self.instruction_breakpoints) {
            if !self.source_breakpoints.contains(&addr) {
                session.remove_breakpoint(addr);
            }
        }

        let mut results = Vec::with_capacity(requested.len());
        for item in &requested {
            let Some(reference) = item.get("instructionReference").and_then(Value::as_str) else {
                results.push(json!({
                    "verified": false,
                    "message": "breakpoint request is missing an instructionReference",
                }));
                continue;
            };
            match parse_instruction_reference(reference) {
                Some(addr) => {
                    session.add_breakpoint(addr);
                    self.instruction_breakpoints.insert(addr);
                    results.push(json!({"verified": true}));
                }
                None => {
                    results.push(json!({
                        "verified": false,
                        "message": format!("invalid instructionReference: {reference}"),
                    }));
                }
            }
        }
        Ok(json!({"breakpoints": results}))
    }

    /// Shared precondition for `next`/`stepIn`/`stepOut`: a session must
    /// exist and be genuinely paused — mirrors `handle_continue`'s `Done`
    /// guard and `handle_pause`'s run-state check, the stepping-command
    /// analog of both (a step only makes sense from a stopped debuggee, and
    /// never after termination).
    fn ensure_can_step(&self) -> Result<(), String> {
        if self.session.is_none() {
            return Err("step before launch".to_string());
        }
        match self.run_state {
            RunState::Running => Err("cannot step: the program is running".to_string()),
            RunState::Done => Err("cannot step: the program has already finished".to_string()),
            RunState::Stopped => Ok(()),
        }
    }

    /// `next` (`StepKind::Over`) / `stepIn` (`StepKind::Into`): granularity
    /// (spec §5) toggles between two shapes over the SAME underlying
    /// session primitive (`step_over`/`step_in`) — `"instruction"` stops
    /// after exactly one session step; anything else (the default, and
    /// DAP's own default `"statement"`) repeats session steps until
    /// `LineIndex::resolve(ip).line` changes from the line steps started on,
    /// treating a transition into unmapped code (`Some` -> `None`) as a
    /// change too, so stepping never silently swallows a function with no
    /// `-g` data. A breakpoint/`brk`/trap interrupt wins over BOTH shapes
    /// and reports its own reason instead of the line's.
    ///
    /// The two granularities need this adapter-level loop for the SAME
    /// reason `source_breakpoints`/`instruction_breakpoints` exist as their
    /// own fields: `step_over`/`step_in`'s raw per-instruction path never
    /// consults `DebugSession`'s own breakpoint set (only the
    /// `continue`-shaped motions do) — this loop's own membership check on
    /// every retired instruction is what makes a mid-line breakpoint
    /// interrupt a line step at all. `step_over`'s OWN internal fast-forward
    /// through a call it steps over (depth increased) DOES go through that
    /// `continue`-shaped path, so a breakpoint inside a stepped-over call
    /// still surfaces as `DebugEvent::Paused(PauseCause::Breakpoint(_))`
    /// directly from `session.step_over` itself — `nonstep_outcome` handles
    /// that case uniformly with the loop's own check.
    fn handle_step(
        &mut self,
        arguments: &Value,
        out: &mut Vec<AdapterEvent>,
        kind: StepKind,
    ) -> Result<Value, String> {
        self.ensure_can_step()?;
        let instruction_granularity =
            arguments.get("granularity").and_then(Value::as_str) == Some("instruction");

        let session = self.session.as_mut().expect("checked by ensure_can_step");
        let tape = self
            .tape
            .as_mut()
            .expect("session and tape are set together");

        let start_line = if instruction_granularity {
            None
        } else {
            self.line_index
                .as_ref()
                .and_then(|idx| idx.resolve(session.ip()))
                .and_then(|(_, l)| l)
        };

        let outcome = loop {
            let event = match kind {
                StepKind::Over => session.step_over(tape),
                StepKind::Into => session.step_in(tape),
            };
            if let Some(outcome) = nonstep_outcome(event) {
                break outcome;
            }
            // A bare `Step` pause: decide whether to report it now or keep
            // stepping.
            let ip = session.ip();
            if self.source_breakpoints.contains(&ip) || self.instruction_breakpoints.contains(&ip) {
                break StepOutcome::Stop("breakpoint", None);
            }
            if instruction_granularity {
                break StepOutcome::Stop("step", None);
            }
            let now_line = self
                .line_index
                .as_ref()
                .and_then(|idx| idx.resolve(ip))
                .and_then(|(_, l)| l);
            if now_line != start_line {
                break StepOutcome::Stop("step", None);
            }
        };
        self.apply_step_outcome(outcome, out);
        Ok(Value::Null)
    }

    /// `stepOut`: depth-based via `step_out_tapes`'s single-tape sibling
    /// `step_out` — ALWAYS one call, granularity does not apply (spec §5):
    /// "step out of the current call" already names its own stopping point
    /// (the caller, one depth up), so there is no separate line-vs-
    /// instruction shape to choose between the way there is for
    /// `next`/`stepIn`.
    fn handle_step_out(&mut self, out: &mut Vec<AdapterEvent>) -> Result<Value, String> {
        self.ensure_can_step()?;
        let session = self.session.as_mut().expect("checked by ensure_can_step");
        let tape = self
            .tape
            .as_mut()
            .expect("session and tape are set together");
        let event = session.step_out(tape);
        let outcome = nonstep_outcome(event).unwrap_or(StepOutcome::Stop("step", None));
        self.apply_step_outcome(outcome, out);
        Ok(Value::Null)
    }

    /// Shared tail of `handle_step`/`handle_step_out`: a `Stop` pushes one
    /// `Stopped` event and leaves the session `Stopped`; a `Finished` outcome
    /// hands off to `finish` (the same termination path `tick` uses).
    fn apply_step_outcome(&mut self, outcome: StepOutcome, out: &mut Vec<AdapterEvent>) {
        match outcome {
            StepOutcome::Stop(reason, description) => {
                out.push(AdapterEvent::Stopped {
                    reason,
                    description,
                });
                self.run_state = RunState::Stopped;
            }
            StepOutcome::Finished(outcome) => self.finish(outcome, out),
        }
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
            // `supportsSetVariable` / `supportsDisassembleRequest` join once
            // stack/scopes/variables land in a later task.
            "initialize" => Ok(json!({
                "supportsConfigurationDoneRequest": true,
                "supportsSteppingGranularity": true,
                "supportsInstructionBreakpoints": true,
            })),
            "launch" => self.handle_launch(arguments, out),
            "configurationDone" => self.handle_configuration_done(out),
            "threads" => Ok(json!({"threads": [{"id": 1, "name": "machine"}]})),
            "continue" => self.handle_continue(),
            "pause" => self.handle_pause(out),
            "setBreakpoints" => self.handle_set_breakpoints(arguments),
            "setInstructionBreakpoints" => self.handle_set_instruction_breakpoints(arguments),
            "next" => self.handle_step(arguments, out, StepKind::Over),
            "stepIn" => self.handle_step(arguments, out, StepKind::Into),
            "stepOut" => self.handle_step_out(out),
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
