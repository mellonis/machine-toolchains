//! `.pmc` lexer (docs/language.md): source text → tokens with line:col.

use crate::compiler::{CompileError, CompileErrorKind};
use mtc_core::diagnostics::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident(String),
    Number(u32),
    At,
    Bang,
    Comma,
    Semi,
    Colon,
    /// `::` — lexed greedily; a single `:` stays [`TokenKind::Colon`],
    /// so numeric labels (`1:`) are unaffected.
    ColonColon,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Eof,
    /// Only produced in [`LexMode::WithComments`]. [`lex`] (equivalent to
    /// `lex_with(_, LexMode::WithoutComments)`) never emits this variant,
    /// so callers on the default path see an unchanged token stream.
    Comment(Comment),
}

/// Which delimiter pair produced a [`Comment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// `// ...` to end of line.
    Line,
    /// `/* ... */`, possibly spanning multiple source lines.
    Block,
}

/// A comment retained as trivia (docs/language.md), produced only in
/// [`LexMode::WithComments`].
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
/// [`lex`] does) or retains them as [`TokenKind::Comment`] trivia (for a
/// future formatter).
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
    /// Length in characters. Every token is single-line and 0 only for
    /// Eof — EXCEPT a [`TokenKind::Comment`] of [`CommentKind::Block`],
    /// which may span multiple source lines. For those, `len` is the
    /// total character count of `Comment::text` (delimiters and any
    /// embedded newlines all counted), and [`Token::span`]'s end
    /// position is not meaningful past the first line.
    pub len: u32,
}

impl Token {
    /// End-exclusive span of this token's source text.
    pub fn span(&self) -> Span {
        Span::new(self.line, self.col, self.line, self.col + self.len)
    }
}

