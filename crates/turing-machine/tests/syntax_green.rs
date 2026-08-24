//! Green-parser goldens for `.tmc`: emission woven into the existing
//! parser for the outer container productions. Expected shapes are
//! derived by hand from the tree-shape rules — trivia flushes into the
//! current node before a child opens, so a node starts at its first
//! significant token — never pasted from a run.

use mtc_core::syntax::{SyntaxNode, debug_dump};
use mtc_turing_machine::compiler::CompileError;
use mtc_turing_machine::lexer::lex;
use mtc_turing_machine::parser::parse as parse_ast;
use mtc_turing_machine::parser::parse_green;
use mtc_turing_machine::syntax::kind_name;

fn dump(source: &str) -> String {
    let tree = parse_green(source).expect("parses");
    let root = SyntaxNode::new_root(tree);
    assert_eq!(root.text(), source, "lossless law");
    debug_dump(&root, &|k| kind_name(k).to_string())
}

fn parse(source: &str) -> SyntaxNode {
    let tree = parse_green(source).expect("parses");
    let root = SyntaxNode::new_root(tree);
    assert_eq!(root.text(), source, "lossless law");
    root
}

fn kind_of(node: &SyntaxNode) -> &'static str {
    kind_name(node.kind())
}

/// `"alphabet ab { '_' }\n"` — ALPHABET spans `alphabet`..`}` inclusive.
/// The trailing newline belongs to ROOT, not to ALPHABET: the node
/// closes right after its `}`.
///
/// An `assert!(d.contains(...))`/`ends_with(...)` pair, as originally
/// drafted, cannot discriminate a correctly-shaped tree from a
/// differently-shaped one: `debug_dump` always renders a token line as
/// `KIND@lo..hi "text"\n`, so the string NEVER ends with the bare word
/// `WHITESPACE` — the quoted text always follows it. That check could
/// not pass for ANY tree, correct or not (confirmed by running it: it
/// failed on the tree this task's own derivation says is right). The
/// exact dump below is the fix — hand-derived from the source's byte
/// offsets (`alphabet`=0..8, ` `=8..9, `ab`=9..11, ` `=11..12,
/// `{`=12..13, ` `=13..14, `'_'`=14..17, ` `=17..18, `}`=18..19,
/// `\n`=19..20) — and it is strictly stronger: it pins every child's
/// kind, span and text, not merely that some node contains the word
/// `ALPHABET` somewhere.
#[test]
fn an_alphabet_declaration() {
    let d = dump("alphabet ab { '_' }\n");
    let expected = "ROOT@0..20\n\
        \x20 ALPHABET@0..19\n\
        \x20   IDENT@0..8 \"alphabet\"\n\
        \x20   WHITESPACE@8..9 \" \"\n\
        \x20   IDENT@9..11 \"ab\"\n\
        \x20   WHITESPACE@11..12 \" \"\n\
        \x20   L_BRACE@12..13 \"{\"\n\
        \x20   WHITESPACE@13..14 \" \"\n\
        \x20   GLYPH@14..17 \"'_'\"\n\
        \x20   WHITESPACE@17..18 \" \"\n\
        \x20   R_BRACE@18..19 \"}\"\n\
        \x20 WHITESPACE@19..20 \"\\n\"\n";
    assert_eq!(d, expected);
}

