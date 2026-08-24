//! Pins the APPLIED TEXT of the three deletion-quickfix helpers shared by the
//! `unused-*` rules (`docs/tmt/lint.md` (the `.tmc` rules)) — `decl_span`
//! (`unused-alphabet`), `braced_world_decl_span` (`unused-routine`), and
//! `reuse_statement_span` (`unused-binding`). Each helper locates the
//! declared name's token, then requires the token immediately BEFORE it in
//! the token stream to be the declaration's keyword — strict adjacency, not
//! a search.
//!
//! Today the compiler's front end lexes comment-free, so a `/* ... */`
//! comment written between a declaration's keyword and its name never
//! reaches the token stream these helpers walk; the fix still finds the
//! keyword immediately before the name and deletes exactly the declaration,
//! comment included (the span is a pair of source byte offsets taken from
//! token boundaries, so raw untokenized text between them — a comment —
//! rides along). Each fixture pair below (a clean declaration and the same
//! declaration with a comment inserted right after its keyword) is
//! asserted against the SAME expected output for exactly that reason: the
//! comment changes nothing about what gets deleted, only whether the
//! adjacency check would still hold once comments become real tokens. If
//! the front end ever starts lexing with comments included, a comment in
//! that position stops being invisible — it becomes the token immediately
//! before the name, the adjacency check fails, and the fix silently
//! disappears (or, for a leading doc/attention run, is silently
//! truncated). Asserting only "a fix exists" or a diagnostic count is
//! blind to both outcomes; asserting the applied text is not.

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
