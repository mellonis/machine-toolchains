//! `.tmc` parser battery: the six canonical example programs parse verbatim, every
//! deliberately-absent construct is rejected with its frozen code, reserved
//! keywords are barred wherever a name is expected, the binding grammar and
//! doc/deprecated attachment work, and the CST is lossless (no comment
//! dropped; `clone() == self`).

use super::*;
use crate::cst::{DocRunKind, ReuseCarrier, RuleKind, TopKind, WorldKind};
use crate::lexer::{LexMode, lex, lex_with};

fn parse_src(src: &str) -> Result<Program, CompileError> {
    parse(&lex(src).unwrap())
}

fn err_code(src: &str) -> &'static str {
    parse_src(src).unwrap_err().kind.code()
}

fn machine(p: &Program) -> &Machine {
    p.machine.as_ref().expect("program has a machine block")
}

// ---------------------------------------------------------------------------
// The six canonical example programs, verbatim, all parse.
// ---------------------------------------------------------------------------

const A1: &str = "\
? Walk right; replace every 'b' with 'a'; stop at the first blank.

alphabet ab { '_', 'a', 'b' }

machine {
  tape main: ab;

  entry state scan {
    ['b'] -> write ['a'] move [>] goto scan;
    ['a'] ->            move [>] goto scan;
    ['_'] -> stop;
  }
}
";

const A2: &str = "\
alphabet bits { '_', '0', '1' }

machine {
  tape num: bits;                    // head on the least significant digit

  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;   // carry
    ['0'] -> write ['1'] stop;
    ['_'] -> write ['1'] stop;
  }
}
";

const A3: &str = "\
alphabet bits { '_', '0', '1' }

machine {
  tape src: bits;
  tape dst: bits;

  entry state copy {
    ['0'..'1' as c, *] -> write [-, {c}] move [>, >] goto copy;
    ['_', *]           -> stop;
  }
}
";

const A4: &str = "\
alphabet bytes { 0..126 }            // 127 symbols; blank = index 0, glyph \"0\"

machine {
  tape cell: bytes;

  entry state inc {
    [1..125 as v] -> write [{v+1}] stop;
    [126]         -> halt;             // overflow
    [0]           -> write [1] stop;   // blank cell = value 0
  }
}
";

const A5: &str = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b', '0', '1' }

namespace mylib {
  export routine plusOne(tape num: bits) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*]   -> write ['1'] return;
    }
  }
}

use mylib::plusOne;

machine {
  tape ctl:  bits;
  tape data: wide;

  entry state main {
    ['1', *] -> call plusOne(num = data with map { '0'->'0', '1'->'1' }) then done;
    [*, *]   -> move [>, .] goto main;
  }

  state done { [*, *] -> stop; }
}
";

const A6: &str = "\
alphabet marks { '_', 'x', 'y', 'z' }

export graph findX(tape t: marks, state found, state missing) {
  entry state walk {
    ['x'] -> found;
    ['_'] -> missing;
    [*]   -> move [>] goto walk;
  }
}

machine {
  tape work: marks;

  entry graft findX(t = work, found = celebrate, missing = giveUp) as seek;

  state celebrate { [*] -> write ['_'] stop; }
  state giveUp    { [*] -> halt; }
}
";

#[test]
fn a1_replace_b_parses() {
    let p = parse_src(A1).expect("A.1 parses");
    assert_eq!(p.alphabets.len(), 1);
    assert_eq!(p.alphabets[0].name, "ab");
    // The leading `?` line documents the alphabet.
    assert!(p.alphabets[0].doc.is_some());
    let m = machine(&p);
    assert_eq!(m.tapes.len(), 1);
    assert_eq!(m.tapes[0].name, "main");
    assert_eq!(m.states.len(), 1);
    let scan = &m.states[0];
    assert!(scan.entry);
    assert_eq!(scan.name, "scan");
    assert_eq!(scan.rules.len(), 3);
    // Last rule: `['_'] -> stop;` — a bare terminator transition.
    assert!(matches!(scan.rules[2].transition, Transition::Stop { .. }));
    assert!(scan.rules[2].write.is_none());
    assert!(scan.rules[2].mov.is_none());
}

#[test]
fn a2_binary_increment_parses_through_trailing_comments() {
    let p = parse_src(A2).expect("A.2 parses");
    let inc = &machine(&p).states[0];
    assert_eq!(inc.name, "inc");
    assert_eq!(inc.rules.len(), 3);
    // `['1'] -> write ['0'] move [<] goto inc;`
    let r0 = &inc.rules[0];
    assert!(matches!(&r0.write, Some(w) if w.cells.len() == 1));
    assert!(matches!(&r0.mov, Some(mv) if matches!(mv.cells[0].dir, MoveDir::Left)));
    assert!(
        matches!(&r0.transition, Transition::Goto { explicit: true, name, .. } if name == "inc")
    );
}

#[test]
fn a3_two_tape_copy_parses_range_binding_and_substitution() {
    let p = parse_src(A3).expect("A.3 parses");
    let m = machine(&p);
    assert_eq!(m.tapes.len(), 2);
    let copy = &m.states[0];
    let r0 = &copy.rules[0];
    assert_eq!(r0.pattern.cells.len(), 2);
    // cell 0: `'0'..'1' as c` — a glyph range bound to `c`.
    assert!(matches!(
        r0.pattern.cells[0].kind,
        PatternCellKind::Range { .. }
    ));
    assert_eq!(
        r0.pattern.cells[0]
            .binding
            .as_ref()
            .map(|b| b.name.as_str()),
        Some("c")
    );
    // cell 1: `*` — an unbound wildcard.
    assert!(matches!(
        r0.pattern.cells[1].kind,
        PatternCellKind::Wildcard
    ));
    // write `[-, {c}]` — keep then a substitution pass-through (a bare name).
    let w = r0.write.as_ref().expect("has a write vector");
    assert!(matches!(w.cells[0].kind, WriteCellKind::Keep));
    assert!(matches!(
        &w.cells[1].kind,
        WriteCellKind::Subst { expr }
            if matches!(&expr.kind, FoldExprKind::Var(name) if name == "c")
    ));
}

