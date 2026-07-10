//! Position mapping between the toolchain's 1-based, character-counted
//! `Pos`/`Span` (`crate::diagnostics`) and LSP's 0-based, UTF-16-counted
//! `Position`/`Range` (`super::types`). The single place char↔UTF-16
//! conversion lives; the server loop (docs/lsp.md) converts every span
//! through it against the document's CURRENT text.
//!
//! No byte arithmetic anywhere: lines come from `text.split('\n')` (a
//! trailing `'\r'` is excluded from the countable content, so CRLF
//! documents are tolerated), and columns are counted by walking
//! `chars()` and accumulating `char::len_utf16()`.
//!
//! Out-of-range input clamps per the LSP convention: a column past a
//! line's end clamps to that line's end; a line past end-of-file
//! clamps to one past the last line's end (the column/character
//! component of the input is disregarded in that case, since there is
//! no such line to measure it against). A UTF-16 offset landing inside
//! a surrogate pair snaps to the character's start.

use crate::diagnostics::{Pos, Span};
use crate::lsp::types::{Position, Range};

/// Splits `text` into lines the same way on every call site: `'\n'`
/// terminated, with a trailing `'\r'` stripped from the countable
/// content. Always yields at least one line (the empty string splits
/// to `[""]`).
fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// Strips a single trailing `'\r'` (CRLF tolerance), leaving the rest
/// of the line untouched.
fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// The UTF-16 length of a line (CR already excluded).
fn line_utf16_len(line: &str) -> u32 {
    strip_cr(line).chars().map(|ch| ch.len_utf16() as u32).sum()
}

/// The character count of a line (CR already excluded).
fn line_char_len(line: &str) -> u32 {
    strip_cr(line).chars().count() as u32
}

/// `Pos` (1-based line, 1-based char col) → `Position` (0-based line,
/// UTF-16 col), against the current text. Out-of-range input clamps
/// (per LSP): col past end-of-line → line end; line past end-of-file →
/// one past the last line's end.
pub fn pos_to_lsp(text: &str, pos: Pos) -> Position {
    let lines = split_lines(text);
    let line_count = lines.len() as u32;

    if pos.line == 0 {
        // Below the valid 1-based range: clamp to the very start.
        return Position {
            line: 0,
            character: 0,
        };
    }
    if pos.line > line_count {
        // Past end-of-file: one past the last line's end, regardless
        // of the requested column.
        let last0 = line_count - 1;
        return Position {
            line: last0,
            character: line_utf16_len(lines[last0 as usize]),
        };
    }

    let line0 = pos.line - 1;
    let content = strip_cr(lines[line0 as usize]);
    let target_chars = pos.col.saturating_sub(1);

    let mut utf16 = 0u32;
    for (char_count, ch) in content.chars().enumerate() {
        if char_count as u32 == target_chars {
            return Position {
                line: line0,
                character: utf16,
            };
        }
        utf16 += ch.len_utf16() as u32;
    }
    // Column was at or past the line's end: clamp to the line end.
    Position {
        line: line0,
        character: utf16,
    }
}

/// Inverse of [`pos_to_lsp`]; clamps the same way. UTF-16 offsets
/// landing inside a surrogate pair snap to the character start.
pub fn pos_from_lsp(text: &str, position: Position) -> Pos {
    let lines = split_lines(text);
    let line_count = lines.len() as u32;

    if position.line >= line_count {
        // Past end-of-file: one past the last line's end, regardless
        // of the requested character offset.
        let last0 = line_count - 1;
        return Pos {
            line: last0 + 1,
            col: line_char_len(lines[last0 as usize]) + 1,
        };
    }

    let content = strip_cr(lines[position.line as usize]);
    let mut utf16_start = 0u32;
    let mut char_idx = 0u32;
    for ch in content.chars() {
        let units = ch.len_utf16() as u32;
        if position.character < utf16_start + units {
            // Target falls at or inside this character's UTF-16 span
            // (the latter only for a surrogate pair) — snap to its
            // start either way.
            return Pos {
                line: position.line + 1,
                col: char_idx + 1,
            };
        }
        utf16_start += units;
        char_idx += 1;
    }
    // Character offset was at or past the line's end: clamp to the
    // line end.
    Pos {
        line: position.line + 1,
        col: char_idx + 1,
    }
}

/// Maps both endpoints of a half-open `Span` to a half-open `Range`.
pub fn span_to_range(text: &str, span: Span) -> Range {
    Range {
        start: pos_to_lsp(text, span.start),
        end: pos_to_lsp(text, span.end),
    }
}

