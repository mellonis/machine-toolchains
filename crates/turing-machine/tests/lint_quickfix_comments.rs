//! Pins the APPLIED TEXT of every quickfix whose edit span comes from
//! `lint/rules/spans.rs` (`docs/tmt/lint.md` (quickfix availability)) —
//! the six span queries over the green tree:
//!
//! | query | rules | what it answers |
//! |---|---|---|
//! | `decl_span::<AlphabetView>` | `unused-alphabet` | the declaration node's range |
//! | `decl_span::<ReuseView>` | `unused-routine`, `unused-graph` | the declaration node's range |
//! | `decl_span::<GraftView>` / `::<BindView>` | `unused-graft-instance`, `unused-binding` | the statement node's range |
//! | `as_clause_span` | `unused-graft-name` | from the `)` to the instance name, inside the GRAFT node |
//! | `marker_span` | `leftover-debugger` | the `debugger` token to the next element, inside the RULE node |
//! | `arrow_span` | `dead-map-pair` | the pair's arrow token |
//!
//! Every query is a RANGE query — the innermost node of a kind containing
//! an anchor the resolved module keeps, then that node's range or a token
//! pair inside it — so a comment written anywhere in a declaration cannot
//! void a span or cut it short: the node is lossless and holds the comment
//! too. The only thing a comment changes is the guard's verdict —
//! `run_rules` withholds a fix whose span holds one, finding kept
//! (docs/tmt/lint.md (quickfix availability)) — and the `*_withheld_*`
//! fixtures here pin exactly that `None`, at the placements that once
//! defeated the token-adjacency helpers these queries replaced: between a
//! keyword and its name (the fix used to vanish), and between a bound
//! doc/attention run and the keyword (the fix used to ship with the run
//! orphaned — a parse error). The `*_not_truncated` fixtures are the pins
//! against that second shape ever returning.
//!
//! The comment-free fixtures' expected texts were verified green against
//! the pre-green front end first, so a failure there means a REGRESSION,
//! never a new expectation that needs deriving. The comment-bearing
//! fixtures' authority is the guard's own posture: finding reported, fix
//! withheld, never a silent comment deletion.

use mtc_core::diagnostics::{Diagnostic, Edit, Pos};
use mtc_turing_machine::lint::{LintOptions, lint};

/// Apply a fix's edits to `src` (char-position spans to byte offsets,
/// applied descending so earlier offsets stay valid). Mirrors
/// `apply_fix` in `tests/lint_programs.rs` — this crate keeps no shared
/// test-support module, so each integration-test file defines its own copy.
fn apply_fix(src: &str, edits: &[Edit]) -> String {
    fn byte_offset(src: &str, pos: Pos) -> usize {
        let (mut line, mut col) = (1u32, 1u32);
        for (i, c) in src.char_indices() {
            if line == pos.line && col == pos.col {
                return i;
            }
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        src.len()
    }
    let mut ranges: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|e| {
            (
                byte_offset(src, e.span.start),
                byte_offset(src, e.span.end),
                e.replacement.clone(),
            )
        })
        .collect();
    ranges.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut out = src.to_string();
    for (s, e, rep) in ranges {
        out.replace_range(s..e, &rep);
    }
    out
}

/// The findings for one rule `code`, source-ordered.
fn findings_for(src: &str, code: &str) -> Vec<Diagnostic> {
    lint(src, LintOptions::default())
        .unwrap()
        .diagnostics
        .into_iter()
        .filter(|d| d.code == code)
        .collect()
}

// -- unused-alphabet / decl_span -------------------------------------------
//
// A `machine` needs an `entry state` or `entry-count` fails before any lint
// runs; the entry state must MOVE its tape or `unused-tape` also fires,
// making "the fix" ambiguous; and `cd` must stay drawn on by `main` or it
// becomes a second `unused-alphabet`. All three are load-bearing — each was
// wrong in an earlier draft of this fixture.

const ALPHABET_CLEAN: &str = "\
alphabet ab { '_' }

alphabet cd { '_' }

machine {
  tape main: cd;
  entry state s { [*] -> move [>] stop; }
}
";

/// Identical to `ALPHABET_CLEAN` but for a comment sitting between the
/// `alphabet` keyword and the `ab` name it declares.
const ALPHABET_COMMENT: &str = "\
alphabet /* c */ ab { '_' }

alphabet cd { '_' }

machine {
  tape main: cd;
  entry state s { [*] -> move [>] stop; }
}
";

