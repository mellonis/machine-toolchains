//! Source layout for the green tree: verbatim per-token text and the
//! trivia (whitespace + comments) between tokens, reconstructed from
//! the UNCHANGED lexer's output plus the source text. Token start
//! positions (1-based line, 1-based char column) are trusted; ends are
//! derived per kind and validated by the invariant that everything
//! between two tokens is whitespace. The concatenation of all pieces
//! is the source, byte for byte — the green tree's lossless law starts
//! here.

use crate::lexer::{CommentKind, Token, TokenKind};

use super::kinds::PmcKind;

pub struct SigLayout {
    /// Verbatim source text of this significant token ("" for Eof).
    pub text: String,
    /// Trivia pieces between the previous significant token and this
    /// one, in source order: whitespace runs and comment tokens.
    pub trivia_before: Vec<(PmcKind, String)>,
}

/// Byte offset of each (line, col) token start, computed by one pass
/// over the source tracking 1-based line and char column.
fn start_offsets(source: &str, tokens: &[Token]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(tokens.len());
    let mut ti = 0;
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    for (byte, ch) in source.char_indices() {
        while ti < tokens.len() && tokens[ti].line == line && tokens[ti].col == col {
            offsets.push(byte);
            ti += 1;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    // Eof (and any token starting exactly at end-of-text).
    while ti < tokens.len() {
        assert!(
            matches!(tokens[ti].kind, TokenKind::Eof),
            "unplaced non-Eof token at {}:{}",
            tokens[ti].line,
            tokens[ti].col
        );
        offsets.push(source.len());
        ti += 1;
    }
    offsets
}

/// End byte of token `i`: start + `len` chars for ordinary tokens and
/// comments (`Comment::text` is verbatim, `len` counts its chars); for
/// doc/attention lines the payload is normalized, so the token runs to
/// the end of its source line instead.
fn end_offset(source: &str, token: &Token, start: usize) -> usize {
    match &token.kind {
        TokenKind::DocLine(_) | TokenKind::AttentionLine(_) => source[start..]
            .find('\n')
            .map(|nl| start + nl)
            .unwrap_or(source.len()),
        TokenKind::Eof => start,
        _ => {
            let mut it = source[start..].char_indices();
            for _ in 0..token.len {
                it.next();
            }
            it.next().map(|(o, _)| start + o).unwrap_or(source.len())
        }
    }
}

pub fn layout(source: &str, tokens: &[Token]) -> Vec<SigLayout> {
    let starts = start_offsets(source, tokens);
    let mut entries = Vec::new();
    let mut pending: Vec<(PmcKind, String)> = Vec::new();
    let mut cursor = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        let start = starts[i];
        let gap = &source[cursor..start];
        assert!(
            gap.chars().all(char::is_whitespace),
            "non-whitespace between tokens at byte {cursor}: {gap:?}"
        );
        if !gap.is_empty() {
            pending.push((PmcKind::Whitespace, gap.to_string()));
        }
        let end = end_offset(source, t, start);
        let text = &source[start..end];
        cursor = end;
        match &t.kind {
            TokenKind::Comment(c) => {
                debug_assert_eq!(text, c.text, "comment slice vs lexer text");
                let kind = match c.kind {
                    CommentKind::Line => PmcKind::LineComment,
                    CommentKind::Block => PmcKind::BlockComment,
                };
                pending.push((kind, text.to_string()));
            }
            _ => {
                entries.push(SigLayout {
                    text: text.to_string(),
                    trivia_before: std::mem::take(&mut pending),
                });
            }
        }
    }
    assert!(pending.is_empty(), "trivia after Eof");
    assert_eq!(cursor, source.len(), "source tail not covered");
    entries
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
