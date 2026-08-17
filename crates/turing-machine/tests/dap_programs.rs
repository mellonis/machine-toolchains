//! `TmDapAdapter` scripted-conversation tests, mirroring
//! `mtc-post-machine`'s `dap_programs.rs` structure over TM-1's
//! multi-tape, table-dispatched model: `handle`/`tick` driven directly (no
//! stdio) against tiny fixture `.tmx` + `.tmt` files built in-process and
//! written to a pid+counter scratch dir (the `lint_programs.rs` isolation
//! pattern).
//!
//! TM-only scope this file covers that has no PM analog: the mandatory
//! launch tape (no empty-tape default), per-tape alphabet windows +
//! cross-tape poke isolation, the `MR` general register (steered
//! behaviorally through a hand-authored dispatch table, not just
//! echoed), `FR`'s frames-profile gating, band-count/cardinality launch
//! errors, and a hand-assembled `.tma` fixture proving the "debug at
//! assembly-line granularity for free" bonus.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use mtc_core::dap::server::{AdapterEvent, DebugAdapter, RunState};
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_turing_machine::asm::{assemble, link};
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::dap::TmDapAdapter;

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter.
fn scratch(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("dap-{name}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Assembles hand-written `.tma` (no debug info) and links it standalone,
/// writing the resulting `.tmx` into `dir`.
fn write_tmx(dir: &Path, name: &str, tma_source: &str, opts: LinkOptions) -> PathBuf {
    let obj = assemble(tma_source, false).unwrap();
    let out = link(&[obj], &[], opts).unwrap();
    let path = dir.join(format!("{name}.tmx"));
    fs::write(&path, out.executable.to_bytes()).unwrap();
    path
}

/// Assembles hand-written `.tma` WITH debug info — its own `-g` line table
/// carries the `.tma` file's OWN assembly lines (no source remap, unlike a
/// compiled `.tmc`) — links it, and writes both the `.tmx` and its
/// `.tmx.map` sidecar. The fixture this module's assembly-line-granularity
/// test exercises.
fn write_tmx_debug_asm(dir: &Path, name: &str, tma_source: &str) -> PathBuf {
    let obj = assemble(tma_source, true).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    let path = dir.join(format!("{name}.tmx"));
    fs::write(&path, out.executable.to_bytes()).unwrap();
    let mut map_path = path.clone().into_os_string();
    map_path.push(".map");
    fs::write(&map_path, out.map.to_json()).unwrap();
    path
}

/// Compiles `.tmc` source WITH debug info (remapped to `.tmc` source
/// lines), links it, and writes both the `.tmx` and its sidecar.
fn write_tmc_debug(dir: &Path, name: &str, tmc_source: &str) -> PathBuf {
    write_tmc_debug_multi(dir, name, &[tmc_source])
}

/// `write_tmc_debug`'s multi-compilation-unit sibling: compiles EACH
/// source separately (its own file, its own line numbering restarting at
/// 1) and links them together.
fn write_tmc_debug_multi(dir: &Path, name: &str, tmc_sources: &[&str]) -> PathBuf {
    let objects: Vec<_> = tmc_sources
        .iter()
        .map(|source| {
            compile(
                source,
                CompileOptions {
                    debug_info: true,
                    ..Default::default()
                },
            )
            .unwrap()
            .object
        })
        .collect();
    let linked = link(&objects, &[], LinkOptions::default()).unwrap();
    let path = dir.join(format!("{name}.tmx"));
    fs::write(&path, linked.executable.to_bytes()).unwrap();
    let mut map_path = path.clone().into_os_string();
    map_path.push(".map");
    fs::write(&map_path, linked.map.to_json()).unwrap();
    path
}

/// The 1-based physical line number of the first line containing `needle`.
fn line_of(source: &str, needle: &str) -> u32 {
    source.lines().position(|l| l.contains(needle)).unwrap() as u32 + 1
}

/// A single-tape `.tmt` block, `width`-symbol alphabet (glyphs named
/// numerically past the blank), seeded with `seed` at the head.
fn write_one_tape_block(dir: &Path, name: &str, width: u32, seed: u8) -> PathBuf {
    let mut alphabet = vec!["_".to_string()];
    for i in 1..width {
        alphabet.push(i.to_string());
    }
    let block = TapeBlockFile {
        alphabet,
        tapes: vec![TapeSnapshot {
            origin: 0,
            cells: vec![seed],
            head: 0,
            alphabet: None,
        }],
    };
    let path = dir.join(format!("{name}.tmt"));
    fs::write(&path, block.to_bytes().unwrap()).unwrap();
    path
}

/// A 2-tape `.tmt` block: tape 0 seeded with `num_seed` under `{_, 0, 1}`,
/// tape 1 seeded with `aux_seed` under a DIFFERENT `{_, a, b}` alphabet —
/// deliberately distinct glyph tables so a `variables` test can prove
/// per-tape (not shared) rendering.
fn write_two_tape_block(dir: &Path, name: &str, num_seed: u8, aux_seed: u8) -> PathBuf {
    let block = TapeBlockFile {
        alphabet: vec!["_".to_string(), "0".to_string(), "1".to_string()],
        tapes: vec![
            TapeSnapshot {
                origin: 0,
                cells: vec![num_seed],
                head: 0,
                alphabet: None,
            },
            TapeSnapshot {
                origin: 0,
                cells: vec![aux_seed],
                head: 0,
                alphabet: Some(vec!["_".to_string(), "a".to_string(), "b".to_string()]),
            },
        ],
    };
    let path = dir.join(format!("{name}.tmt"));
    fs::write(&path, block.to_bytes().unwrap()).unwrap();
    path
}

fn launch_args(program: &Path, tape: &Path, stop_on_entry: bool) -> Value {
    json!({
        "program": program.to_str().unwrap(),
        "tape": tape.to_str().unwrap(),
        "stopOnEntry": stop_on_entry,
    })
}

/// Drives `tick` while the adapter reports `Running`, collecting every
/// pushed event, stopping the instant it reports `Stopped` or `Done`.
fn drive_to_pause_or_done(adapter: &mut TmDapAdapter) -> Vec<AdapterEvent> {
    let mut events = Vec::new();
    for _ in 0..10_000 {
        match adapter.run_state() {
            RunState::Running => {
                adapter.tick(&mut events);
            }
            RunState::Stopped | RunState::Done => return events,
        }
    }
    panic!("drive_to_pause_or_done: adapter never left Running (events so far: {events:?})");
}

/// A single `next`/`stepIn` instruction-granularity step, asserting it
/// reports exactly one `stopped("step")` and returning nothing — a
/// terser helper for the many tests that just need to walk forward a
/// known number of raw instructions.
fn step_instruction(adapter: &mut TmDapAdapter, command: &str) {
    let mut out = Vec::new();
    adapter
        .handle(command, &json!({"granularity": "instruction"}), &mut out)
        .unwrap();
    assert_eq!(
        out,
        vec![AdapterEvent::Stopped {
            reason: "step",
            description: None,
        }],
        "instruction step on '{command}' did not land cleanly: {out:?}"
    );
}

fn scope_ref(scopes_response: &Value, name: &str) -> i64 {
    scopes_response["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no '{name}' scope in {scopes_response:?}"))["variablesReference"]
        .as_i64()
        .unwrap()
}

fn variable_ref(variables_response: &Value, name: &str) -> i64 {
    variables_response["variables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .unwrap_or_else(|| panic!("no '{name}' variable in {variables_response:?}"))
        ["variablesReference"]
        .as_i64()
        .unwrap()
}

fn variable_value<'a>(variables_response: &'a Value, name: &str) -> &'a str {
    variables_response["variables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .unwrap_or_else(|| panic!("no '{name}' variable in {variables_response:?}"))["value"]
        .as_str()
        .unwrap()
}

fn has_variable(variables_response: &Value, name: &str) -> bool {
    variables_response["variables"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["name"] == name)
}

fn registers(adapter: &mut TmDapAdapter) -> Value {
    let mut out = Vec::new();
    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers_ref = scope_ref(&scopes, "Registers");
    adapter
        .handle(
            "variables",
            &json!({"variablesReference": registers_ref}),
            &mut out,
        )
        .unwrap()
}

fn steps_count(adapter: &mut TmDapAdapter) -> u64 {
    let vars = registers(adapter);
    variable_value(&vars, "steps").parse().unwrap()
}

// ---- fixtures: hand-written `.tma`, single tape --------------------------

const STP_TMA: &str = "\
.routine main, tapes=1, alpha=(2)
.section code
.func main
        stp
";

const HLT_TMA: &str = "\
.routine main, tapes=1, alpha=(2)
.section code
.func main
        hlt
";

/// `ret` on an empty return stack traps `StackUnderflow` — the entry's
/// own implicit `ent` (rides in via `.func`) retires normally first.
const TRAP_TMA: &str = "\
.routine main, tapes=1, alpha=(2)
.section code
.func main
        ret
";

/// Never `stp`/`hlt`/traps on its own — used only by the `pause` test.
const LOOP_TMA: &str = "\
.routine main, tapes=1, alpha=(2)
.section code
.func main
L1:     mov [>]
        jmp L1
";

const BRK_TMA: &str = "\
.routine main, tapes=1, alpha=(2)
.section code
.func main
        brk
        stp
";

/// Three retired instructions beyond the implicit `ent`: `nop`, `nop`,
/// `stp` — FOUR `step_in` calls total (mirrors PM's `TRACE_PROGRAM`).
const TRACE_TMA: &str = "\
.routine main, tapes=1, alpha=(2)
.section code
.func main
        nop
        nop
        stp
";

/// A hand-authored dispatch table with KNOWN row values (no empirical
/// derivation needed, unlike a `.tmc`-compiled table): row 1 (`[1]`)
/// matches a seeded '1' cell and goes to `branchA` (a no-op `stp`); row 2
/// (`[2]`) goes to `branchB` (writes symbol 2, THEN `stp`). Seeding the
/// tape with symbol 1 makes `branchA` the NATURAL outcome — the MR
/// behavioral test overrides `MR` from 1 to 2 between `mtc` and `djmp`
/// and proves the override steers `djmp` to `branchB` instead.
const MR_TMA: &str = "\
.routine main, tapes=1, alpha=(3)
.section tables
T:  .row    [1]
    .row    [2]
D:  .targets branchA, branchB
.section code
.func main
        rd
        mtc     T
        djmp    D
branchA:
        stp
branchB:
        wr      [2]
        stp
";

/// A bound-call program linked with `--call-mech=frames`, forcing the
/// frames execution profile (mirrors `composition_engine.rs`'s
/// `TWO_CONTEXT`) — `main` calls the SAME `writer` routine under two
/// different symbol-swap bindings.
const FRAMES_TMA: &str = "\
.routine main, tapes=1, alpha=(4)
.routine writer, tapes=1, alpha=(4)
.section code
.func main
        call    writer [0{1->2, 2->1}]
        call    writer [0{1->3, 3->1}]
        stp
.func writer
        wr      [1]
        mov     [>]
        ret
";

fn frames_opts() -> LinkOptions {
    LinkOptions {
        call_mech: CallMech::Frames,
        ..Default::default()
    }
}

// ---- fixtures: compiled `.tmc`, two tapes with a bound call --------------

/// `scan`'s three rows: `'0'` calls `mark` (writes an `'a'` onto `aux`,
/// returns) then continues at `done0`; `'1'` writes `'b'` onto `aux`
/// directly (no call) then goes to `done1`; `'_'` stops outright.
/// Compiled layout (empirically stable, but every test below locates
/// positions via `line_of`/response introspection rather than hand-derived
/// addresses): `scan`'s `rd`/`mtc`/`djmp` all derive from its own
/// `entry state scan` decl line (mirrors `compiler.rs`'s own
/// `debug_info_remaps_object_lines_to_tmc_sources` test, which pins this
/// exact rule — a state's dispatch preamble maps to the state's own decl
/// line, and each rule's body maps to that rule's own line).
const CALLSTEP_TMC: &str = "\
alphabet bits { '_', '0', '1' }
alphabet abc  { '_', 'a', 'b' }

export routine mark(tape t: abc) {
  entry state m {
    [*] -> write ['a'] return;
  }
}

machine {
  tape num: bits;
  tape aux: abc;

  entry state scan {
    ['0', *] -> call mark(t = aux) then done0;
    ['1', *] -> write [-, 'b'] goto done1;
    ['_', *] -> stop;
  }

  state done0 { [*, *] -> stop; }
  state done1 { [*, *] -> stop; }
}
";

/// Two SEPARATE compilation units, each with its own line numbering
/// restarting at 1 — deliberately laid out (verified empirically against
/// this exact pair) so `mark`'s write/return rule (callee line 5, the
/// SAME instruction range twice: once mid-body, once at the `ret` that
/// crosses back to the caller) lands on the SAME number as the caller's
/// OWN `done` state's `stop` (caller line 5) once linked into one address
/// space — the collision a line-only comparison would misread as "no
/// change". The cross-unit call is bindless (`call mark() then done;`) —
/// `docs/tmt/language.md`'s own rule: a call whose target lives in another
/// compilation unit must not bind tapes explicitly, since this unit does
/// not have the callee's signature; the linker resolves it (identity
/// placement, matching arity 1-to-1).
const RETURN_CALLER_TMC: &str = "\
alphabet bits { '_', '0', '1' }
use mark;
machine { tape num: bits;
  entry state main { [*] -> call mark() then done; }
  state done { [*] -> stop; }
}
";

const RETURN_CALLEE_TMC: &str = "\
alphabet bits { '_', '0', '1' }

export routine mark(tape t: bits) {
  entry state m {
    [*] -> write ['1'] return;
  }
}
";

// ---- lifecycle: launch, run control, termination -------------------------

#[test]
fn stp_program_runs_to_completion_and_exits_0() {
    let dir = scratch("stp");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "stp", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    assert_eq!(
        out,
        vec![AdapterEvent::Initialized],
        "a successful launch reports readiness for configuration"
    );

    out.clear();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    assert!(out.is_empty());
    assert_eq!(adapter.run_state(), RunState::Running);

    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    assert_eq!(adapter.run_state(), RunState::Running);

    let events = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Done);
    match events.as_slice() {
        [
            AdapterEvent::Output { category, output },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*category, "console");
            assert!(output.contains("Stopped"), "got: {output}");
            assert_eq!(*code, 0);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn hlt_program_runs_to_completion_and_exits_2() {
    let dir = scratch("hlt");
    let program = write_tmx(&dir, "hlt", HLT_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "hlt", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    adapter.handle("continue", &Value::Null, &mut out).unwrap();

    let events = drive_to_pause_or_done(&mut adapter);
    match events.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 2);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn trapping_program_stops_with_exception_reason_then_exits_3_on_the_next_continue() {
    let dir = scratch("trap");
    let program = write_tmx(&dir, "trap", TRAP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "trap", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    adapter.handle("continue", &Value::Null, &mut out).unwrap();

    let first = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Stopped);
    match first.as_slice() {
        [
            AdapterEvent::Stopped {
                reason,
                description,
            },
        ] => {
            assert_eq!(*reason, "exception");
            assert!(
                description.as_deref().is_some_and(|d| d.contains("stack")),
                "got: {description:?}"
            );
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }

    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let second = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Done);
    match second.as_slice() {
        [
            AdapterEvent::Output { output, .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 3);
            assert!(output.contains("steps 1"), "got: {output}");
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn stop_on_entry_yields_stopped_entry_before_any_step() {
    let dir = scratch("entry");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "stp", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut launch_out = Vec::new();
    adapter
        .handle(
            "launch",
            &launch_args(&program, &tape, true),
            &mut launch_out,
        )
        .unwrap();
    assert_eq!(launch_out, vec![AdapterEvent::Initialized]);

    let mut configured_out = Vec::new();
    adapter
        .handle("configurationDone", &Value::Null, &mut configured_out)
        .unwrap();
    assert_eq!(
        configured_out,
        vec![AdapterEvent::Stopped {
            reason: "entry",
            description: None,
        }]
    );
    assert_eq!(adapter.run_state(), RunState::Stopped);

    let mut out = Vec::new();
    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let events = drive_to_pause_or_done(&mut adapter);
    match events.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 0);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn launch_initialized_event_precedes_any_stopped_event() {
    let dir = scratch("entry-order");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "stp", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    assert_eq!(
        out,
        vec![
            AdapterEvent::Initialized,
            AdapterEvent::Stopped {
                reason: "entry",
                description: None,
            },
        ]
    );
}

#[test]
fn pause_mid_run_stops_an_infinite_loop() {
    let dir = scratch("pause");
    let program = write_tmx(&dir, "loop", LOOP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "loop", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    assert_eq!(adapter.run_state(), RunState::Running);

    let mut tick_events = Vec::new();
    assert_eq!(adapter.tick(&mut tick_events), RunState::Running);
    assert!(tick_events.is_empty());

    let mut pause_events = Vec::new();
    adapter
        .handle("pause", &Value::Null, &mut pause_events)
        .unwrap();
    assert_eq!(
        pause_events,
        vec![AdapterEvent::Stopped {
            reason: "pause",
            description: None,
        }]
    );
    assert_eq!(adapter.run_state(), RunState::Stopped);

    let mut ignored = Vec::new();
    let err = adapter
        .handle("pause", &Value::Null, &mut ignored)
        .unwrap_err();
    assert!(err.contains("not running"), "got: {err}");
}

#[test]
fn launch_without_a_program_argument_is_rejected() {
    let dir = scratch("no-program");
    let tape = write_one_tape_block(&dir, "stp", 2, 0);
    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle("launch", &json!({"tape": tape.to_str().unwrap()}), &mut out)
        .unwrap_err();
    assert!(err.contains("program"), "got: {err}");
    assert!(out.is_empty());
}

/// TM deviation with no PM analog (module doc): a launch with no `"tape"`
/// argument is a clean error, not an empty-tape default.
#[test]
fn launch_without_a_tape_argument_is_rejected() {
    let dir = scratch("no-tape");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle(
            "launch",
            &json!({"program": program.to_str().unwrap()}),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("tape"), "got: {err}");
    assert!(out.is_empty(), "no phantom initialization");
}

#[test]
fn unsupported_commands_answer_the_uniform_error() {
    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle("evaluate", &Value::Null, &mut out)
        .unwrap_err();
    assert!(err.contains("evaluate"), "got: {err}");
}

#[test]
fn launch_with_a_nonexistent_program_path_is_rejected_without_touching_state() {
    let dir = scratch("bad-program");
    let tape = write_one_tape_block(&dir, "stp", 2, 0);
    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle(
            "launch",
            &launch_args(Path::new("/definitely/not/a/real/path.tmx"), &tape, false),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("cannot read"), "got: {err}");
    assert!(out.is_empty());
}

#[test]
fn launch_with_a_nonexistent_tape_path_is_rejected_without_touching_state() {
    let dir = scratch("bad-tape");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle(
            "launch",
            &launch_args(
                &program,
                Path::new("/definitely/not/a/real/tape.tmt"),
                false,
            ),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("cannot read"), "got: {err}");
    assert!(out.is_empty());
}

/// TM-only launch error path (module doc, no PM analog): the tape block's
/// band count must equal the executable's declared tape count.
#[test]
fn launch_with_a_tape_band_count_mismatch_is_rejected_without_touching_state() {
    let dir = scratch("band-mismatch");
    // A 1-tape program...
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    // ...but a 2-tape block.
    let tape = write_two_tape_block(&dir, "mismatch", 0, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap_err();
    assert!(err.contains("tape(s)"), "got: {err}");
    assert!(err.contains("expects 1"), "got: {err}");
    assert!(out.is_empty());
}

/// TM-only launch error path (module doc, no PM analog): each band's
/// alphabet width must match the executable's declared cardinality.
#[test]
fn launch_with_a_tape_cardinality_mismatch_is_rejected_without_touching_state() {
    let dir = scratch("cardinality-mismatch");
    // STP_TMA declares alpha=(2); seed a 1-tape block with a WIDER
    // alphabet (3 glyphs) instead.
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "wide", 3, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap_err();
    assert!(err.contains("glyph(s)"), "got: {err}");
    assert!(err.contains("expects 2"), "got: {err}");
    assert!(out.is_empty());
}

#[test]
fn brk_pauses_with_a_debugger_statement_description_then_a_further_continue_finishes() {
    let dir = scratch("brk");
    let program = write_tmx(&dir, "brk", BRK_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "brk", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    adapter.handle("continue", &Value::Null, &mut out).unwrap();

    let first = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Stopped);
    match first.as_slice() {
        [
            AdapterEvent::Stopped {
                reason,
                description,
            },
        ] => {
            assert_eq!(*reason, "breakpoint");
            assert_eq!(description.as_deref(), Some("debugger statement"));
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }

    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let second = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Done);
    match second.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 0);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

// ---- stepping and breakpoints (compiled `.tmc`) ---------------------------

/// Line granularity must collapse MORE than one raw instruction into a
/// single `next` call at least once (the state-decl line, whose
/// `rd`/`mtc`/`djmp` all map to it — `compiler.rs`'s own
/// `debug_info_remaps_object_lines_to_tmc_sources` test pins this rule),
/// while instruction granularity always advances exactly one. Asserted via
/// the `steps` register delta rather than a hand-derived total call count
/// (Task 8's review lesson: a hardcoded expected instruction count for a
/// compiled program is exactly the kind of guess that can be silently
/// wrong in both the code and the test) — the `>= 2` bound is
/// deliberately loose even though the observed delta for this exact
/// fixture is 3 (`rd`, `mtc`, `djmp`, verified via a throwaway debug
/// dump during development): the assertion only needs "more than one",
/// not the precise count, to stay robust against incidental codegen
/// changes.
#[test]
fn line_next_collapses_more_instructions_than_instruction_granularity_does() {
    let dir = scratch("line-collapse");
    let program = write_tmc_debug(&dir, "callstep", CALLSTEP_TMC);
    let tape = write_two_tape_block(&dir, "callstep", 1, 0); // num='0' seed

    // Position exactly at the state-decl line's first instruction via a
    // source breakpoint — `stopOnEntry` instead would pause one
    // instruction earlier, at the implicit (unmapped) `ent`, whose own
    // `next` call retires only itself (unmapped -> mapped already counts
    // as a line change) and would not exercise the collapse this test is
    // about.
    let scan_line = line_of(CALLSTEP_TMC, "entry state scan");
    let mut line_adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    line_adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    line_adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": scan_line}]}),
            &mut out,
        )
        .unwrap();
    line_adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    drive_to_pause_or_done(&mut line_adapter);
    assert_eq!(line_adapter.run_state(), RunState::Stopped);

    let before = steps_count(&mut line_adapter);
    line_adapter.handle("next", &Value::Null, &mut out).unwrap();
    let line_delta = steps_count(&mut line_adapter) - before;
    assert!(
        line_delta >= 2,
        "expected the state-decl line to collapse at least 2 instructions, got delta {line_delta}"
    );

    let mut instr_adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    instr_adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    instr_adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    let before = steps_count(&mut instr_adapter);
    instr_adapter
        .handle("next", &json!({"granularity": "instruction"}), &mut out)
        .unwrap();
    let instr_delta = steps_count(&mut instr_adapter) - before;
    assert_eq!(
        instr_delta, 1,
        "instruction granularity must advance exactly one"
    );

    assert!(
        line_delta > instr_delta,
        "line granularity ({line_delta}) must retire more instructions per call than instruction granularity ({instr_delta})"
    );
}

#[test]
fn instruction_granularity_step_advances_exactly_one_instruction_per_call() {
    let dir = scratch("instr-step");
    let program = write_tmc_debug(&dir, "callstep", CALLSTEP_TMC);
    let tape = write_two_tape_block(&dir, "callstep", 1, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    out.clear();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    assert_eq!(
        out,
        vec![AdapterEvent::Stopped {
            reason: "entry",
            description: None,
        }]
    );

    for _ in 0..2 {
        step_instruction(&mut adapter, "stepIn");
        assert_eq!(adapter.run_state(), RunState::Stopped);
    }
}

#[test]
fn set_breakpoints_replaces_the_previous_list() {
    let dir = scratch("replace-bp");
    let program = write_tmc_debug(&dir, "callstep", CALLSTEP_TMC);
    let tape = write_two_tape_block(&dir, "callstep", 0, 0); // num='_' -> the plain stop row
    let scan_line = line_of(CALLSTEP_TMC, "entry state scan");

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": scan_line}]}),
            &mut out,
        )
        .unwrap();
    // REPLACE semantics: an empty list clears the previous one.
    adapter
        .handle("setBreakpoints", &json!({"breakpoints": []}), &mut out)
        .unwrap();

    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    let events = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Done);
    match events.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 0, "the cleared breakpoint must not fire");
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn source_breakpoint_on_an_unmapped_line_is_unverified() {
    let dir = scratch("unmapped-bp");
    let program = write_tmc_debug(&dir, "callstep", CALLSTEP_TMC);
    let tape = write_two_tape_block(&dir, "callstep", 1, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    let response = adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": 9999}]}),
            &mut out,
        )
        .unwrap();
    let entries = response["breakpoints"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["verified"], false);
    assert!(
        entries[0]["message"].as_str().unwrap().contains("-g"),
        "got: {entries:?}"
    );
}

#[test]
fn instruction_breakpoint_round_trips_through_a_plain_continue() {
    let dir = scratch("instr-bp-roundtrip");
    let program = write_tmc_debug(&dir, "callstep", CALLSTEP_TMC);
    let tape = write_two_tape_block(&dir, "callstep", 0, 0); // '_' blank -> stop row

    let stop_line = line_of(CALLSTEP_TMC, "['_', *] -> stop");

    let mut probe = TmDapAdapter::new();
    let mut probe_out = Vec::new();
    probe
        .handle(
            "launch",
            &launch_args(&program, &tape, false),
            &mut probe_out,
        )
        .unwrap();
    let response = probe
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": stop_line}]}),
            &mut probe_out,
        )
        .unwrap();
    let stop_addr = response["breakpoints"][0]["instructionReference"]
        .as_str()
        .unwrap()
        .to_string();

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    let ib_response = adapter
        .handle(
            "setInstructionBreakpoints",
            &json!({"breakpoints": [{"instructionReference": stop_addr}]}),
            &mut out,
        )
        .unwrap();
    assert_eq!(ib_response["breakpoints"][0]["verified"], true);

    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    let hit = drive_to_pause_or_done(&mut adapter);
    assert_eq!(
        hit,
        vec![AdapterEvent::Stopped {
            reason: "breakpoint",
            description: None,
        }]
    );

    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let done = drive_to_pause_or_done(&mut adapter);
    match done.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 0);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

/// Seeded so `scan` naturally takes the CALL row (`num` = `'0'`):
/// `stepIn` from the call site lands inside `mark`, proven by depth (via
/// `stackTrace`'s frame count) rather than a hand-derived address.
#[test]
fn step_in_and_step_out_behave_depth_wise_around_a_call() {
    let dir = scratch("depth-wise");
    let program = write_tmc_debug(&dir, "callstep", CALLSTEP_TMC);
    let tape = write_two_tape_block(&dir, "callstep", 1, 0); // num='0'
    let call_line = line_of(CALLSTEP_TMC, "call mark");

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": call_line}]}),
            &mut out,
        )
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let at_call = drive_to_pause_or_done(&mut adapter);
    assert_eq!(
        at_call,
        vec![AdapterEvent::Stopped {
            reason: "breakpoint",
            description: None,
        }]
    );
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    assert_eq!(trace["totalFrames"], json!(1), "not yet inside the call");

    let mut step_in_out = Vec::new();
    adapter
        .handle("stepIn", &Value::Null, &mut step_in_out)
        .unwrap();
    assert_eq!(
        step_in_out,
        vec![AdapterEvent::Stopped {
            reason: "step",
            description: None,
        }]
    );
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    assert_eq!(
        trace["totalFrames"],
        json!(2),
        "genuinely depth 1 inside mark: {trace:?}"
    );
    // A bound in-unit call site (`t = aux`) gets its own specialized
    // callee copy from the composition engine, named `mark.<digest>` —
    // the digest suffix is content-derived and not this test's concern,
    // so it checks the stable prefix rather than an exact match.
    assert!(
        trace["stackFrames"][0]["name"]
            .as_str()
            .unwrap()
            .starts_with("mark"),
        "got: {trace:?}"
    );

    let mut step_out_out = Vec::new();
    adapter
        .handle("stepOut", &Value::Null, &mut step_out_out)
        .unwrap();
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    assert_eq!(
        trace["totalFrames"],
        json!(1),
        "back in scan after stepping out: {trace:?}"
    );

    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let finished = drive_to_pause_or_done(&mut adapter);
    match finished.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 0);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

/// The regression this covers: comparing ONLY the resolved line number
/// (not the function it belongs to) while walking a line-granularity step
/// would read a same-numbered line in a DIFFERENT function as "no
/// change" and keep walking straight past a real function boundary.
#[test]
fn line_step_compares_function_identity_not_just_the_line_number() {
    let dir = scratch("cross-fn-line");
    let program = write_tmc_debug_multi(
        &dir,
        "return-collide",
        &[RETURN_CALLER_TMC, RETURN_CALLEE_TMC],
    );
    let tape = write_one_tape_block(&dir, "return-collide", 3, 1); // num='0'

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    // Walk forward at line granularity until genuinely inside `mark`'s OWN
    // MAPPED code (depth 1 AND a known source line — skips past the `call`
    // instruction itself and mark's own unmapped entry marker, both of
    // which also register as depth-1 stops along the way). Bounded, since
    // the exact number of preceding line-steps depends on the compiled
    // layout, not something this test hardcodes.
    let mut steps_taken = 0;
    loop {
        let mut step_out = Vec::new();
        adapter
            .handle("stepIn", &Value::Null, &mut step_out)
            .unwrap();
        assert_eq!(
            step_out,
            vec![AdapterEvent::Stopped {
                reason: "step",
                description: None,
            }]
        );
        steps_taken += 1;
        assert!(steps_taken < 50, "never reached mark's own mapped line");
        let trace = adapter
            .handle("stackTrace", &Value::Null, &mut out)
            .unwrap();
        if trace["totalFrames"] == json!(2) && trace["stackFrames"][0]["line"] != json!(0) {
            break;
        }
    }

    // The critical step: retiring `write`/`return` inside `mark` returns
    // DIRECTLY into `main` — the fix must stop there (function identity
    // differs even where the raw line number might coincide), not swallow
    // `main`'s remaining code and finish the program in one call.
    let mut final_step = Vec::new();
    adapter
        .handle("stepIn", &Value::Null, &mut final_step)
        .unwrap();
    assert_eq!(
        final_step,
        vec![AdapterEvent::Stopped {
            reason: "step",
            description: None,
        }],
        "must stop back in main, not swallow it and finish the program"
    );
    assert_eq!(adapter.run_state(), RunState::Stopped);
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    assert_eq!(trace["totalFrames"], json!(1), "genuinely back in main");
    assert_eq!(trace["stackFrames"][0]["name"], json!("main"));

    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let done = drive_to_pause_or_done(&mut adapter);
    match done.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 0);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

// ---- the assembly-line-granularity bonus (hand-written `.tma`) -----------

/// The arc's stated bonus: a hand-assembled program's `-g` line table
/// carries the ASSEMBLY's own lines, so breakpoints and stepping in the
/// `.tma` text work for free, with no source remap. No PM analog test
/// exercises this — PM's own suite only proves the degrade-to-instruction
/// path for a `-g`-less `.pma`.
#[test]
fn tma_breakpoint_hits_at_the_assembly_line() {
    let dir = scratch("tma-debug");
    // TRACE_TMA (`nop; nop; stp`) carries no `brk` of its own, so the
    // planted breakpoint is the only pause on the way — unlike `BRK_TMA`,
    // which would hit its own `brk` first.
    let program = write_tmx_debug_asm(&dir, "trace", TRACE_TMA);
    let tape = write_one_tape_block(&dir, "trace", 2, 0);
    let stp_line = line_of(TRACE_TMA, "stp");

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    let response = adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": stp_line}]}),
            &mut out,
        )
        .unwrap();
    assert_eq!(response["breakpoints"][0]["verified"], true);
    assert_eq!(response["breakpoints"][0]["line"], json!(stp_line));

    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    let hit = drive_to_pause_or_done(&mut adapter);
    assert_eq!(
        hit,
        vec![AdapterEvent::Stopped {
            reason: "breakpoint",
            description: None,
        }]
    );
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    assert_eq!(trace["stackFrames"][0]["line"], json!(stp_line));
}

// ---- state surface: stack, scopes, variables, setVariable, disassemble,
// trace ----------------------------------------------------------------

#[test]
fn stack_trace_reports_frame_names_and_lines_against_the_known_map() {
    let dir = scratch("stack-trace");
    let program = write_tmc_debug(&dir, "callstep", CALLSTEP_TMC);
    let tape = write_two_tape_block(&dir, "callstep", 1, 0);
    let call_line = line_of(CALLSTEP_TMC, "call mark");

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": call_line}]}),
            &mut out,
        )
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    drive_to_pause_or_done(&mut adapter);

    let mut step_out = Vec::new();
    adapter
        .handle("stepIn", &Value::Null, &mut step_out)
        .unwrap();

    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let frames = trace["stackFrames"].as_array().unwrap();
    assert_eq!(trace["totalFrames"], json!(2));
    assert_eq!(frames.len(), 2);

    // See `step_in_and_step_out_behave_depth_wise_around_a_call`'s own
    // comment: a bound call site's callee copy is named `mark.<digest>`.
    // Frame ids are salted by the current stop generation (dap/mod.rs's
    // module doc, "Handle-scheme generation salt") — decode via `% 4096`
    // to recover the depth, the only part this fixture cares about.
    assert!(
        frames[0]["name"].as_str().unwrap().starts_with("mark"),
        "got: {frames:?}"
    );
    assert_eq!(frames[0]["id"].as_i64().unwrap() % 4096, 0);
    assert!(
        frames[0]["instructionPointerReference"]
            .as_str()
            .unwrap()
            .starts_with("0x")
    );

    // The WHOLE `machine { }` block (every state, `scan` included) compiles
    // into ONE function named after the entry world (`main`) — `scan` is a
    // label inside it, not a separate `MapFunction`, so the return frame's
    // name is `main`, not `scan`.
    assert_eq!(frames[1]["name"], json!("main"));
    assert_eq!(frames[1]["id"].as_i64().unwrap() % 4096, 1);
}

/// Pins `dap/mod.rs`'s generation salt (module doc, "Handle-scheme
/// generation salt"): two consecutive stops must issue DIFFERENT
/// `variablesReference`/frame-id handles even though the underlying
/// scopes/frame are otherwise identical — this is what busts VS Code's
/// per-reference cache, which otherwise rendered a prior stop's
/// Variables/Call Stack values after a step (live-observed in VS Code).
/// Both stops' handles must still decode (`% 4096`) to the SAME base, and
/// a STALE reference from the first stop must still resolve live data at
/// the second — a client may briefly still hold one, and it must never be
/// rejected as merely old. Mirrors PM's own pinning test.
#[test]
fn stop_generation_salts_handles_across_stops_while_stale_references_still_resolve() {
    let dir = scratch("stop-generation");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default()); // ent + stp
    let tape = write_one_tape_block(&dir, "stp", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out) // stopOnEntry
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    assert_eq!(adapter.run_state(), RunState::Stopped); // generation 1

    let scopes1 = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers1 = scope_ref(&scopes1, "Registers");
    let tapes1 = scope_ref(&scopes1, "Tapes");
    let trace1 = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let frame1 = trace1["stackFrames"][0]["id"].as_i64().unwrap();

    // Instruction-granularity `stepIn`: retires `ent` only, landing on
    // `stp` (not yet executed) — still genuinely `Stopped`, a second
    // `Stopped` event, generation 2.
    adapter
        .handle("stepIn", &json!({"granularity": "instruction"}), &mut out)
        .unwrap();
    assert_eq!(adapter.run_state(), RunState::Stopped); // generation 2

    let scopes2 = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers2 = scope_ref(&scopes2, "Registers");
    let tapes2 = scope_ref(&scopes2, "Tapes");
    let trace2 = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let frame2 = trace2["stackFrames"][0]["id"].as_i64().unwrap();

    // Different raw handles across the two stops...
    assert_ne!(
        registers1, registers2,
        "Registers scope ref must change across stops"
    );
    assert_ne!(tapes1, tapes2, "Tapes scope ref must change across stops");
    assert_ne!(frame1, frame2, "frame 0's id must change across stops");
    // ...decoding to the SAME base every time.
    assert_eq!(registers1 % 4096, registers2 % 4096);
    assert_eq!(tapes1 % 4096, tapes2 % 4096);
    assert_eq!(frame1 % 4096, frame2 % 4096);

    // A STALE (generation-1) reference must still resolve live data at
    // generation 2, not error as stale.
    let stale_tapes = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tapes1}),
            &mut out,
        )
        .unwrap();
    assert!(
        stale_tapes["variables"]
            .as_array()
            .is_some_and(|v| !v.is_empty()),
        "a stale-generation reference must still resolve, got: {stale_tapes:?}"
    );
}

