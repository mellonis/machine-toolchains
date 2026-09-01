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
use mtc_core::asm::cst::{AsmCst, AsmItem, AsmItemKind, parse_asm_cst_with, parse_asm_green};
use mtc_core::asm::views::{ItemView, LocatedItem, ReptView, RootView, locate_items};
use mtc_core::syntax::{AstNode, SyntaxNode, TextLineIndex};
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
    holds_location_laws(src, &cst, &root);
}

/// The flattened item walk the location pairing promises to follow: a
/// `.rept` header followed by its body items, spliced in place.
fn flat_items(items: &[AsmItem], out: &mut Vec<*const AsmItem>) {
    for item in items {
        out.push(item as *const AsmItem);
        if let AsmItemKind::Rept(r) = &item.kind {
            flat_items(&r.body, out);
        }
    }
}

/// Laws (c) and (d) of phase 2: every flattened CST item is located, in
/// order, and the tree's positions agree with the CST's own lexer-built
/// spans — line AND column at the start, and for the multi-line shapes
/// (`.rept` blocks, continued lists) at the end too.
#[track_caller]
fn holds_location_laws(src: &str, cst: &AsmCst, root: &SyntaxNode) {
    let located: Vec<LocatedItem<'_>> = locate_items(cst, root);
    let mut expected = Vec::new();
    flat_items(&cst.items, &mut expected);
    assert_eq!(
        located.len(),
        expected.len(),
        "every flattened item is located"
    );
    let index = TextLineIndex::new(src);
    for (l, want) in located.iter().zip(&expected) {
        assert!(
            std::ptr::eq(l.item, *want),
            "located items follow the flattened walk order"
        );
        let (line, col) = index.line_col(l.range.start);
        let text = &src[l.range.start as usize..l.range.end as usize];
        match &l.item.kind {
            AsmItemKind::Comment(c) => {
                assert_eq!(text, c.text, "a comment's range is its own token");
                assert_eq!(col, c.col, "a comment's column");
            }
            AsmItemKind::Rept(r) => {
                assert_eq!((line, col), (r.span.start.line, r.span.start.col));
                let (el, ec) = index.line_col(l.range.end);
                assert_eq!((el, ec), (r.endr_span.end.line, r.endr_span.end.col));
            }
            AsmItemKind::Func(f) => assert_eq!((line, col), (f.span.start.line, f.span.start.col)),
            AsmItemKind::Line(x) => assert_eq!((line, col), (x.span.start.line, x.span.start.col)),
            AsmItemKind::Raw(x) => assert_eq!((line, col), (x.span.start.line, x.span.start.col)),
            AsmItemKind::Section(x) => {
                assert_eq!((line, col), (x.span.start.line, x.span.start.col))
            }
            AsmItemKind::TableDirective(x) => {
                assert_eq!((line, col), (x.span.start.line, x.span.start.col))
            }
            AsmItemKind::RoutineDirective(x) => {
                assert_eq!((line, col), (x.span.start.line, x.span.start.col))
            }
            AsmItemKind::Volatile(x) => {
                assert_eq!((line, col), (x.span.start.line, x.span.start.col))
            }
            AsmItemKind::FrameDirective(x) => {
                assert_eq!((line, col), (x.span().start.line, x.span().start.col))
            }
        }
        if let Some(cont) = &l.item.continuation {
            let (el, _) = index.line_col(l.range.end);
            assert_eq!(el, cont.last().expect("two or more").line_no);
        }
    }
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
        // An indented comment on the FIRST line: own-line, yet no
        // newline precedes its indentation (the noise sweep's find).
        "\t;",
        "  ; indented first\n\t; indented second\n",
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

/// Line fragments that reach every structure the location pairing has
/// to walk: block openers and closers (matched, stray, nested), own-line
/// comments at several indents (the first line included), trailing
/// comments with and without a space, continued lists, blanks, raw
/// lines, and CRLF endings.
fn asm_fragment() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        ".rept v, 0, 1",
        ".rept w, 1, 2 ; hdr",
        ".endr",
        ".endr ; after",
        "; c",
        "  ; c",
        "\t; c",
        "L0: nop",
        "L0: L1: nop ; t",
        "        jmp L0;x",
        ".func f",
        ".func g local ; t",
        ".targets L0,",
        ".exits a, b,",
        "        L1",
        ".section code",
        ".row [1, *, -]",
        ".volatile",
        "F: .frame tapes=(0, 1)",
        ".map 0, rmap=(1 -> 2)",
        "",
        "\t",
        "@@ ; raw with comment",
        "  0004: 21 33",
        "\r",
    ])
}

