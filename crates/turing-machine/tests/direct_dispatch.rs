//! The dispatch-target threading emission contract
//! (docs/tmt/optimizer.md (dispatch-target threading)).

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, Trap, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{assemble, link};
use mtc_turing_machine::compiler::{CompileOptions, CompileOutput, compile};
use mtc_turing_machine::ir::IrDispatch;
use mtc_turing_machine::optimizer::OptLevel;

// ── fixtures ─────────────────────────────────────────────────────────────

/// A conditional (Table-dispatch) state with one bare rule (`-> goto s1`)
/// and one payload rule (`write ['1'] goto s2`) — the `conditional()` codegen
/// path the dispatch-target-threading emission changes.
const BARE: &str = "\
alphabet a { '_', '1' }
machine {
  tape t: a;
  entry state s0 {
    ['_'] -> goto s1;
    ['1'] -> write ['1'] goto s2;
  }
  state s1 { [*] -> write ['1'] stop; }
  state s2 { [*] -> stop; }
}
";

/// A selective-then-catch-all state whose selective rule is bare: after
/// `dispatch_select` flips it to `Branch` and `jump_threading` marks the
/// selective rule `direct`, the `branch()` codegen path threads the `jm`
/// target straight to `other`.
const BRANCH_DIRECT: &str = "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state scan {
    ['a'] -> goto other;
    [*]   -> stop;
  }
  state other { [*] -> stop; }
}
";

// ── helpers (cribbed from tests/opt_equivalence.rs's TM build pattern) ─────

fn object_of(src: &str, level: OptLevel) -> CompileOutput {
    object_of_disabled(src, level, &[])
}

fn object_of_disabled(src: &str, level: OptLevel, disabled: &[&str]) -> CompileOutput {
    compile(
        src,
        CompileOptions {
            opt_level: level,
            disabled_passes: disabled.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
    )
    .expect("the program compiles")
}

fn emitted_asm(src: &str, level: OptLevel) -> String {
    object_of(src, level).tma
}

fn emitted_asm_disabled(src: &str, disabled: &[&str]) -> String {
    object_of_disabled(src, OptLevel::O1, disabled).tma
}

fn assert_assembles(text: &str) {
    assemble(text, false)
        .unwrap_or_else(|e| panic!("generated .tma failed to assemble: {e}\n{text}"));
}

fn link_default(obj: &ObjectFile) -> Executable {
    link(std::slice::from_ref(obj), &[], LinkOptions::default())
        .unwrap_or_else(|e| panic!("the default link failed: {e}"))
        .executable
}

/// A trap's KIND, stripped of its `at` offset (layout legitimately differs
/// between -O0 and -O1; the KIND is the invariant). Exhaustive on purpose.
fn trap_kind(t: Trap) -> &'static str {
    match t {
        Trap::InvalidOpcode { .. } => "invalid-opcode",
        Trap::CodeOutOfBounds { .. } => "code-out-of-bounds",
        Trap::BadOperand { .. } => "bad-operand",
        Trap::CallTargetNotEntry { .. } => "call-target-not-entry",
        Trap::StackOverflow => "stack-overflow",
        Trap::StackUnderflow => "stack-underflow",
        Trap::StepLimit => "step-limit",
        Trap::TactLimit => "tact-limit",
        Trap::Device { .. } => "device",
        Trap::NoTransition { .. } => "no-transition",
        Trap::TableOutOfBounds { .. } => "table-out-of-bounds",
        Trap::DispatchOutOfRange { .. } => "dispatch-out-of-range",
        Trap::UnmappedRead { .. } => "unmapped-read",
        Trap::UnmappedWrite { .. } => "unmapped-write",
        Trap::ExitOutOfRange { .. } => "exit-out-of-range",
        Trap::ProfileViolation { .. } => "profile-violation",
    }
}

fn outcome_kind(o: Outcome) -> String {
    match o {
        Outcome::Stopped => "stopped".to_string(),
        Outcome::Halted => "halted".to_string(),
        Outcome::Trapped(t) => format!("trapped:{}", trap_kind(t)),
    }
}

type Case = &'static [(&'static [u8], i64)];