#[test]
fn tape_window_marks_the_head_and_renders_per_tape_glyphs() {
    let dir = scratch("tape-window");
    let program = write_tmx(&dir, "stp2", TWO_TAPE_STP_TMA, LinkOptions::default());
    let tape = write_two_tape_block(&dir, "stp2", 0, 0); // both blank

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let tapes_ref = scope_ref(&scopes, "Tapes");
    let tapes_vars = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tapes_ref}),
            &mut out,
        )
        .unwrap();
    assert_eq!(tapes_vars["variables"].as_array().unwrap().len(), 2);
    let tape0_ref = variable_ref(&tapes_vars, "tape 0");
    let tape1_ref = variable_ref(&tapes_vars, "tape 1");
    assert_ne!(tape0_ref, tape1_ref);

    let window0 = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tape0_ref}),
            &mut out,
        )
        .unwrap();
    let cells0 = window0["variables"].as_array().unwrap();
    assert_eq!(cells0.len(), 17); // head ± 8
    assert_eq!(variable_value(&window0, "» [0]"), "'_'");

    // Tape 1's own alphabet renders — the SAME blank index reads as a
    // DIFFERENT glyph string from tape 0's, proving per-tape (not shared)
    // rendering.
    let window1 = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tape1_ref}),
            &mut out,
        )
        .unwrap();
    assert_eq!(variable_value(&window1, "» [0]"), "'_'");
}

