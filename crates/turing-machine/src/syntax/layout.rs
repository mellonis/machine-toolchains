//! Source layout for the green tree: verbatim per-token text and the
//! trivia (whitespace + comments) between tokens, reconstructed from
//! the UNCHANGED lexer's output plus the source text. Token start
//! positions (1-based line, 1-based char column) are trusted; ends are
//! derived per kind and validated by the invariant that everything
//! between two tokens is whitespace. The concatenation of all pieces
//! is the source, byte for byte — the green tree's lossless law
//! starts here. Ported from the sibling `.pmc` crate's pass of the
//! same name.
//!
//! Three `.tmc` token kinds carry a source spelling that differs from
//! their payload — [`TokenKind::Glyph`]'s decoded content,
//! [`TokenKind::Number`]'s parsed value, and the two-character
//! operators' greedy match against their one-character siblings — but
//! the lexer's own [`Token::len`] already counts SOURCE characters for
//! every one of them (quotes and escape backslashes for a glyph, the
//! digit spelling including leading zeros for a number, 2 vs 1 for an
//! operator and its sibling), so the single per-kind end derivation
//! below needs no extra case for any of the three: advancing `len`
//! source characters from the start already lands past the closing
//! quote, past the last written digit, or past the second operator
//! character, exactly as it does for every other token.

use crate::lexer::{Token, TokenKind};

use super::kinds::{TmcKind, token_kind};

pub struct SigLayout {
    /// Verbatim source text of this significant token ("" for Eof).
    pub text: String,
    /// Trivia pieces between the previous significant token and this
    /// one, in source order: whitespace runs and comment tokens.
    pub trivia_before: Vec<(TmcKind, String)>,
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

/// End byte of token `i`: start + `len` chars for ordinary tokens
/// (`len` is already a source-character count for every kind — see
/// the module doc comment for `Glyph`, `Number` and the two-character
/// operators specifically); for doc/attention lines the payload is
/// normalized, so the token runs to the end of its source line
/// instead.
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
    let mut pending: Vec<(TmcKind, String)> = Vec::new();
    let mut cursor = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        let start = starts[i];
        let gap = &source[cursor..start];
        assert!(
            gap.chars().all(char::is_whitespace),
            "non-whitespace between tokens at byte {cursor}: {gap:?}"
        );
        if !gap.is_empty() {
            pending.push((TmcKind::Whitespace, gap.to_string()));
        }
        let end = end_offset(source, t, start);
        let text = &source[start..end];
        cursor = end;
        match &t.kind {
            TokenKind::Comment(c) => {
                debug_assert_eq!(text, c.text, "comment slice vs lexer text");
                pending.push((token_kind(&t.kind), text.to_string()));
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

    /// The concatenation of every piece — each token's verbatim text and
    /// the trivia before it — is the source, byte for byte. Everything
    /// downstream inherits its losslessness from this.
    #[track_caller]
    fn round_trips(src: &str) {
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let entries = layout(src, &tokens);
        let mut out = String::new();
        for e in &entries {
            for (_, t) in &e.trivia_before {
                out.push_str(t);
            }
            out.push_str(&e.text);
        }
        assert_eq!(out, src, "layout is not lossless");
    }

    #[test]
    fn the_pieces_concatenate_to_the_source() {
        round_trips("alphabet ab { '_', 'a' }\n");
        round_trips("machine {\n  tape main: ab;\n}\n");
        round_trips("");
        round_trips("\n\n\n");
    }

    #[test]
    fn comments_and_doc_runs_are_preserved_verbatim() {
        round_trips("// leading\nalphabet ab { '_' }\n");
        round_trips("alphabet ab { '_' } // trailing\n");
        round_trips("/* block\n   spanning */\nalphabet ab { '_' }\n");
        round_trips("? doc line\n! attention\nalphabet ab { '_' }\n");
    }

    #[test]
    fn glyph_and_number_spellings_survive() {
        round_trips("alphabet ab { '_', '\\'', '\\\\' }\n");
        round_trips("machine {\n  tape main: ab;\n  entry state s { [*] -> stop; }\n}\n");
    }

    /// `end_offset`'s generic path advances by CHARACTERS and returns a
    /// BYTE offset — the distinction the whole pass rests on. Every
    /// fixture above is ASCII, where a char count and a byte count
    /// coincide and would hide a fall back to byte arithmetic; a
    /// multi-byte glyph, identifier and comment force the two apart.
    #[test]
    fn multibyte_glyphs_and_identifiers_survive() {
        round_trips("alphabet ab { 'λ', '🎉' } // λ note\nmachine {\n  tape главная: ab;\n}\n");
    }

    #[test]
    fn the_whole_shipped_corpus_round_trips() {
        for dir in ["tests/golden", "src/stdlib"] {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries {
                let path = entry.expect("entry").path();
                if path.extension().and_then(|e| e.to_str()) != Some("tmc") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).expect("readable");
                let tokens = lex_with(&src, LexMode::WithComments).expect("lexes");
                let out: String = layout(&src, &tokens)
                    .iter()
                    .flat_map(|e| {
                        e.trivia_before
                            .iter()
                            .map(|(_, t)| t.clone())
                            .chain(std::iter::once(e.text.clone()))
                    })
                    .collect();
                assert_eq!(out, src, "{} is not lossless", path.display());
            }
        }
    }
}
