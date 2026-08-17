//! `PmDapAdapter`: the PM-1 half of the Debug Adapter Protocol surface
//! (mtc_core::dap), serving `pmt dap`. This covers program-mode launch,
//! the v1 lifecycle commands, run control (`continue`/`pause`),
//! termination, source/instruction breakpoints, stepping
//! (`next`/`stepIn`/`stepOut` at line or instruction granularity), and
//! the state surface: stack/scopes/variables, `setVariable`,
//! `disassemble`, and the opt-in `"trace": true` per-instruction output
//! stream. The user-facing contract this module implements — the launch
//! schema, the closed output-events list, the writable-state contract,
//! and the degradation rules without `-g` — is documented at docs/dap.md.
//!
//! Two `launch` shapes, dispatched on which of `"program"`/`"target"` the
//! arguments carry (`handle_launch`), sharing everything past that point
//! through `finish_launch`:
//!
//! - **Program mode**: names a prebuilt `.pmx` executable (`"program"`)
//!   and an optional `.pmt` tape snapshot (`"tape"`) — PM defaults to the
//!   empty tape when none is given. `"strictCells": bool` (default
//!   false) wraps the tape in the strict-cells decorator, the same
//!   semantics `pmt run --strict-cells` gives a plain run.
//! - **Target mode**: names a manifest target (`"target": "<name>"`) and
//!   an optional `"project"` path override (the discovery walk's
//!   starting point — `cli::driver::build_target_for_launch`'s own
//!   `current_dir` fallback otherwise). The target builds IN PROCESS,
//!   through that same `cli::driver` seam `pmt build TARGET` itself
//!   runs — never shelling out — always with `-g` forced regardless of
//!   the target's resolved profile (a debug session without line maps is
//!   crippled). Its compile warnings stream as `stderr`-category
//!   `Output` events, one per diagnostic line, BEFORE `Initialized`; a
//!   failed build fails the `launch` request with the driver's rendered
//!   error text (module-wide rule: a failed launch must never claim
//!   readiness). The tape comes from the target's own `run` settings,
//!   resolved through the exact tape/tape-block/head/strict-cells rules
//!   `pmt build --run` uses — this adapter reimplements none of it, and
//!   has no `"tape"`/`"strictCells"` arguments of its own in this mode.
//!
//! Both modes share `"stopOnEntry"`/`"trace"`.
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
//!
//! `tape` is `Box<dyn Tape>`, not the concrete `InfiniteTape`, so
//! `"strictCells"` can wrap it without a second field or an enum — every
//! session-driving call already goes through `DebugSession`'s `&mut dyn
//! Tape` parameter regardless.
//!
//! `variablesReference` handle scheme (DAP's per-scope/per-container
//! integer handle; `0` is DAP's own "no children" sentinel and is never
//! issued by this adapter): `SCOPE_REGISTERS` and `SCOPE_TAPES` are the
//! two scopes `scopes` always answers (machine state is global —
//! identical for any selected frame, so `scopes`/`variables` do not
//! thread the requested frame id through at all); `TAPE_WINDOW_BASE + n`
//! is tape `n`'s own head±8 cell window, reached by expanding its entry
//! inside the Tapes scope. PM-1 has exactly one tape, so only
//! `TAPE_WINDOW_BASE` itself is ever issued; a TM adapter reusing this
//! scheme would issue `TAPE_WINDOW_BASE + 1`, `+ 2`, … for its other tapes.
//!
//! **Handle stability.** Every handle above, and every stack-frame `id`
//! `stackTrace` hands back, is issued as the bare constant, identical
//! across stops. That is the DAP contract working as intended: a
//! `variablesReference` is only valid while the session is paused, and a
//! client re-fetches scopes and variables on every stop (VS Code
//! discards its variable model on the `continued` event). An earlier
//! revision salted every handle with a per-stop generation to bust a
//! suspected client-side cache — the stale-Variables report that
//! motivated it was actually the missing frame `source` object (with no
//! openable source, VS Code never auto-focused the stopped frame and so
//! never asked for its scopes at all — docs/dap.md (source
//! provenance)); once frames carried sources, the salt had nothing left
//! to do, and stable handles are what let a client correlate its own
//! view state across stops. A reference a client held from a PRIOR stop
//! is therefore simply the current reference and resolves live data.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use mtc_core::asm::listing_line;
use mtc_core::dap::server::{AdapterEvent, DebugAdapter, RunState};
use mtc_core::formats::ARCH_PM1;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeBlockFile;
use mtc_core::linemap::LineIndex;
use mtc_core::linker::MapFile;
use mtc_core::vm::{
    DebugEvent, DebugSession, InfiniteTape, Machine, Outcome, PauseCause, RunOptions, StrictTape,
    Tape,
};

use crate::arch::Pm1;
use crate::asm::pm1_syntax;

/// Fixed `variablesReference` handles — see the module doc for the full
/// scheme.
const SCOPE_REGISTERS: i64 = 1;
const SCOPE_TAPES: i64 = 2;
const TAPE_WINDOW_BASE: i64 = 100;

/// Half-width of the tape variables window (`TAPE_WINDOW_BASE`): head±8,
/// 17 cells total.
const TAPE_WINDOW_RADIUS: i64 = 8;

/// Per-tick step slice `tick` drives the session through
/// (`session.run_steps(tape, BUDGET)`): sub-millisecond on any program
/// this toolchain compiles, so a queued `pause`/other request is noticed
/// promptly by the server loop's `try_recv`/`tick` alternation
/// (`mtc_core::dap::server::run`) instead of waiting for a long or
/// non-terminating run to yield on its own. Deliberately unbounded
/// otherwise — the CLI's step cap does not apply to an interactive
/// session with a human `pause` button. Also the per-tick slice `tick`
/// drives through under `"trace": true` (`run_traced`), for the same
/// responsiveness reason.
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

/// `handle_set_breakpoints`'s answer when the map DOES carry source
/// provenance but none of its files is the one this request names — the
/// per-file filter (docs/dap.md (breakpoints and stepping)) then has no
/// line table to search, and falling back to the global one would plant
/// breakpoints from an unrelated file's identical line numbers.
const FOREIGN_SOURCE_BREAKPOINT_MESSAGE: &str =
    "no code in this program comes from this file (per the map sidecar's source records)";

/// `next`'s underlying primitive steps OVER a call (runs it to completion
/// before reporting); `stepIn`'s steps INTO one (lands on the callee's
/// first instruction). `handle_step` is the one loop parameterized by this.
#[derive(Clone, Copy)]
enum StepKind {
    Over,
    Into,
}

/// How a `setBreakpoints` request's file constrains the line search
/// (docs/dap.md (breakpoints and stepping)). `Global` — no provenance in
/// the map (or no file in the request): every function's lines are
/// searched, the pre-provenance behavior. `File` — the request's file
/// matched a map source record; only that file's lines are searched, so
/// identical line numbers across compilation units cannot collide.
/// `Foreign` — the map HAS provenance but names no file matching the
/// request: nothing is searched, and the un-verified answer says why.
enum SourceFilter {
    Global,
    File(String),
    Foreign,
}

