//! Glyph-list notation — the surface both tape-block CLIs use to name a
//! tape's symbols. Deliberately identical to the alphabet-element syntax of
//! the architecture source languages, so a list copy-pastes out of a program
//! and inclusive ranges come along for free (docs/formats.md (glyph tables)).
//!
//! Arch-agnostic by contract: a glyph list is presentation data attached to a
//! tape block, and this module knows nothing about any architecture.

use std::collections::HashSet;
use std::fmt;

/// The most glyphs one tape may distinguish. The compact symbol family caps
/// an alphabet at 127 (docs/formats.md (the compact symbol family)).
const MAX_GLYPHS: usize = 127;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlyphListError {
    Empty,
    ExpectedElement,
    UnterminatedLiteral,
    InvalidEscape(char),
    BadNumber(String),
    RangeKindMismatch,
    RangeDescending,
    RangeEndpointNotScalar,
    Duplicate(String),
    TooMany(usize),
}

impl fmt::Display for GlyphListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty glyph list"),
            Self::ExpectedElement => write!(f, "expected a glyph or number"),
            Self::UnterminatedLiteral => write!(f, "unterminated glyph literal"),
            Self::InvalidEscape(c) => write!(
                f,
                "invalid escape `\\{c}` in glyph literal — only `\\'` and `\\\\` are allowed"
            ),
            Self::BadNumber(t) => write!(f, "bad number `{t}`"),
            Self::RangeKindMismatch => write!(f, "range endpoints must be the same kind"),
            Self::RangeDescending => write!(f, "range endpoints must ascend"),
            Self::RangeEndpointNotScalar => {
                write!(f, "a glyph range endpoint must be a single character")
            }
            Self::Duplicate(g) => write!(f, "duplicate glyph `{g}`"),
            Self::TooMany(n) => write!(f, "{n} glyphs: at most {MAX_GLYPHS} are allowed"),
        }
    }
}

/// One parsed element, before range expansion.
enum Lit {
    Glyph(String),
    Number(u32),
}

impl Lit {
    /// The label this literal contributes. A number's identity is its VALUE,
    /// so `05` and `5` both label `"5"`.
    fn label(&self) -> String {
        match self {
            Self::Glyph(v) => v.clone(),
            Self::Number(v) => v.to_string(),
        }
    }
}

/// Parse a comma-separated glyph list as an ALPHABET: glyphs are unique and
/// at most [`MAX_GLYPHS`] of them. Index 0 is the blank by convention; this
/// parser imposes no meaning on it.
///
/// For a run of tape CELLS use [`parse_glyph_sequence`] — cells are a
/// sequence, not a set, so neither restriction applies there.
pub fn parse_glyph_list(text: &str) -> Result<Vec<String>, GlyphListError> {
    let glyphs = parse_glyph_sequence(text)?;
    let mut seen: HashSet<&str> = HashSet::new();
    for glyph in &glyphs {
        if !seen.insert(glyph.as_str()) {
            return Err(GlyphListError::Duplicate(glyph.clone()));
        }
    }
    if glyphs.len() > MAX_GLYPHS {
        return Err(GlyphListError::TooMany(glyphs.len()));
    }
    Ok(glyphs)
}

/// Parse the same notation as a SEQUENCE: repeats are expected (a tape run
/// like `'1','1','1'` is ordinary) and there is no length cap — an alphabet
/// is bounded at 127 symbols, but the tape written over it is not.
pub fn parse_glyph_sequence(text: &str) -> Result<Vec<String>, GlyphListError> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0usize;
    let mut glyphs: Vec<String> = Vec::new();

    skip_spaces(&chars, &mut at);
    if at == chars.len() {
        return Err(GlyphListError::Empty);
    }

    loop {
        let lo = parse_lit(&chars, &mut at)?;
        skip_spaces(&chars, &mut at);

        if chars.get(at) == Some(&'.') && chars.get(at + 1) == Some(&'.') {
            at += 2;
            skip_spaces(&chars, &mut at);
            let hi = parse_lit(&chars, &mut at)?;
            glyphs.extend(expand_range(&lo, &hi)?);
        } else {
            glyphs.push(lo.label());
        }

        skip_spaces(&chars, &mut at);
        match chars.get(at) {
            Some(',') => {
                at += 1;
                skip_spaces(&chars, &mut at);
            }
            Some(_) => return Err(GlyphListError::ExpectedElement),
            None => break,
        }
    }

    Ok(glyphs)
}

