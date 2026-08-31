//! The fix-withhold comment guard, exercised per fix-emitting rule: a fix
//! whose edit span contains a comment token is WITHHELD — the finding is
//! still reported, the remedy is not offered — because applying it would
//! silently delete the comment (docs/pmt/lint.md (quickfix availability)).
//! `pmt lint --fix` writes the fixed source back to disk, so a deleting fix
//! is not an editor nicety going wrong but data loss on the user's file.
//!
//! The guard is one central chokepoint over every rule's output, not a
//! per-rule check, so a future fix-emitting rule is covered by construction
//! — the same mechanism the sibling `.tmc` crate carries, pinned by the
//! same-named test file there. These tests pin the posture per CURRENT
//! rule: each fixture places a comment inside the rule's fix span and
//! asserts the pair (finding present, fix withheld), next to a comment-free
//! sibling proving the fixture does trigger a fix at all.
//!
//! Roster: five rules emit a `Fix`. Four edit a span a comment can sit in
//! and are pinned here — `leftover-debugger` and the `goto` shape of
//! `redundant-jump-to-next` delete a whole statement, its successor shape
//! deletes the parenthesized successor, `identical-check-arms` replaces the
//! whole check item, and `unused-label` deletes the label prefix, whose
//! number-to-`:` span can carry a wedged comment. The fifth,
//! `leading-zeros`, is exempt BY MECHANISM: its one edit replaces a single
//! number token, and a span covering one token can never contain a comment.

use mtc_post_machine::lint::{LintOptions, lint};

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

/// Asserts the guard pair for one rule: `clean` offers a fix, and the same
/// source with `/* keep me */ ` spliced in immediately before `anchor`
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
fn leftover_debugger_withholds_the_fix_when_the_span_holds_a_comment() {
    // The fix deletes the whole `debugger;` statement; the comment sits
    // between the keyword and the `;`, inside that span.
    assert_guard_pair(
        "main() {\n    debugger;\n    right;\n}\n",
        ";\n    right",
        "leftover-debugger",
    );
}

#[test]
fn redundant_goto_withholds_the_fix_when_the_span_holds_a_comment() {
    // The fix deletes the whole `goto 5;` statement; the comment sits
    // between `goto` and its target, inside that span.
    assert_guard_pair(
        "main() {\n    goto 5;\n 5: right;\n}\n",
        "5;",
        "redundant-jump-to-next",
    );
}

#[test]
fn redundant_successor_withholds_the_fix_when_the_span_holds_a_comment() {
    // The fix deletes the `(5)` successor; the comment sits inside the
    // parens, inside that span.
    assert_guard_pair(
        "main() {\n    right(5);\n 5: left;\n}\n",
        "5);",
        "redundant-jump-to-next",
    );
}

#[test]
fn unused_label_withholds_the_fix_when_the_span_holds_a_comment() {
    // The fix deletes the label prefix `5:`; a comment wedged between the
    // number and the `:` parses and sits inside that span.
    assert_guard_pair("main() {\n 5: right;\n}\n", ": right", "unused-label");
}

#[test]
fn identical_check_arms_withholds_the_fix_when_the_span_holds_a_comment() {
    // The fix REPLACES the whole `check(5, 5)` item with `goto 5`; a
    // comment inside the arms is inside the replaced span and would go
    // with it — replacement is no safer than deletion here.
    assert_guard_pair(
        "main() {\n    check(5, 5);\n 5: right;\n}\n",
        "5);",
        "identical-check-arms",
    );
}

#[test]
fn the_pma_asm_surface_carries_the_same_guard() {
    // `.pma` lint is core's arch-agnostic layer under the PM-1 dialect;
    // its whole-line "delete this instruction" edit on an unlabeled line
    // swallows a trailing comment, so the same withhold posture applies
    // there (docs/pmt/lint.md (quickfix availability)). The comment-free
    // sibling proves the fixture triggers a fix at all.
    use mtc_core::asm::lint::lint as asm_lint;
    use mtc_post_machine::asm::pm1_syntax;

    let clean = ".func f\n        brk\n        stp\n";
    let report = asm_lint(&pm1_syntax(), clean, &[]).unwrap();
    let d = report
        .iter()
        .find(|d| d.code == "leftover-debugger")
        .unwrap();
    assert!(d.fix.is_some(), "the comment-free sibling must offer a fix");

    let commented = ".func f\n        brk ; breadcrumb\n        stp\n";
    let report = asm_lint(&pm1_syntax(), commented, &[]).unwrap();
    let d = report
        .iter()
        .find(|d| d.code == "leftover-debugger")
        .expect("the finding itself must survive the guard");
    assert!(d.fix.is_none(), "fix over a comment must be withheld");
}