/// What a stepping request settled on, once its underlying `DebugSession`
/// motion(s) are done: either report one `Stopped` event with this reason
/// (and stay `Stopped`), or the debuggee finished — hand off to `finish`.
enum StepOutcome {
    Stop(&'static str, Option<String>),
    Finished(Outcome),
}

/// `step_traced`'s stopping rule — the trace-mode analog of the two
/// shapes `session.step_in`/`step_over`'s fast-forward give the untraced
/// path: exactly one retired instruction, or "keep going while depth
/// stays above this target" (the `step_over`/`step_out` fast-forward
/// shape, mirroring `DebugSession::run_until_tapes`'s own `until_depth`
/// gate).
#[derive(Clone, Copy)]
enum StopWhen {
    OneInstruction,
    DepthAtMost(usize),
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

/// The label-resolve closure `drive_traced`'s PM twin uses (`cli/run.rs`):
/// a call site's exact target names its function; any other resolved
/// address names `function.label`. Shared by the trace renderer and
/// `disassemble`, which both walk `listing_line` over the same code image.
fn resolve_label(map: Option<&MapFile>, target: u32) -> Option<String> {
    let m = map?;
    m.functions.iter().find_map(|f| {
        if f.start == target {
            return Some(f.name.clone());
        }
        f.labels
            .iter()
            .find(|(_, a)| *a == target)
            .map(|(l, _)| format!("{}.{l}", f.name))
    })
}

/// One `"trace": true` output line, byte-format matching `pmt run
/// --trace`'s (`drive_traced`, `cli/run.rs`): the `listing_line` text
/// (or a synthetic line for a fetch beyond the code image) plus the
/// post-instruction `MF`/head suffix.
fn trace_output_line(code: &[u8], map: Option<&MapFile>, ip: u32, mf: bool, head: i64) -> String {
    let syntax = pm1_syntax();
    let resolve = |target: u32| resolve_label(map, target);
    let line = if (ip as usize) < code.len() {
        listing_line(&syntax, code, ip, &resolve).0
    } else {
        format!("  {ip:04x}:  <beyond code image>")
    };
    format!("{line}  ; MF={} head={}", u8::from(mf), head)
}

/// Reads the `pos`±`TAPE_WINDOW_RADIUS` window around the tape's current
/// head, restoring the head to where it started (the same walk-and-restore
/// discipline `Tape::poke` uses, since `Tape` exposes no positional read).
/// Returns `(position, raw cell index)` pairs, ascending by position.
fn tape_window(tape: &mut dyn Tape) -> Vec<(i64, u32)> {
    let origin = tape.head();
    for _ in 0..TAPE_WINDOW_RADIUS {
        tape.left();
    }
    let span = TAPE_WINDOW_RADIUS * 2 + 1;
    let mut cells = Vec::with_capacity(span as usize);
    for i in 0..span {
        cells.push((origin - TAPE_WINDOW_RADIUS + i, tape.read()));
        if i < span - 1 {
            tape.right();
        }
    }
    for _ in 0..TAPE_WINDOW_RADIUS {
        tape.left();
    }
    cells
}

/// A cell's rendered glyph: the launch tape block's alphabet where known
/// (a cell whose index falls outside that alphabet's declared range still
/// falls back to the raw index, rather than failing), the raw index as a
/// string otherwise (no tape block was given at launch).
fn glyph_for(alphabet: &Option<Vec<String>>, index: u32) -> String {
    alphabet
        .as_ref()
        .and_then(|a| a.get(index as usize))
        .cloned()
        .unwrap_or_else(|| index.to_string())
}

/// `variables`' rendering of a glyph, quoted (`'x'`) so a blank glyph
/// (PM-1's is a literal space) still renders visibly.
fn quote_glyph(glyph: &str) -> String {
    format!("'{glyph}'")
}

/// The inverse of `quote_glyph`, tolerant of an unquoted value too — a
/// `setVariable` client may echo back exactly what `variables` rendered
/// (quoted) or a user-typed bare glyph (unquoted); both round-trip.
fn unquote_glyph(value: &str) -> &str {
    value
        .strip_prefix('\'')
        .and_then(|v| v.strip_suffix('\''))
        .unwrap_or(value)
}

/// A tape-window variable's `name` (`"[3]"`, or `"» [3]"` under the head)
/// back to its position — the inverse of the format `handle_variables`
/// renders, so `setVariable` can recover which cell a client named.
fn parse_cell_name(name: &str) -> Option<i64> {
    let trimmed = name.strip_prefix("» ").unwrap_or(name);
    trimmed.strip_prefix('[')?.strip_suffix(']')?.parse().ok()
}

fn readonly_var(name: &str, value: String) -> Value {
    json!({
        "name": name,
        "value": value,
        "variablesReference": 0,
        "presentationHint": {"attributes": ["readOnly"]},
    })
}

fn writable_var(name: &str, value: String) -> Value {
    json!({"name": name, "value": value, "variablesReference": 0})
}

/// What EITHER launch mode resolves down to before
/// `PmDapAdapter::finish_launch` takes over — a plain data bundle rather
/// than a long `finish_launch` parameter list (`clippy::too_many_arguments`
/// territory otherwise: 7+ independent values). `program` is the
/// executable path already on disk (a launch argument in program mode,
/// the just-built output path in target mode); `tape_arg` is the RAW
/// `"tape"` launch argument if there was one — `None` in target mode,
/// which has no such argument of its own (its tape comes from the
/// target's `run` settings instead, already folded into `tape`/
/// `alphabet` by the time this struct exists).
struct ResolvedLaunch {
    program: String,
    tape_arg: Option<String>,
    tape: InfiniteTape,
    alphabet: Option<Vec<String>>,
    strict_cells: bool,
    stop_on_entry: bool,
    trace: bool,
}

/// The resolved `launch` request's state, kept for the lifetime of the
/// session regardless of which mode produced it (`stopOnEntry` is
/// consulted once, at `configurationDone`; `program`/`tape` are carried
/// for future tasks — e.g. a termination summary naming the program;
/// `trace` is read on every `tick`/step to choose the traced code path).
struct LaunchOpts {
    #[allow(dead_code)] // carried for later tasks (e.g. re-launch, summaries)
    program: String,
    #[allow(dead_code)]
    tape: Option<String>,
    stop_on_entry: bool,
    trace: bool,
}

/// The PM-1 debug adapter. Nothing is populated before a successful
/// `launch`; the launch/session fields are `None` until then and `Some`
/// for the rest of the session's life (program mode never re-launches in
/// place — a second `launch` simply overwrites them).
pub struct PmDapAdapter {
    session: Option<DebugSession<'static>>,
    tape: Option<Box<dyn Tape>>,
    line_index: Option<LineIndex>,
    /// The launched executable's code image — retained (not just used to
    /// build `Machine`/`session`) because `disassemble` and the trace
    /// renderer both need to re-decode it after launch.
    code: Option<Vec<u8>>,
    /// The sidecar map itself, not just the `LineIndex` built from it:
    /// `disassemble`/trace label resolution needs `MapFunction::labels`,
    /// which `LineIndex` does not carry (it only indexes lines).
    map: Option<MapFile>,
    /// The sidecar's directory, lexically absolutized at launch — the
    /// anchor the map's relative `source` entries resolve against, both
    /// for the `source` objects frames carry and for matching a
    /// `setBreakpoints` request's file (docs/formats.md (map sidecar)).
    /// `None` until a launch discovers a sidecar.
    map_dir: Option<PathBuf>,
    /// The launch tape block's glyph table. Program mode only: `None`
    /// when no `"tape"` was given (the default empty tape carries no
    /// alphabet) — `variables` falls back to raw indices in that case
    /// (module doc). Target mode is never `None` — `cli::run::initial_tape`
    /// (which `cli::driver::build_target_for_launch` resolves the
    /// target's tape through) falls back to the CLI's own default glyphs
    /// when the target's `run` block names no tape of its own, matching
    /// `pmt run`'s own behavior: a target-mode session always has an
    /// effective alphabet, program mode only sometimes does. A program
    /// launched both ways can render a marked cell as `'*'` (target,
    /// default glyphs or its own tape/tape-block) vs. a raw index
    /// (program mode, no `"tape"` argument) — a deliberate mode
    /// difference, not a bug.
    alphabet: Option<Vec<String>>,
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
    /// breakpoint kinds). Bucketed per source file — DAP's own contract
    /// is per-source replacement ("clears all previous breakpoints in
    /// that source"), which matters once the map's provenance lets two
    /// files hold breakpoints at once (docs/dap.md (breakpoints and
    /// stepping)): the key is the matched map source record, or `""` for
    /// a request resolved against the global table. Also consulted
    /// directly by the stepping loop (`handle_step`) and the traced
    /// motions (`step_traced`/`run_traced`): `step_in`/`step_over`'s raw
    /// per-instruction path never checks `DebugSession`'s own breakpoint
    /// set (only the `continue`-shaped motions do — docs/core.md
    /// (DebugSession)), so a breakpoint hit mid-line has to be noticed
    /// here instead.
    source_breakpoints: BTreeMap<String, BTreeSet<u32>>,
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
            code: None,
            map: None,
            map_dir: None,
            alphabet: None,
            launch_opts: None,
            run_state: RunState::Stopped,
            source_breakpoints: BTreeMap::new(),
            instruction_breakpoints: BTreeSet::new(),
        }
    }

