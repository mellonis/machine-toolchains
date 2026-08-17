//! Green-parser goldens: `parse_green` woven into the existing `.pmc`
//! parser for the container productions (FILE, USE_DECL, USE_PATH,
//! NAMESPACE). Expected dumps are derived BY HAND from the plan's
//! tree-shape rules (derivation-first: never pasted from a run) —
//! trivia flushes into the current node before a child opens, so a
//! node starts at its first significant token; USE_DECL spans
//! `use`..`;` inclusive; USE_PATH spans its first path ident..its last
//! token inclusive (the `as` alias, when present) and excludes the
//! separating comma; NAMESPACE spans `namespace`..`}` inclusive.

use mtc_core::syntax::{SyntaxNode, debug_dump};
use mtc_post_machine::parser::parse_green;
use mtc_post_machine::syntax::kind_name;

fn dump(source: &str) -> String {
    let tree = parse_green(source).expect("parses");
    let root = SyntaxNode::new_root(tree);
    assert_eq!(root.text(), source, "lossless law");
    debug_dump(&root, &|k| kind_name(k).to_string())
}

/// `"use std::goToEnd;\n"` — bytes: use=0..3, ws=3..4, std=4..7,
/// ::=7..9, goToEnd=9..16, ;=16..17, \n=17..18. The trailing `\n`
/// belongs to FILE, not USE_DECL: USE_DECL closes right after `;`.
#[test]
fn use_decl_golden() {
    let src = "use std::goToEnd;\n";
    let expected = "\
FILE@0..18
  USE_DECL@0..17
    IDENT@0..3 \"use\"
    WHITESPACE@3..4 \" \"
    USE_PATH@4..16
      IDENT@4..7 \"std\"
      COLON_COLON@7..9 \"::\"
      IDENT@9..16 \"goToEnd\"
    SEMI@16..17 \";\"
  WHITESPACE@17..18 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// `"namespace ns {}\n"` — bytes: namespace=0..9, ws=9..10, ns=10..12,
/// ws=12..13, `{`=13..14, `}`=14..15, \n=15..16. An empty body: the
/// recursive `top_items` call bumps the closing `}` straight into the
/// still-open NAMESPACE node before it closes.
#[test]
fn empty_namespace_golden() {
    let src = "namespace ns {}\n";
    let expected = "\
FILE@0..16
  NAMESPACE@0..15
    IDENT@0..9 \"namespace\"
    WHITESPACE@9..10 \" \"
    IDENT@10..12 \"ns\"
    WHITESPACE@12..13 \" \"
    L_BRACE@13..14 \"{\"
    R_BRACE@14..15 \"}\"
  WHITESPACE@15..16 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// `"// c\nuse a;\n"` — a leading own-line comment. `flush` is called
/// (via `g_flush_start(UseDecl)`) BEFORE `UseDecl` opens, so both the
/// comment and its trailing newline land as FILE's own children, not
/// USE_DECL's — the same "flush into the still-open parent" rule the
/// inter-token whitespace goldens above exercise, just with a comment
/// piece in the trivia run this time. Byte map: `//`=0..2, ` `=2..3,
/// `c`=3..4 (all one LINE_COMMENT token 0..4), \n=4..5, use=5..8,
/// ws=8..9, a=9..10, `;`=10..11, \n=11..12.
#[test]
fn leading_comment_lands_outside_use_decl_golden() {
    let src = "// c\nuse a;\n";
    let expected = "\
FILE@0..12
  LINE_COMMENT@0..4 \"// c\"
  WHITESPACE@4..5 \"\\n\"
  USE_DECL@5..11
    IDENT@5..8 \"use\"
    WHITESPACE@8..9 \" \"
    USE_PATH@9..10
      IDENT@9..10 \"a\"
    SEMI@10..11 \";\"
  WHITESPACE@11..12 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// `"use a; // t\n"` — a same-line trailing comment after `;`. USE_DECL
/// closes right after the `;` bump (the boundary rule is purely
/// token-position-based, unlike the C1 CST's `UseCst::trailing`, which
/// attaches a same-line comment to the logical node it documents
/// regardless of where the node's own span ends): the comment and its
/// surrounding trivia are only flushed later, at `into_tree`'s trailing
/// flush, by which point USE_DECL has already closed — so they land as
/// FILE's own children. A deliberate CST/green-tree divergence, not a
/// defect: the green tree is a lossless CONCRETE tree keyed purely on
/// token boundaries, with no semantic "this comment documents that
/// node" attachment. Byte map: use=0..3, ws=3..4, a=4..5, `;`=5..6,
/// ws=6..7, `//`/` `/`t`=7..11 (one LINE_COMMENT token), \n=11..12.
#[test]
fn trailing_comment_after_semi_lands_outside_use_decl_golden() {
    let src = "use a; // t\n";
    let expected = "\
FILE@0..12
  USE_DECL@0..6
    IDENT@0..3 \"use\"
    WHITESPACE@3..4 \" \"
    USE_PATH@4..5
      IDENT@4..5 \"a\"
    SEMI@5..6 \";\"
  WHITESPACE@6..7 \" \"
  LINE_COMMENT@7..11 \"// t\"
  WHITESPACE@11..12 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// `"use a, std::b as c;\n"` — a comma-separated list with an alias on
/// the second path, exercising both halves of the USE_PATH boundary
/// rule at once: the separating `,` (byte 5..6) sits OUTSIDE both
/// USE_PATH nodes as a USE_DECL child, while the alias `as c` (bytes
/// 14..18) sits INSIDE the second USE_PATH as its last tokens. Byte
/// map: use=0..3, ws=3..4, a=4..5, `,`=5..6, ws=6..7, std=7..10,
/// ::=10..12, b=12..13, ws=13..14, as=14..16, ws=16..17, c=17..18,
/// ;=18..19, \n=19..20.
#[test]
fn use_decl_multi_path_alias_golden() {
    let src = "use a, std::b as c;\n";
    let expected = "\
FILE@0..20
  USE_DECL@0..19
    IDENT@0..3 \"use\"
    WHITESPACE@3..4 \" \"
    USE_PATH@4..5
      IDENT@4..5 \"a\"
    COMMA@5..6 \",\"
    WHITESPACE@6..7 \" \"
    USE_PATH@7..18
      IDENT@7..10 \"std\"
      COLON_COLON@10..12 \"::\"
      IDENT@12..13 \"b\"
      WHITESPACE@13..14 \" \"
      IDENT@14..16 \"as\"
      WHITESPACE@16..17 \" \"
      IDENT@17..18 \"c\"
    SEMI@18..19 \";\"
  WHITESPACE@19..20 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// `"main() {\n  right;\n}\n"` — m=0..4 `(`=4..5 `)`=5..6 sp=6..7 `{`=7..8
/// `\n  `=8..11 right=11..16 `;`=16..17 `\n`=17..18 `}`=18..19 `\n`=19..20;
/// total 20. A bare STATEMENT/ITEM pair with no doc run and no label.
#[test]
fn function_statement_golden() {
    let src = "main() {\n  right;\n}\n";
    let expected = "\
FILE@0..20
  FUNCTION@0..19
    IDENT@0..4 \"main\"
    L_PAREN@4..5 \"(\"
    R_PAREN@5..6 \")\"
    WHITESPACE@6..7 \" \"
    L_BRACE@7..8 \"{\"
    WHITESPACE@8..11 \"\\n  \"
    STATEMENT@11..17
      ITEM@11..16
        IDENT@11..16 \"right\"
      SEMI@16..17 \";\"
    WHITESPACE@17..18 \"\\n\"
    R_BRACE@18..19 \"}\"
  WHITESPACE@19..20 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// `"? doc\nmain() {\n1: right;\n}\n"` — `?`-line=0..5 `\n`=5..6 main=6..10
/// `(`=10..11 `)`=11..12 sp=12..13 `{`=13..14 `\n`=14..15 `1`=15..16
/// `:`=16..17 sp=17..18 right=18..23 `;`=23..24 `\n`=24..25 `}`=25..26
/// `\n`=26..27; total 27. Exercises DOC_RUN (FUNCTION's retro-open wraps
/// it) and LABEL (STATEMENT's retro-open wraps it) in one golden.
#[test]
fn doc_run_label_golden() {
    let src = "? doc\nmain() {\n1: right;\n}\n";
    let expected = "\
FILE@0..27
  FUNCTION@0..26
    DOC_RUN@0..5
      DOC_LINE@0..5 \"? doc\"
    WHITESPACE@5..6 \"\\n\"
    IDENT@6..10 \"main\"
    L_PAREN@10..11 \"(\"
    R_PAREN@11..12 \")\"
    WHITESPACE@12..13 \" \"
    L_BRACE@13..14 \"{\"
    WHITESPACE@14..15 \"\\n\"
    STATEMENT@15..24
      LABEL@15..17
        NUMBER@15..16 \"1\"
        COLON@16..17 \":\"
      WHITESPACE@17..18 \" \"
      ITEM@18..23
        IDENT@18..23 \"right\"
      SEMI@23..24 \";\"
    WHITESPACE@24..25 \"\\n\"
    R_BRACE@25..26 \"}\"
  WHITESPACE@26..27 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// `"export f() {\ng() {\n1: 2: right, mark;\n}\ncheck(1, !);\n}\n"` — a
/// composed shape exercising every instrumented construct the two
/// bare goldens above don't reach: the `export` prefix inside
/// FUNCTION's extent (the reason `fn_cp`/`g_checkpoint` exists at
/// all), a nested FUNCTION (the recursive `g_start_at` path), stacked
/// LABELs (two folds retro-wrapped by one STATEMENT), a comma-group
/// ITEM pair (the separating `,` stays outside both items, at
/// STATEMENT level), and a `check` ITEM's two CHECK_ARMs. Byte map:
/// export=0..6, ws=6..7, f=7..8, `(`=8..9, `)`=9..10, ws=10..11,
/// `{`=11..12, `\n`=12..13, g=13..14, `(`=14..15, `)`=15..16,
/// ws=16..17, `{`=17..18, `\n`=18..19, `1`=19..20, `:`=20..21,
/// ws=21..22, `2`=22..23, `:`=23..24, ws=24..25, right=25..30,
/// `,`=30..31, ws=31..32, mark=32..36, `;`=36..37, `\n`=37..38,
/// `}`=38..39, `\n`=39..40, check=40..45, `(`=45..46, `1`=46..47,
/// `,`=47..48, ws=48..49, `!`=49..50, `)`=50..51, `;`=51..52,
/// `\n`=52..53, `}`=53..54, `\n`=54..55; total 55.
#[test]
fn composed_export_nested_labels_check_golden() {
    let src = "export f() {\ng() {\n1: 2: right, mark;\n}\ncheck(1, !);\n}\n";
    let expected = "\
FILE@0..55
  FUNCTION@0..54
    IDENT@0..6 \"export\"
    WHITESPACE@6..7 \" \"
    IDENT@7..8 \"f\"
    L_PAREN@8..9 \"(\"
    R_PAREN@9..10 \")\"
    WHITESPACE@10..11 \" \"
    L_BRACE@11..12 \"{\"
    WHITESPACE@12..13 \"\\n\"
    FUNCTION@13..39
      IDENT@13..14 \"g\"
      L_PAREN@14..15 \"(\"
      R_PAREN@15..16 \")\"
      WHITESPACE@16..17 \" \"
      L_BRACE@17..18 \"{\"
      WHITESPACE@18..19 \"\\n\"
      STATEMENT@19..37
        LABEL@19..21
          NUMBER@19..20 \"1\"
          COLON@20..21 \":\"
        WHITESPACE@21..22 \" \"
        LABEL@22..24
          NUMBER@22..23 \"2\"
          COLON@23..24 \":\"
        WHITESPACE@24..25 \" \"
        ITEM@25..30
          IDENT@25..30 \"right\"
        COMMA@30..31 \",\"
        WHITESPACE@31..32 \" \"
        ITEM@32..36
          IDENT@32..36 \"mark\"
        SEMI@36..37 \";\"
      WHITESPACE@37..38 \"\\n\"
      R_BRACE@38..39 \"}\"
    WHITESPACE@39..40 \"\\n\"
    STATEMENT@40..52
      ITEM@40..51
        IDENT@40..45 \"check\"
        L_PAREN@45..46 \"(\"
        CHECK_ARM@46..47
          NUMBER@46..47 \"1\"
        COMMA@47..48 \",\"
        WHITESPACE@48..49 \" \"
        CHECK_ARM@49..50
          BANG@49..50 \"!\"
        R_PAREN@50..51 \")\"
      SEMI@51..52 \";\"
    WHITESPACE@52..53 \"\\n\"
    R_BRACE@53..54 \"}\"
  WHITESPACE@54..55 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// `"! caution\nmain() { right; }\n"` — an attention-only run (no `?`
/// block at all — legal per the run grammar, "at most two contiguous
/// blocks... at least one line total"), pinning ATTENTION_LINE emission
/// and the DOC_RUN wrap through `parse_green` end to end (previously
/// only DOC_LINE had a golden). Byte map: `!`=0..1, ` caution`=1..9 (the
/// whole ATTENTION_LINE token runs to end of line per the layout's
/// doc/attention end-of-line rule, so the token is `"! caution"`@0..9),
/// `\n`=9..10, main=10..14, `(`=14..15, `)`=15..16, ws=16..17, `{`=17..18,
/// ws=18..19, right=19..24, `;`=24..25, ws=25..26, `}`=26..27, `\n`=27..28;
/// total 28.
#[test]
fn attention_only_doc_run_golden() {
    let src = "! caution\nmain() { right; }\n";
    let expected = "\
FILE@0..28
  FUNCTION@0..27
    DOC_RUN@0..9
      ATTENTION_LINE@0..9 \"! caution\"
    WHITESPACE@9..10 \"\\n\"
    IDENT@10..14 \"main\"
    L_PAREN@14..15 \"(\"
    R_PAREN@15..16 \")\"
    WHITESPACE@16..17 \" \"
    L_BRACE@17..18 \"{\"
    WHITESPACE@18..19 \" \"
    STATEMENT@19..25
      ITEM@19..24
        IDENT@19..24 \"right\"
      SEMI@24..25 \";\"
    WHITESPACE@25..26 \" \"
    R_BRACE@26..27 \"}\"
  WHITESPACE@27..28 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// `"main() {\n? d\nf() { right; }\nleft;\n}\n"` — a nested function
/// carrying its OWN doc run, exercising the body-item loop's checkpoint
/// (taken once per iteration, before either a doc run or a nested
/// definition is known — parser.rs's body-item loop): the checkpoint's
/// own `flush` lands the `\n` after the outer `{` in the OUTER
/// FUNCTION (the still-open parent at that point), then the nested
/// FUNCTION is retro-opened at that same checkpoint, so it wraps the
/// DOC_RUN that follows — same retro-open shape `doc_run_label_golden`
/// exercises at top level, one level deeper. Byte map: main=0..4,
/// `(`=4..5, `)`=5..6, ws=6..7, `{`=7..8, `\n`=8..9, `? d`=9..12 (one
/// DOC_LINE token, end-of-line rule), `\n`=12..13, f=13..14, `(`=14..15,
/// `)`=15..16, ws=16..17, `{`=17..18, ws=18..19, right=19..24, `;`=24..25,
/// ws=25..26, `}`=26..27, `\n`=27..28, left=28..32, `;`=32..33, `\n`=33..34,
/// `}`=34..35, `\n`=35..36; total 36.
#[test]
fn nested_function_own_doc_run_golden() {
    let src = "main() {\n? d\nf() { right; }\nleft;\n}\n";
    let expected = "\
FILE@0..36
  FUNCTION@0..35
    IDENT@0..4 \"main\"
    L_PAREN@4..5 \"(\"
    R_PAREN@5..6 \")\"
    WHITESPACE@6..7 \" \"
    L_BRACE@7..8 \"{\"
    WHITESPACE@8..9 \"\\n\"
    FUNCTION@9..27
      DOC_RUN@9..12
        DOC_LINE@9..12 \"? d\"
      WHITESPACE@12..13 \"\\n\"
      IDENT@13..14 \"f\"
      L_PAREN@14..15 \"(\"
      R_PAREN@15..16 \")\"
      WHITESPACE@16..17 \" \"
      L_BRACE@17..18 \"{\"
      WHITESPACE@18..19 \" \"
      STATEMENT@19..25
        ITEM@19..24
          IDENT@19..24 \"right\"
        SEMI@24..25 \";\"
      WHITESPACE@25..26 \" \"
      R_BRACE@26..27 \"}\"
    WHITESPACE@27..28 \"\\n\"
    STATEMENT@28..33
      ITEM@28..32
        IDENT@28..32 \"left\"
      SEMI@32..33 \";\"
    WHITESPACE@33..34 \"\\n\"
    R_BRACE@34..35 \"}\"
  WHITESPACE@35..36 \"\\n\"
";
    assert_eq!(dump(src), expected);
}

/// Error parity between `parse_cst` and `parse_green` over a few invalid
/// inputs: both walk the same grammar with the same sink-optional
/// `Parser`, so a divergence here would mean a green-only early return
/// slipped in somewhere. `CompileError` already derives `PartialEq`, so
/// the errors are compared directly (no derive added by this plan).
#[test]
fn error_parity_with_parse_cst() {
    use mtc_post_machine::lexer::{LexMode, lex_with};
    use mtc_post_machine::parser::parse_cst;
    // Unterminated function body; a bare `use` with no path; a missing
    // `;` after a command; a doc run with nothing bound to it.
    for src in ["main() {", "use ;", "main() { right }", "? dangling\n"] {
        let old = lex_with(src, LexMode::WithComments)
            .and_then(|t| parse_cst(&t).map(|_| ()))
            .expect_err("sample must be invalid .pmc");
        let new = parse_green(src)
            .map(|_| ())
            .expect_err("sample must be invalid .pmc");
        assert_eq!(old, new, "error parity for {src:?}");
    }
}

/// Every .pmc file in the crate (test programs, lint fixtures, the
/// embedded stdlib) — the corpus for the lossless law. Walked
/// explicitly so a future fixture is picked up automatically.
fn corpus() -> Vec<(std::path::PathBuf, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "pmc") {
                let text = std::fs::read_to_string(&path).expect("readable .pmc");
                files.push((path, text));
            }
        }
    }
    // 10 files at the time of writing: 7 golden programs
    // (tests/golden/{ex000001,ex000002,sum,sum2,test1,ty,ty2}.pmc), 1
    // lint fixture (tests/lint/unused_labels.pmc), the embedded stdlib
    // (src/stdlib/std.pmc), and the rich-shape syntax fixture
    // (tests/syntax/rich.pmc). The floor below is `>= 10` rather than
    // `== 10` so a future fixture doesn't need this comment touched, only
    // the count restated if it drifts meaningfully.
    assert!(
        files.len() >= 10,
        "corpus unexpectedly small: {} files — did the walk break?",
        files.len()
    );
    files
}

/// The lossless law over the whole corpus: every real `.pmc` file in the
/// crate round-trips through `parse_green` byte-for-byte, not just the
/// hand-picked golden fixtures above.
#[test]
fn corpus_lossless_law() {
    for (path, source) in corpus() {
        let tree = parse_green(&source)
            .unwrap_or_else(|e| panic!("{}: green parse failed: {e:?}", path.display()));
        let root = SyntaxNode::new_root(tree);
        assert_eq!(root.text(), source, "{}: text law", path.display());
    }
}

/// Acceptance parity between `parse_cst` and `parse_green` over the same
/// corpus: every file the C1 pipeline accepts, the green parser accepts
/// too, and vice versa. All corpus files are valid `.pmc`, so this is
/// expected to be `old_ok == new_ok == true` throughout — the assertion
/// still holds the general shape so a future invalid fixture is caught
/// the same way.
#[test]
fn corpus_acceptance_parity() {
    use mtc_post_machine::lexer::{LexMode, lex_with};
    use mtc_post_machine::parser::parse_cst;
    for (path, source) in corpus() {
        let old_ok = lex_with(&source, LexMode::WithComments)
            .and_then(|t| parse_cst(&t).map(|_| ()))
            .is_ok();
        let new_ok = parse_green(&source).is_ok();
        assert_eq!(old_ok, new_ok, "{}: acceptance parity", path.display());
    }
}
