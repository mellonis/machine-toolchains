//! Pins the APPLIED TEXT of every quickfix helper that locates its edit by
//! ADJACENCY — by indexing off a neighbouring token rather than searching
//! (`docs/tmt/lint.md` (the `.tmc` rules)). There are five:
//!
//! | helper | rules | the indexed neighbour |
//! |---|---|---|
//! | `decl_span` | `unused-alphabet` | the keyword before the name |
//! | `braced_world_decl_span` | `unused-routine`, `unused-graph` | the keyword before the name |
//! | `reuse_statement_span` | `unused-binding`, `unused-graft-instance` | the keyword before the target |
//! | `as_clause_span` | `unused-graft-name` | the `)` before `as`, and the name after it |
//! | `marker_span` | `leftover-debugger` | the token after `->`, and the one after that |
//!
//! **That is the complete set, established by enumeration rather than by
//! counting.** Every one of them reads `LintContext.tokens`, and `tokens` has
//! exactly six readers in the whole lint layer — these five plus `arrow_span`
//! (`dead-map-pair`). `arrow_span` is excluded on a mechanism, not on a
//! guess: it finds its arrow by RANGE CONTAINMENT, so extra tokens in the
//! stream cannot move its answer. The last test in this file is the witness
//! for that exclusion, and it was measured — it passes with the filter
//! removed, while the other eight comment-bearing tests fail.
//!
//! The compiler front end lexes `LexMode::WithComments` — the green tree it
//! parses through is built from the source text and its trivia together — so
//! comments DO reach `crate::compiler::Analysis`. What keeps them away from
//! these helpers is one filter: `lint()` passes the stream through
//! `parser::significant_tokens` before filling `LintContext.tokens`. This
//! file is what proves that filter is load-bearing rather than decorative.
//!
//! Two distinct things break if the raw stream reaches them, and a fixture
//! for one is blind to the other:
//!
//! 1. **The fix disappears.** A comment between a declaration's keyword and
//!    its name becomes the token immediately before the name, the adjacency
//!    check fails, the helper returns `None`, and no fix ships at all. The
//!    `*_is_unchanged_by_a_comment_before_the_name` fixtures cover this; they
//!    panic at their `.expect(...)` when it happens.
//! 2. **The fix ships BROKEN source.** A comment between a leading
//!    doc/attention run and the keyword leaves adjacency intact, so a fix is
//!    still produced — but the doc-run walk-back stops at the comment, and
//!    the deleted declaration leaves its `?`/`!` run behind. An orphaned run
//!    is a parse error, so this outcome is strictly worse than shipping
//!    nothing. The `*_takes_a_doc_run_a_comment_interrupts` fixtures cover
//!    it, and only an applied-text assertion can see it — the fix exists and
//!    the diagnostic count is right in both the correct and the broken case.
//!
//! Each fixture's expected text is the same one its comment-free sibling
//! asserts: a comment interior to the deleted span rides along with it (the
//! span is a pair of source offsets taken from token boundaries, so raw
//! untokenized text between them goes too), exactly as it did before the
//! front end moved. These are preservation claims — every one of them was
//! verified green against the pre-green front end first, so a failure here
//! means a REGRESSION, never a new expectation that needs deriving. That
//! provenance stays checkable rather than becoming folklore: the pre-green
//! front end (`lex` + `parse`) is still callable and is held to the current
//! one, source for source, by `tests/tmc_green_analyze.rs`.

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
/// `ab`), so the fix is produced either way; what the comment threatens is
/// the doc-run walk-back, and a span that stopped at the comment would leave
/// the `?` line behind as an orphaned run — a parse error.
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
fn unused_alphabet_fix_is_unchanged_by_a_comment_before_the_name() {
    let ds = findings_for(ALPHABET_COMMENT, "unused-alphabet");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("decl_span found the declaration");
    // Comment-free tokens make `alphabet` still the token immediately
    // before `ab`, so the deleted span is the whole first line either way
    // — the comment rides inside it and this must equal ALPHABET_FIXED.
    assert_eq!(apply_fix(ALPHABET_COMMENT, &fix.edits), ALPHABET_FIXED);
}

#[test]
fn unused_alphabet_fix_takes_a_doc_run_a_comment_interrupts() {
    let ds = findings_for(ALPHABET_DOC_RUN_COMMENT, "unused-alphabet");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("decl_span found the declaration");
    // The span must start at the `?` line, not at `alphabet`: the doc run
    // belongs to `ab` and an orphaned run is a parse error. The `/* c */`
    // between them is interior to that span and goes with it — the same
    // way a comment inside the deleted body already does.
    assert_eq!(
        apply_fix(ALPHABET_DOC_RUN_COMMENT, &fix.edits),
        ALPHABET_FIXED
    );
}

