//! First real Post-machine programs: hand-assembled PM-1 bytecode,
//! end-to-end through Executable → Machine → tape, with spec §4.4
//! tact arithmetic pinned exactly.

use mtc_core::formats::ARCH_PM1;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::vm::{
    ArchRegistry, InfiniteTape, LoadError, Machine, Outcome, RunLimits, RunOptions, RunStats,
    TactProfile, Trap,
};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::arch::opcodes::*;

fn registry() -> ArchRegistry {
    let mut r = ArchRegistry::new();
    r.register(Box::new(Pm1));
    r
}

fn machine_for(code: Vec<u8>) -> Executable {
    Executable {
        arch: ARCH_PM1,
        entry: 0,
        code,
    }
}

#[test]
fn go_to_end_walks_to_first_blank() {
    // ent; L: rgt; jm.s L; stp        (the 2012 goToEnd, hand-assembled)
    // jm.s at 2..4, instr_end 4, target 1 → off -3
    let code = vec![ENT, RGT, JM_S, 0xFD, STP];
    let reg = registry();
    let machine = Machine::from_executable(&machine_for(code), &reg).unwrap();

    // marks at 0,1,2 — head starts on 0
    let mut tape = InfiniteTape::from_cells([true, true, true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());

    assert_eq!(result.outcome, Outcome::Stopped);
    assert_eq!(tape.head(), 3); // first blank after the section
    assert_eq!(tape.marked_cells(), vec![0, 1, 2]); // tape unchanged
    // ent 2 | 3 × rgt (2 core + 2 stall) | 3 × jm.s 3 | stp 1
    assert_eq!(
        result.stats,
        RunStats {
            steps: 7,
            core_tacts: 18,
            stall_tacts: 6
        }
    );
}

#[test]
fn spec_tact_numbers_hold() {
    let reg = registry();

    // rgt: 2 core + 2 stall (program total: ent 2 + rgt 4 + stp 1)
    let m = Machine::from_executable(&machine_for(vec![ENT, RGT, STP]), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(
        r.stats,
        RunStats {
            steps: 2,
            core_tacts: 5,
            stall_tacts: 2
        }
    );

    // wr: 3 core + 2 stall (spec: wr = 5 total, electronic)
    let m = Machine::from_executable(&machine_for(vec![ENT, WR, 0x81, STP]), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(
        r.stats,
        RunStats {
            steps: 2,
            core_tacts: 6,
            stall_tacts: 2
        }
    );
    assert_eq!(t.marked_cells(), vec![0]);

    // call far = 8 core (spec §4.4): ent 2 + call 8 + ent 2 + ret 3 + stp 1
    let code = vec![ENT, CALL, 0x01, 0x00, 0x00, 0x00, STP, ENT, RET];
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(r.outcome, Outcome::Stopped);
    assert_eq!(
        r.stats,
        RunStats {
            steps: 4,
            core_tacts: 16,
            stall_tacts: 0
        }
    );

    // call.s = 5 core: ent 2 + call.s 5 + ent 2 + ret 3 + stp 1
    let code = vec![ENT, CALL_S, 0x01, STP, ENT, RET];
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(
        r.stats,
        RunStats {
            steps: 4,
            core_tacts: 13,
            stall_tacts: 0
        }
    );
}

#[test]
fn mechanical_profile_shows_the_stall_economy() {
    let reg = registry();
    let m = Machine::from_executable(&machine_for(vec![ENT, RGT, STP]), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let mech = TactProfile {
        move_cost: 50,
        read_cost: 5,
        write_cost: 10,
    };
    let r = m.run(
        &mut t,
        RunOptions {
            profile: mech,
            ..RunOptions::default()
        },
    );
    assert_eq!(r.stats.core_tacts, 5);
    assert_eq!(r.stats.stall_tacts, 55); // one move + one latch read
}

#[test]
fn call_to_non_entry_traps() {
    // call targets the stp byte (not ent)
    let code = vec![ENT, CALL, 0x01, 0x00, 0x00, 0x00, STP, STP];
    let reg = registry();
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(
        r.outcome,
        Outcome::Trapped(Trap::CallTargetNotEntry { target: 7 })
    );
}

#[test]
fn runaway_recursion_overflows_the_stack() {
    // ent; call -6 (targets its own ent → infinite recursion)
    let code = vec![ENT, CALL, 0xFA, 0xFF, 0xFF, 0xFF, STP];
    let reg = registry();
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let opts = RunOptions {
        stack_depth: 8,
        ..RunOptions::default()
    };
    let r = m.run(&mut t, opts);
    assert_eq!(r.outcome, Outcome::Trapped(Trap::StackOverflow));
}

#[test]
fn step_limit_stops_the_infinite_loop() {
    // ent; L: jmp.s L    (jmp.s at 1..3, instr_end 3, target 1 → off -2)
    let code = vec![ENT, JMP_S, 0xFE];
    let reg = registry();
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let opts = RunOptions {
        limits: RunLimits {
            max_steps: Some(1000),
            max_tacts: None,
        },
        ..RunOptions::default()
    };
    let r = m.run(&mut t, opts);
    assert_eq!(r.outcome, Outcome::Trapped(Trap::StepLimit));
}

#[test]
fn loader_rejects_bad_entry_and_unknown_arch() {
    let reg = registry();
    let bad_entry = Executable {
        arch: ARCH_PM1,
        entry: 0,
        code: vec![RGT, STP],
    };
    assert_eq!(
        Machine::from_executable(&bad_entry, &reg).unwrap_err(),
        LoadError::EntryNotEntryMarker { at: 0 }
    );
    let alien = Executable {
        arch: 0x42,
        entry: 0,
        code: vec![ENT, STP],
    };
    assert_eq!(
        Machine::from_executable(&alien, &reg).unwrap_err(),
        LoadError::UnknownArch(0x42)
    );
}

#[test]
fn pmt_in_run_pmt_out() {
    // Input tape-block file: marks at 0,1,2 and 4, head 0. Run goToEnd.
    let input = TapeBlockFile {
        alphabet: vec![" ".into(), "*".into()],
        tapes: vec![TapeSnapshot {
            origin: 0,
            cells: vec![1, 1, 1, 0, 1],
            head: 0,
        }],
    };
    let bytes = input.to_bytes();
    let parsed = TapeBlockFile::from_bytes(&bytes).unwrap();
    let mut tape = InfiniteTape::from_snapshot(&parsed.tapes[0]).unwrap();

    let code = vec![ENT, RGT, JM_S, 0xFD, STP];
    let reg = registry();
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let r = m.run(&mut tape, RunOptions::default());
    assert_eq!(r.outcome, Outcome::Stopped);
    assert_eq!(tape.head(), 3);

    // Snapshot the result back into a .pmt and round-trip it.
    let output = TapeBlockFile {
        alphabet: parsed.alphabet.clone(),
        tapes: vec![tape.to_snapshot()],
    };
    let out_bytes = output.to_bytes();
    let reparsed = TapeBlockFile::from_bytes(&out_bytes).unwrap();
    assert_eq!(reparsed.tapes[0].head, 3);
    assert_eq!(reparsed.tapes[0].cells, vec![1, 1, 1, 0, 1]); // data intact
}
