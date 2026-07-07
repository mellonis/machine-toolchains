//! `pmt fmt` objective-guard harness
//! (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`, "Contracts").
//! Three corpus-wide checks that hold for every input the pretty-printer
//! claims to support: idempotence, behaviour preservation (compiled
//! bytes unchanged at `-O0` and `-O1`), and comment fidelity. This is the
//! objective backstop for the whole fmt build — reviewer approval does
//! NOT substitute for these passing.
//!
//! At this task (fmt build Tasks 4-8b) the pretty-printer implements the
//! TRIVIAL subset plus label/command-column alignment plus comma-group
//! layout plus comment placement/alignment plus namespaces, the general
//! blank-line policy, imports/export printing, and the full
//! intra-statement spacing table / spaced-form normalization / textual
//! hygiene / edge cases (see `crate::fmt`'s module doc): labeled or
//! unlabeled statements, comma groups including multi-line ones (author
//! line breaks preserved, greedy-fill on overflow), every comment
//! placement — leading, standalone, dangling, trailing
//! (lone/aligned-run/ragged-run), block-comment re-indent, and
//! mid-comma-group (block-inline / line-forces-a-break) — namespace
//! blocks (nested recursion), blank lines (preserve/collapse/never
//! force), grouped `use` lists, the verbatim `export` keyword, spaced
//! forms (`1 : right`, `std :: goToEnd`) normalizing to tight, and the
//! comments-only-file / empty-function-body edge cases. `SIMPLE` is
//! scoped to exactly that subset.
//!
//! Task 8b's own contribution needed no renderer changes: `parse_cst`
//! only ever hands the printer the parsed VALUE (a label's number, a
//! path's segments), never the author's original spacing, so the
//! spaced-form entries below normalize for free — they widen the
//! corpus to PIN that, not to fix a gap.
//!
//! **`comment_fidelity` was VACUOUS through Task 6** — `SIMPLE` carried
//! no comments, so `comment_texts(src)` was always `[]` on both sides.
//! The commented entries added here (Task 7) are what make the check
//! actually exercise something: each carries at least one real comment,
//! so a regression that lost or reordered one would fail this test.
//!
//! **Task 9 widens all three checks to the FULL corpus** (`CORPUS`
//! below): the embedded stdlib (`src/stdlib/std.pmc` — namespaces, doc
//! comments, labels, calls, all together for the first time), the two
//! historic goldens (`tests/golden/sum.pmc` + `ty.pmc`), and the lint
//! fixture (`tests/lint/unused_labels.pmc`) — every `.pmc` file under
//! this crate, same corpus `tests/parser_parity.rs` already parses both
//! ways. This is the real validation: `SIMPLE` was hand-picked to
//! exercise one shape at a time, but nothing before this task had run
//! the printer over a real, organically-written program. No dogfood
//! `format(std.pmc) == std.pmc` assertion is added here (std.pmc is not
//! fmt-clean yet — that's Task 11's job); these three checks are
//! format-RELATIVE and hold regardless of whether the input already
//! matches the canonical style.

use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::format;
use mtc_post_machine::lexer::{LexMode, TokenKind, lex_with};
use mtc_post_machine::optimizer::OptLevel;

/// The full `.pmc` corpus: the embedded stdlib, the two historic
/// goldens, and the lint fixture — the same set `tests/parser_parity.rs`
/// parses through both the legacy and CST paths. Labelled so a failing
/// assertion names the file, not just an opaque `&str`.
const CORPUS: &[(&str, &str)] = &[
    ("std.pmc", include_str!("../src/stdlib/std.pmc")),
    ("golden/sum.pmc", include_str!("golden/sum.pmc")),
    ("golden/ty.pmc", include_str!("golden/ty.pmc")),
    (
        "lint/unused_labels.pmc",
        include_str!("lint/unused_labels.pmc"),
    ),
];

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
    // Task 8a: a namespace block, reached by a qualified call (spec
    // "Namespaces" — printed at +1 indent; nested recursion is exercised
    // by the module's own unit tests).
    "namespace ns { f() { right; } } main() { @ns::f(); }",
    // Task 8a: a blank line preserved between two top-level declarations
    // (spec "Blank lines" — preserve, collapse runs, never force).
    "f() {\n    right;\n}\n\ng() {\n    left;\n}\n",
    // Task 8a: a grouped `use` list — never split into separate
    // statements (spec "Imports" — order and grouping preserved
    // verbatim).
    "use a, b::c as d;\nmain() { @a(); @d(); }",
    // Task 8a: the literal `export` keyword on `main`, preserved verbatim
    // even though `main`'s auto-export makes it semantically redundant
    // (fmt design doc §D).
    "export main() { right; }",
    // Task 8b §B: spaced-form normalization — a spaced label (`1 :
    // right`) normalizes to tight (`1: right`) because the CST only
    // ever stores the parsed VALUE, never the author's spacing.
    "main() {\n1 : right;\n}\n",
    // Task 8b §B: a spaced path in both an import and a qualified call
    // (`std :: goToEnd`) normalizes to tight `std::goToEnd` the same
    // way.
    "use std :: goToEnd;\nmain() { @std :: goToEnd(); }",
    // Task 8b §D: a file of only comments, no declarations at all —
    // reprints the comments with one final newline (`compile` needs no
    // `main` to succeed, so this is a valid corpus entry too).
    "// just a comment\n// and another\n",
    // Task 8b §D: an empty function body, alone in the file — `f() { }`
    // prints as header + closing brace with no blank line between.
    "f() { }",
];

/// `SIMPLE` entries paired with a label equal to their own source (the
/// existing failure-message shape), chained ahead of the real-world
/// `CORPUS` — every entry below iterates `(label, src)` pairs so a
/// corpus failure names the file instead of printing the whole source.
fn all_sources() -> impl Iterator<Item = (&'static str, &'static str)> {
    SIMPLE
        .iter()
        .map(|s| (*s, *s))
        .chain(CORPUS.iter().copied())
}

#[test]
fn idempotence() {
    for (label, src) in all_sources() {
        let once = format(src).expect("formats");
        let twice = format(&once).expect("reformats");
        assert_eq!(twice, once, "format(format(x)) != format(x) for {label:?}");
    }
}

#[test]
fn behaviour_preservation_at_o0_and_o1() {
    for (label, src) in all_sources() {
        let formatted = format(src).expect("formats");

        let orig_o0 = compile(src, CompileOptions::default()).expect("compiles at -O0");
        let fmt_o0 =
            compile(&formatted, CompileOptions::default()).expect("formatted compiles at -O0");
        assert_eq!(
            orig_o0.object, fmt_o0.object,
            "-O0 object bytes diverged for {label:?}"
        );

        let o1 = CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        };
        let orig_o1 = compile(src, o1.clone()).expect("compiles at -O1");
        let fmt_o1 = compile(&formatted, o1).expect("formatted compiles at -O1");
        assert_eq!(
            orig_o1.object, fmt_o1.object,
            "-O1 object bytes diverged for {label:?}"
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
    for (label, src) in all_sources() {
        let formatted = format(src).expect("formats");
        assert_eq!(
            comment_texts(&formatted),
            comment_texts(src),
            "comment sequence diverged for {label:?}"
        );
    }
}
