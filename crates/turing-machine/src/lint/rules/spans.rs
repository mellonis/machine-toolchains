//! Quickfix span queries over the green tree (docs/tmt/lint.md (quickfix
//! availability)). Every fix that deletes or edits a stretch of source
//! takes its span from here, and every query is a RANGE query: the
//! innermost node of the wanted kind that contains an anchor position the
//! resolved module already keeps (a name, a target, a rule, a map
//! literal), then that node's own range or a token pair inside it. Nothing
//! here indexes off a neighbouring token, so a comment sitting anywhere in
//! a declaration can neither void a span nor truncate it — the tree is
//! lossless and the node's range holds whatever was written inside it,
//! comment included; whether such a span may ship as a fix is the comment
//! guard's decision (`crate::lint::run_rules`), never a helper's.
//!
//! A declaration's extent is its node: a bound doc/attention run is
//! retro-wrapped into the declaration it documents, `export`/`entry` are
//! header tokens, and the closing `}` or `;` is the node's last token
//! (`crate::syntax`'s module doc) — so "the doc goes with what it
//! documents" is the node's shape, not a walk-back. Every query returns
//! `None` only for an anchor no node of the kind contains, which no
//! resolved anchor can produce; the rule then ships no fix.

use mtc_core::diagnostics::Span;
use mtc_core::syntax::{AstNode, SyntaxElement, SyntaxNode, SyntaxToken};

use crate::lint::LintContext;
use crate::parser::MapPair;
use crate::syntax::{GraftView, RuleView, TmcKind};

/// The innermost node castable to `V` on the path from `root` down to
/// the node containing byte `offset` — the range query every helper is
/// built on. `None` when no node of that kind lies on the path.
fn innermost<V: AstNode>(root: &SyntaxNode, offset: u32) -> Option<V> {
    let mut found = None;
    let mut node = root.clone();
    loop {
        if let Some(v) = V::cast(node.clone()) {
            found = Some(v);
        }
        let Some(child) = node
            .children()
            .find(|child| child.text_range().contains(offset))
        else {
            break;
        };
        node = child;
    }
    found
}

/// The full source span of the `V`-kind declaration or statement
/// containing `anchor`'s start — an `alphabet`/`routine`/`graph`
/// declaration through its closing `}`, a `graft`/`bind` statement
/// through its `;`, a bound doc/attention run and any `export`/`entry`
/// included. `anchor` is the name or target span the resolved module
/// keeps for it.
pub(crate) fn decl_span<V: AstNode>(ctx: &LintContext, anchor: Span) -> Option<Span> {
    let node = innermost::<V>(ctx.root, ctx.index.offset(anchor.start))?;
    Some(ctx.index.span(node.syntax().text_range()))
}

fn is_trivia(kind: mtc_core::syntax::SyntaxKind) -> bool {
    kind == TmcKind::Whitespace.into()
        || kind == TmcKind::LineComment.into()
        || kind == TmcKind::BlockComment.into()
}

/// The previous significant sibling TOKEN of `t` — trivia skipped;
/// `None` at the node's start or when a node sits there instead.
fn prev_significant_token(t: &SyntaxToken) -> Option<SyntaxToken> {
    let mut cur = t.prev_sibling_or_token();
    while let Some(SyntaxElement::Token(p)) = &cur {
        if !is_trivia(p.kind()) {
            return Some(p.clone());
        }
        cur = p.prev_sibling_or_token();
    }
    None
}

/// The next significant sibling ELEMENT of `t` — trivia skipped; a node
/// counts (a rule's `move` vector, its transition).
fn next_significant(t: &SyntaxToken) -> Option<SyntaxElement> {
    let mut cur = t.next_sibling_or_token();
    while let Some(SyntaxElement::Token(n)) = &cur {
        if !is_trivia(n.kind()) {
            break;
        }
        cur = n.next_sibling_or_token();
    }
    cur
}

/// The span of a graft's ` as NAME` clause — from the end of the binding
/// list's `)` through the end of the instance name — so deleting it
/// turns `graft T(args) as N;` into `graft T(args);`. Anchored on the
/// graft's own span. `None` for an unnamed graft.
pub(crate) fn as_clause_span(ctx: &LintContext, graft_span: Span) -> Option<Span> {
    let graft = innermost::<GraftView>(ctx.root, ctx.index.offset(graft_span.start))?;
    let name = graft.as_name()?;
    let as_kw = prev_significant_token(&name)?;
    let rparen = prev_significant_token(&as_kw)?;
    if as_kw.text() != "as" || rparen.kind() != TmcKind::RParen.into() {
        return None;
    }
    Some(Span {
        start: ctx.index.span(rparen.text_range()).end,
        end: ctx.index.span(name.text_range()).end,
    })
}

/// The span to delete to remove a rule's `debugger` marker — the marker
/// keyword through the start of what follows it (its vector, or its
/// transition), so the trailing space goes too. The marker is the first
/// significant token after the rule's `->`. `None` when the rule carries
/// no marker there.
pub(crate) fn marker_span(ctx: &LintContext, rule_span: Span) -> Option<Span> {
    let rule = innermost::<RuleView>(ctx.root, ctx.index.offset(rule_span.start))?;
    let arrow = rule.syntax().children_with_tokens().find_map(|e| match e {
        SyntaxElement::Token(t) if t.kind() == TmcKind::Arrow.into() => Some(t),
        _ => None,
    })?;
    let SyntaxElement::Token(marker) = next_significant(&arrow)? else {
        return None;
    };
    if marker.kind() != TmcKind::Ident.into() || marker.text() != "debugger" {
        return None;
    }
    let next = next_significant(&marker)?;
    Some(Span {
        start: ctx.index.span(marker.text_range()).start,
        end: ctx.index.span(next.text_range()).start,
    })
}

