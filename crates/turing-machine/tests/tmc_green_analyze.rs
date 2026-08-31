//! `analyze`'s front end lexes `LexMode::WithComments` and parses through
//! the green tree (`parse_green_from_tokens` + `syntax::extract_program`).
//! Everything downstream of the parse is untouched by that shape, so the
//! risk of it lives in two claims, and this file is where both are pinned:
//!
//! 1. **Errors did not move when the front end changed under them.** Each
//!    source in [`BROKEN`] fails with a fixed kind at a fixed span, both
//!    written out in the table itself. The spans are literals captured from
//!    the pre-green front — a comment-free lex then the retired
//!    hand-written-CST lowering — while that path was still callable; the broken sources carry the weight
//!    because a front-end switch changes which function raises the error,
//!    and only a source that errors exercises that at all.
//!
//! 2. **The lint layer still sees the old stream.** `analyze` keeps a
//!    comment-bearing `tokens`, and `lint()` filters it back through
//!    `parser::significant_tokens` before the rules walk it. That filter is
//!    only sound if the filtered stream equals a comment-free lex EXACTLY —
//!    same kinds, same lines, same columns, same lengths — because several
//!    quickfixes locate a declaration by token adjacency
//!    (`tests/lint_quickfix_comments.rs` pins what breaks when they don't).
//!    This one runs over every `.tmc` the repo ships, so a corpus file added
//!    later is covered for free.
//!
//! **What this file cannot see, by construction.** It computes the front
//! itself and never calls `analyze` — which is `pub(crate)` and out of an
//! integration test's reach anyway — so it pins that the RECIPE behaves,
//! not that `analyze` is wired to it. The wiring is pinned one level down,
//! by `compiler::tests::analyze_keeps_comment_trivia_in_its_token_stream`.
//!
//! What this file does NOT check: that the extracted `Program` is CORRECT
//! on a source that parses. Nothing here reads a successful `Program` at
//! all. Per-field extraction fidelity lives in `syntax::extract`'s own unit
//! tests and in the goldens.

use mtc_core::diagnostics::Span;
use mtc_core::syntax::SyntaxNode;
use mtc_turing_machine::CompileError;
use mtc_turing_machine::lexer::{LexMode, lex, lex_with};
use mtc_turing_machine::parser::{Program, parse_green_from_tokens};
use mtc_turing_machine::syntax::extract_program;

/// Every directory in this repo that ships a `.tmc` file, rooted at
/// `CARGO_MANIFEST_DIR` rather than the process CWD.
const CORPUS_ROOTS: [&str; 3] = ["tests/golden", "src/stdlib", "../../docs/examples"];

/// `(path, source)` for every `.tmc` under [`CORPUS_ROOTS`], sorted so a
/// failure names the same first file on every machine.
fn corpus() -> Vec<(std::path::PathBuf, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for dir in CORPUS_ROOTS {
        let full = root.join(dir);
        // A missing root is a broken test, not a reason to check less.
        let entries = std::fs::read_dir(&full)
            .unwrap_or_else(|e| panic!("corpus root {} is unreadable: {e}", full.display()));
        // `docs/examples/` holds one directory per worked example, so
        // the walk descends one level before filtering.
        let mut paths: Vec<std::path::PathBuf> = entries
            .map(|entry| entry.expect("readable entry").path())
            .flat_map(|path| {
                if path.is_dir() {
                    std::fs::read_dir(&path)
                        .unwrap_or_else(|e| {
                            panic!("example directory {} is unreadable: {e}", path.display())
                        })
                        .map(|sub| sub.expect("readable entry").path())
                        .collect()
                } else {
                    vec![path]
                }
            })
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("tmc"))
            .collect();
        paths.sort();
        assert!(
            !paths.is_empty(),
            "corpus root {} contributed no .tmc file",
            full.display()
        );
        for path in paths {
            let src = std::fs::read_to_string(&path).expect("readable source");
            out.push((path, src));
        }
    }
    assert!(
        out.len() >= 9,
        "expected the whole .tmc corpus, saw {}",
        out.len()
    );
    out
}

/// The front end `analyze` runs now: a `WithComments` lex, the green parse
/// over the same grammar walk, then extraction.
fn new_front(source: &str) -> Result<Program, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    Ok(extract_program(&SyntaxNode::new_root(green), source))
}

/// Sources that must NOT compile, one per failure position worth
/// distinguishing. Each is asserted `Err` on the old front below, so a
/// fixture that drifts valid cannot quietly turn this test into a no-op.
///
/// Half of them carry a comment sitting immediately before, or inside, the
/// failing construct. Those are the ones that discriminate: a comment-free
/// broken source cannot tell a `WithComments` lex from a comment-free one at
/// all, so a set without them would prove nothing about the switch.
/// One broken-source row: a name for the failure position, the source,
/// the error code it must fail with, and that error's span as
/// `(start line, start col, end line, end col)` — spelled as numbers
/// because `Span::new` is not `const`.
type BrokenCase = (
    &'static str,
    &'static str,
    &'static str,
    (u32, u32, u32, u32),
);

