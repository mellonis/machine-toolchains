use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunLimits, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::link;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::stdlib;

/// Compile `source`, link against std, run from `cells`/`head`; return
/// (marked cells, final head). Cells index from origin 0.
fn run_std(source: &str, cells: &[bool], head: i64) -> (Vec<i64>, i64) {
    let out = compile(source, CompileOptions::default()).expect("compiles");
    let linked = link(
        &[out.object],
        std::slice::from_ref(stdlib::object()),
        LinkOptions::default(),
    )
    .expect("links");
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(&linked.executable, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells(cells.iter().copied(), 0, head);
    let result = machine.run(
        &mut tape,
        RunOptions {
            limits: RunLimits {
                max_steps: Some(100_000),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    assert_eq!(
        result.outcome,
        Outcome::Stopped,
        "program must stop normally"
    );
    (tape.marked_cells(), tape.head())
}

const M: bool = true;
const B: bool = false;

#[test]
fn go_to_end_lands_on_the_last_mark() {
    // {0,1,2} h0: right→1,2,3(blank)→left→2
    let (marks, head) = run_std("use std::goToEnd; main() { @goToEnd(!); }", &[M, M, M], 0);
    assert_eq!((marks, head), (vec![0, 1, 2], 2));
}

#[test]
fn go_to_begin_lands_on_the_first_mark() {
    let (marks, head) = run_std(
        "use std::goToBegin; main() { @goToBegin(!); }",
        &[M, M, M],
        2,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], 0));
}

#[test]
fn go_to_mark_right_finds_a_distant_mark() {
    // {4} h0: rights through 1..3, stops on 4
    let (marks, head) = run_std(
        "use std::goToMarkRight; main() { @goToMarkRight(!); }",
        &[B, B, B, B, M],
        0,
    );
    assert_eq!((marks, head), (vec![4], 4));
}

#[test]
fn go_to_mark_left_finds_a_distant_mark() {
    let (marks, head) = run_std(
        "use std::goToMarkLeft; main() { @goToMarkLeft(!); }",
        &[M, B, B, B],
        3,
    );
    assert_eq!((marks, head), (vec![0], 0));
}

#[test]
fn go_to_blank_right_exits_the_section() {
    let (marks, head) = run_std(
        "use std::goToBlankRight; main() { @goToBlankRight(!); }",
        &[M, M, M],
        0,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], 3));
}

#[test]
fn go_to_blank_left_exits_the_section() {
    let (marks, head) = run_std(
        "use std::goToBlankLeft; main() { @goToBlankLeft(!); }",
        &[M, M, M],
        2,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], -1));
}

#[test]
fn erase_section_clears_it_from_the_middle() {
    // {0..=3} h2: goToBegin→0; unmark,right ×4 → stops at 4
    let (marks, head) = run_std(
        "use std::eraseSection; main() { @eraseSection(!); }",
        &[M, M, M, M],
        2,
    );
    assert_eq!((marks, head), (vec![], 4));
}

#[test]
fn append_mark_grows_right() {
    // {0,1} h0: goToEnd→1; right→2; mark
    let (marks, head) = run_std(
        "use std::appendMark; main() { @appendMark(!); }",
        &[M, M],
        0,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], 2));
}

#[test]
fn prepend_mark_grows_left() {
    let (marks, head) = run_std(
        "use std::prependMark; main() { @prependMark(!); }",
        &[M, M],
        1,
    );
    assert_eq!((marks, head), (vec![-1, 0, 1], -1));
}

#[test]
fn remove_last_mark_shrinks_right() {
    // {0,1,2} h1: goToEnd→2; unmark; left→1
    let (marks, head) = run_std(
        "use std::removeLastMark; main() { @removeLastMark(!); }",
        &[M, M, M],
        1,
    );
    assert_eq!((marks, head), (vec![0, 1], 1));
}

#[test]
fn remove_first_mark_shrinks_left() {
    let (marks, head) = run_std(
        "use std::removeFirstMark; main() { @removeFirstMark(!); }",
        &[M, M, M],
        1,
    );
    assert_eq!((marks, head), (vec![1, 2], 1));
}

#[test]
fn remove_last_mark_on_a_single_mark_empties_the_tape() {
    // {0} h0: goToEnd stays (right→1 blank→left→0); unmark; left→-1
    let (marks, head) = run_std(
        "use std::removeLastMark; main() { @removeLastMark(!); }",
        &[M],
        0,
    );
    assert_eq!((marks, head), (vec![], -1));
}

#[test]
fn stdlib_compiles_clean_and_exports_exactly_the_roster() {
    use mtc_core::formats::object::SymbolDef;
    let out = compile(stdlib::SOURCE, CompileOptions::default()).expect("compiles");
    assert!(
        out.report.diagnostics.is_empty(),
        "{:?}",
        out.report.diagnostics
    );
    let mut names: Vec<&str> = stdlib::object()
        .symbols
        .iter()
        .filter(|s| matches!(s.def, SymbolDef::Defined { .. }))
        .map(|s| s.name.as_str())
        .collect();
    names.sort_unstable();
    let mut expected = vec![
        "std::appendMark",
        "std::eraseSection",
        "std::goToBegin",
        "std::goToBlankLeft",
        "std::goToBlankRight",
        "std::goToEnd",
        "std::goToMarkLeft",
        "std::goToMarkRight",
        "std::prependMark",
        "std::removeFirstMark",
        "std::removeLastMark",
    ];
    expected.sort_unstable();
    assert_eq!(names, expected);
}

#[test]
fn user_namespace_injection_overrides_a_std_routine() {
    // docs/pmt/stdlib.md interposition: same-namespace export, user beats library.
    let (marks, head) = run_std(
        "namespace std { export goToEnd() { 1: left(!); } }\n\
         use std::goToEnd; main() { @goToEnd(!); }",
        &[M, M, M],
        1,
    );
    assert_eq!((marks, head), (vec![0, 1, 2], 0)); // the override: one left
}
