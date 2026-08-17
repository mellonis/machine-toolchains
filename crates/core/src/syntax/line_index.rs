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

use crate::diagnostics::Span;

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
}