/// A minimal 2-tape base-profile program, used only as a `variables`
/// fixture (never run to completion in the tests that use it).
const TWO_TAPE_STP_TMA: &str = "\
.routine main, tapes=2, alpha=(3, 3)
.section code
.func main
        stp
";

#[test]
fn poke_on_tape_1_is_visible_there_and_leaves_tape_0_untouched() {
    let dir = scratch("poke-tape1");
    let program = write_tmx(&dir, "stp2", TWO_TAPE_STP_TMA, LinkOptions::default());
    let tape = write_two_tape_block(&dir, "stp2", 0, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let tapes_ref = scope_ref(&scopes, "Tapes");
    let tapes_vars = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tapes_ref}),
            &mut out,
        )
        .unwrap();
    let tape0_ref = variable_ref(&tapes_vars, "tape 0");
    let tape1_ref = variable_ref(&tapes_vars, "tape 1");

    // Poke tape 1's head cell to 'a' (aux's own alphabet).
    let set_response = adapter
        .handle(
            "setVariable",
            &json!({"variablesReference": tape1_ref, "name": "» [0]", "value": "a"}),
            &mut out,
        )
        .unwrap();
    assert_eq!(set_response["value"], json!("'a'"));

    let reread1 = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tape1_ref}),
            &mut out,
        )
        .unwrap();
    assert_eq!(variable_value(&reread1, "» [0]"), "'a'");

    // Tape 0 is untouched by the tape-1 poke — index routing is genuinely
    // per-tape, not a shared cell.
    let reread0 = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tape0_ref}),
            &mut out,
        )
        .unwrap();
    assert_eq!(variable_value(&reread0, "» [0]"), "'_'");
}

