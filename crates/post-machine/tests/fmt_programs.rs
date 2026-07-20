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
//! never hands the printer the author's original spacing (a path's
//! segments print tight regardless of source spacing), so the
//! spaced-form entries below normalize for free — they widen the
//! corpus to PIN that, not to fix a gap. (A number's own digits are a
//! separate matter — the CST carries those as WRITTEN, leading zeros
//! and all; see `zero_token_changes_over_every_fixture` below.)
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
//! the printer over a real, organically-written program. These three
//! checks are format-RELATIVE and hold regardless of whether the input
//! already matches the canonical style.
//!
//! **Task 11** reformats `std.pmc` and the two goldens fmt-clean
//! (labels hang left into the command column) and adds the dogfood
//! assertions below: `format(x) == x` byte-identical for each, the
//! regression lock that catches any future drift from the canonical
//! style. The lint fixture (`tests/lint/unused_labels.pmc`) is
//! deliberately left NOT fmt-clean — `lint_programs.rs` pins its
//! diagnostics' `span.start.line`, which a reformat would shift.

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
    // The literal `export` keyword on `main`, preserved verbatim
    // even though `main`'s auto-export makes it semantically redundant.
    "export main() { right; }",
    // Task 8b §B: spaced-form normalization — a spaced label (`1 :
    // right`) normalizes to tight (`1: right`) because the CST never
    // stores the author's interior spacing around `:` (the digits
    // themselves are stored and reprinted as written, unaffected here
    // since `1` has no leading zeros to preserve).
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
    // Finalize §1 ("c-brace"): a comment on the SAME line as the
    // closing `}` rides that line instead of forcing its own — the
    // corpus-level partner to `fmt::tests::cbrace_a_...`.
    "f() { right; } // t",
    // Finalize §1: a comment on the SAME line as the opening `{`, before
    // the first statement — a LINE comment (`fmt::tests::cbrace_b_...`)
    // and a BLOCK comment (`fmt::tests::cbrace_c_...`) both stay on the
    // header line; either way the body starts on the next line.
    "f() { // note\n right;\n}",
    "f() { /* c */ right; }",
    // Finalize §2 (M3): a LINE comment leading a statement's first
    // comma-group item (between an own-line label's `:` and the first
    // command) forces the group onto multiple lines
    // (`fmt::tests::m3_item0_...`).
    "f() { 1: // c\n left, right; }",
    // Namespace c-brace fix: a comment on the SAME line as the
    // namespace's opening `{` and another on its closing `}` both stay
    // on their respective brace lines — the corpus-level partner to
    // `fmt::tests::ns_cbrace_*`.
    "namespace ns { // note\n    f() { right; }\n} // t\nmain() { @ns::f(); }",
    // M2 fix: a mid-comma-group BLOCK comment that spans two physical
    // source lines — the corpus-level partner to
    // `fmt::tests::m2_multiline_comment_greedy_fill_uses_last_line_width`.
    // Forces `right` to start a new greedy-fill group whose `first` item
    // embeds a `\n`; the width tracker must measure only the comment's
    // last physical line, not both lines summed, when deciding whether
    // the five `mark`s that follow need to wrap.
    "main() { left, /* xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\ny */ right, mark, mark, mark, mark, mark; }",
    // Zero-token-changes (module doc "fmt changes no tokens, only
    // layout"): a leading-zero label definition and its `goto` reference
    // both keep their written spelling — fmt is not `leading-zeros`'s
    // job (docs/fmt.md, docs/lint.md). Right-aligns by the WRITTEN
    // width of "007:" (4 chars), not the canonical value's width.
    "main() {\n    007: right;\n    goto 007;\n}\n",
    // fmt build Task 1 (`docs/superpowers/specs/2026-07-12-pmc-doc-lines-\
    // attributes-design.md`, "fmt"): the canonical doc/attention-run
    // shape — a top-level function documented with a two-paragraph doc
    // (an empty `?` line breaks the paragraph) and a `[deprecated]`
    // attention line with a message, plus a nested function with its own
    // one-line doc, run printed at the nested indent. Already fmt-clean
    // (byte-identical round trip pinned locally in `fmt::tests`); doubles
    // as the token-spelling guard's doc-text coverage (`007` embedded in
    // the first paragraph, `zero_token_changes_over_every_fixture` below
    // proves the digits inside prose are untouched, same discipline as
    // the leading-zero label case above but for a `String` payload
    // instead of a `Number`).
    "? Adds one to the accumulator, wrapping through cell 007 as a sentinel.\n?\n? Steps the head by calling the nested helper below.\n! [deprecated] use addTwoAndKeep instead\nexport addOne() {\n    ? Moves the head one cell to the right.\n    step() {\n        right;\n    }\n    @step();\n}\n",
    // fmt build Task 1: the same run, but every `?`/`!` line's canonical
    // single space is scrambled (dropped where present, added as a bare
    // trailing space on the empty paragraph-break line) — the lexer's
    // one-leading-space-stripped rule (`docs/language.md` (doc lines))
    // already normalizes this to the SAME stored text as the canonical
    // entry above, so fmt's output is expected to be byte-identical to
    // it (pinned directly in `fmt::tests::scrambled_doc_run_spacing_\
    // normalizes_to_canonical`); listed here too so the corpus-wide
    // idempotence/behaviour/comment-fidelity/token-spelling checks cover
    // the scrambled INPUT shape as well, not just the already-canonical
    // one.
    "?Adds one to the accumulator, wrapping through cell 007 as a sentinel.\n? \n?Steps the head by calling the nested helper below.\n![deprecated] use addTwoAndKeep instead\nexport addOne() {\n    ?Moves the head one cell to the right.\n    step() {\n        right;\n    }\n    @step();\n}\n",
    // fmt build Task 1: an ordinary `//` comment interleaved inside a doc
    // run — prints under the existing comment rules (design doc's fmt
    // section), widening `comment_fidelity` to actually exercise a doc
    // run (the two entries above carry no `//`/`/* */` trivia at all).
    "? first\n// mid comment\n? second\nmain() {\n    right;\n}\n",
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