#[test]
fn a4_byte_increment_parses_number_ranges_and_plus_delta() {
    let p = parse_src(A4).expect("A.4 parses");
    assert_eq!(p.alphabets[0].elems.len(), 1);
    assert!(matches!(
        p.alphabets[0].elems[0],
        AlphabetElem::Range { .. }
    ));
    let inc = &machine(&p).states[0];
    assert_eq!(inc.rules.len(), 3);
    // `[1..125 as v] -> write [{v+1}] stop;` — now a `Bin{Add, Var, Int}`.
    let w = inc.rules[0].write.as_ref().unwrap();
    assert!(matches!(
        &w.cells[0].kind,
        WriteCellKind::Subst { expr }
            if matches!(
                &expr.kind,
                FoldExprKind::Bin { op: FoldOp::Add, lhs, rhs }
                    if matches!(&lhs.kind, FoldExprKind::Var(n) if n == "v")
                        && matches!(&rhs.kind, FoldExprKind::Int(1))
            )
    ));
    // `[126] -> halt;`
    assert!(matches!(inc.rules[1].transition, Transition::Halt { .. }));
}

#[test]
fn a5_routine_call_across_alphabets_parses() {
    let p = parse_src(A5).expect("A.5 parses");
    assert_eq!(p.alphabets.len(), 2);
    // The routine is namespaced and exported.
    assert_eq!(p.routines.len(), 1);
    let r = &p.routines[0];
    assert_eq!(r.name, "plusOne");
    assert!(r.exported);
    assert_eq!(r.ns, vec!["mylib"]);
    assert_eq!(r.sig.params.len(), 1);
    assert!(matches!(r.sig.params[0].kind, SigParamKind::Tape { .. }));
    // The import.
    assert_eq!(p.imports.len(), 1);
    assert_eq!(p.imports[0].path, vec!["mylib", "plusOne"]);
    // The machine's `call … then done`.
    let m = machine(&p);
    let main = m.states.iter().find(|s| s.name == "main").unwrap();
    let Transition::Call {
        target, args, then, ..
    } = &main.rules[0].transition
    else {
        panic!("expected a call transition");
    };
    assert_eq!(target.joined(), "plusOne");
    assert_eq!(args.len(), 1);
    assert_eq!(args[0].name, "num");
    let BindingValue::Named { target, map, .. } = &args[0].value else {
        panic!("expected a named binding value");
    };
    assert_eq!(target, "data");
    let map = map.as_ref().expect("the binding carries a map");
    assert_eq!(map.pairs.len(), 2);
    assert!(matches!(map.pairs[0].arrow, MapArrow::Bidirectional));
    assert!(matches!(then, Continuation::State { name, .. } if name == "done"));
    assert!(m.states.iter().any(|s| s.name == "done"));
}

#[test]
fn a6_graph_graft_with_entry_instance_parses() {
    let p = parse_src(A6).expect("A.6 parses");
    assert_eq!(p.graphs.len(), 1);
    let g = &p.graphs[0];
    assert!(g.exported);
    assert_eq!(g.sig.params.len(), 3);
    assert!(matches!(g.sig.params[0].kind, SigParamKind::Tape { .. }));
    assert!(matches!(g.sig.params[1].kind, SigParamKind::State));
    assert!(matches!(g.sig.params[2].kind, SigParamKind::State));
    // Bare-name goto sugar: `['x'] -> found;`.
    let walk = &g.states[0];
    assert!(matches!(
        &walk.rules[0].transition,
        Transition::Goto { explicit: false, name, .. } if name == "found"
    ));
    // The machine's entry graft.
    let m = machine(&p);
    assert_eq!(m.grafts.len(), 1);
    let seek = &m.grafts[0];
    assert!(seek.entry);
    assert_eq!(seek.target.joined(), "findX");
    assert_eq!(seek.args.len(), 3);
    assert_eq!(seek.as_name.as_ref().map(|i| i.name.as_str()), Some("seek"));
}

// ---------------------------------------------------------------------------
// Deliberately absent constructs are rejected with the right code.
// ---------------------------------------------------------------------------

#[test]
fn multi_step_rule_body_is_rejected() {
    // A second `write` where the transition is expected.
    assert_eq!(
        err_code("machine { entry state s { ['a'] -> write ['a'] write ['b'] stop; } }"),
        "unexpected-token"
    );
    // A second `move`, likewise.
    assert_eq!(
        err_code("machine { entry state s { [*] -> move [>] move [<] stop; } }"),
        "unexpected-token"
    );
}

#[test]
fn omitted_transition_parses_as_stay_when_an_action_is_present() {
    // A rule carrying at least one of write / move / debugger may omit the
    // transition — the omission parses to `Transition::Stay`, later resolved
    // to a self-`goto` (stay in the current state).
    let p = parse_src("machine { entry state s { ['a'] -> write ['b'] move [>]; } }")
        .expect("omitted transition with an action parses");
    let r = &machine(&p).states[0].rules[0];
    assert!(matches!(r.transition, Transition::Stay { .. }));
    // move-only and debugger-only are enough on their own.
    let p = parse_src("machine { entry state s { [*] -> move [>]; } }")
        .expect("move-only omission parses");
    assert!(matches!(
        machine(&p).states[0].rules[0].transition,
        Transition::Stay { .. }
    ));
    let p = parse_src("machine { entry state s { [*] -> debugger; } }")
        .expect("debugger-only omission parses");
    assert!(matches!(
        machine(&p).states[0].rules[0].transition,
        Transition::Stay { .. }
    ));
}

#[test]
fn empty_rule_body_still_errors() {
    // `['a'] -> ;` — arrow then nothing. With no write / move / debugger the
    // transition is NOT optional, so this stays the "expected a transition"
    // error family.
    assert_eq!(
        err_code("machine { entry state s { ['a'] -> ; } }"),
        "unexpected-token"
    );
    let msg = parse_src("machine { entry state s { ['a'] -> ; } }")
        .unwrap_err()
        .to_string();
    assert!(msg.contains("transition"), "message: {msg}");
}