fn skip_spaces(chars: &[char], at: &mut usize) {
    while matches!(chars.get(*at), Some(c) if c.is_whitespace()) {
        *at += 1;
    }
}

fn parse_lit(chars: &[char], at: &mut usize) -> Result<Lit, GlyphListError> {
    match chars.get(*at) {
        Some('\'') => {
            *at += 1;
            let mut value = String::new();
            loop {
                match chars.get(*at) {
                    None => return Err(GlyphListError::UnterminatedLiteral),
                    Some('\'') => {
                        *at += 1;
                        return Ok(Lit::Glyph(value));
                    }
                    Some('\\') => {
                        *at += 1;
                        match chars.get(*at) {
                            Some('\'') => value.push('\''),
                            Some('\\') => value.push('\\'),
                            None => return Err(GlyphListError::UnterminatedLiteral),
                            Some(bad) => return Err(GlyphListError::InvalidEscape(*bad)),
                        }
                        *at += 1;
                    }
                    Some(c) => {
                        value.push(*c);
                        *at += 1;
                    }
                }
            }
        }
        Some(c) if c.is_ascii_digit() => {
            let start = *at;
            while matches!(chars.get(*at), Some(c) if c.is_ascii_digit()) {
                *at += 1;
            }
            let text: String = chars[start..*at].iter().collect();
            text.parse::<u32>()
                .map(Lit::Number)
                .map_err(|_| GlyphListError::BadNumber(text))
        }
        _ => Err(GlyphListError::ExpectedElement),
    }
}

/// Inclusive, ascending, same-kind. Glyph ranges walk Unicode scalar
/// succession, skipping the surrogate gap (never a valid `char`); numeric
/// ranges mint each value's decimal string.
fn expand_range(lo: &Lit, hi: &Lit) -> Result<Vec<String>, GlyphListError> {
    match (lo, hi) {
        (Lit::Number(l), Lit::Number(h)) => {
            if l > h {
                return Err(GlyphListError::RangeDescending);
            }
            Ok((*l..=*h).map(|v| v.to_string()).collect())
        }
        (Lit::Glyph(l), Lit::Glyph(h)) => {
            let (Some(lc), Some(hc)) = (single_scalar(l), single_scalar(h)) else {
                return Err(GlyphListError::RangeEndpointNotScalar);
            };
            if lc as u32 > hc as u32 {
                return Err(GlyphListError::RangeDescending);
            }
            Ok((lc as u32..=hc as u32)
                .filter_map(char::from_u32)
                .map(|c| c.to_string())
                .collect())
        }
        _ => Err(GlyphListError::RangeKindMismatch),
    }
}

