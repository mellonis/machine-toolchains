//! Extraction: rebuilding the C1 `parser::Function`/`Statement`/`Label`
//! AST straight from typed views over the green tree, function by
//! function (docs/core.md (syntax tree)). Two halves:
//!
//! - **Retokenization** (`sig_tokens`/`token_from_syntax`): rebuilding a
//!   real [`Token`] slice from a green subtree's own descendant tokens,
//!   so extraction can reuse the C1 parser's existing productions
//!   (`crate::parser::reparse_item`/`reparse_doc_items`) instead of
//!   re-deriving their grammar decisions.
//! - **Assembly** ([`extract_function`]): walking the views themselves
//!   (headers, statements, labels, nesting) and mirroring
//!   `crate::parser::lower_function`'s own container-building decisions
//!   exactly — never re-deriving a rule the parser already encodes,
//!   only naming the shape the green tree already carries.

use mtc_core::syntax::{AstNode, SyntaxKind, SyntaxNode, SyntaxToken, TextLineIndex, TextRange};

use super::kinds::PmcKind;
use super::views::{
    FileView, FunctionView, ItemView, LabelView, StatementView, TopView, UsePathView,
};
use crate::lexer::{Token, TokenKind, normalize_doc_payload};
use crate::parser::{
    Function, Import, Label, Program, Statement, reduce_doc_run, reparse_doc_items, reparse_item,
};

/// The three trivia kinds `sig_tokens` filters out before mapping —
/// whitespace and both comment kinds. Every significant token kind, and
/// every node kind, is `false`.
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

/// True iff `node`'s own direct parent is another FUNCTION node — a
/// nested definition, per `FunctionView::nested`'s own "direct children
/// only" contract. A FUNCTION at file or namespace level (the shape
/// `FileView`/`NamespaceView::items` yield) is never nested.
fn is_nested(node: &SyntaxNode) -> bool {
    node.parent()
        .is_some_and(|p| p.kind() == PmcKind::Function.into())
}

/// True iff `node`'s own direct parent is a NAMESPACE node — i.e. this
/// (necessarily top-level, per `is_nested`) FUNCTION was parsed with a
/// non-empty enclosing `ns` path. Namespaces never nest inside a
/// function, so — unlike a general "any NAMESPACE ancestor" walk — the
/// direct parent alone settles it: `Parser::top_items`'s own recursion
/// (parser.rs:1300) only ever calls itself with an EXTENDED (non-empty)
/// `ns`, so a FUNCTION whose immediate container is a NAMESPACE always
/// has `ns.is_empty() == false`, regardless of how deeply that
/// namespace itself is nested.
fn is_namespaced(node: &SyntaxNode) -> bool {
    node.parent()
        .is_some_and(|p| p.kind() == PmcKind::Namespace.into())
}

/// One label `N:` — `crate::parser::Label`'s own doc: span runs from
/// the number's start to the colon's END, spanning any interior
/// whitespace.
fn extract_label(view: &LabelView, index: &TextLineIndex) -> Label {
    let number_tok = view.number_token();
    let colon_tok = view.colon_token();
    let written = number_tok.text().to_string();
    let value = written
        .parse()
        .expect("LABEL's NUMBER token text is always digits (docs/pmt/language.md (labels))");
    Label {
        value,
        span: index.span(TextRange::new(
            number_tok.text_range().start,
            colon_tok.text_range().end,
        )),
        written,
    }
}

