//! The `.pmc` syntax-kind space over the core framework's opaque
//! `SyntaxKind` (docs/core.md (syntax tree)): token kinds mirror the
//! lexer's `TokenKind` (plus the two trivia kinds the token stream
//! carries only implicitly — whitespace runs and comments), node kinds
//! mirror the grammar's containers. `Eof` has no kind: the green tree
//! carries trailing trivia instead of a zero-length sentinel.
//!
//! Node granularity: containers down to `CHECK_ARM`. Successor arrows
//! and check internals stay as tokens inside `ITEM`/`CHECK_ARM` — a
//! view derives them; a finer node is an additive kind if a later
//! plan wants one.

use mtc_core::syntax::SyntaxKind;

/// `.pmc` kinds. Token kinds first, then trivia, then nodes. The
/// discriminant IS the wire value inside `SyntaxKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum PmcKind {
    // Significant tokens (mirror lexer::TokenKind, minus Eof/Comment).
    Ident = 0,
    Number = 1,
    At = 2,
    Bang = 3,
    Comma = 4,
    Semi = 5,
    Colon = 6,
    ColonColon = 7,
    LParen = 8,
    RParen = 9,
    LBrace = 10,
    RBrace = 11,
    DocLine = 12,
    AttentionLine = 13,
    // Trivia tokens.
    LineComment = 14,
    BlockComment = 15,
    Whitespace = 16,
    // Nodes.
    File = 32,
    UseDecl = 33,
    UsePath = 34,
    Namespace = 35,
    Function = 36,
    DocRun = 37,
    Statement = 38,
    Label = 39,
    Item = 40,
    CheckArm = 41,
}

impl From<PmcKind> for SyntaxKind {
    fn from(k: PmcKind) -> SyntaxKind {
        SyntaxKind(k as u16)
    }
}

/// Debug name for a `.pmc` kind — the `kind_name` callback for
/// `mtc_core::syntax::debug_dump`. Unknown values render as `"?"`.
pub fn kind_name(kind: SyntaxKind) -> &'static str {
    match kind {
        k if k == PmcKind::Ident.into() => "IDENT",
        k if k == PmcKind::Number.into() => "NUMBER",
        k if k == PmcKind::At.into() => "AT",
        k if k == PmcKind::Bang.into() => "BANG",
        k if k == PmcKind::Comma.into() => "COMMA",
        k if k == PmcKind::Semi.into() => "SEMI",
        k if k == PmcKind::Colon.into() => "COLON",
        k if k == PmcKind::ColonColon.into() => "COLON_COLON",
        k if k == PmcKind::LParen.into() => "L_PAREN",
        k if k == PmcKind::RParen.into() => "R_PAREN",
        k if k == PmcKind::LBrace.into() => "L_BRACE",
        k if k == PmcKind::RBrace.into() => "R_BRACE",
        k if k == PmcKind::DocLine.into() => "DOC_LINE",
        k if k == PmcKind::AttentionLine.into() => "ATTENTION_LINE",
        k if k == PmcKind::LineComment.into() => "LINE_COMMENT",
        k if k == PmcKind::BlockComment.into() => "BLOCK_COMMENT",
        k if k == PmcKind::Whitespace.into() => "WHITESPACE",
        k if k == PmcKind::File.into() => "FILE",
        k if k == PmcKind::UseDecl.into() => "USE_DECL",
        k if k == PmcKind::UsePath.into() => "USE_PATH",
        k if k == PmcKind::Namespace.into() => "NAMESPACE",
        k if k == PmcKind::Function.into() => "FUNCTION",
        k if k == PmcKind::DocRun.into() => "DOC_RUN",
        k if k == PmcKind::Statement.into() => "STATEMENT",
        k if k == PmcKind::Label.into() => "LABEL",
        k if k == PmcKind::Item.into() => "ITEM",
        k if k == PmcKind::CheckArm.into() => "CHECK_ARM",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtc_core::syntax::SyntaxKind;

    #[test]
    fn kinds_convert_and_name_round_trip() {
        let k: SyntaxKind = PmcKind::Ident.into();
        assert_eq!(kind_name(k), "IDENT");
        assert_eq!(kind_name(PmcKind::File.into()), "FILE");
        assert_eq!(kind_name(PmcKind::CheckArm.into()), "CHECK_ARM");
        assert_eq!(kind_name(SyntaxKind(u16::MAX)), "?");
    }

    #[test]
    fn kind_values_are_distinct() {
        // The discriminants are the kind space — a duplicate would alias
        // two kinds. Collect and compare counts.
        let all = [
            PmcKind::Ident,
            PmcKind::Number,
            PmcKind::At,
            PmcKind::Bang,
            PmcKind::Comma,
            PmcKind::Semi,
            PmcKind::Colon,
            PmcKind::ColonColon,
            PmcKind::LParen,
            PmcKind::RParen,
            PmcKind::LBrace,
            PmcKind::RBrace,
            PmcKind::DocLine,
            PmcKind::AttentionLine,
            PmcKind::LineComment,
            PmcKind::BlockComment,
            PmcKind::Whitespace,
            PmcKind::File,
            PmcKind::UseDecl,
            PmcKind::UsePath,
            PmcKind::Namespace,
            PmcKind::Function,
            PmcKind::DocRun,
            PmcKind::Statement,
            PmcKind::Label,
            PmcKind::Item,
            PmcKind::CheckArm,
        ];
        let mut vals: Vec<u16> = all.iter().map(|k| *k as u16).collect();
        vals.sort_unstable();
        vals.dedup();
        assert_eq!(vals.len(), all.len());
    }
}