struct Observed {
    outcome: String,
    snaps: Vec<TapeSnapshot>,
    heads: Vec<i64>,
}

fn run(exe: &Executable, seeds: Case) -> Observed {
    assert_eq!(
        seeds.len(),
        exe.tape_count as usize,
        "a case must seed exactly one tape per machine tape"
    );
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tapes: Vec<WideTape> = seeds
        .iter()
        .zip(&exe.alphabet_cardinalities)
        .map(|(&(cells, head), &width)| {
            WideTape::from_snapshot(
                &TapeSnapshot {
                    origin: 0,
                    cells: cells.to_vec(),
                    head,
                    alphabet: None,
                },
                width,
            )
            .expect("the seed fits the tape width")
        })
        .collect();
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
    let snaps: Vec<TapeSnapshot> = tapes.iter().map(WideTape::to_snapshot).collect();
    let heads = snaps.iter().map(|s| s.head).collect();
    Observed {
        outcome: outcome_kind(result.outcome),
        snaps,
        heads,
    }
}

// ── tests ────────────────────────────────────────────────────────────────

#[test]
fn o1_targets_name_the_state_and_the_stub_is_gone() {
    let o1 = emitted_asm(BARE, OptLevel::O1);
    // The bare rule's .targets line names s1's own label; the minted
    // s0__0-style stub label for that rule does not appear at all.
    assert!(o1.contains("s1"), "targets must name the state: {o1}");
    assert!(
        !o1.contains("s0__0"),
        "no stub block for the direct rule: {o1}"
    );
    // The payload rule keeps its stub:
    assert!(o1.contains("s0__1"), "{o1}");
    assert_assembles(&o1);
}

#[test]
fn o0_and_fno_jump_threading_are_byte_unchanged() {
    let o0 = emitted_asm(BARE, OptLevel::O0);
    assert!(o0.contains("s0__0"), "-O0 keeps the stub: {o0}");
    assert_assembles(&o0);
    let fno = emitted_asm_disabled(BARE, &["jump-threading"]);
    assert!(fno.contains("s0__0"), "--fno keeps the stub: {fno}");
    assert_assembles(&fno);
}

#[test]
fn a_branch_state_jm_targets_the_state_directly() {
    // Two-row selective-then-catch-all state whose selective rule is bare:
    // after dispatch_select + threading, the jm operand is the destination
    // state's label.
    let out = object_of(BRANCH_DIRECT, OptLevel::O1);
    let scan = &out.ir.worlds[0].states[0];
    assert_eq!(
        scan.dispatch,
        IrDispatch::Branch,
        "scan must reach the Branch dispatch shape for this test to be non-vacuous"
    );
    assert!(
        scan.rules[0].direct,
        "the selective rule must thread direct for this test to be non-vacuous"
    );

    let asm = out.tma;
    let jm_line = asm
        .lines()
        .find(|l| l.trim_start().starts_with("jm"))
        .unwrap_or_else(|| panic!("no jm line: {asm}"));
    assert!(
        jm_line.contains("other"),
        "jm must target other directly: {jm_line}"
    );
    assert!(
        !asm.contains("scan__m"),
        "no minted hit-block stub for the direct selective rule: {asm}"
    );
    // `other`'s own label must still print — it is now a genuine jm target,
    // not merely a fall-through-adjacent block.
    assert!(asm.contains("other:"), "{asm}");
    assert_assembles(&asm);
}

const CASE_BLANK: Case = &[(&[0], 0)];
const CASE_ONE: Case = &[(&[1], 0)];

#[test]
fn behavior_is_unchanged_o0_vs_o1() {
    let o0 = link_default(&object_of(BARE, OptLevel::O0).object);
    let o1 = link_default(&object_of(BARE, OptLevel::O1).object);
    for case in [CASE_BLANK, CASE_ONE] {
        let r0 = run(&o0, case);
        let r1 = run(&o1, case);
        assert_eq!(
            (&r0.outcome, &r0.snaps, &r0.heads),
            (&r1.outcome, &r1.snaps, &r1.heads),
            "O0 vs O1 divergence on case {case:?}"
        );
    }
}