    /// The ONE place that pushes an `AdapterEvent::Stopped` — a single
    /// funnel keeps every stop's event shape (and anything a stop must
    /// ever do uniformly) at one site instead of eight.
    fn push_stopped(
        &mut self,
        out: &mut Vec<AdapterEvent>,
        reason: &'static str,
        description: Option<String>,
    ) {
        out.push(AdapterEvent::Stopped {
            reason,
            description,
        });
    }

    /// Dispatches on which of `"program"`/`"target"` the arguments carry
    /// (module doc) — the two are mutually exclusive, checked explicitly
    /// rather than letting `"target"` silently win, mirroring
    /// `pmt build`'s own "file inputs or target names, not both"
    /// rejection (`cli/driver.rs`).
    fn handle_launch(
        &mut self,
        arguments: &Value,
        out: &mut Vec<AdapterEvent>,
    ) -> Result<Value, String> {
        let has_program = arguments.get("program").and_then(Value::as_str).is_some();
        let has_target = arguments.get("target").and_then(Value::as_str).is_some();
        match (has_program, has_target) {
            (true, true) => Err(
                "launch: 'program' and 'target' are mutually exclusive — name exactly one"
                    .to_string(),
            ),
            (_, true) => self.handle_launch_target(arguments, out),
            _ => self.handle_launch_program(arguments, out),
        }
    }

    /// Program mode: a prebuilt `.pmx` used as-is (module doc).
    fn handle_launch_program(
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
        let trace = arguments
            .get("trace")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let strict_cells = arguments
            .get("strictCells")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let (tape, alphabet) = build_tape(tape_path.as_deref())?;
        self.finish_launch(
            ResolvedLaunch {
                program,
                tape_arg: tape_path,
                tape,
                alphabet,
                strict_cells,
                stop_on_entry,
                trace,
            },
            out,
        )
    }

    /// Target mode (module doc): builds `"target"` IN PROCESS through
    /// `cli::driver::build_target_for_launch`, always forcing `-g`.
    /// Diagnostics stream as `stderr` `Output` events — one per
    /// diagnostic line, per the seam's own contract — BEFORE this
    /// function reaches [`PmDapAdapter::finish_launch`], which is what
    /// pushes `Initialized`; a build failure returns `Err` before any of
    /// that ever runs, so a failed target launch pushes nothing at all,
    /// the same "no phantom initialization" contract program mode's own
    /// error paths already prove.
    fn handle_launch_target(
        &mut self,
        arguments: &Value,
        out: &mut Vec<AdapterEvent>,
    ) -> Result<Value, String> {
        let target_name = arguments
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| "launch requires a 'target' name".to_string())?;
        let project = arguments
            .get("project")
            .and_then(Value::as_str)
            .map(Path::new);
        let stop_on_entry = arguments
            .get("stopOnEntry")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let trace = arguments
            .get("trace")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let built = crate::cli::driver::build_target_for_launch(project, target_name, true)?;

        for line in built.diagnostics {
            out.push(AdapterEvent::Output {
                category: "stderr",
                output: line,
            });
        }