/// One `;`-terminated statement. `span` is the STATEMENT node's own
/// extent taken directly from the tree: `Parser::statement`'s green
/// checkpoint (`cp`, parser.rs:1531) is captured before the label loop
/// and before the first item — the same point `StatementCst::span`
/// (cst.rs) starts from (the first label if any, else the first item) —
/// and the node closes right after the trailing `;` is bumped
/// (parser.rs:1679, after `self.statement(...)` returns), so the node's
/// start/end already equal `StatementCst::span`'s "first token through
/// `;` end" exactly; no separate first/last-token lookup is needed.
/// `line`, however, is NOT the node's own start line — `Statement::line`
/// is always the first ITEM's line (`Parser::statement` reads
/// `self.peek().line` right after the label loop, parser.rs:1715),
/// which differs from a label's own line whenever the author put the
/// first command on its own line after the label (`label_break`).
///
/// `pub(crate)` because the `.pmc` language service needs one
/// statement's item internals — label references, a call's name — and
/// must get them the same way extraction does, through the parser's own
/// production (docs/lsp.md (semantic tokens)). Re-deriving that
/// enumeration over views instead would duplicate grammar the parser
/// already owns.
pub(crate) fn extract_statement(view: &StatementView, index: &TextLineIndex) -> Statement {
    let labels = view.labels().map(|l| extract_label(&l, index)).collect();
    let item_views: Vec<ItemView> = view.items().collect();
    // `Parser::statement` parses a statement's first entry with
    // `item(false)` and every following comma-separated entry with
    // `item(true)` (parser.rs:1723, 1787) — but `in_group` only ever
    // changes what `Parser::item` accepts for ONE shape, `goto`
    // (parser.rs:1881: `in_group` rejects it outright), and `goto` can
    // never be anything but a multi-item statement's SOLE item — the
    // comma loop rejects a `,` after any `Goto` unconditionally
    // (parser.rs:1756-1761, "goto cannot appear in a comma group", no
    // last-position exception). So on any tree that already parsed once,
    // a multi-item statement's first entry can never be `goto` either,
    // and passing `in_group: true` to every entry of such a statement —
    // first included — reproduces the exact same `Item` the original
    // per-entry `false`/`true` split produced. Controller-ruled
    // simplification, verified against `Parser::statement`'s code as
    // above.
    let in_group = item_views.len() > 1;
    let items = item_views
        .iter()
        .map(|iv| reparse_item(&sig_tokens(iv.syntax(), index), in_group))
        .collect();
    let line = index
        .line_col(
            item_views
                .first()
                .expect("Parser::statement always parses at least one item")
                .syntax()
                .text_range()
                .start,
        )
        .0;
    Statement {
        labels,
        items,
        line,
        span: index.span(view.syntax().text_range()),
    }
}

/// Rebuild one C1 `parser::Function` from its green-tree view — mirrors
/// `crate::parser::lower_function` (parser.rs:441) exactly.
///
/// `name`/`name_span`/`line`/`col` come from the header's name token
/// (`FunctionView::header`'s own contextual-keyword decode already
/// picked it out). `exported`/`volatile` mirror
/// `Parser::top_items`'s stamping (parser.rs:1377-1381) — the ONLY call
/// path that ever sets either flag to something other than the
/// all-`false` defaults `Parser::function` itself initializes a
/// `FunctionCst` with (parser.rs:1696-1698, `Ok(FunctionCst { ...
/// has_volatile: false, exported: false, has_export: false, ... })`).
/// That stamping never runs for a nested definition — `function`'s own
/// nested-definition branch calls itself directly
/// (`self.function(None, doc_run)`) and stores the result unstamped —
/// so `exported`/`volatile` are unconditionally `false` whenever this
/// FUNCTION's own parent is another FUNCTION, matching
/// `lower_function`'s own recursive call for nested children
/// (`lower_function(g, &[])`, parser.rs:453) exactly. For a top-level
/// FUNCTION (file- or namespace-level parent), `exported` is the
/// literal `export` keyword OR the un-namespaced top-level `main`
/// auto-export (`ns.is_empty() && name == "main"`, decided here via
/// `is_namespaced` rather than a threaded `ns` path — see that
/// function's own doc); `volatile` is the literal `volatile` keyword
/// (never auto-applied, unlike `exported` — parser.rs:236-243's own
/// `FunctionCst::has_volatile` doc).
///
/// `nested` hoists nested definitions out of body order (recursing into
/// this same function, so an arbitrarily deep nesting chain unwinds the
/// same way `lower_function`'s own recursion does), each carrying an
/// empty `ns` — both automatic here, since a nested view's own
/// `is_nested`/`is_namespaced` checks and its statements/nested-of-its-
/// own are all computed independently by its own `extract_function`
/// call. `doc` is [`reduce_doc_run`] over the bound `DOC_RUN`'s own
/// retokenized items (`None` when the function carries no doc run,
/// matching `reduce_doc_run(&[])`'s own empty-run `None`). `ns` stays
/// empty (Task 5's recursion stamps the real path on top-level
/// definitions only, exactly as `lower_items`/`lower_function` do) and
/// `local` stays `false` (flatten computes it).
fn extract_function(view: &FunctionView, index: &TextLineIndex) -> Function {
    let header = view.header();
    let name = header.name.text().to_string();
    let name_span = index.span(header.name.text_range());
    let (line, col) = index.line_col(header.name.text_range().start);

    let nested_fn = is_nested(view.syntax());
    let exported =
        !nested_fn && (header.has_export || (!is_namespaced(view.syntax()) && name == "main"));
    let volatile = !nested_fn && header.has_volatile;

    let body = view
        .statements()
        .map(|s| extract_statement(&s, index))
        .collect();
    let nested = view.nested().map(|n| extract_function(&n, index)).collect();
    let doc = view.doc_run().and_then(|dr| {
        let tokens = sig_tokens(dr.syntax(), index);
        reduce_doc_run(&reparse_doc_items(&tokens))
    });

    Function {
        name,
        line,
        col,
        name_span,
        body,
        exported,
        volatile,
        local: false,
        nested,
        ns: Vec::new(),
        doc,
    }
}

