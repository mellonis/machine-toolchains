//! The first LINKED Post-machine programs: assemble → link → run,
//! relaxation economics measured in tacts, and the linked-executable
//! disassembly round trip.

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunOptions, RunStats};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::arch::opcodes::*;
use mtc_post_machine::asm::{assemble, disassemble_executable, link};

const SPEC_SAMPLE: &str = "\
.func goToEnd
L1:     rgt
        jm      L1
        lft
        ret

.func main
        call    goToEnd
        rgt
        wr      1
        stp
";

fn registry() -> ArchRegistry {
    let mut r = ArchRegistry::new();
    r.register(Box::new(Pm1));
    r
}

#[test]
fn spec_sample_links_byte_exact_and_runs() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    // Layout: main first. Relaxed: main = [ENT][CALL_S off][RGT][WR 81][STP]
    // = 7 bytes; goToEnd at 7 = [ENT][RGT][JM_S FD][LFT][RET].
    // call.s at 1, end 3 → off = 7 − 3 = 4.
    assert_eq!(
        out.executable.code,
        vec![
            ENT, CALL_S, 0x04, RGT, WR, 0x81, STP, ENT, RGT, JM_S, 0xFD, LFT, RET
        ]
    );
    assert_eq!(out.executable.entry, 0);
    assert_eq!(out.executable.arch, mtc_core::formats::ARCH_PM1);

    // Run on marks [0,1,2], head 0.
    let reg = registry();
    let machine = Machine::from_executable(&out.executable, &reg).unwrap();
    let mut tape = InfiniteTape::from_cells([true, true, true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Stopped);
    // goToEnd walks to head 3, lft → head 2, ret; main: rgt → head 3, wr 1.
    assert_eq!(tape.head(), 3);
    assert_eq!(tape.marked_cells(), vec![0, 1, 2, 3]);
    // Tacts (electronic), derived by hand:
    // core: ent 2 + call.s 5 + [ent 2 + 3×rgt 2 + 3×jm.s 3 + lft 2 + ret 3]
    //       + rgt 2 + wr 3 + stp 1 = 35; stall: moves/writes/latches = 12.
    // steps: 13 — the terminal stp returns Stopped before the Step event,
    // so it is fetched (1 core tact) but never step-counted.
    assert_eq!(
        result.stats,
        RunStats {
            steps: 13,
            core_tacts: 35,
            stall_tacts: 12
        }
    );
}

#[test]
fn relaxation_saves_exactly_three_fetch_tacts() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    let relaxed = link(std::slice::from_ref(&obj), &[], LinkOptions::default()).unwrap();
    let far = link(&[obj], &[], LinkOptions { relax: false }).unwrap();
    assert_eq!(far.executable.code.len(), relaxed.executable.code.len() + 3);

    let reg = registry();
    let mut t1 = InfiniteTape::from_cells([true, true, true], 0, 0);
    let mut t2 = InfiniteTape::from_cells([true, true, true], 0, 0);
    let r1 = Machine::from_executable(&relaxed.executable, &reg)
        .unwrap()
        .run(&mut t1, RunOptions::default());
    let r2 = Machine::from_executable(&far.executable, &reg)
        .unwrap()
        .run(&mut t2, RunOptions::default());
    assert_eq!(t1.marked_cells(), t2.marked_cells()); // same behavior
    assert_eq!(r2.stats.core_tacts, r1.stats.core_tacts + 3); // 3 more operand fetches
    assert_eq!(r2.stats.stall_tacts, r1.stats.stall_tacts);
}

#[test]
fn linked_executable_disassembly_reassembles_and_relinks_identically() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    let text = disassemble_executable(&out.executable);
    // Short call prints as far `call` with the synthesized root name:
    assert!(text.contains("call    func_0007"), "{text}");
    assert!(!text.contains("call.s"), "{text}");
    let obj2 = assemble(&text, false).unwrap();
    let out2 = link(&[obj2], &[], LinkOptions::default()).unwrap();
    assert_eq!(out2.executable.code, out.executable.code);
}

#[test]
fn map_names_the_functions() {
    let obj = assemble(SPEC_SAMPLE, true).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    let names: Vec<&str> = out.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["main", "goToEnd"]);
    assert_eq!(out.map.functions[1].labels, vec![("L1".to_string(), 8)]); // ent at 7, L1 at 8
    let json = out.map.to_json();
    assert_eq!(
        mtc_core::linker::MapFile::from_json(&json).unwrap(),
        out.map
    );
}

#[test]
fn report_accounts_for_drops_and_relaxations() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    let lib = assemble(".func spare\n        hlt\n", false).unwrap();
    let out = link(&[obj], &[lib], LinkOptions::default()).unwrap();
    assert_eq!(out.report.dropped, vec!["spare".to_string()]);
    assert_eq!(out.report.relaxed_calls, 1);
    assert_eq!(out.report.far_calls, 0);
}

#[test]
fn library_supplies_go_to_end_lazily() {
    let main_only = assemble(".func main\n        call    goToEnd\n        stp\n", false).unwrap();
    let lib = assemble(
        ".func goToEnd\nL:      rgt\n        jm      L\n        lft\n        ret\n.func unusedHelper\n        hlt\n",
        false,
    )
    .unwrap();
    let out = link(&[main_only], &[lib], LinkOptions::default()).unwrap();
    let names: Vec<&str> = out.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["main", "goToEnd"]); // unusedHelper dropped
    assert!(!out.executable.code.contains(&HLT));
}

#[test]
fn tail_call_layout_round_trips_through_disassembly() {
    // g is called (a root) AND tail-jumped: both forms must survive.
    let src = "\
.func main
        call    g
        rgt
        call    f
        stp
.func f
        lft
        jmp     @g
.func g
        ret
";
    let obj = assemble(src, false).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    let text = disassemble_executable(&out.executable);
    assert!(text.contains("jmp     @"), "{text}");
    let obj2 = assemble(&text, false).unwrap();
    let out2 = link(&[obj2], &[], LinkOptions::default()).unwrap();
    assert_eq!(out2.executable.code, out.executable.code);
}