// -- unused-routine / braced_world_decl_span -------------------------------
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
        .expect("braced_world_decl_span found the declaration");
    assert_eq!(apply_fix(ROUTINE_CLEAN, &fix.edits), ROUTINE_FIXED);
}

#[test]
fn unused_routine_fix_is_unchanged_by_a_comment_before_the_name() {
    let ds = findings_for(ROUTINE_COMMENT, "unused-routine");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("braced_world_decl_span found the declaration");
    assert_eq!(apply_fix(ROUTINE_COMMENT, &fix.edits), ROUTINE_FIXED);
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
        .expect("braced_world_decl_span found the declaration");
    // The doc line is bound to `helper` and must go with it — `back_over_doc_run`
    // is what walks the span start back over it.
    assert_eq!(apply_fix(ROUTINE_DOC_RUN, &fix.edits), ROUTINE_FIXED);
}

#[test]
fn unused_routine_fix_takes_a_doc_run_a_comment_interrupts() {
    let ds = findings_for(ROUTINE_DOC_RUN_COMMENT, "unused-routine");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("braced_world_decl_span found the declaration");
    // `back_over_doc_run` must reach the `?` line across the `/* c */`, not
    // stop at it: a span starting at `routine` would leave the doc line
    // orphaned, turning a valid file into a parse error.
    assert_eq!(
        apply_fix(ROUTINE_DOC_RUN_COMMENT, &fix.edits),
        ROUTINE_FIXED
    );
}

// -- unused-binding / reuse_statement_span ---------------------------------
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
/// has one — is gone. `reuse_statement_span` anchors on the TARGET name
/// (`helper`), so its span starts at the `bind`/`?` keyword-or-doc-run
/// token, not at the start of the (indented) line: the two leading spaces
/// that indented the deleted statement are untouched text and stay behind,
/// leaving a line of bare indentation where the statement sat. That is not
/// a typo — deleting it would be a different, wider fix this helper does
/// not make.
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
        .expect("reuse_statement_span found the statement");
    assert_eq!(apply_fix(BIND_CLEAN, &fix.edits), BIND_FIXED);
}

#[test]
fn unused_binding_fix_is_unchanged_by_a_comment_before_the_name() {
    let ds = findings_for(BIND_COMMENT, "unused-binding");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("reuse_statement_span found the statement");
    assert_eq!(apply_fix(BIND_COMMENT, &fix.edits), BIND_FIXED);
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
        .expect("reuse_statement_span found the statement");
    // The doc line is bound to the `bind` statement and must go with it —
    // `back_over_doc_run` is what walks the span start back over it.
    assert_eq!(apply_fix(BIND_DOC_RUN, &fix.edits), BIND_FIXED);
}

#[test]
fn unused_binding_fix_takes_a_doc_run_a_comment_interrupts() {
    let ds = findings_for(BIND_DOC_RUN_COMMENT, "unused-binding");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("reuse_statement_span found the statement");
    // Same orphaning risk as the routine shape: the span must reach the `?`
    // line across the `/* c */`, or the deleted statement leaves its doc run
    // behind and the file no longer parses.
    assert_eq!(apply_fix(BIND_DOC_RUN_COMMENT, &fix.edits), BIND_FIXED);
}

// -- unused-graft-name / as_clause_span ------------------------------------
//
// `as_clause_span` anchors on the `as` keyword and requires the token BEFORE
// it to be the signature's `)` and the token AFTER it to be the instance
// name. Two adjacency checks rather than one, so a comment on either side of
// `as` voids the fix.

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
/// variant the `/* c */` is interior to that span and goes with it.
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
fn unused_graft_name_fix_is_unchanged_by_a_comment_before_as() {
    let ds = findings_for(GRAFT_NAME_COMMENT, "unused-graft-name");
    assert_eq!(ds.len(), 1, "{ds:?}");
    let fix = ds
        .into_iter()
        .next()
        .unwrap()
        .fix
        .expect("as_clause_span found the clause");
    assert_eq!(apply_fix(GRAFT_NAME_COMMENT, &fix.edits), GRAFT_NAME_FIXED);
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

// -- the one ctx.tokens consumer that is NOT adjacency-sensitive -----------
//
// `arrow_span` (`dead-map-pair`) is the sixth and last reader of
// `LintContext.tokens`, and the only one the filter does not protect —
// because it needs no protection. It locates the `=>`/`->` of a map pair by
// RANGE CONTAINMENT (an Arrow token whose span lies between the pair's source
// and destination glyphs), never by index arithmetic off a neighbour, and a
// comment lexes as one `Comment` token that can never match `TokenKind::Arrow`.
// Extra tokens in the stream therefore cannot move the result.
//
// This fixture is the witness for that exclusion, which is what makes the
// five-helper hazard set a CLOSED enumeration rather than a count. It does
// not discriminate the filter (it passes with the filter removed — measured);
// it discriminates a future rewrite of `arrow_span` into an index walk, which
// is exactly when the exclusion would stop holding.

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