/// The `->` token inside one map pair — the span a demotion edit
/// replaces. Nothing but the arrow can sit between the pair's two
/// literals, so the search is by range containment between them.
pub(crate) fn arrow_span(ctx: &LintContext, pair: &MapPair) -> Option<Span> {
    let after_src = ctx.index.offset(pair.src.span().end);
    let before_dst = ctx.index.offset(pair.dst.span().start);
    ctx.root
        .descendant_tokens()
        .find(|t| {
            t.kind() == TmcKind::Arrow.into()
                && after_src <= t.text_range().start
                && t.text_range().end <= before_dst
        })
        .map(|t| ctx.index.span(t.text_range()))
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::{Pos, Span};
    use mtc_core::syntax::{SyntaxNode, TextLineIndex};

    use super::*;
    use crate::compiler::{Analysis, analyze};
    use crate::lint::LintContext;
    use crate::syntax::{AlphabetView, BindView, GraftView, ReuseView};

    /// The batch context over `src`, the way `lint()` builds it.
    fn with_context<R>(src: &str, f: impl FnOnce(&LintContext) -> R) -> R {
        let a: Analysis = analyze(src).unwrap();
        let root = SyntaxNode::new_root(std::rc::Rc::clone(&a.green));
        let index = TextLineIndex::new(src);
        let ctx = LintContext {
            resolved: &a.resolved,
            diagnostics: &a.diagnostics,
            program: &a.program,
            root: &root,
            index: &index,
            comment_tokens: &a.tokens,
        };
        f(&ctx)
    }

    fn text_of(src: &str, span: Span, index: &TextLineIndex) -> String {
        src[index.offset(span.start) as usize..index.offset(span.end) as usize].to_string()
    }

    const BASE: &str = "\
machine {
  tape t: bit;
  entry state s { [*] -> move [>] stop; }
}
";

    #[test]
    fn decl_span_is_the_declaration_node_comment_included() {
        // A comment between the keyword and the name: the adjacency
        // helper found a comment where it expected the keyword and gave
        // up; the node query answers the whole declaration, comment and
        // all (the guard then decides what to do with it — not us).
        let src = format!("alphabet bit {{ '_', '1' }}\nalphabet /* c */ dead {{ '0' }}\n{BASE}");
        with_context(&src, |ctx| {
            let dead = &ctx.resolved.alphabets["dead"];
            let span = decl_span::<AlphabetView>(ctx, dead.name_span).expect("a node");
            assert_eq!(
                text_of(&src, span, ctx.index),
                "alphabet /* c */ dead { '0' }"
            );
        });
    }

    #[test]
    fn decl_span_takes_the_bound_doc_run() {
        let src = format!(
            "alphabet bit {{ '_', '1' }}\n? documented\n! carefully\nexport alphabet dead {{ '0' }}\n{BASE}"
        );
        with_context(&src, |ctx| {
            let dead = &ctx.resolved.alphabets["dead"];
            let span = decl_span::<AlphabetView>(ctx, dead.name_span).expect("a node");
            assert_eq!(
                text_of(&src, span, ctx.index),
                "? documented\n! carefully\nexport alphabet dead { '0' }"
            );
        });
    }

    #[test]
    fn decl_span_covers_a_braced_world_and_a_statement() {
        let src = "\
alphabet bit { '_', '1' }
routine r(tape t: bit) { entry state s { [*] -> return; } }
graph g(tape t: bit, state out) { entry state s { [*] -> out; } }
machine {
  tape t: bit;
  entry graft g(t = t, out = done) as gi;
  bind r(t = t) as b;
  state done { [*] -> stop; }
}
";
        with_context(src, |ctx| {
            let world = ctx
                .resolved
                .worlds
                .iter()
                .find(|w| w.name == "r")
                .expect("the routine");
            let span = decl_span::<ReuseView>(ctx, world.name_span).expect("a node");
            assert_eq!(
                text_of(src, span, ctx.index),
                "routine r(tape t: bit) { entry state s { [*] -> return; } }"
            );
            let machine = ctx
                .resolved
                .worlds
                .iter()
                .find(|w| !w.grafts.is_empty())
                .expect("the machine");
            let graft = &machine.grafts[0];
            let span = decl_span::<GraftView>(ctx, graft.target_span).expect("a node");
            assert_eq!(
                text_of(src, span, ctx.index),
                "entry graft g(t = t, out = done) as gi;"
            );
            let bind = &machine.binds[0];
            let span = decl_span::<BindView>(ctx, bind.target_span).expect("a node");
            assert_eq!(text_of(src, span, ctx.index), "bind r(t = t) as b;");
            // The as-clause: from the `)` through the name.
            let span = as_clause_span(ctx, graft.span).expect("a clause");
            assert_eq!(text_of(src, span, ctx.index), " as gi");
        });
    }

    #[test]
    fn marker_span_ends_at_the_next_token() {
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { [*] -> debugger move [>] stop; }
}
";
        with_context(src, |ctx| {
            let rule = &ctx.resolved.worlds[0].states[0].rules[0];
            let span = marker_span(ctx, rule.span).expect("a marker");
            assert_eq!(text_of(src, span, ctx.index), "debugger ");
            assert_eq!(span.start, Pos { line: 4, col: 26 });
        });
    }
}
