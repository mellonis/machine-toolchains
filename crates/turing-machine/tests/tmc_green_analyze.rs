//! `analyze`'s front end changed shape: it lexes `LexMode::WithComments` and
//! parses through the green tree (`parse_green_from_tokens` +
//! `syntax::extract_program`) where it used to lex comment-free and run
//! `parse` (`lower_cst ∘ parse_cst`). Everything downstream of the parse is
//! untouched, so the whole risk of that switch lives in two claims, and this
//! file is where both are pinned:
//!
//! 1. **Acceptance and errors did not move.** For the same source, the two
//!    fronts produce the same `Result<Program, CompileError>` — the same
//!    program on success, and on failure the same error kind at the same
//!    span. The BROKEN sources carry most of the weight here: the switch
//!    changes which function produces the error, and only a source that
//!    errors exercises that.
//!
//! 2. **The lint layer still sees the old stream.** `analyze` now keeps a
//!    comment-bearing `tokens`, and `lint()` filters it back through
//!    `parser::significant_tokens` before the rules walk it. That filter is
//!    only sound if the filtered stream equals a comment-free lex EXACTLY —
//!    same kinds, same lines, same columns, same lengths — because several
//!    quickfixes locate a declaration by token adjacency
//!    (`tests/lint_quickfix_comments.rs` pins what breaks when they don't).
//!
//! Both run over every `.tmc` the repo ships, the same corpus the extraction
//! oracles use (`tests/syntax_parity.rs`), so a corpus file added later is
//! covered here for free.
//!
//! **What this file cannot see, by construction.** It computes both fronts
//! itself and never calls `analyze` — which is `pub(crate)` and out of an
//! integration test's reach anyway — so it pins that the two RECIPES agree,
//! not that `analyze` is wired to the new one. Every test here passes
//! unchanged against the pre-migration tree, and stays green if `analyze`'s
//! body is reverted to the pre-migration recipe — a comment-free `lex`
//! then `lower_cst(parse_cst(...))`, exactly what `old_front` below
//! computes directly rather than through `parser::parse` (measured, by
//! doing exactly that). The wiring itself is pinned one level down, by
//! `compiler::tests::analyze_keeps_comment_trivia_in_its_token_stream`.
//!
//! What this file does NOT re-check: that the extracted `Program` equals the
//! CST lowering on the shipped corpus and on generated programs — that is
//! `tests/syntax_parity.rs` and `tests/tmc_property.rs`. Claim 1 below
//! compares the two fronts as whole `Result`s, so it happens to subsume the
//! corpus half of that; the point of asserting it here anyway is the error
//! side, which neither oracle looks at.

use mtc_core::syntax::SyntaxNode;
use mtc_turing_machine::CompileError;
use mtc_turing_machine::lexer::{LexMode, lex, lex_with};
use mtc_turing_machine::parser::{Program, lower_cst, parse_cst, parse_green_from_tokens};
use mtc_turing_machine::syntax::extract_program;

/// Every directory in this repo that ships a `.tmc` file — the same three
/// roots `syntax_parity.rs` walks, rooted at `CARGO_MANIFEST_DIR` rather than
/// the process CWD.
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
        let mut paths: Vec<std::path::PathBuf> = entries
            .map(|entry| entry.expect("readable entry").path())
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

/// The front end `analyze` ran before the green-tree switch: a comment-free
/// lex, then `lower_cst ∘ parse_cst` over the C1 CST — composed here
/// directly, since `parser::parse` itself now runs the green front this
/// file calls `new_front` below and can no longer stand in for the old one.
fn old_front(source: &str) -> Result<Program, CompileError> {
    let tokens = lex(source)?;
    parse_cst(&tokens).map(|cst| lower_cst(&cst))
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
const BROKEN: &[(&str, &str)] = &[
    // -- lexical -----------------------------------------------------------
    ("unterminated block comment", "/* never closed"),
    (
        "unterminated block comment after a good declaration",
        "alphabet ab { '_', 'a' }\n/* never closed\n",
    ),
    ("stray character", "alphabet ab { '_' }\n$\n"),
    ("unterminated glyph literal", "alphabet ab { '_', 'a }\n"),
    // -- parse: truncation -------------------------------------------------
    ("bare keyword, EOF", "alphabet"),
    ("declaration cut off mid-body", "alphabet ab { '_',"),
    (
        "comment where the name should be, then EOF",
        "alphabet /* c */",
    ),
    // -- parse: wrong token ------------------------------------------------
    (
        "reserved word as an alphabet name",
        "alphabet machine { '_' }\n",
    ),
    (
        "reserved word as a name, behind a comment",
        "alphabet /* c */ machine { '_' }\n",
    ),
    (
        "bracket-less rule pattern",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s { * -> stop; }\n}\n",
    ),
    (
        "bracket-less rule pattern, comment before the pattern",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s { /* c */ * -> stop; }\n}\n",
    ),
    // -- parse: language rules ---------------------------------------------
    (
        "two machine blocks",
        "alphabet ab { '_' }\nmachine {\n  tape t: ab;\n  entry state s { [*] -> move [>] stop; }\n}\nmachine {\n  tape u: ab;\n  entry state s { [*] -> move [>] stop; }\n}\n",
    ),
    (
        "tape declaration inside a routine",
        "alphabet ab { '_' }\nroutine r(tape t: ab) {\n  tape u: ab;\n  entry state s { [*] -> return; }\n}\n",
    ),
    (
        "tape declaration inside a routine, doc-run and comment above it",
        "alphabet ab { '_' }\nroutine r(tape t: ab) {\n  ? a tape that may not live here\n  /* c */\n  tape u: ab;\n  entry state s { [*] -> return; }\n}\n",
    ),
    (
        "wildcard binding",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s { [* as v] -> stop; }\n}\n",
    ),
    (
        "mismatched range endpoints",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s { ['a'..3] -> stop; }\n}\n",
    ),
];

/// Claim 1 over the shipped corpus: every real `.tmc` file must produce the
/// same `Program` on both fronts. `Program` derives `PartialEq` over every
/// field, spans included, so this compares names, order, namespace stamping
/// and every anchored `line`/`col` — not merely that both sides succeeded.
#[test]
fn corpus_analyze_fronts_agree() {
    for (path, src) in corpus() {
        let old = old_front(&src);
        let new = new_front(&src);
        assert!(
            old.is_ok(),
            "{}: the shipped corpus must parse on the old front: {:?}",
            path.display(),
            old.err()
        );
        assert_eq!(
            old,
            new,
            "{}: the two analyze fronts disagree",
            path.display()
        );
    }
}

/// Claim 1 over the broken set — the half that carries the weight, since the
/// switch changes which function produces the error. `CompileError` derives
/// `PartialEq` over `kind` AND `span`, so `assert_eq!` on the whole `Result`
/// pins both without spelling either out.
#[test]
fn broken_sources_fail_identically_on_both_fronts() {
    for (name, src) in BROKEN {
        let old = old_front(src);
        let new = new_front(src);
        assert!(
            old.is_err(),
            "`{name}` is supposed to be a broken source, but the old front accepted it"
        );
        assert_eq!(old, new, "`{name}`: the two analyze fronts disagree");
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