/// Identical to `ALPHABET_CLEAN` but for a bound `?` doc line with a comment
/// sitting BETWEEN the doc line and the `alphabet` keyword — the orphaning
/// shape. Adjacency holds here (`alphabet` is still the token right before
/// `ab`), so the helper computes a span either way; what the comment
/// threatens is the doc-run walk-back, and a span that stopped at the
/// comment would leave the `?` line behind as an orphaned run — a parse
/// error — while holding no comment, slipping past the guard as `Some`.
const ALPHABET_DOC_RUN_COMMENT: &str = "\
? an unused alphabet
/* c */
alphabet ab { '_' }

alphabet cd { '_' }

machine {
  tape main: cd;
  entry state s { [*] -> move [>] stop; }
}
";

/// The `alphabet ab { '_' }` line is gone entirely (its own newline stays,
/// producing the leading blank line); `cd`, the `machine`, and the blank
/// line between the two `alphabet`s are byte-identical to the input.
const ALPHABET_FIXED: &str = "\n\nalphabet cd { '_' }\n\nmachine {\n  tape main: cd;\n  entry state s { [*] -> move [>] stop; }\n}\n";

#[test]
fn unused_alphabet_fix_deletes_the_declaration() {
    let ds = findings_for(ALPHABET_CLEAN, "unused-alphabet");
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].message, "alphabet `ab` is never used by any tape");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("decl_span found the declaration");
    assert_eq!(apply_fix(ALPHABET_CLEAN, &fix.edits), ALPHABET_FIXED);
}

#[test]
fn unused_alphabet_fix_is_withheld_when_a_comment_rides_the_span() {
    let ds = findings_for(ALPHABET_COMMENT, "unused-alphabet");
    assert_eq!(ds.len(), 1, "{ds:?}");
    // Comment-free tokens make `alphabet` still the token immediately
    // before `ab`, so `decl_span` computes the whole first line — with the
    // `/* c */` interior to it. `run_rules`' comment guard withholds a fix
    // whose span holds a comment (deleting it silently is data loss), so
    // the finding ships without a remedy.
    assert!(
        ds[0].fix.is_none(),
        "a fix spanning a comment must be withheld"
    );
}

#[test]
fn unused_alphabet_doc_run_fix_is_withheld_not_truncated() {
    let ds = findings_for(ALPHABET_DOC_RUN_COMMENT, "unused-alphabet");
    assert_eq!(ds.len(), 1, "{ds:?}");
    // The span walks back to the `?` line ACROSS the `/* c */` (the doc run
    // belongs to `ab`, and an orphaned run is a parse error), which puts the
    // comment interior to the span, and the guard withholds the fix. The
    // `None` here still discriminates the significant-token filter: without
    // it the walk-back stops AT the comment, the truncated span holds no
    // comment, and a fix ships that orphans the `?` line — as `Some`, which
    // this assertion rejects.
    assert!(
        ds[0].fix.is_none(),
        "a fix spanning a comment must be withheld"
    );
}

// -- unused-routine / decl_span::<ReuseView> -------------------------------
//
// `helper` must go unused while `api` (exported) stays, so exactly one
// `unused-routine` finding fires; the machine's tape is moved so no
// `unused-tape` finding joins it.

const ROUTINE_CLEAN: &str = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) {
  entry state s { [*] -> return; }
}
export routine api(tape t: ab) {
  entry state s { [*] -> return; }
}
machine {
  tape t: ab;
  entry state go { [*] -> move [>] stop; }
}
";

/// Identical to `ROUTINE_CLEAN` but for a comment between the `routine`
/// keyword and the `helper` name it declares.
const ROUTINE_COMMENT: &str = "\
alphabet ab { '_', 'a' }
routine /* c */ helper(tape t: ab) {
  entry state s { [*] -> return; }
}
export routine api(tape t: ab) {
  entry state s { [*] -> return; }
}
machine {
  tape t: ab;
  entry state go { [*] -> move [>] stop; }
}
";

/// Identical to `ROUTINE_CLEAN` but for a leading `?` doc line bound to the
/// `helper` declaration.
const ROUTINE_DOC_RUN: &str = "\
alphabet ab { '_', 'a' }
? an unused helper routine
routine helper(tape t: ab) {
  entry state s { [*] -> return; }
}
export routine api(tape t: ab) {
  entry state s { [*] -> return; }
}
machine {
  tape t: ab;
  entry state go { [*] -> move [>] stop; }
}
";