        let program = built.output.to_string_lossy().into_owned();
        self.finish_launch(
            ResolvedLaunch {
                program,
                tape_arg: None,
                tape: built.tape,
                alphabet: Some(built.alphabet),
                strict_cells: built.strict_cells,
                stop_on_entry,
                trace,
            },
            out,
        )
    }

    /// The shared tail of BOTH launch modes, once each has resolved its
    /// own executable path and initial tape: loads the `.pmx`, validates
    /// its arch byte, builds the `Machine`/`DebugSession`, wraps the tape
    /// in `StrictTape` when asked, discovers the sidecar map, and
    /// populates every session field. `Initialized` fires here — not from
    /// `initialize` itself, and not from either mode's own body — because
    /// readiness-for-configuration is genuinely gated on a program having
    /// loaded: a failure anywhere above this point returns `Err` and
    /// never reaches it, which an automatic post-`initialize` emission
    /// (fired before any program exists to configure against) could not
    /// distinguish from a successful one.
    fn finish_launch(
        &mut self,
        resolved: ResolvedLaunch,
        out: &mut Vec<AdapterEvent>,
    ) -> Result<Value, String> {
        let ResolvedLaunch {
            program,
            tape_arg,
            tape,
            alphabet,
            strict_cells,
            stop_on_entry,
            trace,
        } = resolved;

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

        let tape: Box<dyn Tape> = if strict_cells {
            Box::new(StrictTape::new(tape))
        } else {
            Box::new(tape)
        };
        let session = machine.debug(RunOptions::default());
        let map = sidecar_map(&program);
        let line_index = map.as_ref().map(LineIndex::new);
        // The anchor for the map's relative `source` entries: the
        // sidecar sits next to the executable, so its directory is the
        // program's — absolutized here (lexically, docs/formats.md (map
        // sidecar)) because the frames' `source` objects hand the editor
        // absolute paths.
        let map_dir = map.as_ref().and_then(|_| {
            let cwd = std::env::current_dir().unwrap_or_default();
            Path::new(&program)
                .parent()
                .map(|dir| mtc_core::source_path::lexical_absolute(&cwd, dir))
        });

        self.session = Some(session);
        self.tape = Some(tape);
        self.line_index = line_index;
        self.code = Some(exe.code.clone());
        self.map = map;
        self.map_dir = map_dir;
        self.alphabet = alphabet;
        self.launch_opts = Some(LaunchOpts {
            program,
            tape: tape_arg,
            stop_on_entry,
            trace,
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
        // Hoisted out of the `&self` borrow (rather than matching on
        // `launch_opts.stop_on_entry` directly) so `push_stopped`'s `&mut
        // self` below has nothing left to conflict with.
        let stop_on_entry = self
            .launch_opts
            .as_ref()
            .ok_or_else(|| "configurationDone before launch".to_string())?
            .stop_on_entry;
        // Mirrors `handle_continue`'s guard: a repeat `configurationDone`
        // after the program has already finished must not re-run
        // `finish()`'s termination events a second time.
        if self.run_state == RunState::Done {
            return Err("cannot configure: the program has already finished".to_string());
        }
        if stop_on_entry {
            self.push_stopped(out, "entry", None);
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
        self.push_stopped(out, "pause", None);
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
        if self.session.is_none() {
            return Err("setBreakpoints before launch".to_string());
        }
        // Resolved before the session borrow below (a `&self` method call
        // cannot overlap it). See `breakpoint_source_filter` for the
        // per-file rule.
        let source_filter = self.breakpoint_source_filter(arguments);
        let session = self.session.as_mut().expect("checked above");
        let requested = arguments
            .get("breakpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        // DAP's per-source REPLACE contract: this request's list is the
        // whole new set for ITS bucket — the matched file, or the one
        // global bucket (`""`) when the search is global. A `Foreign`
        // request owns no bucket: it can plant nothing, so it clears
        // nothing. An address is removed from the session only when no
        // other bucket (and no instruction breakpoint) still holds it.
        let bucket = match &source_filter {
            SourceFilter::Global => Some(String::new()),
            SourceFilter::File(raw) => Some(raw.clone()),
            SourceFilter::Foreign => None,
        };
        if let Some(key) = &bucket {
            for addr in self.source_breakpoints.remove(key).unwrap_or_default() {
                let still_owned = self.instruction_breakpoints.contains(&addr)
                    || self
                        .source_breakpoints
                        .values()
                        .any(|set| set.contains(&addr));
                if !still_owned {
                    session.remove_breakpoint(addr);
                }
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
            let planted = match &source_filter {
                SourceFilter::Foreign => None,
                SourceFilter::Global => self
                    .line_index
                    .as_ref()
                    .and_then(|idx| idx.address_for_line(line, None)),
                SourceFilter::File(raw) => self
                    .line_index
                    .as_ref()
                    .and_then(|idx| idx.address_for_line(line, Some(raw))),
            };
            match planted {
                Some(addr) => {
                    session.add_breakpoint(addr);
                    self.source_breakpoints
                        .entry(bucket.clone().unwrap_or_default())
                        .or_default()
                        .insert(addr);
                    let resolved_line = self
                        .line_index
                        .as_ref()
                        .and_then(|idx| idx.resolve(addr))
                        .and_then(|loc| loc.line)
                        .unwrap_or(line);
                    results.push(json!({
                        "verified": true,
                        "line": resolved_line,
                        "instructionReference": format!("0x{addr:x}"),
                    }));
                }
                None => {
                    let message = if matches!(source_filter, SourceFilter::Foreign) {
                        FOREIGN_SOURCE_BREAKPOINT_MESSAGE
                    } else {
                        UNMAPPED_BREAKPOINT_MESSAGE
                    };
                    results.push(json!({
                        "verified": false,
                        "line": line,
                        "message": message,
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
            let still_owned = self
                .source_breakpoints
                .values()
                .any(|set| set.contains(&addr));
            if !still_owned {
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

    fn is_traced(&self) -> bool {
        self.launch_opts.as_ref().is_some_and(|o| o.trace)
    }

    /// One retired instruction, `"trace": true`'s atomic primitive: the
    /// only session motion that exposes one instruction at a time is
    /// `step_in`, so every traced code path (`tick`'s `run_traced`,
    /// `next`/`stepIn`/`stepOut`'s `step_traced`) is built on repeated
    /// calls to this. Pushes one `Output` line per call — byte-format
    /// matching `pmt run --trace` (`drive_traced`, `cli/run.rs`), which
    /// writes unconditionally on every `step_in` call including the one
    /// that retires a terminal `stp`/`hlt` or the one that traps
    /// (`trace_streams_lines_with_post_state_into_the_writer` asserts the
    /// terminal `stp` line; `traced_trap_prints_the_faulting_line_exactly_once`
    /// asserts the faulting line is present) — EXCEPT when the session was
    /// *already* finished before this very call: `drive_traced` never
    /// calls `step_in` again once it observes a trap pause, so that state
    /// never arises there, but this adapter's two-phase trap flow does
    /// reach it (a further client `continue` after the `stopped("exception")`
    /// pause calls this again to reach `Finished`) — `DebugSession::step_in`
    /// short-circuits via its internal `gate()` in that case, retiring
    /// nothing and reporting the SAME faulting address again, which would
    /// otherwise print the fault line twice. `session.finished()` (checked
    /// BEFORE the call, since the call itself may set it) is the signal:
    /// `None` beforehand means a real instruction is about to retire.
    fn step_once_traced(&mut self, out: &mut Vec<AdapterEvent>) -> DebugEvent {
        let code = self
            .code
            .as_ref()
            .expect("trace requires code, set at launch");
        let map = self.map.as_ref();
        let session = self.session.as_mut().expect("trace requires a session");
        let tape = self.tape.as_deref_mut().expect("trace requires a tape");
        let already_finished = session.finished().is_some();
        let ip = session.ip();
        let event = session.step_in(tape);
        if !already_finished {
            let mf = session.mf();
            let head = tape.head();
            out.push(AdapterEvent::Output {
                category: "console",
                output: trace_output_line(code, map, ip, mf, head),
            });
        }
        event
    }

    /// `step_in`(`OneInstruction`)/`step_over`/`step_out`
    /// (`DepthAtMost(target)`)'s traced replacement, built on
    /// `step_once_traced`. `DepthAtMost` reimplements
    /// `DebugSession::run_until_tapes`'s own depth gate — trace mode needs
    /// the per-instruction visibility that primitive doesn't expose — and,
    /// like the untraced loop this parallels, checks the adapter-tracked
    /// breakpoint sets on every retired instruction's next address: this
    /// whole path is built on raw `step_in`, which (like `step_over`'s own
    /// per-instruction advance) never consults `DebugSession`'s internal
    /// breakpoint set.
    fn step_traced(&mut self, stop_when: StopWhen, out: &mut Vec<AdapterEvent>) -> DebugEvent {
        loop {
            let event = self.step_once_traced(out);
            let DebugEvent::Paused(PauseCause::Step) = event else {
                return event;
            };
            let session = self.session.as_ref().expect("checked by step_once_traced");
            let ip = session.ip();
            if self.source_breakpoint_at(ip) || self.instruction_breakpoints.contains(&ip) {
                return DebugEvent::Paused(PauseCause::Breakpoint(ip));
            }
            match stop_when {
                StopWhen::OneInstruction => return DebugEvent::Paused(PauseCause::Step),
                StopWhen::DepthAtMost(target) if session.depth() <= target => {
                    return DebugEvent::Paused(PauseCause::Step);
                }
                StopWhen::DepthAtMost(_) => {}
            }
        }
    }

    /// `run_steps`'s traced replacement, `tick`'s own budgeted analog of
    /// `step_traced` (a bounded slice, not a depth gate, so `next`'s
    /// call-skipping fast-forward and `continue`'s run share nothing but
    /// `step_once_traced`).
    fn run_traced(&mut self, budget: u64, out: &mut Vec<AdapterEvent>) -> DebugEvent {
        for _ in 0..budget {
            let event = self.step_once_traced(out);
            let DebugEvent::Paused(PauseCause::Step) = event else {
                return event;
            };
            let session = self.session.as_ref().expect("checked by step_once_traced");
            let ip = session.ip();
            if self.source_breakpoint_at(ip) || self.instruction_breakpoints.contains(&ip) {
                return DebugEvent::Paused(PauseCause::Breakpoint(ip));
            }
        }
        DebugEvent::Paused(PauseCause::Manual)
    }

    /// `next` (`StepKind::Over`) / `stepIn` (`StepKind::Into`): the user-
    /// facing granularity contract, including the without-`-g` degradation
    /// this loop produces, is documented at docs/dap.md (stepping
    /// granularity). Granularity toggles between two shapes over the SAME
    /// underlying session primitive (`step_over`/`step_in`) —
    /// `"instruction"` stops after exactly one session step; anything else
    /// (the default, and DAP's own default `"statement"`) repeats session
    /// steps until `LineIndex::resolve(ip)` differs from the position
    /// stepping started
    /// on, treating a transition into unmapped code (`Some` -> `None`) as
    /// a change too, so stepping never silently swallows a function with
    /// no `-g` data. The comparison is the WHOLE resolved position —
    /// function name AND line, not the line alone: two different
    /// functions can restart their line numbering at the same number
    /// (reachable once a second compilation unit, e.g. the stdlib, is
    /// linked in), and a line-only comparison would read stepping straight
    /// into such a function as "no change" and keep walking past it. A
    /// breakpoint/`brk`/trap interrupt wins over BOTH shapes and reports
    /// its own reason instead of the line's.
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
    /// that case uniformly with the loop's own check. Under `"trace": true`
    /// the same shape runs through `step_traced` instead, which
    /// reimplements `step_over`'s "retire one, then fast-forward only if
    /// that one was a call" structure by hand (see its own doc comment).
    fn handle_step(
        &mut self,
        arguments: &Value,
        out: &mut Vec<AdapterEvent>,
        kind: StepKind,
    ) -> Result<Value, String> {
        self.ensure_can_step()?;
        let instruction_granularity =
            arguments.get("granularity").and_then(Value::as_str) == Some("instruction");
        let traced = self.is_traced();

        // The walk's position: function name (owned, to avoid tying this
        // binding to `self.line_index`'s borrow across the loop) plus the
        // resolved line, if any — see the doc comment above for why the
        // name is part of the comparison, not just the line.
        let start_position: Option<(String, Option<u32>)> = if instruction_granularity {
            None
        } else {
            let ip = self
                .session
                .as_ref()
                .expect("checked by ensure_can_step")
                .ip();
            self.line_index
                .as_ref()
                .and_then(|idx| idx.resolve(ip))
                .map(|loc| (loc.function.to_string(), loc.line))
        };

        let outcome = loop {
            let event = if traced {
                match kind {
                    StepKind::Into => self.step_traced(StopWhen::OneInstruction, out),
                    StepKind::Over => {
                        let depth0 = self
                            .session
                            .as_ref()
                            .expect("checked by ensure_can_step")
                            .depth();
                        match self.step_once_traced(out) {
                            DebugEvent::Paused(PauseCause::Step) => {
                                let session =
                                    self.session.as_ref().expect("checked by ensure_can_step");
                                if session.depth() > depth0 {
                                    self.step_traced(StopWhen::DepthAtMost(depth0), out)
                                } else {
                                    DebugEvent::Paused(PauseCause::Step)
                                }
                            }
                            other => other,
                        }
                    }
                }
            } else {
                let session = self.session.as_mut().expect("checked by ensure_can_step");
                let tape = self
                    .tape
                    .as_deref_mut()
                    .expect("session and tape are set together");
                match kind {
                    StepKind::Over => session.step_over(tape),
                    StepKind::Into => session.step_in(tape),
                }
            };
            if let Some(outcome) = nonstep_outcome(event) {
                break outcome;
            }
            // A bare `Step` pause: decide whether to report it now or keep
            // stepping.
            let ip = self
                .session
                .as_ref()
                .expect("checked by ensure_can_step")
                .ip();
            if self.source_breakpoint_at(ip) || self.instruction_breakpoints.contains(&ip) {
                break StepOutcome::Stop("breakpoint", None);
            }
            if instruction_granularity {
                break StepOutcome::Stop("step", None);
            }
            let now_position: Option<(String, Option<u32>)> = self
                .line_index
                .as_ref()
                .and_then(|idx| idx.resolve(ip))
                .map(|loc| (loc.function.to_string(), loc.line));
            if now_position != start_position {
                break StepOutcome::Stop("step", None);
            }
        };
        self.apply_step_outcome(outcome, out);
        Ok(Value::Null)
    }

    /// `stepOut`: depth-based via `step_out_tapes`'s single-tape sibling
    /// `step_out` — ALWAYS one call, granularity does not apply: "step out
    /// of the current call" already names its own stopping point (the
    /// caller, one depth up), so there is no separate line-vs-instruction
    /// shape to choose between the way there is for `next`/`stepIn`. At the
    /// outermost frame (depth 0) `step_out` has no caller to return to —
    /// `DebugSession::step_out` falls back to running to completion in that
    /// case, not an error, and this handler inherits that behavior as-is
    /// (the traced path's analog: `run_traced` with no depth gate at all).
    fn handle_step_out(&mut self, out: &mut Vec<AdapterEvent>) -> Result<Value, String> {
        self.ensure_can_step()?;
        let event = if self.is_traced() {
            let depth = self
                .session
                .as_ref()
                .expect("checked by ensure_can_step")
                .depth();
            match depth.checked_sub(1) {
                Some(target) => self.step_traced(StopWhen::DepthAtMost(target), out),
                None => self.run_traced(u64::MAX, out),
            }
        } else {
            let session = self.session.as_mut().expect("checked by ensure_can_step");
            let tape = self
                .tape
                .as_deref_mut()
                .expect("session and tape are set together");
            session.step_out(tape)
        };
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
                self.push_stopped(out, reason, description);
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

    /// `stackTrace`: frame 0 is the current `ip()`; older frames come from
    /// `session.stack()` (return addresses, oldest call first), reversed so
    /// the most recent call is frame 1 — the natural top-to-bottom call
    /// stack order. Every frame resolves through the sidecar map the same
    /// way a breakpoint line does. Available in any run state (including
    /// after termination) — the session object outlives the run.
    fn handle_stack_trace(&self) -> Result<Value, String> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| "stackTrace before launch".to_string())?;
        let mut frames = vec![self.frame_json(0, session.ip())];
        for (i, &addr) in session.stack().iter().rev().enumerate() {
            frames.push(self.frame_json((i + 1) as i64, addr));
        }
        let total = frames.len();
        Ok(json!({"stackFrames": frames, "totalFrames": total}))
    }

    /// One `stackTrace` frame: a resolvable address names its containing
    /// function and (if mapped) source line; an unresolvable one (no
    /// sidecar, or an address outside every function) falls back to its
    /// hex address as the name, line 0. `instructionPointerReference` is
    /// always the hex address, resolvable regardless — `disassemble`'s
    /// `memoryReference` argument.
    fn frame_json(&self, id: i64, addr: u32) -> Value {
        let loc = self.line_index.as_ref().and_then(|idx| idx.resolve(addr));
        // An address before the function's first mapped instruction (a
        // linker-synthesized prelude) renders at the function's OPENING
        // line — the native-debugger prologue convention — so the frame
        // stays sourced and focusable. Only a function with no mapped
        // lines at all leaves `frame_line` empty.
        let frame_line = loc.as_ref().and_then(|l| l.line.or(l.function_first_line));
        let name = match &loc {
            Some(loc) => loc.function.to_string(),
            None => format!("0x{addr:04x}"),
        };
        let mut frame = json!({
            "id": id,
            "name": name,
            "line": frame_line.unwrap_or(0),
            "column": 0,
            "instructionPointerReference": format!("0x{addr:x}"),
        });
        // Source provenance (docs/dap.md (source provenance)): a frame
        // whose function record names its file gets a DAP `source`
        // object, which is what lets the client focus the frame and
        // highlight the line in the editor. Attached only when the
        // resolved file actually exists — a moved tree degrades to
        // today's sourceless frame instead of a dead editor tab — AND
        // only when a real line was emitted above: DAP's `line: 0` is
        // legal solely on a sourceless frame (1-based lines otherwise),
        // and a client maps a sourced line 0 to editor line -1.
        if let Some(path) = loc
            .filter(|_| frame_line.is_some())
            .and_then(|loc| loc.source)
            .and_then(|raw| self.resolved_source(raw))
        {
            frame["source"] = source_json(&path);
        }
        frame
    }

    /// A map `source` entry resolved to the absolute file the editor
    /// should open: relative entries anchor at the sidecar's directory
    /// (docs/formats.md (map sidecar)), and a file that does not exist
    /// resolves to `None` — the caller then omits provenance rather than
    /// handing the client a dead path.
    fn resolved_source(&self, raw: &str) -> Option<PathBuf> {
        let path = self.map_source_path(raw)?;
        fs::metadata(&path).is_ok().then_some(path)
    }

    /// Whether ANY source-breakpoint bucket holds `addr` — the stepping
    /// loop's view, which does not care which file planted a breakpoint,
    /// only that one exists at the position just reached.
    fn source_breakpoint_at(&self, addr: u32) -> bool {
        self.source_breakpoints
            .values()
            .any(|set| set.contains(&addr))
    }

    /// The same resolution without the existence gate — the breakpoint
    /// filter compares identities, and `source_identity` already treats
    /// a missing file gracefully (lexical fallback).
    fn map_source_path(&self, raw: &str) -> Option<PathBuf> {
        let dir = self.map_dir.as_ref()?;
        Some(mtc_core::source_path::lexical_absolute(dir, Path::new(raw)))
    }

    /// Classifies a `setBreakpoints` request's file against the map's
    /// source records — see [`SourceFilter`]. Identity is
    /// [`source_identity`]'s: canonicalized when the file exists (so a
    /// symlinked workspace still matches), lexical otherwise.
    fn breakpoint_source_filter(&self, arguments: &Value) -> SourceFilter {
        if !self.line_index.as_ref().is_some_and(LineIndex::has_sources) {
            return SourceFilter::Global;
        }
        let Some(request_path) = arguments
            .get("source")
            .and_then(|s| s.get("path"))
            .and_then(Value::as_str)
        else {
            return SourceFilter::Global;
        };
        let request = source_identity(Path::new(request_path));
        let raws = self
            .map
            .as_ref()
            .map(|map| map.functions.iter().filter_map(|f| f.source.as_deref()))
            .into_iter()
            .flatten();
        for raw in raws {
            let Some(resolved) = self.map_source_path(raw) else {
                continue;
            };
            if source_identity(&resolved) == request {
                return SourceFilter::File(raw.to_string());
            }
        }
        SourceFilter::Foreign
    }

    /// `scopes`: identical for any frame id (machine state is global), so
    /// the requested `frameId` is not even inspected beyond the session
    /// having launched.
    fn handle_scopes(&self) -> Result<Value, String> {
        if self.session.is_none() {
            return Err("scopes before launch".to_string());
        }
        Ok(json!({"scopes": [
            {"name": "Registers", "variablesReference": SCOPE_REGISTERS, "expensive": false},
            {"name": "Tapes", "variablesReference": SCOPE_TAPES, "expensive": false},
        ]}))
    }

    /// `variables`, dispatched on the requested `variablesReference` — see
    /// the module doc for the handle scheme. Available in any run state,
    /// same as `stackTrace` — a poked cell must stay readable after the
    /// program terminates (that's how the poke's persistence is proven,
    /// rather than by a snapshot API `dyn Tape` doesn't expose).
    fn handle_variables(&mut self, arguments: &Value) -> Result<Value, String> {
        let reference = arguments
            .get("variablesReference")
            .and_then(Value::as_i64)
            .ok_or_else(|| "variables requires a variablesReference".to_string())?;
        if self.session.is_none() {
            return Err("variables before launch".to_string());
        }
        match reference {
            SCOPE_REGISTERS => {
                let session = self.session.as_ref().expect("checked above");
                let stats = session.stats();
                Ok(json!({"variables": [
                    readonly_var("IP", format!("0x{:x}", session.ip())),
                    writable_var("MF", session.mf().to_string()),
                    readonly_var("steps", stats.steps.to_string()),
                    readonly_var("core tacts", stats.core_tacts.to_string()),
                    readonly_var("stall tacts", stats.stall_tacts.to_string()),
                ]}))
            }
            SCOPE_TAPES => {
                let tape = self.tape.as_deref().expect("checked above");
                Ok(json!({"variables": [{
                    "name": "tape 0",
                    "value": format!("head {}", tape.head()),
                    "variablesReference": TAPE_WINDOW_BASE,
                }]}))
            }
            TAPE_WINDOW_BASE => {
                let alphabet = self.alphabet.clone();
                let tape = self.tape.as_deref_mut().expect("checked above");
                let head_pos = tape.head();
                let cells = tape_window(tape);
                let vars: Vec<Value> = cells
                    .into_iter()
                    .map(|(pos, idx)| {
                        let marker = if pos == head_pos { "» " } else { "" };
                        json!({
                            "name": format!("{marker}[{pos}]"),
                            "value": quote_glyph(&glyph_for(&alphabet, idx)),
                            "variablesReference": 0,
                        })
                    })
                    .collect();
                Ok(json!({"variables": vars}))
            }
            _ => Err(format!("unknown variablesReference {reference}")),
        }
    }

    /// `setVariable` — ruled to set only while genuinely stopped (paused),
    /// not merely "not currently running" (excludes `Done` too: the
    /// program has nothing left to run the effect through, even though the
    /// tape stays readable via `variables`).
    fn handle_set_variable(&mut self, arguments: &Value) -> Result<Value, String> {
        if self.session.is_none() {
            return Err("setVariable before launch".to_string());
        }
        if self.run_state != RunState::Stopped {
            return Err("cannot set a variable: the program is not stopped".to_string());
        }
        let reference = arguments
            .get("variablesReference")
            .and_then(Value::as_i64)
            .ok_or_else(|| "setVariable requires a variablesReference".to_string())?;
        let name = arguments
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "setVariable requires a name".to_string())?;
        let value = arguments
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| "setVariable requires a value".to_string())?;

        match reference {
            SCOPE_REGISTERS => self.set_register_variable(name, value),
            TAPE_WINDOW_BASE => self.set_tape_variable(name, value),
            _ => Err(format!("cannot set a variable in scope {reference}")),
        }
    }

    /// `MF` is the one writable register (`DebugSession::set_mf`); `IP`
    /// and the read-only stat trio are rejected by name — overwriting `IP`
    /// can desynchronize the return stack, deferred until wanted.
    fn set_register_variable(&mut self, name: &str, value: &str) -> Result<Value, String> {
        match name {
            "MF" => {
                let mf = match value {
                    "true" => true,
                    "false" => false,
                    other => return Err(format!("MF must be 'true' or 'false', got '{other}'")),
                };
                let session = self.session.as_mut().expect("checked by caller");
                session.set_mf(mf);
                Ok(json!({"value": mf.to_string()}))
            }
            "IP" | "steps" | "core tacts" | "stall tacts" => Err(format!("{name} is read-only")),
            other => Err(format!("unknown register '{other}'")),
        }
    }

    /// A tape cell via `Tape::poke` — a `StrictTape`-launched session
    /// surfaces its fault text directly (`{fault:?}` — `DeviceFault` has
    /// no `Display`, matching this crate's existing fault-rendering
    /// convention); an unknown glyph is rejected before ever touching the
    /// tape, naming the legal glyphs.
    fn set_tape_variable(&mut self, name: &str, value: &str) -> Result<Value, String> {
        let pos =
            parse_cell_name(name).ok_or_else(|| format!("cannot parse cell name '{name}'"))?;
        let index = self.glyph_to_index(unquote_glyph(value))?;
        let tape = self.tape.as_deref_mut().expect("checked by caller");
        tape.poke(pos, index)
            .map_err(|fault| format!("{fault:?}"))?;
        Ok(json!({"value": quote_glyph(&glyph_for(&self.alphabet, index))}))
    }

    /// The reverse of `glyph_for`: a known alphabet requires an exact
    /// glyph match; an unknown alphabet (raw-index mode) accepts a decimal
    /// index within the tape's own `alphabet_size()`. Either way, an
    /// unrecognized value is rejected naming every legal glyph — never
    /// silently clamped or defaulted.
    fn glyph_to_index(&self, glyph: &str) -> Result<u32, String> {
        if let Some(alphabet) = &self.alphabet {
            return alphabet
                .iter()
                .position(|g| g == glyph)
                .map(|i| i as u32)
                .ok_or_else(|| {
                    format!(
                        "unknown glyph '{glyph}'; legal glyphs: {}",
                        alphabet
                            .iter()
                            .map(|g| format!("'{g}'"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                });
        }
        let size = self.tape.as_deref().map(|t| t.alphabet_size()).unwrap_or(0);
        glyph
            .parse::<u32>()
            .ok()
            .filter(|i| *i < size)
            .ok_or_else(|| {
                let legal: Vec<String> = (0..size).map(|i| i.to_string()).collect();
                format!(
                    "unknown glyph '{glyph}'; legal glyphs: {}",
                    legal.join(", ")
                )
            })
    }

    /// `disassemble`: renders `instructionCount` `listing_line` entries
    /// starting at `memoryReference` (+ byte `offset`, folded in first),
    /// shifted by `instructionOffset` instructions. `instructionOffset >=
    /// 0` (the common case — every frame's own `instructionPointerReference`
    /// at offset 0 included) walks forward from the base address directly,
    /// never needing to know where in the code image it falls. A negative
    /// offset needs a full linear ordinal index from address 0 first
    /// (mirrors `listing_executable`'s own linear walk) since instructions
    /// are variable-length and there is no way to know how many bytes
    /// "N instructions back" spans without decoding forward from a known
    /// boundary. That ordinal walk CLAMPS at the image start (`ord.max(0)`)
    /// rather than answering `None` for an offset that overshoots it — VS
    /// Code's real Disassembly-view request shape is exactly this
    /// (`instructionOffset: -50` from a frame near address 0), and the
    /// pre-clamp behavior returned an all-`<out of range>` window, the
    /// real code included, live-observed in VS Code. `None` (and every row
    /// answering the placeholder) stays reserved for a `memoryReference`
    /// address the index genuinely never contains. Once decoding runs off
    /// the end of the code image (either direction) after that, remaining
    /// entries answer a marked `<out of range>` placeholder rather than
    /// truncating the response short.
    fn handle_disassemble(&self, arguments: &Value) -> Result<Value, String> {
        let code = self
            .code
            .as_ref()
            .ok_or_else(|| "disassemble before launch".to_string())?;
        let reference = arguments
            .get("memoryReference")
            .and_then(Value::as_str)
            .ok_or_else(|| "disassemble requires a memoryReference".to_string())?;
        let base = parse_instruction_reference(reference)
            .ok_or_else(|| format!("invalid memoryReference: {reference}"))?;
        let byte_offset = arguments.get("offset").and_then(Value::as_i64).unwrap_or(0);
        let base = (i64::from(base) + byte_offset).max(0) as u32;
        let instruction_offset = arguments
            .get("instructionOffset")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let instruction_count = arguments
            .get("instructionCount")
            .and_then(Value::as_i64)
            .ok_or_else(|| "disassemble requires instructionCount".to_string())?;
        if instruction_count < 0 {
            return Err("instructionCount must not be negative".to_string());
        }

        let syntax = pm1_syntax();
        let map = self.map.as_ref();
        let resolve = |target: u32| resolve_label(map, target);

        // The whole instruction index, decoded once — both window
        // directions need ordinal arithmetic over it.
        let mut addrs = Vec::new();
        let mut addr = 0u32;
        let len = code.len() as u32;
        while addr < len {
            addrs.push(addr);
            let (_, ilen) = listing_line(&syntax, code, addr, &|_| None);
            addr += ilen.max(1);
        }

        // A `memoryReference` that is not an instruction boundary at all
        // (never issued by this adapter — a client-side invention) gets an
        // all-invalid window anchored at the raw address, one byte per row.
        let Some(idx) = addrs.iter().position(|&a| a == base) else {
            let instructions: Vec<Value> = (0..instruction_count)
                .map(|i| {
                    json!({
                        "address": format!("0x{:x}", u64::from(base) + i as u64),
                        "instruction": "<out of range>",
                        "presentationHint": "invalid",
                    })
                })
                .collect();
            return Ok(json!({"instructions": instructions}));
        };

        // The window is strictly POSITIONAL: row `i` is instruction
        // ordinal `idx + instructionOffset + i`, never shifted — VS
        // Code's Disassembly view learns a new reference's memory address
        // from the row at index `-instructionOffset` of the response, so
        // a window slid to the image start would teach it a wrong address
        // for every reference within `-instructionOffset` instructions of
        // the entry (the live symptom: the current-instruction marker
        // pinned to one late address forever). Ordinals outside the image
        // pad with `<out of range>` placeholders instead (docs/dap.md
        // (the Disassembly view)): before ordinal 0 the placeholder
        // addresses are NEGATIVE (`ord - 1`, so `-1` itself — the one
        // value clients treat as a skip-me sentinel — never appears),
        // past the last instruction they continue one byte at a time from
        // the code end. Addresses are strictly increasing and distinct
        // across the whole response either way.
        let mut instructions = Vec::new();
        for i in 0..instruction_count {
            let ord = idx as i64 + instruction_offset + i;
            if ord < 0 {
                instructions.push(json!({
                    "address": format!("-0x{:x}", 1 - ord),
                    "instruction": "<out of range>",
                    "presentationHint": "invalid",
                }));
            } else if let Some(&at) = addrs.get(ord as usize) {
                let (line, _) = listing_line(&syntax, code, at, &resolve);
                instructions.push(json!({
                    "address": format!("0x{at:x}"),
                    "instruction": line,
                }));
            } else {
                let past = ord as u64 - addrs.len() as u64;
                instructions.push(json!({
                    "address": format!("0x{:x}", u64::from(len) + past),
                    "instruction": "<out of range>",
                    "presentationHint": "invalid",
                }));
            }
        }
        Ok(json!({"instructions": instructions}))
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
            "initialize" => Ok(json!({
                "supportsConfigurationDoneRequest": true,
                "supportsSteppingGranularity": true,
                "supportsInstructionBreakpoints": true,
                "supportsSetVariable": true,
                "supportsDisassembleRequest": true,
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
            "stackTrace" => self.handle_stack_trace(),
            "scopes" => self.handle_scopes(),
            "variables" => self.handle_variables(arguments),
            "setVariable" => self.handle_set_variable(arguments),
            "disassemble" => self.handle_disassemble(arguments),
            "disconnect" => {
                self.run_state = RunState::Done;
                Ok(Value::Null)
            }
            other => Err(mtc_core::dap::server::unsupported_command(other)),
        }
    }

    fn tick(&mut self, out: &mut Vec<AdapterEvent>) -> RunState {
        if self.session.is_none() || self.tape.is_none() {
            self.run_state = RunState::Done;
            return RunState::Done;
        }
        let event = if self.is_traced() {
            self.run_traced(BUDGET, out)
        } else {
            let session = self.session.as_mut().expect("checked above");
            let tape = self.tape.as_deref_mut().expect("checked above");
            session.run_steps(tape, BUDGET)
        };
        match event {
            // Budget exhaustion, DebugSession's only `Manual` cause
            // (docs/core.md's `PauseCause` doc): invisible to the
            // client per the design's run-loop rule — stay Running.
            DebugEvent::Paused(PauseCause::Manual) => {}
            DebugEvent::Paused(PauseCause::Trap(trap)) => {
                self.push_stopped(out, "exception", Some(trap.to_string()));
                self.run_state = RunState::Stopped;
            }
            DebugEvent::Paused(PauseCause::Breakpoint(_)) => {
                self.push_stopped(out, "breakpoint", None);
                self.run_state = RunState::Stopped;
            }
            DebugEvent::Paused(PauseCause::Brk) => {
                self.push_stopped(out, "breakpoint", Some("debugger statement".to_string()));
                self.run_state = RunState::Stopped;
            }
            DebugEvent::Paused(PauseCause::Step) => {
                // Neither `run_steps` nor `run_traced` surfaces a bare Step
                // on its own (only budget exhaustion, a breakpoint, a brk,
                // or a trap end one of their iterations) — kept for
                // exhaustiveness against future `PauseCause` growth, mapped
                // the same as a manual step would be.
                self.push_stopped(out, "step", None);
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
/// `"tape"` argument is given (alphabet `None` — `variables` falls back
/// to raw indices, module doc); otherwise loads a `.pmt` block the same
/// way `pmt run --tape-block` does (a per-tape glyph table wins over the
/// block's shared fallback) and reports that effective alphabet.
fn build_tape(tape_path: Option<&str>) -> Result<(InfiniteTape, Option<Vec<String>>), String> {
    let Some(path) = tape_path else {
        return Ok((InfiniteTape::new(), None));
    };
    let bytes = fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let file = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let [snapshot] = file.tapes.as_slice() else {
        return Err(format!("{path}: PM-1 blocks hold exactly one tape"));
    };
    let tape = InfiniteTape::from_snapshot(snapshot).map_err(|e| format!("{path}: {e:?}"))?;
    let alphabet = snapshot
        .alphabet
        .clone()
        .unwrap_or_else(|| file.alphabet.clone());
    Ok((tape, Some(alphabet)))
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

/// The DAP `source` object for a resolved provenance path
/// (docs/dap.md (source provenance)): `name` is the display leaf, `path`
/// the absolute file the editor opens.
fn source_json(path: &Path) -> Value {
    json!({
        "name": path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        "path": path.display().to_string(),
    })
}

/// One file's identity for the breakpoint filter: canonicalized when the
/// file exists — a debug session compares paths from two independent
/// producers (the editor's request and the sidecar's records), and on a
/// symlinked tree those legitimately spell one file two ways — with the
/// purely lexical form as the fallback for a path that cannot be
/// canonicalized (docs/dap.md (source provenance)).
fn source_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        let cwd = std::env::current_dir().unwrap_or_default();
        mtc_core::source_path::lexical_absolute(&cwd, path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tape_defaults_to_empty_when_no_tape_path_is_given() {
        let (tape, alphabet) = build_tape(None).unwrap();
        assert_eq!(tape.head(), 0);
        assert_eq!(alphabet, None);
    }

    #[test]
    fn sidecar_map_is_none_for_a_missing_file() {
        assert!(sidecar_map("/definitely/not/a/real/path.pmx").is_none());
    }

    #[test]
    fn parse_cell_name_round_trips_the_rendered_format() {
        assert_eq!(parse_cell_name("[3]"), Some(3));
        assert_eq!(parse_cell_name("» [3]"), Some(3));
        assert_eq!(parse_cell_name("[-5]"), Some(-5));
        assert_eq!(parse_cell_name("nonsense"), None);
    }

    #[test]
    fn quote_and_unquote_glyph_round_trip() {
        assert_eq!(quote_glyph(" "), "' '");
        assert_eq!(unquote_glyph("' '"), " ");
        // A bare, unquoted value passes through unchanged.
        assert_eq!(unquote_glyph("*"), "*");
    }
}
