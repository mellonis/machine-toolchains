//! `PmDapAdapter` scripted-conversation tests: `handle`/`tick` driven
//! directly (no stdio, no `mtc_core::dap::server::run` loop) against a
//! tiny fixture `.pmx` compiled+linked in-process and written to a
//! pid+counter scratch dir (the `lint_programs.rs` isolation pattern —
//! `crates/turing-machine/tests/lint_programs.rs`'s `scratch` helper).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use mtc_core::dap::server::{AdapterEvent, DebugAdapter, RunState};
use mtc_core::linker::LinkOptions;
use mtc_post_machine::asm::{assemble, link};
use mtc_post_machine::dap::PmDapAdapter;

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter — two concurrently running
/// `cargo test` invocations must never resolve `scratch` to the same
/// directory.
fn scratch(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("dap-{name}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Assembles `pma_source`, links it standalone, and writes the resulting
/// `.pmx` into `dir`, returning its path — the `launch` request's
/// `"program"` argument.
fn write_pmx(dir: &Path, name: &str, pma_source: &str) -> PathBuf {
    let obj = assemble(pma_source, false).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    // Verifies `dap/mod.rs`'s load-bearing claim that a real PM-1 `.pmx`
    // is always the v1 code-only shape: `Machine::with_arch` (which the
    // adapter uses instead of `Machine::from_executable`+`ArchRegistry`)
    // silently drops these fields, so a PM-1 build that ever set them
    // would misbehave under the adapter without a single test noticing.
    assert!(out.executable.tables.is_empty());
    assert_eq!(out.executable.tape_count, 1);
    assert!(out.executable.alphabet_cardinalities.is_empty());
    let path = dir.join(format!("{name}.pmx"));
    fs::write(&path, out.executable.to_bytes()).unwrap();
    path
}

const STP_PROGRAM: &str = "\
.func main
        stp
";

const HLT_PROGRAM: &str = "\
.func main
        hlt
";

/// `ret` on an empty return stack traps `StackUnderflow` — the entry's
/// own `ent` retires normally first, so the trap fires on the SECOND
/// instruction, proving the fixture genuinely executes before faulting.
const TRAP_PROGRAM: &str = "\
.func main
        ret
";

/// An infinite loop (never `stp`/`hlt`/traps on its own) — used only by
/// the `pause` test, which must interrupt a run that would otherwise
/// never finish.
const LOOP_PROGRAM: &str = "\
.func main
L1:     rgt
        jmp L1
";

fn launch_args(program: &Path, stop_on_entry: bool) -> Value {
    json!({
        "program": program.to_str().unwrap(),
        "stopOnEntry": stop_on_entry,
    })
}

/// Drives `tick` while the adapter reports `Running`, collecting every
/// pushed event, stopping the instant it reports `Stopped` or `Done`.
/// Bounded so a design bug that never leaves `Running` fails the test
/// instead of hanging the process.
fn drive_to_pause_or_done(adapter: &mut PmDapAdapter) -> Vec<AdapterEvent> {
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

#[test]
fn stp_program_runs_to_completion_and_exits_0() {
    let dir = scratch("stp");
    let program = write_pmx(&dir, "stp", STP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("initialize", &Value::Null, &mut out)
        .unwrap();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    assert!(out.is_empty(), "no stopOnEntry: no event expected yet");

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
    let program = write_pmx(&dir, "hlt", HLT_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
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
    let program = write_pmx(&dir, "trap", TRAP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    adapter.handle("continue", &Value::Null, &mut out).unwrap();

    // First drive: the trap pauses the session before it is finished —
    // the adapter reports `Stopped`, not `Done`, and the run is resumable.
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

    // A further `continue` drives the same (already-trapped) session to
    // its terminal `Finished` and the termination events.
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
            // Non-vacuity: exactly one step (the leading `ent`) retired
            // before the `ret` faulted — proves the fixture genuinely
            // executed an instruction rather than trapping at address 0
            // before anything ran.
            assert!(output.contains("steps 1"), "got: {output}");
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn stop_on_entry_yields_stopped_entry_before_any_step() {
    let dir = scratch("entry");
    let program = write_pmx(&dir, "stp", STP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut launch_out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut launch_out)
        .unwrap();
    assert!(
        launch_out.is_empty(),
        "no event expected from launch itself"
    );

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
    // Nothing ran yet: the adapter is paused, awaiting an explicit continue.
    assert_eq!(adapter.run_state(), RunState::Stopped);

    // The session is still perfectly resumable from here.
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
fn pause_mid_run_stops_an_infinite_loop() {
    let dir = scratch("pause");
    let program = write_pmx(&dir, "loop", LOOP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    assert_eq!(adapter.run_state(), RunState::Running);

    // One tick spends exactly one budget slice without finishing — the
    // budget exhaustion (`PauseCause::Manual`) is invisible to the
    // client, so no event is pushed and the adapter stays Running.
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

    // pause while already stopped is rejected, not silently accepted.
    let mut ignored = Vec::new();
    let err = adapter
        .handle("pause", &Value::Null, &mut ignored)
        .unwrap_err();
    assert!(err.contains("not running"), "got: {err}");
}

#[test]
fn launch_without_a_program_argument_is_rejected() {
    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter.handle("launch", &json!({}), &mut out).unwrap_err();
    assert!(err.contains("program"), "got: {err}");
}

#[test]
fn unsupported_commands_answer_the_uniform_error() {
    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle("evaluate", &Value::Null, &mut out)
        .unwrap_err();
    assert!(err.contains("evaluate"), "got: {err}");
}
