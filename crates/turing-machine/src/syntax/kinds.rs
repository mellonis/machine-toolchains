//! The `.tmc` syntax-kind space over the core framework's opaque
//! `SyntaxKind` (docs/core.md (syntax trees)): token kinds mirror the
//! lexer's `TokenKind` (plus the three trivia kinds the token stream
//! carries only implicitly — a comment's line/block flavor and
//! whitespace runs), node kinds mirror the grammar's containers.
//! `Eof` has no kind: the green tree carries trailing trivia instead
//! of a zero-length sentinel.

use crate::lexer::{CommentKind, TokenKind};
use mtc_core::syntax::SyntaxKind;

/// `.tmc` kinds. Token kinds first, then trivia, then nodes. The
/// discriminant IS the wire value inside `SyntaxKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum TmcKind {
    // Significant tokens (mirror lexer::TokenKind, minus Eof/Comment).
    Ident = 0,
    Number = 1,
    Glyph = 2,
    DotDot = 3,
    Arrow = 4,
    FatArrow = 5,
    ColonColon = 6,
    Dot = 7,
    Dash = 8,
    Plus = 9,
    Eq = 10,
    Star = 11,
    Percent = 12,
    Lt = 13,
    Gt = 14,
    LBracket = 15,
    RBracket = 16,
    LBrace = 17,
    RBrace = 18,
    LParen = 19,
    RParen = 20,
    Comma = 21,
    Semi = 22,
    Colon = 23,
    At = 24,
    Bang = 25,
    DocLine = 26,
    AttentionLine = 27,
    // Trivia tokens.
    LineComment = 28,
    BlockComment = 29,
    Whitespace = 30,
    // Nodes.
    Use = 32,
    UsePath = 33,
    Alphabet = 34,
    Reuse = 35,
    Machine = 36,
    Namespace = 37,
    World = 38,
    Tape = 39,
    State = 40,
    Rule = 41,
    Graft = 42,
    Bind = 43,
    DocRun = 44,
    Attr = 45,
    Root = 46,
}

impl From<TmcKind> for SyntaxKind {
    fn from(k: TmcKind) -> SyntaxKind {
        SyntaxKind(k as u16)
    }
}

/// Debug name for a `.tmc` kind — the `kind_name` callback for
/// `mtc_core::syntax::debug_dump`. Unknown values render as `"?"`.
pub fn kind_name(kind: SyntaxKind) -> &'static str {
    match kind {
        k if k == TmcKind::Ident.into() => "IDENT",
        k if k == TmcKind::Number.into() => "NUMBER",
        k if k == TmcKind::Glyph.into() => "GLYPH",
        k if k == TmcKind::DotDot.into() => "DOT_DOT",
        k if k == TmcKind::Arrow.into() => "ARROW",
        k if k == TmcKind::FatArrow.into() => "FAT_ARROW",
        k if k == TmcKind::ColonColon.into() => "COLON_COLON",
        k if k == TmcKind::Dot.into() => "DOT",
        k if k == TmcKind::Dash.into() => "DASH",
        k if k == TmcKind::Plus.into() => "PLUS",
        k if k == TmcKind::Eq.into() => "EQ",
        k if k == TmcKind::Star.into() => "STAR",
        k if k == TmcKind::Percent.into() => "PERCENT",
        k if k == TmcKind::Lt.into() => "LT",
        k if k == TmcKind::Gt.into() => "GT",
        k if k == TmcKind::LBracket.into() => "L_BRACKET",
        k if k == TmcKind::RBracket.into() => "R_BRACKET",
        k if k == TmcKind::LBrace.into() => "L_BRACE",
        k if k == TmcKind::RBrace.into() => "R_BRACE",
        k if k == TmcKind::LParen.into() => "L_PAREN",
        k if k == TmcKind::RParen.into() => "R_PAREN",
        k if k == TmcKind::Comma.into() => "COMMA",
        k if k == TmcKind::Semi.into() => "SEMI",
        k if k == TmcKind::Colon.into() => "COLON",
        k if k == TmcKind::At.into() => "AT",
        k if k == TmcKind::Bang.into() => "BANG",
        k if k == TmcKind::DocLine.into() => "DOC_LINE",
        k if k == TmcKind::AttentionLine.into() => "ATTENTION_LINE",
        k if k == TmcKind::LineComment.into() => "LINE_COMMENT",
        k if k == TmcKind::BlockComment.into() => "BLOCK_COMMENT",
        k if k == TmcKind::Whitespace.into() => "WHITESPACE",
        k if k == TmcKind::Use.into() => "USE",
        k if k == TmcKind::UsePath.into() => "USE_PATH",
        k if k == TmcKind::Alphabet.into() => "ALPHABET",
        k if k == TmcKind::Reuse.into() => "REUSE",
        k if k == TmcKind::Machine.into() => "MACHINE",
        k if k == TmcKind::Namespace.into() => "NAMESPACE",
        k if k == TmcKind::World.into() => "WORLD",
        k if k == TmcKind::Tape.into() => "TAPE",
        k if k == TmcKind::State.into() => "STATE",
        k if k == TmcKind::Rule.into() => "RULE",
        k if k == TmcKind::Graft.into() => "GRAFT",
        k if k == TmcKind::Bind.into() => "BIND",
        k if k == TmcKind::DocRun.into() => "DOC_RUN",
        k if k == TmcKind::Attr.into() => "ATTR",
        k if k == TmcKind::Root.into() => "ROOT",
        _ => "?",
    }
}