#[test]
fn call_without_then_still_errors() {
    // `then` stays mandatory on a `call`, action present or not.
    let msg = parse_src("machine { entry state s { [*] -> call f(); } }")
        .unwrap_err()
        .to_string();
    assert!(msg.contains("then"), "message: {msg}");
    let msg = parse_src("machine { entry state s { [*] -> move [>] call f(); } }")
        .unwrap_err()
        .to_string();
    assert!(msg.contains("then"), "message: {msg}");
}

#[test]
fn wildcard_binding_is_rejected() {
    assert_eq!(
        err_code("machine { entry state s { [* as v] -> stop; } }"),
        "wildcard-binding"
    );
}

#[test]
fn count_form_range_is_rejected() {
    // `'a'..3` — a mixed glyph/number range (there is no count form).
    assert_eq!(err_code("alphabet x { 'a'..3 }"), "range-kind-mismatch");
    // Also in a pattern position.
    assert_eq!(
        err_code("machine { entry state s { ['a'..3] -> stop; } }"),
        "range-kind-mismatch"
    );
}

#[test]
fn char_arithmetic_is_rejected() {
    // `c` is glyph-bound (a glyph range), so `{c+1}` is char arithmetic.
    assert_eq!(
        err_code("machine { entry state s { ['a'..'b' as c] -> write [{c+1}] stop; } }"),
        "char-arithmetic"
    );
    // The pass-through `{c}` on a glyph binding stays legal (delta 0).
    assert!(
        parse_src("machine { entry state s { ['a'..'b' as c] -> write [{c}] stop; } }").is_ok()
    );
    // Numeric arithmetic `{v+1}` stays legal.
    assert!(parse_src("machine { entry state s { [0..9 as v] -> write [{v+1}] stop; } }").is_ok());
}

#[test]
fn bare_single_tape_pattern_is_rejected() {
    assert_eq!(
        err_code("machine { entry state s { 'a' -> stop; } }"),
        "naked-pattern"
    );
    assert_eq!(
        err_code("machine { entry state s { 0 -> stop; } }"),
        "naked-pattern"
    );
    assert_eq!(
        err_code("machine { entry state s { * -> stop; } }"),
        "naked-pattern"
    );
}

#[test]
fn state_redirect_form_is_rejected() {
    assert_eq!(err_code("machine { entry state s; }"), "state-redirect");
    assert_eq!(err_code("machine { state s; }"), "state-redirect");
}

// ---------------------------------------------------------------------------
// Machine multiplicity and world-boundary structural checks.
// ---------------------------------------------------------------------------

#[test]
fn more_than_one_machine_in_a_file_is_rejected_at_parse() {
    assert_eq!(err_code("machine { } machine { }"), "multiple-machines");
}

#[test]
fn zero_machines_is_a_library_and_parses() {
    // A library file: no machine block. (The "a program needs one" rule is a
    // later semantic check — parsing is content with `machine: None`.)
    let p = parse_src("alphabet bits { '_', '0', '1' }").expect("a library parses");
    assert!(p.machine.is_none());
}

#[test]
fn machine_cannot_nest_in_a_namespace() {
    assert_eq!(err_code("namespace n { machine { } }"), "unexpected-token");
}

#[test]
fn tape_declaration_outside_a_machine_is_rejected() {
    assert_eq!(
        err_code("routine r() { tape x: bits; }"),
        "tape-not-in-machine"
    );
    assert_eq!(
        err_code("graph g() { tape x: bits; }"),
        "tape-not-in-machine"
    );
}