/// Identifier rule (docs/language.md): Unicode; first char alphabetic or
/// `_`, then alphanumeric or `_` — the same classes as the `.pma` symbol
/// grammar (docs/formats.md (assembly text)), so every `.pmc` name
/// survives the trip through generated assembly.
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
    /// newline (or since the start of input). Read at the top of the
    /// main loop — before a comment's own characters are bumped, which
    /// would otherwise flip it to false — to compute `Comment::own_line`.
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
        if c == '/' {
            cur.bump();
            match cur.peek() {
                Some('/') => {
                    cur.bump();
                    // Built regardless of mode (harmless when discarded)
                    // so the consumption loop below is identical either
                    // way — the WithoutComments token stream depends
                    // only on whether the Comment token is pushed.
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
        if c == '@' {
            cur.bump();
            // Sigil adjacency (docs/language.md): `@` is part of the
            // callee name's spelling — whitespace, digits, punctuation,
            // comments, or end of input after it are lex errors.
            if !cur.peek().is_some_and(is_ident_start) {
                return Err(err(
                    line,
                    col,
                    "expected a function name immediately after `@`".into(),
                ));
            }
            tokens.push(Token {
                kind: TokenKind::At,
                line,
                col,
                len: 1,
            });
            continue;
        }
        let single = match c {
            '!' => Some(TokenKind::Bang),
            ',' => Some(TokenKind::Comma),
            ';' => Some(TokenKind::Semi),
            '(' => Some(TokenKind::LParen),
            ')' => Some(TokenKind::RParen),
            '{' => Some(TokenKind::LBrace),
            '}' => Some(TokenKind::RBrace),
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
            tokens.push(Token {
                kind: TokenKind::Number(value),
                line,
                col,
                len: digits.len() as u32, // ASCII digits: bytes == chars
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

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_the_shape_of_a_function() {
        use TokenKind::*;
        assert_eq!(
            kinds("f() { 1: right(!); }"),
            vec![
                Ident("f".into()),
                LParen,
                RParen,
                LBrace,
                Number(1),
                Colon,
                Ident("right".into()),
                LParen,
                Bang,
                RParen,
                Semi,
                RBrace,
                Eof,
            ]
        );
    }

    #[test]
    fn tracks_line_and_column() {
        let tokens = lex("f()\n{\n  goto 7;\n}").unwrap();
        let goto = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Ident("goto".into()))
            .unwrap();
        assert_eq!((goto.line, goto.col), (3, 3));
        let seven = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Number(7))
            .unwrap();
        assert_eq!((seven.line, seven.col), (3, 8));
    }

    #[test]
    fn unicode_identifiers() {
        assert_eq!(
            kinds("идиВКонец()"),
            vec![
                TokenKind::Ident("идиВКонец".into()),
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(
            kinds("// line\nleft /* block\n over lines */ ;"),
            vec![
                TokenKind::Ident("left".into()),
                TokenKind::Semi,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn without_comments_matches_the_comment_free_program() {
        let commented = "// header\nleft(!); // trail\n/* block\n   spanning */\nright(!);";
        let bare = "left(!); right(!);";
        assert_eq!(kinds(commented), kinds(bare));
    }

    #[test]
    fn with_comments_retains_comments_as_interleaved_trivia() {
        // Leading own-line `//`, a trailing `// ...` after code, and a
        // multi-line `/* */` block that starts its own line.
        let src = "// header\nleft(!); // trail\n/* block\n   spanning */\nright(!);";
        let tokens = lex_with(src, LexMode::WithComments).unwrap();

        // The significant (non-comment) tokens are exactly what `lex`
        // would produce on the comment-free program.
        let significant: Vec<TokenKind> = tokens
            .iter()
            .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
            .map(|t| t.kind.clone())
            .collect();
        assert_eq!(significant, kinds("left(!); right(!);"));

        // Comment tokens are interleaved at the right positions: index 0
        // (before any code), index 6 (after the first statement's `;`,
        // same line), index 7 (its own line, right before `right`).
        let comment_positions: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| matches!(t.kind, TokenKind::Comment(_)))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(comment_positions, vec![0, 6, 7]);

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
        assert_eq!((tokens[0].line, tokens[0].col), (1, 1));

        assert_eq!(comments[1].kind, CommentKind::Line);
        assert_eq!(comments[1].text, "// trail");
        assert!(
            !comments[1].own_line,
            "trailing comment follows code on the same line"
        );
        assert_eq!((tokens[6].line, tokens[6].col), (2, 10));

        assert_eq!(comments[2].kind, CommentKind::Block);
        assert_eq!(comments[2].text, "/* block\n   spanning */");
        assert!(
            comments[2].own_line,
            "block comment starts its own line, even though it spans lines"
        );
        assert_eq!((tokens[7].line, tokens[7].col), (3, 1));

        // len is the total char count of `text` (documented convention),
        // for both single-line and multi-line comments.
        for (tok, comment) in [
            (&tokens[0], comments[0]),
            (&tokens[6], comments[1]),
            (&tokens[7], comments[2]),
        ] {
            assert_eq!(tok.len, comment.text.chars().count() as u32);
        }
    }

    #[test]
    fn comment_errors_fire_in_with_comments_mode_too() {
        let e = lex_with("/* never closed", LexMode::WithComments).unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unterminated")));

        let e = lex_with("left(!) / right(!);", LexMode::WithComments).unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unexpected character `/`"))
        );
    }

    #[test]
    fn colon_colon_is_greedy_and_labels_keep_single_colons() {
        assert_eq!(
            kinds("std::api"),
            vec![
                TokenKind::Ident("std".into()),
                TokenKind::ColonColon,
                TokenKind::Ident("api".into()),
                TokenKind::Eof
            ]
        );
        assert_eq!(
            kinds("1:"),
            vec![TokenKind::Number(1), TokenKind::Colon, TokenKind::Eof]
        );
        // Greedy: `:::` is `::` then `:`.
        assert_eq!(
            kinds(":::"),
            vec![TokenKind::ColonColon, TokenKind::Colon, TokenKind::Eof]
        );
    }

    #[test]
    fn error_positions_and_kinds() {
        let e = lex("f() { $ }").unwrap_err();
        assert_eq!((e.span.start.line, e.span.start.col), (1, 7));
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
        let tokens = lex("std::api 12 идиВКонец").unwrap();
        // std (len 3) :: (len 2) api (len 3) 12 (len 2) идиВКонец (len 9, chars)
        let lens: Vec<u32> = tokens.iter().map(|t| t.len).collect();
        assert_eq!(lens, vec![3, 2, 3, 2, 9, 0]); // trailing 0 = Eof
        let colon_colon = &tokens[1];
        let s = colon_colon.span();
        assert_eq!((s.start.line, s.start.col, s.end.col), (1, 4, 6));
    }

    #[test]
    fn sigil_must_touch_the_callee_name() {
        for src in [
            "f() { @ qq(); }", // space after @
            "f() { @5(); }",   // digit after @
            "f() { @(); }",    // punctuation after @
            "@",               // trailing @
        ] {
            let e = lex(src).unwrap_err();
            assert!(
                matches!(e.kind, CompileErrorKind::Lex(ref m)
                    if m.contains("immediately after")),
                "{src} should be a lex error about sigil adjacency, got {e:?}"
            );
        }
    }

    #[test]
    fn tight_sigil_still_lexes() {
        let tokens = lex("@qq()").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::At);
        assert_eq!(tokens[1].kind, TokenKind::Ident("qq".into()));
    }
}