/// A leading comment is a ROOT-level token before ALPHABET opens, not a
/// child of it — trivia flushes into the CURRENT node, and ALPHABET is
/// not open yet.
#[test]
fn a_leading_comment_belongs_to_the_root() {
    let d = dump("// lead\nalphabet ab { '_' }\n");
    let lead = d.find("LINE_COMMENT").expect("comment present");
    let alpha = d.find("ALPHABET").expect("alphabet present");
    assert!(lead < alpha, "comment must precede the node it leads: {d}");

    // The `lead < alpha` ordering above only rules out the comment
    // landing as a DESCENDANT of ALPHABET (a child node's own line
    // always prints after its parent's opening line, so a wrongly
    // nested comment would print `ALPHABET@..` first and the comment
    // line after it, flipping the order). It does not by itself prove
    // the comment is a ROOT child rather than, say, a second unrelated
    // top-level node — the direct-tree walk below closes that gap:
    // ROOT's first child token is the comment, its second is the
    // leading whitespace, and ALPHABET is ROOT's only child NODE.
    let root = parse("// lead\nalphabet ab { '_' }\n");
    let mut children = root.children_with_tokens();
    let first = children.next().expect("a leading token");
    assert_eq!(kind_name(first.kind()), "LINE_COMMENT");
    let mtc_core::syntax::SyntaxElement::Token(first_tok) = &first else {
        panic!("ROOT's leading child must be a token, not a node: {first:?}");
    };
    assert_eq!(first_tok.text(), "// lead");
    let alphabet = root.children().next().expect("an ALPHABET child node");
    assert_eq!(kind_of(&alphabet), "ALPHABET");
}

/// A weak `contains("MACHINE")` check would still pass for a MACHINE
/// node closed the instant the `machine` keyword is consumed, with the
/// whole `{ … }` body left dangling as ROOT-level tokens beside it —
/// the substring is present either way. Asserting the node's own
/// `text()` against the hand-derived expected span (the whole
/// declaration, trailing `\n` excluded — same rule as ALPHABET's)
/// closes that gap: it only holds if MACHINE actually wraps its body.
#[test]
fn a_machine_with_one_tape() {
    let source = "machine {\n  tape main: ab;\n}\n";
    let root = parse(source);
    let machine = root.children().next().expect("a MACHINE child");
    assert_eq!(kind_of(&machine), "MACHINE");
    assert_eq!(
        machine.text(),
        "machine {\n  tape main: ab;\n}",
        "MACHINE must span the whole block, body included, trailing \\n excluded"
    );
}

/// Namespaces nest, and a `machine` block may NOT sit inside one — that
/// is a language rule (docs/tmt/language.md (namespaces)), so the
/// nesting fixture uses declarations, not a machine.
///
/// A `matches("NAMESPACE").count() == 2` plus a textual `first_ns <
/// alpha` ordering check, as originally drafted, would both still pass
/// for a tree where the inner `namespace b` and its ALPHABET were
/// closed too early and reopened as ROOT-level SIBLINGS of `namespace
/// a` rather than actual descendants — the count only counts opens,
/// and every one of these nodes still appears in source order
/// regardless of nesting depth, so the ordering check can't tell a
/// sibling from a child either. Walking the real tree via `children()`
/// one level at a time is the only check that actually proves each
/// node is nested INSIDE the previous one, not merely printed after it.
#[test]
fn nested_namespaces() {
    let source = "namespace a {\n  namespace b {\n    export alphabet ab { '_', 'a' }\n  }\n}\n";
    let root = parse(source);
    let outer = root.children().next().expect("outer NAMESPACE");
    assert_eq!(kind_of(&outer), "NAMESPACE");
    let inner = outer
        .children()
        .next()
        .expect("inner NAMESPACE nested inside the outer");
    assert_eq!(kind_of(&inner), "NAMESPACE");
    let alphabet = inner
        .children()
        .next()
        .expect("ALPHABET nested inside the inner namespace");
    assert_eq!(kind_of(&alphabet), "ALPHABET");
    // The fixture's `export` prefix is deliberate: ALPHABET's own
    // checkpoint is taken before the top-level dispatch match runs, so
    // it must retroactively wrap `export` too, not just the `alphabet`
    // keyword onward — the same gap a plain kind/nesting check above
    // cannot see, since a mis-scoped checkpoint that drops `export` as
    // a loose child of the inner namespace still leaves ALPHABET
    // correctly nested one level down.
    assert_eq!(alphabet.text(), "export alphabet ab { '_', 'a' }");
    // Both NAMESPACE extents, pinned directly: neither the kind/nesting
    // walk above nor the corpus test (lossless-only — it can't tell a
    // node's tokens from a sibling's) would notice if OUTER stopped
    // including its own `{`/`}` pair, or if INNER's checkpoint drifted
    // to include or exclude the wrong indentation.
    assert_eq!(
        outer.text(),
        "namespace a {\n  namespace b {\n    export alphabet ab { '_', 'a' }\n  }\n}"
    );
    assert_eq!(
        inner.text(),
        "namespace b {\n    export alphabet ab { '_', 'a' }\n  }"
    );
    // Every NAMESPACE the corpus contains is one of these two — no
    // stray third one hiding somewhere else in the tree.
    assert_eq!(
        outer
            .children()
            .filter(|c| kind_of(c) == "NAMESPACE")
            .count(),
        1
    );
    assert_eq!(
        inner
            .children()
            .filter(|c| kind_of(c) == "NAMESPACE")
            .count(),
        0
    );
}