/// Identical to `ROUTINE_DOC_RUN` but for a comment sitting BETWEEN the doc
/// line and the `routine` keyword — the orphaning shape (see
/// `ALPHABET_DOC_RUN_COMMENT`), here against the SHARED `back_over_doc_run`
/// rather than `unused_alphabet.rs`'s own inlined walk.
const ROUTINE_DOC_RUN_COMMENT: &str = "\
alphabet ab { '_', 'a' }
? an unused helper routine
/* c */
routine helper(tape t: ab) {
  entry state s { [*] -> return; }
}
export routine api(tape t: ab) {
  entry state s { [*] -> return; }
}
machine {
  tape t: ab;
  entry state go { [*] -> move [>] stop; }
}
";

/// The whole `helper` declaration — brace-matched body included, plus a
/// leading doc run when the input has one — is gone; a single blank line
/// remains where it sat (its own opening line's newline meets the closing
/// brace's trailing newline). `api`, `machine`, and everything else are
/// byte-identical to the input.
const ROUTINE_FIXED: &str = "\
alphabet ab { '_', 'a' }

export routine api(tape t: ab) {
  entry state s { [*] -> return; }
}
machine {
  tape t: ab;
  entry state go { [*] -> move [>] stop; }
}
";

#[test]
fn unused_routine_fix_deletes_the_declaration() {
    let ds = findings_for(ROUTINE_CLEAN, "unused-routine");
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].message, "routine `helper` is never called");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("decl_span found the declaration node");
    assert_eq!(apply_fix(ROUTINE_CLEAN, &fix.edits), ROUTINE_FIXED);
}

#[test]
fn unused_routine_fix_is_withheld_when_a_comment_rides_the_span() {
    let ds = findings_for(ROUTINE_COMMENT, "unused-routine");
    assert_eq!(ds.len(), 1, "{ds:?}");
    // `decl_span::<ReuseView>` answers the whole declaration node with the
    // `/* c */` interior to it; the comment guard withholds the fix (see
    // the alphabet sibling above).
    assert!(
        ds[0].fix.is_none(),
        "a fix spanning a comment must be withheld"
    );
}

#[test]
fn unused_routine_fix_deletes_the_declaration_with_its_doc_run() {
    let ds = findings_for(ROUTINE_DOC_RUN, "unused-routine");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("decl_span found the declaration node");
    // The doc line is bound to `helper` and must go with it — `back_over_doc_run`
    // is what walks the span start back over it.
    assert_eq!(apply_fix(ROUTINE_DOC_RUN, &fix.edits), ROUTINE_FIXED);
}

#[test]
fn unused_routine_doc_run_fix_is_withheld_not_truncated() {
    let ds = findings_for(ROUTINE_DOC_RUN_COMMENT, "unused-routine");
    assert_eq!(ds.len(), 1, "{ds:?}");
    // `back_over_doc_run` reaches the `?` line across the `/* c */` (a span
    // starting at `routine` would orphan the doc line — a parse error), so
    // the comment is interior and the guard withholds the fix. Without the
    // significant-token filter the walk-back stops at the comment and a
    // truncated, run-orphaning fix ships as `Some` — which this rejects
    // (see the alphabet sibling above).
    assert!(
        ds[0].fix.is_none(),
        "a fix spanning a comment must be withheld"
    );
}

// -- unused-binding / decl_span::<BindView> --------------------------------
//
// `h` binds `helper` but nothing calls `h`, so exactly one `unused-binding`
// finding fires (not `unused-routine`: a bind target counts as a use, by
// design); the machine's tape is moved so no `unused-tape` finding joins it.

const BIND_CLEAN: &str = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) {
  entry state s { [*] -> return; }
}
machine {
  tape t: ab;
  bind helper(t = t) as h;
  entry state go { [*] -> move [>] stop; }
}
";

/// Identical to `BIND_CLEAN` but for a comment between the `bind` keyword
/// and the `helper` target name.
const BIND_COMMENT: &str = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) {
  entry state s { [*] -> return; }
}
machine {
  tape t: ab;
  bind /* c */ helper(t = t) as h;
  entry state go { [*] -> move [>] stop; }
}
";

/// Identical to `BIND_CLEAN` but for a leading `?` doc line bound to the
/// `bind` statement.
const BIND_DOC_RUN: &str = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) {
  entry state s { [*] -> return; }
}
machine {
  tape t: ab;
  ? an unused binding
  bind helper(t = t) as h;
  entry state go { [*] -> move [>] stop; }
}
";

/// Identical to `BIND_DOC_RUN` but for a comment sitting BETWEEN the doc
/// line and the `bind` keyword — the orphaning shape (see
/// `ALPHABET_DOC_RUN_COMMENT`), inside a world body rather than at file
/// level.
const BIND_DOC_RUN_COMMENT: &str = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) {
  entry state s { [*] -> return; }
}
machine {
  tape t: ab;
  ? an unused binding
  /* c */
  bind helper(t = t) as h;
  entry state go { [*] -> move [>] stop; }
}
";

