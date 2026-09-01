//! Source layout for the green tree — the `.tmc` adapter over core's
//! language-agnostic layout pass (docs/core.md (syntax trees)): builds
//! per-token [`LayoutToken`] facts from the UNCHANGED lexer's output
//! and lets the shared skeleton reconstruct verbatim text and trivia.
//! What is `.tmc`-specific lives entirely here: which token kinds are
//! trivia (comments, mapped through [`token_kind`]), and which end
//! rule each kind takes.
//!
//! Three `.tmc` token kinds carry a source spelling that differs from
//! their payload — [`TokenKind::Glyph`]'s decoded content,
//! [`TokenKind::Number`]'s parsed value, and the two-character
//! operators' greedy match against their one-character siblings — but
//! the lexer's own [`Token::len`] already counts SOURCE characters for
//! every one of them (quotes and escape backslashes for a glyph, the
//! digit spelling including leading zeros for a number, 2 vs 1 for an
//! operator and its sibling), so the per-kind end derivation below
//! needs no extra case for any of the three: advancing `len` source
//! characters from the start already lands past the closing quote,
//! past the last written digit, or past the second operator character.
//!
//! Doc/attention lines get the explicit end-of-line rule instead — not
//! because `len` is unusable there: the lexer computes it from the
//! RAW, unstripped line before normalizing the payload, so it already
//! counts the same source characters. The special case decouples this
//! pass from that coincidence, so a future lexer change coupling `len`
//! to the normalized payload could not silently corrupt it.

use mtc_core::syntax::{EndRule, LayoutToken, TokenClass};

use crate::lexer::{Token, TokenKind};

use super::kinds::{TmcKind, token_kind};

/// The `.tmc` layout entry — core's, with this crate's kind space.
pub type SigLayout = mtc_core::syntax::SigLayout<TmcKind>;

pub fn layout(source: &str, tokens: &[Token]) -> Vec<SigLayout> {
    let facts: Vec<LayoutToken<TmcKind>> = tokens
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
                TokenKind::Comment(_) => TokenClass::Trivia(token_kind(&t.kind)),
                _ => TokenClass::Significant,
            },
        })
        .collect();
    mtc_core::syntax::layout(source, &facts, TmcKind::Whitespace)
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
    /// typed, not just byte-equal. Both comment tags are asserted, not
    /// just one — a source with only a line comment leaves the
    /// block-comment mapping unguarded in the direction that matters
    /// (mapping a real block comment to the wrong tag), even though the
    /// reverse mutation (a line comment mistagged as block) is already
    /// caught elsewhere by the two fixtures below that pin
    /// `TmcKind::LineComment` directly.
    #[test]
    fn trivia_pieces_are_typed_and_verbatim() {
        let src = "// c\n/* b */\nalphabet ab { '_' }\n";
        let entries = layout_of(src);
        // First significant token is `alphabet`; its trivia is the line
        // comment, a newline, the block comment, then another newline.
        assert_eq!(
            entries[0].trivia_before,
            vec![
                (TmcKind::LineComment, "// c".to_string()),
                (TmcKind::Whitespace, "\n".to_string()),
                (TmcKind::BlockComment, "/* b */".to_string()),
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
        let mut checked = 0;
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
                checked += 1;
            }
        }
        // 7 golden programs plus the embedded stdlib at the time of
        // writing — narrower than `syntax_green.rs`'s twin corpus test,
        // which additionally walks `../../docs/examples` (9 there vs 8
        // here). A failing `read_dir` must not make this pass by doing
        // nothing.
        assert!(
            checked >= 8,
            "expected the whole .tmc corpus, saw {checked}"
        );
    }
}
