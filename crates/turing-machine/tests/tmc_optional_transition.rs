//! Optional transition in `.tmc` rules (docs/tmt/language.md (rules)): a rule
//! carrying at least one of write / move / debugger may omit its transition,
//! which means "stay in the current state" — a self-`goto`. In a grafted graph
//! the self-loop targets the rule's OWN spliced instance, never the shared
//! graph-source state, so two instances of one graph loop independently.
//!
//! Result cases are derivation-first: the expected final tape is derived by
//! hand here and the run must reproduce it.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, compile};

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

/// Compile, link, and run `src` on the given seed `(snapshot, width)` bands
/// (no CLI round-trip, matching `tmc_fold`/`tmc_golden`), asserting a
/// warning-free compile and returning the outcome and final per-tape snapshots.
fn compile_link_run(src: &str, seeds: &[(TapeSnapshot, u32)]) -> (Outcome, Vec<TapeSnapshot>) {
    let out =
        compile(src, CompileOptions::default()).unwrap_or_else(|e| panic!("must compile: {e}"));
    assert!(
        out.report.diagnostics.is_empty(),
        "expected no diagnostics, got {:?}",
        out.report.diagnostics
    );
    let exe: Executable = link(
        std::slice::from_ref(&out.object),
        &[],
        LinkOptions::default(),
    )
    .unwrap_or_else(|e| panic!("link failed: {e}"))
    .executable;
    let mut tapes: Vec<WideTape> = seeds
        .iter()
        .map(|(s, w)| WideTape::from_snapshot(s, *w).expect("seed fits its width"))
        .collect();
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");
    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
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
    let snaps = tapes.iter().map(WideTape::to_snapshot).collect();
    (result.outcome, snaps)
}

#[test]
fn omitted_transition_stays_in_state() {
    // ab = {'_'=0, 'a'=1, 'b'=2}. The scan rule omits its transition, so after
    // writing 'b' and stepping right it STAYS in `scan`. Seed "aa_" (cells
    // [1,1,0], head 0):
    //   [0]='a' → write 'b', >   ⇒ cell0='b'(2), head 1, stay scan
    //   [1]='a' → write 'b', >   ⇒ cell1='b'(2), head 2, stay scan
    //   [2]='_' → stop           ⇒ head 2
    // Derived final tape: "bb_" (cells [2,2,0], head 2).
    let src = "\
alphabet ab { '_', 'a', 'b' }
machine {
  tape t: ab;
  entry state scan {
    ['a'] -> write ['b'] move [>];
    ['_'] -> stop;
  }
}
";
    let (outcome, snaps) = compile_link_run(src, &[(snap(0, &[1, 1, 0], 0), 3)]);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(snaps, vec![snap(0, &[2, 2, 0], 2)]);
}

#[test]
fn omitted_transition_in_graph_self_loops_to_instance() {
    // marks = {'_'=0, 'x'=1, 'z'=2}. `findX`'s scan rule `[*] -> move [>];`
    // omits its transition — it self-loops to walk. The graph is grafted TWICE
    // (`first`, `second`); a self-loop must target the SPLICED instance's own
    // walk, so each instance scans independently. Chained: first → mid → second
    // → win. Seed "zxzx" (cells [2,1,2,1], head 0):
    //   first.walk : [0]='z' → >              ⇒ head 1  (first's self-loop)
    //   first.walk : [1]='x' → found = mid    ⇒ head 1
    //   mid        : [*]     → >, goto second ⇒ head 2
    //   second.walk: [2]='z' → >              ⇒ head 3  (second's self-loop)
    //   second.walk: [3]='x' → found = win    ⇒ head 3
    //   win        : [*]     → stop           ⇒ head 3
    // No writes anywhere, so the tape is unchanged; head ends at 3.
    let src = "\
alphabet marks { '_', 'x', 'z' }

graph findX(tape t: marks, state found, state missing) {
  entry state walk {
    ['x'] -> found;
    ['_'] -> missing;
    [*]   -> move [>];
  }
}

machine {
  tape work: marks;

  entry graft findX(t = work, found = mid, missing = lose) as first;
  graft findX(t = work, found = win, missing = lose) as second;

  state mid  { [*] -> move [>] second; }
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";
    let (outcome, snaps) = compile_link_run(src, &[(snap(0, &[2, 1, 2, 1], 0), 3)]);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(snaps, vec![snap(0, &[2, 1, 2, 1], 3)]);
}
