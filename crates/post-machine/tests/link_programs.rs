//! The first LINKED Post-machine programs: assemble → link → run,
//! relaxation economics measured in tacts, and the linked-executable
//! disassembly round trip.

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunOptions, RunStats};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::arch::opcodes::*;
use mtc_post_machine::asm::{
    assemble, disassemble_executable, disassemble_executable_with_map, link,
};

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
    let far = link(
        &[obj],
        &[],
        LinkOptions {
            relax: false,
            ..Default::default()
        },
    )
    .unwrap();
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

fn no_relax() -> LinkOptions {
    LinkOptions {
        relax: false,
        ..Default::default()
    }
}

#[test]
fn jump_only_callee_keeps_its_own_func_under_both_link_widths() {
    // `target` is reached ONLY by main's tail jump — never called, never
    // the entry. Its site's width is the linker's to choose, so the
    // disassembly has to keep it a symbol site: rendered as a local label
    // inside main it would become the assembler's choice instead, and a
    // link that held the site far would not survive reassembly.
    let src = "\
.func main
        jmp     @target
.func target
        stp
";
    // Objects: main = [ENT][JMP <hole>] (6 bytes), target = [ENT][STP].
    // Relaxation off, main first: target sits at 6, the far site ends at 6,
    // so its displacement is 0. Relaxation on, the site narrows to 2 bytes:
    // target moves to 3, the short site ends at 3, displacement 0 again.
    for (opts, expected, synthesized) in [
        (
            no_relax(),
            vec![ENT, JMP, 0, 0, 0, 0, ENT, STP],
            "func_0006",
        ),
        (
            LinkOptions::default(),
            vec![ENT, JMP_S, 0, ENT, STP],
            "func_0003",
        ),
    ] {
        let obj = assemble(src, true).unwrap();
        let out = link(&[obj], &[], opts.clone()).unwrap();
        assert_eq!(out.executable.code, expected);

        // Map-less (the synthesis the round-trip law runs on) and with the
        // map (the debugger view) must both reproduce the image.
        let mapless = disassemble_executable(&out.executable);
        let mapped = disassemble_executable_with_map(&out.executable, &out.map);
        for text in [&mapless, &mapped] {
            let out2 = link(&[assemble(text, false).unwrap()], &[], opts.clone()).unwrap();
            assert_eq!(out2.executable.code, out.executable.code, "{text}");
        }

        // …and both keep the boundary that makes that work.
        assert!(
            mapless.contains(&format!(".func {synthesized}")),
            "{mapless}"
        );
        assert!(
            mapless.contains(&format!("jmp     @{synthesized}")),
            "{mapless}"
        );
        assert!(mapped.contains(".func target"), "{mapped}");
        assert!(mapped.contains("jmp     @target"), "{mapped}");
    }
}

#[test]
fn a_folded_tail_jump_would_shift_the_reassembled_object_layout() {
    // The same fold is lossy even where the tail-jump site itself narrows
    // cleanly, because a folded site is 2 bytes of text where the object
    // held a 5-byte relocation. Every jump spanning it shrinks by 3 on
    // reassembly, and one sitting just past the signed-byte boundary flips
    // width — so the loss lands on an unrelated instruction, under a
    // default link. PAD is tuned to put `jm` exactly there.
    const PAD: usize = 120;
    let mut src = String::from(".func main\n        jm      LEND\n");
    for _ in 0..PAD {
        src.push_str("        nop\n");
    }
    src.push_str(
        "        jnm     L2
        jmp     @tail
L2:     nop
LEND:   stp
.func tail
        stp
",
    );

    // main's object blob, far site and both branches resolved by the
    // assembler's own fixpoint: ent(1) + jm(5) + PAD nops + jnm.s(2) +
    // jmp<hole>(5) + nop(1) + stp(1). `jm` reaches LEND at 6 + PAD + 8 =
    // 134 from its end at 6 — displacement 128, one past the signed byte,
    // so it is far and stays far. Linking narrows the tail-jump site to 2
    // bytes, which pulls LEND back to 131: `jm`'s displacement becomes 125
    // and would now fit a byte, but a linked jump never changes width.
    let mut expected = vec![ENT, JM];
    expected.extend(125i32.to_le_bytes());
    expected.extend(std::iter::repeat_n(NOP, PAD));
    expected.extend([JNM_S, 2, JMP_S, 2, NOP, STP, ENT, STP]);

    let obj = assemble(&src, false).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    assert_eq!(out.executable.code, expected);
    assert_eq!(out.report.relaxed_calls, 1); // the tail-jump site

    let text = disassemble_executable(&out.executable);
    let out2 = link(
        &[assemble(&text, false).unwrap()],
        &[],
        LinkOptions::default(),
    )
    .unwrap();
    assert_eq!(out2.executable.code, out.executable.code, "{text}");

    let tail = format!("func_{:04X}", PAD + 12);
    assert!(text.contains(&format!(".func {tail}")), "{text}");
    assert!(text.contains(&format!("jmp     @{tail}")), "{text}");
}

