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
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
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
    write_pmc_debug_multi(dir, name, &[pmc_source])
}

/// `write_pmc_debug`'s multi-compilation-unit sibling: compiles EACH source
/// in `pmc_sources` separately (its own file, its own line numbering
/// restarting at 1) and links them together — the shape needed to build a
/// fixture where two DIFFERENT functions happen to share a line number
/// (each source's numbering is independent, unlike two functions in one
/// file, which can never collide).
fn write_pmc_debug_multi(dir: &Path, name: &str, pmc_sources: &[&str]) -> PathBuf {
    let objects: Vec<_> = pmc_sources
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

/// Two SEPARATE compilation units, each with its own line numbering
/// restarting at 1: `main`'s `halt` (its own file's line 2) and
/// `callee`'s `right`/`ret` (that file's line 2 too) collide on the SAME
/// number in DIFFERENT functions. `callee` must be `export`ed to resolve
/// as an external symbol at link time.
const RETURN_CALLER_PMC: &str = "main() {@callee();\n    halt;\n}\n";
const RETURN_CALLEE_PMC: &str = "export callee() {\n    right(!);\n}\n";

/// The regression this covers: comparing ONLY the resolved line number
/// (not the function it belongs to) while walking a line-granularity step
/// would read `callee`'s `ret` returning straight into `main`'s `halt` —
/// same line number, different function — as "no change", and keep
/// walking straight through `halt` to program termination inside a SINGLE
/// `stepIn` call, instead of stopping the instant execution left `callee`.
#[test]
fn line_step_compares_function_identity_not_just_the_line_number() {
    let dir = scratch("cross-fn-line");
    let program = write_pmc_debug_multi(
        &dir,
        "return-collide",
        &[RETURN_CALLER_PMC, RETURN_CALLEE_PMC],
    );

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    // Walk to callee's line-2 `right` in three single steps — `main`'s
    // `ent` -> the call (line 1) -> callee's `ent` (unmapped) -> callee's
    // `right` (line 2) — each landing on a position distinct enough that
    // any comparison, line-only or full, agrees they differ.
    for _ in 0..3 {
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
    }

    // The critical step: retiring `right` then `ret` returns DIRECTLY to
    // `main`'s `halt` — also line 2, but a different function. The fix
    // must stop here, reporting exactly one "step" pause with the
    // program still running, not swallow `halt` and finish it.
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
        "must stop back in main, not swallow halt and finish the program"
    );
    assert_eq!(adapter.run_state(), RunState::Stopped);

    // Sanity: the program is genuinely still paused (about to execute
    // `halt`, not already finished) — one more continue finishes it
    // normally.
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

// ---- state surface: stack, scopes, variables, setVariable, disassemble,
// trace -----------------------------------------------------------------

/// Looks up a scope's `variablesReference` by name from a `scopes`
/// response body — never hardcodes the adapter's private handle constants.
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

/// Looks up a variable's own `variablesReference` (for expanding into its
/// children) by name from a `variables` response body.
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

/// Looks up a variable's `value` by exact name from a `variables` response
/// body.
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

#[test]
fn stack_trace_reports_frame_names_and_lines_against_the_known_map() {
    let dir = scratch("stack-trace");
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);
    let call_line = line_of(CALLSTEP_PMC, "@callee()");
    let halt_line = line_of(CALLSTEP_PMC, "halt;");

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
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
    drive_to_pause_or_done(&mut adapter); // stops right before the call

    // stepIn: retires the call, landing on callee's (unmapped) entry —
    // now `main`'s return address (right after the call) is on the stack.
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

    // Frame 0: current position, inside `callee`, at its unmapped entry —
    // rendered at the function's OPENING line per the prologue
    // convention (docs/dap.md (source provenance)). Frame ids are the
    // bare depth (dap/mod.rs's module doc, "Handle stability").
    assert_eq!(frames[0]["name"], json!("callee"));
    assert_eq!(frames[0]["line"], json!(line_of(CALLSTEP_PMC, "right(!)")));
    assert_eq!(frames[0]["id"], json!(0));
    assert!(
        frames[0]["instructionPointerReference"]
            .as_str()
            .unwrap()
            .starts_with("0x")
    );

    // Frame 1: the return address, back in `main`, right after the call —
    // which the fixture's own layout puts exactly at `halt`'s address.
    assert_eq!(frames[1]["name"], json!("main"));
    assert_eq!(frames[1]["line"], json!(halt_line));
    assert_eq!(frames[1]["id"], json!(1));
}