/// A `matches("USE_PATH").count() == 2` check, as originally drafted,
/// says nothing about WHERE the two nodes sit or what text each one
/// actually claims — a USE_PATH that swallowed the separating comma
/// into its own span, or one that left a stray path as a ROOT-level
/// sibling instead of a USE child, would still count to 2. Walking
/// USE's actual children and asserting their exact text (each path's
/// own span, comma and surrounding whitespace excluded — the same
/// "closes right after its own last token" rule as UsePath's alias
/// case) closes both gaps.
#[test]
fn a_use_declaration_with_two_paths() {
    let source = "use std::binaryNumbers,\n    other::thing;\n";
    let root = parse(source);
    let use_node = root.children().next().expect("a USE child");
    assert_eq!(kind_of(&use_node), "USE");
    let paths: Vec<SyntaxNode> = use_node.children().collect();
    assert_eq!(paths.len(), 2, "two USE_PATH children");
    for p in &paths {
        assert_eq!(kind_of(p), "USE_PATH");
    }
    assert_eq!(paths[0].text(), "std::binaryNumbers");
    assert_eq!(paths[1].text(), "other::thing");
    // USE's own extent, both endpoints at once: this catches USE opened
    // late (after the `use` keyword instead of at it) as readily as USE
    // closed early (before the `;`) — the corpus test can't see either,
    // since it only asserts losslessness, which holds even when a
    // token is emitted OUTSIDE the node it should be inside.
    assert_eq!(
        use_node.text(),
        "use std::binaryNumbers,\n    other::thing;"
    );
}

/// The `alias`, when present, is USE_PATH's own last token — a claim
/// the `g_finish` comment inside `parse_use` makes but nothing
/// verifies: the two-path fixture above has no alias, and no shipped
/// `.tmc` file uses `use … as` either, so the whole corpus is silent on
/// this shape.
#[test]
fn a_use_path_with_an_alias() {
    let source = "use std::binaryNumbers as bn;\n";
    let root = parse(source);
    let use_node = root.children().next().expect("a USE child");
    let path = use_node
        .children()
        .next()
        .expect("a USE_PATH child carrying the alias");
    assert_eq!(kind_of(&path), "USE_PATH");
    assert_eq!(path.text(), "std::binaryNumbers as bn");
}