#[test]
fn volatile_tape_parses_in_a_machine_block() {
    let src = "machine { volatile tape sensor: bits; tape scratch: bits; }";
    let tokens = lex(src).unwrap();
    let cst = parse_cst(&tokens).expect("parses");
    let TopKind::Machine(m) = &cst.items[0].kind else {
        panic!("expected a machine block");
    };
    let tapes: Vec<&TapeCst> = m
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            WorldKind::Tape(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(tapes.len(), 2);
    let (sensor, scratch) = (tapes[0], tapes[1]);
    assert!(sensor.volatile);
    assert_eq!(sensor.name, "sensor");
    assert!(!scratch.volatile);
    assert_eq!(scratch.name, "scratch");

    // The tape's span starts at the `volatile` token, not at `tape`.
    let volatile_tok = tokens
        .iter()
        .find(|t| matches!(&t.kind, TokenKind::Ident(w) if w == "volatile"))
        .expect("a `volatile` token in the stream");
    assert_eq!(sensor.span.start, volatile_tok.span().start);
}

#[test]
fn volatile_must_be_followed_by_tape_in_a_world_body() {
    let err = parse_src("machine { volatile state s { [*] -> stop; } }").unwrap_err();
    assert_eq!(err.kind.code(), "unexpected-token");
    assert!(
        matches!(&err.kind, CompileErrorKind::Expected { what, .. } if *what == "`tape` after `volatile`"),
        "{err:?}"
    );
}

#[test]
fn volatile_tape_outside_a_machine_is_tape_not_in_machine() {
    assert_eq!(
        err_code(
            "routine r(tape t: bits) { volatile tape x: bits; entry state s { [*] -> stop; } }"
        ),
        "tape-not-in-machine"
    );
}

#[test]
fn volatile_tape_parameter_parses() {
    let src = "routine r(volatile tape t: bits, tape u: bits, state done) \
               { entry state s { [*] -> done; } }";
    let p = parse_src(src).expect("parses");
    let r = &p.routines[0];
    assert_eq!(r.sig.params.len(), 3);
    let SigParamKind::Tape { volatile, .. } = &r.sig.params[0].kind else {
        panic!("expected a tape parameter");
    };
    assert!(*volatile, "the first param is `volatile tape t`");
    let SigParamKind::Tape { volatile, .. } = &r.sig.params[1].kind else {
        panic!("expected a tape parameter");
    };
    assert!(!*volatile, "the second param is a plain `tape u`");
    assert!(matches!(r.sig.params[2].kind, SigParamKind::State));

    // The param's span starts at the `volatile` token, not at `tape`.
    let tokens = lex(src).unwrap();
    let volatile_tok = tokens
        .iter()
        .find(|t| matches!(&t.kind, TokenKind::Ident(w) if w == "volatile"))
        .expect("a `volatile` token in the stream");
    assert_eq!(r.sig.params[0].span.start, volatile_tok.span().start);
}

#[test]
fn volatile_state_parameter_is_an_error() {
    let err =
        parse_src("routine r(volatile state done) { entry state s { [*] -> done; } }").unwrap_err();
    assert_eq!(err.kind.code(), "unexpected-token");
    assert!(
        matches!(&err.kind, CompileErrorKind::Expected { what, .. }
            if *what == "`tape` after `volatile` (only tape parameters can be volatile)"),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// `writes`/`preserves` contract clauses on signature tape parameters.
// ---------------------------------------------------------------------------

#[test]
fn contract_clauses_round_trip_on_a_tape_param() {
    let src = "routine r(tape t: bits writes { 'a', 'b'..'c' } preserves { 'd' }) \
               { entry state s { [*] -> stop; } }";
    let p = parse_src(src).expect("parses");
    let r = &p.routines[0];
    assert_eq!(r.sig.params.len(), 1);
    let SigParamKind::Tape {
        writes, preserves, ..
    } = &r.sig.params[0].kind
    else {
        panic!("expected a tape parameter");
    };
    let writes = writes.as_ref().expect("a `writes` clause");
    assert_eq!(writes.elems.len(), 2);
    assert!(matches!(writes.elems[0], AlphabetElem::Single(_)));
    assert!(matches!(writes.elems[1], AlphabetElem::Range { .. }));
    let preserves = preserves.as_ref().expect("a `preserves` clause");
    assert_eq!(preserves.elems.len(), 1);
    assert!(matches!(preserves.elems[0], AlphabetElem::Single(_)));

    // `kw_span` pins to the keyword token itself; each clause's span starts
    // there and extends past it (through the closing `}`); `preserves`
    // starts only after `writes` closes; `SigParam.span` reaches the last
    // clause's close.
    let tokens = lex(src).unwrap();
    let writes_tok = tokens
        .iter()
        .find(|t| matches!(&t.kind, TokenKind::Ident(w) if w == "writes"))
        .expect("a `writes` token in the stream");
    assert_eq!(writes.kw_span, writes_tok.span());
    let preserves_tok = tokens
        .iter()
        .find(|t| matches!(&t.kind, TokenKind::Ident(w) if w == "preserves"))
        .expect("a `preserves` token in the stream");
    assert_eq!(preserves.kw_span, preserves_tok.span());
    assert_eq!(writes.span.start, writes.kw_span.start);
    assert!(writes.span.end > writes.kw_span.end);
    assert!(preserves.kw_span.start >= writes.span.end);
    assert_eq!(preserves.span.start, preserves.kw_span.start);
    assert!(preserves.span.end > preserves.kw_span.end);
    assert_eq!(r.sig.params[0].span.end, preserves.span.end);
}

#[test]
fn writes_clause_may_be_empty() {
    let src = "routine r(tape t: bits writes {}) { entry state s { [*] -> stop; } }";
    let p = parse_src(src).expect("an empty `writes {}` clause parses");
    let r = &p.routines[0];
    let SigParamKind::Tape {
        writes, preserves, ..
    } = &r.sig.params[0].kind
    else {
        panic!("expected a tape parameter");
    };
    let writes = writes.as_ref().expect("a `writes` clause");
    assert!(writes.elems.is_empty());
    assert!(preserves.is_none());
}

#[test]
fn preserves_clause_may_be_empty() {
    let src = "routine r(tape t: bits preserves {}) { entry state s { [*] -> stop; } }";
    let p = parse_src(src).expect("an empty `preserves {}` clause parses");
    let r = &p.routines[0];
    let SigParamKind::Tape {
        writes, preserves, ..
    } = &r.sig.params[0].kind
    else {
        panic!("expected a tape parameter");
    };
    assert!(writes.is_none());
    let preserves = preserves.as_ref().expect("a `preserves` clause");
    assert!(preserves.elems.is_empty());
}

#[test]
fn a_tape_param_with_no_clauses_leaves_both_fields_none() {
    let p =
        parse_src("routine r(tape t: bits) { entry state s { [*] -> stop; } }").expect("parses");
    let r = &p.routines[0];
    let SigParamKind::Tape {
        writes, preserves, ..
    } = &r.sig.params[0].kind
    else {
        panic!("expected a tape parameter");
    };
    assert!(writes.is_none());
    assert!(preserves.is_none());
}

#[test]
fn preserves_before_writes_is_a_targeted_order_error() {
    let err = parse_src(
        "routine r(tape t: bits preserves { 'a' } writes { 'b' }) \
         { entry state s { [*] -> stop; } }",
    )
    .unwrap_err();
    assert_eq!(err.kind.code(), "contract-clause-order");
    assert_eq!(
        err.kind.to_string(),
        "`writes` must come before `preserves`"
    );
}

#[test]
fn duplicate_writes_clause_is_an_error() {
    let err = parse_src(
        "routine r(tape t: bits writes { 'a' } writes { 'b' }) \
         { entry state s { [*] -> stop; } }",
    )
    .unwrap_err();
    assert_eq!(err.kind.code(), "duplicate-contract-clause");
    assert_eq!(err.kind.to_string(), "duplicate `writes` clause");
}

#[test]
fn duplicate_preserves_clause_is_an_error() {
    let err = parse_src(
        "routine r(tape t: bits preserves { 'a' } preserves { 'b' }) \
         { entry state s { [*] -> stop; } }",
    )
    .unwrap_err();
    assert_eq!(err.kind.code(), "duplicate-contract-clause");
    assert_eq!(err.kind.to_string(), "duplicate `preserves` clause");
}

#[test]
fn contract_clause_on_a_machine_tape_decl_is_rejected() {
    // Falls out of `parse_tape`'s own `;` expectation — clauses are
    // signature-only, and a machine tape decl never looks for one.
    let err = parse_src("machine { tape t: bits writes {}; entry state s { [*] -> stop; } }")
        .unwrap_err();
    assert_eq!(err.kind.code(), "unexpected-token");
    assert!(
        matches!(&err.kind, CompileErrorKind::Expected { what, .. } if *what == "`;`"),
        "{err:?}"
    );
}

#[test]
fn contract_clause_on_a_state_param_is_rejected() {
    // Falls out of `signature()`'s list loop — the `State` branch never
    // looks for a clause keyword, so `writes` here is just an unexpected
    // token where `,` or `)` was expected.
    let err = parse_src("routine r(state done writes {}) { entry state s { [*] -> done; } }")
        .unwrap_err();
    assert_eq!(err.kind.code(), "unexpected-token");
    assert!(
        matches!(&err.kind, CompileErrorKind::Expected { what, .. } if *what == "`,` or `)`"),
        "{err:?}"
    );
}

#[test]
fn non_entry_graft_needs_a_name() {
    assert_eq!(
        err_code("machine { graft findX(t = work); }"),
        "graft-needs-name"
    );
    // An entry graft may omit `as name`.
    assert!(parse_src("machine { entry graft findX(t = work); }").is_ok());
}

// ---------------------------------------------------------------------------
// Keyword-misuse battery — a reserved keyword is barred wherever a name goes.
// ---------------------------------------------------------------------------

#[test]
fn reserved_keywords_cannot_name_things() {
    // Each of these puts a reserved keyword where a name is expected.
    let cases = [
        "alphabet state { '_' }",
        "routine goto() { }",
        "graph move() { }",
        "namespace use { }",
        "machine { entry state move { [*] -> stop; } }",
        "machine { tape return: bits; }",
        "use mylib::graph;",
    ];
    for src in cases {
        assert_eq!(err_code(src), "reserved-name", "{src}");
    }
    // `deprecated` is contextual, not reserved — it can name things.
    assert!(parse_src("alphabet deprecated { '_' }").is_ok());
}

#[test]
fn volatile_is_reserved_as_a_name() {
    // A tape may not be NAMED volatile — the word is reserved.
    assert_eq!(
        err_code("machine { tape volatile: bits; }"),
        "reserved-name"
    );
}

#[test]
fn writes_is_reserved_as_a_name() {
    // A tape may not be NAMED writes — the word is reserved.
    assert_eq!(err_code("machine { tape writes: bits; }"), "reserved-name");
}

#[test]
fn preserves_is_reserved_as_a_name() {
    // A tape may not be NAMED preserves — the word is reserved.
    assert_eq!(
        err_code("machine { tape preserves: bits; }"),
        "reserved-name"
    );
}

// ---------------------------------------------------------------------------
// Binding grammar (the shared call/graft/bind algebra).
// ---------------------------------------------------------------------------

#[test]
fn binding_grammar_covers_maps_terminators_and_bare_targets() {
    let src = "\
machine {
  bind plusOne(num = data with map { '0'->'0', '1'=>'1' }, done = return) as inc;
  entry state s {
    [*] -> call inc() then halt;
  }
}
";
    let p = parse_src(src).expect("binding forms parse");
    let m = machine(&p);
    assert_eq!(m.binds.len(), 1);
    let b = &m.binds[0];
    assert_eq!(b.target.joined(), "plusOne");
    assert_eq!(b.as_name.name, "inc");
    assert_eq!(b.args.len(), 2);
    // arg 0: a tape target with a two-pair map (bidirectional + read-only).
    let BindingValue::Named { target, map, .. } = &b.args[0].value else {
        panic!("expected a named binding");
    };
    assert_eq!(target, "data");
    let map = map.as_ref().expect("map present");
    assert!(matches!(map.pairs[0].arrow, MapArrow::Bidirectional));
    assert!(matches!(map.pairs[1].arrow, MapArrow::ReadOnly));
    // arg 1: a terminator continuation.
    assert!(matches!(
        &b.args[1].value,
        BindingValue::Terminator {
            kind: TermKind::Return,
            ..
        }
    ));
    // The call transition: empty args, `then halt`.
    let Transition::Call { args, then, .. } = &m.states[0].rules[0].transition else {
        panic!("expected a call transition");
    };
    assert!(args.is_empty());
    assert!(matches!(then, Continuation::Halt { .. }));
}

#[test]
fn every_continuation_shape_parses() {
    for (cont, check) in [
        ("done", "state"),
        ("return", "return"),
        ("stop", "stop"),
        ("halt", "halt"),
    ] {
        let src = format!("machine {{ entry state s {{ [*] -> call f() then {cont}; }} }}");
        let p = parse_src(&src).unwrap();
        let Transition::Call { then, .. } = &machine(&p).states[0].rules[0].transition else {
            panic!("expected a call");
        };
        let got = match then {
            Continuation::State { .. } => "state",
            Continuation::Return { .. } => "return",
            Continuation::Stop { .. } => "stop",
            Continuation::Halt { .. } => "halt",
        };
        assert_eq!(got, check, "{cont}");
    }
}

// ---------------------------------------------------------------------------
// Doc lines, attention lines, `[deprecated]`.
// ---------------------------------------------------------------------------

#[test]
fn doc_run_attaches_and_reduces_onto_the_declaration() {
    let p = parse_src("? use it wisely\n! [deprecated] use plusOne instead\nalphabet ab { '_' }")
        .unwrap();
    let doc = p.alphabets[0].doc.as_ref().expect("documented alphabet");
    assert_eq!(doc.paragraphs, vec!["use it wisely"]);
    assert!(doc.attention.is_empty());
    assert_eq!(doc.deprecated.as_deref(), Some("use plusOne instead"));

    // A doc run also attaches to a routine (any doc-accepting declaration).
    let p = parse_src("? increments\nroutine r() { }").unwrap();
    assert!(p.routines[0].doc.is_some());

    // An undocumented declaration has `doc: None`.
    let p = parse_src("alphabet ab { '_' }").unwrap();
    assert!(p.alphabets[0].doc.is_none());
}

#[test]
fn dangling_doc_run_is_rejected() {
    // A run before a non-doc-accepting item (an import), at EOF, and before a
    // tape declaration in a world body.
    assert_eq!(err_code("? orphan\nuse mylib::x;"), "dangling-doc-run");
    assert_eq!(err_code("? orphan\n"), "dangling-doc-run");
    assert_eq!(
        err_code("machine {\n? orphan\ntape t: bits;\n}"),
        "dangling-doc-run"
    );
}

#[test]
fn doc_line_order_and_attribute_errors() {
    assert_eq!(
        err_code("! attn\n? doc\nalphabet ab { '_' }"),
        "doc-line-order"
    );
    assert_eq!(
        err_code("! [bogus] x\nalphabet ab { '_' }"),
        "unknown-attribute"
    );
    assert_eq!(
        err_code("! [deprecated] a\n! [deprecated] b\nalphabet ab { '_' }"),
        "duplicate-attribute"
    );
}

// ---------------------------------------------------------------------------
// CST losslessness.
// ---------------------------------------------------------------------------

/// Every comment text anywhere in the CST — the losslessness "nothing dropped"
/// probe.
fn collect_comments(cst: &Cst) -> Vec<String> {
    let mut out = Vec::new();
    collect_top(&cst.items, &mut out);
    out
}

fn push_doc_run(run: &[DocRunItem], out: &mut Vec<String>) {
    for it in run {
        if let DocRunKind::Comment(c) = &it.kind {
            out.push(c.text.clone());
        }
    }
}

fn collect_top(items: &[TopItem], out: &mut Vec<String>) {
    for item in items {
        match &item.kind {
            TopKind::Comment(c) => out.push(c.text.clone()),
            TopKind::Import(u) => {
                if let Some(c) = &u.trailing {
                    out.push(c.text.clone());
                }
            }
            TopKind::Alphabet(a) => {
                push_doc_run(&a.doc_run, out);
                out.extend(a.open_trailing.iter().map(|c| c.text.clone()));
                if let Some(c) = &a.close_trailing {
                    out.push(c.text.clone());
                }
            }
            TopKind::Namespace(n) => {
                push_doc_run(&n.doc_run, out);
                out.extend(n.open_trailing.iter().map(|c| c.text.clone()));
                collect_top(&n.items, out);
                if let Some(c) = &n.close_trailing {
                    out.push(c.text.clone());
                }
            }
            TopKind::Reuse(r) => {
                push_doc_run(&r.doc_run, out);
                out.extend(r.open_trailing.iter().map(|c| c.text.clone()));
                collect_world(&r.items, out);
                if let Some(c) = &r.close_trailing {
                    out.push(c.text.clone());
                }
            }
            TopKind::Machine(m) => {
                push_doc_run(&m.doc_run, out);
                out.extend(m.open_trailing.iter().map(|c| c.text.clone()));
                collect_world(&m.items, out);
                if let Some(c) = &m.close_trailing {
                    out.push(c.text.clone());
                }
            }
        }
    }
}

fn collect_world(items: &[WorldItem], out: &mut Vec<String>) {
    for item in items {
        match &item.kind {
            WorldKind::Comment(c) => out.push(c.text.clone()),
            WorldKind::Tape(t) => {
                if let Some(c) = &t.trailing {
                    out.push(c.text.clone());
                }
            }
            WorldKind::State(s) => {
                push_doc_run(&s.doc_run, out);
                out.extend(s.open_trailing.iter().map(|c| c.text.clone()));
                for ri in &s.rules {
                    match &ri.kind {
                        RuleKind::Comment(c) => out.push(c.text.clone()),
                        RuleKind::Rule(rc) => {
                            if let Some(c) = &rc.trailing {
                                out.push(c.text.clone());
                            }
                        }
                    }
                }
                if let Some(c) = &s.close_trailing {
                    out.push(c.text.clone());
                }
            }
            WorldKind::Graft(g) => {
                push_doc_run(&g.doc_run, out);
                if let Some(c) = &g.trailing {
                    out.push(c.text.clone());
                }
            }
            WorldKind::Bind(b) => {
                push_doc_run(&b.doc_run, out);
                if let Some(c) = &b.trailing {
                    out.push(c.text.clone());
                }
            }
        }
    }
}

#[test]
fn cst_significant_tokens_match_the_comment_free_stream() {
    // The significant-token walk (comments stripped) is identical to lexing the
    // comment-free twin — the first half of pmc's losslessness proof.
    let commented = A2; // has two trailing `//` comments
    let bare = "\
alphabet bits { '_', '0', '1' }
machine {
  tape num: bits;
  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;
    ['0'] -> write ['1'] stop;
    ['_'] -> write ['1'] stop;
  }
}
";
    let strip = |src: &str| -> Vec<TokenKind> {
        lex_with(src, LexMode::WithComments)
            .unwrap()
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
            .map(|t| t.kind)
            .collect()
    };
    assert_eq!(strip(commented), strip(bare));
}

#[test]
fn cst_retains_every_comment_and_clones_equal() {
    // Comments at each modelled position: a leading own-line comment, a
    // trailing comment after a `;`, a trailing after a rule, an open-brace
    // comment, a mid-run comment, and a dangling comment.
    let src = "\
// leading top comment
? doc /* not this — doc payload is verbatim */
// mid-run comment
alphabet ab { '_', 'a' } // after the alphabet
machine { // on the open brace
  tape num: ab; // after the tape
  entry state s {
    // a rule-list comment
    ['a'] -> stop; // after the rule
  }
}
// dangling end-of-file comment
";
    let tokens = lex_with(src, LexMode::WithComments).unwrap();
    let cst = parse_cst(&tokens).expect("parses with comments");

    // Every comment token in the input is retained somewhere in the CST.
    let mut in_src: Vec<String> = tokens
        .iter()
        .filter_map(|t| match &t.kind {
            TokenKind::Comment(c) => Some(c.text.clone()),
            _ => None,
        })
        .collect();
    let mut got = collect_comments(&cst);
    in_src.sort();
    got.sort();
    assert_eq!(got, in_src, "no comment may be dropped");

    // The lossless round-trip contract on the whole tree.
    assert_eq!(cst.clone(), cst);

    // The `?` doc payload is verbatim (the block comment inside is NOT part of
    // the doc text).
    let TopKind::Alphabet(a) = &cst
        .items
        .iter()
        .find_map(|i| match &i.kind {
            TopKind::Alphabet(_) => Some(&i.kind),
            _ => None,
        })
        .unwrap()
    else {
        unreachable!()
    };
    let DocRunKind::Doc { text, .. } = &a.doc_run[0].kind else {
        panic!("expected a doc line first in the run");
    };
    assert_eq!(text, "doc /* not this — doc payload is verbatim */");
}

#[test]
fn parse_equals_lower_cst_after_parse_cst() {
    // The seam contract, exercised on a real program.
    let tokens = lex(A5).unwrap();
    let via_seam = lower_cst(&parse_cst(&tokens).unwrap());
    let via_parse = parse(&tokens).unwrap();
    assert_eq!(via_seam, via_parse);
}

// ---------------------------------------------------------------------------
// Spans are retained on the things later phases will name.
// ---------------------------------------------------------------------------

#[test]
fn spans_are_retained_for_names_and_rules() {
    let p = parse_src("machine {\n  entry state scan {\n    ['b'] -> stop;\n  }\n}").unwrap();
    let scan = &machine(&p).states[0];
    // `scan` name token at line 2.
    assert_eq!(scan.name_span.start.line, 2);
    // The rule spans from its `[` to the `;`.
    let r = &scan.rules[0];
    assert_eq!(r.span.start.line, 3);
    assert_eq!(r.span.start.col, 5);
    // The pattern cell's span covers `'b'`.
    assert_eq!(r.pattern.cells[0].span.start.col, 6);
}

// ---------------------------------------------------------------------------
// Cross-carrier: routine and graph share a CST shape but lower distinctly.
// ---------------------------------------------------------------------------

#[test]
fn routine_and_graph_lower_to_distinct_ast_lists() {
    let src = "routine r() { } graph g() { }";
    let cst = parse_cst(&lex(src).unwrap()).unwrap();
    // In the CST both are `Reuse`, discriminated by carrier.
    let carriers: Vec<ReuseCarrier> = cst
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            TopKind::Reuse(r) => Some(r.carrier),
            _ => None,
        })
        .collect();
    assert_eq!(carriers, vec![ReuseCarrier::Routine, ReuseCarrier::Graph]);
    // In the AST they land in separate lists.
    let p = lower_cst(&cst);
    assert_eq!(p.routines.len(), 1);
    assert_eq!(p.graphs.len(), 1);
    assert_eq!(p.routines[0].name, "r");
    assert_eq!(p.graphs[0].name, "g");
}