/// One `use a::b as c` path — mirrors the parser's own `UsePath`
/// construction (parser.rs's `use` production): `path` is the segment
/// texts in order, `alias` the trailing `as`-bound name if any, `line`
/// the FIRST segment's line, `span` FIRST segment start → LAST segment
/// end — alias-exclusive, matching [`Import`]'s own doc and
/// `UsePath`'s C1 span convention. `ns` is the caller's accumulated
/// namespace path, copied straight through (an import's own path never
/// contributes to it — only `NAMESPACE` blocks do).
fn extract_import(view: &UsePathView, ns: &[String], index: &TextLineIndex) -> Import {
    let segments = view.segments();
    let first = segments
        .first()
        .expect("USE_PATH always carries at least one segment");
    let last = segments
        .last()
        .expect("USE_PATH always carries at least one segment");
    let path = segments.iter().map(|t| t.text().to_string()).collect();
    let alias = view.alias_token().map(|t| t.text().to_string());
    let line = index.line_col(first.text_range().start).0;
    let span = index.span(TextRange::new(
        first.text_range().start,
        last.text_range().end,
    ));
    Import {
        path,
        alias,
        line,
        ns: ns.to_vec(),
        span,
    }
}

/// Walk one level of `FileView`/`NamespaceView::items` — mirrors
/// `crate::parser::lower_items` (parser.rs:402) exactly: a `USE_DECL`
/// contributes one [`Import`] per path (ns stamped as-is); a
/// `NAMESPACE` recurses with its name pushed onto `ns`; a `FUNCTION` is
/// extracted and stamped with the CURRENT `ns` — top-level only, since a
/// nested definition never reaches this level (`FunctionView::nested`
/// hoists it out, and `extract_function`'s own recursion already leaves
/// a nested `Function`'s `ns` empty). Top-level comments carry no green
/// node at all (trivia, dropped before `FileView`/`NamespaceView::items`
/// ever sees them), so — unlike `lower_items`'s explicit
/// `TopKind::Comment(_) => {}` arm — there is no matching arm to skip
/// here.
fn extract_items(
    items: impl Iterator<Item = TopView>,
    ns: &[String],
    index: &TextLineIndex,
    functions: &mut Vec<Function>,
    imports: &mut Vec<Import>,
) {
    for item in items {
        match item {
            TopView::Use(use_decl) => {
                for path in use_decl.paths() {
                    imports.push(extract_import(&path, ns, index));
                }
            }
            TopView::Namespace(nsv) => {
                let mut child = ns.to_vec();
                child.push(nsv.name());
                extract_items(nsv.items(), &child, index, functions, imports);
            }
            TopView::Function(f) => {
                let mut function = extract_function(&f, index);
                function.ns = ns.to_vec();
                functions.push(function);
            }
        }
    }
}