/// Map a significant `TokenKind` to its green-tree kind — the sink's
/// counterpart to `TokenKind`, since `TmcKind`'s token variants mirror
/// it 1:1. A `match` with no wildcard arm by design: adding a lexer
/// token later fails this build instead of the token silently
/// resolving to the wrong kind. `Eof` is never bumped into a tree — the
/// green tree carries trailing trivia instead of a zero-length
/// sentinel — so that arm is unreachable rather than mapped.
#[allow(dead_code)] // wired up once the green-tree builder lands and calls it
pub(crate) fn token_kind(t: &TokenKind) -> TmcKind {
    match t {
        TokenKind::Ident(_) => TmcKind::Ident,
        TokenKind::Number(_, _) => TmcKind::Number,
        TokenKind::Glyph(_) => TmcKind::Glyph,
        TokenKind::DotDot => TmcKind::DotDot,
        TokenKind::Arrow => TmcKind::Arrow,
        TokenKind::FatArrow => TmcKind::FatArrow,
        TokenKind::ColonColon => TmcKind::ColonColon,
        TokenKind::Dot => TmcKind::Dot,
        TokenKind::Dash => TmcKind::Dash,
        TokenKind::Plus => TmcKind::Plus,
        TokenKind::Eq => TmcKind::Eq,
        TokenKind::Star => TmcKind::Star,
        TokenKind::Percent => TmcKind::Percent,
        TokenKind::Lt => TmcKind::Lt,
        TokenKind::Gt => TmcKind::Gt,
        TokenKind::LBracket => TmcKind::LBracket,
        TokenKind::RBracket => TmcKind::RBracket,
        TokenKind::LBrace => TmcKind::LBrace,
        TokenKind::RBrace => TmcKind::RBrace,
        TokenKind::LParen => TmcKind::LParen,
        TokenKind::RParen => TmcKind::RParen,
        TokenKind::Comma => TmcKind::Comma,
        TokenKind::Semi => TmcKind::Semi,
        TokenKind::Colon => TmcKind::Colon,
        TokenKind::At => TmcKind::At,
        TokenKind::Bang => TmcKind::Bang,
        TokenKind::DocLine(_) => TmcKind::DocLine,
        TokenKind::AttentionLine(_) => TmcKind::AttentionLine,
        TokenKind::Comment(c) => match c.kind {
            CommentKind::Line => TmcKind::LineComment,
            CommentKind::Block => TmcKind::BlockComment,
        },
        TokenKind::Eof => unreachable!("Eof carries no kind and is never bumped into a tree"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};

    /// Every significant token the lexer can produce maps to a kind, and
    /// distinct token kinds never collapse onto one syntax kind. `Eof`
    /// carries no kind — the tree holds trailing trivia instead of a
    /// zero-length sentinel — and `Comment` becomes trivia, not a
    /// significant kind.
    ///
    /// The distinctness half asserts injectivity on the PRODUCED
    /// `TmcKind`, not on the token discriminant used to key the walk:
    /// keying on `std::mem::discriminant(&t.kind)` (as an earlier
    /// version of this test did) can only ever re-confirm that
    /// `token_kind` is a pure function of its input, since each
    /// discriminant is only ever compared against itself — it can
    /// never observe two DIFFERENT token kinds collapsing onto one
    /// `TmcKind`. Keying on the `TmcKind` instead, and failing when a
    /// slot already holds a different token's discriminant, is what
    /// actually catches a collision such as `Comma` and `Semi` both
    /// mapping to `TmcKind::Semi`.
    #[test]
    fn every_significant_token_kind_maps_to_a_distinct_kind() {
        let src = "? doc\n! attention\nalphabet ab { '_', 'a' }\n\
                   machine {\n  tape main: ab;\n  entry state s {\n\
                     ['a'] -> write ['_'] move [>] goto s;\n\
                     [*] -> stop;\n  }\n}\n";
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let mut seen_tokens = std::collections::HashSet::new();
        let mut by_kind = std::collections::HashMap::new();
        for t in &tokens {
            if matches!(t.kind, crate::lexer::TokenKind::Eof) {
                continue;
            }
            let disc = std::mem::discriminant(&t.kind);
            seen_tokens.insert(disc);
            let k = token_kind(&t.kind);
            if let Some(prev_disc) = by_kind.insert(k, disc) {
                assert_eq!(
                    prev_disc, disc,
                    "two distinct token kinds both mapped to {k:?}"
                );
            }
        }
        assert!(
            seen_tokens.len() >= 12,
            "fixture exercised only {} kinds",
            seen_tokens.len()
        );
    }

    /// `kind_name` answers for every kind the enum defines, so a tree
    /// dump can never print a bare number. This is NOT a completeness
    /// check on the kind space: a raw value with no `TmcKind` behind
    /// it at all (e.g. a deleted variant) falls into the `_ => "?"`
    /// fallback and still satisfies this walk — `"?"` is non-empty.
    #[test]
    fn kind_name_answers_for_every_kind() {
        for raw in 0u16..=(TmcKind::Root as u16) {
            let name = kind_name(SyntaxKind(raw));
            assert!(!name.is_empty(), "kind {raw} has no name");
        }
    }
}