// ---------------------------------------------------------------------------
// Rule-action ordering, the substitution `%` fold operator, the `debugger`
// flag, and the `{c+0}` corner.
// ---------------------------------------------------------------------------

const MACHINE_HEAD: &str = "alphabet b { '_', '0', '1' }\nmachine {\n  tape t: b;\n";

fn one_rule(rule: &str) -> String {
    format!("{MACHINE_HEAD}  entry state s {{ {rule} }}\n}}\n")
}

#[test]
fn write_must_precede_move_in_an_action() {
    // The action grammar is `[write] [move] transition` — `move` before
    // `write` puts the reserved `write` keyword where a transition is
    // expected, so it is rejected.
    assert_eq!(
        err_code(&one_rule("['0'] -> move [>] write ['1'] goto s;")),
        "unexpected-token"
    );
    // The canonical order parses.
    assert!(parse_src(&one_rule("['0'] -> write ['1'] move [>] goto s;")).is_ok());
}

#[test]
fn percent_is_a_fold_operator() {
    // `%` is the remainder operator in a write-cell fold expression — the
    // substitution grammar matches the assembler's (`+ - * %`, parens, i64),
    // so `{v%2}` on a numeric binding parses.
    assert!(parse_src(&one_rule("[0..9 as v] -> write [{v%2}] goto s;")).is_ok());
}

