//! `TmDapAdapter`: the TM-1 half of the Debug Adapter Protocol surface
//! (mtc_core::dap), serving `tmt dap`. This is the TM mirror of
//! `mtc_post_machine::dap::PmDapAdapter` — same v1 lifecycle, run control,
//! termination, breakpoints, stepping, and state surface — over TM-1's
//! multi-tape, table-dispatched model, plus PM's two `launch` shapes
//! (`handle_launch` dispatches on which of `"program"`/`"target"` the
//! arguments carry, mirroring PM exactly). The user-facing contract both
//! adapters implement is documented at docs/dap.md.
//!
//! - **Program mode**: a prebuilt `.tmx` (`"program"`) plus a mandatory
//!   `.tmt` tape snapshot (`"tape"` — see the tape-required bullet below).
//! - **Target mode**: names a manifest target (`"target": "<name>"`) and
//!   an optional `"project"` path override (the discovery walk's starting
//!   point — `cli::driver::build_target_for_launch`'s own `current_dir`
//!   fallback otherwise). The target builds IN PROCESS, through that same
//!   `cli::driver` seam `tmt build TARGET` itself runs — never shelling
//!   out — always with `-g` forced regardless of the target's resolved
//!   profile. Its compile warnings stream as `stderr`-category `Output`
//!   events, one per diagnostic line, BEFORE `Initialized`; a failed
//!   build (or a failed tape load, module doc: the tape-only run-block
//!   rule below) fails the `launch` request with the driver's rendered
//!   error text — a failed target launch pushes nothing at all. Target
//!   mode has no `"tape"`/`"strictCells"` arguments of its own — the tape
//!   comes from the target's own `run` block, resolved through
//!   `cli::driver::build_target_for_launch` exactly as `tmt build --run`
//!   resolves it. **The tape-only run-block rule** (docs/tmt/project.md
//!   (run block)): unlike PM, TM-1 has no empty-tape default (the next
//!   bullet), so a target with no `run` block, or one whose `run` block
//!   declares no `tape`, is a launch-time error — the same two guards
//!   `tmt build --run` itself enforces via `run_once` (`cli/driver.rs`),
//!   never a phantom `Initialized`.
//!
//! Both modes share `"stopOnEntry"`/`"trace"`.
//!
//! ## TM specifics and deliberate deviations from the PM mirror
//!
//! - **Launch REQUIRES a tape block.** PM defaults to the empty tape when
//!   no `"tape"` argument is given; TM-1 has no such default (`tmt run`
//!   itself requires `--tape-block` — module doc, `cli/run.rs`). A
//!   program-mode launch with no `"tape"` argument is therefore a clean
//!   launch-time error, same shape as every other pre-`Initialized` launch
//!   failure: `out` stays empty, nothing is claimed ready.
//! - **No `"strictCells"` launch argument.** `tmt run` has no
//!   `--strict-cells` flag at all (`cli/run.rs`'s own module doc: "no
//!   `--head`, no `--strict-cells`") — PM-1's strict-cells decorator has no
//!   TM-1 CLI precedent to mirror, so this adapter does not invent one.
//!   Every tape is a plain `WideTape`; there is no `Box<dyn Tape>`
//!   indirection to allow a decorator to wrap it (contrast PM's `tape:
//!   Box<dyn Tape>` field, which exists ONLY to let `"strictCells"` wrap
//!   the concrete tape without a second field or an enum).
//! - **Multi-tape, not single-tape.** `tapes: Vec<WideTape>` replaces PM's
//!   single `tape` field; every session-driving call assembles a fresh
//!   `Vec<&mut dyn Tape>` device slice on demand (mirrors
//!   `cli::run::execute_run`/`drive_traced`'s own `devices` construction),
//!   rather than storing one. The Tapes scope lists one child per tape
//!   (`"tape 0"`, `"tape 1"`, …); expanding tape `n` reaches its own head±8
//!   window at `variablesReference = TAPE_WINDOW_BASE + n` — the exact
//!   scheme PM's own module doc predicts for a multi-tape adapter reusing
//!   its handle scheme ("`TAPE_WINDOW_BASE + 1`, `+ 2`, … for its other
//!   tapes").
//! - **Registers: `IP`, `MR` (writable), `FR` (read-only, frames-profile
//!   images only), then the read-only `steps`/`core tacts`/`stall tacts`
//!   trio** — replacing PM's `IP`/`MF`. `MR` is the SAME one general
//!   register `MF`/`set_mf` view in `DebugSession`
//!   (docs/core.md (registers)); TM-1 exposes it as a `u32` value
//!   (`session.mr()`/`session.set_mr()`) rather than PM's boolean `MF`
//!   view, matching how TM-1 programs actually use it (multi-way `mtc`/
//!   `djmp` table dispatch, not a single check flag). `FR` — the frame
//!   register — is present in the Registers scope ONLY when the launched
//!   image's execution profile is `PROFILE_FRAMES`; a base-profile image
//!   shows no `FR` variable at all, matching how `tmt run --trace`
//!   conditionally appends ` FR=<n>` (`cli/run.rs::drive_traced`). Like
//!   PM's `IP`, both `IP` and `FR` stay read-only — overwriting either can
//!   desynchronize engine-internal state, deferred until wanted.
//! - **No initial-mark latch to step past.** PM-1's `DebugSession` latches
//!   an initial MF from the tape's head cell on its very first step (the
//!   PM-1 loading step), which can clobber an `MF` `setVariable` issued
//!   right at the `stopOnEntry` pause before anything has run — PM's own
//!   tests step past `ent` first to dodge it. TM-1 sessions are always
//!   built via `Machine::debug_tapes`, which routes through
//!   `DebugSession::with_tables` and clears `latch_initial_mark`
//!   (docs/core.md (registers); `core::vm::debug`) — MR starts at 0 and
//!   nothing overwrites a `set_mr` issued at the entry pause. There is no
//!   TM analog of PM's "step past `ent` first" workaround.
//! - **Registry-backed loading, not `Machine::with_arch`.** PM-1 images
//!   are always the v1 code-only shape, so `PmDapAdapter` builds its
//!   `Machine` via `Machine::with_arch(&'static Pm1, …)` — a bare unit
//!   struct behind a `static`, since `Pm1` is trivially `Sync`. A TM-1
//!   image is ALWAYS the full v2 shape (multi-tape, a table ROM, and
//!   sometimes the frames profile): the fields that carry that shape
//!   (`tables`/`tape_count`/`profile`/`alphabet_cardinalities`/
//!   `frames_offset`) are private to `core::vm::machine` outside
//!   `with_arch`'s v1-only defaults, so only `Machine::from_executable`
//!   populates them, and that takes a borrowed `&'a ArchRegistry`.
//!   `ArchRegistry` holds `Vec<Box<dyn Arch>>` — `dyn Arch` carries no
//!   `Send`/`Sync` bound, so (mirroring PM's own module doc) it cannot sit
//!   behind a `static` directly. `Box::leak` sidesteps that: it needs only
//!   `'static`, not `Sync` (unlike a `static` item), so
//!   `leaked_tm1_registry` builds one once per adapter instance — real
//!   usage (`tmt dap`) constructs exactly one adapter per process, so this
//!   is a single, small, deliberate, permanent leak. The two-tape fixture
//!   this module's tests launch is the actual proof this is load-bearing,
//!   not just a private-fields argument: under `Machine::with_arch` the
//!   image would load with `tape_count: 1`, and any `wr`/`mov` targeting
//!   device 1 would trap `BadOperand` on first execution.
//! - **Trace lines carry every head, plus an `FR=` suffix on frames
//!   images.** `trace_output_line` renders `heads=[h0, h1, …]` instead of
//!   PM's single `head=N`, and appends ` FR=<n>` exactly when the launched
//!   image's profile is `PROFILE_FRAMES` — byte-format matching `tmt run
//!   --trace` (`cli::run::drive_traced`). The already-finished short-circuit
//!   in `step_once_traced` — needed so the two-phase trap flow (a further
//!   client `continue` after the `stopped("exception")` pause) does not
//!   re-print the faulting line a second time — is the exact nuance
//!   `PmDapAdapter::step_once_traced`'s own doc comment describes; nothing
//!   about it is PM-specific, so it applies here unchanged.
//! - **Launch validates the tape block against the executable's own tape
//!   header** (band count, then per-tape alphabet cardinality) — a TM-only
//!   concern with no PM analog, since PM images are always single-tape.
//!   Mirrors `cli::run::execute_run`'s own checks byte-for-byte (a
//!   standalone copy, same reasoning as `sidecar_map` below: that logic is
//!   private to `cli::run`).
//!
//! Everything else — the breakpoint sets, the stepping-granularity walk
//! (including the function-identity tuple comparison, not just the line),
//! the `Stopped`/`Finished` mapping, the termination summary and exit
//! codes, the `variablesReference` handle scheme's scope constants, and
//! `disassemble` — mirrors `PmDapAdapter` structurally; only the
//! per-instruction/per-register plumbing changes shape for multiple tapes.
//!
//! **Handle-scheme generation salt** (shared, unchanged, from
//! `PmDapAdapter`'s module doc — see its "Generation salt" section for the
//! full reasoning): every `variablesReference` this adapter issues —
//! `SCOPE_REGISTERS`, `SCOPE_TAPES`, and `TAPE_WINDOW_BASE + n` for each of
//! its tapes — and every stack-frame `id` `stackTrace` hands back are
//! salted by the current `stop_generation` (`salted`, incremented by
//! exactly one per `Stopped` event through the shared `push_stopped`
//! choke point — every `AdapterEvent::Stopped` push in this module goes
//! through it). Without the salt, a step or pause left the handles
//! genuinely IDENTICAL to the previous stop's even though the underlying
//! scopes/frames had moved, and VS Code's own per-reference cache kept
//! rendering the prior stop's Variables/Call Stack values instead of
//! re-requesting them — live-observed in VS Code. Decoding
//! (`handle_variables`/`handle_set_variable`) accepts ANY generation
//! (`base = raw % GENERATION_STRIDE`) rather than only the current one, so
//! a reference a client still holds from the stop just before this one
//! keeps resolving live data instead of erroring as stale. Frame ids get
//! the same salt for the same cache-busting reason even though nothing in
//! this adapter decodes one back (`scopes`'s dispatch takes no `arguments`
//! at all). `GENERATION_STRIDE` stays comfortably above every base either
//! adapter issues: `TAPE_WINDOW_BASE + n` tops out at `100 + 255` (a TM-1
//! tape count is a `u8`), still under the 4096 stride. `stop_generation`
//! is never reset by `finish_launch` on a re-launch — zeroing it would
//! reissue generation-1 handles a client may still have cached from the
//! PRIOR session, exactly the staleness this salt exists to prevent.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use mtc_core::asm::listing_line;
use mtc_core::dap::server::{AdapterEvent, DebugAdapter, RunState};
use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeBlockFile;
use mtc_core::formats::{ARCH_TM1, PROFILE_BASE, PROFILE_FRAMES};
use mtc_core::linemap::LineIndex;
use mtc_core::linker::MapFile;
use mtc_core::vm::{
    ArchRegistry, DebugEvent, DebugSession, Machine, Outcome, PauseCause, RunOptions, Tape,
    WideTape,
};

