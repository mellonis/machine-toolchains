//! The fix-withhold comment guard, exercised per fix-emitting rule: a fix
//! whose edit span contains a comment token is WITHHELD — the finding is
//! still reported, the remedy is not offered — because applying it would
//! silently delete the comment (`docs/tmt/lint.md` (quickfix availability)).
//!
//! The guard is one central chokepoint over every rule's output, not a
//! per-rule check, so a future fix-emitting rule is covered by construction.
//! These tests pin the posture per CURRENT rule anyway: each fixture places a
//! comment inside the rule's fix span and asserts the pair (finding present,
//! fix withheld), next to a comment-free sibling proving the fixture does
//! trigger a fix at all — a fixture that never produced a fix would pass the
//! withhold assertion vacuously.
//!
//! Roster: nine rules emit a `Fix`. Seven are pinned here; the other two are
//! accounted for rather than skipped. `contract-clause-overlap` — the rule
//! the guard was hoisted out of — keeps its withhold test in its own unit
//! tests. `dead-map-pair` is exempt BY MECHANISM: its one edit replaces the
//! pair's `->` arrow token with `=>`, and a span covering a single token can
//! never contain a comment, so the guard has nothing to withhold there.
//!
//! Comment placement matters: each comment sits where the rule's span helper
//! still produces a fix (inside a body, a binding list, or the `as` clause),
//! not between a keyword and its name — THAT position voids the fix through
//! the adjacency helpers instead, and is pinned by
//! `tests/lint_quickfix_comments.rs`, a different mechanism from this guard.

use mtc_turing_machine::lint::{LintOptions, lint};

/// The one finding with `code` in `src`'s report; panics if absent or
/// ambiguous, so a fixture that stops triggering its rule fails loudly.
fn the_finding(src: &str, code: &str) -> mtc_core::diagnostics::Diagnostic {
    let report = lint(src, LintOptions::default()).unwrap();
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == code)
        .cloned()
        .collect();
    assert_eq!(hits.len(), 1, "expected exactly one `{code}` finding");
    hits.into_iter().next().unwrap()
}

// --- leftover-debugger ------------------------------------------------------
// The marker span runs from the `debugger` keyword through the start of the
// next token, so a comment written right after the marker sits inside it.

const DEBUGGER_CLEAN: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> debugger move [>] goto s;
    ['_'] -> stop;
  }
}
";

const DEBUGGER_COMMENTED: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> debugger /* keep me */ move [>] goto s;
    ['_'] -> stop;
  }
}
";

#[test]
fn leftover_debugger_offers_a_fix_on_a_comment_free_marker() {
    let d = the_finding(DEBUGGER_CLEAN, "leftover-debugger");
    assert!(d.fix.is_some(), "the comment-free sibling must offer a fix");
}

#[test]
fn leftover_debugger_withholds_the_fix_when_the_span_holds_a_comment() {
    let d = the_finding(DEBUGGER_COMMENTED, "leftover-debugger");
    assert!(
        d.fix.is_none(),
        "the fix span contains `/* keep me */`; applying it would delete the comment"
    );
}

// --- the unused-* declaration removals ---------------------------------------
// Each fix deletes a whole declaration or reuse statement; a comment written
// inside it (a body, a binding list) sits inside the deletion span. The
// comment-free source triggers the fix; the commented sibling differs by the
// `/* keep me */` insertion alone.

/// Asserts the guard pair for one rule: `clean` offers a fix, and the same
/// source with `/* keep me */` spliced in at `anchor` (immediately before it)
/// withholds it while still reporting the finding.
fn assert_guard_pair(clean: &str, anchor: &str, code: &str) {
    let d = the_finding(clean, code);
    assert!(
        d.fix.is_some(),
        "`{code}`: the comment-free sibling must offer a fix"
    );
    let commented = {
        let at = clean.find(anchor).expect("anchor present in the fixture");
        let mut s = clean.to_string();
        s.insert_str(at, "/* keep me */ ");
        s
    };
    let d = the_finding(&commented, code);
    assert!(
        d.fix.is_none(),
        "`{code}`: the fix span contains `/* keep me */`; applying it would delete the comment"
    );
}

#[test]
fn unused_alphabet_withholds_the_fix_when_the_body_holds_a_comment() {
    assert_guard_pair(
        "\
alphabet bit { '_', '1' }
alphabet marks { '_', 'x' }
machine {
  tape t: bit;
  entry state s { [*] -> stop; }
}
",
        "'x' }",
        "unused-alphabet",
    );
}

#[test]
fn unused_routine_withholds_the_fix_when_the_body_holds_a_comment() {
    assert_guard_pair(
        "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}
",
        "return;",
        "unused-routine",
    );
}

#[test]
fn unused_graph_withholds_the_fix_when_the_body_holds_a_comment() {
    assert_guard_pair(
        "\
alphabet marks { '_', 'x' }
graph dead(tape t: marks, state hit, state miss) {
  entry state w { ['x'] -> hit; [*] -> miss; }
}
machine {
  tape work: marks;
  entry state go { [*] -> stop; }
}
",
        "[*] -> miss;",
        "unused-graph",
    );
}

#[test]
fn unused_binding_withholds_the_fix_when_the_binding_list_holds_a_comment() {
    assert_guard_pair(
        "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  bind helper(t = t) as h;
  entry state go { [*] -> stop; }
}
",
        ") as h;",
        "unused-binding",
    );
}

#[test]
fn unused_graft_instance_withholds_the_fix_when_the_binding_list_holds_a_comment() {
    assert_guard_pair(
        "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  graft findX(t = work, found = win, missing = lose) as seek;
  entry state go { [*] -> stop; }
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
",
        ") as seek;",
        "unused-graft-instance",
    );
}

// --- unused-graft-name -------------------------------------------------------
// The fix deletes the `as` clause alone — from the binding's closing `)`
// through the instance name — so a comment between the `)` and the `as` sits
// inside the span (the very position the issue tracker measured).

#[test]
fn unused_graft_name_withholds_the_fix_when_the_as_clause_holds_a_comment() {
    assert_guard_pair(
        "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  entry graft findX(t = work, found = win, missing = lose) as seek;
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
",
        "as seek;",
        "unused-graft-name",
    );
}

#[test]
fn the_tma_asm_surface_carries_the_same_guard() {
    // `.tma` lint is core's arch-agnostic layer under the TM-1 dialect
    // (caps all on); its whole-line "delete this instruction" edit on an
    // unlabeled line swallows a trailing comment, so the same withhold
    // posture applies there (docs/tmt/lint.md (quickfix availability)).
    // The comment-free sibling proves the fixture triggers a fix at all.
    use mtc_core::asm::lint::lint as asm_lint;
    use mtc_turing_machine::asm::tm1_syntax;

    let clean = ".func f\n        brk\n        stp\n";
    let report = asm_lint(&tm1_syntax(), clean, &[]).unwrap();
    let d = report
        .iter()
        .find(|d| d.code == "leftover-debugger")
        .unwrap();
    assert!(d.fix.is_some(), "the comment-free sibling must offer a fix");

    let commented = ".func f\n        brk ; breadcrumb\n        stp\n";
    let report = asm_lint(&tm1_syntax(), commented, &[]).unwrap();
    let d = report
        .iter()
        .find(|d| d.code == "leftover-debugger")
        .expect("the finding itself must survive the guard");
    assert!(d.fix.is_none(), "fix over a comment must be withheld");
}
