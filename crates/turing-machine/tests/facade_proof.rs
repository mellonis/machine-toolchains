//! The facade anatomy, proven end-to-end. The stdlib twins (a later phase)
//! are built almost entirely out of ONE construct: a `routine` whose whole
//! body is an `entry graft` of a behavior graph, with the graph's exits mapped
//! straight onto terminator continuations —
//!
//! ```tmc
//! routine f(tape t: X) { entry graft g(t = t, done = return); }
//! ```
//!
//! Phase 6a shipped every piece of the machinery (grafts parse in a routine
//! body; a graft binding accepts a terminator continuation like `done =
//! return`; expansion maps a terminator continuation to a `return`
//! transition) but NO 6a test compiled-and-RAN a graft-inside-a-routine with a
//! terminator continuation. This file is that proof: the load-bearing shape of
//! the stdlib is exercised through the full compile -> link -> run pipeline, so
//! the stdlib work stands on a demonstrated foundation rather than an assumed
//! one.
//!
//! The fixture wraps a TWO-exit walker so both continuation kinds are covered
//! in one routine: the `found` exit maps to `return` (the facade's essence —
//! it hands control back to the caller), and the `missing` exit maps to a
//! LOCAL state inside the routine. A caller machine calls the routine and, on
//! `return`, resumes at a distinguishable state that leaves an unmistakable
//! mark — so a broken return path could not masquerade as success.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, compile};

/// The facade fixture. `marks = {'_'=0, 'x'=1, 'y'=2}`.
///
/// - `seek` walks right: 'x' takes the `found` exit, '_' takes `missing`,
///   anything else steps right and keeps walking.
/// - `findX` is the FACADE: its entry is a graft of `seek` with `found =
///   return` (control returns to the caller) and `missing = stuck` (a local
///   state that halts). Nothing else — the routine IS the graft.
/// - `main` calls `findX`, and on return writes 'y' where the walk finished
///   and stops. Reaching `afterReturn` at all is the proof the return resumed
///   the caller; the write pins WHERE it resumed (the post-walk head).
const FACADE: &str = "\
alphabet marks { '_', 'x', 'y' }

graph seek(tape t: marks, state found, state missing) {
  entry state walk {
    ['x'] -> found;
    ['_'] -> missing;
    [*]   -> move [>] goto walk;
  }
}

routine findX(tape t: marks) {
  entry graft seek(t = t, found = return, missing = stuck) as scan;
  state stuck { [*] -> halt; }
}

machine {
  tape work: marks;
  entry state start {
    [*] -> call findX(t = work) then afterReturn;
  }
  state afterReturn { [*] -> write ['y'] stop; }
}
";

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

/// Compile the fixture (asserting warning-free) and link it with defaults.
fn build() -> Executable {
    let out = compile(FACADE, CompileOptions::default()).expect("the facade compiles");
    assert!(
        out.report.diagnostics.is_empty(),
        "the facade compiles warning-free, got {:?}",
        out.report.diagnostics
    );
    let obj: ObjectFile = out.object;
    link(std::slice::from_ref(&obj), &[], LinkOptions::default())
        .expect("the facade links")
        .executable
}

/// Run `exe` on one `marks` band (width 3) built directly from a snapshot.
fn run(exe: &Executable, seed: TapeSnapshot) -> (Outcome, TapeSnapshot) {
    let mut tape = WideTape::from_snapshot(&seed, 3).expect("seed fits width 3");
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut devices: Vec<&mut dyn Tape> = vec![&mut tape as &mut dyn Tape];
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(1_000_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    drop(devices);
    (result.outcome, tape.to_snapshot())
}

#[test]
fn return_continuation_resumes_the_caller() {
    // Seed "yx" (cells [2,1], head 0):
    //   start[0]='y'      -> call findX(t=work) then afterReturn, head 0
    //   findX/walk[0]='y' -> [*] move [>] goto walk, head 1
    //   findX/walk[1]='x' -> ['x'] found => RETURN, head 1
    //   (return resumes the caller at afterReturn, head 1)
    //   afterReturn[1]='x'-> [*] write ['y'] stop, cell1='y'(2), head 1
    // Final "yy" (cells [2,2]), head 1 — the 'x' became 'y' ONLY because the
    // return resumed `afterReturn`; the head at 1 proves the walk moved right.
    let exe = build();
    let (outcome, tape) = run(&exe, snap(0, &[2, 1], 0));
    assert_eq!(outcome, Outcome::Stopped, "the return path stops");
    assert_eq!(
        tape,
        snap(0, &[2, 2], 1),
        "afterReturn ran at the post-walk head — the return resumed the caller"
    );
}

#[test]
fn local_state_continuation_halts_inside_the_routine() {
    // Seed "y" (cells [2], head 0) — no 'x' to the right:
    //   start[0]='y'      -> call findX(t=work) then afterReturn, head 0
    //   findX/walk[0]='y' -> [*] move [>] goto walk, head 1
    //   findX/walk[1]='_' -> ['_'] missing => goto stuck, head 1
    //   findX/stuck[1]='_'-> [*] halt, head 1
    // Halt inside the routine halts the whole machine; afterReturn never runs,
    // so the tape is unchanged ("y" with the head parked at the blank).
    let exe = build();
    let (outcome, tape) = run(&exe, snap(0, &[2], 0));
    assert_eq!(outcome, Outcome::Halted, "the missing path halts");
    assert_eq!(
        tape,
        snap(0, &[2, 0], 1),
        "the routine halted via its local state; the caller never resumed"
    );
}
