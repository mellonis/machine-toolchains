//! Per-line spanned tokenizer for assembly text (docs/formats.md
//! (assembly text)). Total: any input tokenizes; unknown characters
//! become Junk tokens. Arch-agnostic — mnemonics are just Words here.

use crate::asm::syntax::AsmCaps;
use crate::diagnostics::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AsmTokenKind {
    /// Identifier-ish text: mnemonics, labels, directives, symbol names.
    /// May contain `.` and embedded `::` (maximal munch); never a
    /// trailing single `:`.
    Word(String),
    /// Integer literal, raw spelling retained (`007`, `-3`).
    Number(String),
    Colon,
    Comma,
    At,
    /// `;` to end of line, verbatim including the `;`.
    Comment(String),
    // --- Capability-gated single-character tokens ---
    // Emitted only when the owning cap is on (and, for the markers, only
    // inside the relevant depth). With `AsmCaps::default()` these never
    // appear: the characters fall through to `Junk`, exactly as before.
    /// `[` (vectors cap).
    LBracket,
    /// `]` (vectors cap).
    RBracket,
    /// `{` (rept cap).
    LBrace,
    /// `}` (rept cap).
    RBrace,
    /// `(` inside `{…}` (rept cap).
    LParen,
    /// `)` inside `{…}` (rept cap).
    RParen,
    /// `*` inside `[…]` or `{…}` (vectors / rept caps).
    Star,
    /// `-` inside `[…]` or `{…}` (vectors / rept caps).
    Dash,
    /// `+` inside `{…}` (rept cap).
    Plus,
    /// `%` inside `{…}` (rept cap).
    Percent,
    /// `<` inside `[…]` (vectors cap).
    Lt,
    /// `>` inside `[…]` (vectors cap).
    Gt,
    /// `.` inside `[…]` (vectors cap).
    Dot,
    /// Any character no other rule accepts (`<`, `>`, `"`, …).
    Junk(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AsmToken {
    pub kind: AsmTokenKind,
    pub line: u32, // 1-based
    pub col: u32,  // 1-based, char-counted
    pub len: u32,  // in chars
}

impl AsmToken {
    pub fn span(&self) -> Span {
        Span::new(self.line, self.col, self.line, self.col + self.len)
    }
}

// Unicode-aware, matching the legacy parser's `is_ident`/`is_symbol_name`
// (they used `char::is_alphabetic`/`is_alphanumeric`) so identifiers like
// `идиВКонец` still tokenize as a single Word. Label-name tightening to
// ASCII lives in lower.rs, not here.
fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '.'
}

fn is_word_cont(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// Scans a `Word` starting at `start`, which must be either a
/// word-start character or the first `:` of a leading `::` pair (a
/// `::` pair with no already-open word still starts a word). Munches
/// further embedded `::` pairs by two-char lookahead alongside
/// ordinary continuation characters; a lone `:` ends the word.
/// Returns the word text and the number of chars consumed.
fn scan_word(chars: &[char], start: usize) -> (String, usize) {
    let n = chars.len();
    let mut pos = if chars[start] == ':' {
        start + 2 // the leading `::` pair
    } else {
        start + 1 // the single word-start char
    };
    loop {
        if pos >= n {
            break;
        }
        if pos + 1 < n && chars[pos] == ':' && chars[pos + 1] == ':' {
            pos += 2;
        } else if is_word_cont(chars[pos]) {
            pos += 1;
        } else {
            break;
        }
    }
    (chars[start..pos].iter().collect(), pos - start)
}

/// Scans a `Number`: an optional leading `-` followed by one or more
/// ASCII digits. `start` must be a digit, or a `-` immediately
/// followed by a digit. Returns the raw spelling and chars consumed.
fn scan_number(chars: &[char], start: usize) -> (String, usize) {
    let n = chars.len();
    let mut pos = if chars[start] == '-' {
        start + 1
    } else {
        start
    };
    while pos < n && chars[pos].is_ascii_digit() {
        pos += 1;
    }
    (chars[start..pos].iter().collect(), pos - start)
}

/// A capability-gated single-character token, or `None` if `c` is not
/// gated on in the current context. Two depth rules drive it:
///
///   * vectors: `caps.vectors` lets `[` open a bracket depth (any `[`,
///     no operand-position gate) and `]` close it; inside depth ≥ 1 the
///     vector markers `. < > - *` lex as `Dot`/`Lt`/`Gt`/`Dash`/`Star`.
///     Outside brackets nothing changes — `.func` still lexes as a Word.
///   * rept: `caps.rept` lets `{` open a brace depth and `}` close it;
///     inside depth ≥ 1 the substitution operators `( ) + % - *` lex as
///     `LParen`/`RParen`/`Plus`/`Percent`/`Dash`/`Star`.
///
/// With `AsmCaps::default()` (both off) every arm is dead and the caller
/// falls through to the classic path, so the token stream is byte-identical.
fn caps_token(
    c: char,
    caps: AsmCaps,
    bracket_depth: u32,
    brace_depth: u32,
) -> Option<AsmTokenKind> {
    let in_vector = caps.vectors && bracket_depth > 0;
    let in_rept = caps.rept && brace_depth > 0;
    match c {
        '[' if caps.vectors => Some(AsmTokenKind::LBracket),
        ']' if caps.vectors => Some(AsmTokenKind::RBracket),
        '{' if caps.rept => Some(AsmTokenKind::LBrace),
        '}' if caps.rept => Some(AsmTokenKind::RBrace),
        '.' if in_vector => Some(AsmTokenKind::Dot),
        '<' if in_vector => Some(AsmTokenKind::Lt),
        '>' if in_vector => Some(AsmTokenKind::Gt),
        '-' if in_vector || in_rept => Some(AsmTokenKind::Dash),
        '*' if in_vector || in_rept => Some(AsmTokenKind::Star),
        '(' if in_rept => Some(AsmTokenKind::LParen),
        ')' if in_rept => Some(AsmTokenKind::RParen),
        '+' if in_rept => Some(AsmTokenKind::Plus),
        '%' if in_rept => Some(AsmTokenKind::Percent),
        _ => None,
    }
}

/// Tokenizes one line (no `\n` inside). Total — never fails. `caps`
/// selects the per-dialect opt-in surface; `AsmCaps::default()` is the
/// classic assembly grammar (bracket/brace depths never open).
pub(crate) fn lex_line(text: &str, line_no: u32, caps: AsmCaps) -> Vec<AsmToken> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut pos = 0usize;
    let mut col: u32 = 1;
    // Depths reset per line: vector operands `[…]` and `{…}` substitution
    // never span a physical line. They open only under their cap (see
    // `caps_token`); with default caps they stay 0 and gate nothing.
    let mut bracket_depth: u32 = 0;
    let mut brace_depth: u32 = 0;

    while pos < n {
        let c = chars[pos];

        if c == ' ' || c == '\t' {
            pos += 1;
            col += 1;
            continue;
        }

        // Capability-gated tokens come before word/number scanning so an
        // in-bracket `.` beats word-start and an in-context `-` beats a
        // negative-number literal. Guarded on caps, so it is inert under
        // `AsmCaps::default()`.
        if let Some(kind) = caps_token(c, caps, bracket_depth, brace_depth) {
            match kind {
                AsmTokenKind::LBracket => bracket_depth += 1,
                AsmTokenKind::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
                AsmTokenKind::LBrace => brace_depth += 1,
                AsmTokenKind::RBrace => brace_depth = brace_depth.saturating_sub(1),
                _ => {}
            }
            tokens.push(AsmToken {
                kind,
                line: line_no,
                col,
                len: 1,
            });
            pos += 1;
            col += 1;
            continue;
        }

        if c == ';' {
            let comment_text: String = chars[pos..].iter().collect();
            let len = (n - pos) as u32;
            tokens.push(AsmToken {
                kind: AsmTokenKind::Comment(comment_text),
                line: line_no,
                col,
                len,
            });
            pos = n;
            col += len;
            continue;
        }

        let starts_leading_double_colon = c == ':' && pos + 1 < n && chars[pos + 1] == ':';
        if is_word_start(c) || starts_leading_double_colon {
            let (word, consumed) = scan_word(&chars, pos);
            let len = consumed as u32;
            tokens.push(AsmToken {
                kind: AsmTokenKind::Word(word),
                line: line_no,
                col,
                len,
            });
            pos += consumed;
            col += len;
            continue;
        }

        if c == ':' {
            tokens.push(AsmToken {
                kind: AsmTokenKind::Colon,
                line: line_no,
                col,
                len: 1,
            });
            pos += 1;
            col += 1;
            continue;
        }

        let starts_number =
            c.is_ascii_digit() || (c == '-' && pos + 1 < n && chars[pos + 1].is_ascii_digit());
        if starts_number {
            let (num, consumed) = scan_number(&chars, pos);
            let len = consumed as u32;
            tokens.push(AsmToken {
                kind: AsmTokenKind::Number(num),
                line: line_no,
                col,
                len,
            });
            pos += consumed;
            col += len;
            continue;
        }

        let kind = match c {
            '@' => AsmTokenKind::At,
            ',' => AsmTokenKind::Comma,
            other => AsmTokenKind::Junk(other),
        };
        tokens.push(AsmToken {
            kind,
            line: line_no,
            col,
            len: 1,
        });
        pos += 1;
        col += 1;
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Two-arg shim shadowing the real `lex_line`: the classic-grammar
    /// tests below (and the `proptest`) drive the default-caps path.
    /// Capability tests reach the real entry via `super::lex_line`.
    fn lex_line(text: &str, line_no: u32) -> Vec<AsmToken> {
        super::lex_line(text, line_no, AsmCaps::default())
    }

    /// Lex a single line under the given caps, hitting the real entry.
    fn lex_for_test(source: &str, caps: AsmCaps) -> Vec<AsmToken> {
        super::lex_line(source, 1, caps)
    }

    /// The token kinds a single line lexes to under the given caps.
    fn kinds_for_test(source: &str, caps: AsmCaps) -> Vec<AsmTokenKind> {
        lex_for_test(source, caps)
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    fn word(s: &str, line: u32, col: u32, len: u32) -> AsmToken {
        AsmToken {
            kind: AsmTokenKind::Word(s.to_string()),
            line,
            col,
            len,
        }
    }

    fn number(s: &str, line: u32, col: u32, len: u32) -> AsmToken {
        AsmToken {
            kind: AsmTokenKind::Number(s.to_string()),
            line,
            col,
            len,
        }
    }

    fn colon(line: u32, col: u32) -> AsmToken {
        AsmToken {
            kind: AsmTokenKind::Colon,
            line,
            col,
            len: 1,
        }
    }

    fn comma(line: u32, col: u32) -> AsmToken {
        AsmToken {
            kind: AsmTokenKind::Comma,
            line,
            col,
            len: 1,
        }
    }

    fn at(line: u32, col: u32) -> AsmToken {
        AsmToken {
            kind: AsmTokenKind::At,
            line,
            col,
            len: 1,
        }
    }

    fn comment(s: &str, line: u32, col: u32, len: u32) -> AsmToken {
        AsmToken {
            kind: AsmTokenKind::Comment(s.to_string()),
            line,
            col,
            len,
        }
    }

    fn junk(c: char, line: u32, col: u32) -> AsmToken {
        AsmToken {
            kind: AsmTokenKind::Junk(c),
            line,
            col,
            len: 1,
        }
    }

    #[test]
    fn label_colon_and_mnemonic_get_exact_spans() {
        let tokens = lex_line("L1:     rgt", 7);
        assert_eq!(
            tokens,
            vec![word("L1", 7, 1, 2), colon(7, 3), word("rgt", 7, 9, 3)]
        );
    }

    #[test]
    fn trailing_comment_is_verbatim_and_char_counted() {
        let tokens = lex_line("        jm      L1              ; loop", 1);
        assert_eq!(
            tokens,
            vec![
                word("jm", 1, 9, 2),
                word("L1", 1, 17, 2),
                comment("; loop", 1, 33, 6),
            ]
        );
    }

    #[test]
    fn embedded_double_colon_is_munched_into_the_word() {
        let tokens = lex_line("std::x:", 1);
        assert_eq!(tokens, vec![word("std::x", 1, 1, 6), colon(1, 7)]);
    }

    #[test]
    fn dotted_word_before_a_label_colon() {
        let tokens = lex_line("foo.bar:", 1);
        assert_eq!(tokens, vec![word("foo.bar", 1, 1, 7), colon(1, 8)]);
    }

    #[test]
    fn directive_and_namespaced_dotted_words() {
        let tokens = lex_line(".func std::api.helper local", 1);
        assert_eq!(
            tokens,
            vec![
                word(".func", 1, 1, 5),
                word("std::api.helper", 1, 7, 15),
                word("local", 1, 23, 5),
            ]
        );
    }

    #[test]
    fn numbers_keep_their_raw_spelling() {
        let tokens = lex_line("wr      007, -1", 1);
        assert_eq!(
            tokens,
            vec![
                word("wr", 1, 1, 2),
                number("007", 1, 9, 3),
                comma(1, 12),
                number("-1", 1, 14, 2),
            ]
        );
    }

    #[test]
    fn at_prefixed_symbol_reference() {
        let tokens = lex_line("call    @std::api", 1);
        assert_eq!(
            tokens,
            vec![word("call", 1, 1, 4), at(1, 9), word("std::api", 1, 10, 8)]
        );
    }

    #[test]
    fn disassembly_listing_line_mixes_numbers_colon_and_junk() {
        let tokens = lex_line("  0004:  21 05 <goToEnd>", 1);
        assert_eq!(
            tokens,
            vec![
                number("0004", 1, 3, 4),
                colon(1, 7),
                number("21", 1, 10, 2),
                number("05", 1, 13, 2),
                junk('<', 1, 16),
                word("goToEnd", 1, 17, 7),
                junk('>', 1, 24),
            ]
        );
    }

    #[test]
    fn empty_and_whitespace_only_lines_yield_no_tokens() {
        assert_eq!(lex_line("", 1), vec![]);
        assert_eq!(lex_line("    ", 1), vec![]);
        assert_eq!(lex_line("\t \t", 1), vec![]);
    }

    #[test]
    fn non_ascii_comment_columns_are_char_counted_not_byte_counted() {
        // "wr 1 ; тест": the comment starts at char column 6 even
        // though the Cyrillic letters are multi-byte in UTF-8.
        let tokens = lex_line("wr 1 ; тест", 1);
        assert_eq!(
            tokens,
            vec![
                word("wr", 1, 1, 2),
                number("1", 1, 4, 1),
                comment("; тест", 1, 6, 6),
            ]
        );
    }

    #[test]
    fn unicode_identifiers_tokenize_as_a_single_word() {
        // Legacy acceptance: non-ASCII letters are word characters, so a
        // Unicode function name is one Word, not a run of Junk. Guards the
        // contract at the layer where it lives (a downstream pipeline test
        // otherwise catches this only three layers up).
        let tokens = lex_line("идиВКонец", 1);
        assert_eq!(tokens, vec![word("идиВКонец", 1, 1, 9)]);
    }

    #[test]
    fn leading_double_colon_with_no_open_word_starts_a_word() {
        // Rule: a leading `::` with no open word still starts a Word
        // (lowering rejects it later; the lexer never emits Junk here).
        let tokens = lex_line("::x", 1);
        assert_eq!(tokens, vec![word("::x", 1, 1, 3)]);
    }

    #[test]
    fn span_matches_line_col_and_len() {
        let token = word("rgt", 7, 9, 3);
        assert_eq!(token.span(), Span::new(7, 9, 7, 12));
    }

    proptest! {
        #[test]
        fn never_panics_and_tokens_never_exceed_the_input_length(
            text in proptest::collection::vec(
                any::<char>().prop_filter("no embedded newlines", |c| *c != '\n'),
                0..64,
            )
            .prop_map(|chars: Vec<char>| chars.into_iter().collect::<String>()),
            line_no in any::<u32>(),
        ) {
            let tokens = lex_line(&text, line_no);
            let rendered_len: usize = tokens.iter().map(|t| kind_rendered_len(&t.kind)).sum();
            prop_assert!(rendered_len <= text.chars().count());
        }
    }

    /// Test-only rendering of a token kind's own text, used to sanity-check
    /// that lexing never manufactures more text than the input contained.
    fn kind_rendered_len(kind: &AsmTokenKind) -> usize {
        match kind {
            AsmTokenKind::Word(s) | AsmTokenKind::Number(s) | AsmTokenKind::Comment(s) => {
                s.chars().count()
            }
            AsmTokenKind::Colon
            | AsmTokenKind::Comma
            | AsmTokenKind::At
            | AsmTokenKind::LBracket
            | AsmTokenKind::RBracket
            | AsmTokenKind::LBrace
            | AsmTokenKind::RBrace
            | AsmTokenKind::LParen
            | AsmTokenKind::RParen
            | AsmTokenKind::Star
            | AsmTokenKind::Dash
            | AsmTokenKind::Plus
            | AsmTokenKind::Percent
            | AsmTokenKind::Lt
            | AsmTokenKind::Gt
            | AsmTokenKind::Dot
            | AsmTokenKind::Junk(_) => 1,
        }
    }

    #[test]
    fn default_caps_keep_symbols_junk() {
        let toks = lex_for_test("wr [1,*]", AsmCaps::default());
        assert!(
            toks.iter()
                .any(|t| matches!(t.kind, AsmTokenKind::Junk('[')))
        );
    }

    #[test]
    fn vector_caps_tokenize_brackets_and_markers() {
        let caps = AsmCaps {
            vectors: true,
            ..Default::default()
        };
        let kinds = kinds_for_test("wr [1, *, -, <, >, .]", caps);
        assert!(kinds.contains(&AsmTokenKind::LBracket));
        assert!(kinds.contains(&AsmTokenKind::Star));
        assert!(kinds.contains(&AsmTokenKind::Dash));
        assert!(kinds.contains(&AsmTokenKind::Lt));
        assert!(kinds.contains(&AsmTokenKind::Gt));
        assert!(kinds.contains(&AsmTokenKind::Dot));
        assert!(kinds.contains(&AsmTokenKind::RBracket));
    }

    #[test]
    fn dot_outside_vectors_still_words() {
        let caps = AsmCaps {
            vectors: true,
            rept: true,
            tables: true,
        };
        let toks = lex_for_test(".section tables", caps);
        assert!(matches!(&toks[0].kind, AsmTokenKind::Word(w) if w == ".section"));
    }

    #[test]
    fn rept_caps_tokenize_substitution() {
        let caps = AsmCaps {
            rept: true,
            ..Default::default()
        };
        let kinds = kinds_for_test("Linc{(v+1)%127}", caps);
        assert!(kinds.contains(&AsmTokenKind::LBrace));
        assert!(kinds.contains(&AsmTokenKind::LParen));
        assert!(kinds.contains(&AsmTokenKind::Plus));
        assert!(kinds.contains(&AsmTokenKind::Percent));
        assert!(kinds.contains(&AsmTokenKind::RBrace));
    }
}
