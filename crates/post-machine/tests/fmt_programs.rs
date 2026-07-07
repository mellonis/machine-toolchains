//! `pmt fmt` objective-guard harness
//! (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`, "Contracts").
//! Three corpus-wide checks that hold for every input the pretty-printer
//! claims to support: idempotence, behaviour preservation (compiled
//! bytes unchanged at `-O0` and `-O1`), and comment fidelity. This is the
//! objective backstop for the whole fmt build — reviewer approval does
//! NOT substitute for these passing.
//!
//! At this task (fmt build Task 4 / 4b) the pretty-printer implements
//! only the TRIVIAL subset (see `crate::fmt`'s module doc): unlabeled
//! statements, no comments, no namespaces/imports, single-line comma
//! groups. `SIMPLE` is scoped to exactly that subset — Tasks 5-8 widen
//! it (labels, comma-group wrapping, comments, blank
//! lines/imports/namespaces/spacing/edge-cases) as each seam closes,
//! eventually pointing this harness at the full corpus (Task 9).

use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::format;
use mtc_post_machine::lexer::{LexMode, TokenKind, lex_with};
use mtc_post_machine::optimizer::OptLevel;

/// Valid `.pmc` programs the trivial printer fully supports.
const SIMPLE: &[&str] = &[
    "main() { right; }",
    "f() { right; @g(); } g() { left; }",
    "main() { left, right, mark; }",
    "export f() { right(!); } g() { @f(); mark(!); } main() { @g(); debugger; halt; }",
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