use crate::arch::Tm1;
use crate::asm::tm1_syntax;

/// Fixed `variablesReference` handles — see the module doc for the full
/// scheme (mirrors PM's constants; `TAPE_WINDOW_BASE + n` reaches tape
/// `n`'s window, unlike PM which only ever issues the base itself).
const SCOPE_REGISTERS: i64 = 1;
const SCOPE_TAPES: i64 = 2;
const TAPE_WINDOW_BASE: i64 = 100;

/// The generation-salt stride (module doc's "Handle-scheme generation
/// salt" section). Mirrors `PmDapAdapter::GENERATION_STRIDE`.
const GENERATION_STRIDE: i64 = 4096;

// A TM-1 tape count is a `u8`, so `TAPE_WINDOW_BASE + n` tops out at
// `100 + 255` — the widest base either adapter ever issues. Pins the
// module doc's "comfortably above every base" claim.
const _: () = assert!(TAPE_WINDOW_BASE + 255 < GENERATION_STRIDE);

/// Half-width of a tape variables window (`TAPE_WINDOW_BASE + n`): head±8,
/// 17 cells total. Same radius PM uses.
const TAPE_WINDOW_RADIUS: i64 = 8;

/// Per-tick step slice `tick` drives the session through
/// (`session.run_steps_tapes(devices, BUDGET)`): see `PmDapAdapter`'s own
/// `BUDGET` doc — identical reasoning, unbounded run apart from this
/// responsiveness slice.
const BUDGET: u64 = 10_000;