const BROKEN: &[BrokenCase] = &[
    // -- lexical -----------------------------------------------------------
    (
        "unterminated block comment",
        "/* never closed",
        "lex-error",
        (1, 1, 1, 2),
    ),
    (
        "unterminated block comment after a good declaration",
        "alphabet ab { '_', 'a' }\n/* never closed\n",
        "lex-error",
        (2, 1, 2, 2),
    ),
    (
        "stray character",
        "alphabet ab { '_' }\n$\n",
        "lex-error",
        (2, 1, 2, 2),
    ),
    (
        "unterminated glyph literal",
        "alphabet ab { '_', 'a }\n",
        "lex-error",
        (1, 20, 1, 21),
    ),
    // -- parse: truncation -------------------------------------------------
    (
        "bare keyword, EOF",
        "alphabet",
        "unexpected-token",
        (1, 9, 1, 9),
    ),
    (
        "declaration cut off mid-body",
        "alphabet ab { '_',",
        "unexpected-token",
        (1, 19, 1, 19),
    ),
    (
        "comment where the name should be, then EOF",
        "alphabet /* c */",
        "unexpected-token",
        (1, 17, 1, 17),
    ),
    // -- parse: wrong token ------------------------------------------------
    (
        "reserved word as an alphabet name",
        "alphabet machine { '_' }\n",
        "reserved-name",
        (1, 10, 1, 17),
    ),
    (
        "reserved word as a name, behind a comment",
        "alphabet /* c */ machine { '_' }\n",
        "reserved-name",
        (1, 18, 1, 25),
    ),
    (
        "bracket-less rule pattern",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s { * -> stop; }\n}\n",
        "naked-pattern",
        (4, 19, 4, 20),
    ),
    (
        "bracket-less rule pattern, comment before the pattern",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s { /* c */ * -> stop; }\n}\n",
        "naked-pattern",
        (4, 27, 4, 28),
    ),
    // -- parse: language rules ---------------------------------------------
    (
        "two machine blocks",
        "alphabet ab { '_' }\nmachine {\n  tape t: ab;\n  entry state s { [*] -> move [>] stop; }\n}\nmachine {\n  tape u: ab;\n  entry state s { [*] -> move [>] stop; }\n}\n",
        "multiple-machines",
        (6, 1, 6, 8),
    ),
    (
        "tape declaration inside a routine",
        "alphabet ab { '_' }\nroutine r(tape t: ab) {\n  tape u: ab;\n  entry state s { [*] -> return; }\n}\n",
        "tape-not-in-machine",
        (3, 3, 3, 7),
    ),
    (
        "tape declaration inside a routine, doc-run and comment above it",
        "alphabet ab { '_' }\nroutine r(tape t: ab) {\n  ? a tape that may not live here\n  /* c */\n  tape u: ab;\n  entry state s { [*] -> return; }\n}\n",
        "dangling-doc-run",
        (3, 3, 3, 34),
    ),
    (
        "wildcard binding",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s { [* as v] -> stop; }\n}\n",
        "wildcard-binding",
        (4, 20, 4, 21),
    ),
    (
        "mismatched range endpoints",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s { ['a'..3] -> stop; }\n}\n",
        "range-kind-mismatch",
        (4, 20, 4, 26),
    ),
];

/// The front end, over the shipped corpus: every `.tmc` the repo ships
/// must survive it — lex, green parse and extraction alike.
///
/// This is reach, not correctness: nothing here reads the `Program` back.
/// It exists because the corpus sweep is the only thing that runs the
/// front over `../../docs/examples` and over any `.tmc` a later plan adds
/// to one of the roots, and a file that made extraction panic or the
/// parse reject would otherwise surface only wherever that file happens
/// to be compiled.
#[test]
fn every_shipped_tmc_parses_and_extracts() {
    for (path, src) in corpus() {
        assert!(
            new_front(&src).is_ok(),
            "{}: the shipped corpus must survive the front end: {:?}",
            path.display(),
            new_front(&src).err()
        );
    }
}

/// The front end, over the broken set — the half that carries the
/// weight, since the switch to the green tree changed which function
/// produces the error. Each source must fail with a FIXED kind at a
/// FIXED span, both written out below rather than compared against a
/// second computation of the same thing: the spans are literals captured
/// from the pre-switch front (a comment-free lex then the retired
/// hand-written-CST lowering) while that path was still callable, so this table is what says the
/// error positions did not move when the front end changed under them.
#[test]
fn broken_sources_fail_with_a_fixed_kind_and_span() {
    for (name, src, code, (sl, sc, el, ec)) in BROKEN {
        let err = new_front(src)
            .err()
            .unwrap_or_else(|| panic!("`{name}` is supposed to be a broken source, but it parsed"));
        assert_eq!(
            (err.kind.code(), err.span),
            (*code, Span::new(*sl, *sc, *el, *ec)),
            "`{name}`: the error kind and span must not move"
        );
    }
}

/// Claim 2: `significant_tokens` of the `WithComments` stream `analyze` now
/// keeps is element-for-element identical to the comment-free lex it used to
/// keep — kind, line, col and len alike, not merely the kinds.
///
/// `Token` derives `PartialEq`, and the whole-`Vec` `assert_eq!` also catches
/// a length difference, so a mode that dropped or added a NON-comment token
/// anywhere would fail here. `significant_tokens` is `pub(crate)`, so the
/// filter is re-derived inline: an independent expression of the same rule is
/// what makes this an oracle rather than a tautology.
#[test]
fn corpus_significant_tokens_equal_a_comment_free_lex() {
    let mut with_comments = 0;
    for (path, src) in corpus() {
        let raw = lex_with(&src, LexMode::WithComments).expect("lexes");
        let significant: Vec<_> = raw
            .iter()
            .filter(|t| !matches!(t.kind, mtc_turing_machine::lexer::TokenKind::Comment(_)))
            .cloned()
            .collect();
        if significant.len() < raw.len() {
            with_comments += 1;
        }
        assert_eq!(
            significant,
            lex(&src).expect("lexes"),
            "{}: the filtered stream is not the comment-free lex",
            path.display()
        );
    }
    // A corpus of comment-free files would pass the assertion above without
    // the filter ever removing anything, which would make this test vacuous.
    assert!(
        with_comments >= 3,
        "only {with_comments} corpus files carry a comment — this check is close to vacuous"
    );
}