/// Rebuild the whole C1 `parser::Program` from the green tree's root —
/// mirrors `crate::parser::lower_cst` (parser.rs:395): one
/// [`TextLineIndex`] built once and threaded through the whole walk,
/// then [`extract_items`] over the file's own top-level items with an
/// empty starting `ns`.
pub fn extract_program(root: &SyntaxNode, source: &str) -> Program {
    let index = TextLineIndex::new(source);
    let file = FileView::cast(root.clone()).expect("root is FILE");
    let mut functions = Vec::new();
    let mut imports = Vec::new();
    extract_items(file.items(), &[], &index, &mut functions, &mut imports);
    Program { functions, imports }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::{
        Builtin, DocRunItem, DocRunKind, Item, Successor, parse_green, reduce_doc_run,
    };
    use crate::syntax::{FileView, ItemView, StatementView, TopView};
    use mtc_core::diagnostics::Span;
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

        // Every field below is a literal captured from the C1 lowering
        // of this fixture while that path was still callable.
        let expected_items: Vec<Item> = vec![
            Item::Builtin {
                which: Builtin::Right,
                succ: Successor::Label(2),
                succ_span: Some(Span::new(2, 9, 2, 12)),
                succ_label_span: Some(Span::new(2, 10, 2, 11)),
                succ_label_written: Some("2".to_string()),
                line: 2,
            },
            Item::Goto {
                label: 1,
                label_span: Span::new(3, 6, 3, 7),
                label_written: "1".to_string(),
                line: 3,
            },
            Item::Builtin {
                which: Builtin::Right,
                succ: Successor::FallThrough,
                succ_span: None,
                succ_label_span: None,
                succ_label_written: None,
                line: 4,
            },
            // The group's SECOND entry — reparsed with `in_group: true`,
            // which is the path neither statement above reaches.
            Item::Builtin {
                which: Builtin::Mark,
                succ: Successor::FallThrough,
                succ_span: None,
                succ_label_span: None,
                succ_label_written: None,
                line: 4,
            },
        ];

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

        assert_eq!(green_items, expected_items);
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

        // A literal captured from the C1 lowering of this fixture while
        // that path was still callable.
        let expected_doc_run = vec![
            DocRunItem {
                blank_before: false,
                kind: DocRunKind::Doc {
                    text: "doc line".to_string(),
                    span: Span::new(1, 1, 1, 11),
                },
            },
            DocRunItem {
                blank_before: false,
                kind: DocRunKind::Attention {
                    attr: None,
                    text: "caution".to_string(),
                    span: Span::new(2, 1, 2, 10),
                },
            },
            DocRunItem {
                blank_before: false,
                kind: DocRunKind::Attention {
                    attr: Some(crate::parser::AttrCst {
                        name: "deprecated".to_string(),
                        span: Span::new(3, 4, 3, 14),
                    }),
                    text: "[deprecated] use goToEnd".to_string(),
                    span: Span::new(3, 1, 3, 27),
                },
            },
        ];

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

        assert_eq!(green_doc_run, expected_doc_run);
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

        // Three items as WRITTEN — two `?` lines and the comment between
        // them — folding to one paragraph. Both literals were captured
        // from the C1 lowering of this fixture while that path was still
        // callable.
        let written_items = 3;
        let expected_doc = Some(crate::parser::FnDoc {
            paragraphs: vec!["doc line more doc".to_string()],
            attention: Vec::new(),
            deprecated: None,
        });

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
        assert!(
            green_doc_run.len() < written_items,
            "the comment must actually be dropped, or this test proves nothing"
        );
        let green_doc = reduce_doc_run(&green_doc_run);

        assert_eq!(green_doc, expected_doc);
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

    /// Extraction rebuilds every feature a function declaration can
    /// carry, on a snippet holding all of them: a bound doc run (a `?`
    /// paragraph plus a bare `!` attention line), `export` on the
    /// un-namespaced top-level `main` (already auto-exported, so this
    /// also pins that a written `export` and the auto-export fold-in
    /// agree rather than double-applying), a labeled statement, and a
    /// nested function definition.
    ///
    /// Asserted field by field against literals captured from the C1
    /// lowering of this fixture while that path was still callable.
    #[test]
    fn extracted_function_carries_every_declaration_feature() {
        let src = "? doc line\n! caution\nexport main() {\n1: right;\nh() { right; }\n}\n";

        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let index = TextLineIndex::new(src);
        let file = FileView::cast(root).expect("root is FILE");
        let TopView::Function(f) = file.items().next().expect("one item") else {
            panic!("expected a function");
        };

        let extracted = extract_function(&f, &index);

        assert_eq!(
            (
                extracted.name.as_str(),
                extracted.line,
                extracted.col,
                extracted.name_span
            ),
            ("main", 3, 8, Span::new(3, 8, 3, 12))
        );
        assert!(
            extracted.exported && !extracted.local,
            "a written `export` on `main` folds in once, not twice"
        );
        assert!(!extracted.volatile);
        assert!(extracted.ns.is_empty());
        assert_eq!(
            extracted.doc,
            Some(crate::parser::FnDoc {
                paragraphs: vec!["doc line".to_string()],
                attention: vec!["caution".to_string()],
                deprecated: None,
            })
        );

        assert_eq!(extracted.body.len(), 1);
        let statement = &extracted.body[0];
        assert_eq!(
            statement
                .labels
                .iter()
                .map(|l| (l.value, l.written.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "1")],
            "the statement's own label survives extraction"
        );
        assert_eq!(
            (statement.line, statement.span),
            (4, Span::new(4, 1, 4, 10))
        );

        assert_eq!(extracted.nested.len(), 1);
        let nested = &extracted.nested[0];
        assert_eq!((nested.name.as_str(), nested.line), ("h", 5));
        assert!(
            !nested.exported,
            "a nested definition is never auto-exported"
        );
        assert!(nested.ns.is_empty(), "nesting is not a namespace");
    }

    /// A namespaced, aliased snippet, asserted against literals captured
    /// from the C1 lowering while that path was still callable: the
    /// import's path, alias and (file-level) `ns`, the namespaced
    /// function's own `ns`, and `main`'s auto-export.
    #[test]
    fn extracted_program_stamps_namespaces_aliases_and_the_main_export() {
        let src = "use std::goToEnd as ge;\nnamespace n {\nf() { right; }\n}\nmain() { right; }\n";
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let program = extract_program(&root, src);

        assert_eq!(program.imports.len(), 1);
        let import = &program.imports[0];
        assert_eq!(
            (
                import.path.clone(),
                import.alias.clone(),
                import.ns.clone(),
                import.line,
                import.span
            ),
            (
                vec!["std".to_string(), "goToEnd".to_string()],
                Some("ge".to_string()),
                Vec::new(),
                1,
                Span::new(1, 5, 1, 17)
            )
        );

        assert_eq!(
            program
                .functions
                .iter()
                .map(|f| (f.name.as_str(), f.ns.clone(), f.exported))
                .collect::<Vec<_>>(),
            vec![
                ("f", vec!["n".to_string()], false),
                ("main", Vec::new(), true),
            ],
            "namespace contents come first, and only `main` auto-exports"
        );
    }

    /// Strengthens the test above: its only `use` lives at file level, so
    /// it cannot tell an `Import`'s `ns` stamp apart from an accidentally
    /// dropped one — both read `[]`. This snippet adds a
    /// NAMESPACE-scoped `use` (pinning `Import.ns == ["n"]` against the
    /// file-level one's `[]`) and a function nested inside a namespaced
    /// function (pinning the parent's `ns == ["n"]` against the child's
    /// `[]`, since `lower_function` lowers a nested definition with an
    /// empty namespace of its own).
    ///
    /// Asserted against literals captured from the C1 lowering while that
    /// path was still callable.
    #[test]
    fn extracted_program_pins_namespace_scoped_import_and_nested_function_ns() {
        let src = "use std::goToEnd as ge;\nnamespace n {\nuse std::goToStart as gs;\nf() {\nright;\ng() { left; }\n}\n}\nmain() { right; }\n";
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let extracted = extract_program(&root, src);

        // Whole-value, not just `binding()` and `ns`: nothing in the
        // crate's downstream crossfire was ever measured against PM's
        // `extract_import`, so `path`, `alias`, `line` and `span` have no
        // demonstrated cover elsewhere and are asserted here.
        assert_eq!(
            extracted
                .imports
                .iter()
                .map(|i| {
                    (
                        i.path.clone(),
                        i.alias.clone(),
                        i.ns.clone(),
                        i.line,
                        i.span,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    vec!["std".to_string(), "goToEnd".to_string()],
                    Some("ge".to_string()),
                    Vec::new(),
                    1,
                    Span::new(1, 5, 1, 17),
                ),
                (
                    vec!["std".to_string(), "goToStart".to_string()],
                    Some("gs".to_string()),
                    vec!["n".to_string()],
                    3,
                    Span::new(3, 5, 3, 19),
                ),
            ],
            "a file-level import stamps no namespace; a scoped one stamps its own"
        );

        let f = extracted
            .functions
            .iter()
            .find(|f| f.name == "f")
            .expect("namespaced function present");
        assert_eq!(f.ns, vec!["n".to_string()]);
        assert_eq!(f.nested.len(), 1);
        assert_eq!(f.nested[0].name, "g");
        assert_eq!(
            f.nested[0].ns,
            Vec::<String>::new(),
            "a nested definition carries no namespace of its own"
        );
    }
}
