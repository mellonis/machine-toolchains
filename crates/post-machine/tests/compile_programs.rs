//! The first COMPILED Post-machine programs (spec §11's golden path):
//! `.pmc` → compile → link → run, pinning the spec §3 sample end to end.

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::link;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::ir::IrProgram;

/// Spec §3's source sample, verbatim modulo comments.
const SPEC_PMC: &str = "\
goToEnd() {
1:  right;
    check(1, 2);
2:  left;
}

goToBegin() {
1:  left(2);
2:  check(1, 3);
3:  right(!);
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
";

const EXPECTED_PMA: &str = "\
.func goToEnd local
L1:
        rgt
        jm      L1
        lft
        ret
.func goToBegin local
L1:
        lft
        jm      L1
        rgt
        ret
.func main
        call    goToEnd
        rgt
        jnm     L4
        wr      0
        stp
L4:
        wr      1
        stp
";

fn registry() -> ArchRegistry {
    let mut r = ArchRegistry::new();
    r.register(Box::new(Pm1));
    r
}

#[test]
fn spec_sample_compiles_to_the_expected_assembly() {
    let out = compile(SPEC_PMC, CompileOptions::default()).unwrap();
    assert_eq!(out.pma, EXPECTED_PMA);
    assert!(out.report.warnings.is_empty());
}

#[test]
fn spec_sample_links_byte_exact() {
    use mtc_post_machine::arch::opcodes::*;
    let out = compile(SPEC_PMC, CompileOptions::default()).unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    // main: ent@0, call.s@1 (end 3, goToEnd at 12 → +9), rgt@3,
    // jnm.s@4 (end 6, L4 at 9 → +3), wr 0 @6..7, stp@8, wr 1 @9..10,
    // stp@11; goToEnd at 12: ent, rgt, jm.s −3, lft, ret.
    assert_eq!(
        linked.executable.code,
        vec![
            ENT, CALL_S, 0x09, RGT, JNM_S, 0x03, WR, 0x80, STP, WR, 0x81, STP, // main
            ENT, RGT, JM_S, 0xFD, LFT, RET, // goToEnd
        ]
    );
    assert_eq!(linked.executable.entry, 0);
}

#[test]
fn spec_sample_runs_and_drops_the_dead_function() {
    let out = compile(SPEC_PMC, CompileOptions::default()).unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(linked.report.dropped, Vec::<String>::new());

    // Marks at 0..=2, head on the first mark (the Plan 4 scenario).
    let reg = registry();
    let machine = Machine::from_executable(&linked.executable, &reg).unwrap();
    let mut tape = InfiniteTape::from_cells([true, true, true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Stopped);
    // goToEnd: right to the first blank (3), check → left (2), return;
    // main: right (3), check → blank arm → mark cell 3, stop.
    assert_eq!(tape.head(), 3);
    assert_eq!(tape.marked_cells(), vec![0, 1, 2, 3]);
}

#[test]
fn debug_build_maps_executable_offsets_to_pmc_lines() {
    let out = compile(
        SPEC_PMC,
        CompileOptions {
            debug_info: true,
            strip_debugger: false,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    let main = linked
        .map
        .functions
        .iter()
        .find(|f| f.name == "main")
        .unwrap();
    // rgt at absolute 3 ← `right;` on SPEC_PMC line 15.
    assert!(main.lines.contains(&(3, 15)), "{:?}", main.lines);
    assert!(
        main.labels.contains(&("L4".to_string(), 9)),
        "{:?}",
        main.labels
    );
    let go = linked
        .map
        .functions
        .iter()
        .find(|f| f.name == "goToEnd")
        .unwrap();
    // goToEnd's rgt at absolute 13 ← `right;` on line 2.
    assert!(go.lines.contains(&(13, 2)), "{:?}", go.lines);
    assert!(
        go.labels.contains(&("L1".to_string(), 13)),
        "{:?}",
        go.labels
    );
}

#[test]
fn emitted_ir_is_a_versioned_json_artifact() {
    let out = compile(SPEC_PMC, CompileOptions::default()).unwrap();
    let json = out.ir.to_json();
    let back = IrProgram::from_json(&json).unwrap();
    assert_eq!(back, out.ir);
    assert_eq!(back.version, 3);
    assert_eq!(back.functions.len(), 3);
}

#[test]
fn unicode_identifiers_survive_compile_and_link() {
    let src = "\
идиВКонец() {
1:  right;
    check(1, 2);
2:  left;
}

main() {
    @идиВКонец();
    mark;
}
";
    let out = compile(src, CompileOptions::default()).unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    let names: Vec<&str> = linked
        .map
        .functions
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, vec!["main", "идиВКонец"]);

    let reg = registry();
    let machine = Machine::from_executable(&linked.executable, &reg).unwrap();
    let mut tape = InfiniteTape::from_cells([true, true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Stopped);
    assert_eq!(tape.marked_cells(), vec![0, 1]);
}

#[test]
fn a_pmc_compiled_library_links_lazily() {
    let lib = compile(
        "export goToEnd() { 1: right; check(1, 2); 2: left; } unusedHelper() { halt; }",
        CompileOptions::default(),
    )
    .unwrap();
    let main = compile(
        "main() { @goToEnd(); right; mark; }",
        CompileOptions::default(),
    )
    .unwrap();
    let linked = link(&[main.object], &[lib.object], LinkOptions::default()).unwrap();
    let names: Vec<&str> = linked
        .map
        .functions
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, vec!["main", "goToEnd"]);
}

#[test]
fn halt_program_halts() {
    let out = compile("main() { right; halt; }", CompileOptions::default()).unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    let reg = registry();
    let machine = Machine::from_executable(&linked.executable, &reg).unwrap();
    let mut tape = InfiniteTape::from_cells([true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Halted);
}