#[test]
fn set_variable_on_a_tape_cell_is_visible_on_re_read_and_after_termination() {
    let dir = scratch("set-cell");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "stp", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let tapes_ref = scope_ref(&scopes, "Tapes");
    let tapes_vars = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tapes_ref}),
            &mut out,
        )
        .unwrap();
    let tape0_ref = variable_ref(&tapes_vars, "tape 0");

    let set_response = adapter
        .handle(
            "setVariable",
            &json!({"variablesReference": tape0_ref, "name": "» [0]", "value": "1"}),
            &mut out,
        )
        .unwrap();
    assert_eq!(set_response["value"], json!("'1'"));

    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let events = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Done);
    match events.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 0);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }

    let post_termination = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tape0_ref}),
            &mut out,
        )
        .unwrap();
    assert_eq!(variable_value(&post_termination, "» [0]"), "'1'");
}

#[test]
fn set_variable_on_ip_is_rejected() {
    let dir = scratch("set-ip");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "stp", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers_ref = scope_ref(&scopes, "Registers");
    let err = adapter
        .handle(
            "setVariable",
            &json!({"variablesReference": registers_ref, "name": "IP", "value": "0x5"}),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("read-only"), "got: {err}");
}

#[test]
fn base_profile_image_has_no_fr_register_and_rejects_setting_it() {
    let dir = scratch("no-fr");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "stp", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let vars = registers(&mut adapter);
    assert!(!has_variable(&vars, "FR"), "got: {vars:?}");

    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers_ref = scope_ref(&scopes, "Registers");
    let err = adapter
        .handle(
            "setVariable",
            &json!({"variablesReference": registers_ref, "name": "FR", "value": "0"}),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("read-only"), "got: {err}");
}

