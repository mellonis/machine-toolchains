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