/// `handle_set_breakpoints`'s answer for a line with no mapped code —
/// verbatim from `PmDapAdapter` (arch-neutral wording).
const UNMAPPED_BREAKPOINT_MESSAGE: &str =
    "no code at this line — build with -g and place the breakpoint on an executable line";

/// `handle_set_breakpoints`'s answer for a file the map's source records
/// never name — verbatim from `PmDapAdapter` (arch-neutral wording).
const FOREIGN_SOURCE_BREAKPOINT_MESSAGE: &str =
    "no code in this program comes from this file (per the map sidecar's source records)";

/// `next`'s underlying primitive steps OVER a call; `stepIn`'s steps INTO
/// one. Mirrors `PmDapAdapter::StepKind`.
#[derive(Clone, Copy)]
enum StepKind {
    Over,
    Into,
}

/// How a `setBreakpoints` request's file constrains the line search.
/// Mirrors `PmDapAdapter`'s `SourceFilter` exactly (docs/dap.md
/// (breakpoints and stepping)).
enum SourceFilter {
    Global,
    File(String),
    Foreign,
}

/// What a stepping request settled on. Mirrors `PmDapAdapter::StepOutcome`.
enum StepOutcome {
    Stop(&'static str, Option<String>),
    Finished(Outcome),
}

/// `step_traced`'s stopping rule. Mirrors `PmDapAdapter::StopWhen`.
#[derive(Clone, Copy)]
enum StopWhen {
    OneInstruction,
    DepthAtMost(usize),
}

/// Every `DebugEvent` shape that is NOT a bare `Step` pause, converted to a
/// stepping outcome. Mirrors `PmDapAdapter::nonstep_outcome` exactly — the
/// underlying `DebugEvent`/`PauseCause` types are shared, arch-agnostic
/// core types.
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
        DebugEvent::Paused(PauseCause::Manual) => Some(StepOutcome::Stop("step", None)),
        DebugEvent::Finished(outcome) => Some(StepOutcome::Finished(outcome)),
    }
}

/// `"0x…"`/`"0X…"` (or bare hex, no prefix) -> address. Mirrors
/// `PmDapAdapter::parse_instruction_reference`.
fn parse_instruction_reference(reference: &str) -> Option<u32> {
    let digits = reference
        .strip_prefix("0x")
        .or_else(|| reference.strip_prefix("0X"))
        .unwrap_or(reference);
    u32::from_str_radix(digits, 16).ok()
}

/// The label-resolve closure `drive_traced`'s TM twin uses
/// (`cli/run.rs`): a call site's exact target names its function; any
/// other resolved address names `function.label`. Mirrors
/// `PmDapAdapter::resolve_label` — same `MapFile` shape, arch-agnostic.
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

/// One `"trace": true` output line, byte-format matching `tmt run
/// --trace`'s (`cli::run::drive_traced`): the `listing_line` text (or a
/// synthetic line for a fetch beyond the code image) plus the
/// post-instruction `MF`/heads/(frames) `FR` suffix. TM deviation from
/// `PmDapAdapter::trace_output_line` (module doc): every head renders, and
/// `fr_suffix` is pre-computed by the caller (empty on a base-profile
/// image).
fn trace_output_line(
    code: &[u8],
    map: Option<&MapFile>,
    ip: u32,
    mf: bool,
    heads: &[i64],
    fr_suffix: &str,
) -> String {
    let syntax = tm1_syntax();
    let resolve = |target: u32| resolve_label(map, target);
    let line = if (ip as usize) < code.len() {
        listing_line(&syntax, code, ip, &resolve).0
    } else {
        format!("  {ip:04x}:  <beyond code image>")
    };
    let heads_str = heads
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{line}  ; MF={} heads=[{heads_str}]{fr_suffix}",
        u8::from(mf)
    )
}

/// Reads the `pos`±`TAPE_WINDOW_RADIUS` window around one tape's current
/// head, restoring the head to where it started. Mirrors
/// `PmDapAdapter::tape_window` exactly — the walk-and-restore discipline
/// is per-tape, not per-adapter, so it takes any `&mut dyn Tape` regardless
/// of how many sibling tapes exist.
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

/// A cell's rendered glyph from a tape's own alphabet. TM deviation from
/// `PmDapAdapter::glyph_for`: a TM launch tape is mandatory (module doc),
/// so every tape's alphabet is always known — there is no PM-style
/// raw-index fallback mode, and the parameter is a plain slice rather than
/// an `Option`.
fn glyph_for(alphabet: &[String], index: u32) -> String {
    alphabet
        .get(index as usize)
        .cloned()
        .unwrap_or_else(|| index.to_string())
}

/// `variables`' rendering of a glyph, quoted. Mirrors
/// `PmDapAdapter::quote_glyph`.
fn quote_glyph(glyph: &str) -> String {
    format!("'{glyph}'")
}

/// The inverse of `quote_glyph`. Mirrors `PmDapAdapter::unquote_glyph`.
fn unquote_glyph(value: &str) -> &str {
    value
        .strip_prefix('\'')
        .and_then(|v| v.strip_suffix('\''))
        .unwrap_or(value)
}

/// A tape-window variable's `name` back to its position. Mirrors
/// `PmDapAdapter::parse_cell_name`.
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

