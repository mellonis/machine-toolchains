//! Property tests for the core syntax framework, over a fake kind
//! space — core keeps zero language knowledge (crate contract; the
//! VM core proves the same with its fake arch).

use std::rc::Rc;

use mtc_core::syntax::{
    GreenElement, GreenNode, GreenToken, LineIndex, SyntaxElement, SyntaxKind, SyntaxNode,
};
use proptest::prelude::*;

const NODE: SyntaxKind = SyntaxKind(100);
const TOKEN: SyntaxKind = SyntaxKind(101);

/// Token text: short runs incl. spaces, newlines, and a multi-byte
/// char, so offsets cross UTF-8 boundaries and line breaks.
fn arb_token_text() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[abλ \\n]{0,6}").expect("valid regex")
}

fn arb_green() -> impl Strategy<Value = Rc<GreenNode>> {
    let leaf = proptest::collection::vec(arb_token_text(), 0..5).prop_map(|texts| {
        GreenNode::new(
            NODE,
            texts
                .into_iter()
                .map(|t| GreenElement::Token(GreenToken::new(TOKEN, t)))
                .collect(),
        )
    });
    leaf.prop_recursive(3, 24, 4, |inner| {
        proptest::collection::vec(
            prop_oneof![
                inner.prop_map(GreenElement::Node),
                arb_token_text().prop_map(|t| GreenElement::Token(GreenToken::new(TOKEN, t))),
            ],
            0..4,
        )
        .prop_map(|children| GreenNode::new(NODE, children))
    })
}

/// Recursively assert that children's ranges tile the parent's range
/// exactly, in document order, at every level.
fn assert_tiling(node: &SyntaxNode) {
    let mut cursor = node.text_range().start;
    for e in node.children_with_tokens() {
        assert_eq!(e.text_range().start, cursor, "gap or overlap in tiling");
        cursor = e.text_range().end;
        if let SyntaxElement::Node(n) = e {
            assert_eq!(n.parent().expect("child has parent"), *node);
            assert_tiling(&n);
        }
    }
    assert_eq!(
        cursor,
        node.text_range().end,
        "children under-cover the node"
    );
}

proptest! {
    /// The lossless law's structural half: cached lengths always equal
    /// real text lengths.
    #[test]
    fn text_len_is_the_text_byte_length(root in arb_green()) {
        prop_assert_eq!(root.text_len() as usize, root.text().len());
    }

    /// Red ranges tile parents exactly and parent links invert
    /// children, at every depth.
    #[test]
    fn red_ranges_tile_and_parents_invert(root in arb_green()) {
        assert_tiling(&SyntaxNode::new_root(root));
    }

    /// LineIndex agrees with a naive char-by-char scan at every char
    /// boundary plus the end-of-text offset.
    #[test]
    fn line_index_matches_a_naive_scan(text in "[abλ\\n]{0,40}") {
        let idx = LineIndex::new(&text);
        let mut line = 1u32;
        let mut col = 1u32;
        for (byte_off, ch) in text.char_indices() {
            prop_assert_eq!(idx.line_col(byte_off as u32), (line, col));
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        prop_assert_eq!(idx.line_col(text.len() as u32), (line, col));
    }
}
