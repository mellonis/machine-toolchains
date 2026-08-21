//! Shared CST-walk primitives (docs/lsp.md (go-to-definition), docs/lsp.md
//! (completions), docs/lsp.md (semantic tokens)): the position-to-
//! enclosing-function walk, the function-scoped label lookups, and
//! half-open span containment. `navigate.rs`, `complete.rs`, and
//! `tokens.rs` each need the same shapes for different ends — a
//! match-at-position query, a full-chain assembly, an emit-everything
//! walk — so the ENUMERATION lives here once and every caller supplies
//! its own terminal behavior over the result.

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::syntax::{AstNode, TextLineIndex};

use crate::parser::{CheckArm, Item, Label, Successor};
use crate::syntax::{FileView, FunctionView, TopView, extract_statement};

/// Half-open span containment, 1-based. `Pos`'s derived `Ord` compares
/// `line` then `col` (its field order) — exactly a lexicographic
/// position comparison — so this is correct for a multi-line span with
/// no special-casing.
pub(super) fn span_contains(span: Span, pos: Pos) -> bool {
    pos >= span.start && pos < span.end
}

/// The enclosing function CHAIN at `offset`, outermost first: the
/// top-level function containing `offset`, then its nested descendant
/// containing `offset`, as deep as `offset` still lands inside one.
/// Namespace blocks are walked but never themselves added to the chain —
/// only a function's own extent does that. Empty when `offset` isn't
/// inside any function at all. A caller that only wants the innermost
/// enclosing function takes `.pop()`; a caller that needs every
/// enclosing level (qualified-name reconstruction, hoisted nested defs)
/// walks the whole `Vec`.
///
/// Offsets, not `Pos`: the green tree indexes by byte range
/// (docs/core.md (syntax trees)), so a request's position is converted
/// once by `TextLineIndex::offset` and every containment test below is a
/// range comparison.
pub(super) fn enclosing_function_chain(file: &FileView, offset: u32) -> Vec<FunctionView> {
    fn descend(items: impl Iterator<Item = TopView>, offset: u32) -> Vec<FunctionView> {
        for item in items {
            match item {
                TopView::Namespace(ns) => {
                    if ns.syntax().text_range().contains(offset) {
                        let chain = descend(ns.items(), offset);
                        if !chain.is_empty() {
                            return chain;
                        }
                    }
                }
                TopView::Function(f) => {
                    if f.syntax().text_range().contains(offset) {
                        let mut chain = vec![f.clone()];
                        push_deepest_nested(&f, offset, &mut chain);
                        return chain;
                    }
                }
                TopView::Use(_) => {}
            }
        }
        Vec::new()
    }
    descend(file.items(), offset)
}

/// Descends into `f`'s own nested definitions as long as `offset` stays
/// inside one, pushing each one reached onto `chain`.
fn push_deepest_nested(f: &FunctionView, offset: u32, chain: &mut Vec<FunctionView>) {
    for nested in f.nested() {
        if nested.syntax().text_range().contains(offset) {
            chain.push(nested.clone());
            push_deepest_nested(&nested, offset, chain);
            return;
        }
    }
}

/// Every label `function` declares in its OWN statements, in source
/// order — function-scoped, never descending into nested children (a
/// nested function's labels are a separate scope, reached only by
/// walking to that nested function's own chain entry first, via
/// [`enclosing_function_chain`]).
///
/// Owned `Label`s, not borrowed: each is built through
/// `crate::syntax::extract_statement`, the parser's own production, so a
/// label's `value` and `span` are the exact ones the compiler sees —
/// never a re-derivation over token text (a leading-zero label like
/// `007` would not survive one).
pub(super) fn function_labels(function: &FunctionView, index: &TextLineIndex) -> Vec<Label> {
    function
        .statements()
        .flat_map(|stmt| extract_statement(&stmt, index).labels)
        .collect()
}

