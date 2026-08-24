//! Extraction parity: the `Program` built from the green tree equals the
//! one built from the CST, struct for struct, over every `.tmc` the repo
//! ships. The green tree's own lossless law cannot catch an extraction
//! bug — a tree can round-trip perfectly and still be read wrongly — so
//! this is the check that the two paths agree about MEANING rather than
//! about bytes.
//!
//! `Program` and everything it holds derive `PartialEq` over every
//! field, spans included (there is no hand-written `PartialEq` anywhere
//! in the AST), so `assert_eq!` here compares names, order, namespace
//! stamping, reduced docs, AND every `line`/`col`/`span` the C1 lowering
//! anchors on a token. Two fields are nonetheless out of this oracle's
//! reach BY CONSTRUCTION, and are pinned one level down in
//! `syntax::extract`'s own unit tests instead: `DocRunItem::blank_before`
//! never reaches a `Program` (`crate::parser::reduce_doc_run` folds over
//! `kind` alone), and the `take_trailing` divergence on a multi-line
//! block comment riding a `;` is a `blank_before` disagreement only.
//!
//! The companion property over GENERATED programs lives beside the
//! generator in `tmc_property.rs`; the corpus covers the shapes real
//! `.tmc` files are written in, the generator the ones nobody wrote.

use mtc_core::syntax::SyntaxNode;
use mtc_turing_machine::lexer::{LexMode, lex_with};
use mtc_turing_machine::parser::{lower_cst, parse_cst, parse_green_from_tokens};
use mtc_turing_machine::syntax::extract_program;

/// Every directory in this repo that ships a `.tmc` file: the golden
/// programs, the embedded stdlib, and the flagship example. Rooted at
/// `CARGO_MANIFEST_DIR` rather than the process CWD, matching the rest of
/// this crate's tests (`fmt_tmc.rs`, `footprint_property.rs`).
const CORPUS_ROOTS: [&str; 3] = ["tests/golden", "src/stdlib", "../../docs/examples"];

#[test]
fn the_shipped_corpus_extracts_identically_on_both_paths() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut checked = 0;
    for dir in CORPUS_ROOTS {
        let full = root.join(dir);
        // A missing root is a broken test, not a reason to check less:
        // silently skipping one is how a corpus sweep decays into
        // sweeping nothing.
        let entries = std::fs::read_dir(&full)
            .unwrap_or_else(|e| panic!("corpus root {} is unreadable: {e}", full.display()));
        let mut paths: Vec<std::path::PathBuf> = entries
            .map(|entry| entry.expect("readable entry").path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("tmc"))
            .collect();
        // `read_dir` order is filesystem-dependent; sorting makes a
        // failure report the same first file on every machine.
        paths.sort();
        assert!(
            !paths.is_empty(),
            "corpus root {} contributed no .tmc file",
            full.display()
        );
        for path in paths {
            let src = std::fs::read_to_string(&path).expect("readable source");
            let tokens = lex_with(&src, LexMode::WithComments).expect("lexes");
            let expected = lower_cst(&parse_cst(&tokens).expect("parses"));
            let green = parse_green_from_tokens(&src, &tokens).expect("parses");
            let actual = extract_program(&SyntaxNode::new_root(green), &src);
            assert_eq!(actual, expected, "{} extracts differently", path.display());
            checked += 1;
        }
    }
    assert!(
        checked >= 9,
        "expected the whole .tmc corpus, saw {checked}"
    );
}