/// The `bind … as h;` statement — plus its leading doc run when the input
/// has one — is gone. The span is the BIND node's own range, which starts
/// at the `bind` keyword or the doc run bound to it, not at the start of
/// the (indented) line: the two leading spaces that indented the deleted
/// statement are untouched text and stay behind, leaving a line of bare
/// indentation where the statement sat. That is not a typo — deleting it
/// would be a different, wider fix the rule does not make.
const BIND_FIXED: &str = "alphabet ab { '_', 'a' }\nroutine helper(tape t: ab) {\n  entry state s { [*] -> return; }\n}\nmachine {\n  tape t: ab;\n  \n  entry state go { [*] -> move [>] stop; }\n}\n";

#[test]
fn unused_binding_fix_deletes_the_statement() {
    let ds = findings_for(BIND_CLEAN, "unused-binding");
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].message, "bind `h` is never called");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("decl_span found the statement node");
    assert_eq!(apply_fix(BIND_CLEAN, &fix.edits), BIND_FIXED);
}

#[test]
fn unused_binding_fix_is_withheld_when_a_comment_rides_the_span() {
    let ds = findings_for(BIND_COMMENT, "unused-binding");
    assert_eq!(ds.len(), 1, "{ds:?}");
    // `decl_span::<BindView>` answers the whole statement with the `/* c */`
    // interior to it; the comment guard withholds the fix (see the alphabet
    // sibling above).
    assert!(
        ds[0].fix.is_none(),
        "a fix spanning a comment must be withheld"
    );
}

#[test]
fn unused_binding_fix_deletes_the_statement_with_its_doc_run() {
    let ds = findings_for(BIND_DOC_RUN, "unused-binding");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("decl_span found the statement node");
    // The doc line is bound to the `bind` statement and must go with it —
    // `back_over_doc_run` is what walks the span start back over it.
    assert_eq!(apply_fix(BIND_DOC_RUN, &fix.edits), BIND_FIXED);
}

#[test]
fn unused_binding_doc_run_fix_is_withheld_not_truncated() {
    let ds = findings_for(BIND_DOC_RUN_COMMENT, "unused-binding");
    assert_eq!(ds.len(), 1, "{ds:?}");
    // Same orphaning risk as the routine shape: the span reaches the `?`
    // line across the `/* c */`, so the comment is interior and the guard
    // withholds the fix; a truncated, run-orphaning fix would ship as
    // `Some` — which this rejects (see the alphabet sibling above).
    assert!(
        ds[0].fix.is_none(),
        "a fix spanning a comment must be withheld"
    );
}

// -- unused-graft-name / as_clause_span ------------------------------------
//
// `as_clause_span` finds the GRAFT node, its instance-name token, and the
// binding list's `)` behind the `as` keyword, trivia skipped; the span runs
// from that `)` through the name, so a comment on either side of `as` lands
// inside it and the guard withholds the fix.

const GRAFT_NAME_CLEAN: &str = "\
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
";

/// Identical to `GRAFT_NAME_CLEAN` but for a comment between the signature's
/// closing `)` and the `as` keyword.
const GRAFT_NAME_COMMENT: &str = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  entry graft findX(t = work, found = win, missing = lose) /* c */ as seek;
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";

/// Only ` as seek` goes — the span runs from the `)`'s own end to the name's
/// end, so the graft survives as a valid unnamed entry graft. In the comment
/// variant the `/* c */` is interior to that span, so the guard withholds
/// the fix instead.
const GRAFT_NAME_FIXED: &str = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  entry graft findX(t = work, found = win, missing = lose);
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";

#[test]
fn unused_graft_name_fix_removes_the_as_clause() {
    let ds = findings_for(GRAFT_NAME_CLEAN, "unused-graft-name");
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(
        ds[0].message,
        "entry graft instance name `seek` is never used"
    );
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("as_clause_span found the clause");
    assert_eq!(apply_fix(GRAFT_NAME_CLEAN, &fix.edits), GRAFT_NAME_FIXED);
}

#[test]
fn unused_graft_name_fix_is_withheld_when_a_comment_rides_the_span() {
    let ds = findings_for(GRAFT_NAME_COMMENT, "unused-graft-name");
    assert_eq!(ds.len(), 1, "{ds:?}");
    // `as_clause_span` runs from the `)`'s end through the name's end, with
    // the `/* c */` interior to it; the comment guard withholds the fix
    // (see the alphabet sibling above).
    assert!(
        ds[0].fix.is_none(),
        "a fix spanning a comment must be withheld"
    );
}

