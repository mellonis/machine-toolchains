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
use mtc_post_machine::{CompileOptions, compile};

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

/// Compiles `.pmc` source WITH debug info, links it, and writes both the
/// `.pmx` and its `.pmx.map` sidecar into `dir` — the shape `sidecar_map`
/// (`dap/mod.rs`) discovers automatically from the `program` path, giving
/// `launch` a real `LineIndex` so the stepping/breakpoint tests have source
/// lines to work against.
fn write_pmc_debug(dir: &Path, name: &str, pmc_source: &str) -> PathBuf {
    let out = compile(
        pmc_source,
        CompileOptions {
            debug_info: true,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    let path = dir.join(format!("{name}.pmx"));
    fs::write(&path, linked.executable.to_bytes()).unwrap();
    let mut map_path = path.clone().into_os_string();
    map_path.push(".map");
    fs::write(&map_path, linked.map.to_json()).unwrap();
    path
}

/// The 1-based physical line number of the first line containing `needle` —
/// avoids hand-counted line numbers going stale if the fixture text above
/// is ever reformatted.
fn line_of(source: &str, needle: &str) -> u32 {
    source.lines().position(|l| l.contains(needle)).unwrap() as u32 + 1
}

/// A fixture with both features the stepping tests need: a comma group
/// (`right, right, right, mark;`) compiling to FOUR instructions on ONE
/// source line, and a call into a second function. Compiled layout
/// (verified empirically against this exact source): `main` is
/// `[ent@0 (unmapped), rgt@1, rgt@2, rgt@3, wr@4 (line 6 x4), call.s@6
/// (line 7), hlt@8 (line 8)]`; `callee` is `[ent@9 (unmapped), rgt@10,
/// ret@11 (line 2 x2)]`.
const CALLSTEP_PMC: &str = "\
callee() {
    right(!);
}

main() {
    right, right, right, mark;
    @callee();
    halt;
}
";

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
    assert_eq!(
        out,
        vec![AdapterEvent::Initialized],
        "a successful launch reports readiness for configuration"
    );

    out.clear();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    // No stopOnEntry: configurationDone itself starts the run — no
    // stopped event, no explicit continue needed.
    assert!(out.is_empty());
    assert_eq!(adapter.run_state(), RunState::Running);

    // A `continue` while already Running stays legal (idempotent) — a
    // client that sends one anyway (as this test's earlier shape did) is
    // not punished for it.
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

/// A single ordered event stream, the shape a real client actually
/// observes (both requests answered over the same connection): the
/// launch-time `initialized` event must precede the `stopOnEntry`
/// pause it makes possible — `setBreakpoints`/`configurationDone` are
/// only legal for a client once it has seen `initialized`, so a
/// `stopped` event arriving first (or `initialized` arriving late)
/// would be a real protocol violation, not just a cosmetic ordering
/// nit.
#[test]
fn launch_initialized_event_precedes_any_stopped_event() {
    let dir = scratch("entry-order");
    let program = write_pmx(&dir, "stp", STP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out)
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

// ---- carried scope from Task 6's review --------------------------------

#[test]
fn launch_with_a_nonexistent_program_path_is_rejected_without_touching_state() {
    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    let err = adapter
        .handle(
            "launch",
            &launch_args(Path::new("/definitely/not/a/real/path.pmx"), false),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("cannot read"), "got: {err}");
    // No phantom initialization: a failed launch must not claim readiness.
    assert!(out.is_empty());
}

#[test]
fn launch_with_a_valid_program_and_a_bad_tape_path_is_rejected_without_touching_state() {
    let dir = scratch("bad-tape");
    let program = write_pmx(&dir, "stp", STP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({
        "program": program.to_str().unwrap(),
        "tape": "/definitely/not/a/real/tape.pmt",
        "stopOnEntry": false,
    });
    let err = adapter.handle("launch", &args, &mut out).unwrap_err();
    assert!(err.contains("cannot read"), "got: {err}");
    assert!(out.is_empty());
}

/// `tick`'s `PauseCause::Brk` arm was reachable from the skeleton task
/// onward but had no fixture exercising it — a `brk` instruction pauses
/// with the SAME "breakpoint" reason an address-based breakpoint uses, but
/// a distinct "debugger statement" description, and a further `continue`
/// resumes past it to natural completion.
const BRK_PROGRAM: &str = "\
.func main
        brk
        stp
";

#[test]
fn brk_pauses_with_a_debugger_statement_description_then_a_further_continue_finishes() {
    let dir = scratch("brk");
    let program = write_pmx(&dir, "brk", BRK_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
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

// ---- stepping and breakpoints -------------------------------------------

/// Drives `next` with the given `arguments` (the request body: `Value::Null`
/// for the default line granularity, `json!({"granularity": "instruction"})`
/// for instruction granularity) repeatedly until the adapter reports `Done`,
/// returning how many `next` calls that took.
fn drive_next_to_done(adapter: &mut PmDapAdapter, arguments: &Value) -> u32 {
    let mut calls = 0;
    loop {
        let mut out = Vec::new();
        adapter.handle("next", arguments, &mut out).unwrap();
        calls += 1;
        if adapter.run_state() == RunState::Done {
            return calls;
        }
        assert!(calls < 1_000, "drive_next_to_done: runaway loop");
    }
}

#[test]
fn line_next_collapses_the_comma_group_into_fewer_calls_than_instruction_granularity() {
    let dir = scratch("line-collapse");
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);

    let mut line_adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    line_adapter
        .handle("launch", &launch_args(&program, true), &mut out)
        .unwrap();
    line_adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    // Empirically (see CALLSTEP_PMC's doc comment): `ent`(unmapped)->line 6
    // is one `next`; the 4-instruction comma group (all line 6) collapses
    // into one `next` landing on the call (line 7); stepping OVER the call
    // runs the whole callee to completion in one more `next` (landing on
    // `hlt`, line 8); `hlt`'s `next` finishes the program. Four calls.
    let line_calls = drive_next_to_done(&mut line_adapter, &Value::Null);
    assert_eq!(line_calls, 4, "line-granularity next calls");

    let mut instr_adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    instr_adapter
        .handle("launch", &launch_args(&program, true), &mut out)
        .unwrap();
    instr_adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    // Same program at instruction granularity: `ent`, then each of the
    // THREE `rgt`s and the `wr` is its own call (5 total); stepping OVER
    // the call is still atomic regardless of granularity (one call runs
    // the whole callee); `hlt` finishes it. Seven calls total.
    let instr_calls =
        drive_next_to_done(&mut instr_adapter, &json!({"granularity": "instruction"}));
    assert_eq!(instr_calls, 7, "instruction-granularity next calls");

    assert!(
        line_calls < instr_calls,
        "line granularity ({line_calls}) must need fewer next calls than instruction granularity ({instr_calls})"
    );
}

#[test]
fn instruction_granularity_step_advances_exactly_one_instruction_per_call() {
    let dir = scratch("instr-step");
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out)
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

    // Each single instruction-granularity `stepIn` call retires exactly
    // one raw instruction and reports exactly one `stopped("step")` — no
    // accidental extra looping past that boundary.
    for _ in 0..2 {
        let mut step_out = Vec::new();
        adapter
            .handle(
                "stepIn",
                &json!({"granularity": "instruction"}),
                &mut step_out,
            )
            .unwrap();
        assert_eq!(
            step_out,
            vec![AdapterEvent::Stopped {
                reason: "step",
                description: None,
            }]
        );
        assert_eq!(adapter.run_state(), RunState::Stopped);
    }
}

#[test]
fn instruction_breakpoint_mid_line_interrupts_a_line_step() {
    let dir = scratch("mid-line-bp");
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);
    let group_line = line_of(CALLSTEP_PMC, "right, right, right, mark");

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();

    // Position exactly at the comma group's first instruction via a
    // source breakpoint on its line, recovering the resolved address from
    // the response's `instructionReference` — no other introspection
    // surface is needed to learn it.
    let response = adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": group_line}]}),
            &mut out,
        )
        .unwrap();
    let entries = response["breakpoints"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["verified"], true);
    let group_start_hex = entries[0]["instructionReference"].as_str().unwrap();
    let group_start = u32::from_str_radix(group_start_hex.trim_start_matches("0x"), 16).unwrap();

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

    // Plant an instruction breakpoint on the group's THIRD item — mid-line,
    // not the line's own first address (`address_for_line` only ever
    // answers the first).
    let mid_addr = format!("0x{:x}", group_start + 2);
    let ib_response = adapter
        .handle(
            "setInstructionBreakpoints",
            &json!({"breakpoints": [{"instructionReference": mid_addr}]}),
            &mut out,
        )
        .unwrap();
    assert_eq!(ib_response["breakpoints"][0]["verified"], true);

    let mut step_out = Vec::new();
    adapter.handle("next", &Value::Null, &mut step_out).unwrap();
    assert_eq!(
        step_out,
        vec![AdapterEvent::Stopped {
            reason: "breakpoint",
            description: None,
        }],
        "the mid-line instruction breakpoint must win over the line step"
    );
    assert_eq!(adapter.run_state(), RunState::Stopped);
}

#[test]
fn set_breakpoints_replaces_the_previous_list() {
    let dir = scratch("replace-bp");
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);
    let group_line = line_of(CALLSTEP_PMC, "right, right, right, mark");

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();
    adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": group_line}]}),
            &mut out,
        )
        .unwrap();
    // REPLACE semantics: an empty list clears the previous one, not
    // merely leaves it un-refreshed.
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
            assert_eq!(*code, 2, "the cleared breakpoint must not fire");
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn source_breakpoint_on_an_unmapped_line_is_unverified() {
    let dir = scratch("unmapped-bp");
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
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
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);
    let halt_line = line_of(CALLSTEP_PMC, "halt;");

    // Learn `halt`'s address via a throwaway adapter's source breakpoint —
    // addresses are a property of the compiled program, not of any one
    // adapter instance, so the string is reusable against a fresh session
    // over the SAME `.pmx`.
    let mut probe = PmDapAdapter::new();
    let mut probe_out = Vec::new();
    probe
        .handle("launch", &launch_args(&program, false), &mut probe_out)
        .unwrap();
    let response = probe
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": halt_line}]}),
            &mut probe_out,
        )
        .unwrap();
    let halt_addr = response["breakpoints"][0]["instructionReference"]
        .as_str()
        .unwrap()
        .to_string();

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();
    let ib_response = adapter
        .handle(
            "setInstructionBreakpoints",
            &json!({"breakpoints": [{"instructionReference": halt_addr}]}),
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
            assert_eq!(*code, 2);
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn step_in_and_step_out_behave_depth_wise_around_a_call() {
    let dir = scratch("depth-wise");
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);
    let call_line = line_of(CALLSTEP_PMC, "@callee()");
    let callee_line = line_of(CALLSTEP_PMC, "right(!)");

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();
    adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": call_line}, {"line": callee_line}]}),
            &mut out,
        )
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    // Runs unimpeded through the comma group (no breakpoint there) and
    // stops right before the call.
    let at_call = drive_to_pause_or_done(&mut adapter);
    assert_eq!(
        at_call,
        vec![AdapterEvent::Stopped {
            reason: "breakpoint",
            description: None,
        }]
    );

    // stepIn: retires the call in one session step, landing on the
    // callee's (unmapped) entry — a different mapped line (`None`), so the
    // line loop stops after exactly this one step.
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

    // Proof we're genuinely at depth 1 inside `callee`: only reachable
    // from there does a plain `continue` hit `callee`'s own breakpoint
    // next, rather than running straight through to the program's end.
    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let hit_callee_bp = drive_to_pause_or_done(&mut adapter);
    assert_eq!(
        hit_callee_bp,
        vec![AdapterEvent::Stopped {
            reason: "breakpoint",
            description: None,
        }]
    );

    // stepOut: runs the rest of `callee` and returns to `main`, right
    // after the call site.
    let mut step_out_out = Vec::new();
    adapter
        .handle("stepOut", &Value::Null, &mut step_out_out)
        .unwrap();
    assert_eq!(
        step_out_out,
        vec![AdapterEvent::Stopped {
            reason: "step",
            description: None,
        }]
    );

    // Genuinely back in `main`: a plain continue now runs straight to
    // `halt` with no further breakpoints in the way.
    adapter.handle("continue", &Value::Null, &mut out).unwrap();
    let finished = drive_to_pause_or_done(&mut adapter);
    match finished.as_slice() {
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
