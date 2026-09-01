//! WHERE a comment may sit after `pmt fmt` — the never-move rule, as an
//! executable audit: a comment is printed between the same two significant
//! tokens it was written between; the formatter may change the whitespace
//! around it, never which tokens it sits between. The sibling `.tmc`
//! formatter already holds this rule (docs/tmt/fmt.md (comments are never
//! moved)); this file is the `.pmc` port's work list and regression guard,
//! one template per grammatical position a comment can occupy — 39 of
//! them (20 correct at base + 19 in the three moving families), each
//! with a `@C@` slot run in BOTH flavours (`/* c */` and `// c`).
//!
//! Each case runs three mechanical gates: the comment's significant-token
//! NEIGHBOURS are unchanged, the significant-token stream is unchanged
//! (fmt is whitespace-only), and the output is a fixed point.
//!
//! The groups mirror the port's work plan: `ALREADY_CORRECT` is the
//! regression guard — 20 positions the current printer already gets right
//! — and each `#[ignore]`d group is one task's work list, its ignore
//! reason recording the MEASURED destination the comment moves to today
//! (2026-09-01, this branch's base). Un-ignoring a group is how its task
//! proves its surface.
//!
//! Baseline facts, measured on this harness at the branch base: no
//! comment is ever lost and no fixture fails to parse; the three moving
//! families are declaration headers (the relocation docs/pmt/fmt.md
//! documents today), every parenthesized list (`check`/successor/call
//! arguments, and a comment inside a `use` path), and the label slots —
//! including the stacked-label shape that is also fmt's one
//! non-idempotent position.

use mtc_post_machine::format;
use mtc_post_machine::lexer::{LexMode, TokenKind, lex_with};

/// The comment's nearest significant token on each side, by lexing with
/// comments retained. Positions are deliberately NOT part of the answer —
/// layout may change; neighbours may not.
fn neighbours(src: &str) -> (Option<String>, Option<String>) {
    let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
    let ci = tokens
        .iter()
        .position(|t| matches!(t.kind, TokenKind::Comment(_)))
        .expect("the fixture carries one comment");
    let before = tokens[..ci]
        .iter()
        .rev()
        .find(|t| !matches!(t.kind, TokenKind::Comment(_)))
        .map(|t| format!("{:?}", t.kind));
    let after = tokens[ci + 1..]
        .iter()
        .find(|t| !matches!(t.kind, TokenKind::Comment(_)))
        .map(|t| format!("{:?}", t.kind));
    (before, after)
}

/// The comment-free token-kind stream — the whitespace-only gate's view.
fn significant_kinds(src: &str) -> Vec<String> {
    lex_with(src, LexMode::WithoutComments)
        .expect("lexes")
        .into_iter()
        .map(|t| format!("{:?}", t.kind))
        .collect()
}

/// Runs one template in both flavours through the three gates.
#[track_caller]
fn never_moves(id: &str, tpl: &str) {
    for sub in ["/* c */", "// c"] {
        let src = tpl.replace("@C@", sub);
        let out = format(&src).unwrap_or_else(|e| panic!("{id} [{sub}]: fmt failed: {e:?}"));
        assert_eq!(
            neighbours(&src),
            neighbours(&out),
            "{id} [{sub}]: the comment moved\n--- input ---\n{src}\n--- output ---\n{out}"
        );
        assert_eq!(
            significant_kinds(&src),
            significant_kinds(&out),
            "{id} [{sub}]: fmt changed a token\n--- input ---\n{src}\n--- output ---\n{out}"
        );
        let again = format(&out).unwrap_or_else(|e| panic!("{id} [{sub}]: second pass: {e:?}"));
        assert_eq!(
            out, again,
            "{id} [{sub}]: fmt is not idempotent\n--- input ---\n{src}"
        );
    }
}

#[track_caller]
fn run_group(group: &[(&str, &str)]) {
    for (id, tpl) in group {
        never_moves(id, tpl);
    }
}

// ---------------------------------------------------------------------------
// The regression guard: 19 positions the current printer already gets
// right. These pass at the branch base and must never stop passing.
// ---------------------------------------------------------------------------

