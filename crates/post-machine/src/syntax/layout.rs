//! Source layout for the green tree — the `.pmc` adapter over core's
//! language-agnostic layout pass (docs/core.md (syntax trees)): builds
//! per-token [`LayoutToken`] facts from the UNCHANGED lexer's output
//! and lets the shared skeleton reconstruct verbatim text and trivia.
//! What is `.pmc`-specific lives entirely here: which token kinds are
//! trivia (comments, mapped to their line/block kinds), and which end
//! rule each kind takes — doc/attention lines run to end-of-line
//! (their payload is normalized, so no char count exists), Eof is
//! empty at its start, everything else advances the lexer's own
//! char-counted `len`.

use mtc_core::syntax::{EndRule, LayoutToken, TokenClass};

use crate::lexer::{CommentKind, Token, TokenKind};

use super::kinds::PmcKind;

/// The `.pmc` layout entry — core's, with this crate's kind space.
pub type SigLayout = mtc_core::syntax::SigLayout<PmcKind>;

pub fn layout(source: &str, tokens: &[Token]) -> Vec<SigLayout> {
    let facts: Vec<LayoutToken<PmcKind>> = tokens
        .iter()
        .map(|t| LayoutToken {
            line: t.line,
            col: t.col,
            end: match &t.kind {
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_) => EndRule::ToLineEnd,
                TokenKind::Eof => EndRule::AtStart,
                _ => EndRule::Chars(t.len),
            },
            class: match &t.kind {
                TokenKind::Comment(c) => TokenClass::Trivia(match c.kind {
                    CommentKind::Line => PmcKind::LineComment,
                    CommentKind::Block => PmcKind::BlockComment,
                }),
                _ => TokenClass::Significant,
            },
        })
        .collect();
    mtc_core::syntax::layout(source, &facts, PmcKind::Whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};
    use crate::syntax::PmcKind;

    fn layout_of(src: &str) -> Vec<SigLayout> {
        layout(src, &lex_with(src, LexMode::WithComments).expect("lexes"))
    }

    /// The foundation invariant: trivia + token texts concatenate back
    /// to the source, byte for byte.
    fn concat(entries: &[SigLayout]) -> String {
        let mut out = String::new();
        for e in entries {
            for (_, t) in &e.trivia_before {
                out.push_str(t);
            }
            out.push_str(&e.text);
        }
        out
    }

    #[test]
    fn concat_reproduces_source_with_comments_and_multibyte() {
        let src = "use std::goToEnd; // λ note\nmain() {\n  right; /* b\nlock */ left;\n}\n";
        let entries = layout_of(src);
        assert_eq!(concat(&entries), src);
    }

    #[test]
    fn trivia_pieces_are_typed_and_verbatim() {
        let src = "// c\nmain() { right; }\n";
        let entries = layout_of(src);
        // First significant token is `main`; its trivia is the comment
        // then the newline.
        assert_eq!(
            entries[0].trivia_before,
            vec![
                (PmcKind::LineComment, "// c".to_string()),
                (PmcKind::Whitespace, "\n".to_string()),
            ]
        );
        assert_eq!(entries[0].text, "main");
    }

    #[test]
    fn doc_lines_span_to_end_of_line() {
        // DocLine payloads are normalized (sigil + one space stripped),
        // so verbatim text comes from the end-of-line rule, never the
        // payload.
        let src = "? doc  text\nmain() { right; }\n";
        let entries = layout_of(src);
        assert_eq!(entries[0].text, "? doc  text");
        assert_eq!(
            entries[1].trivia_before,
            vec![(PmcKind::Whitespace, "\n".to_string())]
        );
    }

    #[test]
    fn no_trailing_newline_eof_trivia_is_empty() {
        // No trailing whitespace at all after the last significant token
        // (`}`) — the Eof entry's `trivia_before` schedule is empty, and
        // the concatenation law still holds with nothing left over.
        let src = "main() { right; }";
        let entries = layout_of(src);
        assert_eq!(concat(&entries), src);
        let eof = entries.last().expect("eof entry");
        assert_eq!(eof.text, "");
        assert!(eof.trivia_before.is_empty());
    }

    #[test]
    fn eof_entry_carries_trailing_trivia() {
        let src = "main() { right; }\n// tail\n";
        let entries = layout_of(src);
        let eof = entries.last().expect("eof entry");
        assert_eq!(eof.text, "");
        assert_eq!(
            eof.trivia_before,
            vec![
                (PmcKind::Whitespace, "\n".to_string()),
                (PmcKind::LineComment, "// tail".to_string()),
                (PmcKind::Whitespace, "\n".to_string()),
            ]
        );
        assert_eq!(concat(&entries), src);
    }
}