#[test]
fn frames_profile_image_exposes_fr_and_rejects_setting_it() {
    let dir = scratch("frames-fr");
    let program = write_tmx(&dir, "frames", FRAMES_TMA, frames_opts());
    let tape = write_one_tape_block(&dir, "frames", 4, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    // Step into the first framed call so FR is genuinely non-default —
    // presence is asserted either way, but this proves the whole path is
    // live, not just a static "always show FR" flag.
    step_instruction(&mut adapter, "stepIn"); // ent
    step_instruction(&mut adapter, "stepIn"); // call

    let vars = registers(&mut adapter);
    assert!(has_variable(&vars, "FR"), "got: {vars:?}");

    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers_ref = scope_ref(&scopes, "Registers");
    let err = adapter
        .handle(
            "setVariable",
            &json!({"variablesReference": registers_ref, "name": "FR", "value": "0"}),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("read-only"), "got: {err}");
}

/// Proves `MR` is genuinely LIVE, not merely echoed back by `setVariable`:
/// overriding it between `mtc` and `djmp` steers `djmp` to a DIFFERENT
/// dispatch target than the one the seeded tape naturally matches
/// (`MR_TMA`'s own doc comment spells out the deterministic row layout).
#[test]
fn mr_set_steers_a_subsequent_djmp_to_a_different_branch() {
    let dir = scratch("mr-steer");
    let program = write_tmx(&dir, "mr", MR_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "mr", 3, 1); // seed symbol '1' -> natural row 1

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    step_instruction(&mut adapter, "stepIn"); // ent (implicit)
    step_instruction(&mut adapter, "stepIn"); // rd
    step_instruction(&mut adapter, "stepIn"); // mtc -> MR naturally set

    let natural_mr: u32 = variable_value(&registers(&mut adapter), "MR")
        .parse()
        .unwrap();
    assert_eq!(natural_mr, 1, "row 1 ['1'] must match the seeded symbol");

    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers_ref = scope_ref(&scopes, "Registers");
    let set_response = adapter
        .handle(
            "setVariable",
            &json!({"variablesReference": registers_ref, "name": "MR", "value": "2"}),
            &mut out,
        )
        .unwrap();
    assert_eq!(set_response["value"], json!("2"));

    step_instruction(&mut adapter, "stepIn"); // djmp, now steered to branchB
    step_instruction(&mut adapter, "stepIn"); // branchB's wr [2]

    // branchB (and ONLY branchB) writes symbol 2 onto the tape head —
    // branchA is a bare `stp` with no tape effect at all.
    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let tapes_ref = scope_ref(&scopes, "Tapes");
    let tapes_vars = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tapes_ref}),
            &mut out,
        )
        .unwrap();
    let tape0_ref = variable_ref(&tapes_vars, "tape 0");
    let window = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tape0_ref}),
            &mut out,
        )
        .unwrap();
    assert_eq!(
        variable_value(&window, "» [0]"),
        "'2'",
        "the MR override must have steered djmp into branchB"
    );

    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let done = drive_to_pause_or_done(&mut adapter);
    match done.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 0);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn disassemble_renders_listing_line_text_and_the_top_frames_reference_resolves_within_it() {
    let dir = scratch("disassemble");
    let program = write_tmc_debug(&dir, "callstep", CALLSTEP_TMC);
    let tape = write_two_tape_block(&dir, "callstep", 1, 0);
    let call_line = line_of(CALLSTEP_TMC, "call mark");

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": call_line}]}),
            &mut out,
        )
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    drive_to_pause_or_done(&mut adapter);

    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let top_ref = trace["stackFrames"][0]["instructionPointerReference"]
        .as_str()
        .unwrap()
        .to_string();

    let disassembly = adapter
        .handle(
            "disassemble",
            &json!({"memoryReference": top_ref, "instructionCount": 2}),
            &mut out,
        )
        .unwrap();
    let instructions = disassembly["instructions"].as_array().unwrap();
    assert_eq!(instructions.len(), 2);
    assert_eq!(instructions[0]["address"], json!(top_ref));
    let text = instructions[0]["instruction"].as_str().unwrap();
    assert!(text.contains("call"), "got: {text}");
}