// -- leftover-debugger / marker_span ---------------------------------------
//
// `marker_span` anchors on the rule's `->` and requires the token right after
// it to be the `debugger` marker; the token after THAT is the span's end. The
// fixture puts the comment BEFORE the marker, which exercises the void
// cleanly: the comment then sits outside the deleted span and must survive
// the fix, so the assertion sees the marker go and the comment stay.

const DEBUGGER_CLEAN: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> debugger goto s;
    ['_'] -> stop;
  }
}
";

/// Identical to `DEBUGGER_CLEAN` but for a comment between the rule's `->`
/// and the `debugger` marker it precedes.
const DEBUGGER_COMMENT: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> /* c */ debugger goto s;
    ['_'] -> stop;
  }
}
";

/// The marker and the single space after it are gone; the rule keeps its
/// explicit `goto s`, which is why the fix was offered at all.
const DEBUGGER_FIXED: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> goto s;
    ['_'] -> stop;
  }
}
";

/// The comment variant's own expected text: the span runs from the marker's
/// start to the following token's start, and the comment sits BEFORE the
/// marker — outside that span — so it survives untouched.
const DEBUGGER_COMMENT_FIXED: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> /* c */ goto s;
    ['_'] -> stop;
  }
}
";

#[test]
fn leftover_debugger_fix_removes_the_marker() {
    let ds = findings_for(DEBUGGER_CLEAN, "leftover-debugger");
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].message, "leftover 'debugger' marker");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("marker_span found the marker");
    assert_eq!(apply_fix(DEBUGGER_CLEAN, &fix.edits), DEBUGGER_FIXED);
}

#[test]
fn leftover_debugger_fix_is_unchanged_by_a_comment_before_the_marker() {
    let ds = findings_for(DEBUGGER_COMMENT, "leftover-debugger");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("marker_span found the marker");
    assert_eq!(
        apply_fix(DEBUGGER_COMMENT, &fix.edits),
        DEBUGGER_COMMENT_FIXED
    );
}

// -- dead-map-pair / arrow_span --------------------------------------------
//
// `arrow_span` locates the `=>`/`->` of a map pair by RANGE CONTAINMENT —
// the ARROW token whose range lies between the pair's source and
// destination glyphs — and a comment is a trivia token that can never be
// that arrow. A single-token replacement never spans a comment, so the
// guard has nothing to withhold either: the fix ships unchanged next to a
// comment, which this fixture pins.

/// A comment sitting immediately before the arrow of the very pair that
/// fires. `dead-map-pair` is opt-in, so it is requested explicitly below.
const DEAD_MAP_PAIR_COMMENT: &str = "\
alphabet host5 { '_', '^', '$', '0', '1' }
alphabet bare3 { '_', '0', '1' }

graph zeroing(tape v: bare3, state done) {
  entry state s {
    ['1'] -> write ['0'] move [>] goto s;
    [*] -> done;
  }
}

machine {
  tape t: host5;
  entry graft zeroing(v = t with map { '^' => '_', '$' => '_', '0' -> '0', '1' /* c */ -> '1' }, done = fin) as z;
  state fin { [*] -> stop; }
}
";

/// The fix demotes the dead pair by rewriting its `->` to `=>`; the comment
/// is untouched, since the edit replaces the arrow token's own span only.
const DEAD_MAP_PAIR_COMMENT_FIXED: &str = "\
alphabet host5 { '_', '^', '$', '0', '1' }
alphabet bare3 { '_', '0', '1' }

graph zeroing(tape v: bare3, state done) {
  entry state s {
    ['1'] -> write ['0'] move [>] goto s;
    [*] -> done;
  }
}

machine {
  tape t: host5;
  entry graft zeroing(v = t with map { '^' => '_', '$' => '_', '0' -> '0', '1' /* c */ => '1' }, done = fin) as z;
  state fin { [*] -> stop; }
}
";

#[test]
fn dead_map_pair_fix_is_unaffected_by_an_adjacent_comment() {
    let ds: Vec<Diagnostic> = lint(
        DEAD_MAP_PAIR_COMMENT,
        LintOptions {
            warn: vec!["dead-map-pair".to_string()],
            ..Default::default()
        },
    )
    .unwrap()
    .diagnostics
    .into_iter()
    .filter(|d| d.code == "dead-map-pair")
    .collect();
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("arrow_span found the arrow");
    assert_eq!(
        apply_fix(DEAD_MAP_PAIR_COMMENT, &fix.edits),
        DEAD_MAP_PAIR_COMMENT_FIXED
    );
}
