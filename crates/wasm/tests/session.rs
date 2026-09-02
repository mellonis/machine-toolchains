//! The Session owns the tapes; JS pumps it. These pin the pump events, the
//! final tapes against the goldens' derivations, seed validation, and the
//! after-stop contract.

use mtc_wasm::inner::program::build;
use mtc_wasm::inner::session::{
    Cause, Event, Limits, OutcomeInfo, Seed, Session, SessionError, check_tape_count,
};
use mtc_wasm::inner::{Arch, Lang};

const PMC_INC: &str = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";
const TMC_REPLACE_B: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";
/// Only 'a' has a rule; seeding 'b' traps on entry with no applicable transition.
const TMC_NO_TRANSITION: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['a'] -> move [>] goto scan;\n  }\n}\n";

fn no_limits() -> Limits {
    Limits {
        max_steps: None,
        max_tacts: None,
    }
}

fn seed(cells: &[u8]) -> Seed {
    Seed {
        cells: cells.to_vec(),
        head: 0,
        origin: 0,
    }
}

// `pump` returns on its first event; every event but `Finished` is a test
// failure here, so there is never a second iteration to loop back for — a
// plain match, not a loop.
fn run_to_end(s: &mut Session) -> mtc_wasm::inner::session::Finished {
    match s.pump(None).unwrap() {
        Event::Finished(f) => f,
        Event::Paused(c) => panic!("unexpected pause {c:?}"),
        Event::BudgetSpent => panic!("no budget was given"),
        Event::DeviceWait => panic!("owned devices are always ready"),
    }
}

#[test]
fn pmc_increment_runs_to_stopped_with_the_golden_tape() {
    let (program, _) = build(Lang::Pmc, PMC_INC, 1).unwrap();
    let mut s = Session::new(&program, &[seed(&[1, 1, 1])], no_limits()).unwrap();
    let fin = run_to_end(&mut s);
    assert!(matches!(fin.outcome, OutcomeInfo::Stopped));
    let snap = s.snapshot(0).unwrap();
    assert_eq!(snap.head, 0);
    assert_eq!(
        &snap.cells[..4],
        &[1, 1, 1, 1],
        "two becomes three; head back on the first mark"
    );
    assert!(snap.cells[4..].iter().all(|&c| c == 0));
    assert_eq!(snap.glyphs, vec![" ", "*"]);
    assert_eq!(snap.name, "tape");
    let stats = s.stop().unwrap();
    assert_eq!(stats.steps, fin.stats.steps);
    assert!(matches!(s.pump(None), Err(SessionError::Stopped)));
    assert!(matches!(s.snapshot(0), Err(SessionError::Stopped)));
}

#[test]
fn tmc_replace_b_runs_to_stopped_with_the_expected_tape() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    let mut s = Session::new(&program, &[seed(&[1, 2, 2])], no_limits()).unwrap();
    let fin = run_to_end(&mut s);
    assert!(matches!(fin.outcome, OutcomeInfo::Stopped));
    let snap = s.snapshot(0).unwrap();
    assert_eq!(snap.head, 3, "stopped on the first blank");
    assert_eq!(&snap.cells[..3], &[1, 1, 1], "every b became a");
    assert_eq!(snap.glyphs, vec!["_", "a", "b"]);
    assert_eq!(snap.name, "main");
}

#[test]
fn budget_pauses_without_losing_progress() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 0).unwrap();
    let mut s = Session::new(&program, &[seed(&[2, 2, 2, 2, 2, 2])], no_limits()).unwrap();
    let mut spent = 0;
    let fin = loop {
        match s.pump(Some(1)).unwrap() {
            Event::BudgetSpent => spent += 1,
            Event::Finished(f) => break f,
            other => panic!("{other:?}"),
        }
    };
    assert!(spent >= 6, "at least one instruction per cell: {spent}");
    assert_eq!(fin.stats.steps, s.stats().unwrap().steps);
    assert_eq!(&s.snapshot(0).unwrap().cells[..6], &[1, 1, 1, 1, 1, 1]);
}