/// Pins `dap/mod.rs`'s stable handle scheme (module doc, "Handle
/// stability"): two consecutive stops issue IDENTICAL
/// `variablesReference`/frame-id handles — per DAP they are only valid
/// while paused and a client re-fetches on every stop, so stability is
/// what lets it correlate its own view state — and a reference held from
/// the first stop resolves live data at the second (it IS the current
/// reference). Replaces the retired per-stop generation salt, whose
/// stale-Variables motivation turned out to be the missing frame
/// `source` (docs/dap.md (source provenance)).
#[test]
fn handles_are_stable_across_stops_and_prior_stop_references_resolve() {
    let dir = scratch("stable-handles");
    let program = write_pmx(&dir, "stp", STP_PROGRAM); // ent + stp

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out) // stopOnEntry
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    assert_eq!(adapter.run_state(), RunState::Stopped); // first stop

    let scopes1 = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers1 = scope_ref(&scopes1, "Registers");
    let tapes1 = scope_ref(&scopes1, "Tapes");
    let trace1 = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let frame1 = trace1["stackFrames"][0]["id"].as_i64().unwrap();

    // Instruction-granularity `stepIn`: retires `ent` only, landing on
    // `stp` (not yet executed) — still genuinely `Stopped`, a second
    // `Stopped` event.
    adapter
        .handle("stepIn", &json!({"granularity": "instruction"}), &mut out)
        .unwrap();
    assert_eq!(adapter.run_state(), RunState::Stopped); // second stop

    let scopes2 = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers2 = scope_ref(&scopes2, "Registers");
    let tapes2 = scope_ref(&scopes2, "Tapes");
    let trace2 = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let frame2 = trace2["stackFrames"][0]["id"].as_i64().unwrap();

    // Identical handles across the two stops.
    assert_eq!(registers1, registers2);
    assert_eq!(tapes1, tapes2);
    assert_eq!(frame1, frame2);

    // A reference held from the first stop resolves live data at the
    // second — trivially, being the same handle.
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
        "a prior-stop reference must still resolve, got: {stale_tapes:?}"
    );
}

/// A small tape block carrying a non-default alphabet (`"_"` blank,
/// `"#"` mark) — proves `variables` renders the LAUNCH tape block's own
/// glyphs, not PM-1's CLI-default `" "`/`"*"` pair.
fn write_tape_block(dir: &Path, name: &str) -> PathBuf {
    let block = TapeBlockFile {
        alphabet: vec!["_".to_string(), "#".to_string()],
        tapes: vec![TapeSnapshot {
            origin: 0,
            cells: vec![0],
            head: 0,
            alphabet: None,
        }],
    };
    let path = dir.join(format!("{name}.pmt"));
    fs::write(&path, block.to_bytes().unwrap()).unwrap();
    path
}

#[test]
fn tape_window_marks_the_head_and_renders_glyphs_from_the_launch_alphabet() {
    let dir = scratch("tape-window");
    let program = write_pmx(&dir, "stp", STP_PROGRAM);
    let tape = write_tape_block(&dir, "stp");

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({
        "program": program.to_str().unwrap(),
        "tape": tape.to_str().unwrap(),
        "stopOnEntry": true,
    });
    adapter.handle("launch", &args, &mut out).unwrap();
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
    let cells = window["variables"].as_array().unwrap();
    assert_eq!(cells.len(), 17); // head ± 8

    // Head sits at position 0 (blank tape): marked, glyph '_' (the launch
    // block's own blank glyph, not PM-1's CLI-default ' ').
    assert_eq!(variable_value(&window, "» [0]"), "'_'");
    // A neighboring cell, unmarked.
    assert_eq!(variable_value(&window, "[1]"), "'_'");
    assert!(
        !cells.iter().any(|c| c["name"] == "» [1]"),
        "only the head cell carries the marker"
    );
}

