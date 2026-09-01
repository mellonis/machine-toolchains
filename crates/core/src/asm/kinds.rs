//! The assembly syntax-kind space over the syntax framework's opaque
//! `SyntaxKind` (docs/core.md (syntax trees)): token kinds mirror the
//! asm lexer's [`AsmTokenKind`] plus the two trivia kinds the token
//! stream carries only implicitly (whitespace runs, comments), node
//! kinds mirror [`super::cst::AsmItemKind`]'s shapes plus the root.
//! One shared space for every asm dialect — the kinds describe the
//! line grammar, which is arch-agnostic; mnemonics stay plain WORD
//! tokens exactly as they are plain `Word`s in the lexer.
//!
//! Phase-1 granularity: one node per CST item, tokens flat inside it.
//! Labels, instructions, and operands are typed-view territory for a
//! later round, not tree structure here.

use super::lexer::AsmTokenKind;
use crate::syntax::SyntaxKind;

/// Assembly kinds. Token kinds first, then trivia, then nodes. The
/// discriminant IS the wire value inside `SyntaxKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum AsmKind {
    // Significant tokens (mirror AsmTokenKind, minus Comment).
    Word = 0,
    Number = 1,
    Colon = 2,
    Comma = 3,
    At = 4,
    LBracket = 5,
    RBracket = 6,
    LBrace = 7,
    RBrace = 8,
    LParen = 9,
    RParen = 10,
    Eq = 11,
    Hash = 12,
    Arrow = 13,
    FatArrow = 14,
    Star = 15,
    Dash = 16,
    Plus = 17,
    Percent = 18,
    Lt = 19,
    Gt = 20,
    Dot = 21,
    Junk = 22,
    // Trivia tokens.
    Whitespace = 30,
    Comment = 31,
    // Nodes (mirror AsmItemKind, plus the root).
    Root = 32,
    Line = 33,
    Func = 34,
    Raw = 35,
    Section = 36,
    TableDirective = 37,
    Rept = 38,
    RoutineDirective = 39,
    Volatile = 40,
    FrameDirective = 41,
}

impl From<AsmKind> for SyntaxKind {
    fn from(k: AsmKind) -> SyntaxKind {
        SyntaxKind(k as u16)
    }
}

/// Debug name for an assembly kind — the `kind_name` callback for
/// `crate::syntax::debug_dump`. Unknown values render as `"?"`.
pub fn kind_name(kind: SyntaxKind) -> &'static str {
    match kind {
        k if k == AsmKind::Word.into() => "WORD",
        k if k == AsmKind::Number.into() => "NUMBER",
        k if k == AsmKind::Colon.into() => "COLON",
        k if k == AsmKind::Comma.into() => "COMMA",
        k if k == AsmKind::At.into() => "AT",
        k if k == AsmKind::LBracket.into() => "L_BRACKET",
        k if k == AsmKind::RBracket.into() => "R_BRACKET",
        k if k == AsmKind::LBrace.into() => "L_BRACE",
        k if k == AsmKind::RBrace.into() => "R_BRACE",
        k if k == AsmKind::LParen.into() => "L_PAREN",
        k if k == AsmKind::RParen.into() => "R_PAREN",
        k if k == AsmKind::Eq.into() => "EQ",
        k if k == AsmKind::Hash.into() => "HASH",
        k if k == AsmKind::Arrow.into() => "ARROW",
        k if k == AsmKind::FatArrow.into() => "FAT_ARROW",
        k if k == AsmKind::Star.into() => "STAR",
        k if k == AsmKind::Dash.into() => "DASH",
        k if k == AsmKind::Plus.into() => "PLUS",
        k if k == AsmKind::Percent.into() => "PERCENT",
        k if k == AsmKind::Lt.into() => "LT",
        k if k == AsmKind::Gt.into() => "GT",
        k if k == AsmKind::Dot.into() => "DOT",
        k if k == AsmKind::Junk.into() => "JUNK",
        k if k == AsmKind::Whitespace.into() => "WHITESPACE",
        k if k == AsmKind::Comment.into() => "COMMENT",
        k if k == AsmKind::Root.into() => "ROOT",
        k if k == AsmKind::Line.into() => "LINE",
        k if k == AsmKind::Func.into() => "FUNC",
        k if k == AsmKind::Raw.into() => "RAW",
        k if k == AsmKind::Section.into() => "SECTION",
        k if k == AsmKind::TableDirective.into() => "TABLE_DIRECTIVE",
        k if k == AsmKind::Rept.into() => "REPT",
        k if k == AsmKind::RoutineDirective.into() => "ROUTINE_DIRECTIVE",
        k if k == AsmKind::Volatile.into() => "VOLATILE",
        k if k == AsmKind::FrameDirective.into() => "FRAME_DIRECTIVE",
        _ => "?",
    }
}

