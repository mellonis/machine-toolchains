//! Comment and blank-line classification, re-derived from green trivia.
//!
//! The C1 CST stored these as fields the parser filled in — `blank_before`,
//! `trailing`, `leading`, `open_trailing`, `close_trailing`, `label_break`.
//! The green tree stores nothing derived:
//! trivia are ordinary tokens sitting between a node's children, so every
//! one of those classifications is a local query over `children_with_tokens`
//! or a sibling walk. Same decisions, new source of truth
//! (`docs/pmt/fmt.md` (comments)).
//!
//! Two shapes drive most of this file. A comment after a closing `}` is the
//! next sibling token in the PARENT's stream, not a child of the node it
//! follows — so what the CST called `close_trailing` and what it called a
//! statement's `trailing` are one function here. And a leading comment run
//! is a run of sibling tokens rather than part of the item, so the gap
//! before the whole unit has to be found by walking back over the run.

use crate::syntax::PmcKind;
use mtc_core::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

pub(crate) fn is_comment(k: SyntaxKind) -> bool {
    k == PmcKind::LineComment.into() || k == PmcKind::BlockComment.into()
}

pub(crate) fn is_ws(k: SyntaxKind) -> bool {
    k == PmcKind::Whitespace.into()
}

/// The tokens immediately before `node`, newest-first, stopping at the
/// first sibling that is a node.
fn preceding_tokens(node: &SyntaxNode) -> Vec<SyntaxToken> {
    let mut out = Vec::new();
    let mut cur = node.prev_sibling_or_token();
    while let Some(SyntaxElement::Token(t)) = cur {
        cur = t.prev_sibling_or_token();
        out.push(t);
    }
    out
}

/// The comment run bound to `node` as its leading block, in source order.
/// A blank line ends the run: comments above the gap belong to whatever
/// came before, exactly as the CST's attachment pass decided.
pub(crate) fn leading_comments(node: &SyntaxNode) -> Vec<SyntaxToken> {
    let mut out = Vec::new();
    for t in preceding_tokens(node) {
        if is_comment(t.kind()) {
            out.push(t);
        } else if is_ws(t.kind()) {
            if t.text().matches('\n').count() >= 2 {
                break;
            }
        } else {
            break;
        }
    }
    out.reverse();
    out
}

/// Whether the author left an empty line before the whole unit — the item
/// together with its leading comment run. Keyed off the unit's start, never
/// off `node.text_range()`: a FUNCTION node already retro-wraps its doc run,
/// whereas a NAMESPACE node never carries a doc run (any comment run before
/// `namespace` is a parse error: `DanglingDocRun`).
pub(crate) fn blank_before_unit(node: &SyntaxNode) -> bool {
    let lead = leading_comments(node);
    let before = match lead.first() {
        Some(first) => first.prev_sibling_or_token(),
        None => node.prev_sibling_or_token(),
    };
    match before {
        Some(SyntaxElement::Token(t)) => is_ws(t.kind()) && t.text().matches('\n').count() >= 2,
        _ => false,
    }
}

/// A comment riding the same source line as `node`'s last token — what the
/// CST recorded as `trailing` on a statement and as `close_trailing` on a
/// namespace or function.
pub(crate) fn trailing_comment(node: &SyntaxNode) -> Option<SyntaxToken> {
    let mut cur = node.next_sibling_or_token();
    while let Some(SyntaxElement::Token(t)) = cur {
        if is_ws(t.kind()) {
            if t.text().contains('\n') {
                return None;
            }
            cur = t.next_sibling_or_token();
        } else if is_comment(t.kind()) {
            return Some(t);
        } else {
            return None;
        }
    }
    None
}

/// Comments after an opening `{` still on its line.
pub(crate) fn open_trailing(open: &SyntaxToken) -> Vec<SyntaxToken> {
    let mut out = Vec::new();
    let mut cur = open.next_sibling_or_token();
    while let Some(SyntaxElement::Token(t)) = cur {
        if is_ws(t.kind()) {
            if t.text().contains('\n') {
                break;
            }
        } else if is_comment(t.kind()) {
            out.push(t.clone());
        } else {
            break;
        }
        cur = t.next_sibling_or_token();
    }
    out
}

