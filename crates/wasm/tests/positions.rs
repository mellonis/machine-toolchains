//! Core spans are 1-based (line, character column); the browser editor
//! wants half-open UTF-16 string offsets. These pin the conversion where
//! the two disagree: astral glyphs, CRLF, and positions past the end.

use mtc_core::diagnostics::{Pos, Span};
use mtc_wasm::inner::positions::Utf16Index;

fn pos(line: u32, col: u32) -> Pos {
    Pos { line, col }
}

#[test]
fn ascii_lines_are_plain_arithmetic() {
    let idx = Utf16Index::new("ab\ncde\n");
    assert_eq!(idx.offset(pos(1, 1)), 0);
    assert_eq!(idx.offset(pos(1, 3)), 2); // one past 'b' — a span end
    assert_eq!(idx.offset(pos(2, 1)), 3);
    assert_eq!(idx.offset(pos(2, 4)), 6);
    assert_eq!(idx.len(), 7);
}

#[test]
fn astral_glyph_counts_two_units() {
    // '𝔞' (U+1D51E) is one character and two UTF-16 code units.
    let idx = Utf16Index::new("x𝔞y\nz");
    assert_eq!(idx.offset(pos(1, 2)), 1); // before the glyph
    assert_eq!(idx.offset(pos(1, 3)), 3); // after the glyph: 1 + 2
    assert_eq!(idx.offset(pos(1, 4)), 4);
    assert_eq!(idx.offset(pos(2, 1)), 5); // the newline is one unit
}

#[test]
fn crlf_keeps_the_carriage_return_in_the_line() {
    let idx = Utf16Index::new("ab\r\ncd");
    assert_eq!(idx.offset(pos(2, 1)), 4);
    assert_eq!(idx.offset(pos(1, 3)), 2); // the '\r' is column 3, still line 1
}

#[test]
fn positions_past_the_end_clamp() {
    let idx = Utf16Index::new("ab\ncd");
    assert_eq!(idx.offset(pos(2, 99)), 5);
    assert_eq!(idx.offset(pos(99, 1)), 5);
    assert_eq!(
        idx.offset(pos(0, 0)),
        0,
        "a zero position clamps to the start"
    );
}

#[test]
fn span_is_half_open_and_ordered() {
    let idx = Utf16Index::new("hello\nworld\n");
    let (from, to) = idx.span(&Span {
        start: pos(2, 1),
        end: pos(2, 6),
    });
    assert_eq!((from, to), (6, 11));
    let (from, to) = idx.span(&Span {
        start: pos(2, 6),
        end: pos(2, 1),
    });
    assert_eq!(
        (from, to),
        (6, 11),
        "a reversed span is normalised, never negative"
    );
}