/// Total significant-token mapping for green emission. `Comment` never
/// reaches it — the layout adapter classifies comments as trivia.
pub(crate) fn token_green_kind(t: &AsmTokenKind) -> AsmKind {
    match t {
        AsmTokenKind::Word(_) => AsmKind::Word,
        AsmTokenKind::Number(_) => AsmKind::Number,
        AsmTokenKind::Colon => AsmKind::Colon,
        AsmTokenKind::Comma => AsmKind::Comma,
        AsmTokenKind::At => AsmKind::At,
        AsmTokenKind::LBracket => AsmKind::LBracket,
        AsmTokenKind::RBracket => AsmKind::RBracket,
        AsmTokenKind::LBrace => AsmKind::LBrace,
        AsmTokenKind::RBrace => AsmKind::RBrace,
        AsmTokenKind::LParen => AsmKind::LParen,
        AsmTokenKind::RParen => AsmKind::RParen,
        AsmTokenKind::Eq => AsmKind::Eq,
        AsmTokenKind::Hash => AsmKind::Hash,
        AsmTokenKind::Arrow => AsmKind::Arrow,
        AsmTokenKind::FatArrow => AsmKind::FatArrow,
        AsmTokenKind::Star => AsmKind::Star,
        AsmTokenKind::Dash => AsmKind::Dash,
        AsmTokenKind::Plus => AsmKind::Plus,
        AsmTokenKind::Percent => AsmKind::Percent,
        AsmTokenKind::Lt => AsmKind::Lt,
        AsmTokenKind::Gt => AsmKind::Gt,
        AsmTokenKind::Dot => AsmKind::Dot,
        AsmTokenKind::Junk(_) => AsmKind::Junk,
        AsmTokenKind::Comment(_) => {
            unreachable!("comments are trivia; the layout adapter never maps them here")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_convert_and_name_round_trip() {
        assert_eq!(kind_name(AsmKind::Word.into()), "WORD");
        assert_eq!(kind_name(AsmKind::Rept.into()), "REPT");
        assert_eq!(kind_name(SyntaxKind(999)), "?");
    }

    #[test]
    fn kinds_are_pairwise_distinct() {
        let all: &[AsmKind] = &[
            AsmKind::Word,
            AsmKind::Number,
            AsmKind::Colon,
            AsmKind::Comma,
            AsmKind::At,
            AsmKind::LBracket,
            AsmKind::RBracket,
            AsmKind::LBrace,
            AsmKind::RBrace,
            AsmKind::LParen,
            AsmKind::RParen,
            AsmKind::Eq,
            AsmKind::Hash,
            AsmKind::Arrow,
            AsmKind::FatArrow,
            AsmKind::Star,
            AsmKind::Dash,
            AsmKind::Plus,
            AsmKind::Percent,
            AsmKind::Lt,
            AsmKind::Gt,
            AsmKind::Dot,
            AsmKind::Junk,
            AsmKind::Whitespace,
            AsmKind::Comment,
            AsmKind::Root,
            AsmKind::Line,
            AsmKind::Func,
            AsmKind::Raw,
            AsmKind::Section,
            AsmKind::TableDirective,
            AsmKind::Rept,
            AsmKind::RoutineDirective,
            AsmKind::Volatile,
            AsmKind::FrameDirective,
        ];
        let mut vals: Vec<u16> = all.iter().map(|k| *k as u16).collect();
        vals.sort_unstable();
        vals.dedup();
        assert_eq!(vals.len(), all.len());
    }
}