#[test]
fn debugger_flag_parses_at_the_action_head() {
    let p = parse_src(&one_rule("['0'] -> debugger write ['1'] goto s;")).unwrap();
    let rule = &machine(&p).states[0].rules[0];
    assert!(rule.debugger);
    assert!(rule.write.is_some());
    // Without it, the flag is false.
    let p = parse_src(&one_rule("['0'] -> write ['1'] goto s;")).unwrap();
    assert!(!machine(&p).states[0].rules[0].debugger);
}

#[test]
fn only_a_bare_name_is_pass_through() {
    // Passthrough is a bare name — `FoldExprKind::Var` at top level, no
    // operators. `{c}` on a glyph binding is legal.
    let p = parse_src(&one_rule("['a' as c] -> write [{c}] goto s;")).unwrap();
    let cell = &machine(&p).states[0].rules[0].write.as_ref().unwrap().cells[0];
    assert!(matches!(
        &cell.kind,
        WriteCellKind::Subst { expr }
            if matches!(&expr.kind, FoldExprKind::Var(name) if name == "c")
    ));
    // `{c+0}` is no longer a passthrough — it is a fold (an `Add`
    // application), so on a glyph binding it is char arithmetic. The old
    // delta-0-is-identity special case does not survive the expression
    // grammar.
    assert_eq!(
        err_code(&one_rule("['a' as c] -> write [{c+0}] goto s;")),
        "char-arithmetic"
    );
    // `{c+1}` on a glyph binding stays char arithmetic.
    assert_eq!(
        err_code(&one_rule("['a' as c] -> write [{c+1}] goto s;")),
        "char-arithmetic"
    );
    // Arithmetic on a numeric binding is fine (folded at expansion),
    // including the identity `{v+0}`.
    assert!(parse_src(&one_rule("[0 as v] -> write [{v+0}] goto s;")).is_ok());
    assert!(parse_src(&one_rule("[0 as v] -> write [{v+1}] goto s;")).is_ok());
}

