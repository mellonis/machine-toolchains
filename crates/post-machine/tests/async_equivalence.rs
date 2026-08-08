//! sync ≡ pump equivalence over real PM-1 programs: a pumped run through
//! always-ready adapters must match `Machine::run` bit-exactly — outcome,
//! stats, ip, stack — at -O0 and -O1; a latency device must change
//! nothing but the number of pump calls.

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{
    ArchRegistry, InfiniteTape, LatencyProfile, LatencyTape, Machine, PumpEvent, RunOptions,
    RunResult, SyncAsAsync,
};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::link;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;
use mtc_post_machine::stdlib;

/* local helpers, copied per golden_programs.rs's build pattern (source ->
.pmx Executable + machine construction + the PM-1 arch registry) — each
integration test file defines its own, repo convention. The stdlib object
is always offered as a link library (golden_programs.rs's own pattern):
unreferenced by most corpus entries, reachability drops it for them, and
`stdlib_user` below is the one entry that actually pulls it in. */

fn build(src: &str, opt: OptLevel) -> mtc_core::formats::executable::Executable {
    let out = compile(
        src,
        CompileOptions {
            opt_level: opt,
            ..Default::default()
        },
    )
    .expect("compiles");
    link(
        &[out.object],
        std::slice::from_ref(stdlib::object()),
        LinkOptions::default(),
    )
    .expect("links")
    .executable
}

fn machine<'a>(
    exe: &mtc_core::formats::executable::Executable,
    registry: &'a ArchRegistry,
) -> Machine<'a> {
    Machine::from_executable(exe, registry).expect("loads")
}

fn pump_to_end(machine: &Machine<'_>, opts: RunOptions) -> RunResult {
    let mut session = machine.async_session(opts);
    let mut tape = SyncAsAsync::new(InfiniteTape::new());
    loop {
        match session.pump(&mut [&mut tape], None) {
            PumpEvent::Finished(result) => return result,
            PumpEvent::DeviceWait | PumpEvent::BudgetSpent => continue,
            other => panic!("unexpected: {other:?}"),
        }
    }
}

const CORPUS: &[(&str, &str)] = &[
    ("mark_run", "main() { mark; right; mark; right; mark; }"),
    (
        "walk_and_erase",
        r#"
        export eraseOne() { unmark; right; }
        main() { mark; right; mark; left; @eraseOne(); @eraseOne(); }
    "#,
    ),
    (
        "branchy",
        r#"
        main() {
         1: mark;
            right;
            check(3, 5);
         3: unmark;
         5: mark;
            left;
        }
    "#,
    ),
    (
        "stdlib_user",
        "main() { mark; right; mark; right; mark; @std::goToBegin(); @std::eraseSection(); }",
    ),
];

#[test]
fn pumped_runs_match_sync_runs_across_the_corpus() {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    for (name, source) in CORPUS {
        for opt in [OptLevel::O0, OptLevel::O1] {
            let exe = build(source, opt);
            let m = machine(&exe, &registry);
            let mut sync_tape = InfiniteTape::new();
            let sync = m.run(&mut sync_tape, RunOptions::default());
            let pumped = pump_to_end(&m, RunOptions::default());
            assert_eq!(pumped, sync, "{name} at {opt:?}");
        }
    }
}

#[test]
fn latency_changes_nothing_but_the_pump_count() {
    // One corpus entry, run through a LatencyTape whose per-op costs match
    // TactProfile::ELECTRONIC (the profile RunOptions::default() drives the
    // sync run with) but whose polls force at least one WAIT per device
    // transaction: the accounting must land bit-identical to the sync run
    // and to the always-ready pumped run — latency changes only how many
    // times the embedder calls `pump`, never the observable result.
    let profile = LatencyProfile {
        move_polls: 3,
        read_polls: 3,
        write_polls: 3,
        move_cost: 1,
        read_cost: 1,
        write_cost: 1,
    };
    let (name, source) = CORPUS
        .iter()
        .find(|(n, _)| *n == "stdlib_user")
        .expect("stdlib_user is in the corpus");
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    for opt in [OptLevel::O0, OptLevel::O1] {
        let exe = build(source, opt);
        let m = machine(&exe, &registry);

        let mut sync_tape = InfiniteTape::new();
        let sync = m.run(&mut sync_tape, RunOptions::default());

        let mut session = m.async_session(RunOptions::default());
        let mut tape = LatencyTape::new(InfiniteTape::new(), profile);
        let mut waits = 0;
        let pumped = loop {
            match session.pump(&mut [&mut tape], None) {
                PumpEvent::Finished(result) => break result,
                PumpEvent::DeviceWait => {
                    waits += 1;
                    continue;
                }
                other => panic!("unexpected: {other:?} ({name} at {opt:?})"),
            }
        };
        assert_eq!(pumped, sync, "{name} at {opt:?}");
        assert_eq!(pumped.stats, sync.stats, "{name} at {opt:?}");
        assert!(
            waits > 0,
            "{name} at {opt:?}: expected at least one DeviceWait, saw none"
        );
    }
}
