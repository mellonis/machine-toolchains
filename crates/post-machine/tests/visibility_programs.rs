//! Visibility end-to-end (spec §3/§6.2/§9 as amended by plan 6c):
//! locals coexist across objects, foreign locals are unreachable,
//! nesting mangles and runs, and the visibility flip changed no bytes.

use mtc_core::linker::{LinkError, LinkOptions};
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunLimits, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::{disassemble_object, link};
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;

fn run_exe(
    exe: &mtc_core::formats::executable::Executable,
    cells: &[bool],
    head: i64,
) -> (Outcome, Vec<i64>, i64) {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells(cells.iter().copied(), 0, head);
    let options = RunOptions {
        limits: RunLimits {
            max_steps: Some(10_000),
            ..Default::default()
        },
        ..Default::default()
    };
    let r = machine.run(&mut tape, options);
    (r.outcome, tape.marked_cells(), tape.head())
}

const LIB: &str = "helper() { right; } export api() { @helper(); mark(!); }";

#[test]
fn same_named_locals_coexist_across_objects() {
    // Library's local helper moves RIGHT; user's local helper moves LEFT.
    // Both link; neither shadows the other; DuplicateSymbol impossible.
    let lib = compile(LIB, CompileOptions::default()).unwrap();
    let user = compile(
        "helper() { left; } main() { @api(); @helper(); }",
        CompileOptions::default(),
    )
    .unwrap();
    let linked = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap();
    // Blank tape, head 0: api → lib helper: right (1), mark(!) writes 1;
    // main → user helper: left (0). Stop.
    let (outcome, marks, head) = run_exe(&linked.executable, &[false], 0);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(marks, vec![1]);
    assert_eq!(head, 0);
}

#[test]
fn foreign_locals_are_unresolved() {
    let lib = compile(LIB, CompileOptions::default()).unwrap();
    let user = compile("main() { @helper(); }", CompileOptions::default()).unwrap();
    let e = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap_err();
    assert_eq!(e, LinkError::Unresolved(vec!["helper".into()]));
}

#[test]
fn nested_functions_mangle_run_and_round_trip() {
    let src = "main() { walk() { right; check(1, !); 1: @walk(!); } @walk(); mark; }";
    let out = compile(src, CompileOptions::default()).unwrap();
    assert!(out.pma.contains(".func main.walk local"), "{}", out.pma);
    let text = disassemble_object(&out.object);
    assert!(text.contains(".func main.walk local"), "{text}");

    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    let (outcome, marks, head) = run_exe(&linked.executable, &[true, true, false], 0);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(marks, vec![0, 1, 2]);
    assert_eq!(head, 2);

    // -O1: "main.walk" != "main", so its self-call tail-converts; behavior
    // must match (this program terminates quickly on every tape used).
    let o1 = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let l1 = link(&[o1.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(
        run_exe(&l1.executable, &[true, true, false], 0),
        (Outcome::Stopped, vec![0, 1, 2], 2)
    );
}

#[test]
fn visibility_flip_changed_no_linked_bytes() {
    // The 6b inline golden lengths: symbol kinds changed, bytes did not.
    let src = "\
goToEnd() {
1:  right;
    check(1, 2);
2:  left;
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
";
    let o1 = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let l1 = link(&[o1.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(l1.executable.code.len(), 14);
    let o0 = compile(src, CompileOptions::default()).unwrap();
    let l0 = link(&[o0.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(l0.executable.code.len(), 18);
}

#[test]
fn locals_still_appear_in_the_map() {
    let lib = compile(LIB, CompileOptions::default()).unwrap();
    let user = compile("main() { @api(); }", CompileOptions::default()).unwrap();
    let linked = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap();
    let names: Vec<&str> = linked
        .map
        .functions
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert!(names.contains(&"helper"), "{names:?}"); // local, reached, mapped
}