#[test]
fn an_entry_byte_in_a_body_is_promoted_only_across_a_control_flow_cut() {
    // `ent` is a landing pad that executes as a no-op, so an image can
    // legally hold one inside a body — where it is indistinguishable from a
    // function prologue, since an image records no function boundaries. A
    // jump onto one is therefore promoted to a root only when the boundary
    // it opens is a genuine cut of the code. These three are not, and each
    // would break in its own way if it were split: the halves become
    // separate functions the linker orders by discovery, so the program can
    // change, and a local edge that spanned the boundary is left with no
    // way to name its target at all.
    for (name, src) in [
        // Fall-through into the entry byte. Split, `main` runs off its end
        // into whatever was ordered next — here `helper`'s `ret`, which
        // turns a clean stop into a stack underflow.
        (
            "fall-through",
            "\
.func main
        call    helper
        jnm     L2
        jmp     L1
L2:     nop
L1:     ent
        stp
.func helper
        ret
",
        ),
        // A branch from above the boundary to below it — the minimal
        // witness: one function, one branch, one `ent`.
        (
            "forward edge",
            "\
.func main
        jnm     L3
        jmp     L1
L1:     ent
L3:     nop
        stp
",
        ),
        // …and from below the boundary back above it.
        (
            "backward edge",
            "\
.func main
        call    helper
LBACK:  nop
        jmp     L1
L1:     ent
        jm      LBACK
        stp
.func helper
        ret
",
        ),
    ] {
        for opts in [LinkOptions::default(), no_relax()] {
            let out = link(&[assemble(src, false).unwrap()], &[], opts.clone()).unwrap();
            let text = disassemble_executable(&out.executable);
            // The text must still link at all — a cross-region edge falls
            // back to `.byte`, which assembles and then fails to link.
            let out2 = link(&[assemble(&text, false).unwrap()], &[], opts).unwrap();
            assert_eq!(out2.executable.code, out.executable.code, "{name}:\n{text}");
        }
    }
}

#[test]
fn a_declined_boundary_keeps_the_program_it_described() {
    // The fall-through shape above, run rather than compared: the harm a
    // wrongly invented boundary does is not byte drift but a different
    // program. `main` marks, falls past `L2`, and stops; if the split
    // happened it would fall into `helper`'s `ret` instead and underflow.
    let src = "\
.func main
        call    helper
        jnm     L2
        jmp     L1
L2:     wr      1
L1:     ent
        stp
.func helper
        rgt
        ret
";
    let out = link(
        &[assemble(src, false).unwrap()],
        &[],
        LinkOptions::default(),
    )
    .unwrap();
    let text = disassemble_executable(&out.executable);
    let out2 = link(
        &[assemble(&text, false).unwrap()],
        &[],
        LinkOptions::default(),
    )
    .unwrap();

    let reg = registry();
    let mut outcomes = Vec::new();
    for exe in [&out.executable, &out2.executable] {
        // Blank tape, head 0: helper steps right, the unmatched cell sends
        // `jnm` to L2, which marks cell 1, then the `ent` no-op and `stp`.
        let mut tape = InfiniteTape::from_cells([], 0, 0);
        let machine = Machine::from_executable(exe, &reg).unwrap();
        let result = machine.run(&mut tape, RunOptions::default());
        outcomes.push((result.outcome, tape.marked_cells()));
    }
    assert_eq!(outcomes[0], (Outcome::Stopped, vec![1]));
    assert_eq!(outcomes[1], outcomes[0]);
}
