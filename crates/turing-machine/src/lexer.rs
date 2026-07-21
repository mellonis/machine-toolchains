//! `.tmc` lexer: source text → tokens with line:col. The front-end mirror
//! of the `.pmc` lexer in the sibling PM-1 crate, sharing its token shape
//! (`Token { kind, line, col, len }` with char-counted `len` and
//! [`Token::span`]), its `lex` / `lex_with` split, its `// … ` / `/* … */`
//! comment handling, and its positional `?` / `!` doc-line rule verbatim.
//!
//! What is new for TM-1: single-quoted glyph literals (`'a'`), the range
//! `..` token, the rule arrow `->` and the map arrow `=>`, and the pattern /
//! vector punctuation `* - < > . [ ]`. The 24 reserved keywords are NOT
//! recognized here — they lex as plain [`TokenKind::Ident`] and reservation
//! is enforced once, at parse time; see [`RESERVED`].

use crate::compiler::{CompileError, CompileErrorKind};
use mtc_core::diagnostics::Span;

/// The 24 fully-reserved `.tmc` keywords. Canonical home: they are ordinary
/// identifiers to the lexer (this list only pins the "keywords lex as
/// `Ident`" contract in the test battery below), and the parser is the ONE
/// place that rejects them where a name is expected — one place of truth,
/// so this array is `pub` for the parser to consume rather than redefine.
/// (`deprecated` is contextual — an attribute word, not a keyword — and is
/// deliberately absent.)
pub const RESERVED: [&str; 24] = [
    "alphabet",
    "machine",
    "tape",
    "state",
    "entry",
    "routine",
    "graph",
    "namespace",
    "export",
    "use",
    "graft",
    "bind",
    "as",
    "map",
    "with",
    "write",
    "move",
    "goto",
    "call",
    "then",
    "return",
    "stop",
    "halt",
    "debugger",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident(String),
    /// The parsed value, plus the digits as WRITTEN (leading zeros
    /// preserved) — a lossless printer needs the spelling, not just the
    /// value.
    Number(u32, String),
    /// A single-quoted glyph literal. The payload is the DECODED content
    /// (escapes resolved), which is what alphabet uniqueness compares and
    /// what a formatter re-encodes losslessly (only `'` and `\` ever need
    /// re-escaping). Content is any non-empty UTF-8 string — one grapheme,
    /// an emoji, or a multi-scalar ZWJ sequence are all one glyph. Only two
    /// escapes are legal inside: `\'` and `\\`; any other backslash
    /// sequence, an empty `''`, or a literal reaching end-of-line unclosed
    /// is a lex error.
    Glyph(String),
    /// `..` — a range, lexed greedily; a lone `.` stays [`TokenKind::Dot`]
    /// (the move-vector "stay" glyph), so `..` and `.` never collide.
    DotDot,
    /// `->` — the rule arrow (`pattern -> action`).
    Arrow,
    /// `=>` — the map arrow (the read-only map form `'a' => 'b'`).
    FatArrow,
    /// `::` — lexed greedily; a single `:` stays [`TokenKind::Colon`].
    ColonColon,
    /// `.` — the move-vector "stay" glyph. Never anything else on its own.
    Dot,
    /// `-` when it does not begin an `->` (write-vector "keep").
    Dash,
    /// `+` — the write-vector substitution's positive delta (`{v+1}`); the
    /// negative form reuses [`TokenKind::Dash`] (`{v-1}`). `+` never begins a
    /// multi-character operator, so it is always this single token.
    Plus,
    /// `=` when it does not begin a `=>` (binding `name = target`).
    Eq,
    /// `*` — the wildcard pattern element, and the multiplication operator in
    /// a write-cell fold expression (`{a*2}`).
    Star,
    /// `%` — the remainder operator in a write-cell fold expression
    /// (`{(v+1)%6}`). Never begins a multi-character operator.
    Percent,
    /// `<` — a move-vector glyph (left), never a comparison.
    Lt,
    /// `>` — a move-vector glyph (right), never a comparison.
    Gt,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Comma,
    Semi,
    Colon,
    At,
    Bang,
    Eof,
    /// Only produced in [`LexMode::WithComments`]. [`lex`] (equivalent to
    /// `lex_with(_, LexMode::WithoutComments)`) never emits this variant,
    /// so callers on the default path see an unchanged token stream.
    Comment(Comment),
    /// `?` as the first non-whitespace character of a line — a doc line.
    /// Payload is the raw text after the sigil, minus ONE leading space if
    /// present (canonical), verbatim otherwise. Semantic, not trivia:
    /// emitted in both [`LexMode`]s.
    DocLine(String),
    /// `!` as the first non-whitespace character of a line — an attention
    /// line. Same payload rule as [`TokenKind::DocLine`]. `!` anywhere else
    /// on a line still lexes as [`TokenKind::Bang`].
    AttentionLine(String),
}