/// Loads one `.tmt` band as a `WideTape`, sized to its own effective
/// alphabet (its own override, else the block's shared fallback) — mirrors
/// `cli::run::execute_run`'s own `alphabets`/`tapes` construction. Guards
/// the two ways a bad or degenerate tape block would otherwise panic
/// through `WideTape::new`'s `1..=256` assert (an empty alphabet, or one
/// wider than the `u8` snapshot-cell ceiling) rather than let a malformed
/// `.tmt` file kill the adapter process.
fn build_tapes(tape_path: &str) -> Result<(Vec<WideTape>, Vec<Vec<String>>), String> {
    let bytes = fs::read(tape_path).map_err(|e| format!("cannot read {tape_path}: {e}"))?;
    let file = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{tape_path}: {e}"))?;
    let alphabets: Vec<Vec<String>> = file
        .tapes
        .iter()
        .map(|t| t.alphabet.clone().unwrap_or_else(|| file.alphabet.clone()))
        .collect();
    let mut tapes = Vec::with_capacity(file.tapes.len());
    for (i, (snap, alphabet)) in file.tapes.iter().zip(&alphabets).enumerate() {
        let width = alphabet.len();
        if width == 0 {
            return Err(format!("{tape_path}: tape {i} declares an empty alphabet"));
        }
        if width > 256 {
            return Err(format!(
                "{tape_path}: tape {i} declares {width} glyphs, past the 256-glyph ceiling"
            ));
        }
        let tape = WideTape::from_snapshot(snap, width as u32)
            .map_err(|e| format!("{tape_path}: tape {i}: {e:?}"))?;
        tapes.push(tape);
    }
    Ok((tapes, alphabets))
}

/// `<program>.map` sidecar discovery — a standalone copy of
/// `cli::inspect::sidecar_map`'s logic (private to `cli`); a missing or
/// unparsable sidecar degrades to no line info rather than failing the
/// launch. Mirrors `PmDapAdapter::sidecar_map`.
fn sidecar_map(program: &str) -> Option<MapFile> {
    let mut sidecar = std::ffi::OsString::from(program);
    sidecar.push(".map");
    fs::read_to_string(sidecar)
        .ok()
        .and_then(|text| MapFile::from_json(&text).ok())
}

/// The DAP `source` object for a resolved provenance path — mirrors
/// `PmDapAdapter`'s `source_json` (docs/dap.md (source provenance)).
fn source_json(path: &Path) -> Value {
    json!({
        "name": path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        "path": path.display().to_string(),
    })
}

/// One file's identity for the breakpoint filter — mirrors
/// `PmDapAdapter`'s `source_identity` (docs/dap.md (source provenance)).
fn source_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        let cwd = std::env::current_dir().unwrap_or_default();
        mtc_core::source_path::lexical_absolute(&cwd, path)
    })
}

/// One `'static` `ArchRegistry` carrying the one `Tm1` this adapter ever
/// needs — see the module doc for why `Box::leak` (needs `'static`, not
/// `Sync`) rather than a `static` item (needs `Sync`, which `ArchRegistry`
/// is not). `Tm1::new`'s `tape_count` argument is validated but never
/// retained (`arch/mod.rs`'s own doc comment), so `1` — always in range —
/// is as good as any real tape count here.
fn leaked_tm1_registry() -> &'static ArchRegistry {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(1)));
    Box::leak(Box::new(registry))
}

/// What `handle_launch`'s program-mode resolution produces before
/// `TmDapAdapter::finish_launch` takes over. Mirrors PM's
/// `ResolvedLaunch`, minus `strict_cells` (no TM analog, module doc) and
/// with a plural `tapes`/`alphabets` in place of PM's single tape.
struct ResolvedLaunch {
    program: String,
    tape_path: String,
    tapes: Vec<WideTape>,
    alphabets: Vec<Vec<String>>,
    stop_on_entry: bool,
    trace: bool,
}

/// The resolved `launch` request's state, kept for the session's life.
/// Mirrors `PmDapAdapter::LaunchOpts`, with `tape` mandatory (a plain
/// `String`, not PM's `Option<String>`).
struct LaunchOpts {
    #[allow(dead_code)] // carried for later tasks (e.g. re-launch, summaries)
    program: String,
    #[allow(dead_code)]
    tape: String,
    stop_on_entry: bool,
    trace: bool,
}

/// The TM-1 debug adapter — see the module doc for the full TM-specifics
/// list. Nothing is populated before a successful `launch`.
pub struct TmDapAdapter {
    registry: &'static ArchRegistry,
    session: Option<DebugSession<'static>>,
    tapes: Vec<WideTape>,
    line_index: Option<LineIndex>,
    /// The launched executable's code image — retained for `disassemble`
    /// and the trace renderer, same reason as PM's own `code` field.
    code: Option<Vec<u8>>,
    map: Option<MapFile>,
    /// The sidecar's directory, lexically absolutized at launch — the
    /// anchor the map's relative `source` entries resolve against.
    /// Mirrors `PmDapAdapter::map_dir`.
    map_dir: Option<PathBuf>,
    /// Every tape's own glyph table, index-aligned with `tapes` — always
    /// fully populated once launched (module doc: the launch tape is
    /// mandatory, unlike PM's optional one).
    alphabets: Vec<Vec<String>>,
    /// The launched image's execution profile (`PROFILE_BASE` or
    /// `PROFILE_FRAMES`) — gates whether `FR` appears in the Registers
    /// scope and whether trace lines carry the ` FR=` suffix.
    profile: u8,
    launch_opts: Option<LaunchOpts>,
    run_state: RunState,
    /// Addresses this adapter added on behalf of `setBreakpoints`,
    /// bucketed per source file. Mirrors
    /// `PmDapAdapter::source_breakpoints` — same reasoning (kept separate
    /// from instruction breakpoints; DAP's per-source REPLACE contract;
    /// consulted directly by the stepping loop since
    /// `step_in_tapes`/`step_over_tapes` never check `DebugSession`'s
    /// own breakpoint set).
    source_breakpoints: BTreeMap<String, BTreeSet<u32>>,
    /// Addresses added on behalf of `setInstructionBreakpoints`. Mirrors
    /// `PmDapAdapter::instruction_breakpoints`.
    instruction_breakpoints: BTreeSet<u32>,
    /// The current stop's generation, salted into every issued
    /// `variablesReference`/frame `id`. Mirrors
    /// `PmDapAdapter::stop_generation` exactly — never reset by
    /// `finish_launch`.
    stop_generation: u64,
}

