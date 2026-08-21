//! Byte-offset → line/column conversion for diagnostics — docs/core.md (syntax tree).
//! Lines are 1-based; columns are 1-based CHARACTER counts — the convention
//! the lexers' tokens already carry (`Token.len` is chars), so spans built
//! through a `TextLineIndex` are byte-identical to lexer-built spans for the
//! single-line tokens the lexers produce. Span parity across the front-end
//! migrations is a test-pinned contract.
//!
//! Deliberate tradeoffs at this toolchain's file scale: the index owns
//! a full copy of the source text (so callers don't have to keep the
//! original borrow alive), and `line_col`'s column is an O(line-length)
//! char scan rather than a precomputed table.

use crate::diagnostics::{Pos, Span};

use super::red::TextRange;

pub struct TextLineIndex {
    text: String,
    /// Byte offset of each line's first byte; `line_starts[0] == 0`.
    line_starts: Vec<u32>,
}

impl TextLineIndex {
    pub fn new(text: &str) -> TextLineIndex {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        TextLineIndex {
            text: text.to_owned(),
            line_starts,
        }
    }

    /// 1-based (line, char-column) of a byte offset. The offset must
    /// lie on a char boundary; it must also be `<= text.len()` — it
    /// panics past the end. The end-of-text offset itself is valid.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let line_ix = self.line_starts.partition_point(|&s| s <= offset) - 1;
        let line_start = self.line_starts[line_ix] as usize;
        let col = self.text[line_start..offset as usize].chars().count() as u32 + 1;
        (line_ix as u32 + 1, col)
    }

    /// Byte offset of a 1-based (line, char-column) position — the
    /// inverse of [`TextLineIndex::line_col`], and total where
    /// `line_col` is partial.
    ///
    /// It clamps exactly the way `crate::lsp::position::pos_from_lsp`
    /// clamps, because that function is what produces every `Pos` a
    /// language service ever sees: a line past the last one yields the
    /// end-of-text offset, and a column at or past a line's last
    /// character yields that line's end — the newline's own offset, or
    /// end-of-text on the final line. The end-of-line column (one past
    /// the last character) is the commonest cursor position an editor
    /// sends, so it resolves rather than failing; that is why this
    /// returns `u32` and not `Option<u32>`.
    pub fn offset(&self, pos: Pos) -> u32 {
        let text_len = self.text.len() as u32;
        if pos.line == 0 || pos.line as usize > self.line_starts.len() {
            return text_len;
        }
        let line_ix = (pos.line - 1) as usize;
        let line_start = self.line_starts[line_ix];
        // The line's end excludes its own `\n`; the last line ends at
        // end-of-text. `new` splits on `\n` only, so a `\r` stays part
        // of the line content — matching `line_col`'s own view.
        let line_end = self
            .line_starts
            .get(line_ix + 1)
            .map_or(text_len, |&next| next - 1);
        let mut offset = line_start;
        for (col, ch) in (1u32..).zip(self.text[line_start as usize..line_end as usize].chars()) {
            if col >= pos.col {
                return offset;
            }
            offset += ch.len_utf8() as u32;
        }
        offset
    }

    /// The `Span` of a byte range — end-exclusive columns, matching the
    /// lexer's `Token::span` convention.
    pub fn span(&self, range: TextRange) -> Span {
        let (sl, sc) = self.line_col(range.start);
        let (el, ec) = self.line_col(range.end);
        Span::new(sl, sc, el, ec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::TextRange;

    #[test]
    fn lines_and_columns_are_one_based() {
        let idx = TextLineIndex::new("ab\ncd\n");
        assert_eq!(idx.line_col(0), (1, 1));
        assert_eq!(idx.line_col(1), (1, 2));
        assert_eq!(idx.line_col(2), (1, 3)); // the '\n' itself: end-exclusive col
        assert_eq!(idx.line_col(3), (2, 1));
        assert_eq!(idx.line_col(6), (3, 1)); // end-of-text after trailing newline
    }

    #[test]
    fn columns_count_chars_not_bytes() {
        // "λx" — λ is 2 bytes, 1 char; the lexer's Token.len counts
        // chars, so span parity requires char columns.
        let idx = TextLineIndex::new("λx");
        assert_eq!(idx.line_col(0), (1, 1));
        assert_eq!(idx.line_col(2), (1, 2)); // byte offset 2 = after λ
        assert_eq!(idx.line_col(3), (1, 3)); // end of text
    }

    #[test]
    fn span_matches_the_lexers_end_exclusive_convention() {
        // Token "cd" on line 2 at col 1: the lexer builds
        // Span::new(2, 1, 2, 3); the byte range through TextLineIndex must
        // produce the identical Span.
        let idx = TextLineIndex::new("ab\ncd\n");
        let span = idx.span(TextRange::new(3, 5));
        assert_eq!(span, crate::diagnostics::Span::new(2, 1, 2, 3));
    }

    #[test]
    fn empty_text_has_one_line() {
        let idx = TextLineIndex::new("");
        assert_eq!(idx.line_col(0), (1, 1));
    }

    #[test]
    fn offset_inverts_line_col_at_every_char_boundary() {
        // Round-trip over a text with a trailing newline, a blank line,
        // and a multi-byte character — the three shapes where a naive
        // byte/char mix-up shows.
        let text = "ab\n\nжc\n";
        let idx = TextLineIndex::new(text);
        for (offset, _) in text
            .char_indices()
            .chain(std::iter::once((text.len(), '\0')))
        {
            let offset = offset as u32;
            let (line, col) = idx.line_col(offset);
            assert_eq!(
                idx.offset(Pos { line, col }),
                offset,
                "round trip at byte {offset} ({line}:{col})"
            );
        }
    }

    #[test]
    fn offset_clamps_a_column_past_the_line_end_to_that_line_end() {
        // The end-of-line cursor: `pos_from_lsp` hands us col == chars+1,
        // and anything beyond must land on the same place — the newline's
        // own offset, never the next line.
        let idx = TextLineIndex::new("ab\ncd\n");
        assert_eq!(
            idx.offset(Pos { line: 1, col: 3 }),
            2,
            "one past 'b' is the \\n"
        );
        assert_eq!(
            idx.offset(Pos { line: 1, col: 99 }),
            2,
            "far past clamps the same"
        );
        assert_eq!(idx.offset(Pos { line: 2, col: 3 }), 5);
    }

    #[test]
    fn offset_clamps_a_line_past_the_end_to_end_of_text() {
        let idx = TextLineIndex::new("ab\ncd\n");
        assert_eq!(idx.offset(Pos { line: 99, col: 1 }), 6);
        // Line 3 exists (the empty line after the trailing newline).
        assert_eq!(idx.offset(Pos { line: 3, col: 1 }), 6);
    }

    #[test]
    fn offset_handles_an_empty_document() {
        let idx = TextLineIndex::new("");
        assert_eq!(idx.offset(Pos { line: 1, col: 1 }), 0);
        assert_eq!(idx.offset(Pos { line: 9, col: 9 }), 0);
    }
}
