//! The assembly green tree (docs/core.md (syntax trees)): phase 1 of
//! the asm port onto the syntax framework. Two laws:
//!
//! (a) **Lossless** — for ANY input (the asm parse is total), the green
//!     tree's `text()` equals the source byte-for-byte: pinned over the
//!     flagship `.tma`, hand-written `.pma`/`.tma` fixtures covering
//!     every item shape, and a proptest noise sweep.
//! (b) **Same CST** — the pair entry's `AsmCst` equals
//!     `parse_asm_cst_with`'s on the same input (one shaping walk by
//!     construction; this pins the construction stays shared).

use mtc_core::asm::AsmCaps;
use mtc_core::asm::cst::{parse_asm_cst_with, parse_asm_green};
use mtc_core::syntax::SyntaxNode;
use proptest::prelude::*;

fn all_caps() -> AsmCaps {
    AsmCaps {
        tables: true,
        rept: true,
        vectors: true,
        volatile: true,
    }
}

#[track_caller]
fn holds_both_laws(src: &str, caps: AsmCaps) {
    let (cst, green) = parse_asm_green(src, caps);
    let root = SyntaxNode::new_root(green);
    assert_eq!(root.text(), src, "the green tree lost source text");
    assert_eq!(
        cst,
        parse_asm_cst_with(src, caps),
        "the pair entry's CST diverged from the plain entry's"
    );
}

#[test]
fn the_flagship_utm_round_trips() {
    let src = include_str!("../../../docs/examples/brainfuck-utm/brainfuck-utm-handwritten.tma");
    holds_both_laws(src, all_caps());
}

#[test]
fn pma_shapes_round_trip_under_default_caps() {
    for src in [
        "",
        "\n\n\n",
        "; a leading comment\n.func main\n        jmp L1 ; note\nL1:     stp\n",
        ".func f local\nL0: L1: nop\n\n\n        hlt\n",
        "not assembly at all <listing>\n  0004: 21 33\n",
        "; comment only\n",
        ".func f\n        nop ; trailing\n; own line\n        stp",
    ] {
        holds_both_laws(src, AsmCaps::default());
    }
}

#[test]
fn tma_shapes_round_trip_under_full_caps() {
    for src in [
        ".section tables\n.row [1, 2, 3]\n.targets L1, L2,\n        L3\n",
        ".rept v, 0, 7\n        wr [{v}]\n.endr ; done\n",
        ".routine r, tapes=2, alpha=(3, 5)\n",
        ".volatile\n",
        ".frame F\n.map 1 -> 2, 3 => 4\n.exits L1, L2\n",
        ".rept v, 0, 1\n; unterminated block degrades\n",
    ] {
        holds_both_laws(src, all_caps());
    }
}

/// Structure, not just bytes: the tree carries one node per CST item,
/// kind-mapped, with comment-only items as pure trivia — the check a
/// broken emitter (tokens without nodes) cannot pass.
#[test]
fn the_tree_mirrors_the_cst_item_structure() {
    use mtc_core::asm::kinds::{AsmKind, kind_name};
    let src = "; header comment\n.func main\n        jmp L1 ; note\nL1:     stp\n.volatile\n";
    let (cst, green) = parse_asm_green(src, all_caps());
    let root = SyntaxNode::new_root(green);
    let node_kinds: Vec<&str> = root.children().map(|n| kind_name(n.kind())).collect();
    assert_eq!(
        node_kinds,
        vec!["FUNC", "LINE", "LINE", "VOLATILE"],
        "one node per non-comment CST item, kind-mapped"
    );
    // The CST sees five items (the comment included); the tree holds the
    // comment as trivia, not a node.
    assert_eq!(cst.items.len(), 5);
    // Each node's text is its item's own lines — the FUNC node carries
    // exactly the directive line (sans the leading comment, which is
    // root-level trivia).
    let func = root.children().next().expect("FUNC");
    assert_eq!(func.kind(), AsmKind::Func.into());
    assert_eq!(func.text(), ".func main");
}

proptest! {
    /// The parse is total, so ANY input must build a lossless tree —
    /// including carriage returns, quotes, unicode, and half-formed
    /// directives — under both cap profiles.
    #[test]
    fn arbitrary_input_round_trips(src in ".{0,200}") {
        holds_both_laws(&src, AsmCaps::default());
        holds_both_laws(&src, all_caps());
    }
}