impl Default for TmDapAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl TmDapAdapter {
    pub fn new() -> Self {
        TmDapAdapter {
            registry: leaked_tm1_registry(),
            session: None,
            tapes: Vec::new(),
            line_index: None,
            code: None,
            map: None,
            map_dir: None,
            alphabets: Vec::new(),
            profile: PROFILE_BASE,
            launch_opts: None,
            run_state: RunState::Stopped,
            source_breakpoints: BTreeMap::new(),
            instruction_breakpoints: BTreeSet::new(),
            stop_generation: 0,
        }
    }

    /// Salts a handle-scheme base constant with the CURRENT stop
    /// generation. Mirrors `PmDapAdapter::salted` exactly.
    fn salted(&self, base: i64) -> i64 {
        self.stop_generation as i64 * GENERATION_STRIDE + base
    }

    /// The ONE place allowed to push an `AdapterEvent::Stopped`. Mirrors
    /// `PmDapAdapter::push_stopped` exactly.
    fn push_stopped(
        &mut self,
        out: &mut Vec<AdapterEvent>,
        reason: &'static str,
        description: Option<String>,
    ) {
        self.stop_generation += 1;
        out.push(AdapterEvent::Stopped {
            reason,
            description,
        });
    }

    /// Dispatches on which of `"program"`/`"target"` the arguments carry
    /// (module doc) — the two are mutually exclusive, checked explicitly
    /// rather than letting `"target"` silently win, mirroring
    /// `PmDapAdapter::handle_launch` exactly.
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