/// Whether the author broke the line between a statement's last label and
/// its first item (`docs/pmt/fmt.md` (own-line labels)). The printer
/// preserves this choice and never infers or overrides it.
#[allow(dead_code)]
pub(crate) fn label_break(stmt: &SyntaxNode) -> bool {
    let mut seen_label = false;
    for e in stmt.children_with_tokens() {
        match e {
            SyntaxElement::Node(n) if n.kind() == PmcKind::Label.into() => seen_label = true,
            SyntaxElement::Node(_) => return false,
            SyntaxElement::Token(t) if seen_label && is_ws(t.kind()) => {
                if t.text().contains('\n') {
                    return true;
                }
            }
            SyntaxElement::Token(_) => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_green;
    use crate::syntax::PmcKind;
    use mtc_core::syntax::SyntaxNode;

    fn f(src: &str) -> SyntaxNode {
        SyntaxNode::new_root(parse_green(src).expect("parses"))
    }

    fn functions(root: &SyntaxNode) -> Vec<SyntaxNode> {
        root.children()
            .filter(|c| c.kind() == PmcKind::Function.into())
            .collect()
    }

    fn statements(fun: &SyntaxNode) -> Vec<SyntaxNode> {
        fun.children()
            .filter(|c| c.kind() == PmcKind::Statement.into())
            .collect()
    }

    /// The gap C1 recorded as `blank_before` sits before the item's
    /// leading comment run, not before the item node — the run is a
    /// sequence of sibling tokens in green, not part of the item.
    #[test]
    fn blank_before_unit_looks_past_the_leading_run() {
        let r = f("main() {\n 1: left;\n}\n\n// lead\nother() {\n 1: left;\n}\n");
        let fns = functions(&r);
        assert_eq!(fns.len(), 2);
        assert!(
            !blank_before_unit(&fns[0]),
            "nothing precedes the first item"
        );
        assert_eq!(leading_comments(&fns[1]).len(), 1);
        assert!(blank_before_unit(&fns[1]), "the gap is before the comment");
    }

    /// A blank line inside a comment run cuts it: only the part below
    /// the gap binds to the item.
    #[test]
    fn a_blank_line_cuts_the_leading_run() {
        let r = f("main() {\n 1: left;\n}\n\n// far\n\n// near\nother() {\n 1: left;\n}\n");
        let fns = functions(&r);
        let lead = leading_comments(&fns[1]);
        assert_eq!(lead.len(), 1, "only `// near` binds");
        assert_eq!(lead[0].text(), "// near");
    }

    /// A trailing comment rides the same source line; a newline ends it.
    #[test]
    fn trailing_comment_stops_at_the_line_end() {
        let r = f("main() { // open\n 1: left; // ride\n // not mine\n 2: left;\n}\n");
        let st = statements(&functions(&r)[0]);
        assert_eq!(
            trailing_comment(&st[0]).map(|t| t.text().to_string()),
            Some("// ride".to_string())
        );
        assert_eq!(trailing_comment(&st[1]), None);
    }

    /// `open_trailing` reads forward off the brace token itself.
    #[test]
    fn open_trailing_reads_off_the_brace() {
        let r = f("main() { // open\n 1: left;\n}\n");
        let brace = functions(&r)[0]
            .children_with_tokens()
            .find_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == PmcKind::LBrace.into() => Some(t),
                _ => None,
            })
            .expect("the body brace");
        let open = open_trailing(&brace);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].text(), "// open");
    }

    /// What C1 called `close_trailing` is `trailing_comment` applied to
    /// the closed node: the comment lives in the PARENT's child stream.
    #[test]
    fn close_trailing_is_trailing_comment_on_the_node() {
        let r = f("main() {\n 1: left;\n} // bye\n");
        let fun = &functions(&r)[0];
        assert_eq!(
            trailing_comment(fun).map(|t| t.text().to_string()),
            Some("// bye".to_string())
        );
    }

    /// `label_break` is a newline between the last label and the first item.
    #[test]
    fn label_break_sees_the_own_line_label() {
        let r = f("main() {\n 1:\n    left;\n 2: left;\n}\n");
        let st = statements(&functions(&r)[0]);
        assert!(label_break(&st[0]));
        assert!(!label_break(&st[1]));
    }

    /// Mutation test: `blank_before_unit`'s `>= 2` threshold → `>= 1`.
    /// With just one newline between items, there is no blank line, so
    /// blank_before_unit must return false. If the threshold is lowered to `>= 1`,
    /// this would incorrectly return true.
    #[test]
    fn blank_before_unit_requires_two_newlines_not_one() {
        let r = f("main() {\n 1: left;\n}\nother() {\n 1: left;\n}\n");
        let fns = functions(&r);
        assert_eq!(fns.len(), 2);
        assert!(
            !blank_before_unit(&fns[1]),
            "single newline is not a blank line"
        );
    }

    /// Mutation test: `leading_comments`'s `.reverse()` dropped.
    /// Comments are collected newest-first from preceding_tokens, so they must be
    /// reversed to restore source order. Without reverse, they'd appear in reverse order.
    #[test]
    fn leading_comments_returns_source_order() {
        let r = f("main() {\n 1: left;\n}\n// first\n// second\nother() {\n 1: left;\n}\n");
        let fns = functions(&r);
        let lead = leading_comments(&fns[1]);
        assert_eq!(lead.len(), 2);
        assert_eq!(lead[0].text(), "// first");
        assert_eq!(lead[1].text(), "// second");
    }

    /// Mutation test: `trailing_comment`'s newline early-return removed.
    /// When we encounter a newline token before finding a comment, we must return None
    /// immediately. Without this check, we'd continue and find comments on the next line.
    #[test]
    fn trailing_comment_returns_none_before_newline() {
        let r = f("main() {\n 1: left;\n // comment on next line\n 2: right;\n}\n");
        let st = statements(&functions(&r)[0]);
        assert_eq!(
            trailing_comment(&st[0]),
            None,
            "comment on next line is not a trailing comment"
        );
    }

    /// Mutation test: `open_trailing`'s newline `break` removed.
    /// When we encounter a newline token while walking after an opening brace,
    /// we must stop immediately. Without the break, we'd continue and find comments
    /// on the next line.
    #[test]
    fn open_trailing_stops_at_newline() {
        let r = f("main() {\n // comment on next line\n 1: left;\n}\n");
        let brace = functions(&r)[0]
            .children_with_tokens()
            .find_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == PmcKind::LBrace.into() => Some(t),
                _ => None,
            })
            .expect("the body brace");
        let open = open_trailing(&brace);
        assert!(open.is_empty(), "comment on next line is not open_trailing");
    }

    /// Mutation test: `label_break`'s node-vs-token arm removed.
    /// When we see a non-Label node (the first item), we must return false immediately.
    /// This marks the end of the label sequence. Without this check, we'd continue and
    /// find newlines between items, incorrectly returning true.
    #[test]
    fn label_break_detects_inline_label() {
        let r = f("main() {\n 1: left,\n    right;\n}\n");
        let st = statements(&functions(&r)[0]);
        assert!(
            !label_break(&st[0]),
            "label inline with first item means no break"
        );
    }

    /// Mutation test: `is_comment`'s `BlockComment` arm dropped.
    /// Block comments like /* … */ must be recognized as comments, not skipped.
    #[test]
    fn is_comment_recognizes_block_comments() {
        let r = f("main() {\n 1: left;\n}\n/* block comment */\nother() {\n 1: left;\n}\n");
        let fns = functions(&r);
        let lead = leading_comments(&fns[1]);
        assert_eq!(
            lead.len(),
            1,
            "block comment should bind as leading comment"
        );
        assert!(
            lead[0].text().starts_with("/*"),
            "the comment is a block comment"
        );
    }
}