/// `TokenKind` sequence, comments stripped. Unlike a check that only
/// compares each `Number`'s parsed VALUE, `TokenKind`'s derived
/// `PartialEq` compares the whole variant — `Number` carries the raw
/// digit text alongside the value, so `007` and `7` are UNEQUAL tokens
/// here even though they parse to the same label. Mirrors the `.pma`
/// fmt suite's kind+text guard (`crates/core/src/asm/fmt.rs`,
/// "zero token changes").
fn kinds(src: &str) -> Vec<TokenKind> {
    lex_with(src, LexMode::WithoutComments)
        .expect("lexes")
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

/// fmt's own zero-token-changes contract (docs/fmt.md: "fmt changes
/// whitespace and comment placement only — it never touches a token"),
/// checked over the whole corpus by SPELLING, not just parsed value —
/// the class of bug this guards is a printer that re-derives a token's
/// text from its parsed value instead of reprinting what the author
/// wrote (leading zeros collapsing being the motivating case).
#[test]
fn zero_token_changes_over_every_fixture() {
    for (label, src) in all_sources() {
        let formatted = format(src).expect("formats");
        assert_eq!(
            kinds(src),
            kinds(&formatted),
            "token spelling changed for {label:?}"
        );
    }
}

/// A leading-zero label definition and its `goto` reference both keep
/// `007` verbatim (docs/fmt.md, docs/lint.md), and the label
/// right-aligns using the WRITTEN width ("007:", 4 chars) — command
/// column 8, not the canonical value's width ("7:", 2 chars) — which
/// would floor the column at 4.
#[test]
fn leading_zero_label_preserves_spelling_and_aligns_by_written_width() {
    let src = "main() {\n    007: right;\n    goto 007;\n}\n";
    let formatted = format(src).expect("formats");
    assert_eq!(
        formatted,
        "main() {\n   007: right;\n        goto 007;\n}\n"
    );
}

/// Task 11's dogfood lock (fmt design doc, Acceptance #1): the embedded
/// stdlib and the two historic goldens are committed in fmt-clean form,
/// so `format` must be a no-op on them byte-for-byte. This is the
/// regression guard — any future printer change that would reformat
/// these files fails here first, not silently on the next `pmt fmt` run.
/// The lint fixture is excluded on purpose (module doc above).
#[test]
fn dogfood_stdlib_and_goldens_are_already_fmt_clean() {
    for (label, src) in [
        ("std.pmc", include_str!("../src/stdlib/std.pmc")),
        ("golden/sum.pmc", include_str!("golden/sum.pmc")),
        ("golden/ty.pmc", include_str!("golden/ty.pmc")),
    ] {
        let formatted = format(src).expect("formats");
        assert_eq!(formatted, src, "{label} is not fmt-clean");
    }
}
