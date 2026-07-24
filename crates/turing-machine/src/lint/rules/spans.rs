//! Shared source-span reconstruction for the deletion quickfixes on the
//! unused-* rules (docs/tmt/lint.md (the `.tmc` rules)). Each recovers the full
//! source extent of a declaration or statement from the COMMENT-FREE token
//! stream, anchored on a span the resolved module keeps (a world name, a reuse
//! target). A leading doc/attention run always goes with what it documents — an
//! orphaned `?`/`!` run is a parse error, so "still compiles" would fail
//! otherwise. Every helper returns `None` when the token neighbourhood is not
//! the expected shape; the rule then simply ships no fix.

use mtc_core::diagnostics::Span;

use crate::lexer::{Token, TokenKind};

fn is_kw(t: &Token, kw: &str) -> bool {
    matches!(&t.kind, TokenKind::Ident(k) if k == kw)
}

fn is_doc(t: &Token) -> bool {
    matches!(t.kind, TokenKind::DocLine(_) | TokenKind::AttentionLine(_))
}

/// Back `start_ix` up over any contiguous leading doc/attention run.
fn back_over_doc_run(tokens: &[Token], mut start_ix: usize) -> usize {
    while let Some(prev) = start_ix.checked_sub(1)
        && is_doc(&tokens[prev])
    {
        start_ix = prev;
    }
    start_ix
}

/// The full span of a braced world declaration — a `graph` or `routine`: its
/// leading doc/attention run, the `export`/KEYWORD header, and the body `{ … }`
/// through the brace-matched closing `}`. Anchored on the declared NAME;
/// `keyword` is `"graph"` or `"routine"`. Substitution `{…}` markers inside the
/// body are balanced, so the depth count reaches the real end. `None` for an
/// unexpected shape (a NAME not sitting immediately after its keyword).
pub(crate) fn braced_world_decl_span(
    tokens: &[Token],
    name_span: Span,
    keyword: &str,
) -> Option<Span> {
    let name_ix = tokens
        .iter()
        .position(|t| t.span().start == name_span.start)?;
    let mut start_ix = name_ix.checked_sub(1)?;
    if !is_kw(&tokens[start_ix], keyword) {
        return None;
    }
    if let Some(prev) = start_ix.checked_sub(1)
        && is_kw(&tokens[prev], "export")
    {
        start_ix = prev;
    }
    start_ix = back_over_doc_run(tokens, start_ix);

    // The body opens at the first `{` after the NAME (the signature uses `(…)`,
    // never braces) and closes at its brace-matched `}`.
    let open_rel = tokens[name_ix..]
        .iter()
        .position(|t| matches!(t.kind, TokenKind::LBrace))?;
    let open_ix = name_ix + open_rel;
    let mut depth = 0usize;
    let mut close_ix = None;
    for (ix, t) in tokens.iter().enumerate().skip(open_ix) {
        match t.kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => {
                depth -= 1;
                if depth == 0 {
                    close_ix = Some(ix);
                    break;
                }
            }
            _ => {}
        }
    }
    let close_ix = close_ix?;
    Some(Span {
        start: tokens[start_ix].span().start,
        end: tokens[close_ix].span().end,
    })
}

/// The full span of a single-line reuse statement — a `graft` or `bind`: any
/// leading doc/attention run through the terminating `;`. Anchored on the
/// TARGET name (a `QualName` span starts at its first segment, so the keyword
/// sits immediately before it); `keyword` is `"graft"` or `"bind"`. A graft may
/// carry a leading `entry`, taken with it. A graft/bind nests no braces, so the
/// first `;` at or after the target ends the statement. `None` for an
/// unexpected shape.
pub(crate) fn reuse_statement_span(
    tokens: &[Token],
    target_span: Span,
    keyword: &str,
) -> Option<Span> {
    let target_ix = tokens
        .iter()
        .position(|t| t.span().start == target_span.start)?;
    let mut start_ix = target_ix.checked_sub(1)?;
    if !is_kw(&tokens[start_ix], keyword) {
        return None;
    }
    if let Some(prev) = start_ix.checked_sub(1)
        && is_kw(&tokens[prev], "entry")
    {
        start_ix = prev;
    }
    start_ix = back_over_doc_run(tokens, start_ix);

    let semi_rel = tokens[target_ix..]
        .iter()
        .position(|t| matches!(t.kind, TokenKind::Semi))?;
    let semi_ix = target_ix + semi_rel;
    Some(Span {
        start: tokens[start_ix].span().start,
        end: tokens[semi_ix].span().end,
    })
}
