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
/// operators specifically). Doc/attention lines get an explicit
/// end-of-line rule instead — not because `len` is unusable there:
/// the lexer computes it from the RAW, unstripped line before
/// normalizing the payload (one leading space dropped after the
/// sigil), so it already counts the same source characters the
/// generic path would advance through, exactly as it does on the
/// sibling `.pmc` side. The special case is kept anyway, ported
/// unchanged from `.pmc`: it decouples this function from the
/// coincidence that `len` still tracks the raw line, so a future
/// lexer change coupling `len` to the normalized payload instead
/// could not silently corrupt this pass.
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

    fn layout_of(src: &str) -> Vec<SigLayout> {
        layout(src, &lex_with(src, LexMode::WithComments).expect("lexes"))
    }

    /// The foundation invariant: trivia + token texts concatenate back
    /// to the source, byte for byte. Ported from the sibling `.pmc`
    /// crate's helper of the same name.
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

    /// `Number`'s spelling length, not its parsed value's width, must
    /// drive the end offset — leading zeros make the two diverge.
    #[test]
    fn number_leading_zeros_survive() {
        round_trips(
            "alphabet bytes { 0..007 }\nmachine {\n  tape cell: bytes;\n  entry state s { [007] -> stop; }\n}\n",
        );
    }

    /// Round-trip concatenation is blind to the TAG half of each trivia
    /// piece — `round_trips` only ever reads the string, never the
    /// `TmcKind` beside it. This is the structural check that closes
    /// that gap: the trivia before the first significant token must be
    /// typed, not just byte-equal.
    #[test]
    fn trivia_pieces_are_typed_and_verbatim() {
        let src = "// c\nalphabet ab { '_' }\n";
        let entries = layout_of(src);
        // First significant token is `alphabet`; its trivia is the
        // comment then the newline.
        assert_eq!(
            entries[0].trivia_before,
            vec![
                (TmcKind::LineComment, "// c".to_string()),
                (TmcKind::Whitespace, "\n".to_string()),
            ]
        );
        assert_eq!(entries[0].text, "alphabet");
    }

    /// Pins the doc/attention-line end boundary in BOTH directions: the
    /// token's own text stops before the trailing newline (not one byte
    /// short, not one byte long), and that newline survives as the next
    /// entry's leading trivia rather than being swallowed into the doc
    /// line's own text — the failure mode a too-long end offset produces
    /// invisibly under the round-trip law alone (it moves the byte
    /// between entries without losing it).
    #[test]
    fn doc_lines_span_to_end_of_line() {
        let src = "? doc  text\nalphabet ab { '_' }\n";
        let entries = layout_of(src);
        assert_eq!(entries[0].text, "? doc  text");
        assert_eq!(
            entries[1].trivia_before,
            vec![(TmcKind::Whitespace, "\n".to_string())]
        );
        assert_eq!(entries[1].text, "alphabet");
    }

    /// No trailing whitespace at all after the last significant token
    /// (`}`) — the Eof entry's `trivia_before` is an empty `Vec`, not,
    /// say, a vector holding a spurious empty-string piece that would
    /// round-trip fine while still being structurally wrong.
    #[test]
    fn no_trailing_newline_eof_trivia_is_empty() {
        let src = "alphabet ab { '_' }";
        let entries = layout_of(src);
        assert_eq!(concat(&entries), src);
        let eof = entries.last().expect("eof entry");
        assert_eq!(eof.text, "");
        assert!(eof.trivia_before.is_empty());
    }

    /// Trailing trivia after the last significant token attaches to the
    /// `Eof` entry, typed and in source order — the round-trip law
    /// already proves those bytes end up somewhere; this proves they
    /// end up on `Eof`, tagged correctly, and nowhere else.
    #[test]
    fn eof_entry_carries_trailing_trivia() {
        let src = "alphabet ab { '_' }\n// tail\n";
        let entries = layout_of(src);
        let eof = entries.last().expect("eof entry");
        assert_eq!(eof.text, "");
        assert_eq!(
            eof.trivia_before,
            vec![
                (TmcKind::Whitespace, "\n".to_string()),
                (TmcKind::LineComment, "// tail".to_string()),
                (TmcKind::Whitespace, "\n".to_string()),
            ]
        );
        assert_eq!(concat(&entries), src);
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