/// Which delimiter pair produced a [`Comment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// `// ...` to end of line.
    Line,
    /// `/* ... */`, possibly spanning multiple source lines.
    Block,
}

/// A comment retained as trivia, produced only in [`LexMode::WithComments`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// Verbatim source text, including the `//` or `/* … */` delimiters.
    pub text: String,
    pub kind: CommentKind,
    /// True iff only whitespace preceded the comment on its physical
    /// line — i.e. the comment begins at that line's first
    /// non-whitespace column.
    pub own_line: bool,
}

/// Whether [`lex_with`] discards comments (the compiler's path, and what
/// [`lex`] does) or retains them as [`TokenKind::Comment`] trivia (for the
/// formatter and language server the later phases add).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexMode {
    WithoutComments,
    WithComments,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
    pub col: u32,
    /// Length in characters of the token's SOURCE spelling (a glyph's `len`
    /// counts the quotes and any escape backslashes as written, not its
    /// decoded payload). Every token is single-line and 0 only for Eof —
    /// EXCEPT a [`TokenKind::Comment`] of [`CommentKind::Block`], which may
    /// span multiple source lines. For those, `len` is the total character
    /// count of `Comment::text` (delimiters and any embedded newlines all
    /// counted), and [`Token::span`]'s end position is not meaningful past
    /// the first line.
    pub len: u32,
}

impl Token {
    /// End-exclusive span of this token's source text.
    pub fn span(&self) -> Span {
        Span::new(self.line, self.col, self.line, self.col + self.len)
    }
}

/// Identifier rule: Unicode; first char alphabetic or `_`, then
/// alphanumeric or `_` — the same classes as the `.tma` symbol grammar, so
/// every `.tmc` name survives the trip through generated assembly.
fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

struct Cursor<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: u32,
    col: u32,
    /// True iff nothing but whitespace has been consumed since the last
    /// newline (or since the start of input). Read at the top of the main
    /// loop — before a comment's own characters are bumped, which would
    /// otherwise flip it to false — to compute `Comment::own_line`.
    at_line_start: bool,
}

impl Cursor<'_> {
    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
            self.at_line_start = true;
        } else {
            self.col += 1;
            if !c.is_whitespace() {
                self.at_line_start = false;
            }
        }
        Some(c)
    }
}

fn err(line: u32, col: u32, message: String) -> CompileError {
    CompileError {
        span: Span::point(line, col),
        kind: CompileErrorKind::Lex(message),
    }
}

/// Lex with comments discarded — the compiler's path. Equivalent to
/// `lex_with(source, LexMode::WithoutComments)`.
pub fn lex(source: &str) -> Result<Vec<Token>, CompileError> {
    lex_with(source, LexMode::WithoutComments)
}

