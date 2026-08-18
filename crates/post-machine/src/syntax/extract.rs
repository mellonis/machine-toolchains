//! Retokenization: rebuilding a real [`Token`] slice from a green
//! subtree's own descendant tokens, so extraction can reuse the C1
//! parser's existing productions (`crate::parser::reparse_item`/
//! `reparse_doc_items`) instead of re-deriving their grammar decisions
//! (docs/core.md (syntax tree)).

use mtc_core::syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextLineIndex};

use super::kinds::PmcKind;
use crate::lexer::{Token, TokenKind, normalize_doc_payload};

/// The three trivia kinds `sig_tokens` filters out before mapping —
/// whitespace and both comment kinds. Every significant token kind, and
/// every node kind, is `false`.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "wired into extract_function in the next task of this plan; \
                   exercised today only by this module's own tests"
    )
)]
fn is_trivia(kind: SyntaxKind) -> bool {
    kind == PmcKind::Whitespace.into()
        || kind == PmcKind::LineComment.into()
        || kind == PmcKind::BlockComment.into()
}

/// The sigil's own byte length, for slicing a `DOC_LINE`/`ATTENTION_LINE`
/// token's verbatim text past it. `?`/`!` are one-byte ASCII characters,
/// but this reads `char::len_utf8` off the token's actual first
/// character rather than assuming `1`, so a byte-index slice of a
/// multi-byte-adjacent string never lands off a char boundary.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "wired into extract_function in the next task of this plan; \
                   exercised today only by this module's own tests"
    )
)]
fn sigil_len(text: &str) -> usize {
    text.chars()
        .next()
        .expect(
            "DOC_LINE/ATTENTION_LINE token text is never empty — it always carries its own sigil",
        )
        .len_utf8()
}

/// Every non-trivia descendant token of `node`, rebuilt as a real
/// [`Token`] — the retokenization half of extraction. `node` is a green
/// subtree already known to have parsed once (the tree only exists
/// because it did); this turns it back into the same token shape the C1
/// parser's productions accept. Ends with a synthetic `Eof` at `node`'s
/// own end position, matching every real token stream's own convention.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "wired into extract_function in the next task of this plan; \
                   exercised today only by this module's own tests"
    )
)]
fn sig_tokens(node: &SyntaxNode, index: &TextLineIndex) -> Vec<Token> {
    let mut tokens: Vec<Token> = node
        .descendant_tokens()
        .filter(|t| !is_trivia(t.kind()))
        .map(|t| token_from_syntax(&t, index))
        .collect();
    let (line, col) = index.line_col(node.text_range().end);
    tokens.push(Token {
        kind: TokenKind::Eof,
        line,
        col,
        len: 0,
    });
    tokens
}