fn single_scalar(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_quoted_glyph_list() {
        assert_eq!(
            parse_glyph_list("' ','s','b','k','1'").unwrap(),
            vec![" ", "s", "b", "k", "1"]
        );
    }

    #[test]
    fn tolerates_whitespace_around_elements() {
        assert_eq!(parse_glyph_list(" ' ' , 's' ").unwrap(), vec![" ", "s"]);
    }

    #[test]
    fn expands_a_glyph_range_by_scalar_succession() {
        assert_eq!(
            parse_glyph_list("'0'..'4'").unwrap(),
            vec!["0", "1", "2", "3", "4"]
        );
    }

    #[test]
    fn expands_a_numeric_range_into_decimal_labels() {
        assert_eq!(parse_glyph_list("0..3").unwrap(), vec!["0", "1", "2", "3"]);
    }

    #[test]
    fn a_numeric_label_is_its_value_not_its_spelling() {
        assert_eq!(parse_glyph_list("05").unwrap(), vec!["5"]);
    }

    #[test]
    fn decodes_the_two_legal_escapes() {
        assert_eq!(parse_glyph_list(r"'\'','\\'").unwrap(), vec!["'", r"\"]);
    }

    #[test]
    fn accepts_a_multi_character_glyph() {
        assert_eq!(parse_glyph_list("'ab','c'").unwrap(), vec!["ab", "c"]);
    }

    #[test]
    fn rejects_an_empty_list() {
        assert!(matches!(parse_glyph_list(""), Err(GlyphListError::Empty)));
        assert!(matches!(
            parse_glyph_list("   "),
            Err(GlyphListError::Empty)
        ));
    }

    #[test]
    fn rejects_a_duplicate_glyph() {
        assert!(matches!(
            parse_glyph_list("'a','a'"),
            Err(GlyphListError::Duplicate(g)) if g == "a"
        ));
    }

    #[test]
    fn rejects_an_unterminated_literal() {
        assert!(matches!(
            parse_glyph_list("'a"),
            Err(GlyphListError::UnterminatedLiteral)
        ));
    }

    #[test]
    fn rejects_an_invalid_escape() {
        assert!(matches!(
            parse_glyph_list(r"'\n'"),
            Err(GlyphListError::InvalidEscape('n'))
        ));
    }

    #[test]
    fn rejects_a_descending_range() {
        assert!(matches!(
            parse_glyph_list("'9'..'0'"),
            Err(GlyphListError::RangeDescending)
        ));
        assert!(matches!(
            parse_glyph_list("9..0"),
            Err(GlyphListError::RangeDescending)
        ));
    }

    #[test]
    fn rejects_mixed_kind_range_endpoints() {
        assert!(matches!(
            parse_glyph_list("'a'..3"),
            Err(GlyphListError::RangeKindMismatch)
        ));
    }

    #[test]
    fn rejects_a_multi_scalar_glyph_range_endpoint() {
        assert!(matches!(
            parse_glyph_list("'ab'..'z'"),
            Err(GlyphListError::RangeEndpointNotScalar)
        ));
    }

    #[test]
    fn rejects_more_than_127_glyphs() {
        assert!(matches!(
            parse_glyph_list("0..127"),
            Err(GlyphListError::TooMany(128))
        ));
    }

    #[test]
    fn rejects_a_trailing_or_empty_element() {
        assert!(matches!(
            parse_glyph_list("'a',"),
            Err(GlyphListError::ExpectedElement)
        ));
        assert!(matches!(
            parse_glyph_list("'a',,'b'"),
            Err(GlyphListError::ExpectedElement)
        ));
    }

    #[test]
    fn a_sequence_allows_repeats_that_an_alphabet_rejects() {
        // Cells are a sequence: `1,1,1` is an ordinary tape run.
        assert_eq!(
            parse_glyph_sequence("'1','1','1'").unwrap(),
            vec!["1", "1", "1"]
        );
        assert!(matches!(
            parse_glyph_list("'1','1','1'"),
            Err(GlyphListError::Duplicate(_))
        ));
    }

    #[test]
    fn a_sequence_has_no_length_cap() {
        // An alphabet is bounded at 127 symbols; the tape written over it
        // is not.
        let seq = parse_glyph_sequence("0..300").unwrap();
        assert_eq!(seq.len(), 301);
        assert!(matches!(
            parse_glyph_list("0..300"),
            Err(GlyphListError::TooMany(301))
        ));
    }

    #[test]
    fn a_sequence_still_rejects_malformed_input() {
        assert!(matches!(
            parse_glyph_sequence(""),
            Err(GlyphListError::Empty)
        ));
        assert!(matches!(
            parse_glyph_sequence("'a"),
            Err(GlyphListError::UnterminatedLiteral)
        ));
        assert!(matches!(
            parse_glyph_sequence("'9'..'0'"),
            Err(GlyphListError::RangeDescending)
        ));
    }
}
