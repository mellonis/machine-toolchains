//! The resilient `.pmc` parse (docs/core.md (syntax trees), error
//! recovery): any input that lexes yields a lossless green tree —
//! broken regions wrapped in ERROR nodes at top-level item boundaries —
//! and the SAME first error the fatal entry reports. The batch
//! pipeline's fatal contract is untouched; these tests pin the new
//! entry's three laws: losslessness on broken input, first-error
//! equality with the fatal parse, and byte-identical trees on clean
//! input.

use mtc_core::syntax::SyntaxNode;
use mtc_post_machine::lexer::{LexMode, lex_with};
use mtc_post_machine::parser::{parse_green_from_tokens, parse_green_resilient};
use mtc_post_machine::syntax::{PmcKind, kind_name};

fn resilient(src: &str) -> (SyntaxNode, Vec<mtc_post_machine::compiler::CompileError>) {
    let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
    let parse = parse_green_resilient(src, &tokens);
    (SyntaxNode::new_root(parse.green), parse.errors)
}

/// A broken first declaration does not take the rest of the file with
/// it: the tree is lossless, the error matches the fatal entry's, and
/// the LATER declaration is still a real FUNCTION node.
#[test]
fn a_broken_declaration_leaves_its_neighbours_parsed() {
    let src = "use ;\nmain() {\n 1: right;\n}\n";
    let (root, errors) = resilient(src);
    assert_eq!(root.text(), src, "the tree lost source text");
    let fatal = {
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        parse_green_from_tokens(src, &tokens).expect_err("the fatal entry errors here")
    };
    assert_eq!(errors.len(), 1, "one recovery region, one error");
    assert_eq!(
        errors[0], fatal,
        "the resilient error must equal the fatal one"
    );
    assert!(
        root.children().any(|c| c.kind() == PmcKind::Error.into()),
        "the broken region is wrapped in an ERROR node"
    );
    assert!(
        root.children()
            .any(|c| c.kind() == PmcKind::Function.into()),
        "the later declaration still parses as a FUNCTION"
    );
}

/// INNER recovery: a broken STATEMENT does not take its function with
/// it — the FUNCTION node survives at top level with the error region
/// wrapped inside its body, and the sibling function is untouched.
#[test]
fn a_broken_statement_leaves_its_function_parsed() {
    let src = "main() {\n 1: right;\n 2: wat wat;\n 3: left;\n}\nhelper() {\n 1: right;\n}\n";
    let (root, errors) = resilient(src);
    assert_eq!(root.text(), src, "the tree lost source text");
    assert_eq!(errors.len(), 1, "one recovery region, one error");
    let functions: Vec<_> = root
        .children()
        .filter(|c| c.kind() == PmcKind::Function.into())
        .collect();
    assert_eq!(
        functions.len(),
        2,
        "BOTH functions survive at top level — the error region is inner"
    );
    assert!(
        root.children().all(|c| c.kind() != PmcKind::Error.into()),
        "no top-level error region — recovery happened inside the body"
    );
    assert!(
        functions[0]
            .children()
            .any(|c| c.kind() == PmcKind::Error.into()),
        "the broken statement is wrapped inside its own function"
    );
}

/// Total junk (that still lexes) becomes one lossless ERROR region.
#[test]
fn junk_input_yields_one_lossless_error_region() {
    let src = ") ( : ,\n";
    let (root, errors) = resilient(src);
    assert_eq!(root.text(), src);
    assert!(!errors.is_empty());
    assert!(
        root.children().all(|c| c.kind() == PmcKind::Error.into()),
        "nothing here parses as a real declaration"
    );
}

/// On a clean document the resilient entry builds the IDENTICAL tree
/// the fatal entry does, with no errors — over the whole shipped
/// corpus.
#[test]
fn clean_sources_build_the_identical_tree() {
    let mut checked = 0;
    for dir in ["tests/golden", "src/stdlib"] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("pmc") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable");
            let tokens = lex_with(&src, LexMode::WithComments).expect("lexes");
            let fatal = parse_green_from_tokens(&src, &tokens).expect("corpus parses");
            let parse = parse_green_resilient(&src, &tokens);
            assert!(
                parse.errors.is_empty(),
                "{}: spurious error",
                path.display()
            );
            let name = |k| kind_name(k).to_string();
            assert_eq!(
                mtc_core::syntax::debug_dump(&SyntaxNode::new_root(parse.green), &name),
                mtc_core::syntax::debug_dump(&SyntaxNode::new_root(fatal), &name),
                "{}: resilient tree differs on clean input",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked >= 3, "expected the corpus, saw {checked}");
}

/// Deleting any single significant token from a valid source leaves an
/// input the resilient parse still covers losslessly, reporting the
/// fatal entry's own first error whenever the fatal entry errors.
#[test]
fn single_token_deletions_stay_lossless_and_error_equal() {
    let src = include_str!("golden/sum.pmc");
    let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
    let spans: Vec<(usize, usize)> = {
        // Byte ranges of the significant tokens, via the lexer's own
        // line/col and a scan — deleting one produces the mutant.
        let index = mtc_core::syntax::TextLineIndex::new(src);
        tokens
            .iter()
            .filter(|t| {
                !matches!(
                    t.kind,
                    mtc_post_machine::lexer::TokenKind::Eof
                        | mtc_post_machine::lexer::TokenKind::Comment(_)
                )
            })
            .map(|t| {
                let start = index.offset(mtc_core::diagnostics::Pos {
                    line: t.line,
                    col: t.col,
                }) as usize;
                (start, t.len as usize)
            })
            .collect()
    };
    for &(start, len_chars) in &spans {
        let end = src[start..]
            .char_indices()
            .nth(len_chars)
            .map(|(o, _)| start + o)
            .unwrap_or(src.len());
        let mutant: String = format!("{}{}", &src[..start], &src[end..]);
        let Ok(mtokens) = lex_with(&mutant, LexMode::WithComments) else {
            continue; // a deletion that breaks the lexer is out of scope
        };
        let parse = parse_green_resilient(&mutant, &mtokens);
        let root = SyntaxNode::new_root(parse.green);
        assert_eq!(
            root.text(),
            mutant,
            "lossless law broken for deletion at byte {start}"
        );
        match parse_green_from_tokens(&mutant, &mtokens) {
            Ok(_) => assert!(
                parse.errors.is_empty(),
                "spurious resilient error at byte {start}"
            ),
            Err(fatal) => {
                assert_eq!(
                    parse.errors.first(),
                    Some(&fatal),
                    "first-error inequality for deletion at byte {start}"
                );
            }
        }
    }
}