const ALREADY_CORRECT: &[(&str, &str)] = &[
    ("file/before-first", "@C@\nmain() {\n 1: right;\n}\n"),
    (
        "file/between-items",
        "use a::b;\n@C@\nmain() {\n 1: right;\n}\n",
    ),
    ("file/after-last", "main() {\n 1: right;\n}\n@C@\n"),
    (
        "use/after-keyword",
        "use @C@\n    a::b;\nmain() {\n 1: right;\n}\n",
    ),
    (
        "use/between-paths",
        "use a::b, @C@\n    c::d;\nmain() {\n 1: right;\n}\n",
    ),
    (
        "use/before-semi",
        "use a::b @C@\n    ;\nmain() {\n 1: right;\n}\n",
    ),
    ("use/trailing", "use a::b; @C@\nmain() {\n 1: right;\n}\n"),
    (
        "doc/between-lines",
        "? contract line one\n@C@\n? contract line two\nmain() {\n 1: right;\n}\n",
    ),
    (
        "doc/run-to-header",
        "? contract\n@C@\nmain() {\n 1: right;\n}\n",
    ),
    ("header/open-brace", "main() { @C@\n 1: right;\n}\n"),
    (
        "namespace/open-brace",
        "namespace n { @C@\n    f() {\n     1: right;\n    }\n}\nmain() {\n 1: right;\n}\n",
    ),
    (
        "namespace/before-close",
        "namespace n {\n    f() {\n     1: right;\n    }\n    @C@\n}\nmain() {\n 1: right;\n}\n",
    ),
    (
        "namespace/trailing",
        "namespace n {\n    f() {\n     1: right;\n    }\n} @C@\nmain() {\n 1: right;\n}\n",
    ),
    ("stmt/own-line-before", "main() {\n @C@\n 1: right;\n}\n"),
    ("stmt/after-colon", "main() {\n 1: @C@\n    right;\n}\n"),
    (
        "stmt/between-commands-same-line",
        "main() {\n 1: right, @C@\n    left;\n}\n",
    ),
    (
        "stmt/between-commands-own-line",
        "main() {\n 1: right,\n @C@\n    left;\n}\n",
    ),
    ("stmt/trailing", "main() {\n 1: right; @C@\n}\n"),
    ("stmt/before-close", "main() {\n 1: right;\n @C@\n}\n"),
    ("fn/trailing", "main() {\n 1: right;\n} @C@\n"),
];

#[test]
fn already_correct_positions_stay() {
    run_group(ALREADY_CORRECT);
}

// ---------------------------------------------------------------------------
// Declaration headers: today a comment written inside a function's or
// namespace's header relocates to its own line at body indent, ahead of
// the first body element (the exception docs/pmt/fmt.md documents).
// ---------------------------------------------------------------------------

const HEADERS: &[(&str, &str)] = &[
    ("header/name-paren", "main @C@\n() {\n 1: right;\n}\n"),
    ("header/inside-parens", "main( @C@\n) {\n 1: right;\n}\n"),
    ("header/paren-brace", "main() @C@\n{\n 1: right;\n}\n"),
    (
        "header/export-name",
        "namespace n {\n    export @C@\n    f() {\n     1: right;\n    }\n}\nmain() {\n 1: right;\n}\n",
    ),
    (
        "namespace/kw-name",
        "namespace @C@\nn {\n    f() {\n     1: right;\n    }\n}\nmain() {\n 1: right;\n}\n",
    ),
    (
        "namespace/name-brace",
        "namespace n @C@\n{\n    f() {\n     1: right;\n    }\n}\nmain() {\n 1: right;\n}\n",
    ),
];

#[test]
fn header_comments_stay_in_their_headers() {
    run_group(HEADERS);
}

// ---------------------------------------------------------------------------
// Parenthesized lists: today a comment inside `check(...)`, a command's
// successor `(...)`, a call's `(...)`, or inside a `use` path relocates
// past the enclosing statement to its trailing position (past the whole
// path, for `use`).
// ---------------------------------------------------------------------------

const PAREN_LISTS: &[(&str, &str)] = &[
    (
        "use/inside-path",
        "use a @C@\n    ::b;\nmain() {\n 1: right;\n}\n",
    ),
    (
        "check/after-kw",
        "main() {\n 1: right;\n    check @C@\n    (1, 3);\n 3: left;\n}\n",
    ),
    (
        "check/open-paren",
        "main() {\n 1: right;\n    check( @C@\n    1, 3);\n 3: left;\n}\n",
    ),
    (
        "check/between-args",
        "main() {\n 1: right;\n    check(1, @C@\n    3);\n 3: left;\n}\n",
    ),
    (
        "check/before-close",
        "main() {\n 1: right;\n    check(1, 3 @C@\n    );\n 3: left;\n}\n",
    ),
    (
        "succ/before-parens",
        "main() {\n 1: right @C@\n    (3);\n 3: left;\n}\n",
    ),
    (
        "succ/inside-parens",
        "main() {\n 1: right( @C@\n    3);\n 3: left;\n}\n",
    ),
    (
        "succ/before-close",
        "main() {\n 1: right(3 @C@\n    );\n 3: left;\n}\n",
    ),
    (
        "call/name-parens",
        "use a::b;\nmain() {\n 1: @b @C@\n    ();\n}\n",
    ),
    (
        "call/inside-parens",
        "use a::b;\nmain() {\n 1: @b( @C@\n    3);\n 3: left;\n}\n",
    ),
];

#[test]
fn paren_list_comments_stay_in_their_lists() {
    run_group(PAREN_LISTS);
}

// ---------------------------------------------------------------------------
// Label slots: today `1 @C@ : right` crosses the `:`; `right @C@ ;`
// crosses the `;` into trailing; and a comment between STACKED labels
// joins the labels onto one line and crosses the second label — the
// stacked shape is also fmt's one non-idempotent position.
// ---------------------------------------------------------------------------

const LABEL_SLOTS: &[(&str, &str)] = &[
    ("stmt/label-colon", "main() {\n 1 @C@\n : right;\n}\n"),
    (
        "stmt/stacked-labels",
        "main() {\n 1:\n @C@\n 2: right;\n}\n",
    ),
    ("stmt/before-semi", "main() {\n 1: right @C@\n    ;\n}\n"),
];

#[test]
fn label_slot_comments_stay_in_their_slots() {
    run_group(LABEL_SLOTS);
}