#[test]
fn manual_pause_and_breakpoint_report_their_causes() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 0).unwrap();
    let mut s = Session::new(&program, &[seed(&[2, 2, 2])], no_limits()).unwrap();
    s.pause().unwrap();
    assert!(matches!(
        s.pump(None).unwrap(),
        Event::Paused(Cause::Manual)
    ));
    // A breakpoint at the current ip is not re-hit on resume; plant one at
    // the next instruction instead.
    let ip = s.ip().unwrap();
    let rows = mtc_wasm::inner::listing::rows(&program);
    let next = rows
        .iter()
        .find(|r| r.addr > ip)
        .expect("a later instruction")
        .addr;
    s.add_breakpoint(next).unwrap();
    match s.pump(None).unwrap() {
        Event::Paused(Cause::Breakpoint(at)) => assert_eq!(at, next),
        other => panic!("{other:?}"),
    }
    s.remove_breakpoint(next).unwrap();
    assert!(matches!(run_to_end(&mut s).outcome, OutcomeInfo::Stopped));
}

#[test]
fn step_limit_is_a_trap_with_its_kind() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 0).unwrap();
    let limits = Limits {
        max_steps: Some(2),
        max_tacts: None,
    };
    let mut s = Session::new(&program, &[seed(&[2, 2, 2, 2, 2, 2, 2, 2])], limits).unwrap();
    let fin = run_to_end(&mut s);
    match fin.outcome {
        OutcomeInfo::Trapped(t) => assert_eq!(t.kind, "step-limit"),
        other => panic!("{other:?}"),
    }
    assert!(
        s.finished().unwrap().is_some(),
        "the result is repeatable after finishing"
    );
}

#[test]
fn no_transition_is_a_trap_with_its_address() {
    let (program, _) = build(Lang::Tmc, TMC_NO_TRANSITION, 0).unwrap();
    let mut s = Session::new(&program, &[seed(&[2])], no_limits()).unwrap();
    let fin = run_to_end(&mut s);
    match fin.outcome {
        OutcomeInfo::Trapped(t) => {
            assert_eq!(t.kind, "no-transition");
            assert!(t.at.is_some(), "a positioned trap carries its address");
            assert!(
                t.detail.contains("no applicable transition"),
                "{}",
                t.detail
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn seeds_are_validated_against_the_band() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    assert!(matches!(
        Session::new(&program, &[seed(&[1, 7])], no_limits()),
        Err(SessionError::BadSeed {
            band: 0,
            index: 7,
            width: 3
        })
    ));
    assert!(matches!(
        Session::new(&program, &[seed(&[1]), seed(&[1])], no_limits()),
        Err(SessionError::TooManySeeds { given: 2, bands: 1 })
    ));
    let s = Session::new(&program, &[], no_limits()).unwrap();
    assert_eq!(s.bands(), 1, "missing seeds are blank bands");
    assert!(matches!(s.snapshot(5), Err(SessionError::NoSuchBand(5))));
}

#[test]
fn tape_count_guard_refuses_a_corrupt_tm_image_before_touching_the_machine() {
    assert!(matches!(
        check_tape_count(Arch::Tm1, 0),
        Err(SessionError::Load(_))
    ));
    assert!(matches!(
        check_tape_count(Arch::Tm1, 17),
        Err(SessionError::Load(_))
    ));
    assert!(check_tape_count(Arch::Tm1, 1).is_ok());
    assert!(check_tape_count(Arch::Tm1, 16).is_ok());
    // The compiler/linker never emits a multi-tape PM-1 image; the guard is
    // a no-op for `Lang::Pmc` regardless of the count.
    assert!(check_tape_count(Arch::Pm1, 0).is_ok());
    assert!(check_tape_count(Arch::Pm1, 17).is_ok());
}

#[test]
fn session_new_refuses_a_program_with_a_hand_corrupted_tape_count() {
    let (mut program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    program.exe.tape_count = 17;
    assert!(matches!(
        Session::new(&program, &[], no_limits()),
        Err(SessionError::Load(_))
    ));
}