    /// Program mode: a prebuilt `.tmx` used as-is (module doc).
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
        // TM deviation (module doc): "tape" is mandatory — there is no
        // empty-tape default, so a missing argument is rejected here,
        // before anything (including this executable) is even read.
        let tape_path = arguments
            .get("tape")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "launch requires a 'tape' path — a TM-1 program has no empty-tape default"
                    .to_string()
            })?
            .to_string();
        let stop_on_entry = arguments
            .get("stopOnEntry")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let trace = arguments
            .get("trace")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let (tapes, alphabets) = build_tapes(&tape_path)?;
        self.finish_launch(
            ResolvedLaunch {
                program,
                tape_path,
                tapes,
                alphabets,
                stop_on_entry,
                trace,
            },
            out,
        )
    }

    /// Target mode (module doc): builds `"target"` IN PROCESS through
    /// `cli::driver::build_target_for_launch`, always forcing `-g`. The
    /// tape-only run-block rule (module doc) surfaces here as
    /// `build_target_for_launch` returning `Err` before ANY event ever
    /// reaches `out` — no diagnostics, no `Initialized`, the same "no
    /// phantom initialization" contract program mode's own error paths
    /// already prove. Order past that point deliberately loads the tape
    /// BEFORE pushing diagnostics: a malformed `run`-declared tape file
    /// (a failure `build_target_for_launch` itself cannot see — module
    /// doc on `DapTargetBuild`) then also pushes nothing, rather than
    /// leaving stray `Output` events in `out` ahead of a failed launch.
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
        let (tapes, alphabets) = build_tapes(&built.tape_path)?;

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
                tape_path: built.tape_path,
                tapes,
                alphabets,
                stop_on_entry,
                trace,
            },
            out,
        )
    }

    /// The shared tail of launch resolution: loads the `.tmx`, validates
    /// its arch byte, validates the tape band against the image's own tape
    /// header (band count, then per-tape cardinality — mirrors
    /// `cli::run::execute_run`, module doc), builds the `Machine` through
    /// the leaked registry (module doc: `Machine::with_arch` cannot carry
    /// the full v2 shape), discovers the sidecar map, and populates every
    /// session field. `Initialized` fires here, on the same "readiness is
    /// genuinely gated on a loaded program" reasoning as
    /// `PmDapAdapter::finish_launch`.
    fn finish_launch(
        &mut self,
        resolved: ResolvedLaunch,
        out: &mut Vec<AdapterEvent>,
    ) -> Result<Value, String> {
        let ResolvedLaunch {
            program,
            tape_path,
            tapes,
            alphabets,
            stop_on_entry,
            trace,
        } = resolved;

        let bytes = fs::read(&program).map_err(|e| format!("cannot read {program}: {e}"))?;
        let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{program}: {e}"))?;
        if exe.arch != ARCH_TM1 {
            return Err(format!(
                "{program}: not a TM-1 executable (arch byte {:#04x})",
                exe.arch
            ));
        }

        let expected = usize::from(exe.tape_count);
        if tapes.len() != expected {
            return Err(format!(
                "{tape_path} has {} tape(s), but {program} expects {expected}",
                tapes.len(),
            ));
        }
        for (i, tape) in tapes.iter().enumerate() {
            if let Some(&declared) = exe.alphabet_cardinalities.get(i) {
                let got = tape.alphabet_size();
                if got != declared {
                    return Err(format!(
                        "{tape_path}: tape {i} has {got} glyph(s), but {program} expects {declared}",
                    ));
                }
            }
        }

        let machine =
            Machine::from_executable(&exe, self.registry).map_err(|e| format!("{program}: {e}"))?;
        let session = machine.debug_tapes(RunOptions::default());
        let map = sidecar_map(&program);
        let line_index = map.as_ref().map(LineIndex::new);
        // The anchor for the map's relative `source` entries — mirrors
        // `PmDapAdapter::finish_launch` (docs/formats.md (map sidecar)).
        let map_dir = map.as_ref().and_then(|_| {
            let cwd = std::env::current_dir().unwrap_or_default();
            Path::new(&program)
                .parent()
                .map(|dir| mtc_core::source_path::lexical_absolute(&cwd, dir))
        });

        self.session = Some(session);
        self.tapes = tapes;
        self.line_index = line_index;
        self.code = Some(exe.code.clone());
        self.map = map;
        self.map_dir = map_dir;
        self.alphabets = alphabets;
        self.profile = exe.profile;
        self.launch_opts = Some(LaunchOpts {
            program,
            tape: tape_path,
            stop_on_entry,
            trace,
        });
        self.run_state = RunState::Stopped;
        self.source_breakpoints.clear();
        self.instruction_breakpoints.clear();
        // `stop_generation` is deliberately NOT reset here — module doc's
        // "Handle-scheme generation salt" section: zeroing it on a
        // re-launch would reissue generation-1 handles a client may still
        // have cached from the PRIOR session.

        out.push(AdapterEvent::Initialized);
        Ok(Value::Null)
    }

    /// Mirrors `PmDapAdapter::handle_configuration_done` exactly.
    fn handle_configuration_done(&mut self, out: &mut Vec<AdapterEvent>) -> Result<Value, String> {
        // Hoisted out of the `&self` borrow so `push_stopped`'s `&mut
        // self` below has nothing left to conflict with — mirrors
        // `PmDapAdapter::handle_configuration_done`.
        let stop_on_entry = self
            .launch_opts
            .as_ref()
            .ok_or_else(|| "configurationDone before launch".to_string())?
            .stop_on_entry;
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

    /// Mirrors `PmDapAdapter::handle_continue` exactly.
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

    /// Mirrors `PmDapAdapter::handle_pause` exactly.
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

    /// Mirrors `PmDapAdapter::handle_set_breakpoints` exactly — arch-neutral
    /// logic over the shared `LineIndex`.
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

        // DAP's per-source REPLACE contract — mirrors
        // `PmDapAdapter::handle_set_breakpoints` exactly (docs/dap.md
        // (breakpoints and stepping)).
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

    /// Mirrors `PmDapAdapter::handle_set_instruction_breakpoints` exactly.
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

    /// Mirrors `PmDapAdapter::ensure_can_step` exactly.
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

    /// One retired instruction, `"trace": true`'s atomic primitive.
    /// Mirrors `PmDapAdapter::step_once_traced` — see that doc comment for
    /// the already-finished short-circuit's full reasoning (the
    /// two-phase-trap nuance is identical here; the module doc only notes
    /// the render-shape deviation).
    fn step_once_traced(&mut self, out: &mut Vec<AdapterEvent>) -> DebugEvent {
        let code = self
            .code
            .as_ref()
            .expect("trace requires code, set at launch");
        let map = self.map.as_ref();
        let profile = self.profile;
        let session = self.session.as_mut().expect("trace requires a session");
        let mut devices: Vec<&mut dyn Tape> =
            self.tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
        let already_finished = session.finished().is_some();
        let ip = session.ip();
        let event = session.step_in_tapes(&mut devices);
        if !already_finished {
            let mf = session.mf();
            let heads: Vec<i64> = devices.iter().map(|d| d.head()).collect();
            let fr_suffix = if profile == PROFILE_FRAMES {
                format!(" FR={}", session.fr())
            } else {
                String::new()
            };
            out.push(AdapterEvent::Output {
                category: "console",
                output: trace_output_line(code, map, ip, mf, &heads, &fr_suffix),
            });
        }
        event
    }

    /// Mirrors `PmDapAdapter::step_traced` exactly.
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

    /// Mirrors `PmDapAdapter::run_traced` exactly.
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

    /// `next`/`stepIn` (docs/dap.md (stepping granularity) for the
    /// user-facing contract). Mirrors `PmDapAdapter::handle_step` exactly —
    /// the function-identity tuple comparison (name AND line, not line
    /// alone) applies unchanged; only the untraced branch's session calls
    /// become the `_tapes` siblings over a freshly assembled device slice.
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
                let mut devices: Vec<&mut dyn Tape> =
                    self.tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
                match kind {
                    StepKind::Over => session.step_over_tapes(&mut devices),
                    StepKind::Into => session.step_in_tapes(&mut devices),
                }
            };
            if let Some(outcome) = nonstep_outcome(event) {
                break outcome;
            }
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

    /// `stepOut`. Mirrors `PmDapAdapter::handle_step_out` exactly.
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
            let mut devices: Vec<&mut dyn Tape> =
                self.tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
            session.step_out_tapes(&mut devices)
        };
        let outcome = nonstep_outcome(event).unwrap_or(StepOutcome::Stop("step", None));
        self.apply_step_outcome(outcome, out);
        Ok(Value::Null)
    }

    /// Mirrors `PmDapAdapter::apply_step_outcome` exactly.
    fn apply_step_outcome(&mut self, outcome: StepOutcome, out: &mut Vec<AdapterEvent>) {
        match outcome {
            StepOutcome::Stop(reason, description) => {
                self.push_stopped(out, reason, description);
                self.run_state = RunState::Stopped;
            }
            StepOutcome::Finished(outcome) => self.finish(outcome, out),
        }
    }

    /// Termination path (docs/tmt/cli.md's `tmt run` exit-code mapping).
    /// Mirrors `PmDapAdapter::finish` exactly — no tape contents in the
    /// summary either, same as PM (the closed output-events list, spec
    /// §5); tape state stays queryable via `variables` after `Done`.
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

    /// Mirrors `PmDapAdapter::handle_stack_trace` exactly.
    fn handle_stack_trace(&self) -> Result<Value, String> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| "stackTrace before launch".to_string())?;
        let mut frames = vec![self.frame_json(self.salted(0), session.ip())];
        for (i, &addr) in session.stack().iter().rev().enumerate() {
            frames.push(self.frame_json(self.salted((i + 1) as i64), addr));
        }
        let total = frames.len();
        Ok(json!({"stackFrames": frames, "totalFrames": total}))
    }

    /// Mirrors `PmDapAdapter::frame_json` exactly (docs/dap.md (source
    /// provenance)).
    fn frame_json(&self, id: i64, addr: u32) -> Value {
        let loc = self.line_index.as_ref().and_then(|idx| idx.resolve(addr));
        let (name, line) = match &loc {
            Some(loc) => (loc.function.to_string(), loc.line.unwrap_or(0)),
            None => (format!("0x{addr:04x}"), 0),
        };
        let mut frame = json!({
            "id": id,
            "name": name,
            "line": line,
            "column": 0,
            "instructionPointerReference": format!("0x{addr:x}"),
        });
        if let Some(path) = loc
            .and_then(|loc| loc.source)
            .and_then(|raw| self.resolved_source(raw))
        {
            frame["source"] = source_json(&path);
        }
        frame
    }

    /// Mirrors `PmDapAdapter::source_breakpoint_at` exactly.
    fn source_breakpoint_at(&self, addr: u32) -> bool {
        self.source_breakpoints
            .values()
            .any(|set| set.contains(&addr))
    }

    /// Mirrors `PmDapAdapter::resolved_source` exactly.
    fn resolved_source(&self, raw: &str) -> Option<PathBuf> {
        let path = self.map_source_path(raw)?;
        fs::metadata(&path).is_ok().then_some(path)
    }

    /// Mirrors `PmDapAdapter::map_source_path` exactly.
    fn map_source_path(&self, raw: &str) -> Option<PathBuf> {
        let dir = self.map_dir.as_ref()?;
        Some(mtc_core::source_path::lexical_absolute(dir, Path::new(raw)))
    }

    /// Mirrors `PmDapAdapter::breakpoint_source_filter` exactly.
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

    /// Mirrors `PmDapAdapter::handle_scopes` exactly.
    fn handle_scopes(&self) -> Result<Value, String> {
        if self.session.is_none() {
            return Err("scopes before launch".to_string());
        }
        Ok(json!({"scopes": [
            {"name": "Registers", "variablesReference": self.salted(SCOPE_REGISTERS), "expensive": false},
            {"name": "Tapes", "variablesReference": self.salted(SCOPE_TAPES), "expensive": false},
        ]}))
    }

    /// `variables`, dispatched on the requested `variablesReference` — see
    /// the module doc for the multi-tape handle scheme
    /// (`TAPE_WINDOW_BASE + n`) and the `IP`/`MR`/`FR` register set. TM
    /// deviation from PM's dispatch: the Tapes scope lists every tape, not
    /// one, and the tape-window arm is a RANGE match, not a single constant.
    fn handle_variables(&mut self, arguments: &Value) -> Result<Value, String> {
        let raw_reference = arguments
            .get("variablesReference")
            .and_then(Value::as_i64)
            .ok_or_else(|| "variables requires a variablesReference".to_string())?;
        if self.session.is_none() {
            return Err("variables before launch".to_string());
        }
        // Decode: dispatch on the base only, ANY generation (module doc's
        // "Handle-scheme generation salt" section) — a stale reference
        // must still resolve live data. The error arm below echoes
        // `raw_reference`, not this decoded value, so an unrecognized
        // handle is reported exactly as the client sent it.
        let reference = raw_reference % GENERATION_STRIDE;
        if reference == SCOPE_REGISTERS {
            let session = self.session.as_ref().expect("checked above");
            let stats = session.stats();
            let mut vars = vec![
                readonly_var("IP", format!("0x{:x}", session.ip())),
                writable_var("MR", session.mr().to_string()),
            ];
            if self.profile == PROFILE_FRAMES {
                vars.push(readonly_var("FR", session.fr().to_string()));
            }
            vars.push(readonly_var("steps", stats.steps.to_string()));
            vars.push(readonly_var("core tacts", stats.core_tacts.to_string()));
            vars.push(readonly_var("stall tacts", stats.stall_tacts.to_string()));
            return Ok(json!({"variables": vars}));
        }
        if reference == SCOPE_TAPES {
            let vars: Vec<Value> = self
                .tapes
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    json!({
                        "name": format!("tape {i}"),
                        "value": format!("head {}", t.head()),
                        "variablesReference": self.salted(TAPE_WINDOW_BASE + i as i64),
                    })
                })
                .collect();
            return Ok(json!({"variables": vars}));
        }
        if let Some(idx) = self.tape_window_index(reference) {
            let alphabet = self.alphabets[idx].clone();
            let tape = &mut self.tapes[idx];
            let head_pos = tape.head();
            let cells = tape_window(tape);
            let vars: Vec<Value> = cells
                .into_iter()
                .map(|(pos, glyph_index)| {
                    let marker = if pos == head_pos { "» " } else { "" };
                    json!({
                        "name": format!("{marker}[{pos}]"),
                        "value": quote_glyph(&glyph_for(&alphabet, glyph_index)),
                        "variablesReference": 0,
                    })
                })
                .collect();
            return Ok(json!({"variables": vars}));
        }
        Err(format!("unknown variablesReference {raw_reference}"))
    }

    /// A `variablesReference` in the tape-window range back to which tape
    /// index it names, or `None` outside the range this adapter's own
    /// tapes occupy (`TAPE_WINDOW_BASE .. TAPE_WINDOW_BASE + tapes.len()`).
    fn tape_window_index(&self, reference: i64) -> Option<usize> {
        let idx = reference - TAPE_WINDOW_BASE;
        if idx >= 0 && (idx as usize) < self.tapes.len() {
            Some(idx as usize)
        } else {
            None
        }
    }

    /// Mirrors `PmDapAdapter::handle_set_variable`'s structure; the tape
    /// arm dispatches through `tape_window_index` instead of a single
    /// constant.
    fn handle_set_variable(&mut self, arguments: &Value) -> Result<Value, String> {
        if self.session.is_none() {
            return Err("setVariable before launch".to_string());
        }
        if self.run_state != RunState::Stopped {
            return Err("cannot set a variable: the program is not stopped".to_string());
        }
        let raw_reference = arguments
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

        // Decode: same rule as `handle_variables` — dispatch on the base,
        // ANY generation.
        let reference = raw_reference % GENERATION_STRIDE;
        if reference == SCOPE_REGISTERS {
            return self.set_register_variable(name, value);
        }
        if let Some(idx) = self.tape_window_index(reference) {
            return self.set_tape_variable(idx, name, value);
        }
        Err(format!("cannot set a variable in scope {raw_reference}"))
    }

    /// `MR` is the one writable register (`DebugSession::set_mr`); `IP`,
    /// `FR`, and the read-only stat trio are rejected by name — TM
    /// deviation from PM's `set_register_variable`: `FR` joins `IP` in the
    /// read-only list (module doc: overwriting either can desynchronize
    /// engine-internal state, deferred until wanted), and the writable
    /// register parses a `u32` rather than PM's `true`/`false` boolean.
    fn set_register_variable(&mut self, name: &str, value: &str) -> Result<Value, String> {
        match name {
            "MR" => {
                let mr: u32 = value
                    .parse()
                    .map_err(|_| format!("MR must be a non-negative integer, got '{value}'"))?;
                let session = self.session.as_mut().expect("checked by caller");
                session.set_mr(mr);
                Ok(json!({"value": mr.to_string()}))
            }
            "IP" | "FR" | "steps" | "core tacts" | "stall tacts" => {
                Err(format!("{name} is read-only"))
            }
            other => Err(format!("unknown register '{other}'")),
        }
    }

    /// A tape cell via `Tape::poke` on tape `idx`. Mirrors
    /// `PmDapAdapter::set_tape_variable`, routed to one of several tapes
    /// instead of the single PM tape.
    fn set_tape_variable(&mut self, idx: usize, name: &str, value: &str) -> Result<Value, String> {
        let pos =
            parse_cell_name(name).ok_or_else(|| format!("cannot parse cell name '{name}'"))?;
        let index = self.glyph_to_index(idx, unquote_glyph(value))?;
        self.tapes[idx]
            .poke(pos, index)
            .map_err(|fault| format!("{fault:?}"))?;
        Ok(json!({"value": quote_glyph(&glyph_for(&self.alphabets[idx], index))}))
    }

    /// The reverse of `glyph_for` for tape `idx`. TM deviation from
    /// `PmDapAdapter::glyph_to_index`: no raw-index fallback branch — the
    /// launch tape's alphabet is always known (module doc), so an
    /// unrecognized glyph is always rejected by exact match, naming every
    /// legal glyph.
    fn glyph_to_index(&self, idx: usize, glyph: &str) -> Result<u32, String> {
        let alphabet = &self.alphabets[idx];
        alphabet
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
            })
    }

    /// Mirrors `PmDapAdapter::handle_disassemble` exactly (including the
    /// negative-`instructionOffset` clamp — see that doc comment for the
    /// full reasoning) — `tm1_syntax()` in place of `pm1_syntax()` is the
    /// only difference.
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

        let syntax = tm1_syntax();
        let map = self.map.as_ref();
        let resolve = |target: u32| resolve_label(map, target);

        let start_addr = if instruction_offset >= 0 {
            let mut addr = base;
            for _ in 0..instruction_offset {
                if (addr as usize) >= code.len() {
                    break;
                }
                let (_, ilen) = listing_line(&syntax, code, addr, &|_| None);
                addr += ilen.max(1);
            }
            Some(addr)
        } else {
            let mut addrs = Vec::new();
            let mut addr = 0u32;
            let len = code.len() as u32;
            while addr < len {
                addrs.push(addr);
                let (_, ilen) = listing_line(&syntax, code, addr, &|_| None);
                addr += ilen.max(1);
            }
            // Clamp rather than fail past the image start — see
            // `PmDapAdapter::handle_disassemble`'s doc comment.
            addrs.iter().position(|&a| a == base).and_then(|idx| {
                let ord = (idx as i64 + instruction_offset).max(0) as usize;
                addrs.get(ord).copied()
            })
        };

        let mut cursor = start_addr.unwrap_or(base);
        let mut in_range = start_addr.is_some();
        let mut instructions = Vec::new();
        for _ in 0..instruction_count {
            if in_range && (cursor as usize) < code.len() {
                let (line, ilen) = listing_line(&syntax, code, cursor, &resolve);
                instructions.push(json!({
                    "address": format!("0x{cursor:x}"),
                    "instruction": line,
                }));
                cursor += ilen.max(1);
            } else {
                in_range = false;
                instructions.push(json!({
                    "address": format!("0x{cursor:x}"),
                    "instruction": "<out of range>",
                    "presentationHint": "invalid",
                }));
                cursor = cursor.wrapping_add(1);
            }
        }
        Ok(json!({"instructions": instructions}))
    }
}