// ---------------------------------------------------------------------------
// Fold expressions: a write-cell substitution accepts the assembler's full
// arithmetic grammar (`+ - * %`, parens, i64), not just `{name±int}`.
// ---------------------------------------------------------------------------

#[test]
fn fold_expr_modulo_parses() {
    // A numeric range binding and a `%` fold — must parse.
    let src = one_rule("[0..5 as v] -> write [{(v+1)%6}] move [>] goto s;");
    assert!(parse_src(&src).is_ok());
}

#[test]
fn fold_expr_precedence_and_multi_var() {
    // `b*2` binds tighter than `a+`, and two distinct vars in one expression
    // are legal.
    let src = one_rule("[0..2 as a, 0..2 as b] -> write [{a+b*2}, -] move [>, .] goto s;");
    assert!(parse_src(&src).is_ok());
}

#[test]
fn fold_expr_char_arithmetic_still_rejected() {
    // A glyph binding inside a non-bare expression is char arithmetic, even
    // through parens and `%`.
    let src = one_rule("['a'..'c' as c] -> write [{(c+1)%3}] move [>] goto s;");
    assert_eq!(err_code(&src), "char-arithmetic");
}

#[test]
fn fold_expr_bare_var_stays_passthrough_for_glyphs() {
    // A bare name keeps passthrough semantics — legal on a glyph binding.
    let src = one_rule("['a'..'c' as c] -> write [{c}] move [>] goto s;");
    assert!(parse_src(&src).is_ok());
}

