//! The `.tmc` formatter's objective guard: on every `.tmc` source in the
//! repository — the Appendix-A examples, the nested-graft fixture, and the
//! embedded standard library — formatting must be IDEMPOTENT and must not
//! change a single token.
//!
//! "Not a single token" is checked by re-lexing the formatted text and
//! comparing the token stream with the original's, not by checking that the
//! output still parses: a printer that dropped a `move` vector or rewrote a
//! number's spelling would still parse fine.

use mtc_turing_machine::fmt::format;
use mtc_turing_machine::lexer::{Comment, LexMode, Token, TokenKind, lex_with};

/// Every `.tmc` source the repository ships, as (name, text).
fn corpus() -> Vec<(String, String)> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");
    let mut out: Vec<(String, String)> = std::fs::read_dir(root)
        .expect("the golden directory exists")
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| path.extension().and_then(|x| x.to_str()) == Some("tmc"))
        .map(|path| {
            (
                path.file_name()
                    .expect("a fixture has a file name")
                    .to_string_lossy()
                    .into_owned(),
                std::fs::read_to_string(&path).expect("a readable fixture"),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.push((
        "std.tmc".to_string(),
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/stdlib/std.tmc"
        ))
        .expect("the embedded stdlib source is readable"),
    ));
    out
}

/// A token reduced to what a whitespace-only reprint must preserve. Comment
/// text is compared with each line's trailing whitespace stripped — that is
/// the one normalization the printer applies to trivia, and it is
/// whitespace-only by construction.
#[derive(Debug, PartialEq, Eq)]
enum Sig {
    Kind(TokenKind),
    Comment { text: String, own_line: bool },
}

fn signature(tokens: &[Token]) -> Vec<Sig> {
    tokens
        .iter()
        .map(|t| match &t.kind {
            TokenKind::Comment(Comment {
                text,
                kind,
                own_line,
            }) => Sig::Comment {
                text: format!(
                    "{kind:?}:{}",
                    text.split('\n')
                        .map(str::trim_end)
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
                own_line: *own_line,
            },
            other => Sig::Kind(other.clone()),
        })
        .collect()
}

fn token_signature(source: &str) -> Vec<Sig> {
    signature(&lex_with(source, LexMode::WithComments).expect("the source lexes"))
}

#[test]
fn every_tmc_source_formats_idempotently() {
    for (name, source) in corpus() {
        let once = format(&source).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let twice = format(&once).unwrap_or_else(|e| panic!("{name} (second pass): {e:?}"));
        assert_eq!(once, twice, "{name}: fmt is not idempotent");
    }
}

#[test]
fn formatting_never_changes_a_token() {
    for (name, source) in corpus() {
        let formatted = format(&source).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            token_signature(&source),
            token_signature(&formatted),
            "{name}: the formatted text does not lex to the same token stream"
        );
    }
}

#[test]
fn every_tmc_source_ends_in_exactly_one_newline() {
    for (name, source) in corpus() {
        let formatted = format(&source).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(formatted.ends_with('\n'), "{name}: no final newline");
        assert!(
            !formatted.ends_with("\n\n"),
            "{name}: a blank line before EOF"
        );
    }
}

#[test]
fn no_line_carries_trailing_whitespace() {
    for (name, source) in corpus() {
        let formatted = format(&source).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        for (n, line) in formatted.lines().enumerate() {
            assert_eq!(
                line.trim_end(),
                line,
                "{name}:{}: trailing whitespace",
                n + 1
            );
        }
    }
}