proptest! {
    /// The parse is total, so ANY input must build a lossless tree —
    /// including carriage returns, quotes, unicode, and half-formed
    /// directives — under both cap profiles. (`.` never matches a
    /// newline, so this is the single-line sweep; the next two cover
    /// multi-line shapes.)
    #[test]
    fn arbitrary_input_round_trips(src in ".{0,200}") {
        holds_both_laws(&src, AsmCaps::default());
        holds_both_laws(&src, all_caps());
    }

    /// Arbitrary MULTI-line noise — `(?s)` lets `.` match newlines.
    #[test]
    fn arbitrary_multiline_input_round_trips(src in "(?s).{0,200}") {
        holds_both_laws(&src, AsmCaps::default());
        holds_both_laws(&src, all_caps());
    }

    /// Assembly-shaped multi-line documents: random sequences of the
    /// fragments above, so blocks, comments in every placement, and
    /// continued lists meet in every order — the pairing's real input.
    #[test]
    fn fragment_documents_round_trip(lines in prop::collection::vec(asm_fragment(), 0..14)) {
        let src = lines.join("\n");
        holds_both_laws(&src, AsmCaps::default());
        holds_both_laws(&src, all_caps());
    }
}

/// A `.rept` block nests: the REPT node holds its header tokens, one
/// child node per body item (a comment-only body line stays trivia),
/// and the closing `.endr` — so a body item has a node of its own to
/// be located by. The `.endr`'s trailing comment is root trivia, like
/// every other item's.
#[test]
fn a_rept_block_nests_its_body_items() {
    use mtc_core::asm::kinds::{AsmKind, kind_name};
    let src =
        ".rept v, 0, 1\n        wr [{v}] ; t\n; own\n        mov [>]\n.endr ; done\n        stp\n";
    holds_both_laws(src, all_caps());
    let (_, green) = parse_asm_green(src, all_caps());
    let root = SyntaxNode::new_root(green);
    let root_kinds: Vec<&str> = root.children().map(|n| kind_name(n.kind())).collect();
    assert_eq!(root_kinds, vec!["REPT", "LINE"]);
    let rept = root.children().next().expect("REPT");
    assert_eq!(rept.kind(), AsmKind::Rept.into());
    let body_kinds: Vec<&str> = rept.children().map(|n| kind_name(n.kind())).collect();
    assert_eq!(body_kinds, vec!["LINE", "LINE"], "one node per body item");
    let text = rept.text();
    assert!(text.starts_with(".rept v, 0, 1"), "{text:?}");
    assert!(text.ends_with(".endr"), "{text:?}");
}

/// The explicit fixture for laws (c)/(d): every shape whose position
/// the services read, with the comment placements that used to need a
/// cursor walk — a leading comment, a trailing one, an own-line one
/// inside a `.rept` body, one after the `.endr`, and a continued list.
#[test]
fn located_items_agree_with_the_cst_spans() {
    let src = "\
; leading
.func main ; t
L0: L1: nop ; trailing
; own line
.rept v, 0, 1
        wr [{v}]
; body comment
.endr ; after
.targets L0,
        L1
.volatile
";
    let (cst, green) = parse_asm_green(src, all_caps());
    let root = SyntaxNode::new_root(green);
    holds_location_laws(src, &cst, &root);
    let located = locate_items(&cst, &root);
    let index = TextLineIndex::new(src);
    let lines: Vec<u32> = located
        .iter()
        .map(|l| index.line_col(l.range.start).0)
        .collect();
    // comment, func, line, comment, rept, wr, body comment, targets, volatile
    assert_eq!(lines, vec![1, 2, 3, 4, 5, 6, 7, 9, 11]);
}

/// The typed views name the shape the tree carries: the root's items
/// and a block's body, kind by kind.
#[test]
fn views_walk_the_item_nodes() {
    let src = ".func f
.rept v, 0, 1
        wr [{v}] ; t
; own
        mov [>]
.endr
        stp
";
    let (_, green) = parse_asm_green(src, all_caps());
    let root = RootView::cast(SyntaxNode::new_root(green)).expect("ROOT");
    let items: Vec<ItemView> = root.items().collect();
    assert!(matches!(items[0], ItemView::Func(_)));
    assert!(matches!(items[2], ItemView::Line(_)));
    let ItemView::Rept(rept) = &items[1] else {
        panic!("the block is a REPT view");
    };
    let rept: &ReptView = rept;
    let body: Vec<ItemView> = rept.body().collect();
    assert_eq!(body.len(), 2);
    assert!(body.iter().all(|i| matches!(i, ItemView::Line(_))));
    assert_eq!(body[1].syntax().text(), "mov [>]");
}
