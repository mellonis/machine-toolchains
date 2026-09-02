//! Core `Pos`/`Span` (1-based line and character column) → UTF-16 offsets,
//! the coordinate system a browser editor indexes strings by.

use mtc_core::diagnostics::{Pos, Span};

/// Built once per source text; `offset` is then O(line length).
pub struct Utf16Index {
    text: String,
    /// Byte offset where each line starts (line i is `line_bytes[i]..`).
    line_bytes: Vec<usize>,
    /// UTF-16 offset where each line starts.
    line_units: Vec<u32>,
    /// Total length in UTF-16 units.
    len_units: u32,
}

impl Utf16Index {
    pub fn new(text: &str) -> Utf16Index {
        let mut line_bytes = vec![0];
        let mut line_units = vec![0];
        let mut units: u32 = 0;
        for (byte, ch) in text.char_indices() {
            units += ch.len_utf16() as u32;
            if ch == '\n' {
                line_bytes.push(byte + 1);
                line_units.push(units);
            }
        }
        Utf16Index {
            text: text.to_owned(),
            line_bytes,
            line_units,
            len_units: units,
        }
    }

    pub fn len(&self) -> u32 {
        self.len_units
    }

    pub fn is_empty(&self) -> bool {
        self.len_units == 0
    }

    /// Total: a line past the end lands at the text end; a column past the
    /// line end lands at the line end (the newline excluded, so a span
    /// ending at end-of-line never swallows it).
    pub fn offset(&self, pos: Pos) -> u32 {
        if pos.line == 0 {
            return 0;
        }
        let line = (pos.line - 1) as usize;
        if line >= self.line_bytes.len() {
            return self.len_units;
        }
        let start = self.line_bytes[line];
        let end = self
            .line_bytes
            .get(line + 1)
            .map(|next| next - 1) // exclude the '\n'
            .unwrap_or(self.text.len());
        let chars_before = pos.col.saturating_sub(1) as usize;
        let units: u32 = self.text[start..end]
            .chars()
            .take(chars_before)
            .map(|c| c.len_utf16() as u32)
            .sum();
        self.line_units[line] + units
    }

    /// Half-open `(from, to)`, normalised so `from <= to`.
    pub fn span(&self, span: &Span) -> (u32, u32) {
        let a = self.offset(span.start);
        let b = self.offset(span.end);
        if a <= b { (a, b) } else { (b, a) }
    }
}
