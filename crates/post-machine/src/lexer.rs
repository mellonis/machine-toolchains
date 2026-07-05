//! `.pmc` lexer (spec §3): source text → tokens with line:col.

use crate::compiler::{CompileError, CompileErrorKind};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
    pub col: u32,
}

/// Identifier rule (spec §3.1): Unicode; first char alphabetic or `_`,
/// then alphanumeric or `_` — the same classes as the `.pma` symbol
/// grammar, so every `.pmc` name survives the trip through generated
/// assembly.
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
        } else {
            self.col += 1;
        }
        Some(c)
    }
}

fn err(line: u32, col: u32, message: String) -> CompileError {
    CompileError {
        line,
        col,
        kind: CompileErrorKind::Lex(message),
    }
}

pub fn lex(source: &str) -> Result<Vec<Token>, CompileError> {
    let mut cur = Cursor {
        chars: source.chars().peekable(),
        line: 1,
        col: 1,
    };
    let mut tokens = Vec::new();

    while let Some(c) = cur.peek() {
        let (line, col) = (cur.line, cur.col);
        if c.is_whitespace() {
            cur.bump();
            continue;
        }
        if c == '/' {
            cur.bump();
            match cur.peek() {
                Some('/') => {
                    while let Some(c) = cur.bump() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                Some('*') => {
                    cur.bump();
                    let mut prev = '\0';
                    let mut closed = false;
                    while let Some(c) = cur.bump() {
                        if prev == '*' && c == '/' {
                            closed = true;
                            break;
                        }
                        prev = c;
                    }
                    if !closed {
                        return Err(err(line, col, "unterminated block comment".into()));
                    }
                }
                _ => return Err(err(line, col, "unexpected character `/`".into())),
            }
            continue;
        }
        if c == ':' {
            cur.bump();
            let kind = if cur.peek() == Some(':') {
                cur.bump();
                TokenKind::ColonColon
            } else {
                TokenKind::Colon
            };
            tokens.push(Token { kind, line, col });
            continue;
        }
        let single = match c {
            '@' => Some(TokenKind::At),
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
            tokens.push(Token { kind, line, col });
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
            tokens.push(Token {
                kind: TokenKind::Ident(name),
                line,
                col,
            });
            continue;
        }
        return Err(err(line, col, format!("unexpected character `{c}`")));
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        line: cur.line,
        col: cur.col,
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
        assert_eq!((e.line, e.col), (1, 7));
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains('$')));

        let e = lex("/* never closed").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unterminated")));

        let e = lex("12abc").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("digit")));

        let e = lex("99999999999").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("too large")));
    }
}