/// VS Code's real Disassembly-view request shape: a large negative
/// `instructionOffset` relative to the current frame's `memoryReference`.
/// Parses a disassemble row address of either sign (`"0x1f"` / `"-0x2"`).
fn parse_row_address(s: &str) -> i128 {
    match s.strip_prefix("-0x") {
        Some(hex) => -i128::from_str_radix(hex, 16).unwrap(),
        None => i128::from_str_radix(s.strip_prefix("0x").unwrap(), 16).unwrap(),
    }
}

/// The POSITIONAL window contract (docs/dap.md (the Disassembly view)):
/// row `i` of the response is instruction ordinal
/// `idx + instructionOffset + i`, out-of-image head ordinals padded with
/// negative-address placeholders (never `-1`). Mirrors PM's own test —
/// its doc comment has the full VS Code reference-learning rationale.
#[test]
fn disassemble_negative_offset_pads_the_head_and_keeps_the_anchor_positional() {
    let dir = scratch("disassemble-neg-offset");
    let program = write_tmc_debug(&dir, "callstep", CALLSTEP_TMC);
    let tape = write_two_tape_block(&dir, "callstep", 1, 0);
    let call_line = line_of(CALLSTEP_TMC, "call mark");

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": call_line}]}),
            &mut out,
        )
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    drive_to_pause_or_done(&mut adapter);

    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let top_ref = trace["stackFrames"][0]["instructionPointerReference"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(
        top_ref, "0x0",
        "fixture must place the call site past the image start"
    );

    let disassembly = adapter
        .handle(
            "disassemble",
            &json!({
                "memoryReference": top_ref,
                "instructionOffset": -50,
                "instructionCount": 100,
            }),
            &mut out,
        )
        .unwrap();
    let instructions = disassembly["instructions"].as_array().unwrap();
    assert_eq!(instructions.len(), 100);

    // THE anchor contract: the reference's own row sits exactly at index
    // `-instructionOffset`, as real code.
    assert_eq!(
        instructions[50]["address"],
        json!(top_ref),
        "the anchor row must sit at index -instructionOffset: {instructions:?}"
    );
    assert_ne!(instructions[50]["instruction"], json!("<out of range>"));

    // Head padding before the image start; real code from `0x0` through
    // the anchor; strictly increasing distinct addresses; no `-1`.
    assert_eq!(instructions[0]["instruction"], json!("<out of range>"));
    assert!(
        instructions[0]["address"]
            .as_str()
            .unwrap()
            .starts_with('-'),
        "head placeholders carry negative addresses: {instructions:?}"
    );
    let first_real = instructions
        .iter()
        .position(|entry| entry["instruction"] != json!("<out of range>"))
        .expect("real code appears in the window");
    assert_eq!(instructions[first_real]["address"], json!("0x0"));
    assert!(
        instructions[first_real..=50]
            .iter()
            .all(|e| e["instruction"] != json!("<out of range>")),
        "everything from the image start to the anchor is real code"
    );
    let parsed: Vec<i128> = instructions
        .iter()
        .map(|e| parse_row_address(e["address"].as_str().unwrap()))
        .collect();
    assert!(parsed.windows(2).all(|w| w[0] < w[1]), "{parsed:?}");
    assert!(!parsed.contains(&-1));
}

#[test]
fn disassemble_past_the_code_image_advances_placeholder_addresses_monotonically() {
    let dir = scratch("disassemble-oob");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "stp", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let disassembly = adapter
        .handle(
            "disassemble",
            &json!({"memoryReference": "0x0", "instructionCount": 10}),
            &mut out,
        )
        .unwrap();
    let instructions = disassembly["instructions"].as_array().unwrap();
    assert_eq!(instructions.len(), 10);

    let addresses: Vec<u32> = instructions
        .iter()
        .map(|entry| {
            let addr = entry["address"].as_str().unwrap();
            u32::from_str_radix(addr.trim_start_matches("0x"), 16).unwrap()
        })
        .collect();
    for pair in addresses.windows(2) {
        assert!(
            pair[1] > pair[0],
            "addresses must strictly increase, got: {addresses:?}"
        );
    }
    assert!(
        instructions
            .iter()
            .any(|entry| entry["presentationHint"] == json!("invalid")),
        "expected at least one out-of-range row, got: {instructions:?}"
    );
}

fn trace_lines(events: &[AdapterEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            AdapterEvent::Output { category, output } if *category == "console" => {
                Some(output.as_str())
            }
            _ => None,
        })
        .filter(|line| line.contains("; MF="))
        .collect()
}

#[test]
fn trace_true_streams_one_output_event_per_step_in_call_with_every_head() {
    let dir = scratch("trace");
    let program = write_tmx(&dir, "trace", TRACE_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "trace", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({
        "program": program.to_str().unwrap(),
        "tape": tape.to_str().unwrap(),
        "trace": true,
        "stopOnEntry": false,
    });
    adapter.handle("launch", &args, &mut out).unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let events = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Done);

    let lines = trace_lines(&events);
    // ent, nop, nop, stp — the terminal `stp` line included.
    assert_eq!(
        lines.len(),
        4,
        "expected one trace line per step_in call including the terminal one, got: {events:?}"
    );
    assert!(
        lines.last().unwrap().contains("stp"),
        "the last trace line must name the terminal instruction, got: {:?}",
        lines.last()
    );
    // TM deviation from PM: multi-head render shape, no `FR=` suffix on a
    // base-profile image.
    assert!(
        lines[0].contains("heads=["),
        "expected the heads=[..] render shape, got: {:?}",
        lines[0]
    );
    assert!(
        !lines[0].contains("FR="),
        "a base-profile image must carry no FR suffix, got: {:?}",
        lines[0]
    );

    match events.last_chunk::<2>() {
        Some([AdapterEvent::Terminated, AdapterEvent::Exited { code }]) => {
            assert_eq!(*code, 0);
        }
        other => panic!("expected the run to end in Terminated/Exited, got: {other:?}"),
    }
}

