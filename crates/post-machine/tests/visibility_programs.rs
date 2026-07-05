//! Visibility end-to-end (spec §3/§6.2/§9 as amended by plan 6c):
//! locals coexist across objects, foreign locals are unreachable,
//! nesting mangles and runs, and the visibility flip changed no bytes.
//! Task 6 appends the namespace goldens: `::`-mangled blocks (nestable,
//! reopenable), scoped `use` with paths and aliases, absolute qualified
//! calls, binding collisions, and the entry rule for namespaced `main`.

use mtc_core::linker::{LinkError, LinkOptions};
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunLimits, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::{disassemble_object, link};
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::ir::IrOp;
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
fn namespaces_prefix_nest_and_disambiguate() {
    let out = compile(
        "namespace a { export f() { left; } namespace b { export f() { right; } } } main() { mark; }",
        CompileOptions::default(),
    )
    .unwrap();
    let names: Vec<&str> = out.ir.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"a::f"));
    assert!(names.contains(&"a::b::f")); // namespace nesting + same bare name, distinct symbols
}

#[test]
fn qualified_use_binds_and_aliases() {
    let lib = compile(
        "namespace std { export goToEnd() { 1: right; check(1, 2); 2: left; } }",
        CompileOptions::default(),
    )
    .unwrap();
    let user = compile(
        "use std::goToEnd as go; main() { @go(); mark; }",
        CompileOptions::default(),
    )
    .unwrap();
    let linked = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap();
    let (outcome, ..) = run_exe(&linked.executable, &[true, true, false], 0);
    assert_eq!(outcome, Outcome::Stopped);
}

#[test]
fn namespace_members_are_bare_inside_qualified_outside() {
    // Inside the block, bare @helper works; outside, the member is
    // reachable only qualified (`@std::api()`) — a bare @api there would
    // be an undeclared external.
    let src =
        "namespace std { helper() { right; } export api() { @helper(); } } main() { @std::api(); }";
    let out = compile(src, CompileOptions::default()).unwrap();
    // And the bare form from file scope IS an undeclared external:
    let bare = compile(
        "namespace std { export api() { right; } } main() { @api(); }",
        CompileOptions::default(),
    )
    .unwrap();
    assert!(
        bare.report
            .warnings
            .iter()
            .any(|w| w.message.contains("undeclared external `api`"))
    );
    assert!(
        out.report
            .warnings
            .iter()
            .all(|w| !w.message.contains("undeclared"))
    );
    let api = out
        .ir
        .functions
        .iter()
        .find(|f| f.name == "std::api")
        .unwrap();
    assert!(api.blocks.iter().any(|b| {
        b.ops
            .iter()
            .any(|op| matches!(op, IrOp::Call { name, .. } if name == "std::helper"))
    }));
}

#[test]
fn deliberate_interposition_via_namespace_injection() {
    // User re-declares inside `std` — same symbol name, user wins.
    let lib = compile(
        "namespace std { export step() { right; } }",
        CompileOptions::default(),
    )
    .unwrap();
    let user = compile(
        "namespace std { export step() { left; } } use std::step; main() { @step(); }",
        CompileOptions::default(),
    )
    .unwrap();
    let linked = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap();
    let (_, _, head) = run_exe(&linked.executable, &[false], 0);
    assert_eq!(
        head, -1,
        "user's std::step (left) must win over the library's (right)"
    );
}

#[test]
fn reopened_namespaces_merge_within_a_file() {
    // Two `namespace std` blocks: members mutually bare-visible.
    let src = "namespace std { helper() { right; } } namespace std { export api() { @helper(); } } main() { @std::api(); }";
    let out = compile(src, CompileOptions::default()).unwrap();
    assert!(
        out.report
            .warnings
            .iter()
            .all(|w| !w.message.contains("undeclared"))
    );
    let api = out
        .ir
        .functions
        .iter()
        .find(|f| f.name == "std::api")
        .unwrap();
    assert!(api.blocks.iter().any(|b| {
        b.ops
            .iter()
            .any(|op| matches!(op, IrOp::Call { name, .. } if name == "std::helper"))
    }));
    // And a real duplicate across the two blocks still errors:
    let e = compile(
        "namespace std { f() { left; } } namespace std { f() { right; } } main() { mark; }",
        CompileOptions::default(),
    );
    assert!(e.is_err());
}

