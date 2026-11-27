//! A binding name repeated within one rule's pattern is refused
//! (docs/tmt/language.md (pattern cells)).

use mtc_turing_machine::compiler::{CompileOptions, compile};

const A1: &str = "\
alphabet d { '0', '1', '2', '3', '4', '5', '6', '7' }
machine {
  tape a: d;
  tape b: d;
  entry state s {
    [0..7 as x, 0..7 as x] -> write [{x}, {x}] stop;
  }
}
";

const A1_NEAR: &str = "\
alphabet d { '0', '1', '2', '3', '4', '5', '6', '7' }
machine {
  tape a: d;
  tape b: d;
  entry state s {
    [0..7 as x, 0..7 as y] -> write [{y}, {x}] stop;
  }
}
";

const A1_TWO_RULES: &str = "\
alphabet d { '0', '1', '2', '3' }
machine {
  tape a: d;
  entry state s {
    [0..1 as x] -> write [{x}] move [>] goto s;
    [2..3 as x] -> write [{x}] stop;
  }
}
";

const A1_IN_GRAPH: &str = "\
alphabet d { '0', '1', '2', '3' }
graph g(tape a: d, tape b: d, state done) {
  entry state w {
    [0..3 as x, 0..3 as x] -> write [{x}, {x}] goto done;
  }
}
machine {
  tape a: d;
  tape b: d;
  entry graft g(a = a, b = b, done = stop) as gg;
}
";

/// Mutation: drop the repeat check in `check_rules` — the program compiles
/// again, the right cell silently winning.
#[test]
fn a_name_bound_twice_in_one_pattern_is_refused() {
    let err = compile(A1, CompileOptions::default()).unwrap_err();
    assert_eq!(err.kind.code(), "duplicate-pattern-binding");
    assert_eq!((err.span.start.line, err.span.start.col), (6, 25));
    assert_eq!(
        err.kind.to_string(),
        "`x` is bound by cell 1 and again by cell 2 of one rule in state `s` — each binding in a pattern needs its own name"
    );
}

/// Mutation: a check that flags any binding (comparing a binding with itself).
#[test]
fn distinct_names_in_one_pattern_compile() {
    compile(A1_NEAR, CompileOptions::default()).expect("x and y are two names");
}

/// Mutation: the name map kept across the rules of one state.
#[test]
fn one_name_in_two_rules_compiles() {
    compile(A1_TWO_RULES, CompileOptions::default()).expect("one binding per rule");
}

/// Mutation: the check run over the machine world only.
#[test]
fn a_repeat_inside_a_graph_body_is_refused() {
    let err = compile(A1_IN_GRAPH, CompileOptions::default()).unwrap_err();
    assert_eq!(err.kind.code(), "duplicate-pattern-binding");
    assert!(err.kind.to_string().contains("state `w`"), "{}", err.kind);
}