#[test]
fn fold_expr_parenthesized_lone_glyph_binding_stays_passthrough() {
    // Parens around a lone glyph binding collapse to a top-level `Var`, so
    // `{(c)}` is passthrough — not char arithmetic — and parses on a glyph
    // binding just as bare `{c}` does.
    let src = one_rule("['a'..'c' as c] -> write [{(c)}] move [>] goto s;");
    assert!(parse_src(&src).is_ok());
    let p = parse_src(&src).unwrap();
    let cell = &machine(&p).states[0].rules[0].write.as_ref().unwrap().cells[0];
    assert!(matches!(
        &cell.kind,
        WriteCellKind::Subst { expr }
            if matches!(&expr.kind, FoldExprKind::Var(name) if name == "c")
    ));
}

// ---------------------------------------------------------------------------
// Interior list comments: a comment inside a comma-separated list is
// captured on the enclosing CST node, keyed by the index of the entry it
// precedes.
// ---------------------------------------------------------------------------

/// A comment inside a comma-separated list is captured on the enclosing
/// CST node, keyed by the index of the entry it precedes. An index equal
/// to the entry count means "after the last entry, before the closer"
/// (docs/tmt/fmt.md (interior comments)).
#[test]
fn interior_comments_are_captured_with_their_entry_index() {
    let src = "alphabet bits { '_', // the blank\n  '0', '1' }\n\n\
               machine { tape t: bits; entry state s { [*] -> stop; } }\n";
    let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
    let cst = parse_cst(&tokens).expect("parses");
    let TopKind::Alphabet(a) = &cst.items[0].kind else {
        panic!("expected an alphabet item");
    };
    assert_eq!(a.elems.len(), 3, "the three glyphs still parse");
    assert_eq!(a.interior.len(), 1, "the comment is captured");
    let (index, comment) = &a.interior[0];
    assert_eq!(*index, 1, "it precedes entry 1 (`'0'`)");
    assert_eq!(comment.text.trim_end(), "// the blank");
    assert!(!comment.own_line, "it trails `'_',` on the same line");
}

/// The tail slot: a comment after the last entry has no following entry,
/// so it is keyed by the entry count itself.
#[test]
fn a_comment_after_the_last_entry_is_keyed_by_the_entry_count() {
    let src = "alphabet bits { '_', '0', '1' // the last\n}\n\n\
               machine { tape t: bits; entry state s { [*] -> stop; } }\n";
    let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
    let cst = parse_cst(&tokens).expect("parses");
    let TopKind::Alphabet(a) = &cst.items[0].kind else {
        panic!("expected an alphabet item");
    };
    assert_eq!(a.interior.len(), 1);
    assert_eq!(a.interior[0].0, 3, "keyed by the entry count, not an index");
}

/// An own-line comment before the first entry keys to index 0, and keeps
/// `own_line` so the printer can put it back on its own line.
#[test]
fn a_comment_before_the_first_entry_keys_to_zero() {
    let src = "alphabet bits {\n  // leading note\n  '_', '0', '1' }\n\n\
               machine { tape t: bits; entry state s { [*] -> stop; } }\n";
    let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
    let cst = parse_cst(&tokens).expect("parses");
    let TopKind::Alphabet(a) = &cst.items[0].kind else {
        panic!("expected an alphabet item");
    };
    assert_eq!(a.interior.len(), 1);
    assert_eq!(a.interior[0].0, 0);
    assert!(a.interior[0].1.own_line, "it sits on its own line");
}

/// `significant_tokens` — the filter `compiler::analyze` puts between the
/// `WithComments` stream the green parse needs and the adjacency-walking lint
/// layer — returns exactly the comment-free lex of the same source: the
/// comments removed and NOTHING else, spans included.
///
/// The corpus-wide form of this law lives in
/// `tests/tmc_green_analyze.rs::corpus_significant_tokens_equal_a_comment_free_lex`,
/// which re-derives the filter inline because the function is `pub(crate)`.
/// This one calls the function itself, so a filter widened to drop a token
/// kind that is NOT trivia — a doc line, say, which both lex modes emit as a
/// semantic token — fails here rather than passing an inline re-derivation.
#[test]
fn significant_tokens_is_the_comment_free_lex() {
    let src = "// leading note\n\
               ? documented\n\
               alphabet /* inline */ bits { '_', '1' }\n\
               machine { /* body */ tape t: bits; entry state s { [*] -> stop; } }\n";
    let raw = lex_with(src, LexMode::WithComments).expect("lexes");
    assert!(
        raw.iter()
            .filter(|t| matches!(t.kind, TokenKind::Comment(_)))
            .count()
            >= 3,
        "the fixture must carry comments, or the filter is a no-op here"
    );
    assert!(
        raw.iter().any(|t| matches!(t.kind, TokenKind::DocLine(_))),
        "the fixture must carry a doc line — it is the non-trivia kind a \
         widened filter would wrongly drop"
    );
    assert_eq!(significant_tokens(&raw), lex(src).expect("lexes"));
}
