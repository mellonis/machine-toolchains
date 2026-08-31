//! Property tests for the core syntax framework, over a fake kind
//! space — core keeps zero language knowledge (crate contract; the
//! VM core proves the same with its fake arch).

use std::rc::Rc;

use mtc_core::syntax::{
    GreenElement, GreenNode, GreenToken, SyntaxElement, SyntaxKind, SyntaxNode, TextLineIndex,
    TreeBuilder,
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

/// One item of a builder script: a leaf token or a nested node
/// carrying its own items. Balanced by construction — every `Node` is
/// emitted as a matched `start_node`/`finish_node` pair, so a script
/// can always drive a `TreeBuilder` without an unbalanced build.
#[derive(Debug, Clone)]
enum ScriptItem {
    Token(String),
    Node(Vec<ScriptItem>),
}

fn arb_script_item() -> impl Strategy<Value = ScriptItem> {
    let leaf = arb_token_text().prop_map(ScriptItem::Token);
    leaf.prop_recursive(3, 24, 4, |inner| {
        proptest::collection::vec(inner, 0..4).prop_map(ScriptItem::Node)
    })
}

/// A random balanced builder script: the top-level list of items
/// (tokens or nested nodes) emitted as the root node's children.
fn arb_script() -> impl Strategy<Value = Vec<ScriptItem>> {
    proptest::collection::vec(arb_script_item(), 0..6)
}

/// The in-order concatenation of every token's text in the script —
/// the text a faithful `TreeBuilder` emission must reproduce.
fn script_text(items: &[ScriptItem]) -> String {
    let mut out = String::new();
    fn walk(item: &ScriptItem, out: &mut String) {
        match item {
            ScriptItem::Token(t) => out.push_str(t),
            ScriptItem::Node(children) => children.iter().for_each(|c| walk(c, out)),
        }
    }
    items.iter().for_each(|item| walk(item, &mut out));
    out
}

/// Drive a `TreeBuilder` through a script, one `token`/`start_node`/
/// `finish_node` call per item.
fn emit_script(b: &mut TreeBuilder, items: &[ScriptItem]) {
    for item in items {
        match item {
            ScriptItem::Token(t) => b.token(TOKEN, t),
            ScriptItem::Node(children) => {
                b.start_node(NODE);
                emit_script(b, children);
                b.finish_node();
            }
        }
    }
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

    /// TextLineIndex agrees with a naive char-by-char scan at every
    /// char boundary plus the end-of-text offset.
    #[test]
    fn line_index_matches_a_naive_scan(text in "[abλ\\n]{0,40}") {
        let idx = TextLineIndex::new(&text);
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

    /// Random balanced builder scripts reproduce their token text exactly —
    /// the lossless law's content half, over the builder path.
    #[test]
    fn builder_round_trips_token_text(script in arb_script()) {
        let expected = script_text(&script);
        let mut b = TreeBuilder::new();
        b.start_node(NODE);
        emit_script(&mut b, &script);
        b.finish_node();
        let root = b.finish();
        prop_assert_eq!(root.text(), expected);
    }
}

/// `checkpoint`/`start_node_at` retroactively wrap an already-emitted
/// prefix; the emitted text must be unaffected by the wrapping, only
/// the tree shape changes.
#[test]
fn builder_checkpoint_wraps_a_prefix_without_changing_text() {
    let mut b = TreeBuilder::new();
    b.start_node(NODE);
    let cp = b.checkpoint();
    b.token(TOKEN, "a");
    b.token(TOKEN, "b");
    b.start_node_at(cp, NODE);
    b.token(TOKEN, "c");
    b.finish_node(); // the retro-wrapped inner NODE ("a", "b", "c")
    b.finish_node(); // the outer NODE
    let root = b.finish();
    assert_eq!(root.text(), "abc");
}
