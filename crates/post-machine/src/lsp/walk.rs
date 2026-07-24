//! Shared CST-walk primitives (docs/lsp.md (go-to-definition), docs/lsp.md
//! (completions), docs/lsp.md (semantic tokens)): the position-to-
//! enclosing-function walk, the function-scoped label lookups, and
//! half-open span containment. `navigate.rs`, `complete.rs`, and
//! `tokens.rs` each need the same shapes for different ends — a
//! match-at-position query, a full-chain assembly, an emit-everything
//! walk — so the ENUMERATION lives here once and every caller supplies
//! its own terminal behavior over the result.

use mtc_core::diagnostics::{Pos, Span};

use crate::cst::{BodyKind, FunctionCst, TopItem, TopKind};
use crate::parser::{CheckArm, Item, Label, Successor};

/// Half-open span containment, 1-based. `Pos`'s derived `Ord` compares
/// `line` then `col` (its field order) — exactly a lexicographic
/// position comparison — so this is correct for a multi-line span with
/// no special-casing.
pub(super) fn span_contains(span: Span, pos: Pos) -> bool {
    pos >= span.start && pos < span.end
}

/// The enclosing function CHAIN at `pos`, outermost first: the
/// top-level function containing `pos`, then its `BodyKind::Nested`
/// descendant containing `pos`, as deep as `pos` still lands inside one.
/// Namespace blocks are walked but never themselves added to the chain —
/// only a function's own extent does that. Empty when `pos` isn't inside
/// any function at all. A caller that only wants the innermost enclosing
/// function takes `.pop()`; a caller that needs every enclosing level
/// (qualified-name reconstruction, hoisted nested defs) walks the whole
/// `Vec`.
pub(super) fn enclosing_function_chain(items: &[TopItem], pos: Pos) -> Vec<&FunctionCst> {
    for item in items {
        match &item.kind {
            TopKind::Namespace(ns) => {
                let chain = enclosing_function_chain(&ns.items, pos);
                if !chain.is_empty() {
                    return chain;
                }
            }
            TopKind::Function(f) => {
                if span_contains(f.span, pos) {
                    let mut chain = vec![f];
                    push_deepest_nested(f, pos, &mut chain);
                    return chain;
                }
            }
            TopKind::Comment(_) | TopKind::Import(_) => {}
        }
    }
    Vec::new()
}

/// Descends into `f`'s own `BodyKind::Nested` children as long as `pos`
/// stays inside one, pushing each one reached onto `chain`.
fn push_deepest_nested<'a>(f: &'a FunctionCst, pos: Pos, chain: &mut Vec<&'a FunctionCst>) {
    for item in &f.body {
        if let BodyKind::Nested(nested) = &item.kind
            && span_contains(nested.span, pos)
        {
            chain.push(nested);
            push_deepest_nested(nested, pos, chain);
            return;
        }
    }
}

/// Every label `function` declares in its OWN statements, in source
/// order — function-scoped, never descending into nested children (a
/// nested function's labels are a separate scope, reached only by
/// walking to that nested function's own chain entry first, via
/// [`enclosing_function_chain`]).
pub(super) fn function_labels(function: &FunctionCst) -> impl Iterator<Item = &Label> {
    function
        .body
        .iter()
        .filter_map(|item| match &item.kind {
            BodyKind::Statement(stmt) => Some(stmt.labels.iter()),
            _ => None,
        })
        .flatten()
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
}