#[test]
fn trace_true_appends_fr_on_a_frames_profile_image() {
    let dir = scratch("trace-frames");
    let program = write_tmx(&dir, "frames", FRAMES_TMA, frames_opts());
    let tape = write_one_tape_block(&dir, "frames", 4, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({
        "program": program.to_str().unwrap(),
        "tape": tape.to_str().unwrap(),
        "trace": true,
        "stopOnEntry": false,
    });
    adapter.handle("launch", &args, &mut out).unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let events = drive_to_pause_or_done(&mut adapter);
    let lines = trace_lines(&events);
    assert!(!lines.is_empty());
    assert!(
        lines.iter().any(|l| l.contains("FR=")),
        "expected at least one FR= suffix on a frames-profile trace, got: {lines:?}"
    );
}

#[test]
fn trace_true_names_the_faulting_instruction_exactly_once_across_the_two_phase_trap_flow() {
    let dir = scratch("trace-trap");
    let program = write_tmx(&dir, "trap", TRAP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "trap", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({
        "program": program.to_str().unwrap(),
        "tape": tape.to_str().unwrap(),
        "trace": true,
        "stopOnEntry": false,
    });
    adapter.handle("launch", &args, &mut out).unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let first = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Stopped);
    let first_lines = trace_lines(&first);
    assert_eq!(first_lines.len(), 2, "got: {first:?}");
    assert!(
        first_lines[1].contains("ret"),
        "the last line of the first phase must name the faulting instruction, got: {first_lines:?}"
    );

    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let second = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Done);
    assert!(
        trace_lines(&second).is_empty(),
        "the second phase must not re-emit the fault line, got: {second:?}"
    );
    match second.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 3);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

// ---- target-mode launch (cli::driver::build_target_for_launch) --------

/// Writes a one-target manifest project into `dir`: `tmt.json` with
/// `project` set to `project_body` verbatim (the whole object after
/// `"project":`) plus `app.tmc` holding `source`. Mirrors PM's
/// `write_target_project` (`crates/post-machine/tests/dap_programs.rs`).
fn write_target_project(dir: &Path, source: &str, project_body: &str) {
    fs::write(dir.join("app.tmc"), source).unwrap();
    fs::write(
        dir.join("tmt.json"),
        format!(r#"{{ "project": {project_body} }}"#),
    )
    .unwrap();
}

fn launch_target_args(dir: &Path, target: &str, stop_on_entry: bool) -> Value {
    json!({
        "target": target,
        "project": dir.to_str().unwrap(),
        "stopOnEntry": stop_on_entry,
    })
}

/// A minimal single-tape `.tmt` block over the `{_, a}` alphabet the
/// target fixtures below all share — satisfies the tape-only run-block
/// rule for tests that need SOME valid tape but don't care about its
/// content (mirrors nothing in PM, which has no such rule to satisfy).
fn write_plain_target_tape(dir: &Path, name: &str) -> PathBuf {
    let block = TapeBlockFile {
        alphabet: vec!["_".to_string(), "a".to_string()],
        tapes: vec![TapeSnapshot {
            origin: 0,
            cells: vec![0],
            head: 0,
            alphabet: None,
        }],
    };
    let path = dir.join(format!("{name}.tmt"));
    fs::write(&path, block.to_bytes().unwrap()).unwrap();
    path
}

/// A tape block with a non-default alphabet (`"_"`/`"#"`) and a SECOND
/// cell so the block's own glyph at index 1 (`"#"`) is distinguishable
/// from any default/coincidental rendering — mirrors PM's
/// `write_target_tape_block`, proving the `tape-block` load path
/// (root-relative resolution + per-tape alphabet) rather than a
/// coincidence.
fn write_target_tape_block(dir: &Path, name: &str) -> PathBuf {
    let block = TapeBlockFile {
        alphabet: vec!["_".to_string(), "#".to_string()],
        tapes: vec![TapeSnapshot {
            origin: 0,
            cells: vec![0, 1],
            head: 0,
            alphabet: None,
        }],
    };
    let path = dir.join(format!("{name}.tmt"));
    fs::write(&path, block.to_bytes().unwrap()).unwrap();
    path
}

const TARGET_APP_TMC: &str = "\
alphabet ab { '_', 'a' }

machine {
  tape main: ab;

  entry state scan {
    [*] -> write ['a'] stop;
  }
}
";

/// The manifest's own debug profile disables `-g`; the adapter must
/// still force it — proven the way `dap_programs.rs` proves anything
/// -g-dependent: a source breakpoint on a mapped line answers
/// `verified: true`. If the seam's `force_debug_info` parameter were
/// ignored, this line would be `verified: false`.
#[test]
fn target_launch_forces_debug_info_so_breakpoints_verify() {
    let dir = scratch("target-force-g");
    write_plain_target_tape(&dir, "app");
    write_target_project(
        &dir,
        TARGET_APP_TMC,
        r#"{
            "profiles": { "debug": { "debug-info": false } },
            "targets": { "app": {
                "sources": ["app.tmc"],
                "run": { "tape": "app.tmt" }
            } }
        }"#,
    );

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("initialize", &Value::Null, &mut out)
        .unwrap();
    adapter
        .handle("launch", &launch_target_args(&dir, "app", true), &mut out)
        .unwrap();
    assert_eq!(
        out,
        vec![AdapterEvent::Initialized],
        "no diagnostics on this fixture — launch reports readiness only"
    );

    out.clear();
    let write_line = line_of(TARGET_APP_TMC, "write");
    let result = adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": write_line}]}),
            &mut out,
        )
        .unwrap();
    let breakpoints = result["breakpoints"].as_array().unwrap();
    assert_eq!(
        breakpoints[0]["verified"],
        json!(true),
        "forced -g must yield a usable line map even though the manifest \
         profile turned debug-info off: {breakpoints:?}"
    );
}

/// An unused `use` import warns `unused-import` at compile time without
/// failing the build — the warning must reach the client as its own
/// `stderr` `Output` event, emitted before `Initialized`.
#[test]
fn target_launch_streams_build_warnings_as_stderr_output_before_initialized() {
    let dir = scratch("target-warnings");
    write_plain_target_tape(&dir, "app");
    write_target_project(
        &dir,
        &format!("use std::binaryNumbersBare::plusOne;\n\n{TARGET_APP_TMC}"),
        r#"{ "targets": { "app": {
            "sources": ["app.tmc"],
            "run": { "tape": "app.tmt" }
        } } }"#,
    );

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_target_args(&dir, "app", false), &mut out)
        .unwrap();

    match out.as_slice() {
        [
            AdapterEvent::Output { category, output },
            AdapterEvent::Initialized,
        ] => {
            assert_eq!(*category, "stderr");
            assert!(output.contains("unused import"), "got: {output}");
            assert!(output.contains("plusOne"), "got: {output}");
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

/// The ordering leg `handle_launch_target`'s own doc comment claims: the
/// tape loads BEFORE diagnostics are pushed to `out`, specifically so a
/// failure past a successful (diagnostic-producing) build still pushes
/// nothing. This fixture builds cleanly — with a warning, so
/// `built.diagnostics` is non-empty — but its declared `run` tape is
/// garbage bytes, so `build_tapes` fails on the SAME launch after the
/// build already succeeded. If diagnostics were pushed first (PM's
/// literal order), `out` would carry the warning `Output` event despite
/// the overall launch failing; proving `out.is_empty()` here is the
/// proof that isn't the case.
#[test]
fn target_launch_with_a_malformed_declared_tape_fails_cleanly_before_any_diagnostic_leaks() {
    let dir = scratch("target-malformed-tape");
    fs::write(dir.join("app.tmt"), b"not a real tape block").unwrap();
    write_target_project(
        &dir,
        &format!("use std::binaryNumbersBare::plusOne;\n\n{TARGET_APP_TMC}"),
        r#"{ "targets": { "app": {
            "sources": ["app.tmc"],
            "run": { "tape": "app.tmt" }
        } } }"#,
    );

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle("launch", &launch_target_args(&dir, "app", false), &mut out)
        .unwrap_err();
    assert!(!err.is_empty(), "got: {err}");
    assert!(
        out.is_empty(),
        "a launch failure after a successful (diagnostic-producing) build \
         must still push nothing — the tape loads BEFORE diagnostics are \
         pushed, not after; got: {out:?}"
    );
}

