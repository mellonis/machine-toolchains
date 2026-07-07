//! `pmt fmt` objective-guard harness
//! (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`, "Contracts").
//! Three corpus-wide checks that hold for every input the pretty-printer
//! claims to support: idempotence, behaviour preservation (compiled
//! bytes unchanged at `-O0` and `-O1`), and comment fidelity. This is the
//! objective backstop for the whole fmt build — reviewer approval does
//! NOT substitute for these passing.
//!
//! At this task (fmt build Tasks 4-7) the pretty-printer implements the
//! TRIVIAL subset plus label/command-column alignment plus comma-group
//! layout plus comment placement/alignment (see `crate::fmt`'s module
//! doc): labeled or unlabeled statements, no namespaces/imports, comma
//! groups including multi-line ones (author line breaks preserved,
//! greedy-fill on overflow), and every comment placement — leading,
//! standalone, dangling, trailing (lone/aligned-run/ragged-run),
//! block-comment re-indent, and mid-comma-group (block-inline /
//! line-forces-a-break). `SIMPLE` is scoped to exactly that subset —
//! Task 8 widens it further (blank lines/imports/namespaces/spacing/
//! edge-cases) as its seam closes, eventually pointing this harness at
//! the full corpus (Task 9).
//!
//! **`comment_fidelity` was VACUOUS through Task 6** — `SIMPLE` carried
//! no comments, so `comment_texts(src)` was always `[]` on both sides.
//! The commented entries added here (Task 7) are what make the check
//! actually exercise something: each carries at least one real comment,
//! so a regression that lost or reordered one would fail this test.

use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::format;
use mtc_post_machine::lexer::{LexMode, TokenKind, lex_with};
use mtc_post_machine::optimizer::OptLevel;

/// Valid `.pmc` programs the printer fully supports.
const SIMPLE: &[&str] = &[
    "main() { right; }",
    "f() { right; @g(); } g() { left; }",
    "main() { left, right, mark; }",
    "export f() { right(!); } g() { @f(); mark(!); } main() { @g(); debugger; halt; }",
    // Task 5: an inline labeled statement + an unlabeled one, sharing a
    // command column (spec "Label / command alignment").
    "main() { 1: right; goto 1; }",
    // Task 5: own-line labels (`label_break`) — one that fits the label
    // field, one that overflows and hangs (spec "Own-line labels"). The
    // riskiest idempotence case: fmt's re-parse must re-derive the same
    // `label_break` from the newline it just emitted.
    "main() {\n11111: right;\n12:\nleft;\n999999999:\nhalt;\n}\n",
    // Task 6: rule 3 — the author split a comma group across two lines;
    // fmt preserves the grouping and aligns the continuation to the
    // command column (spec "Comma-group layout").
    "main() {\n1: left, right,\nmark;\n}\n",
    // Task 6: rule 2 — no author newline, but the one-line join overflows
    // 80 and greedy-fill wraps it. The riskiest idempotence case here:
    // re-parsing the wrapped output reads the greedy-fill break as an
    // author `newline_before`, so a second pass must reproduce the same
    // bytes via rule 3 instead of rule 2.
    "main() { @abcdefghijklmnopq(), @abcdefghijklmnopq(), @abcdefghijklmnopq(), @abcdefghijklmnopq(); } abcdefghijklmnopq() { halt; }",
    // Task 6: rule 3 whose first preserved line itself overflows 80 —
    // greedy-fill applies to that line only, the second preserved line
    // (`mark`) stays untouched.
    "main() { @abcdefghijklmnopq(), @abcdefghijklmnopq(), @abcdefghijklmnopq(), @abcdefghijklmnopq(),\nmark; } abcdefghijklmnopq() { halt; }",
    // Task 7: a leading comment run directly above the function it
    // documents (the `std.pmc` doc-comment shape).
    "// leading comment stays above f at indent 0\n// a note\nf() {\n    right;\n}\n",
    // Task 7: a lone trailing comment — one space, no alignment run.
    "f() {\n    right; // go\n}\n",
    // Task 7: a trailing-comment run the author aligned in source —
    // fmt maintains the alignment, recomputed against the reformatted
    // code (`mark;` is 9 chars incl `;`, `check(!, !);` is 16 — same
    // width as `check(1, 2)`, but both-return arms need no label defs
    // to compile; the shared column is 17, an 8-space pad for `mark`
    // and 1 for `check`).
    "f() {\n    mark;        // a\n    check(!, !); // b\n}\n",
    // Task 7: the same two statements, but ragged in source (one space
    // each, at different absolute columns) — stays ragged.
    "f() {\n    mark; // a\n    check(!, !); // b\n}\n",
    // Task 7: a dangling comment at the end of a body, before `}`.
    "f() {\n    right;\n    // dangling\n}\n",
    // Task 7: a standalone comment, blank-separated on both sides.
    "f() {\n    right;\n\n    // standalone\n\n    left;\n}\n",
    // Task 7: a block comment whose interior line carries its own
    // (unrelated) indentation, preserved verbatim.
    "f() {\n    /* line one\n   line two */\n    right;\n}\n",
    // Task 7: a mid-comma-group BLOCK comment — stays inline.
    "f() {\n 1: left, /* mid */ right;\n}\n",
    // Task 7: a mid-comma-group LINE comment — forces the group onto a
    // second line (nothing can follow `//` on its own physical line).
    "f() {\n    left, // note\n    right;\n}\n",
];

#[test]
fn idempotence() {
    for src in SIMPLE {
        let once = format(src).expect("formats");
        let twice = format(&once).expect("reformats");
        assert_eq!(twice, once, "format(format(x)) != format(x) for {src:?}");
    }
}

#[test]
fn behaviour_preservation_at_o0_and_o1() {
    for src in SIMPLE {
        let formatted = format(src).expect("formats");

        let orig_o0 = compile(src, CompileOptions::default()).expect("compiles at -O0");
        let fmt_o0 =
            compile(&formatted, CompileOptions::default()).expect("formatted compiles at -O0");
        assert_eq!(
            orig_o0.object, fmt_o0.object,
            "-O0 object bytes diverged for {src:?}"
        );

        let o1 = CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        };
        let orig_o1 = compile(src, o1.clone()).expect("compiles at -O1");
        let fmt_o1 = compile(&formatted, o1).expect("formatted compiles at -O1");
        assert_eq!(
            orig_o1.object, fmt_o1.object,
            "-O1 object bytes diverged for {src:?}"
        );
    }
}

fn comment_texts(src: &str) -> Vec<String> {
    lex_with(src, LexMode::WithComments)
        .expect("lexes")
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::Comment(c) => Some(c.text.trim().to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn comment_fidelity() {
    for src in SIMPLE {
        let formatted = format(src).expect("formats");
        assert_eq!(
            comment_texts(&formatted),
            comment_texts(src),
            "comment sequence diverged for {src:?}"
        );
    }
}