/// One green token → one [`Token`]: kind mapped 1:1 from [`PmcKind`]
/// (the two payload-carrying kinds rebuild their payload from the
/// token's own verbatim text — a green `DOC_LINE`/`ATTENTION_LINE` token
/// carries the RAW source line, sigil included and payload
/// un-normalized, per `super::layout`'s end-of-line rule; the lexer's
/// own [`normalize_doc_payload`] rebuilds the exact normalized payload a
/// real lexer token would carry), position from `index`, length in
/// chars (matching every lexer-built [`Token::len`]'s own convention).
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "wired into extract_function in the next task of this plan; \
                   exercised today only by this module's own tests"
    )
)]
fn token_from_syntax(t: &SyntaxToken, index: &TextLineIndex) -> Token {
    let (line, col) = index.line_col(t.text_range().start);
    let text = t.text();
    let kind = match t.kind() {
        k if k == PmcKind::Ident.into() => TokenKind::Ident(text.to_string()),
        k if k == PmcKind::Number.into() => {
            TokenKind::Number(text.parse().expect("lexed digits"), text.to_string())
        }
        k if k == PmcKind::At.into() => TokenKind::At,
        k if k == PmcKind::Bang.into() => TokenKind::Bang,
        k if k == PmcKind::Comma.into() => TokenKind::Comma,
        k if k == PmcKind::Semi.into() => TokenKind::Semi,
        k if k == PmcKind::Colon.into() => TokenKind::Colon,
        k if k == PmcKind::ColonColon.into() => TokenKind::ColonColon,
        k if k == PmcKind::LParen.into() => TokenKind::LParen,
        k if k == PmcKind::RParen.into() => TokenKind::RParen,
        k if k == PmcKind::LBrace.into() => TokenKind::LBrace,
        k if k == PmcKind::RBrace.into() => TokenKind::RBrace,
        k if k == PmcKind::DocLine.into() => {
            TokenKind::DocLine(normalize_doc_payload(&text[sigil_len(text)..]))
        }
        k if k == PmcKind::AttentionLine.into() => {
            TokenKind::AttentionLine(normalize_doc_payload(&text[sigil_len(text)..]))
        }
        // Trivia is filtered out by `sig_tokens` before this is ever
        // called; `File` and every other node kind never reach here —
        // `SyntaxNode::descendant_tokens` yields tokens only, never
        // nodes.
        _ => unreachable!("sig_tokens excludes every kind but the significant token kinds"),
    };
    Token {
        kind,
        line,
        col,
        len: text.chars().count() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cst::{BodyKind, DocRunKind, TopKind};
    use crate::lexer::{LexMode, lex, lex_with};
    use crate::parser::{Item, parse_cst, parse_green, reduce_doc_run};
    use crate::syntax::{FileView, ItemView, StatementView, TopView};
    use mtc_core::syntax::AstNode;

    /// Retokenized items re-parse to the EXACT C1 Item — spans included.
    ///
    /// The brief's originally proposed snippet (`right(2), goto 1` as
    /// ONE comma group) does not parse: a comma-group entry that carries
    /// a successor must be the group's LAST entry (docs/pmt/language.md),
    /// and `goto` can never appear inside a group at all — `right(2)`
    /// already breaks the first rule before `goto` is even reached
    /// (`Parser::statement`'s comma-loop group-position check fires on
    /// the FIRST entry's own trailing successor, independently of
    /// `goto`'s own `in_group` rejection). Recorded substitution: two
    /// plain (ungrouped) statements for `right(2)` (a parenthesised
    /// successor) and `goto 1`, plus a THIRD statement reusing
    /// `tests/syntax/rich.pmc`'s own proven-valid comma group
    /// (`right, mark;`) so the `in_group: true` path — untested by
    /// either of the first two statements — gets its own pin: the
    /// group's second entry (`mark`) is reparsed with `in_group: true`,
    /// mirroring `Parser::statement`'s own rule (`item(false)` for a
    /// statement's first entry, `item(true)` for every entry after a
    /// comma).
    #[test]
    fn reparsed_item_equals_the_c1_item() {
        let src = "main() {\n1: right(2);\ngoto 1;\n2: right, mark;\n}\n";

        // C1 side: every statement's items, straight from parse_cst.
        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let TopKind::Function(func) = &cst.items[0].kind else {
            panic!("expected a top-level function");
        };
        let c1_items: Vec<Item> = func
            .body
            .iter()
            .filter_map(|bi| match &bi.kind {
                BodyKind::Statement(s) => Some(s.items.iter().map(|ci| ci.item.clone())),
                _ => None,
            })
            .flatten()
            .collect();

        // Green side: retokenize each ITEM node and re-parse. `in_group`
        // is derived from the item's own position within its statement
        // — index 0 is never grouped, a later index always is — the
        // same rule `Parser::statement` itself applies.
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let index = TextLineIndex::new(src);
        let file = FileView::cast(root).expect("root is FILE");
        let TopView::Function(func_view) = file.items().next().expect("one item") else {
            panic!("expected a function");
        };

        // Pin sig_tokens's own Eof position and the trivia filter,
        // independent of reparse_item's own use of them: neither
        // `item()` nor `reparse_doc_items` ever reads Eof's line/col
        // (only its kind, as a terminator), so without this check the
        // Eof-position/trivia-filter plumbing in `sig_tokens`/
        // `token_from_syntax` could be silently wrong and every
        // assertion above would still pass. The first statement's only
        // item, `right(2)`, sits at columns 4..11 on line 2 of `src`
        // (`"1: right(2);\n"` — `r` is col 4, the closing `)` is col
        // 11); the node's end-exclusive offset lands on the `;` at col
        // 12, so the synthetic Eof must report `(line: 2, col: 12,
        // len: 0)`. The token count (5, not more) additionally proves
        // the interior whitespace-free `right(2)` carries no smuggled
        // trivia token.
        let first_item = func_view
            .statements()
            .next()
            .expect("first statement")
            .items()
            .next()
            .expect("first item");
        let first_item_tokens = sig_tokens(first_item.syntax(), &index);
        assert_eq!(
            first_item_tokens.len(),
            5,
            "Ident, LParen, Number, RParen, Eof — trivia excluded"
        );
        let eof = first_item_tokens.last().expect("sig_tokens appends Eof");
        assert_eq!(eof.kind, TokenKind::Eof);
        assert_eq!((eof.line, eof.col, eof.len), (2, 12, 0));

        let green_items: Vec<Item> = func_view
            .statements()
            .flat_map(|s: StatementView| {
                s.items()
                    .enumerate()
                    .map(|(pos, iv): (usize, ItemView)| {
                        crate::parser::reparse_item(&sig_tokens(iv.syntax(), &index), pos > 0)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        assert_eq!(green_items, c1_items);
    }

    /// Retokenized `DOC_RUN` tokens re-parse to the EXACT C1 doc-run
    /// items, `blank_before` included — for THIS comment-free snippet;
    /// see `reparse_doc_items`'s own doc comment for why a
    /// comment-interleaved run can't hold to raw item-for-item equality
    /// (`reparsed_doc_items_reduce_to_the_same_fndoc_when_comments_interleave`
    /// below covers that case, at the [`crate::parser::reduce_doc_run`]
    /// level instead). This run is the file's very first item, so both
    /// sides start `doc_run`'s own `prev_end_line` gap-tracking from the
    /// same fresh `0` — `reparse_doc_items` always starts there (an
    /// isolated retokenized slice has no "rest of the file" position to
    /// inherit), and the ORIGINAL parse's own `self.prev_end_line` is
    /// still `0` at this point too, since nothing precedes the run.
    /// Exercises `reparse_doc_items` end to end: `sig_tokens` over a
    /// `DOC_RUN` node, the `DocLine`/bare-`AttentionLine` conversion,
    /// AND — the line carrying `[deprecated]` — `Parser::parse_attr`
    /// actually returning `Some(AttrCst)`. A bare `!` line alone would
    /// leave `attr: None` on every item, which can't discriminate a
    /// wrong `sig_tokens` `len`: `parse_attr` locates the attribute's
    /// `[` column as `token.len - 1 - text.chars().count()`, so only the
    /// `Some` arm actually exercises that arithmetic against the
    /// green-tree-derived `len`.
    #[test]
    fn reparsed_doc_items_equal_the_c1_doc_run() {
        let src = "? doc line\n! caution\n! [deprecated] use goToEnd\nmain() { right; }\n";

        // C1 side: the bound doc run, straight from parse_cst.
        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let TopKind::Function(func) = &cst.items[0].kind else {
            panic!("expected a top-level function");
        };
        let c1_doc_run = func.doc_run.clone();

        // Green side: retokenize the DOC_RUN node and re-parse.
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let index = TextLineIndex::new(src);
        let file = FileView::cast(root).expect("root is FILE");
        let TopView::Function(func_view) = file.items().next().expect("one item") else {
            panic!("expected a function");
        };
        let doc_run_view = func_view.doc_run().expect("function carries a doc run");
        let green_doc_run =
            crate::parser::reparse_doc_items(&sig_tokens(doc_run_view.syntax(), &index));

        assert_eq!(green_doc_run, c1_doc_run);
    }

    /// A comment interleaved inside a `DOC_RUN` (`(Doc, Comment, Doc)`
    /// on the C1 side) cannot survive retokenization — `sig_tokens`
    /// drops it as trivia, so the green side's raw `Vec<DocRunItem>` is
    /// `(Doc, Doc)`, strictly shorter than C1's. What Task 4/6 actually
    /// rely on is `reduce_doc_run` equality, not raw item equality —
    /// this proves that equality holds anyway, because
    /// `DocRunKind::Comment` is fully inert in `reduce_doc_run`
    /// regardless of position (see that function's own doc comment):
    /// dropping the comment can never change the reduced [`FnDoc`].
    #[test]
    fn reparsed_doc_items_reduce_to_the_same_fndoc_when_comments_interleave() {
        let src = "? doc line\n// interleaved comment\n? more doc\nmain() { right; }\n";

        // C1 side: the bound doc run, comment item included, reduced.
        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let TopKind::Function(func) = &cst.items[0].kind else {
            panic!("expected a top-level function");
        };
        assert!(
            func.doc_run
                .iter()
                .any(|i| matches!(i.kind, DocRunKind::Comment(_))),
            "fixture must actually interleave a comment, or this test proves nothing: {:?}",
            func.doc_run
        );
        let c1_doc = reduce_doc_run(&func.doc_run);

        // Green side: retokenize (the comment is dropped as trivia) and
        // reduce.
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let index = TextLineIndex::new(src);
        let file = FileView::cast(root).expect("root is FILE");
        let TopView::Function(func_view) = file.items().next().expect("one item") else {
            panic!("expected a function");
        };
        let doc_run_view = func_view.doc_run().expect("function carries a doc run");
        let green_doc_run =
            crate::parser::reparse_doc_items(&sig_tokens(doc_run_view.syntax(), &index));
        assert_ne!(
            green_doc_run.len(),
            func.doc_run.len(),
            "the comment must actually be dropped, or this test proves nothing"
        );
        let green_doc = reduce_doc_run(&green_doc_run);

        assert_eq!(green_doc, c1_doc);
    }

    /// Drift guard: `token_from_syntax`'s `PmcKind -> TokenKind` mapping
    /// stays the exact inverse of `sig_kind`'s `TokenKind -> PmcKind`
    /// mapping (`Parser::bump`'s green-emission counterpart, private to
    /// `parser.rs`) for all 14 significant kinds. Rather than duplicate
    /// `sig_kind`'s table by hand (itself a drift risk), this exercises
    /// every kind at once through a real round trip: retokenizing the
    /// green tree's FILE root reproduces `lex`'s own real token stream
    /// exactly, kind for kind, position for position — a snippet
    /// engineered to contain all 14 kinds at least once (a qualified
    /// `use` for `ColonColon`, a doc run and an attention run for
    /// `DocLine`/`AttentionLine`, a bare `!` successor for `Bang`
    /// distinctly from an attention line's leading `!`, a comma group,
    /// and an `@`-call for `At`).
    #[test]
    fn sig_tokens_over_the_file_root_matches_lex_for_every_significant_kind() {
        let src = "use std::goToEnd;\n? doc\n! caution\nmain() {\n1: right(!);\ngoto 1;\n2: right, mark;\n@goToEnd();\n}\n";

        let lexed = lex(src).expect("lexes");
        let seen: std::collections::HashSet<&'static str> = lexed
            .iter()
            .map(|t| match &t.kind {
                TokenKind::Ident(_) => "Ident",
                TokenKind::Number(..) => "Number",
                TokenKind::At => "At",
                TokenKind::Bang => "Bang",
                TokenKind::Comma => "Comma",
                TokenKind::Semi => "Semi",
                TokenKind::Colon => "Colon",
                TokenKind::ColonColon => "ColonColon",
                TokenKind::LParen => "LParen",
                TokenKind::RParen => "RParen",
                TokenKind::LBrace => "LBrace",
                TokenKind::RBrace => "RBrace",
                TokenKind::DocLine(_) => "DocLine",
                TokenKind::AttentionLine(_) => "AttentionLine",
                TokenKind::Eof | TokenKind::Comment(_) => "",
            })
            .collect();
        for kind in [
            "Ident",
            "Number",
            "At",
            "Bang",
            "Comma",
            "Semi",
            "Colon",
            "ColonColon",
            "LParen",
            "RParen",
            "LBrace",
            "RBrace",
            "DocLine",
            "AttentionLine",
        ] {
            assert!(seen.contains(kind), "fixture is missing a {kind} token");
        }

        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let index = TextLineIndex::new(src);
        let retokenized = sig_tokens(&root, &index);

        assert_eq!(retokenized, lexed);
    }
}