/// The target's `run` block's `tape` must become the session's initial
/// tape, resolved manifest-relative (root-relative, not the process cwd)
/// — proven by reading back the block's own `"#"` glyph on `[1]` through
/// the ordinary `variables` path.
#[test]
fn target_launch_loads_the_tape_block_from_the_targets_run_settings() {
    let dir = scratch("target-tape");
    write_target_tape_block(&dir, "app");
    write_target_project(
        &dir,
        TARGET_APP_TMC,
        r#"{ "targets": { "app": {
            "sources": ["app.tmc"],
            "run": { "tape": "app.tmt" }
        } } }"#,
    );

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_target_args(&dir, "app", true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let tapes_ref = scope_ref(&scopes, "Tapes");
    let tapes_vars = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tapes_ref}),
            &mut out,
        )
        .unwrap();
    let tape0_ref = variable_ref(&tapes_vars, "tape 0");
    let window = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tape0_ref}),
            &mut out,
        )
        .unwrap();
    assert_eq!(variable_value(&window, "» [0]"), "'_'");
    assert_eq!(variable_value(&window, "[1]"), "'#'");
}

#[test]
fn target_launch_with_a_bad_target_name_fails_without_touching_state() {
    let dir = scratch("target-bad-name");
    write_plain_target_tape(&dir, "app");
    write_target_project(
        &dir,
        TARGET_APP_TMC,
        r#"{ "targets": { "app": {
            "sources": ["app.tmc"],
            "run": { "tape": "app.tmt" }
        } } }"#,
    );

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle(
            "launch",
            &launch_target_args(&dir, "nosuch", false),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("nosuch"), "got: {err}");
    assert!(out.is_empty());
}

/// TM-specific: a target with no `run` block at all cannot launch — there
/// is no empty-tape default for TM-1 to fall back on (module doc, the
/// tape-only run-block rule). The failure must be clean: no diagnostics,
/// no `Initialized`.
#[test]
fn target_without_a_run_block_fails_cleanly() {
    let dir = scratch("target-no-run-block");
    write_target_project(
        &dir,
        TARGET_APP_TMC,
        r#"{ "targets": { "app": { "sources": ["app.tmc"] } } }"#,
    );

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle("launch", &launch_target_args(&dir, "app", false), &mut out)
        .unwrap_err();
    assert!(err.contains("app"), "got: {err}");
    assert!(err.contains("run"), "got: {err}");
    assert!(
        out.is_empty(),
        "a clean launch failure must push nothing, got: {out:?}"
    );
}

/// TM-specific: a target WITH a `run` block that declares no `tape` is
/// the other half of the same rule — `tmt build --run` itself refuses
/// this shape (`run_once`, `cli/driver.rs`); a dap launch must refuse it
/// the same clean way.
#[test]
fn target_run_block_without_a_tape_fails_cleanly() {
    let dir = scratch("target-no-tape");
    write_target_project(
        &dir,
        TARGET_APP_TMC,
        r#"{ "targets": { "app": {
            "sources": ["app.tmc"],
            "run": { "max-steps": 10 }
        } } }"#,
    );

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle("launch", &launch_target_args(&dir, "app", false), &mut out)
        .unwrap_err();
    assert!(err.contains("app"), "got: {err}");
    assert!(err.contains("tape"), "got: {err}");
    assert!(
        out.is_empty(),
        "a clean launch failure must push nothing, got: {out:?}"
    );
}

#[test]
fn launch_rejects_program_and_target_together() {
    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({"program": "x.tmx", "tape": "x.tmt", "target": "app"});
    let err = adapter.handle("launch", &args, &mut out).unwrap_err();
    assert!(
        err.contains("program") && err.contains("target"),
        "got: {err}"
    );
    assert!(out.is_empty());
}

// ---- Done-guard: configurationDone/continue after termination ---------

/// A repeat `configurationDone` or `continue` after the program has
/// already finished must answer the standing rejection WITHOUT re-running
/// `finish()`'s termination events a second time (`handle_configuration_done`/
/// `handle_continue`'s own `RunState::Done` guards) — mirrors PM's own
/// carried-scope test (Task 10 review ruling: this gap was untested in
/// BOTH adapters).
#[test]
fn configuration_done_and_continue_after_done_reject_without_reemitting_termination() {
    let dir = scratch("done-guard");
    let program = write_tmx(&dir, "stp", STP_TMA, LinkOptions::default());
    let tape = write_one_tape_block(&dir, "stp", 2, 0);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, false), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Done);

    out.clear();
    let err = adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap_err();
    assert!(err.contains("already finished"), "got: {err}");
    assert!(
        out.is_empty(),
        "a rejected configurationDone must not push events, got: {out:?}"
    );

    let err = adapter
        .handle("continue", &Value::Null, &mut out)
        .unwrap_err();
    assert!(err.contains("already finished"), "got: {err}");
    assert!(
        out.is_empty(),
        "a rejected continue must not push events, got: {out:?}"
    );
}

// ---- source provenance (docs/dap.md (source provenance)) — mirrors the
// PM suite's provenance tests over the TM fixture pair ------------------

/// `write_tmc_debug_multi`'s provenance sibling: writes each source AS A
/// REAL FILE into `dir` first (a frame's `source` object is attached only
/// when the resolved file exists) and links with per-unit `sources`
/// naming those files relative to the sidecar's directory — the shape
/// `tmt build` emits (docs/formats.md (map sidecar)).
fn write_tmc_debug_with_sources(dir: &Path, name: &str, units: &[(&str, &str)]) -> PathBuf {
    let mut objects = Vec::new();
    let mut sources = Vec::new();
    for (file, text) in units {
        fs::write(dir.join(file), text).unwrap();
        objects.push(
            compile(
                text,
                CompileOptions {
                    debug_info: true,
                    ..Default::default()
                },
            )
            .unwrap()
            .object,
        );
        sources.push(Some((*file).to_string()));
    }
    let linked = link(
        &objects,
        &[],
        LinkOptions {
            sources,
            ..Default::default()
        },
    )
    .unwrap();
    let path = dir.join(format!("{name}.tmx"));
    fs::write(&path, linked.executable.to_bytes()).unwrap();
    let mut map_path = path.clone().into_os_string();
    map_path.push(".map");
    fs::write(&map_path, linked.map.to_json()).unwrap();
    path
}

#[test]
fn frames_carry_source_objects_and_breakpoints_filter_by_file() {
    let dir = scratch("provenance");
    let program = write_tmc_debug_with_sources(
        &dir,
        "provenance",
        &[("a.tmc", RETURN_CALLER_TMC), ("b.tmc", RETURN_CALLEE_TMC)],
    );
    let tape = write_one_tape_block(&dir, "provenance", 3, 1);

    let mut adapter = TmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, &tape, true), &mut out)
        .unwrap();

    // Line 1 is unmapped in both files; the snapping rule plants each
    // request at ITS OWN file's first mapped line — two different
    // addresses for the same requested line number.
    let plant = |adapter: &mut TmDapAdapter, file: &str| -> Value {
        adapter
            .handle(
                "setBreakpoints",
                &json!({
                    "source": {"path": dir.join(file).to_str().unwrap()},
                    "breakpoints": [{"line": 1}],
                }),
                &mut Vec::new(),
            )
            .unwrap()
    };
    let in_a = plant(&mut adapter, "a.tmc");
    let in_b = plant(&mut adapter, "b.tmc");
    assert_eq!(in_a["breakpoints"][0]["verified"], json!(true));
    assert_eq!(in_b["breakpoints"][0]["verified"], json!(true));
    assert_ne!(
        in_a["breakpoints"][0]["instructionReference"],
        in_b["breakpoints"][0]["instructionReference"],
        "each file must snap line 1 to its own first mapped line"
    );

    let foreign = adapter
        .handle(
            "setBreakpoints",
            &json!({
                "source": {"path": dir.join("elsewhere.tmc").to_str().unwrap()},
                "breakpoints": [{"line": 1}],
            }),
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(foreign["breakpoints"][0]["verified"], json!(false));
    assert!(
        foreign["breakpoints"][0]["message"]
            .as_str()
            .unwrap()
            .contains("no code in this program comes from this file"),
        "got: {foreign}"
    );

    // The entry frame belongs to a.tmc's machine world — its `source`
    // object carries the file leaf and the ABSOLUTE resolved path.
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let frame = &trace["stackFrames"][0];
    assert_eq!(frame["source"]["name"], json!("a.tmc"));
    assert_eq!(
        frame["source"]["path"],
        json!(dir.join("a.tmc").to_str().unwrap())
    );
}