pub fn lex_with(source: &str, mode: LexMode) -> Result<Vec<Token>, CompileError> {
    let mut cur = Cursor {
        chars: source.chars().peekable(),
        line: 1,
        col: 1,
        at_line_start: true,
    };
    let mut tokens = Vec::new();

    while let Some(c) = cur.peek() {
        let (line, col) = (cur.line, cur.col);
        let own_line = cur.at_line_start;
        if c.is_whitespace() {
            cur.bump();
            continue;
        }
        // Positional doc/attention lines: `?`/`!` as the first
        // non-whitespace character of a line consume to end of line as one
        // token. `own_line` is the same line-start flag `Comment::own_line`
        // reads below — a `?`/`!` anywhere else on a line falls through
        // unchanged (`!` lexes as `Bang` via the `single` match further
        // down; `?` reaches the catch-all "unexpected character" error at
        // the bottom of this loop).
        if own_line && (c == '?' || c == '!') {
            cur.bump();
            let mut raw = String::new();
            while let Some(nc) = cur.peek() {
                if nc == '\n' {
                    break;
                }
                raw.push(nc);
                cur.bump();
            }
            let len = 1 + raw.chars().count() as u32;
            let text = raw.strip_prefix(' ').unwrap_or(&raw).to_string();
            let kind = if c == '?' {
                TokenKind::DocLine(text)
            } else {
                TokenKind::AttentionLine(text)
            };
            tokens.push(Token {
                kind,
                line,
                col,
                len,
            });
            continue;
        }
        if c == '/' {
            cur.bump();
            match cur.peek() {
                Some('/') => {
                    cur.bump();
                    // Built regardless of mode (harmless when discarded) so
                    // the consumption loop below is identical either way —
                    // the WithoutComments token stream depends only on
                    // whether the Comment token is pushed.
                    let mut text = String::from("//");
                    loop {
                        match cur.bump() {
                            Some('\n') => break,
                            Some(ch) => text.push(ch),
                            None => break,
                        }
                    }
                    if mode == LexMode::WithComments {
                        let len = text.chars().count() as u32;
                        tokens.push(Token {
                            kind: TokenKind::Comment(Comment {
                                text,
                                kind: CommentKind::Line,
                                own_line,
                            }),
                            line,
                            col,
                            len,
                        });
                    }
                }
                Some('*') => {
                    cur.bump();
                    let mut text = String::from("/*");
                    let mut prev = '\0';
                    let mut closed = false;
                    while let Some(c) = cur.bump() {
                        text.push(c);
                        if prev == '*' && c == '/' {
                            closed = true;
                            break;
                        }
                        prev = c;
                    }
                    if !closed {
                        return Err(err(line, col, "unterminated block comment".into()));
                    }
                    if mode == LexMode::WithComments {
                        let len = text.chars().count() as u32;
                        tokens.push(Token {
                            kind: TokenKind::Comment(Comment {
                                text,
                                kind: CommentKind::Block,
                                own_line,
                            }),
                            line,
                            col,
                            len,
                        });
                    }
                }
                _ => return Err(err(line, col, "unexpected character `/`".into())),
            }
            continue;
        }
        if c == '\'' {
            // Single-quoted glyph literal. Decode `\'` and `\\`; every other
            // backslash sequence is a lex error. Track the SOURCE character
            // count (`src_len`, quotes and escape backslashes included) so
            // `len` spans the spelling, not the decoded payload.
            cur.bump();
            let mut value = String::new();
            let mut src_len: u32 = 1; // opening quote
            let mut closed = false;
            loop {
                match cur.peek() {
                    None | Some('\n') => break, // unterminated
                    Some('\'') => {
                        cur.bump();
                        src_len += 1;
                        closed = true;
                        break;
                    }
                    Some('\\') => {
                        cur.bump();
                        src_len += 1;
                        match cur.peek() {
                            Some('\'') => {
                                value.push('\'');
                                cur.bump();
                                src_len += 1;
                            }
                            Some('\\') => {
                                value.push('\\');
                                cur.bump();
                                src_len += 1;
                            }
                            None | Some('\n') => break, // dangling escape → unterminated
                            Some(bad) => {
                                return Err(err(
                                    line,
                                    col,
                                    format!(
                                        "invalid escape `\\{bad}` in glyph literal — only `\\'` and `\\\\` are allowed"
                                    ),
                                ));
                            }
                        }
                    }
                    Some(ch) => {
                        value.push(ch);
                        cur.bump();
                        src_len += 1;
                    }
                }
            }
            if !closed {
                return Err(err(line, col, "unterminated glyph literal".into()));
            }
            if value.is_empty() {
                return Err(err(
                    line,
                    col,
                    "empty glyph literal — a glyph must have at least one character".into(),
                ));
            }
            tokens.push(Token {
                kind: TokenKind::Glyph(value),
                line,
                col,
                len: src_len,
            });
            continue;
        }
        if c == ':' {
            cur.bump();
            let (kind, len) = if cur.peek() == Some(':') {
                cur.bump();
                (TokenKind::ColonColon, 2)
            } else {
                (TokenKind::Colon, 1)
            };
            tokens.push(Token {
                kind,
                line,
                col,
                len,
            });
            continue;
        }
        if c == '-' {
            cur.bump();
            let (kind, len) = if cur.peek() == Some('>') {
                cur.bump();
                (TokenKind::Arrow, 2)
            } else {
                (TokenKind::Dash, 1)
            };
            tokens.push(Token {
                kind,
                line,
                col,
                len,
            });
            continue;
        }
        if c == '=' {
            cur.bump();
            let (kind, len) = if cur.peek() == Some('>') {
                cur.bump();
                (TokenKind::FatArrow, 2)
            } else {
                (TokenKind::Eq, 1)
            };
            tokens.push(Token {
                kind,
                line,
                col,
                len,
            });
            continue;
        }
        if c == '.' {
            cur.bump();
            let (kind, len) = if cur.peek() == Some('.') {
                cur.bump();
                (TokenKind::DotDot, 2)
            } else {
                (TokenKind::Dot, 1)
            };
            tokens.push(Token {
                kind,
                line,
                col,
                len,
            });
            continue;
        }
        // Single-character punctuation. `@` is a plain token here — its
        // role as a qualified-name sigil is a parse concern, so the lexer
        // imposes no adjacency rule on it.
        let single = match c {
            '!' => Some(TokenKind::Bang),
            '+' => Some(TokenKind::Plus),
            ',' => Some(TokenKind::Comma),
            ';' => Some(TokenKind::Semi),
            '*' => Some(TokenKind::Star),
            '%' => Some(TokenKind::Percent),
            '<' => Some(TokenKind::Lt),
            '>' => Some(TokenKind::Gt),
            '[' => Some(TokenKind::LBracket),
            ']' => Some(TokenKind::RBracket),
            '{' => Some(TokenKind::LBrace),
            '}' => Some(TokenKind::RBrace),
            '(' => Some(TokenKind::LParen),
            ')' => Some(TokenKind::RParen),
            '@' => Some(TokenKind::At),
            _ => None,
        };
        if let Some(kind) = single {
            cur.bump();
            tokens.push(Token {
                kind,
                line,
                col,
                len: 1,
            });
            continue;
        }
        if c.is_ascii_digit() {
            let mut digits = String::new();
            while let Some(c) = cur.peek() {
                if c.is_ascii_digit() {
                    digits.push(c);
                    cur.bump();
                } else {
                    break;
                }
            }
            if cur.peek().is_some_and(is_ident_start) {
                return Err(err(
                    line,
                    col,
                    "identifier cannot start with a digit".into(),
                ));
            }
            let value: u32 = digits
                .parse()
                .map_err(|_| err(line, col, format!("number `{digits}` is too large")))?;
            let len = digits.len() as u32; // ASCII digits: bytes == chars
            tokens.push(Token {
                kind: TokenKind::Number(value, digits),
                line,
                col,
                len,
            });
            continue;
        }
        if is_ident_start(c) {
            let mut name = String::new();
            while let Some(c) = cur.peek() {
                if is_ident_continue(c) {
                    name.push(c);
                    cur.bump();
                } else {
                    break;
                }
            }
            let len = name.chars().count() as u32;
            tokens.push(Token {
                kind: TokenKind::Ident(name),
                line,
                col,
                len,
            });
            continue;
        }
        return Err(err(line, col, format!("unexpected character `{c}`")));
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        line: cur.line,
        col: cur.col,
        len: 0,
    });
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CompileErrorKind;
    use proptest::prelude::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_the_shape_of_a_rule() {
        use TokenKind::*;
        assert_eq!(
            kinds("['b'] -> write ['a'] move [>] goto scan;"),
            vec![
                LBracket,
                Glyph("b".into()),
                RBracket,
                Arrow,
                Ident("write".into()),
                LBracket,
                Glyph("a".into()),
                RBracket,
                Ident("move".into()),
                LBracket,
                Gt,
                RBracket,
                Ident("goto".into()),
                Ident("scan".into()),
                Semi,
                Eof,
            ]
        );
    }

    #[test]
    fn tracks_line_and_column() {
        let tokens = lex("machine {\n  tape num: bits;\n}").unwrap();
        let tape = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Ident("tape".into()))
            .unwrap();
        assert_eq!((tape.line, tape.col), (2, 3));
        let bits = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Ident("bits".into()))
            .unwrap();
        assert_eq!((bits.line, bits.col), (2, 13));
    }

    #[test]
    fn unicode_identifiers() {
        assert_eq!(
            kinds("идиВКонец(x)"),
            vec![
                TokenKind::Ident("идиВКонец".into()),
                TokenKind::LParen,
                TokenKind::Ident("x".into()),
                TokenKind::RParen,
                TokenKind::Eof
            ]
        );
    }

    /// The 24 reserved keywords are lexer-transparent: each is a plain
    /// `Ident`. Reservation is a parse-time concern (the parser consumes
    /// [`RESERVED`]); the lexer must never special-case them.
    #[test]
    fn every_reserved_keyword_lexes_as_a_plain_ident() {
        assert_eq!(RESERVED.len(), 24);
        for kw in RESERVED {
            assert_eq!(
                kinds(kw),
                vec![TokenKind::Ident(kw.to_string()), TokenKind::Eof],
                "keyword `{kw}` must lex as a plain Ident"
            );
        }
        // `deprecated` is contextual, not reserved — it also lexes as an
        // ordinary Ident (the difference lives entirely in the parser).
        assert!(!RESERVED.contains(&"deprecated"));
        assert_eq!(
            kinds("deprecated"),
            vec![TokenKind::Ident("deprecated".into()), TokenKind::Eof]
        );
    }

    // ---- glyph literals -------------------------------------------------

    #[test]
    fn glyph_basic_ascii() {
        assert_eq!(
            kinds("'x'"),
            vec![TokenKind::Glyph("x".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn glyph_emoji_is_one_glyph() {
        let tokens = lex("'🙂'").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Glyph("🙂".into()));
        // len counts SOURCE chars: quote + one scalar + quote = 3.
        assert_eq!(tokens[0].len, 3);
    }

    #[test]
    fn glyph_zwj_sequence_is_one_glyph() {
        // A multi-scalar ZWJ emoji sequence is a single glyph; the decoded
        // payload preserves every scalar verbatim.
        let seq = "👨\u{200d}👩\u{200d}👧";
        let src = format!("'{seq}'");
        let tokens = lex(&src).unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Glyph(seq.to_string()));
        assert_eq!(tokens[0].len, seq.chars().count() as u32 + 2);
    }

    #[test]
    fn glyph_escaped_quote_and_backslash() {
        // '\'' decodes to a single-quote glyph; len counts the 4 source
        // chars ( ' \ ' ' ).
        let tokens = lex("'\\''").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Glyph("'".into()));
        assert_eq!(tokens[0].len, 4);

        // '\\' decodes to a backslash glyph; 4 source chars ( ' \ \ ' ).
        let tokens = lex("'\\\\'").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Glyph("\\".into()));
        assert_eq!(tokens[0].len, 4);
    }

    #[test]
    fn glyph_empty_is_rejected() {
        let e = lex("''").unwrap_err();
        assert_eq!((e.span.start.line, e.span.start.col), (1, 1));
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("empty glyph")));
    }

    #[test]
    fn glyph_unterminated_at_eol_is_rejected() {
        let e = lex("'a\n").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unterminated glyph")));

        // Unterminated at end of input, too.
        let e = lex("'a").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unterminated glyph")));
    }

    #[test]
    fn glyph_bad_escape_is_rejected() {
        let e = lex("'\\n'").unwrap_err();
        assert_eq!((e.span.start.line, e.span.start.col), (1, 1));
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("invalid escape")));
    }

    #[test]
    fn glyph_dangling_escape_at_eol_is_unterminated() {
        // A `\` with nothing to escape before end-of-line: the literal can
        // never close, so it reads as unterminated rather than bad-escape.
        let e = lex("'\\").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unterminated glyph")));
    }

    // ---- multi-character operator disambiguation ------------------------

    #[test]
    fn dot_dot_versus_dot_versus_spaced_dots() {
        assert_eq!(
            kinds("'0'..'1'"),
            vec![
                TokenKind::Glyph("0".into()),
                TokenKind::DotDot,
                TokenKind::Glyph("1".into()),
                TokenKind::Eof
            ]
        );
        // A lone dot (move-vector "stay").
        assert_eq!(kinds("."), vec![TokenKind::Dot, TokenKind::Eof]);
        // Two spaced dots are two Dots, not a DotDot.
        assert_eq!(
            kinds(". ."),
            vec![TokenKind::Dot, TokenKind::Dot, TokenKind::Eof]
        );
        // Greedy: `...` is `..` then `.`.
        assert_eq!(
            kinds("..."),
            vec![TokenKind::DotDot, TokenKind::Dot, TokenKind::Eof]
        );
    }

    #[test]
    fn arrow_versus_dash_versus_spaced() {
        assert_eq!(kinds("->"), vec![TokenKind::Arrow, TokenKind::Eof]);
        // A bare dash (write-vector "keep").
        assert_eq!(kinds("-"), vec![TokenKind::Dash, TokenKind::Eof]);
        // `- >` (spaced) is Dash then Gt, not an Arrow.
        assert_eq!(
            kinds("- >"),
            vec![TokenKind::Dash, TokenKind::Gt, TokenKind::Eof]
        );
    }

    #[test]
    fn write_vector_substitution_shapes_lex() {
        use TokenKind::*;
        // `{v}` pass-through, `{v+1}` positive delta, `{v-1}` negative delta.
        assert_eq!(
            kinds("[{v}, {v+1}, {v-1}]"),
            vec![
                LBracket,
                LBrace,
                Ident("v".into()),
                RBrace,
                Comma,
                LBrace,
                Ident("v".into()),
                Plus,
                Number(1, "1".into()),
                RBrace,
                Comma,
                LBrace,
                Ident("v".into()),
                Dash,
                Number(1, "1".into()),
                RBrace,
                RBracket,
                Eof,
            ]
        );
        // A lone `+` is always a single Plus token.
        assert_eq!(kinds("+"), vec![Plus, Eof]);
    }

    #[test]
    fn fold_expr_operators_lex() {
        use TokenKind::*;
        // `%` is a single Percent token; `*` stays Star (its wildcard and
        // multiplication spellings share one token). A full fold expression
        // lexes to atoms and operators.
        assert_eq!(kinds("%"), vec![Percent, Eof]);
        assert_eq!(
            kinds("{(v+1)%6}"),
            vec![
                LBrace,
                LParen,
                Ident("v".into()),
                Plus,
                Number(1, "1".into()),
                RParen,
                Percent,
                Number(6, "6".into()),
                RBrace,
                Eof,
            ]
        );
    }

    #[test]
    fn fat_arrow_versus_eq() {
        assert_eq!(kinds("=>"), vec![TokenKind::FatArrow, TokenKind::Eof]);
        assert_eq!(kinds("="), vec![TokenKind::Eq, TokenKind::Eof]);
        // A map body exercises both arrow flavours side by side.
        assert_eq!(
            kinds("{ 'a'->'b', 'c'=>'d' }"),
            vec![
                TokenKind::LBrace,
                TokenKind::Glyph("a".into()),
                TokenKind::Arrow,
                TokenKind::Glyph("b".into()),
                TokenKind::Comma,
                TokenKind::Glyph("c".into()),
                TokenKind::FatArrow,
                TokenKind::Glyph("d".into()),
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn colon_colon_is_greedy_and_single_colons_survive() {
        assert_eq!(
            kinds("mylib::plusOne"),
            vec![
                TokenKind::Ident("mylib".into()),
                TokenKind::ColonColon,
                TokenKind::Ident("plusOne".into()),
                TokenKind::Eof
            ]
        );
        // `tape num: bits` keeps its single colon.
        assert_eq!(
            kinds("num: bits"),
            vec![
                TokenKind::Ident("num".into()),
                TokenKind::Colon,
                TokenKind::Ident("bits".into()),
                TokenKind::Eof
            ]
        );
        // Greedy: `:::` is `::` then `:`.
        assert_eq!(
            kinds(":::"),
            vec![TokenKind::ColonColon, TokenKind::Colon, TokenKind::Eof]
        );
    }

    #[test]
    fn move_glyphs_are_not_comparisons() {
        // `<` and `>` are move-vector glyphs; they never combine with `=`.
        assert_eq!(
            kinds("[<, >, .]"),
            vec![
                TokenKind::LBracket,
                TokenKind::Lt,
                TokenKind::Comma,
                TokenKind::Gt,
                TokenKind::Comma,
                TokenKind::Dot,
                TokenKind::RBracket,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn at_is_a_plain_token_with_no_adjacency_rule() {
        // Unlike the `.pmc` call sigil, `@` here is a bare token; the parser
        // owns its qualified-name role, so nothing is a lex error.
        assert_eq!(
            kinds("@mylib::plusOne"),
            vec![
                TokenKind::At,
                TokenKind::Ident("mylib".into()),
                TokenKind::ColonColon,
                TokenKind::Ident("plusOne".into()),
                TokenKind::Eof
            ]
        );
        // Even a lone `@` or `@` before punctuation is fine at lex time.
        assert_eq!(kinds("@"), vec![TokenKind::At, TokenKind::Eof]);
    }

    // ---- comments -------------------------------------------------------

    #[test]
    fn comments_are_skipped() {
        assert_eq!(
            kinds("// line\nstop /* block\n over lines */ ;"),
            vec![
                TokenKind::Ident("stop".into()),
                TokenKind::Semi,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn without_comments_matches_the_comment_free_program() {
        let commented = "// header\ntape num: bits; // trail\n/* block\n   spanning */\nstop;";
        let bare = "tape num: bits; stop;";
        assert_eq!(kinds(commented), kinds(bare));
    }

    #[test]
    fn with_comments_retains_comments_as_interleaved_trivia() {
        // Leading own-line `//`, a trailing `// ...` after code, and a
        // multi-line `/* */` block that starts its own line.
        let src = "// header\nstop; // trail\n/* block\n   spanning */\nhalt;";
        let tokens = lex_with(src, LexMode::WithComments).unwrap();

        // The significant (non-comment) tokens are exactly what `lex` would
        // produce on the comment-free program.
        let significant: Vec<TokenKind> = tokens
            .iter()
            .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
            .map(|t| t.kind.clone())
            .collect();
        assert_eq!(significant, kinds("stop; halt;"));

        let comments: Vec<&Comment> = tokens
            .iter()
            .filter_map(|t| match &t.kind {
                TokenKind::Comment(c) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(comments.len(), 3);

        assert_eq!(comments[0].kind, CommentKind::Line);
        assert_eq!(comments[0].text, "// header");
        assert!(comments[0].own_line, "leading comment starts its own line");

        assert_eq!(comments[1].kind, CommentKind::Line);
        assert_eq!(comments[1].text, "// trail");
        assert!(
            !comments[1].own_line,
            "trailing comment follows code on the same line"
        );

        assert_eq!(comments[2].kind, CommentKind::Block);
        assert_eq!(comments[2].text, "/* block\n   spanning */");
        assert!(
            comments[2].own_line,
            "block comment starts its own line, even though it spans lines"
        );

        // len is the total char count of `text`, for both single-line and
        // multi-line comments.
        for c in &comments {
            let tok = tokens
                .iter()
                .find(|t| matches!(&t.kind, TokenKind::Comment(cc) if cc.text == c.text))
                .unwrap();
            assert_eq!(tok.len, c.text.chars().count() as u32);
        }
    }

    #[test]
    fn comment_errors_fire_in_with_comments_mode_too() {
        let e = lex_with("/* never closed", LexMode::WithComments).unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unterminated")));

        let e = lex_with("stop / halt;", LexMode::WithComments).unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unexpected character `/`"))
        );
    }

    // ---- numbers --------------------------------------------------------

    #[test]
    fn numbers_preserve_digits_as_written() {
        assert_eq!(
            kinds("0 007 126"),
            vec![
                TokenKind::Number(0, "0".into()),
                TokenKind::Number(7, "007".into()),
                TokenKind::Number(126, "126".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn number_range_lexes() {
        assert_eq!(
            kinds("1..125"),
            vec![
                TokenKind::Number(1, "1".into()),
                TokenKind::DotDot,
                TokenKind::Number(125, "125".into()),
                TokenKind::Eof
            ]
        );
    }

    // ---- error positions ------------------------------------------------

    #[test]
    fn error_positions_and_kinds() {
        let e = lex("state s { $ }").unwrap_err();
        assert_eq!((e.span.start.line, e.span.start.col), (1, 11));
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains('$')));

        let e = lex("/* never closed").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unterminated")));

        let e = lex("12abc").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("digit")));

        let e = lex("99999999999").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("too large")));
    }

    #[test]
    fn tokens_carry_char_lengths_and_spans() {
        let tokens = lex("mylib::plusOne 12 идиВКонец").unwrap();
        // mylib(5) ::(2) plusOne(7) 12(2) идиВКонец(9 chars) Eof(0)
        let lens: Vec<u32> = tokens.iter().map(|t| t.len).collect();
        assert_eq!(lens, vec![5, 2, 7, 2, 9, 0]);
        let colon_colon = &tokens[1];
        let s = colon_colon.span();
        assert_eq!((s.start.line, s.start.col, s.end.col), (1, 6, 8));
    }

    // ---- doc / attention lines (the pmc rule, verbatim) -----------------

    #[test]
    fn doc_line_lexes_at_the_start_of_a_line() {
        let tokens = lex("? doc text").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::DocLine("doc text".into()));
        assert_eq!((tokens[0].line, tokens[0].col, tokens[0].len), (1, 1, 10));
    }

    #[test]
    fn attention_line_lexes_at_indent_with_the_sigil_column() {
        let tokens = lex("    ! [deprecated] msg").unwrap();
        assert_eq!(
            tokens[0].kind,
            TokenKind::AttentionLine("[deprecated] msg".into())
        );
        assert_eq!((tokens[0].line, tokens[0].col, tokens[0].len), (1, 5, 18));
    }

    #[test]
    fn bare_doc_sigil_has_an_empty_payload() {
        let tokens = lex("?").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::DocLine("".into()));
        assert_eq!((tokens[0].line, tokens[0].col, tokens[0].len), (1, 1, 1));
    }

    #[test]
    fn doc_line_payload_strips_at_most_one_leading_space() {
        let tokens = lex("?text").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::DocLine("text".into()));

        let tokens = lex("?  text").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::DocLine(" text".into()));
    }

    #[test]
    fn doc_line_after_a_comment_line_lexes_correctly() {
        let tokens = lex("// x\n? y").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::DocLine("y".into()));
        assert_eq!((tokens[0].line, tokens[0].col, tokens[0].len), (2, 1, 3));
    }

    #[test]
    fn bang_mid_line_is_a_bang_not_an_attention_line() {
        // A `!` that is not the line's first non-whitespace char lexes as
        // Bang (the attention-line rule is line-start-only).
        assert_eq!(
            kinds("x !"),
            vec![
                TokenKind::Ident("x".into()),
                TokenKind::Bang,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn question_mark_mid_line_still_errors() {
        // Only a LINE-START `?` is a doc line; anywhere else it is the
        // catch-all "unexpected character" lex error.
        let e = lex("x ? y").unwrap_err();
        assert_eq!((e.span.start.line, e.span.start.col), (1, 3));
        assert!(
            matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unexpected character `?`")),
            "{e:?}"
        );
    }

    #[test]
    fn doc_and_attention_lines_emit_in_both_lex_modes() {
        // Semantic, not trivia: unlike Comment, both LexMode variants emit
        // these tokens.
        let src = "? doc\n! attn";
        for mode in [LexMode::WithoutComments, LexMode::WithComments] {
            let tokens = lex_with(src, mode).unwrap();
            assert_eq!(
                tokens[0].kind,
                TokenKind::DocLine("doc".into()),
                "mode {mode:?}"
            );
            assert_eq!(
                tokens[1].kind,
                TokenKind::AttentionLine("attn".into()),
                "mode {mode:?}"
            );
        }
    }

    // ---- never-panic on arbitrary input ---------------------------------

    proptest! {
        /// The lexer either returns a token stream or a spanned error on any
        /// input — it never panics (no unwrap, no slice, no overflow), in
        /// either mode. pmc's lexer ships no property test; this is the
        /// modest never-panic guard the plan asks for.
        #[test]
        fn never_panics_on_arbitrary_input(src in any::<String>()) {
            let _ = lex_with(&src, LexMode::WithoutComments);
            let _ = lex_with(&src, LexMode::WithComments);
        }
    }
}