#[test]
fn namespace_scoped_imports_bind_inside_only() {
    let _lib = compile(
        "namespace std { export goToEnd() { 1: right; check(1, 2); 2: left; } }",
        CompileOptions::default(),
    )
    .unwrap();
    // Binding qq lives inside ns; main outside must not see it.
    let user = compile(
        "namespace ns { use std::goToEnd as qq; export walk() { @qq(); } } main() { @ns::walk(); }",
        CompileOptions::default(),
    )
    .unwrap();
    assert!(
        user.report
            .warnings
            .iter()
            .all(|w| !w.message.contains("undeclared"))
    );
    let outside = compile(
        "namespace ns { use std::goToEnd as qq; } main() { @qq(); }",
        CompileOptions::default(),
    )
    .unwrap();
    // qq out of scope at file level → bare undeclared external warning.
    assert!(
        outside
            .report
            .warnings
            .iter()
            .any(|w| w.message.contains("undeclared"))
    );
}

#[test]
fn qualified_calls_are_absolute_and_self_declaring() {
    // No `use` needed: the qualification is the declaration.
    let lib = compile(
        "namespace std { export goToEnd() { 1: right; check(1, 2); 2: left; } }",
        CompileOptions::default(),
    )
    .unwrap();
    let user = compile(
        "main() { @std::goToEnd(); mark; }",
        CompileOptions::default(),
    )
    .unwrap();
    assert!(
        user.report
            .warnings
            .iter()
            .all(|w| !w.message.contains("undeclared"))
    );
    let linked = link(&[user.object, lib.object], &[], LinkOptions::default());
    // lib passed as a USER object here for simplicity: main + std::goToEnd.
    assert!(linked.is_ok());
    // Inside a namespace, absolute self-reference equals bare:
    let ok = compile(
        "namespace std { helper() { right; } export api() { @std::helper(); } } main() { @std::api(); }",
        CompileOptions::default(),
    )
    .unwrap();
    assert!(
        ok.report
            .warnings
            .iter()
            .all(|w| !w.message.contains("undeclared"))
    );
}

#[test]
fn conflicting_bindings_error_and_aliases_disambiguate() {
    let e = compile(
        "use goToEnd; use std::goToEnd; main() { @goToEnd(); }",
        CompileOptions::default(),
    );
    assert!(e.is_err(), "two imports binding one bare name must error");
    let ok = compile(
        "use goToEnd; use std::goToEnd as stdGoToEnd; main() { @goToEnd(); @stdGoToEnd(); }",
        CompileOptions::default(),
    );
    assert!(ok.is_ok());
    // Collision key is the binding name AFTER aliasing:
    let e = compile(
        "use goToEnd as qq; use std::goToEnd as qq; main() { @qq(); }",
        CompileOptions::default(),
    );
    assert!(e.is_err(), "alias collisions are binding collisions");
    // Same import in DIFFERENT scopes: legal, inner shadows outer.
    let ok = compile(
        "use goToEnd; namespace ns { use goToEnd; export w() { @goToEnd(); } } main() { @ns::w(); }",
        CompileOptions::default(),
    );
    assert!(ok.is_ok());
}

#[test]
fn namespace_and_function_names_share_one_pool() {
    // Human-clarity guard (not collision — the ::/. split already
    // prevents that): namespace `a` + function `a` in one scope would
    // put a::helper and a.helper in the same file. Confusing; error.
    let e = compile(
        "namespace a { helper() { left; } } a() { helper() { right; } } main() { mark; }",
        CompileOptions::default(),
    );
    assert!(e.is_err());
}

#[test]
fn namespaced_main_is_not_the_entry_and_keywords_stay_contextual() {
    let out = compile(
        "namespace app { main() { mark; } }",
        CompileOptions::default(),
    )
    .unwrap();
    let e = link(&[out.object], &[], LinkOptions::default()).unwrap_err();
    assert_eq!(e, LinkError::NoEntrySymbol);
    // Even EXPORTED, a namespaced main is qq::main — not the entry.
    let out = compile(
        "namespace qq { export main() { mark; } }",
        CompileOptions::default(),
    )
    .unwrap();
    let e = link(&[out.object], &[], LinkOptions::default()).unwrap_err();
    assert_eq!(e, LinkError::NoEntrySymbol);
    assert!(
        compile(
            "namespace() { left; } as() { right; } main() { @namespace(); @as(); }",
            CompileOptions::default()
        )
        .is_ok()
    );
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