/// Maps both endpoints of a half-open `Range` to a half-open `Span`.
pub fn range_to_span(text: &str, range: Range) -> Span {
    Span {
        start: pos_from_lsp(text, range.start),
        end: pos_from_lsp(text, range.end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn pos(line: u32, col: u32) -> Pos {
        Pos { line, col }
    }

    fn position(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn ascii_two_lines_maps_both_ways() {
        let text = "abc\ndef";
        assert_eq!(pos_to_lsp(text, pos(2, 2)), position(1, 1));
        assert_eq!(pos_from_lsp(text, position(1, 1)), pos(2, 2));
    }

    #[test]
    fn cyrillic_counts_chars_not_bytes() {
        // "привет x": 8 characters, each 1 UTF-16 unit (BMP); `x` is
        // the 8th char. `п` is 2 bytes — byte-counting would be wrong.
        let text = "привет x";
        assert_eq!(pos_to_lsp(text, pos(1, 8)), position(0, 7));
        assert_eq!(pos_from_lsp(text, position(0, 7)), pos(1, 8));
    }

    #[test]
    fn astral_emoji_counts_two_utf16_units_as_one_char() {
        // "😀x": `😀` is one char but two UTF-16 units; `x` is the 2nd
        // char.
        let text = "😀x";
        assert_eq!(pos_to_lsp(text, pos(1, 2)), position(0, 2));
        assert_eq!(pos_from_lsp(text, position(0, 2)), pos(1, 2));
    }

    #[test]
    fn column_past_end_of_line_clamps_to_line_end() {
        let text = "abc";
        // 3-char line, col 99 clamps to the line end (character 3).
        assert_eq!(pos_to_lsp(text, pos(1, 99)), position(0, 3));
        // And the reverse: a UTF-16 offset past the line end clamps
        // to one past the last character.
        assert_eq!(pos_from_lsp(text, position(0, 99)), pos(1, 4));
    }

    #[test]
    fn line_past_end_of_file_clamps_to_one_past_the_last_lines_end() {
        let text = "ab\ncd";
        // 2-line file, line 99 clamps to the end of the last line,
        // ignoring the requested column.
        assert_eq!(pos_to_lsp(text, pos(99, 1)), position(1, 2));
        // And the reverse: an out-of-range 0-based line clamps to one
        // past the last line's last character, ignoring the requested
        // character offset.
        assert_eq!(pos_from_lsp(text, position(99, 0)), pos(2, 3));
    }

    #[test]
    fn mid_surrogate_offset_snaps_to_the_character_start() {
        // "😀x": `😀` occupies UTF-16 offsets [0, 2). Offset 1 lands on
        // its low surrogate; it must snap back to the character's
        // start (col 1), not round up past it.
        let text = "😀x";
        assert_eq!(pos_from_lsp(text, position(0, 1)), pos(1, 1));
    }

    #[test]
    fn crlf_line_excludes_trailing_cr_from_the_count() {
        let text = "ab\r\ncd";
        // Line 1 is "ab\r": the countable content is "ab" (2 chars),
        // so col 3 (one past the end) is the last valid column, and
        // it maps to UTF-16 offset 2, not 3.
        assert_eq!(pos_to_lsp(text, pos(1, 3)), position(0, 2));
        assert_eq!(pos_from_lsp(text, position(0, 2)), pos(1, 3));
    }

    #[test]
    fn span_to_range_maps_both_endpoints() {
        let text = "abc\nпривет x\n😀x";
        let span = Span::new(1, 2, 3, 2);

        let range = span_to_range(text, span);
        assert_eq!(
            range,
            Range {
                start: position(0, 1),
                end: position(2, 2),
            }
        );
        assert_eq!(range_to_span(text, range), span);
    }

    proptest! {
        #[test]
        fn round_trips_every_valid_position(text in "[a-zа-я😀\n]{0,40}") {
            let lines: Vec<&str> = text.split('\n').collect();
            for (line_ix, line) in lines.iter().enumerate() {
                let content = strip_cr(line);
                let char_len = content.chars().count() as u32;
                // Every column from the line start through one past
                // its last character is a valid caret position.
                for col in 1..=(char_len + 1) {
                    let p = Pos {
                        line: (line_ix + 1) as u32,
                        col,
                    };
                    let lsp = pos_to_lsp(&text, p);
                    prop_assert_eq!(pos_from_lsp(&text, lsp), p);
                }
            }
        }
    }
}