impl DebugAdapter for TmDapAdapter {
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
        if self.session.is_none() {
            self.run_state = RunState::Done;
            return RunState::Done;
        }
        let event = if self.is_traced() {
            self.run_traced(BUDGET, out)
        } else {
            let session = self.session.as_mut().expect("checked above");
            let mut devices: Vec<&mut dyn Tape> =
                self.tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
            session.run_steps_tapes(&mut devices, BUDGET)
        };
        match event {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_map_is_none_for_a_missing_file() {
        assert!(sidecar_map("/definitely/not/a/real/path.tmx").is_none());
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
        assert_eq!(unquote_glyph("*"), "*");
    }

    #[test]
    fn build_tapes_rejects_an_empty_alphabet_instead_of_panicking() {
        let dir = std::env::temp_dir().join(format!(
            "tm-dap-empty-alpha-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let block = TapeBlockFile {
            alphabet: Vec::new(),
            tapes: vec![mtc_core::formats::tapeblock::TapeSnapshot {
                origin: 0,
                cells: vec![0],
                head: 0,
                alphabet: None,
            }],
        };
        let path = dir.join("empty.tmt");
        std::fs::write(&path, block.to_bytes().unwrap()).unwrap();
        let err = build_tapes(path.to_str().unwrap()).unwrap_err();
        assert!(err.contains("empty alphabet"), "got: {err}");
    }
}