/// Every label-reference site one comma-group `Item` carries (the
/// reference shapes: `Goto`'s own target, a `Check` arm's label — only
/// when that arm is `CheckArm::Label` — and a builtin/call's labeled
/// successor — only when `Successor::Label`). At most two references per
/// item (`Check`'s marked/blank arms), returned as a fixed-size array so
/// callers iterate with `.into_iter().flatten()` and no allocation. This
/// one enumeration serves both a match-at-position scan (`navigate.rs`,
/// first hit wins) and an emit-every-reference walk (`tokens.rs`, every
/// hit becomes a token) identically.
pub(super) fn label_refs(item: &Item) -> [Option<(u32, Span)>; 2] {
    match item {
        Item::Goto {
            label, label_span, ..
        } => [Some((*label, *label_span)), None],
        Item::Check {
            marked,
            blank,
            marked_span,
            blank_span,
            ..
        } => [
            match marked {
                CheckArm::Label(value) => Some((*value, *marked_span)),
                CheckArm::Return => None,
            },
            match blank {
                CheckArm::Label(value) => Some((*value, *blank_span)),
                CheckArm::Return => None,
            },
        ],
        Item::Builtin {
            succ,
            succ_label_span: Some(span),
            ..
        }
        | Item::Call {
            succ,
            succ_label_span: Some(span),
            ..
        } => match succ {
            Successor::Label(value) => [Some((*value, *span)), None],
            Successor::FallThrough | Successor::Return => [None, None],
        },
        _ => [None, None],
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::Pos;

    use super::*;

    #[test]
    fn span_contains_excludes_a_position_exactly_at_the_end() {
        // Half-open contract (this module's `span_contains` doc comment):
        // `end` is one past the last contained position, so a cursor
        // sitting exactly there belongs to whatever comes NEXT, not this
        // span — the off-by-one every caller relies on.
        let span = Span::new(1, 1, 1, 5);
        assert!(
            span_contains(span, Pos { line: 1, col: 1 }),
            "start is inclusive"
        );
        assert!(
            span_contains(span, Pos { line: 1, col: 4 }),
            "last contained column"
        );
        assert!(
            !span_contains(span, Pos { line: 1, col: 5 }),
            "end is exclusive"
        );
    }

    use crate::parser::parse_green;
    use mtc_core::syntax::{AstNode, SyntaxNode, TextLineIndex};

    use crate::syntax::FileView;

    fn file(src: &str) -> (FileView, TextLineIndex) {
        let root = SyntaxNode::new_root(parse_green(src).expect("parses"));
        (
            FileView::cast(root).expect("root is FILE"),
            TextLineIndex::new(src),
        )
    }

    #[test]
    fn chain_is_outermost_first_and_descends_into_nested() {
        let src =
            "ns() {\n    right;\n}\nouter() {\n    inner() {\n        left;\n    }\n    mark;\n}\n";
        let (file, index) = file(src);
        // Byte offset of the `left;` inside `inner`.
        let at = index.offset(mtc_core::diagnostics::Pos { line: 6, col: 9 });
        let chain = enclosing_function_chain(&file, at);
        let names: Vec<String> = chain
            .iter()
            .map(|f| f.header().name.text().to_string())
            .collect();
        assert_eq!(names, vec!["outer".to_string(), "inner".to_string()]);
    }

    #[test]
    fn chain_is_empty_outside_every_function() {
        let src = "use std::goToEnd;\nmain() {\n    right;\n}\n";
        let (file, index) = file(src);
        let at = index.offset(mtc_core::diagnostics::Pos { line: 1, col: 3 });
        assert!(enclosing_function_chain(&file, at).is_empty());
    }

    #[test]
    fn namespaces_are_walked_but_never_enter_the_chain() {
        let src = "namespace ns {\n    inside() {\n        right;\n    }\n}\n";
        let (file, index) = file(src);
        let at = index.offset(mtc_core::diagnostics::Pos { line: 3, col: 9 });
        let chain = enclosing_function_chain(&file, at);
        let names: Vec<String> = chain
            .iter()
            .map(|f| f.header().name.text().to_string())
            .collect();
        assert_eq!(names, vec!["inside".to_string()]);
    }

    #[test]
    fn function_labels_are_own_scope_only() {
        let src =
            "outer() {\n    1: right;\n    inner() {\n        2: left;\n    }\n    3: mark;\n}\n";
        let (file, index) = file(src);
        let at = index.offset(mtc_core::diagnostics::Pos { line: 6, col: 5 });
        let chain = enclosing_function_chain(&file, at);
        let outer = chain.first().expect("inside outer");
        let values: Vec<u32> = function_labels(outer, &index)
            .iter()
            .map(|l| l.value)
            .collect();
        assert_eq!(values, vec![1, 3], "inner's label 2 is a separate scope");
    }
}