#[test]
fn set_variable_on_a_tape_cell_is_visible_on_re_read_and_after_termination() {
    let dir = scratch("set-cell");
    let program = write_pmx(&dir, "stp", STP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out)
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

    // No tape block was launched: alphabet is unknown, so the cell's
    // glyph is its raw index — poke index 1 onto the head cell.
    let set_response = adapter
        .handle(
            "setVariable",
            &json!({"variablesReference": tape0_ref, "name": "» [0]", "value": "1"}),
            &mut out,
        )
        .unwrap();
    assert_eq!(set_response["value"], json!("'1'"));

    let reread = adapter
        .handle(
            "variables",
            &json!({"variablesReference": tape0_ref}),
            &mut out,
        )
        .unwrap();
    assert_eq!(variable_value(&reread, "» [0]"), "'1'");

    // Run the (otherwise tape-untouching) program to completion; the poke
    // must still be visible afterward — `variables` stays queryable once
    // the session is `Done`.
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
fn strict_cells_launch_fails_a_same_value_poke_with_the_fault_text() {
    let dir = scratch("strict-cells");
    let program = write_pmx(&dir, "stp", STP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({
        "program": program.to_str().unwrap(),
        "strictCells": true,
        "stopOnEntry": true,
    });
    adapter.handle("launch", &args, &mut out).unwrap();
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

    // The head cell is blank (index 0); poking the SAME value it already
    // holds is a strict-cell violation.
    let err = adapter
        .handle(
            "setVariable",
            &json!({"variablesReference": tape0_ref, "name": "» [0]", "value": "0"}),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("StrictCellViolation"), "got: {err}");
}

#[test]
fn set_variable_on_ip_is_rejected() {
    let dir = scratch("set-ip");
    let program = write_pmx(&dir, "stp", STP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    let scopes = adapter.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers_ref = scope_ref(&scopes, "Registers");
    let err = adapter
        .handle(
            "setVariable",
            &json!({"variablesReference": registers_ref, "name": "IP", "value": "0x0"}),
            &mut out,
        )
        .unwrap_err();
    assert!(err.contains("read-only"), "got: {err}");
}

/// `jm` jumps to `TAKEN` when MF is set, falls through to `stp` otherwise —
/// a blank tape starts with MF false (docs/core.md (initial mark latch)),
/// so an untouched launch takes the fall-through (`stp`, exit 0); flipping
/// MF via `setVariable` before `continue` must flip which arm runs
/// (`hlt` via `TAKEN`, exit 2).
const MF_CHECK_PROGRAM: &str = "\
.func main
        jm      TAKEN
        stp
TAKEN:  hlt
";

#[test]
fn mf_set_flips_a_following_checks_arm() {
    let dir = scratch("mf-baseline");
    let program = write_pmx(&dir, "mfcheck", MF_CHECK_PROGRAM);

    // Baseline: MF starts false on a blank tape, so `jm` does not jump.
    let mut baseline = PmDapAdapter::new();
    let mut out = Vec::new();
    baseline
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();
    baseline
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    let events = drive_to_pause_or_done(&mut baseline);
    match events.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 0, "MF false: falls through to stp");
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }

    // Flipped: setVariable(MF, true) before continuing must take `jm`.
    let mut flipped = PmDapAdapter::new();
    let mut out = Vec::new();
    flipped
        .handle("launch", &launch_args(&program, true), &mut out)
        .unwrap();
    flipped
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    // `stopOnEntry` pauses BEFORE any step (module doc), so the entry
    // instruction (`ent`) has not retired yet — its own first-step initial
    // mark latch (docs/core.md (initial mark latch)) would otherwise
    // overwrite an MF set now the instant it runs. Step past it first
    // (instruction granularity: this raw `.pma` fixture carries no `-g`
    // map, so line granularity's "until the mapped line changes" loop
    // never sees a change and would run to completion in one call).
    flipped
        .handle("stepIn", &json!({"granularity": "instruction"}), &mut out)
        .unwrap();
    let scopes = flipped.handle("scopes", &Value::Null, &mut out).unwrap();
    let registers_ref = scope_ref(&scopes, "Registers");
    let set_response = flipped
        .handle(
            "setVariable",
            &json!({"variablesReference": registers_ref, "name": "MF", "value": "true"}),
            &mut out,
        )
        .unwrap();
    assert_eq!(set_response["value"], json!("true"));

    flipped.handle("continue", &Value::Null, &mut out).unwrap();
    let events = drive_to_pause_or_done(&mut flipped);
    match events.as_slice() {
        [
            AdapterEvent::Output { .. },
            AdapterEvent::Terminated,
            AdapterEvent::Exited { code },
        ] => {
            assert_eq!(*code, 2, "MF true: jm takes the branch to hlt");
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

#[test]
fn disassemble_renders_listing_line_text_and_the_top_frames_reference_resolves_within_it() {
    let dir = scratch("disassemble");
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);
    let call_line = line_of(CALLSTEP_PMC, "@callee()");

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
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
    drive_to_pause_or_done(&mut adapter); // stopped right before the call

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
    // The top frame's own reference resolves within the window: at
    // `instructionOffset` 0 (the default), it is the FIRST entry.
    assert_eq!(instructions[0]["address"], json!(top_ref));
    let text = instructions[0]["instruction"].as_str().unwrap();
    assert!(text.contains("call"), "got: {text}");
}

/// VS Code's real Disassembly-view request shape: a large negative
/// `instructionOffset` relative to the current frame's `memoryReference`.
/// Parses a disassemble row address of either sign for the monotonicity
/// checks below (`"0x1f"` / `"-0x2"`).
fn parse_row_address(s: &str) -> i128 {
    match s.strip_prefix("-0x") {
        Some(hex) => -i128::from_str_radix(hex, 16).unwrap(),
        None => i128::from_str_radix(s.strip_prefix("0x").unwrap(), 16).unwrap(),
    }
}

/// The POSITIONAL window contract (docs/dap.md (the Disassembly view)):
/// row `i` of the response is instruction ordinal
/// `idx + instructionOffset + i` — VS Code learns a new reference's
/// memory address from the row at index `-instructionOffset`, so a
/// window slid to the image start (the first, wrong fix here) teaches it
/// one late address for EVERY reference near the entry, and the
/// Disassembly view's current-instruction marker pins there forever (the
/// live pow2 symptom). Ordinals before the image pad with negative-
/// address placeholders (never `-1`, the client's skip-me sentinel);
/// addresses stay strictly increasing and distinct across the window.
#[test]
fn disassemble_negative_offset_pads_the_head_and_keeps_the_anchor_positional() {
    let dir = scratch("disassemble-neg-offset");
    let program = write_pmc_debug(&dir, "callstep", CALLSTEP_PMC);
    let call_line = line_of(CALLSTEP_PMC, "@callee()");

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
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
    drive_to_pause_or_done(&mut adapter); // stopped right before the call

    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let top_ref = trace["stackFrames"][0]["instructionPointerReference"]
        .as_str()
        .unwrap()
        .to_string();
    // `callee` compiles before `main` in the code image (declaration
    // order), so the call site is genuinely past the image start — a
    // window that clamps instead of pads would visibly misplace it.
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

    // Head padding: the fixture has far fewer than 50 instructions before
    // the call site, so row 0 is an out-of-image placeholder with a
    // NEGATIVE address; every row before the first real one is invalid,
    // every row from there to the anchor is real code.
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
        instructions[..first_real]
            .iter()
            .all(|e| e["presentationHint"] == json!("invalid")),
        "everything before the first real row is padding"
    );
    assert!(
        instructions[first_real..=50]
            .iter()
            .all(|e| e["instruction"] != json!("<out of range>")),
        "everything from the image start to the anchor is real code"
    );

    // Addresses are strictly increasing and distinct across the whole
    // window, and `-1` — the one value clients skip — never appears.
    let parsed: Vec<i128> = instructions
        .iter()
        .map(|e| parse_row_address(e["address"].as_str().unwrap()))
        .collect();
    assert!(parsed.windows(2).all(|w| w[0] < w[1]), "{parsed:?}");
    assert!(!parsed.contains(&-1));
}

/// Three instructions beyond the implicit `.func` entry: `ent`, `nop`,
/// `nop`, `stp` — FOUR `step_in` calls total. `pmt run --trace` prints one
/// line per `step_in` call unconditionally, terminal instruction included
/// (`trace_streams_lines_with_post_state_into_the_writer`,
/// `cli_programs.rs`), so a traced run here must emit exactly four
/// trace-format `Output` events, the last one naming `stp`.
const TRACE_PROGRAM: &str = "\
.func main
        nop
        nop
        stp
";

/// Extracts every trace-format (`"; MF="`-suffixed) console `Output` line,
/// in emission order.
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
fn trace_true_streams_one_output_event_per_step_in_call_including_the_terminal_one() {
    let dir = scratch("trace");
    let program = write_pmx(&dir, "trace", TRACE_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({
        "program": program.to_str().unwrap(),
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
    // ent, nop, nop, stp — the terminal `stp` line included, matching
    // `run --trace`'s own per-`step_in`-call count exactly (the "steps+1"
    // shape: `stats().steps` itself would read 3, since a terminal
    // instruction retires without incrementing it).
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

    // The termination summary must still be present, alongside the trace
    // lines, not replaced by them.
    match events.last_chunk::<2>() {
        Some([AdapterEvent::Terminated, AdapterEvent::Exited { code }]) => {
            assert_eq!(*code, 0);
        }
        other => panic!("expected the run to end in Terminated/Exited, got: {other:?}"),
    }
}

#[test]
fn trace_true_names_the_faulting_instruction_exactly_once_across_the_two_phase_trap_flow() {
    let dir = scratch("trace-trap");
    // `ret` on an empty return stack traps StackUnderflow — `ent` retires
    // normally first (mirrors `TRAP_PROGRAM` above).
    let program = write_pmx(&dir, "trap", TRAP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({
        "program": program.to_str().unwrap(),
        "trace": true,
        "stopOnEntry": false,
    });
    adapter.handle("launch", &args, &mut out).unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    // First phase: traps, pauses (does not yet finish).
    let first = drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Stopped);
    let first_lines = trace_lines(&first);
    // ent, then the faulting ret — exactly two lines, the second naming it.
    assert_eq!(first_lines.len(), 2, "got: {first:?}");
    assert!(
        first_lines[1].contains("ret"),
        "the last line of the first phase must name the faulting instruction, got: {first_lines:?}"
    );

    // Second phase: a further continue reaches Finished/Done. The
    // already-finished session must NOT re-emit the fault line — the
    // underlying `step_in` short-circuits with no new retirement, and
    // `step_once_traced` must recognize that rather than re-render the
    // same faulting address a second time.
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

#[test]
fn disassemble_past_the_code_image_advances_placeholder_addresses_monotonically() {
    let dir = scratch("disassemble-oob");
    let program = write_pmx(&dir, "stp", STP_PROGRAM); // tiny: ent + stp only

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    // Ask for far more instructions than the tiny program has — most rows
    // fall past the code image.
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
    // Strictly increasing across the whole window, in-range rows and
    // out-of-range placeholder rows alike — no two rows share an address.
    for pair in addresses.windows(2) {
        assert!(
            pair[1] > pair[0],
            "addresses must strictly increase, got: {addresses:?}"
        );
    }
    // At least one row is genuinely out of range (a 2-instruction program
    // cannot fill 10 rows) and is marked as such.
    assert!(
        instructions
            .iter()
            .any(|entry| entry["presentationHint"] == json!("invalid")),
        "expected at least one out-of-range row, got: {instructions:?}"
    );
}

// ---- target-mode launch (cli::driver::build_target_for_launch) --------

/// Writes a one-target manifest project into `dir`: `pmt.json` with
/// `project` set to `project_body` verbatim (the whole object after
/// `"project":`) plus `app.pmc` holding `source`. Each target-mode test
/// below needs a different corner of the schema (profile override, run
/// block, …), so the whole project body is the parameter.
fn write_target_project(dir: &Path, source: &str, project_body: &str) {
    fs::write(dir.join("app.pmc"), source).unwrap();
    fs::write(
        dir.join("pmt.json"),
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

/// The manifest's own debug profile disables `-g`; the adapter must
/// still force it — proven the way `dap_programs.rs` proves anything
/// -g-dependent: a source breakpoint on a mapped line answers
/// `verified: true`. If the seam's `force_debug_info` parameter were
/// ignored, this line would be `verified: false` (`UNMAPPED_BREAKPOINT_MESSAGE`).
#[test]
fn target_launch_forces_debug_info_so_breakpoints_verify() {
    let dir = scratch("target-force-g");
    let source = "main() {\n    mark;\n    halt;\n}\n";
    write_target_project(
        &dir,
        source,
        r#"{
            "profiles": { "debug": { "debug-info": false } },
            "targets": { "app": { "sources": ["app.pmc"] } }
        }"#,
    );

    let mut adapter = PmDapAdapter::new();
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
    let mark_line = line_of(source, "mark");
    let result = adapter
        .handle(
            "setBreakpoints",
            &json!({"breakpoints": [{"line": mark_line}]}),
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

/// A private, never-called function warns `unused-function` at compile
/// time without failing the build — the warning must reach the client as
/// its own `stderr` `Output` event, emitted before `Initialized`.
#[test]
fn target_launch_streams_build_warnings_as_stderr_output_before_initialized() {
    let dir = scratch("target-warnings");
    write_target_project(
        &dir,
        "main() {\n    halt;\n}\n\nhelper() {\n    mark;\n}\n",
        r#"{ "targets": { "app": { "sources": ["app.pmc"] } } }"#,
    );

    let mut adapter = PmDapAdapter::new();
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
            assert!(output.contains("unused function"), "got: {output}");
            assert!(output.contains("helper"), "got: {output}");
        }
        other => panic!("unexpected event sequence: {other:?}"),
    }
}

/// A tape block with a non-default alphabet (`"_"`/`"#"`, mirroring
/// `write_tape_block` above) and a SECOND cell so the block's own glyph
/// at index 1 (`"#"`) is distinguishable from `DEFAULT_GLYPHS`'s `"*"`
/// — the inline-tape branch of `run`/`build --run`'s tape resolution
/// (`"tape": " *"`) would ALSO render a marked cell as `'*'` even if
/// block loading were entirely broken, so a test meant to prove the
/// `tape-block` branch (root-relative path resolution +
/// `TapeSnapshot::alphabet`/block-alphabet fallback) has to use a block
/// whose own glyphs a coincidence can't produce.
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
    let path = dir.join(format!("{name}.pmt"));
    fs::write(&path, block.to_bytes().unwrap()).unwrap();
    path
}

/// The target's `run` block's `tape-block` must become the session's
/// initial tape, resolved manifest-relative (root-relative, not the
/// process cwd — `cli::driver::build_target_for_launch`'s own
/// `root.join(normalize_rel(..))` step, the one line in the seam's tape
/// resolution that is not a pure pass-through of `run.rs`'s rules) —
/// proven by reading back the block's own `"#"` glyph on `[1]` through
/// the ordinary `variables` path: a wrong-alphabet OR a wrong-path
/// failure both surface as a mismatch here, exactly as
/// `tape_window_marks_the_head_and_renders_glyphs_from_the_launch_alphabet`
/// proves program mode's own `"tape"` argument.
#[test]
fn target_launch_loads_the_tape_block_from_the_targets_run_settings() {
    let dir = scratch("target-tape");
    write_target_tape_block(&dir, "app");
    write_target_project(
        &dir,
        "main() {\n    halt;\n}\n",
        r#"{ "targets": { "app": {
            "sources": ["app.pmc"],
            "run": { "tape-block": "app.pmt" }
        } } }"#,
    );

    let mut adapter = PmDapAdapter::new();
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
    write_target_project(
        &dir,
        "main() { halt; }",
        r#"{ "targets": { "app": { "sources": ["app.pmc"] } } }"#,
    );

    let mut adapter = PmDapAdapter::new();
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

#[test]
fn launch_rejects_program_and_target_together() {
    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    let args = json!({"program": "x.pmx", "target": "app"});
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
/// `handle_continue`'s own `RunState::Done` guards) — carried scope from
/// Task 10's review (the gap was untested in BOTH adapters).
#[test]
fn configuration_done_and_continue_after_done_reject_without_reemitting_termination() {
    let dir = scratch("done-guard");
    let program = write_pmx(&dir, "stp", STP_PROGRAM);

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
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

// ---- source provenance: frame `source` objects and the per-file
// breakpoint filter (docs/dap.md (source provenance)) -------------------

/// `write_pmc_debug_multi`'s provenance sibling: writes each source AS A
/// REAL FILE into `dir` first (a frame's `source` object is attached only
/// when the resolved file exists), compiles each with debug info, and
/// links with per-unit `sources` naming those files relative to the
/// sidecar's directory — exactly the shape `pmt build` emits
/// (docs/formats.md (map sidecar)). A `None` file skips both the write
/// and the provenance, standing in for a prebuilt-object input.
fn write_pmc_debug_with_sources(dir: &Path, name: &str, units: &[(Option<&str>, &str)]) -> PathBuf {
    let mut objects = Vec::new();
    let mut sources = Vec::new();
    for (file, text) in units {
        if let Some(file) = file {
            fs::write(dir.join(file), text).unwrap();
        }
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
        sources.push(file.map(str::to_string));
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
    let path = dir.join(format!("{name}.pmx"));
    fs::write(&path, linked.executable.to_bytes()).unwrap();
    let mut map_path = path.clone().into_os_string();
    map_path.push(".map");
    fs::write(&map_path, linked.map.to_json()).unwrap();
    path
}

/// Steps into the program until frame 0 resolves to `function`, bounded —
/// a fixture change that makes the function unreachable fails the test
/// instead of hanging it.
fn step_into_function(adapter: &mut PmDapAdapter, function: &str) -> Value {
    for _ in 0..20 {
        let mut step_out = Vec::new();
        adapter
            .handle("stepIn", &Value::Null, &mut step_out)
            .unwrap();
        let trace = adapter
            .handle("stackTrace", &Value::Null, &mut step_out)
            .unwrap();
        if trace["stackFrames"][0]["name"] == json!(function) {
            return trace;
        }
    }
    panic!("never stepped into `{function}`");
}

#[test]
fn frames_carry_source_objects_for_provenanced_functions_only() {
    let dir = scratch("frame-source");
    let program = write_pmc_debug_with_sources(
        &dir,
        "provenance",
        &[
            (Some("app.pmc"), RETURN_CALLER_PMC),
            // No file, no provenance — a prebuilt-object stand-in.
            (None, RETURN_CALLEE_PMC),
        ],
    );

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();

    // At the entry stop, frame 0 is `main` — provenanced, so it carries
    // the `source` object with the file leaf and the ABSOLUTE resolved
    // path (the sidecar stores `app.pmc` relative to its own directory).
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let frame = &trace["stackFrames"][0];
    assert_eq!(frame["name"], json!("main"));
    assert_eq!(frame["source"]["name"], json!("app.pmc"));
    assert_eq!(
        frame["source"]["path"],
        json!(dir.join("app.pmc").to_str().unwrap())
    );

    // Inside `callee` — no provenance, no `source` key at all; the caller
    // frame behind it keeps its own.
    let trace = step_into_function(&mut adapter, "callee");
    let frames = trace["stackFrames"].as_array().unwrap();
    assert!(
        frames[0].get("source").is_none(),
        "an unprovenanced function must not invent a source: {trace}"
    );
    assert_eq!(frames[1]["source"]["name"], json!("app.pmc"));
}

#[test]
fn a_missing_source_file_omits_the_frame_source_object() {
    let dir = scratch("frame-source-missing");
    let program = write_pmc_debug_with_sources(
        &dir,
        "moved-tree",
        &[
            (Some("app.pmc"), RETURN_CALLER_PMC),
            (None, RETURN_CALLEE_PMC),
        ],
    );
    // The tree "moves": the source disappears while the map still names it.
    fs::remove_file(dir.join("app.pmc")).unwrap();

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let frame = &trace["stackFrames"][0];
    assert_eq!(frame["name"], json!("main"));
    assert!(
        frame.get("source").is_none(),
        "a dead path must degrade to a sourceless frame: {trace}"
    );
}

#[test]
fn set_breakpoints_filters_by_the_request_file() {
    let dir = scratch("bp-file-filter");
    // Both units carry a line 2 (`halt;` and `right(!);`) — the collision
    // the per-file filter exists to split.
    let program = write_pmc_debug_with_sources(
        &dir,
        "collide",
        &[
            (Some("a.pmc"), RETURN_CALLER_PMC),
            (Some("b.pmc"), RETURN_CALLEE_PMC),
        ],
    );

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();

    let plant = |adapter: &mut PmDapAdapter, file: &str| -> Value {
        adapter
            .handle(
                "setBreakpoints",
                &json!({
                    "source": {"path": dir.join(file).to_str().unwrap()},
                    "breakpoints": [{"line": 2}],
                }),
                &mut Vec::new(),
            )
            .unwrap()
    };

    let in_a = plant(&mut adapter, "a.pmc");
    let in_b = plant(&mut adapter, "b.pmc");
    assert_eq!(in_a["breakpoints"][0]["verified"], json!(true));
    assert_eq!(in_b["breakpoints"][0]["verified"], json!(true));
    assert_ne!(
        in_a["breakpoints"][0]["instructionReference"],
        in_b["breakpoints"][0]["instructionReference"],
        "the same line number in two files must plant at two addresses"
    );

    // A file the map never names: unverified, with the foreign-file
    // message — NOT a silent fall-through to the global table.
    let foreign = adapter
        .handle(
            "setBreakpoints",
            &json!({
                "source": {"path": dir.join("elsewhere.pmc").to_str().unwrap()},
                "breakpoints": [{"line": 2}],
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

    // The planted b.pmc breakpoint is live: the run pauses inside
    // `callee`, proving the filter picked the right unit's address.
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    drive_to_pause_or_done(&mut adapter);
    assert_eq!(adapter.run_state(), RunState::Stopped);
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    assert_eq!(trace["stackFrames"][0]["name"], json!("callee"));
}

#[test]
fn a_provenance_free_map_keeps_the_global_line_table() {
    let dir = scratch("bp-global-fallback");
    let program = write_pmc_debug(&dir, "legacy", CALLSTEP_PMC);
    let call_line = line_of(CALLSTEP_PMC, "@callee()");

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, false), &mut out)
        .unwrap();
    // The request names a file, but the map carries no provenance at all —
    // the pre-provenance global behavior applies, so the line verifies.
    let response = adapter
        .handle(
            "setBreakpoints",
            &json!({
                "source": {"path": dir.join("whatever.pmc").to_str().unwrap()},
                "breakpoints": [{"line": call_line}],
            }),
            &mut out,
        )
        .unwrap();
    assert_eq!(response["breakpoints"][0]["verified"], json!(true));
}

/// A frame in a function with NO mapped lines at all (provenance without
/// a line table) must stay SOURCELESS — DAP allows `line: 0` only on a
/// frame without a `source` object, and a real client hard-crashes
/// mapping a sourced frame's line 0 to editor line -1
/// (docs/dap.md (source provenance)). Mirrors the TM suite's test.
#[test]
fn frame_in_a_function_with_no_mapped_lines_stays_sourceless() {
    let dir = scratch("line0-bare");
    let program =
        write_pmc_debug_with_sources(&dir, "line0-bare", &[(Some("app.pmc"), CALLSTEP_PMC)]);

    // Hand-edit the sidecar into the no-lines shape while keeping the
    // provenance: every function loses its line table.
    let mut map_path = program.clone().into_os_string();
    map_path.push(".map");
    let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
    for f in map["functions"].as_array_mut().unwrap() {
        f["lines"] = json!([]);
    }
    fs::write(&map_path, map.to_string()).unwrap();

    let mut adapter = PmDapAdapter::new();
    let mut out = Vec::new();
    adapter
        .handle("launch", &launch_args(&program, true), &mut out)
        .unwrap();
    adapter
        .handle("configurationDone", &Value::Null, &mut out)
        .unwrap();
    let trace = adapter
        .handle("stackTrace", &Value::Null, &mut out)
        .unwrap();
    let frame = &trace["stackFrames"][0];
    assert_eq!(frame["name"], json!("main"));
    assert_eq!(frame["line"], json!(0));
    assert!(
        frame.get("source").is_none(),
        "a frame with no known line must not carry a source object \
         (DAP: line 0 is only legal sourceless), got: {frame}"
    );
}