/// Acceptance parity is the whole point of the sink: it only mirrors an
/// UNCHANGED grammar walk, so a rejecting source must fail identically
/// through both parse paths — same error kind, same span, via
/// `CompileError`'s derived `PartialEq` — and an accepting one must
/// succeed on both. This is the one check in this file the six goldens
/// above cannot stand in for: every one of them parses `WithComments`
/// through `parse_green` alone, so a divergence that only shows up on a
/// REJECTING source, or only under `WithoutComments` lexing (the CST
/// path via `parse`), is invisible to them.
///
/// Two failure modes this specifically guards against: `GreenSink`'s own
/// `debug_assert!`s (`flush`'s out-of-order guard, `token`'s
/// already-emitted guard) firing where the CST path returns a clean
/// `Err` — a panic instead of an error IS an acceptance divergence — and
/// a builder left with an unclosed node by an error raised mid-
/// production, untested by the corpus test below since every shipped
/// `.tmc` file parses cleanly.
///
/// None of the fixtures below contain a comment, so `WithComments` vs
/// `WithoutComments` lexing produces the same token positions either
/// way (only `Comment` itself is mode-gated — `DocLine`/`AttentionLine`
/// are emitted in both), making `parse` (comment-free) and `parse_green`
/// (`WithComments`) a fair apples-to-apples comparison on these sources.
#[test]
fn errors_agree_with_the_cst_path() {
    // Each rejecting source and the reason, taken from the existing
    // `parser::tests` battery so the expected code is independently
    // pinned there too (`machine_cannot_nest_in_a_namespace`,
    // `reserved_keywords_cannot_name_things`,
    // `more_than_one_machine_in_a_file_is_rejected_at_parse`, and
    // `dangling_doc_run_is_rejected`).
    let rejecting = [
        "namespace n { machine { } }", // unexpected-token
        "alphabet state { '_' }",      // reserved-name
        "routine goto() { }",          // reserved-name
        "use mylib::graph;",           // reserved-name
        "machine { } machine { }",     // multiple-machines
        "? orphan\nuse mylib::x;",     // dangling-doc-run
        "? orphan\n",                  // dangling-doc-run (nothing follows)
    ];
    for src in rejecting {
        let cst_err = parse_ast(&lex(src).expect("lexes")).expect_err("rejected on the CST path");
        let green_err: CompileError = parse_green(src).expect_err("rejected on the green path");
        assert_eq!(
            cst_err, green_err,
            "{src}: kind and span must agree exactly"
        );
    }

    let accepting = [
        "alphabet ab { '_' }\n",
        "machine {\n  tape main: ab;\n}\n",
        "namespace a {\n  namespace b {\n    export alphabet ab { '_', 'a' }\n  }\n}\n",
        "use std::binaryNumbers,\n    other::thing;\n",
    ];
    for src in accepting {
        assert!(parse_ast(&lex(src).expect("lexes")).is_ok(), "{src}");
        assert!(parse_green(src).is_ok(), "{src}");
    }
}

/// `parse_green` computes `eof_pos = sig.len() - 1` and calls
/// `finish_tree(eof_pos)` — a path none of the goldens above exercise,
/// since every one of them has at least one significant token before
/// EOF. Empty and whitespace-only source have EOF as their only
/// significant token (`sig.len() == 1`, `eof_pos == 0`), the edge where
/// an off-by-one here would underflow or hand `finish_tree` the wrong
/// position. Both must still satisfy the lossless law.
#[test]
fn empty_and_whitespace_only_source_stay_lossless() {
    for src in ["", "\n\n\n"] {
        let tree = parse_green(src).expect("parses");
        let root = SyntaxNode::new_root(tree);
        assert_eq!(root.text(), src, "{src:?} is not lossless");
    }
}

/// The law over every `.tmc` the repo ships, including the flagship
/// brainfuck universal machine and the embedded stdlib.
#[test]
fn the_whole_shipped_corpus_is_lossless() {
    let mut checked = 0;
    for dir in ["tests/golden", "src/stdlib", "../../docs/examples"] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("tmc") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable");
            let tree = parse_green(&src)
                .unwrap_or_else(|e| panic!("{} failed to parse: {e:?}", path.display()));
            let root = SyntaxNode::new_root(tree);
            assert_eq!(root.text(), src, "{} is not lossless", path.display());
            checked += 1;
        }
    }
    assert!(
        checked >= 9,
        "expected the whole .tmc corpus, saw {checked}"
    );
}
