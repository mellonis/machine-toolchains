//! Extraction: rebuilding [`crate::parser::Program`] straight
//! from typed views over the `.tmc` green tree (docs/core.md (syntax
//! trees)). Two halves:
//!
//! - **Retokenization** (`sig_tokens`/`token_from_syntax`): turning a
//!   green subtree's own tokens back into real lexer [`Token`]s, so
//!   extraction can hand them to the parser's OWN productions
//!   (`crate::parser`'s `reparse_*` shims) instead of re-deriving their
//!   grammar decisions from the tree shape.
//! - **Assembly** ([`extract_program`]): walking the views themselves —
//!   items, headers, worlds, rules — and mirroring the parser's own
//!   grammar decisions exactly, never re-deriving a rule the parser
//!   already encodes.
//!
//! # How the shims are pinned
//!
//! The four shims the retokenization bridge shipped with each carry a
//! direct fidelity test in this module (`reparsed_transition_…`,
//! `reparsed_binding_arg_…`, `reparsed_sym_map_…`,
//! `reparsed_sig_param_…`), each asserting a written-out expected value
//! rather than comparing two computations against each other. The five
//! added for assembly — `reparse_alphabet_elems`, `reparse_pattern`,
//! `reparse_write_vec`, `reparse_move_vec`, `reparse_qual_name` —
//! deliberately have none of their own, and are pinned TRANSITIVELY by
//! the crate's own consumers, since extraction is the front end
//! everything downstream reads. Measured, one shim at a time, by
//! corrupting its returned value and counting what turns red:
//!
//! - `reparse_alphabet_elems` — reversing its element list reds 51
//!   tests, across `codegen`, `compiler`, `expand`, `fmt`, `footprint`,
//!   `ir`, `lint` and `lsp`.
//! - `reparse_pattern` — reversing its cells reds 11, `codegen`,
//!   `expand`, `fmt`, `lint`, `optimizer::dead_rows` and `parser`.
//! - `reparse_write_vec` — reversing its cells reds 10; `reparse_move_vec`
//!   reds 3 (`codegen`, `fmt`, `lint::rules::unused_tape`), the narrowest
//!   of the family and still real.
//! - `reparse_qual_name` — dropping a segment reds 111, the widest.
//!
//! What no consumer reads is a `Transition`'s own SPAN — a diagnostic
//! points at a declaration or a pattern, never at a bare transition — so
//! that one dimension has a test of its own here,
//! `transition_spans_are_pinned_by_value`.
//!
//! # Anchor every position on a TOKEN, never on the node
//!
//! A declaration retro-wraps its bound doc run (this crate's `syntax`
//! module doc), so a documented declaration's node STARTS at the doc
//! run, not at its header. Every `line`/`col`/`span.start` a `Program`
//! carries is anchored on a token instead — the name token,
//! the `entry`/`export`/`volatile` prefix, or the declaring keyword —
//! so extraction reads [`header_token`] and the views' own name
//! accessors rather than `SyntaxNode::text_range().start`. A node's
//! `.end` IS safe: `super::GreenSink::finish` closes a node without
//! flushing trailing trivia, so a node ends exactly at its own last
//! significant token.

use mtc_core::diagnostics::Span;
use mtc_core::syntax::{
    AstNode, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, TextLineIndex, TextRange,
};

use super::kinds::TmcKind;
use super::views::{
    AlphabetView, BindView, DocRunView, GraftView, MachineView, ReuseKind, ReuseView, RootView,
    RuleView, StateView, TapeView, TopView, UsePathView, WorldView,
};
use crate::lexer::{Comment, CommentKind, GLYPH_ESCAPES, Token, TokenKind, normalize_doc_payload};
use crate::parser::{
    Alphabet, Bind, Doc, Graft, Graph, Ident, Import, Machine, Program, Routine, Rule, Signature,
    State, TapeDecl, Transition, reduce_doc_run, reparse_alphabet_elems, reparse_binding_arg,
    reparse_doc_items, reparse_move_vec, reparse_pattern, reparse_qual_name, reparse_sig_param,
    reparse_transition, reparse_write_vec,
};
use crate::parser::{DocRunItem, DocRunKind};

/// The three trivia kinds `sig_tokens` filters out before mapping —
/// whitespace and both comment kinds. Every significant token kind, and
/// every node kind, is `false`.
fn is_trivia(kind: SyntaxKind) -> bool {
    kind == TmcKind::Whitespace.into()
        || kind == TmcKind::LineComment.into()
        || kind == TmcKind::BlockComment.into()
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

/// Decodes an already-lexed `GLYPH` token's raw source text — quotes and
/// any escape backslashes included, `'\''` and all — into the same
/// resolved payload `crate::lexer::TokenKind::Glyph` carries.
///
/// A fresh decoder, not a reuse of the lexer's own scan: `lex_with`'s
/// glyph arm fuses terminator detection, `src_len` accounting and
/// column-addressed "invalid escape"/"unterminated" error reporting into
/// one live-[`crate::lexer`]-internal `Cursor` loop with no separable
/// decode-only step, so there is no callable unit to widen visibility
/// on here — restructuring that loop to share one would mean rebuilding
/// the function the lexer's whole proptest battery is pinned against.
/// Sound only because `raw` is a token that already lexed successfully
/// once, so the only escapes it can ever contain are
/// [`crate::lexer::GLYPH_ESCAPES`] — the ONE definition both this loop
/// and `lex_with`'s own scan read, so a third escape added there can
/// never leave this loop silently stale. This duplication is proven
/// equivalent to the lexer's own decoding, not merely careful, by
/// `decode_glyph_body_matches_the_lexer_for_any_legal_content` below.
///
/// TOTAL where the lexer is PARTIAL: given a body carrying a backslash
/// followed by anything outside `GLYPH_ESCAPES` (`'\x'`), this loop
/// still returns a value (`"x"`) — `lex_with` would have rejected that
/// same text as an "invalid escape" lex error before a token, let alone
/// a green tree, ever existed. No input reachable from a parsed tree
/// can ever exercise that divergence (a GLYPH token in a real tree was
/// necessarily accepted by `lex_with` first), so it is not a bug — but
/// it IS the precise scope of the proptest's own equivalence guarantee
/// below: over the LEGAL glyph language only, never over arbitrary text.
fn decode_glyph_body(raw: &str) -> String {
    let body = &raw[1..raw.len() - 1]; // strip the opening/closing `'` (1 ASCII byte each)
    let mut value = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(escaped) = chars.next() {
                debug_assert!(
                    GLYPH_ESCAPES.contains(&escaped),
                    "already-lexed glyph carries only the lexer's own legal escapes"
                );
                value.push(escaped); // both legal escapes resolve to themselves
            }
        } else {
            value.push(c);
        }
    }
    value
}

/// Every non-trivia descendant token of `node`, rebuilt as a real
/// [`Token`] — the retokenization half of the bridge. `node` is a green
/// subtree already known to have parsed once (the tree only exists
/// because it did); this turns it back into the same token shape the
/// parser's productions accept. Ends with a synthetic `Eof` at `node`'s
/// own end position, matching every real token stream's own convention.
pub(crate) fn sig_tokens(node: &SyntaxNode, index: &TextLineIndex) -> Vec<Token> {
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

/// One green token → one [`Token`]: kind mapped 1:1 from [`TmcKind`],
/// position from `index`, length in chars of the token's own verbatim
/// source text (matching every lexer-built [`Token::len`]'s own
/// convention uniformly — a glyph's quotes/backslashes and a doc/
/// attention line's sigil count exactly as written, the same span the
/// green tree already stores). THREE kinds rebuild a STRING payload
/// that is NOT their verbatim text: a green `GLYPH` token carries the
/// raw source text, escapes unresolved (`decode_glyph_body` resolves
/// them); a `DOC_LINE`/`ATTENTION_LINE` token carries the raw line,
/// sigil included and payload un-normalized (`crate::lexer`'s own
/// [`normalize_doc_payload`] rebuilds the exact normalized payload a
/// real lexer token would carry). `NUMBER` is NOT a fourth: its text
/// already IS its written digits, so [`TokenKind::Number`]'s second
/// field is literally the same string its first field parses from —
/// no rebuilding, only an added `u32` alongside the verbatim text.
///
/// No wildcard-free exhaustiveness guard here, unlike
/// `super::kinds::token_kind`'s own `TokenKind -> TmcKind` match: this
/// match is over `TmcKind`/`SyntaxKind` values via `k if k == ...`
/// guards, which is not exhaustiveness-checked by the compiler at all
/// — a new `TmcKind` variant added without an arm here builds clean
/// and falls through to the trailing `unreachable!()`, not a compile
/// error. Only `token_kind`'s own match (over the `TokenKind` enum
/// itself, no wildcard) fails to build on an unhandled lexer variant.
pub(crate) fn token_from_syntax(t: &SyntaxToken, index: &TextLineIndex) -> Token {
    let (line, col) = index.line_col(t.text_range().start);
    let text = t.text();
    let kind = match t.kind() {
        k if k == TmcKind::Ident.into() => TokenKind::Ident(text.to_string()),
        k if k == TmcKind::Number.into() => {
            TokenKind::Number(text.parse().expect("lexed digits"), text.to_string())
        }
        k if k == TmcKind::Glyph.into() => TokenKind::Glyph(decode_glyph_body(text)),
        k if k == TmcKind::DotDot.into() => TokenKind::DotDot,
        k if k == TmcKind::Arrow.into() => TokenKind::Arrow,
        k if k == TmcKind::FatArrow.into() => TokenKind::FatArrow,
        k if k == TmcKind::ColonColon.into() => TokenKind::ColonColon,
        k if k == TmcKind::Dot.into() => TokenKind::Dot,
        k if k == TmcKind::Dash.into() => TokenKind::Dash,
        k if k == TmcKind::Plus.into() => TokenKind::Plus,
        k if k == TmcKind::Eq.into() => TokenKind::Eq,
        k if k == TmcKind::Star.into() => TokenKind::Star,
        k if k == TmcKind::Percent.into() => TokenKind::Percent,
        k if k == TmcKind::Lt.into() => TokenKind::Lt,
        k if k == TmcKind::Gt.into() => TokenKind::Gt,
        k if k == TmcKind::LBracket.into() => TokenKind::LBracket,
        k if k == TmcKind::RBracket.into() => TokenKind::RBracket,
        k if k == TmcKind::LBrace.into() => TokenKind::LBrace,
        k if k == TmcKind::RBrace.into() => TokenKind::RBrace,
        k if k == TmcKind::LParen.into() => TokenKind::LParen,
        k if k == TmcKind::RParen.into() => TokenKind::RParen,
        k if k == TmcKind::Comma.into() => TokenKind::Comma,
        k if k == TmcKind::Semi.into() => TokenKind::Semi,
        k if k == TmcKind::Colon.into() => TokenKind::Colon,
        // Both arms below have no reachable input from a parsed tree:
        // `@` is lexed but consumed by no grammar production
        // (`grep -c 'TokenKind::At\b' src/parser.rs`, `describe()`
        // excluded, is 0), and a bare `!` is only ever an
        // AttentionLine at line-start or a lex/parse error elsewhere —
        // kept for total coverage of the `TmcKind` token space, not
        // because either is exercised.
        k if k == TmcKind::At.into() => TokenKind::At,
        k if k == TmcKind::Bang.into() => TokenKind::Bang,
        k if k == TmcKind::DocLine.into() => {
            TokenKind::DocLine(normalize_doc_payload(&text[sigil_len(text)..]))
        }
        k if k == TmcKind::AttentionLine.into() => {
            TokenKind::AttentionLine(normalize_doc_payload(&text[sigil_len(text)..]))
        }
        // Trivia is filtered out by `sig_tokens` before this is ever
        // called; every node kind never reaches here —
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

// ---------------------------------------------------------------------------
// Assembly — views → `crate::parser::Program`
// ---------------------------------------------------------------------------

/// A node's own direct child tokens, trivia excluded — never a
/// descendant's. The header/keyword scans below all want this: a
/// descendant walk would reach down into a nested SIG_PARAM,
/// BINDING_ARG, WRITE_VEC or TRANSITION and read a token that belongs
/// to it — the RULE dump in [`extract_rule`] below shows exactly that
/// nesting.
fn direct_tokens(node: &SyntaxNode) -> impl Iterator<Item = SyntaxToken> + '_ {
    node.children_with_tokens().filter_map(|e| match e {
        SyntaxElement::Token(t) if !is_trivia(t.kind()) => Some(t),
        _ => None,
    })
}

/// The declaration's own first significant token — the anchor every
/// `line`/`col`/`span.start` in an extracted `Program` is taken from,
/// and the reason the module doc says never to read a node's own start.
///
/// The shape this walk relies on, for `"? doc\nalphabet ab { '0' }\n"`:
///
/// ```text
/// ROOT@0..26
///   ALPHABET@0..25
///     DOC_RUN@0..5
///       DOC_LINE@0..5 "? doc"
///     WHITESPACE@5..6 "\n"
///     IDENT@6..14 "alphabet"
///     WHITESPACE@14..15 " "
///     IDENT@15..17 "ab"
///     …
/// ```
///
/// ALPHABET starts at 0, the doc run — not at 6, its header. The run is
/// a child NODE, so filtering to direct TOKENS steps over the whole
/// thing, and the newline between it and the header is trivia this
/// filter drops. What is left first is the header: `export`/`entry`/
/// `volatile` when written, else the declaring keyword — exactly the
/// `header_start` / `prefix` / `machine_tok` / `graft_tok` /
/// `bind_tok` / `lead_tok` position the matching `Parser::parse_*`
/// production records.
fn header_token(node: &SyntaxNode) -> SyntaxToken {
    direct_tokens(node)
        .next()
        .expect("every extracted declaration node carries at least one significant token")
}

/// A token slice → real [`Token`]s, for the two shims fed an
/// unbracketed run rather than a whole node (`reparse_pattern`,
/// `reparse_qual_name`). Ends with the same synthetic `Eof`
/// [`sig_tokens`] appends, here at the last token's own end.
fn tokens_from(toks: &[SyntaxToken], index: &TextLineIndex) -> Vec<Token> {
    let mut out: Vec<Token> = toks.iter().map(|t| token_from_syntax(t, index)).collect();
    let end = toks
        .last()
        .expect("a retokenized run is never empty — its caller found it by its own first token")
        .text_range()
        .end;
    let (line, col) = index.line_col(end);
    out.push(Token {
        kind: TokenKind::Eof,
        line,
        col,
        len: 0,
    });
    out
}

/// The end line of whatever precedes `node` in the source — the
/// `prev_end_line` seed [`reparse_doc_items`] needs for its first
/// item's `blank_before`, and information a doc run's own tokens can
/// never carry.
///
/// Read off the source rather than the tree. The value to reproduce is
/// where the retired parser's own `prev_end_line` stood at the moment
/// it started a doc run: the end line of the last non-whitespace SOURCE
/// CONTENT before it, comments included. Three things set it there —
/// `capture_open_trailing` and `capture_close_trailing` each advanced
/// it past a captured comment's own last line; all four of
/// `drain_pending`'s CALLERS did the same for a drained own-line
/// comment, from the count it handed back — `top_items`, `world_body`
/// and `state_rules` wrote the field directly, `doc_run` through a
/// local it copied back on the way out, while `drain_pending` itself
/// never touched it; and the preceding declaration's own production set
/// it to its `;` or `}` line.
/// Scanning back to the last non-whitespace character answers all of
/// them with one rule, where the tree would need the preceding sibling
/// AND its trailing trivia treated separately — so that scan stays the
/// default answer here.
///
/// It answers all of them but ONE, and that one is why the `;` arm
/// below exists. `Parser::take_trailing` was the single
/// comment-capturing helper that did NOT advance `prev_end_line` past
/// the comment it claimed — it fired only right after a `;` (the five
/// `;`-terminated productions: `use`, `tape`, `graft`, `bind`, and a
/// rule), where `capture_close_trailing`, the `}` twin, DID advance.
/// So when a `;`'s trailing comment is the last thing before the run,
/// the in-context parse kept the `;`'s line where the scan-back would
/// report the comment's end line — a difference only a MULTI-LINE
/// comment can show, since a single-line one ends where it starts.
///
/// The arm is narrow in both directions, and both edges are pinned by
/// `the_semicolon_arm_is_narrow_in_both_directions`:
///
/// - It is keyed on `;` because `}` behaved the other way, and every
///   other predecessor (a `{` through `capture_open_trailing`, an
///   own-line comment through `drain_pending`) advanced the field too.
/// - `take_trailing` claimed AT MOST ONE comment, so the arm requires
///   the comment run to be exactly one long. With a second comment
///   written after the `;`, the first was the trailing and the rest
///   drained as ordinary pending comments, each advancing the field —
///   which lands the value back on the scan-back's own answer.
///
/// (A list's own interior drain left the field alone too, but was never
/// the last word: a list's closing delimiter always follows it, and the
/// declaration's own production then set the field.)
fn prev_end_line(source: &str, index: &TextLineIndex, node: &SyntaxNode) -> u32 {
    let scan_back = |start: usize| match source[..start].rfind(|c: char| !c.is_whitespace()) {
        Some(i) => index.line_col(i as u32).0,
        None => 0,
    };
    let start = node.text_range().start as usize;
    let (significant, comments) = preceding_trivia(node);
    let semi_line = match &significant {
        Some(sig) if sig.kind() == TmcKind::Semi.into() => {
            Some(index.line_col(sig.text_range().start).0)
        }
        _ => None,
    };
    if let (Some(line), [only]) = (semi_line, comments.as_slice())
        && index.line_col(only.text_range().start).0 == line
    {
        return line;
    }
    scan_back(start)
}

/// Walking backward out of `node` to the last significant token before
/// it, collecting the comment tokens written in between (document
/// order). [`prev_end_line`]'s `;` arm needs both halves, and neither
/// is a sibling lookup: a bound `DOC_RUN` is its declaration's FIRST
/// child, so everything before it lives in an ancestor's stream.
///
/// The cursor never descends. A preceding sibling NODE contributes only
/// its `last_token`, which this module's own doc explains is always
/// significant (`GreenSink::finish` closes a node without flushing
/// trailing trivia) — so the walk stops there, and there is nothing
/// deeper it could still need to see.
fn preceding_trivia(node: &SyntaxNode) -> (Option<SyntaxToken>, Vec<SyntaxToken>) {
    fn step_back(cur: &SyntaxElement) -> Option<SyntaxElement> {
        match cur {
            SyntaxElement::Node(n) => n.prev_sibling_or_token(),
            SyntaxElement::Token(t) => t.prev_sibling_or_token(),
        }
    }
    fn climb(cur: &SyntaxElement) -> Option<SyntaxNode> {
        match cur {
            SyntaxElement::Node(n) => n.parent(),
            SyntaxElement::Token(t) => Some(t.parent()),
        }
    }

    let mut cur = SyntaxElement::Node(node.clone());
    let mut comments: Vec<SyntaxToken> = Vec::new();
    let mut significant = None;
    loop {
        let Some(prev) = step_back(&cur) else {
            match climb(&cur) {
                Some(parent) => {
                    cur = SyntaxElement::Node(parent);
                    continue;
                }
                None => break,
            }
        };
        match &prev {
            SyntaxElement::Token(t) if t.kind() == TmcKind::Whitespace.into() => {}
            // Whitespace is already gone, so the rest of the trivia
            // space is exactly the two comment kinds.
            SyntaxElement::Token(t) if is_trivia(t.kind()) => comments.push(t.clone()),
            SyntaxElement::Token(t) => {
                significant = Some(t.clone());
                break;
            }
            SyntaxElement::Node(n) => {
                if let Some(last) = n.last_token() {
                    significant = Some(last);
                    break;
                }
            }
        }
        cur = prev;
    }
    comments.reverse();
    (significant, comments)
}

/// One green comment token → the [`Comment`] the lexer would have built
/// for the same source. Not a second decoder: a comment carries no
/// decoded payload — [`Comment::text`] is documented as the verbatim
/// source text, delimiters included, which is exactly the token's own
/// text, and the kind is the delimiter pair. Only `own_line` is derived,
/// because it is the one field that is contextual rather than a property
/// of the token — which is why [`token_from_syntax`], whose arms map a
/// kind to a payload and nothing else, has no arm for a comment at all
/// and would hit its `unreachable!()` on one.
///
/// Lives here rather than in `crate::fmt`, even though the formatter is
/// its heaviest reader: a doc run's own items carry `Comment` VALUES
/// ([`DocRunItem`]'s `DocRunKind::Comment`), so extraction needs the
/// same conversion, and `syntax` must not depend on `fmt`. Pinned
/// against `lex_with` itself by `crate::fmt::trivia`'s
/// `comment_values_match_the_lexers_own`.
pub(crate) fn comment_from(t: &SyntaxToken) -> Comment {
    Comment {
        text: t.text().to_string(),
        kind: if t.kind() == TmcKind::LineComment.into() {
            CommentKind::Line
        } else {
            CommentKind::Block
        },
        own_line: starts_its_line(t),
    }
}

/// True iff nothing but whitespace precedes `t` on its physical line —
/// the lexer's `Cursor::at_line_start` read through the tree instead of
/// through a second scan of the source.
///
/// A node begins at its own first significant token, so a comment is
/// never a node's first child and its predecessor is always a sibling.
/// Two things make the line start: a whitespace run holding a newline,
/// and the start of the file — which includes a whitespace run that has
/// no newline but IS the file's first token, the leading-indent case a
/// newline test alone would miss.
fn starts_its_line(t: &SyntaxToken) -> bool {
    match t.prev_sibling_or_token() {
        None => true,
        Some(SyntaxElement::Token(p)) if p.kind() == TmcKind::Whitespace.into() => {
            p.text().contains('\n') || p.prev_sibling_or_token().is_none()
        }
        _ => false,
    }
}

/// Every token `Parser::doc_run` folds into one run, in source order:
/// the run node's own `?`/`!` lines and interleaved comments, plus the
/// comments written between the run's last line and the declaration's
/// header.
///
/// The second half is not in the node. `doc_run`'s item loop drains
/// pending comments AFTER each line it consumes, so a comment written
/// below the run's last line — with only the declaring keyword after it
/// — is still one of the run's items; in the tree that comment sits
/// OUTSIDE the DOC_RUN node, as a direct sibling token of the
/// declaration's own stream, because the run node closes on its last
/// `?`/`!` token. Measured, for `"? doc\n/* c */\nalphabet b { '0' }\n"`:
///
/// ```text
/// ALPHABET@0..32
///   DOC_RUN@0..5
///     DOC_LINE@0..5 "? doc"
///   WHITESPACE@5..6 "\n"
///   BLOCK_COMMENT@6..13 "/* c */"     <- an item of the RUN, not of ALPHABET
///   WHITESPACE@13..14 "\n"
///   IDENT@14..22 "alphabet"
/// ```
///
/// The forward walk stops at the first element that is not trivia, which
/// is always that header token: a declaration's own first significant
/// token follows its bound run with nothing but trivia in between.
fn doc_run_tokens(view: &DocRunView) -> Vec<SyntaxToken> {
    let mut out: Vec<SyntaxToken> = view
        .syntax()
        .descendant_tokens()
        .filter(|t| t.kind() != TmcKind::Whitespace.into())
        .collect();
    let mut cur = view.syntax().next_sibling_or_token();
    while let Some(SyntaxElement::Token(t)) = cur {
        if t.kind() == TmcKind::Whitespace.into() {
            cur = t.next_sibling_or_token();
        } else if is_trivia(t.kind()) {
            out.push(t.clone());
            cur = t.next_sibling_or_token();
        } else {
            break;
        }
    }
    out
}

/// A bound doc run's raw items, reparsed through the parser's own
/// `doc_run` production. Separate from [`extract_doc`] because the raw
/// items are the only place [`prev_end_line`] — and every comment item —
/// is observable: the reduction below drops `blank_before` entirely and
/// treats a comment item as inert.
///
/// # Why the run is reparsed in segments
///
/// [`reparse_doc_items`] runs `Parser::doc_run` over a comment-free
/// token stream, which is what [`sig_tokens`] produces — so a run alone
/// cannot see its own comments, and `doc_run`'s comment arm never fires.
/// Both halves of that matter to `fmt`, which prints these items
/// verbatim (docs/tmt/fmt.md (comments)): the comment is DROPPED, and
/// the item after it then measures its gap against the previous `?`/`!`
/// LINE rather than against the comment, inventing a blank line nobody
/// wrote. Measured, for `"? doc\n// c\n? more\nalphabet b { '0' }\n"`:
/// three items and no blanks become two items with `? more` blank-
/// separated.
///
/// So the run is split at its comments and each `?`/`!` segment goes
/// through the production as before, with `prev_end_line` threaded
/// across the joins exactly as `doc_run`'s own loop threads its local.
/// The comment arm below is the one piece of that loop reproduced here,
/// because there is no comment-carrying entry point into the production
/// to hand these tokens to. Re-running the production per segment loses
/// no check that matters: `DocLineOrder`, the duplicate- and the
/// unknown-attribute checks all passed once already, over these exact
/// tokens, during the parse that produced the tree.
pub(crate) fn extract_doc_items(
    view: &DocRunView,
    source: &str,
    index: &TextLineIndex,
) -> Vec<DocRunItem> {
    /// One pending `?`/`!` segment through the production, leaving
    /// `prev` on its last line — where `doc_run` leaves its own local
    /// after the matching iteration.
    fn flush_lines(
        lines: &mut Vec<SyntaxToken>,
        prev: &mut u32,
        out: &mut Vec<DocRunItem>,
        index: &TextLineIndex,
    ) {
        let Some(last) = lines.last() else { return };
        let last_line = index.line_col(last.text_range().start).0;
        out.extend(reparse_doc_items(&tokens_from(lines, index), *prev));
        *prev = last_line;
        lines.clear();
    }

    let mut prev = prev_end_line(source, index, view.syntax());
    let mut out: Vec<DocRunItem> = Vec::new();
    let mut lines: Vec<SyntaxToken> = Vec::new();
    for t in doc_run_tokens(view) {
        if !is_trivia(t.kind()) {
            lines.push(t);
            continue;
        }
        flush_lines(&mut lines, &mut prev, &mut out, index);
        let line = index.line_col(t.text_range().start).0;
        let comment = comment_from(&t);
        // `Parser::doc_run`'s own comment arm: a multi-line block
        // comment leaves the near edge on the line it ENDS on.
        let blank_before = line > prev + 1;
        prev = line + comment.text.matches('\n').count() as u32;
        out.push(DocRunItem {
            blank_before,
            kind: DocRunKind::Comment(comment),
        });
    }
    flush_lines(&mut lines, &mut prev, &mut out, index);
    out
}

/// A declaration's reduced [`Doc`] — `None` exactly when no run was
/// written, matching `reduce_doc_run(&[])`'s own empty-run answer. A
/// DOC_RUN node that EXISTS always reduces to `Some`: `Parser::doc_run`
/// opens one only on a `?`/`!` line, and both are significant tokens
/// [`sig_tokens`] keeps.
fn extract_doc(run: Option<DocRunView>, source: &str, index: &TextLineIndex) -> Option<Doc> {
    run.and_then(|dr| reduce_doc_run(&extract_doc_items(&dr, source, index)))
}

/// One `use a::b as c` path — mirrors `Parser::parse_use`'s own
/// `UsePath` construction: `path` is the segment texts in order,
/// `alias` the trailing `as`-bound name if any, `line` the FIRST
/// segment's line, `span` first-segment start → LAST-segment end, the
/// alias deliberately excluded ([`Import`]'s own doc). `ns` is the
/// caller's accumulated namespace path — an import's own path never
/// contributes to it, only `namespace` blocks do.
pub(crate) fn extract_import(view: &UsePathView, ns: &[String], index: &TextLineIndex) -> Import {
    let segments = view.segments();
    let first = segments
        .first()
        .expect("USE_PATH always carries at least one segment");
    let last = segments
        .last()
        .expect("USE_PATH always carries at least one segment");
    Import {
        path: segments.iter().map(|t| t.text().to_string()).collect(),
        alias: view.alias_token().map(|t| t.text().to_string()),
        line: index.line_col(first.text_range().start).0,
        ns: ns.to_vec(),
        span: index.span(TextRange::new(
            first.text_range().start,
            last.text_range().end,
        )),
    }
}

/// One `export? alphabet NAME { … }` — mirrors
/// `crate::parser::lower_alphabet` over `Parser::parse_alphabet`'s own
/// stamping. Two normalisations worth naming, because neither copies a
/// single token's position: `line` is the NAME's line (not the
/// header's), while `col` is the HEADER's column (`export` when
/// written, else `alphabet`).
pub(crate) fn extract_alphabet(
    view: &AlphabetView,
    ns: &[String],
    source: &str,
    index: &TextLineIndex,
) -> Alphabet {
    let name = view.name_token();
    let header = header_token(view.syntax());
    Alphabet {
        name: name.text().to_string(),
        name_span: index.span(name.text_range()),
        line: index.line_col(name.text_range().start).0,
        col: index.line_col(header.text_range().start).1,
        exported: view.exported(),
        ns: ns.to_vec(),
        elems: reparse_alphabet_elems(&sig_tokens(view.syntax(), index)),
        doc: extract_doc(view.doc_run(), source, index),
    }
}

/// One `volatile? tape NAME: ALPHABET;` — the one declaration here
/// whose node start IS its header, since `tape` accepts no doc run:
/// `Parser::next_is_world_doc_accepting` excludes it, so a run written
/// before one is rejected outright (measured — parsing
/// `machine {\n  ? doc\n  tape main: ab;\n}\n` fails with
/// `DanglingDocRun`), never retro-wrapped as a child. `line` and
/// `span.start` still read [`header_token`] rather than the node, so
/// the rule holds uniformly instead of by exception.
fn extract_tape(view: &TapeView, index: &TextLineIndex) -> TapeDecl {
    let name = view.name_token();
    let alphabet = view.alphabet_token();
    let header = header_token(view.syntax());
    TapeDecl {
        name: name.text().to_string(),
        name_span: index.span(name.text_range()),
        alphabet: alphabet.text().to_string(),
        alphabet_span: index.span(alphabet.text_range()),
        volatile: view.volatile(),
        line: index.line_col(header.text_range().start).0,
        span: index.span(TextRange::new(
            header.text_range().start,
            view.syntax().text_range().end,
        )),
    }
}

/// One `pattern -> action;` rule. Four of the five action pieces come
/// straight back from the parser's own productions; the fifth,
/// `debugger`, is a bare keyword with no node of its own.
///
/// Reading it as "a direct IDENT child spelling `debugger`" is exact,
/// not a heuristic. A rule carrying every IDENT-bearing piece at once,
/// `['0' as v] -> debugger write [{v}] move [>] call sub(t = t) then
/// stop;`, dumps its own level as:
///
/// ```text
/// RULE@46..116
///   L_BRACKET@46..47 "["
///   GLYPH@47..50 "'0'"
///   IDENT@51..53 "as"
///   IDENT@54..55 "v"
///   R_BRACKET@55..56 "]"
///   ARROW@57..59 "->"
///   IDENT@60..68 "debugger"
///   IDENT@69..74 "write"
///   WRITE_VEC@75..80 …
///   IDENT@81..85 "move"
///   MOVE_VEC@86..89 …
///   TRANSITION@90..115 …
///   SEMI@115..116 ";"
/// ```
/// (whitespace elided.)
///
/// So a RULE's direct IDENT children are exactly: the pattern's `as`
/// markers, the NAMES those markers bind, the `write`/`move` keywords,
/// and `debugger` itself. Every token of a transition — `call`, the
/// target, `then`, the continuation — sits inside TRANSITION, a level
/// down. The one candidate for a false positive is therefore a binding
/// name, and it cannot be one: `debugger` is one of the 27
/// fully-reserved words (`crate::lexer::RESERVED`), so `Parser::name`
/// refuses it wherever a name is expected, `as NAME` included.
///
/// `Transition::Stay` is synthesised here and only here: it is the
/// ABSENCE of a TRANSITION node, so `reparse_transition` is
/// structurally uncallable for it (docs/tmt/language.md
/// (transitions)). Its span is the rule's own `;` — `Parser::rule`
/// builds it as `self.peek().span()` at the point the semicolon is the
/// upcoming token — which is a RULE node's last token, since the node
/// closes right after that semicolon is bumped.
pub(crate) fn extract_rule(view: &RuleView, index: &TextLineIndex) -> Rule {
    let node = view.syntax();
    let pattern = reparse_pattern(&tokens_from(&view.pattern_tokens(), index));
    let debugger =
        direct_tokens(node).any(|t| t.kind() == TmcKind::Ident.into() && t.text() == "debugger");
    let transition = match view.transition() {
        Some(t) => reparse_transition(&sig_tokens(t.syntax(), index)),
        None => {
            let semi = node
                .last_token()
                .expect("RULE always carries at least its own `;`");
            debug_assert_eq!(
                semi.kind(),
                TmcKind::Semi.into(),
                "RULE closes right after its own `;`"
            );
            Transition::Stay {
                span: index.span(semi.text_range()),
            }
        }
    };
    Rule {
        line: pattern.span.start.line,
        pattern,
        debugger,
        write: view
            .write_vec()
            .map(|w| reparse_write_vec(&sig_tokens(w.syntax(), index))),
        mov: view
            .move_vec()
            .map(|m| reparse_move_vec(&sig_tokens(m.syntax(), index))),
        transition,
        span: index.span(node.text_range()),
    }
}

/// One `entry? state NAME { rules }` — `line` is the NAME's line and
/// `col` the header's column, the same split [`extract_alphabet`]
/// carries; `span` runs from the header (`entry` when written) to the
/// body's closing `}`, which is the node's own end.
fn extract_state(view: &StateView, source: &str, index: &TextLineIndex) -> State {
    let name = view.name_token();
    let header = header_token(view.syntax());
    State {
        entry: view.is_entry(),
        name: name.text().to_string(),
        name_span: index.span(name.text_range()),
        line: index.line_col(name.text_range().start).0,
        col: index.line_col(header.text_range().start).1,
        rules: view.rules().map(|r| extract_rule(&r, index)).collect(),
        span: index.span(TextRange::new(
            header.text_range().start,
            view.syntax().text_range().end,
        )),
        doc: extract_doc(view.doc_run(), source, index),
    }
}

/// The `IDENT (:: IDENT)*` target run of a GRAFT or BIND, ready for
/// [`reparse_qual_name`]: every direct token from the target's own
/// first segment onward. `skip` counts the header keywords the view
/// already identified — `entry` (GRAFT only, when written) and the
/// `graft`/`bind` keyword itself.
///
/// Deliberately NOT trimmed at the `(` that follows: `qual_name`
/// advances only while the next token is `::`, so it stops there on its
/// own, and leaving the trailing tokens in means this helper never has
/// to encode where a target ENDS — a question the parser already
/// answers.
fn target_tokens(node: &SyntaxNode, skip: usize, index: &TextLineIndex) -> Vec<Token> {
    let run: Vec<SyntaxToken> = direct_tokens(node).skip(skip).collect();
    tokens_from(&run, index)
}

/// One `entry? graft TARGET(args) [as NAME];`. The one declaration
/// whose `line` and `span.start` come from DIFFERENT tokens:
/// `Parser::parse_graft` records `line` off the `graft` keyword (it
/// reads `self.peek()` after `entry` has already been bumped) but takes
/// `span.start` from the `entry` prefix when there is one.
pub(crate) fn extract_graft(view: &GraftView, source: &str, index: &TextLineIndex) -> Graft {
    let node = view.syntax();
    let entry = view.is_entry();
    let header = header_token(node);
    let graft_kw = direct_tokens(node)
        .nth(usize::from(entry))
        .expect("GRAFT always carries its own `graft` keyword");
    Graft {
        entry,
        target: reparse_qual_name(&target_tokens(node, usize::from(entry) + 1, index)),
        args: view
            .bindings()
            .map(|a| reparse_binding_arg(&sig_tokens(a.syntax(), index)))
            .collect(),
        as_name: view.as_name().map(|t| Ident {
            name: t.text().to_string(),
            span: index.span(t.text_range()),
        }),
        line: index.line_col(graft_kw.text_range().start).0,
        span: index.span(TextRange::new(
            header.text_range().start,
            node.text_range().end,
        )),
        doc: extract_doc(view.doc_run(), source, index),
    }
}

/// One `bind TARGET(args) as NAME;` — [`extract_graft`] without the
/// `entry` prefix, which `bind` never takes (docs/tmt/language.md
/// (entry)), so its header token IS its `bind` keyword.
pub(crate) fn extract_bind(view: &BindView, source: &str, index: &TextLineIndex) -> Bind {
    let node = view.syntax();
    let header = header_token(node);
    let as_name = view.as_name();
    Bind {
        target: reparse_qual_name(&target_tokens(node, 1, index)),
        args: view
            .bindings()
            .map(|a| reparse_binding_arg(&sig_tokens(a.syntax(), index)))
            .collect(),
        as_name: Ident {
            name: as_name.text().to_string(),
            span: index.span(as_name.text_range()),
        },
        line: index.line_col(header.text_range().start).0,
        span: index.span(TextRange::new(
            header.text_range().start,
            node.text_range().end,
        )),
        doc: extract_doc(view.doc_run(), source, index),
    }
}

/// A world body split into its four item kinds — the green-tree
/// counterpart of `crate::parser::lower_world_body`. Each vector keeps
/// document order, because each view accessor walks the WORLD's
/// children in order and keeps only its own kind, which is the same
/// order the CST's single interleaved item list is pushed in.
#[derive(Default)]
struct WorldParts {
    tapes: Vec<TapeDecl>,
    states: Vec<State>,
    grafts: Vec<Graft>,
    binds: Vec<Bind>,
}

/// `None` — a declaration with no WORLD child — yields four empty
/// vectors rather than panicking: `ReuseView::world`'s own doc explains
/// why a view answers absence instead of asserting a shape the parser
/// always produces.
fn extract_world(world: Option<WorldView>, source: &str, index: &TextLineIndex) -> WorldParts {
    let Some(world) = world else {
        return WorldParts::default();
    };
    WorldParts {
        tapes: world.tapes().map(|t| extract_tape(&t, index)).collect(),
        states: world
            .states()
            .map(|s| extract_state(&s, source, index))
            .collect(),
        grafts: world
            .grafts()
            .map(|g| extract_graft(&g, source, index))
            .collect(),
        binds: world
            .binds()
            .map(|b| extract_bind(&b, source, index))
            .collect(),
    }
}

/// Everything a `routine` and a `graph` share, which is every field
/// either one has. `crate::parser::Routine` and
/// `crate::parser::Graph` are distinct types with identical shapes —
/// kept apart because the front end treats the two reuse forms
/// differently — so extraction reads the node once and the caller
/// stamps whichever struct the REUSE's own keyword names.
struct ReuseParts {
    name: String,
    name_span: Span,
    line: u32,
    col: u32,
    exported: bool,
    sig: Signature,
    states: Vec<State>,
    grafts: Vec<Graft>,
    binds: Vec<Bind>,
    doc: Option<Doc>,
}

/// One `export? routine|graph NAME(sig) { … }`. `line`/`col` split the
/// same way [`extract_alphabet`]'s do. The signature's own span is
/// taken from the first and last of `ReuseView::signature`'s tokens,
/// which that accessor's doc pins as the `(` and the matching `)` —
/// there is no node to read it off, and the parameters themselves come
/// back one at a time through `reparse_sig_param`.
///
/// A world body's tapes are dropped, exactly as
/// `crate::parser::lower_routine`/`lower_graph` drop them: parsing
/// rejects a `tape` declaration outside a `machine`, so the vector is
/// always empty here anyway.
fn extract_reuse(view: &ReuseView, source: &str, index: &TextLineIndex) -> ReuseParts {
    let name = view.name_token();
    let header = header_token(view.syntax());
    let sig_tokens_run = view.signature();
    let open = sig_tokens_run
        .first()
        .expect("REUSE's signature run always opens on its own `(`");
    let close = sig_tokens_run
        .last()
        .expect("REUSE's signature run always closes on its own `)`");
    let parts = extract_world(view.world(), source, index);
    ReuseParts {
        name: name.text().to_string(),
        name_span: index.span(name.text_range()),
        line: index.line_col(name.text_range().start).0,
        col: index.line_col(header.text_range().start).1,
        exported: view.exported(),
        sig: Signature {
            params: view
                .params()
                .map(|p| reparse_sig_param(&sig_tokens(p.syntax(), index)))
                .collect(),
            span: index.span(TextRange::new(
                open.text_range().start,
                close.text_range().end,
            )),
        },
        states: parts.states,
        grafts: parts.grafts,
        binds: parts.binds,
        doc: extract_doc(view.doc_run(), source, index),
    }
}

/// The single `machine { … }` block. It carries no name, so `line` and
/// `col` are both the `machine` keyword's, and `span` runs from that
/// keyword — never the node's own start, which a bound doc run moves —
/// to the body's closing `}`.
fn extract_machine(view: &MachineView, source: &str, index: &TextLineIndex) -> Machine {
    let kw = header_token(view.syntax());
    let (line, col) = index.line_col(kw.text_range().start);
    let parts = extract_world(view.world(), source, index);
    Machine {
        line,
        col,
        span: index.span(TextRange::new(
            kw.text_range().start,
            view.syntax().text_range().end,
        )),
        tapes: parts.tapes,
        states: parts.states,
        grafts: parts.grafts,
        binds: parts.binds,
        doc: extract_doc(view.doc_run(), source, index),
    }
}

/// Walk one level of `RootView`/`NamespaceView::items`, in source
/// ORDER: a namespace recurses IN PLACE, depth-first, so a declaration
/// written after a namespace block lands after that namespace's own
/// contents in every vector of the [`Program`]. `ns` is a path stamped
/// on each declaration, never a prefix folded into its name.
///
/// Top-level comments carry no green node at all — they are trivia,
/// dropped before `items()` ever sees them — so there is no comment
/// case to skip here.
fn extract_items(
    items: impl Iterator<Item = TopView>,
    ns: &[String],
    source: &str,
    index: &TextLineIndex,
    program: &mut Program,
) {
    for item in items {
        match item {
            TopView::Use(decl) => {
                for path in decl.paths() {
                    program.imports.push(extract_import(&path, ns, index));
                }
            }
            TopView::Alphabet(a) => program
                .alphabets
                .push(extract_alphabet(&a, ns, source, index)),
            TopView::Namespace(nsv) => {
                let mut child = ns.to_vec();
                child.push(nsv.name());
                extract_items(nsv.items(), &child, source, index, program);
            }
            TopView::Reuse(r) => {
                let parts = extract_reuse(&r, source, index);
                match r.kind() {
                    ReuseKind::Routine => program.routines.push(Routine {
                        name: parts.name,
                        name_span: parts.name_span,
                        line: parts.line,
                        col: parts.col,
                        exported: parts.exported,
                        ns: ns.to_vec(),
                        sig: parts.sig,
                        states: parts.states,
                        grafts: parts.grafts,
                        binds: parts.binds,
                        doc: parts.doc,
                    }),
                    ReuseKind::Graph => program.graphs.push(Graph {
                        name: parts.name,
                        name_span: parts.name_span,
                        line: parts.line,
                        col: parts.col,
                        exported: parts.exported,
                        ns: ns.to_vec(),
                        sig: parts.sig,
                        states: parts.states,
                        grafts: parts.grafts,
                        binds: parts.binds,
                        doc: parts.doc,
                    }),
                }
            }
            TopView::Machine(m) => program.machine = Some(extract_machine(&m, source, index)),
        }
    }
}

/// Rebuild the whole [`Program`] from the green tree's root: one
/// [`TextLineIndex`] built once and threaded through the whole walk,
/// then [`extract_items`] over the file's own top-level items with an
/// empty starting `ns`.
pub fn extract_program(root: &SyntaxNode, source: &str) -> Program {
    let index = TextLineIndex::new(source);
    let file = RootView::cast(root.clone()).expect("root is ROOT");
    let mut program = Program {
        imports: Vec::new(),
        alphabets: Vec::new(),
        routines: Vec::new(),
        graphs: Vec::new(),
        machine: None,
    };
    extract_items(file.items(), &[], source, &index, &mut program);
    program
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use mtc_core::diagnostics::Pos;
    use mtc_core::syntax::AstNode;
    use proptest::prelude::*;

    use super::*;
    use crate::lexer::{CommentKind, LexMode, lex, lex_with};
    use crate::parser::{
        BindingValue, Program, SigParamKind, SymLit, Transition, parse_green, reparse_binding_arg,
        reparse_doc_items, reparse_sig_param, reparse_sym_map, reparse_transition,
    };
    use crate::syntax::{RootView, TopView};

    /// Every payload-carrying kind, and the shapes that make each one
    /// differ from its raw text: an escaped glyph, a glyph holding a
    /// backslash, a number with a leading zero, a doc line with the one
    /// optional space after the sigil, and one without it. Rebuilt
    /// tokens are compared field by field — kind, line, col AND len —
    /// against a real `lex_with` run over the identical source, which
    /// is exactly what would go silently wrong if a payload rebuilt to
    /// the right VALUE from the wrong SPAN, or vice versa.
    ///
    /// The oracle is `LexMode::WithComments`, which is sound here only
    /// because this fixture is comment-free: `sig_tokens` filters
    /// comments as trivia, so a `WithComments` lex of a fixture that
    /// DID contain one would carry a `Comment` token the rebuilt side
    /// can never produce, failing this assertion for a reason that has
    /// nothing to do with retokenization fidelity. Do not add a
    /// comment to this fixture — reach for `LexMode::WithoutComments`
    /// (as the census fixture below does) if that coverage is ever
    /// wanted here.
    #[test]
    fn rebuilt_tokens_are_indistinguishable_from_lexing_the_same_span() {
        let src = "? doc\n?no space\n! [deprecated] gone\n\
                   alphabet ab { '_', '\\'', '\\\\' }\n\
                   machine {\n  tape main: ab;\n\
                   \x20 entry state s {\n    ['_'] -> write [{v+007}] move [>] goto s;\n\
                   \x20   [*] -> stop;\n  }\n}\n";
        let lexed = lex_with(src, LexMode::WithComments).expect("lexes");
        let green = crate::parser::parse_green(src).expect("parses");
        let root = SyntaxNode::new_root(green);
        let index = TextLineIndex::new(src);
        let rebuilt = sig_tokens(&root, &index);

        // Compare kind, line, col and len — not just kind. A payload that
        // rebuilds to the wrong string, or a len counting resolved chars
        // instead of written ones, is exactly the failure this catches.
        assert_eq!(rebuilt.len(), lexed.len(), "token count");
        for (i, (a, b)) in rebuilt.iter().zip(lexed.iter()).enumerate() {
            assert_eq!(
                (&a.kind, a.line, a.col, a.len),
                (&b.kind, b.line, b.col, b.len),
                "token {i} differs"
            );
        }
    }

    /// Drift guard complementing the fixture above: the brief's own
    /// fixture reaches barely half of the SIGNIFICANT `TmcKind` domain,
    /// so a wrong `token_from_syntax` arm on any kind it never holds
    /// (`DotDot`, `FatArrow`, `ColonColon`, `Dot`, `Percent`, `Lt`,
    /// `At`, `Eq`, …) would ship silently. This fixture is engineered
    /// to hold every kind the PARSER can actually accept, and the test
    /// asserts the census before trusting the comparison — the same
    /// two-step shape `PmcKind`'s own sibling test uses.
    ///
    /// `At` and `Bang` are excluded from the census on purpose: neither
    /// is matched by any parser production (`grep -n
    /// 'TokenKind::\(At\|Bang\)\b' src/parser.rs` finds only the
    /// `describe()` error-formatting arm) — `@` is lexed but never
    /// consumed by any grammar rule, and a bare `!` is only ever an
    /// [`TokenKind::AttentionLine`] at line-start or a lex/parse error
    /// elsewhere, so neither kind can ever reach a parsed green tree as
    /// its own leaf.
    #[test]
    fn sig_tokens_over_a_full_file_reaches_every_reachable_significant_kind() {
        let src = "use lib::a, lib::b as c;\n\
                   ? doc line with a trailing space \n\
                   ! plain attention\n\
                   alphabet ab { '0'..'9', '_' }\n\
                   routine r(tape t: ab writes { '0' }) {\n\
                   \x20 entry state s {\n\
                   \x20   ['0'] -> write [-, {(v+1)%6}, {v-1}] move [<, >, .] goto s;\n\
                   \x20   [*] -> stop;\n\
                   \x20 }\n\
                   \x20 graft g(t = t with map { '0'->'1', '2'=>'3' }) as inst;\n\
                   }\n\
                   machine {\n\
                   \x20 tape main: ab;\n\
                   \x20 bind r(t = main) as m;\n\
                   }\n";

        let lexed = lex(src).expect("lexes");
        let seen: HashSet<&'static str> = lexed
            .iter()
            .filter_map(|t| {
                Some(match &t.kind {
                    TokenKind::Ident(_) => "Ident",
                    TokenKind::Number(..) => "Number",
                    TokenKind::Glyph(_) => "Glyph",
                    TokenKind::DotDot => "DotDot",
                    TokenKind::Arrow => "Arrow",
                    TokenKind::FatArrow => "FatArrow",
                    TokenKind::ColonColon => "ColonColon",
                    TokenKind::Dot => "Dot",
                    TokenKind::Dash => "Dash",
                    TokenKind::Plus => "Plus",
                    TokenKind::Eq => "Eq",
                    TokenKind::Star => "Star",
                    TokenKind::Percent => "Percent",
                    TokenKind::Lt => "Lt",
                    TokenKind::Gt => "Gt",
                    TokenKind::LBracket => "LBracket",
                    TokenKind::RBracket => "RBracket",
                    TokenKind::LBrace => "LBrace",
                    TokenKind::RBrace => "RBrace",
                    TokenKind::LParen => "LParen",
                    TokenKind::RParen => "RParen",
                    TokenKind::Comma => "Comma",
                    TokenKind::Semi => "Semi",
                    TokenKind::Colon => "Colon",
                    TokenKind::DocLine(_) => "DocLine",
                    TokenKind::AttentionLine(_) => "AttentionLine",
                    TokenKind::At | TokenKind::Bang | TokenKind::Eof | TokenKind::Comment(_) => {
                        return None;
                    }
                })
            })
            .collect();
        for kind in [
            "Ident",
            "Number",
            "Glyph",
            "DotDot",
            "Arrow",
            "FatArrow",
            "ColonColon",
            "Dot",
            "Dash",
            "Plus",
            "Eq",
            "Star",
            "Percent",
            "Lt",
            "Gt",
            "LBracket",
            "RBracket",
            "LBrace",
            "RBrace",
            "LParen",
            "RParen",
            "Comma",
            "Semi",
            "Colon",
            "DocLine",
            "AttentionLine",
        ] {
            assert!(seen.contains(kind), "fixture is missing a {kind} token");
        }
        // Proves only that THIS fixture's own kind set is exactly these
        // 26 names — no fewer (the loop above already checked that),
        // and no unlisted 27th kind smuggled in alongside them. It is
        // NOT independent evidence that the reachable domain's true
        // size is 26; that count comes from the empirical grep census
        // this module's own doc comments describe (`token_kind`'s 28
        // significant `TmcKind` variants minus the two, `At` and
        // `Bang`, no production ever matches).
        assert_eq!(
            seen.len(),
            26,
            "this fixture's own token-kind set must be exactly the 26 named above"
        );

        let root = SyntaxNode::new_root(parse_green(src).expect("parses"));
        let index = TextLineIndex::new(src);
        let retokenized = sig_tokens(&root, &index);

        assert_eq!(
            retokenized,
            lex_with(src, LexMode::WithoutComments).unwrap()
        );
    }

    /// One glyph-content char, weighted so the escape-triggering
    /// characters (`GLYPH_ESCAPES` — `'`, `\`) turn up often rather
    /// than almost never — `any::<char>()` alone draws either well
    /// under 1% of the time over full Unicode, which would mostly pin
    /// the NON-escape path and leave escape resolution covered only by
    /// the fixed examples elsewhere in this file, not by this
    /// proptest. Reads `GLYPH_ESCAPES` by index rather than
    /// hand-listing its members: this is the ONE remaining site that
    /// would otherwise duplicate the set a third time (`lex_with`'s own
    /// scan and `decode_glyph_body` are the other two, both already
    /// reading the same constant) — the dangerous copy, since a new
    /// escape this generator never produces could never surface a
    /// disagreement between the other two.
    fn glyph_char() -> impl Strategy<Value = char> {
        prop_oneof![
            6 => (0..GLYPH_ESCAPES.len()).prop_map(|i| GLYPH_ESCAPES[i]),
            4 => any::<char>().prop_filter("no embedded newline", |c| *c != '\n'),
        ]
    }

    // `decode_glyph_body` is a SECOND decoder, not a reuse of the
    // lexer's own scan (see its own doc comment for why) — proven
    // equivalent to `lex_with`'s glyph decoding by construction, over
    // arbitrary legal glyph content (escape-heavy by construction, see
    // `glyph_char` above), rather than merely by the fixed examples
    // above. This equivalence holds only over the LEGAL glyph
    // language — `decode_glyph_body`'s own doc comment records why it
    // is TOTAL where the lexer is PARTIAL, and this proptest never
    // constructs an illegal body to probe that gap.
    proptest! {
        #[test]
        fn decode_glyph_body_matches_the_lexer_for_any_legal_content(
            chars in proptest::collection::vec(glyph_char(), 1..12)
        ) {
            let value: String = chars.into_iter().collect();
            let escaped: String = value
                .chars()
                .flat_map(|c| {
                    if GLYPH_ESCAPES.contains(&c) {
                        vec!['\\', c]
                    } else {
                        vec![c]
                    }
                })
                .collect();
            let src = format!("'{escaped}'");
            let tokens = lex(&src).expect("constructed from an accepted value");
            let TokenKind::Glyph(decoded_by_lexer) = &tokens[0].kind else {
                panic!("expected a Glyph token");
            };
            prop_assert_eq!(decoded_by_lexer, &value);
            prop_assert_eq!(decode_glyph_body(&src), value);
        }
    }

    /// Retokenizing a RULE's own TRANSITION node and reparsing it
    /// through `Parser::transition` reproduces the exact `Transition`
    /// written below, across every shape the production itself branches
    /// on: both `Goto` spellings (explicit `goto NAME` and the
    /// bare-name sugar), `Call` (with a real binding list and a
    /// `Terminator` continuation — the shape most likely to disagree,
    /// since it recurses into `binding_args`), `Return`, `Stop`, and
    /// `Halt`.
    ///
    /// `Transition::Stay` — the sixth variant of six — is deliberately
    /// NOT among these: see `reparse_transition`'s own doc comment for
    /// why no fixture could ever exercise it (it is the absence of a
    /// TRANSITION node, never a shape this shim is called for).
    ///
    /// Every expected value below is a literal, captured from the
    /// retired hand-written-CST lowering of this same fixture while that
    /// path was still callable and written down before it went away. The literals are what makes
    /// this a fidelity pin rather than a comparison of two sides that
    /// could drift together: a `goto` that stopped being
    /// `explicit: true`, or a span that moved by a column, fails here
    /// against a fixed value.
    #[test]
    fn reparsed_transition_equals_the_expected_transition_across_every_reachable_shape() {
        let src = "routine r() {\n  entry state a {\n    \
                   ['0'] -> goto a;\n    ['1'] -> a;\n    ['2'] -> return;\n    \
                   ['3'] -> stop;\n    ['4'] -> halt;\n    \
                   ['5'] -> call sub(t = a) then halt;\n  }\n}\n";

        let expected_transitions: Vec<Transition> = vec![
            Transition::Goto {
                name: "a".to_string(),
                explicit: true,
                span: Span::new(3, 14, 3, 20),
            },
            Transition::Goto {
                name: "a".to_string(),
                explicit: false,
                span: Span::new(4, 14, 4, 15),
            },
            Transition::Return {
                span: Span::new(5, 14, 5, 20),
            },
            Transition::Stop {
                span: Span::new(6, 14, 6, 18),
            },
            Transition::Halt {
                span: Span::new(7, 14, 7, 18),
            },
            Transition::Call {
                target: crate::parser::QualName {
                    segments: vec!["sub".to_string()],
                    span: Span::new(8, 19, 8, 22),
                },
                args: vec![crate::parser::BindingArg {
                    name: "t".to_string(),
                    name_span: Span::new(8, 23, 8, 24),
                    value: BindingValue::Named {
                        target: "a".to_string(),
                        target_span: Span::new(8, 27, 8, 28),
                        map: None,
                    },
                    span: Span::new(8, 23, 8, 28),
                }],
                then: crate::parser::Continuation::Halt {
                    span: Span::new(8, 35, 8, 39),
                },
                span: Span::new(8, 14, 8, 39),
            },
        ];

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let TopView::Reuse(reuse_view) = root.items().next().expect("one item") else {
            panic!("expected a REUSE");
        };
        let state_view = reuse_view
            .world()
            .expect("reuse carries a world")
            .states()
            .next()
            .expect("one state");
        let green_transitions: Vec<Transition> = state_view
            .rules()
            .map(|rule_view| {
                let transition_view = rule_view
                    .transition()
                    .expect("every rule in this fixture writes a transition");
                reparse_transition(&sig_tokens(transition_view.syntax(), &index))
            })
            .collect();

        assert_eq!(green_transitions, expected_transitions);
    }

    /// Retokenizing a GRAFT's own BINDING_ARG node and reparsing it
    /// through `Parser::binding_arg` reproduces the exact `BindingArg`
    /// written below, `with map { … }` included — the unit
    /// `crate::syntax::kinds`'s own module doc names as what a caller
    /// "retokenizes and hands back to `Parser::binding_arg`". The
    /// expected value is a literal captured from the retired
    /// hand-written-CST lowering of this fixture while that path was
    /// still callable.
    #[test]
    fn reparsed_binding_arg_equals_the_expected_binding_arg() {
        let src = "routine r() {\n  entry state a {\n    [*] -> stop;\n  }\n  \
                   graft a(x = y with map { '0'->'1' }) as inst;\n}\n";

        let expected_arg = crate::parser::BindingArg {
            name: "x".to_string(),
            name_span: Span::new(5, 11, 5, 12),
            value: BindingValue::Named {
                target: "y".to_string(),
                target_span: Span::new(5, 15, 5, 16),
                map: Some(crate::parser::SymMap {
                    pairs: vec![crate::parser::MapPair {
                        src: SymLit::Glyph {
                            value: "0".to_string(),
                            span: Span::new(5, 28, 5, 31),
                        },
                        dst: SymLit::Glyph {
                            value: "1".to_string(),
                            span: Span::new(5, 33, 5, 36),
                        },
                        arrow: crate::parser::MapArrow::Bidirectional,
                        span: Span::new(5, 28, 5, 36),
                    }],
                    span: Span::new(5, 22, 5, 38),
                }),
            },
            span: Span::new(5, 11, 5, 38),
        };

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let TopView::Reuse(reuse_view) = root.items().next().expect("one item") else {
            panic!("expected a REUSE");
        };
        let graft_view = reuse_view
            .world()
            .expect("reuse carries a world")
            .grafts()
            .next()
            .expect("one graft");
        let arg_view = graft_view.bindings().next().expect("one binding arg");
        let green_arg = reparse_binding_arg(&sig_tokens(arg_view.syntax(), &index));

        assert_eq!(green_arg, expected_arg);
    }

    /// Retokenizing a BINDING_ARG's own SYM_MAP node and reparsing it
    /// through `Parser::sym_map` reproduces the exact `SymMap` written
    /// below, both arrow flavors included. The expected value is a
    /// literal captured from the retired hand-written-CST lowering of
    /// this fixture while that path was still callable.
    ///
    /// This is also `reparse_sym_map`'s ONLY caller anywhere in the
    /// crate — the shim exists for a language-server use case nothing
    /// has reached yet and carries `#[allow(dead_code)]` for it — so
    /// this test is what keeps `Parser::sym_map` reachable from a green
    /// node at all. Deleting it silently retires the shim.
    #[test]
    fn reparsed_sym_map_equals_the_expected_sym_map() {
        let src = "routine r() {\n  entry state a {\n    [*] -> stop;\n  }\n  \
                   graft a(x = y with map { '0'->'1', '2'=>'3' }) as inst;\n}\n";

        let expected_map = crate::parser::SymMap {
            pairs: vec![
                crate::parser::MapPair {
                    src: SymLit::Glyph {
                        value: "0".to_string(),
                        span: Span::new(5, 28, 5, 31),
                    },
                    dst: SymLit::Glyph {
                        value: "1".to_string(),
                        span: Span::new(5, 33, 5, 36),
                    },
                    arrow: crate::parser::MapArrow::Bidirectional,
                    span: Span::new(5, 28, 5, 36),
                },
                crate::parser::MapPair {
                    src: SymLit::Glyph {
                        value: "2".to_string(),
                        span: Span::new(5, 38, 5, 41),
                    },
                    dst: SymLit::Glyph {
                        value: "3".to_string(),
                        span: Span::new(5, 43, 5, 46),
                    },
                    arrow: crate::parser::MapArrow::ReadOnly,
                    span: Span::new(5, 38, 5, 46),
                },
            ],
            span: Span::new(5, 22, 5, 48),
        };

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let TopView::Reuse(reuse_view) = root.items().next().expect("one item") else {
            panic!("expected a REUSE");
        };
        let graft_view = reuse_view
            .world()
            .expect("reuse carries a world")
            .grafts()
            .next()
            .expect("one graft");
        let arg_view = graft_view.bindings().next().expect("one binding arg");
        let map_view = arg_view.sym_map().expect("binding arg carries a map");
        let green_map = reparse_sym_map(&sig_tokens(map_view.syntax(), &index));

        assert_eq!(green_map, expected_map);
    }

    /// Retokenizing a REUSE's own SIG_PARAM node and reparsing it
    /// through `Parser::sig_param` reproduces the exact `SigParam`s
    /// written below, across BOTH shapes the production itself branches
    /// on: `Tape` (`writes`/`preserves` clauses included) and the plain
    /// `State` parameter — a fixture with only the `Tape` shape would
    /// leave the `State` arm of `Parser::sig_param` entirely unpinned.
    /// The expected values are literals captured from the retired
    /// hand-written-CST lowering of this fixture while that path was
    /// still callable.
    #[test]
    fn reparsed_sig_param_equals_the_expected_sig_param_for_both_shapes() {
        let src = "routine r(tape t: ab writes { '0' } preserves { '1' }, state s) {\n  \
                   entry state a {\n    [*] -> stop;\n  }\n}\n";

        let expected_params = vec![
            crate::parser::SigParam {
                kind: SigParamKind::Tape {
                    alphabet: "ab".to_string(),
                    alphabet_span: Span::new(1, 19, 1, 21),
                    volatile: false,
                    writes: Some(crate::parser::ContractClause {
                        elems: vec![crate::parser::AlphabetElem::Single(SymLit::Glyph {
                            value: "0".to_string(),
                            span: Span::new(1, 31, 1, 34),
                        })],
                        kw_span: Span::new(1, 22, 1, 28),
                        span: Span::new(1, 22, 1, 36),
                    }),
                    preserves: Some(crate::parser::ContractClause {
                        elems: vec![crate::parser::AlphabetElem::Single(SymLit::Glyph {
                            value: "1".to_string(),
                            span: Span::new(1, 49, 1, 52),
                        })],
                        kw_span: Span::new(1, 37, 1, 46),
                        span: Span::new(1, 37, 1, 54),
                    }),
                },
                name: "t".to_string(),
                name_span: Span::new(1, 16, 1, 17),
                span: Span::new(1, 11, 1, 54),
            },
            crate::parser::SigParam {
                kind: SigParamKind::State,
                name: "s".to_string(),
                name_span: Span::new(1, 62, 1, 63),
                span: Span::new(1, 56, 1, 63),
            },
        ];

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let TopView::Reuse(reuse_view) = root.items().next().expect("one item") else {
            panic!("expected a REUSE");
        };
        let green_params: Vec<_> = reuse_view
            .params()
            .map(|param_view| reparse_sig_param(&sig_tokens(param_view.syntax(), &index)))
            .collect();

        assert_eq!(green_params, expected_params);
    }

    /// A `?` line item, spelled compactly so the expected doc runs below
    /// read as tables.
    fn doc_item(blank_before: bool, text: &str, span: Span) -> DocRunItem {
        DocRunItem {
            blank_before,
            kind: DocRunKind::Doc {
                text: text.to_string(),
                span,
            },
        }
    }

    /// A `!` line item. `attr` is the `[name]` prefix and its own span
    /// when the line carries one — `Parser::parse_attr` computes that
    /// span from the token's `len`, which is the arithmetic these
    /// fixtures exist to hold.
    fn attention_item(
        blank_before: bool,
        attr: Option<(&str, Span)>,
        text: &str,
        span: Span,
    ) -> DocRunItem {
        DocRunItem {
            blank_before,
            kind: DocRunKind::Attention {
                attr: attr.map(|(name, span)| crate::parser::AttrCst {
                    name: name.to_string(),
                    span,
                }),
                text: text.to_string(),
                span,
            },
        }
    }

    /// A comment item inside a run. A comment carries no span of its
    /// own in the CST — its `own_line` flag and verbatim text are what
    /// the printer reads back.
    fn comment_item(
        blank_before: bool,
        text: &str,
        kind: CommentKind,
        own_line: bool,
    ) -> DocRunItem {
        DocRunItem {
            blank_before,
            kind: DocRunKind::Comment(crate::lexer::Comment {
                text: text.to_string(),
                kind,
                own_line,
            }),
        }
    }

    /// Retokenizing an ALPHABET's own bound DOC_RUN and reparsing it
    /// through `Parser::doc_run` reproduces the exact `Vec<DocRunItem>`
    /// — this run is comment-free and the file's very first construct,
    /// so both sides' `blank_before` gap-tracking starts from the same
    /// fresh line-0, and raw item equality holds (not merely
    /// `reduce_doc_run` equality — `Parser::doc_run` itself never reads
    /// `blank_before`, so there is nothing for the fresh-`prev_end_line`
    /// caveat to distort here).
    ///
    /// The attention line carries `[deprecated]`, not bare prose: a bare
    /// `!` line leaves `attr: None` on every item, which cannot
    /// discriminate a wrong `sig_tokens`-derived `len` —
    /// `Parser::parse_attr` locates the attribute's own `[` column as
    /// `token.len - 1 - text.chars().count()`, so only the `Some` arm
    /// exercises that arithmetic at all. Its result, `AttrCst.span`, is
    /// written out in the expected value below, which compares every
    /// field, span included.
    ///
    /// The expected run is a literal captured from the retired
    /// hand-written-CST lowering of this fixture while that path was
    /// still callable.
    #[test]
    fn reparsed_doc_items_equal_the_expected_doc_run() {
        let src = "? doc line\n! [deprecated] gone\nalphabet ab { '0' }\n";

        let expected_doc_run = vec![
            doc_item(false, "doc line", Span::new(1, 1, 1, 11)),
            attention_item(
                false,
                Some(("deprecated", Span::new(2, 4, 2, 14))),
                "[deprecated] gone",
                Span::new(2, 1, 2, 20),
            ),
        ];

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let TopView::Alphabet(alphabet_view) = root.items().next().expect("one item") else {
            panic!("expected an ALPHABET");
        };
        let doc_run_view = alphabet_view.doc_run().expect("alphabet carries a doc run");
        // `0`: this run is the file's very first construct, so the real
        // parse's own `prev_end_line` was still its initial `0` too —
        // see `reparsed_doc_items_pin_blank_before_when_the_run_abuts_a_preceding_declaration`
        // below for the case where that is NOT `0` and the caller must
        // supply the real value.
        let green_doc_run = reparse_doc_items(&sig_tokens(doc_run_view.syntax(), &index), 0);

        assert_eq!(green_doc_run, expected_doc_run);
    }

    /// A comment interleaved inside a `DOC_RUN` (a three-item
    /// `(Doc, Comment, Attention)` run as written) cannot survive one
    /// pass of retokenization — `sig_tokens` drops it as trivia, so the
    /// SHIM's raw `Vec<DocRunItem>` is strictly SHORTER than the run in
    /// source. This is a
    /// test of the shim alone, called directly and deliberately:
    /// [`extract_doc_items`] no longer hands it a whole comment-bearing
    /// run, precisely because `fmt` needs those comments back, and
    /// segments the run around them instead.
    ///
    /// What a caller reading only a reduced [`Doc`] needs is
    /// `reduce_doc_run` equality, not raw item equality — this proves
    /// that holds even for the lossy one-pass form, because
    /// `DocRunKind::Comment` is fully inert in `.tmc`'s own
    /// `reduce_doc_run` regardless of position (`DocRunKind::Comment(_)
    /// => {}` — no paragraph split, no attention/`deprecated` effect):
    /// dropping the comment can never change the reduced [`Doc`]. That
    /// inertness is also what makes the segmented walk above safe to add
    /// to a path the compiler front runs. Mirrors the PM sibling's own
    /// `reparsed_doc_items_reduce_to_the_same_fndoc_when_comments_interleave`.
    ///
    /// The three-item run this fixture writes, and the reduced [`Doc`]
    /// it folds to, are literals captured from the retired
    /// hand-written-CST lowering of this fixture while that path was
    /// still callable.
    #[test]
    fn reparsed_doc_items_reduce_to_the_same_doc_when_comments_interleave() {
        let src = "? doc line\n// interleaved comment\n? more doc\nalphabet ab { '0' }\n";

        // Three items as WRITTEN — the comment is one of them.
        let written_items = 3;
        let expected_doc = Some(crate::parser::Doc {
            paragraphs: vec!["doc line more doc".to_string()],
            attention: Vec::new(),
            deprecated: None,
        });

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let TopView::Alphabet(alphabet_view) = root.items().next().expect("one item") else {
            panic!("expected an ALPHABET");
        };
        let doc_run_view = alphabet_view.doc_run().expect("alphabet carries a doc run");
        // `0`: again the file's first construct — see the note on the
        // test above.
        let green_doc_run = reparse_doc_items(&sig_tokens(doc_run_view.syntax(), &index), 0);
        assert!(
            green_doc_run.len() < written_items,
            "the comment must actually be dropped (strictly shorter than the three \
             items written, not merely different), or this test proves nothing"
        );
        let green_doc = crate::parser::reduce_doc_run(&green_doc_run);

        assert_eq!(green_doc, expected_doc);
    }

    /// `reparse_doc_items`'s `prev_end_line` parameter, on the FIRST
    /// item's `blank_before` — the one field the two comment-free tests
    /// above cannot exercise: both sit at the very start of the file,
    /// where "the real preceding-line value" and "an isolated slice's
    /// own fresh start" both happen to be `0`, hiding the divergence
    /// when they are NOT the same value. This fixture puts the doc run
    /// immediately after (no blank line) a PRECEDING top-level
    /// declaration, so the two genuinely differ, and passes the real
    /// one — read off the tree the same way a caller (extraction) would,
    /// from the preceding sibling's own end line.
    ///
    /// `blank_before: false` on that first item is the load-bearing
    /// literal: it is what a hardcoded `0` for `prev_end_line` gets
    /// wrong on this fixture, and it was captured from the retired
    /// hand-written-CST lowering of this source while that path was
    /// still callable.
    #[test]
    fn reparsed_doc_items_pin_blank_before_when_the_run_abuts_a_preceding_declaration() {
        let src = "alphabet a { '0' }\n? doc line\n! [deprecated] gone\nalphabet b { '0' }\n";

        let expected_doc_run = vec![
            doc_item(false, "doc line", Span::new(2, 1, 2, 11)),
            attention_item(
                false,
                Some(("deprecated", Span::new(3, 4, 3, 14))),
                "[deprecated] gone",
                Span::new(3, 1, 3, 20),
            ),
        ];

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let mut items = root.items();
        let TopView::Alphabet(first_view) = items.next().expect("first item") else {
            panic!("expected the first item to be an alphabet");
        };
        let TopView::Alphabet(second_view) = items.next().expect("second item") else {
            panic!("expected the second item to be an alphabet");
        };
        // The real value a caller (extraction) would supply: the end
        // line of the run's own preceding sibling in the tree — NOT a
        // hardcoded `0`, which is exactly the bug this test guards
        // against reintroducing.
        let (prev_end_line, _) = index.line_col(first_view.syntax().text_range().end);
        let doc_run_view = second_view
            .doc_run()
            .expect("second alphabet carries a doc run");
        let green_doc_run =
            reparse_doc_items(&sig_tokens(doc_run_view.syntax(), &index), prev_end_line);

        assert_eq!(green_doc_run, expected_doc_run);
    }

    /// The fixture every "anchor on a token, never on the node" claim
    /// rests on: one doc-run-carrying declaration of EVERY shape that
    /// accepts a run. A documented declaration's node starts at its doc
    /// run, so an extraction reading `SyntaxNode::text_range().start`
    /// for a `line`/`col`/`span.start` lands on the run rather than on
    /// the header — a bug a doc-free fixture cannot see at all.
    ///
    /// Every assertion is an `assert_ne!` against exactly that wrong
    /// answer, computed here from the node itself, plus two `assert_eq!`s
    /// on the reduced docs — because every `assert_ne!` above would still
    /// hold with `doc: None` stamped everywhere.
    #[test]
    fn extraction_anchors_positions_on_header_tokens_not_on_node_starts() {
        let src = "? alphabet doc\n\
                   alphabet ab { '_', 'a' }\n\
                   \n\
                   ? routine doc\n\
                   ! [deprecated] old\n\
                   export routine r(tape t: ab, state s) {\n\
                   \x20 ? state doc\n\
                   \x20 entry state a {\n\
                   \x20   [*] -> stop;\n\
                   \x20 }\n\
                   \n\
                   \x20 ? graft doc\n\
                   \x20 graft gr(t = t) as inst;\n\
                   \n\
                   \x20 ? bind doc\n\
                   \x20 bind r2(t = t) as bd;\n\
                   }\n\
                   \n\
                   ? graph doc\n\
                   graph gr(tape t: ab) {\n\
                   \x20 entry state a {\n\
                   \x20   [*] -> stop;\n\
                   \x20 }\n\
                   }\n\
                   \n\
                   ? machine doc\n\
                   machine {\n\
                   \x20 tape main: ab;\n\
                   \n\
                   \x20 ? machine state doc\n\
                   \x20 entry state s {\n\
                   \x20   [*] -> stop;\n\
                   \x20 }\n\
                   }\n";

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let program = extract_program(root.syntax(), src);

        /// The line a node STARTS on — what a wrong extraction would
        /// have used. Every assertion below states that the extracted
        /// value is something else.
        #[track_caller]
        fn node_line(node: &SyntaxNode, index: &TextLineIndex) -> u32 {
            index.line_col(node.text_range().start).0
        }

        let mut items = root.items();
        let TopView::Alphabet(alphabet_view) = items.next().expect("first item") else {
            panic!("expected an ALPHABET");
        };
        let TopView::Reuse(routine_view) = items.next().expect("second item") else {
            panic!("expected a REUSE");
        };
        let TopView::Reuse(graph_view) = items.next().expect("third item") else {
            panic!("expected a REUSE");
        };
        let TopView::Machine(machine_view) = items.next().expect("fourth item") else {
            panic!("expected a MACHINE");
        };
        let world = routine_view.world().expect("the routine carries a world");

        assert_ne!(
            program.alphabets[0].line,
            node_line(alphabet_view.syntax(), &index),
            "the alphabet must carry a doc run above its header"
        );
        assert_ne!(
            program.routines[0].line,
            node_line(routine_view.syntax(), &index),
            "the routine must carry a doc run above its header"
        );
        assert_ne!(
            program.graphs[0].line,
            node_line(graph_view.syntax(), &index),
            "the graph must carry a doc run above its header"
        );
        let machine = program
            .machine
            .as_ref()
            .expect("the fixture declares a machine");
        assert_ne!(
            machine.line,
            node_line(machine_view.syntax(), &index),
            "the machine must carry a doc run above its header"
        );
        assert_ne!(
            machine.span.start.line,
            node_line(machine_view.syntax(), &index),
            "a machine's span starts at its keyword, never at its doc run"
        );
        assert_ne!(
            program.routines[0].states[0].span.start.line,
            node_line(world.states().next().expect("one state").syntax(), &index),
            "a state's span starts at its `entry`/`state` header, never at its doc run"
        );
        assert_ne!(
            program.routines[0].grafts[0].line,
            node_line(world.grafts().next().expect("one graft").syntax(), &index),
            "the graft must carry a doc run above its header"
        );
        assert_ne!(
            program.routines[0].binds[0].line,
            node_line(world.binds().next().expect("one bind").syntax(), &index),
            "the bind must carry a doc run above its header"
        );

        // The reduced docs themselves must actually have landed — every
        // `assert_ne!` above would still hold with `doc: None` stamped
        // everywhere.
        assert_eq!(
            program.alphabets[0]
                .doc
                .as_ref()
                .map(|d| d.paragraphs.clone()),
            Some(vec!["alphabet doc".to_string()])
        );
        assert_eq!(
            program.routines[0]
                .doc
                .as_ref()
                .and_then(|d| d.deprecated.clone()),
            Some("old".to_string()),
            "the routine's `[deprecated]` attention line must survive extraction"
        );
    }

    /// A declaration's own header tokens may sit on DIFFERENT lines
    /// from each other — nothing in the grammar forces `alphabet` and
    /// its name onto one line. Extraction reads `Alphabet::line` and
    /// `Reuse::line` off the NAME token while reading `col` off the
    /// HEADER token, and `Graft::line` off the `graft` keyword while
    /// its span starts at an `entry` prefix; in canonically formatted
    /// source every one of those pairs shares a line, so a wrong anchor
    /// is invisible. This fixture splits them so it is not.
    ///
    /// Two more pairs joined this fixture rather than getting their own
    /// (mutation-proved: `extract_tape`'s `line` swapped to the NAME's
    /// and `extract_import`'s `line` swapped to the LAST segment's both
    /// survived every other test in this module). `extract_tape` reads
    /// `line` off [`header_token`] (`volatile` when written, else
    /// `tape`), and `extract_import` reads it off the FIRST path
    /// segment — a `volatile` tape whose modifier sits on its own line,
    /// and a `use` path split across a `::`, both make those anchors
    /// observable the same way the declarations above do.
    ///
    /// Deliberately NOT canonically formatted: `tmt fmt` would rejoin
    /// every header. It parses — that is all a fixture pinning position
    /// arithmetic needs.
    #[test]
    fn extraction_agrees_when_a_declarations_header_spans_lines() {
        let src = "alphabet\n\
                   \x20 ab { '0', '1' }\n\
                   \n\
                   export\n\
                   routine\n\
                   \x20 r(tape t: ab) {\n\
                   \x20 entry\n\
                   \x20 state\n\
                   \x20   a {\n\
                   \x20   [*] -> stop;\n\
                   \x20 }\n\
                   }\n\
                   \n\
                   machine {\n\
                   \x20 volatile\n\
                   \x20 tape main: ab;\n\
                   \x20 entry\n\
                   \x20 graft\n\
                   \x20   r(t = main);\n\
                   }\n\
                   \n\
                   use lib\n\
                   \x20 ::a;\n";

        let program = extract_program(&SyntaxNode::new_root(parse_green(src).unwrap()), src);
        // Each pair below is what the split buys: a value read off the
        // wrong one of the two tokens would differ.
        assert_eq!(
            (program.alphabets[0].line, program.alphabets[0].col),
            (2, 1),
            "an alphabet's line is its NAME's, its col its `alphabet` keyword's"
        );
        assert_eq!(
            (program.routines[0].line, program.routines[0].col),
            (6, 1),
            "a routine's line is its NAME's, its col its `export` prefix's"
        );
        let state = &program.routines[0].states[0];
        assert_eq!(
            (state.line, state.col, state.span.start.line),
            (9, 3, 7),
            "a state's line is its NAME's, its col and span start its `entry` prefix's"
        );
        let machine = program
            .machine
            .as_ref()
            .expect("the fixture declares a machine");
        let tape = &machine.tapes[0];
        assert!(tape.volatile, "the fixture's own tape is `volatile`");
        assert_eq!(
            tape.line, 15,
            "a tape's line is its HEADER's (`volatile` when written), not its NAME's"
        );
        let graft = &machine.grafts[0];
        assert_eq!(
            (graft.line, graft.span.start.line),
            (18, 17),
            "a graft's line is its `graft` keyword's, but its span starts at `entry`"
        );
        assert_eq!(
            program.imports[0].line, 22,
            "an import's line is its FIRST path segment's, not its LAST's"
        );
    }

    /// A `Rule`'s `transition` field carries its own `span`, and nothing
    /// in the crate ever asserts it: a diagnostic points at a
    /// declaration's or a pattern's span, never at a bare transition's,
    /// so the position half of `reparse_transition`
    /// (`crate::parser::reparse_transition`) had no pin at all once the
    /// differential oracle against the hand-written CST stopped running
    /// — only its CONTENT (the
    /// `explicit` flag, `Call.args`, `Continuation`, …) is read anywhere
    /// downstream.
    ///
    /// Proven to discriminate: a uniform `+1` on every `Transition`
    /// variant's `span.start.col` inside `reparse_transition`'s returned
    /// value (a match over all six variants, each arm bumping the
    /// column before returning) turns this test red by name, while
    /// every other test in the crate (`--lib` and the full single-crate
    /// suite alike) stays green. Reverting the mutation turns it back
    /// green.
    ///
    /// Covers two of the six variants (`Goto`, in both its `explicit`
    /// spellings, and `Stop`) across three assertions in one fixture,
    /// since the mutation is uniform across all six and a single
    /// variant already closes the hole — the extra assertions are
    /// cheap insurance, not required breadth. Every literal below was
    /// validated against the retired hand-written-CST lowering while
    /// that path was still callable; with it gone, these literals are
    /// falsifiable only by re-deriving the grammar by hand.
    #[test]
    fn transition_spans_are_pinned_by_value() {
        let src = "alphabet ab { '0', '1' }\n\
                   \n\
                   machine {\n\
                   \x20 tape main: ab;\n\
                   \x20 entry state a {\n\
                   \x20   [*] -> goto b;\n\
                   \x20 }\n\
                   \x20 state b {\n\
                   \x20   [*] -> c;\n\
                   \x20 }\n\
                   \x20 state c {\n\
                   \x20   [*] -> stop;\n\
                   \x20 }\n\
                   }\n";
        let program: Program = crate::parser::parse(src).expect("parses");
        let states = &program
            .machine
            .as_ref()
            .expect("fixture declares a machine")
            .states;

        assert_eq!(
            states[0].rules[0].transition,
            Transition::Goto {
                name: "b".to_string(),
                explicit: true,
                span: Span {
                    start: Pos { line: 6, col: 12 },
                    end: Pos { line: 6, col: 18 },
                },
            },
            "`goto b` — span runs from the `goto` keyword to the name's end"
        );
        assert_eq!(
            states[1].rules[0].transition,
            Transition::Goto {
                name: "c".to_string(),
                explicit: false,
                span: Span {
                    start: Pos { line: 9, col: 12 },
                    end: Pos { line: 9, col: 13 },
                },
            },
            "bare-name goto sugar — span is the name token alone"
        );
        assert_eq!(
            states[2].rules[0].transition,
            Transition::Stop {
                span: Span {
                    start: Pos { line: 12, col: 12 },
                    end: Pos { line: 12, col: 16 },
                },
            },
            "`stop` — span is the `stop` keyword token"
        );
    }

    /// [`extract_items`] recurses into a namespace IN PLACE,
    /// depth-first, so a declaration written after a namespace block
    /// lands after that namespace's own contents in every vector of
    /// the `Program`
    /// — and `ns` is a PATH stamped on each declaration, never a prefix
    /// folded into its name. A per-item append that hoisted namespaces
    /// to the end, or one that joined the path into the name, still
    /// produces the right COUNT; the assertions below are on order and
    /// on `ns`.
    #[test]
    fn extraction_walks_namespaces_in_place_and_stamps_the_path() {
        let src = "namespace outer {\n\
                   \x20 use lib::a, lib::b as c;\n\
                   \n\
                   \x20 export alphabet inner { '0' }\n\
                   \n\
                   \x20 export routine ir(tape t: inner) {\n\
                   \x20   entry state a {\n\
                   \x20     [*] -> stop;\n\
                   \x20   }\n\
                   \x20 }\n\
                   \n\
                   \x20 namespace deep {\n\
                   \x20   alphabet deeper { '1' }\n\
                   \x20 }\n\
                   }\n\
                   \n\
                   use top::d;\n\
                   \n\
                   alphabet last { '2' }\n";

        let program = extract_program(&SyntaxNode::new_root(parse_green(src).unwrap()), src);
        let names: Vec<(&str, Vec<String>)> = program
            .alphabets
            .iter()
            .map(|a| (a.name.as_str(), a.ns.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("inner", vec!["outer".to_string()]),
                ("deeper", vec!["outer".to_string(), "deep".to_string()]),
                ("last", Vec::new()),
            ],
            "namespace contents come first (depth-first, in place), and `ns` is a path"
        );
        assert_eq!(program.routines[0].ns, vec!["outer".to_string()]);
        assert_eq!(
            program.routines[0].name, "ir",
            "the namespace path is stamped alongside the name, never prefixed onto it"
        );

        // `ns` on an IMPORT, and `exported` on an alphabet: both are
        // stamped by the same walk, and both are unpinned by a fixture
        // that keeps every `use` at file level and never writes
        // `export alphabet`.
        assert_eq!(
            program
                .imports
                .iter()
                .map(|i| (i.binding().to_string(), i.ns.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("a".to_string(), vec!["outer".to_string()]),
                ("c".to_string(), vec!["outer".to_string()]),
                ("d".to_string(), Vec::new()),
            ],
            "an import carries the namespace path it was written in"
        );
        assert_eq!(
            program
                .alphabets
                .iter()
                .map(|a| a.exported)
                .collect::<Vec<_>>(),
            vec![true, false, false],
            "only `inner` is written `export alphabet`"
        );
    }

    /// A `namespace` carrying its OWN doc run — the one container whose
    /// bound run lands as a direct child at the same level as its items
    /// (`super::views::top_items`'s doc pastes the dump). Before the
    /// skip that `top_items` now does, this panicked in a debug build
    /// with `unexpected node kind at top level: SyntaxKind(44)`, since
    /// `TopView::cast` refuses a DOC_RUN and the assert fired.
    ///
    /// The `items()` count is asserted directly: a fix that filtered
    /// the DOC_RUN into oblivion and a fix that turned the run into a
    /// phantom item are both wrong, and only the count separates the
    /// correct answer from the second.
    #[test]
    fn extraction_agrees_on_a_documented_namespace() {
        let src = "? outer doc\n\
                   namespace outer {\n\
                   \x20 ? inner doc\n\
                   \x20 alphabet inner { '0' }\n\
                   }\n\
                   \n\
                   ? last doc\n\
                   alphabet last { '1' }\n";

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let TopView::Namespace(nsv) = root.items().next().expect("first item") else {
            panic!("expected a NAMESPACE");
        };
        assert_eq!(
            nsv.items().count(),
            1,
            "a namespace's own doc run is not one of its items"
        );
        assert!(
            nsv.doc_run().is_some(),
            "…but it is still reachable, through `doc_run()`"
        );
        assert_eq!(
            root.items().count(),
            2,
            "the file's own two declarations, neither doc run counted"
        );

        let program = extract_program(root.syntax(), src);
        assert_eq!(program.alphabets.len(), 2);
        assert_eq!(
            program.alphabets[0]
                .doc
                .as_ref()
                .map(|d| d.paragraphs.clone()),
            Some(vec!["inner doc".to_string()]),
            "the INNER declaration's own run still binds to it"
        );
    }

    /// Document order inside a world body, pinned by VALUE rather than
    /// by count. `lower_world_body` walks one interleaved item list and
    /// pushes into four vectors; extraction walks the WORLD's children
    /// four times, once per kind. The two agree only if each of those
    /// walks preserves order — and with one state, one graft and one
    /// bind per fixture (which is every OTHER fixture here) reversing a
    /// vector changes nothing observable.
    ///
    /// Order is not cosmetic in this language: rule order is table-row
    /// order is priority, and a state's position decides which rows a
    /// world's table gets first (docs/tmt/language.md (which rule
    /// fires)). The same fixture also carries two-argument binding lists
    /// on both a `graft` and a `bind`, since a single-argument list
    /// leaves `args` order equally unpinned.
    #[test]
    fn extraction_keeps_world_body_items_in_document_order() {
        let src = "alphabet ab { '0', '1', '_' }\n\
                   \n\
                   machine {\n\
                   \x20 tape first: ab;\n\
                   \x20 tape second: ab;\n\
                   \n\
                   \x20 entry state alpha {\n\
                   \x20   [*] -> stop;\n\
                   \x20 }\n\
                   \n\
                   \x20 state beta {\n\
                   \x20   [*] -> halt;\n\
                   \x20 }\n\
                   \n\
                   \x20 state gamma {\n\
                   \x20   [*] -> return;\n\
                   \x20 }\n\
                   \n\
                   \x20 graft one(t = first, u = second) as g1;\n\
                   \n\
                   \x20 graft two(t = second) as g2;\n\
                   \n\
                   \x20 bind three(t = first, u = second) as b1;\n\
                   \n\
                   \x20 bind four(t = second) as b2;\n\
                   }\n";

        let program = extract_program(&SyntaxNode::new_root(parse_green(src).unwrap()), src);
        let machine = program
            .machine
            .as_ref()
            .expect("the fixture declares a machine");
        assert_eq!(
            machine
                .tapes
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(
            machine
                .states
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta", "gamma"]
        );
        assert_eq!(
            machine
                .grafts
                .iter()
                .map(|g| g.as_name.as_ref().expect("named").name.as_str())
                .collect::<Vec<_>>(),
            vec!["g1", "g2"]
        );
        assert_eq!(
            machine
                .binds
                .iter()
                .map(|b| b.as_name.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b1", "b2"]
        );
        assert_eq!(
            machine.grafts[0]
                .args
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            vec!["t", "u"],
            "a binding list's argument order is its declaration order"
        );
        assert_eq!(
            machine.binds[0]
                .args
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            vec!["t", "u"]
        );
    }

    /// Every rule shape in one state, plus the pieces a rule's own
    /// grammar branches on: both `goto` spellings, `call … then` with a
    /// binding list and a `with map`, `return`/`stop`/`halt`, a
    /// wildcard, a range, an `as` binding, a `debugger` flag with and
    /// without one, `write`/`move` vectors present and absent, a `-`
    /// keep cell, and a `{…}` substitution — passthrough on a glyph
    /// binding, and a real fold on a numeric one.
    ///
    /// Two shapes get their own assertions because both are carried by
    /// an ABSENCE that a wrong extraction could reproduce by accident:
    /// `Transition::Stay` (no TRANSITION node) and `debugger: false` (no
    /// keyword token).
    #[test]
    fn extraction_agrees_across_every_rule_shape() {
        let src = "alphabet ab { '0'..'9', '_' }\n\
                   \n\
                   alphabet nm { 0..9 }\n\
                   \n\
                   export graph gr(tape t: ab writes { '0'..'5' } preserves { '_' }, state k) {\n\
                   \x20 entry state a {\n\
                   \x20   ['0'] -> write ['1'] move [>] goto a;\n\
                   \x20   ['1'] -> a;\n\
                   \x20   ['2' as v] -> write [{v}] move [.];\n\
                   \x20   ['3'..'5'] -> debugger write [-] move [<] return;\n\
                   \x20   ['6'] -> debugger;\n\
                   \x20   ['7'] -> call sub::deep(t = t with map { '0'->'1', '2'=>'3' }, k = halt) \
                   then stop;\n\
                   \x20   [*] -> halt;\n\
                   \x20 }\n\
                   }\n\
                   \n\
                   routine num(tape n: nm) {\n\
                   \x20 entry state a {\n\
                   \x20   [3 as v] -> write [{(v + 1) % 6}] move [>] goto a;\n\
                   \x20   [007 as w] -> write [{w - 1}];\n\
                   \x20   [*] -> stop;\n\
                   \x20 }\n\
                   }\n";

        let program = extract_program(&SyntaxNode::new_root(parse_green(src).unwrap()), src);
        let rules = &program.graphs[0].states[0].rules;
        assert_eq!(rules.len(), 7, "the fixture must keep every rule shape");
        assert!(
            matches!(rules[2].transition, Transition::Stay { .. }),
            "a rule with an action and no written transition extracts as `Stay`: {:?}",
            rules[2].transition
        );
        assert!(
            !rules[2].debugger && rules[3].debugger && rules[4].debugger,
            "the fixture must carry `debugger` rules AND non-`debugger` ones"
        );
        assert!(
            matches!(rules[4].transition, Transition::Stay { .. })
                && rules[4].write.is_none()
                && rules[4].mov.is_none(),
            "a bare `-> debugger;` is the action-only `Stay` shape"
        );
    }

    /// The prefixes and the qualified names, which no other fixture
    /// here reaches: `volatile tape`, an `entry graft` that OMITS its
    /// `as` name (only an entry graft may), a multi-segment `a::b::c`
    /// reuse target on both a `graft` and a `bind`, and a `use` list
    /// with an alias.
    #[test]
    fn extraction_agrees_on_prefixes_aliases_and_qualified_targets() {
        let src = "use lib::a, lib::b as c;\n\
                   \n\
                   alphabet ab { '0', '_' }\n\
                   \n\
                   machine {\n\
                   \x20 volatile tape dev: ab;\n\
                   \x20 tape main: ab;\n\
                   \n\
                   \x20 entry graft lib::sub::g(t = main);\n\
                   \n\
                   \x20 bind lib::sub::r(t = dev) as bd;\n\
                   }\n";

        let program = extract_program(&SyntaxNode::new_root(parse_green(src).unwrap()), src);
        assert_eq!(program.imports.len(), 2);
        assert_eq!(program.imports[1].alias.as_deref(), Some("c"));
        assert_eq!(program.imports[1].path, vec!["lib", "b"]);
        let machine = program
            .machine
            .as_ref()
            .expect("the fixture declares a machine");
        assert!(machine.tapes[0].volatile && !machine.tapes[1].volatile);
        assert!(
            machine.grafts[0].entry && machine.grafts[0].as_name.is_none(),
            "an entry graft may omit its instance name"
        );
        assert_eq!(
            machine.grafts[0].target.segments,
            vec!["lib", "sub", "g"],
            "a qualified target keeps every segment"
        );
        assert_eq!(machine.binds[0].target.segments, vec!["lib", "sub", "r"]);
    }

    /// Comments live only as trivia in the green tree, so every one of
    /// them — between declarations, inside a world, inside a rule list,
    /// inside a bracketed list, riding a `;` — must vanish from the
    /// extracted `Program`, leaving the declarations at the positions
    /// they would carry with the comments deleted.
    ///
    /// Every position below is a literal. A comment claimed as content —
    /// folded into an alphabet's elements, counted as a rule, or read as
    /// the token a `line`/`col` anchors on — moves one of them.
    #[test]
    fn extraction_keeps_no_trace_of_comments_scattered_through_the_file() {
        let src = "// leading\n\
                   alphabet ab { /* interior */ '0', '_' /* after */ }\n\
                   // between\n\
                   \n\
                   machine { // open trailing\n\
                   \x20 // own line\n\
                   \x20 tape main: ab; // riding the semicolon\n\
                   \x20 entry state s {\n\
                   \x20   // before a rule\n\
                   \x20   [*] -> stop; // after a rule\n\
                   \x20 } // after the state\n\
                   } // after the machine\n\
                   // trailing\n";

        let program = extract_program(&SyntaxNode::new_root(parse_green(src).unwrap()), src);

        assert_eq!(program.alphabets.len(), 1);
        let ab = &program.alphabets[0];
        assert_eq!((ab.name.as_str(), ab.line, ab.col), ("ab", 2, 1));
        assert_eq!(
            ab.elems.len(),
            2,
            "the two interior comments are trivia, not elements: {:?}",
            ab.elems
        );
        assert!(ab.doc.is_none(), "a `//` comment is never a doc run");

        let machine = program
            .machine
            .as_ref()
            .expect("the fixture declares a machine");
        assert_eq!(machine.line, 5, "the machine's own header line");
        assert_eq!(machine.tapes.len(), 1);
        assert_eq!(
            (machine.tapes[0].name.as_str(), machine.tapes[0].line),
            ("main", 7)
        );
        assert_eq!(machine.states.len(), 1);
        let state = &machine.states[0];
        assert_eq!((state.name.as_str(), state.line, state.col), ("s", 8, 3));
        assert_eq!(
            state.rules.len(),
            1,
            "the comments around the rule are not rules of their own"
        );
        assert_eq!(state.rules[0].span.start.line, 10);
    }

    /// `prev_end_line` at the level where it IS observable.
    ///
    /// A `Program` can NEVER discriminate this argument: it feeds only
    /// `blank_before`, and `crate::parser::reduce_doc_run` folds over
    /// `DocRunItem::kind` alone — so a `Program` built with a hardcoded
    /// `0` is byte-identical to a correct one. This test therefore
    /// asserts extraction's own [`extract_doc_items`] output directly,
    /// which is the only place the value surfaces. Each expected run is
    /// a literal captured from the retired hand-written-CST lowering of
    /// its fixture while that path was still callable.
    ///
    /// Three cases, because one alone under-determines the rule:
    /// abutting a preceding declaration (`0` fails), abutting a
    /// preceding COMMENT (reading the preceding SIBLING NODE's end
    /// instead of the last non-whitespace content fails — the comment is
    /// trivia, not a sibling), and a genuine blank line (a rule that
    /// always answered `false` fails).
    #[test]
    fn extracted_doc_items_pin_the_runs_first_blank_before() {
        #[track_caller]
        fn check(src: &str, expected: &[DocRunItem]) {
            let root = RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap()))
                .expect("root is ROOT");
            let index = TextLineIndex::new(src);
            let TopView::Alphabet(view) = root.items().last().expect("a last item") else {
                panic!("expected the last item to be an ALPHABET");
            };
            let run = view.doc_run().expect("the alphabet carries a doc run");
            assert_eq!(
                extract_doc_items(&run, src, &index),
                expected,
                "for:\n{src}"
            );
        }

        check(
            "alphabet a { '0' }\n? doc\nalphabet b { '0' }\n",
            &[doc_item(false, "doc", Span::new(2, 1, 2, 6))],
        );
        check(
            "alphabet a { '0' }\n// note\n? doc\nalphabet b { '0' }\n",
            &[doc_item(false, "doc", Span::new(3, 1, 3, 6))],
        );
        check(
            "alphabet a { '0' }\n\n? doc\nalphabet b { '0' }\n",
            &[doc_item(true, "doc", Span::new(3, 1, 3, 6))],
        );
    }

    /// A comment written INSIDE a doc run is one of the run's items, and
    /// [`extract_doc_items`] must produce it where `Parser::doc_run`
    /// does — the whole item, `blank_before` included, since the gap of
    /// every LATER item is measured against it.
    ///
    /// Two placements, and the tree puts them in different places: one
    /// BETWEEN two `?` lines sits inside the DOC_RUN node, one written
    /// below the run's last line sits outside it, in the declaration's
    /// own stream ahead of the header. A fix that reads only the node
    /// handles the first and silently drops the second, so both are
    /// here, on ALPHABET and on NAMESPACE alike.
    ///
    /// Each expected run below is a literal captured from the retired
    /// hand-written-CST lowering of its fixture while that path was
    /// still callable. The
    /// COMPILER path is unaffected by these items either way:
    /// `reduce_doc_run` folds over `kind` and treats a comment item as
    /// inert, so a `Program` cannot discriminate them at all — this
    /// level is the only one that can.
    #[test]
    fn extracted_doc_items_carry_the_comments_written_inside_the_run() {
        #[track_caller]
        fn alphabet_run(src: &str, expected: &[DocRunItem]) {
            let root = RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap()))
                .expect("root is ROOT");
            let TopView::Alphabet(view) = root.items().next().expect("one item") else {
                panic!("expected an ALPHABET");
            };
            let index = TextLineIndex::new(src);
            let run = view.doc_run().expect("the declaration carries a doc run");
            assert!(
                expected
                    .iter()
                    .any(|i| matches!(i.kind, DocRunKind::Comment(_))),
                "fixture must actually write a comment inside the run: {src}"
            );
            assert_eq!(
                extract_doc_items(&run, src, &index),
                expected,
                "for:\n{src}"
            );
        }

        #[track_caller]
        fn namespace_run(src: &str, expected: &[DocRunItem]) {
            let root = RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap()))
                .expect("root is ROOT");
            let TopView::Namespace(view) = root.items().next().expect("one item") else {
                panic!("expected a NAMESPACE");
            };
            let index = TextLineIndex::new(src);
            let run = view.doc_run().expect("the declaration carries a doc run");
            assert!(
                expected
                    .iter()
                    .any(|i| matches!(i.kind, DocRunKind::Comment(_))),
                "fixture must actually write a comment inside the run: {src}"
            );
            assert_eq!(
                extract_doc_items(&run, src, &index),
                expected,
                "for:\n{src}"
            );
        }

        alphabet_run(
            "? doc\n// c\n? more\nalphabet b { '0' }\n",
            &[
                doc_item(false, "doc", Span::new(1, 1, 1, 6)),
                comment_item(false, "// c", CommentKind::Line, true),
                doc_item(false, "more", Span::new(3, 1, 3, 7)),
            ],
        );
        alphabet_run(
            "? doc\n/* c */\nalphabet b { '0' }\n",
            &[
                doc_item(false, "doc", Span::new(1, 1, 1, 6)),
                comment_item(false, "/* c */", CommentKind::Block, true),
            ],
        );
        alphabet_run(
            "? doc\n/* multi\nline */\n? more\nalphabet b { '0' }\n",
            &[
                doc_item(false, "doc", Span::new(1, 1, 1, 6)),
                comment_item(false, "/* multi\nline */", CommentKind::Block, true),
                doc_item(false, "more", Span::new(4, 1, 4, 7)),
            ],
        );
        // The one fixture whose comment sits BELOW a blank line, so the
        // item carries `blank_before: true` — the field every later
        // item's own gap is measured against.
        alphabet_run(
            "? doc\n\n/* c */\nalphabet b { '0' }\n",
            &[
                doc_item(false, "doc", Span::new(1, 1, 1, 6)),
                comment_item(true, "/* c */", CommentKind::Block, true),
            ],
        );

        namespace_run(
            "? doc\n// c\n? more\nnamespace n {\n  alphabet b { '0' }\n}\n",
            &[
                doc_item(false, "doc", Span::new(1, 1, 1, 6)),
                comment_item(false, "// c", CommentKind::Line, true),
                doc_item(false, "more", Span::new(3, 1, 3, 7)),
            ],
        );
        namespace_run(
            "? doc\n/* c */\nnamespace n {\n  alphabet b { '0' }\n}\n",
            &[
                doc_item(false, "doc", Span::new(1, 1, 1, 6)),
                comment_item(false, "/* c */", CommentKind::Block, true),
            ],
        );
    }

    /// A MULTI-LINE block comment riding a `;` was claimed by the
    /// retired parser's `take_trailing`, which left `prev_end_line` at
    /// the `;`.
    /// The green walk must do the same, because the field it feeds — the
    /// FIRST doc item's `blank_before` — is what `fmt` turns into a blank
    /// line before a doc run.
    ///
    /// This closed the last divergence in the SEED, and the claim stops
    /// there: over the shapes this test and the two above cover — a
    /// `}`-closing declaration's trailing comment, an own-line comment, a
    /// chained pair after the `;`, a comment ahead of the `;`, and a
    /// genuine blank line — the two paths agree item for item.
    ///
    /// The seed is no longer the only thing pinned. A divergence INSIDE
    /// the run — [`sig_tokens`] drops an interleaved comment as trivia, so
    /// the run came back SHORTER than the source's and the item after it
    /// measured its gap against the previous DOC LINE, inventing a blank
    /// line nobody wrote — was closed by segmenting the reparse around the
    /// run's own comments (see [`extract_doc_items`]). It was invisible to
    /// every `Program`-level oracle, because `reduce_doc_run` treats a
    /// comment item as fully inert; `a_comment_inside_a_doc_run_agrees` in
    /// `crate::fmt::print` is what watches it now, from the one consumer
    /// that reads these items verbatim.
    #[test]
    fn a_block_comment_riding_a_semicolon_agrees_on_blank_before() {
        let src = "use a; /* one\ntwo */\n? doc\nalphabet b { '0' }\n";

        // `blank_before: true` is the whole point: `take_trailing`
        // claims the comment and leaves `prev_end_line` back at the `;`,
        // two lines above the run, so the run reads as blank-separated.
        let expected = vec![doc_item(true, "doc", Span::new(3, 1, 3, 6))];

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let TopView::Alphabet(view) = root.items().last().expect("a last item") else {
            panic!("expected the last item to be an ALPHABET");
        };
        let green = extract_doc_items(&view.doc_run().expect("a doc run"), src, &index);

        assert_eq!(green, expected);
    }

    /// Both edges of [`prev_end_line`]'s `;` arm, so neither can be
    /// widened away. Every fixture writes a MULTI-LINE block comment
    /// after a declaration, which is the only shape where the two
    /// candidate answers — the terminator's line and the comment's end
    /// line — differ at all:
    ///
    /// - **Alone after a `;`** — `take_trailing` claims it and leaves the
    ///   field on the `;`, so the run reads as blank-separated.
    /// - **Second after a `;`** — `take_trailing` claims AT MOST ONE, so
    ///   the first is the trailing and the rest are drained as ordinary
    ///   pending comments, each of which DOES advance the field, landing
    ///   it back on the last comment's end line.
    /// - **Alone after a `}`** — `capture_close_trailing`, not
    ///   `take_trailing`, so the field advances past it.
    ///
    /// Measured, both directions: dropping the one-comment condition
    /// makes the second fixture report `true` where the literal below
    /// says `false`; dropping the `;` key does the same to the third.
    /// Every literal was captured from the retired hand-written-CST
    /// lowering of its fixture while that path was still callable. A
    /// `Program` cannot
    /// discriminate any of them — `reduce_doc_run` folds over `kind`
    /// alone — so this level is the only one that can.
    #[test]
    fn the_semicolon_arm_is_narrow_in_both_directions() {
        #[track_caller]
        fn check(src: &str, expected: &[DocRunItem]) {
            let root = RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap()))
                .expect("root is ROOT");
            let index = TextLineIndex::new(src);
            let TopView::Alphabet(view) = root.items().last().expect("a last item") else {
                panic!("expected the last item to be an ALPHABET");
            };
            let green = extract_doc_items(&view.doc_run().expect("a doc run"), src, &index);

            assert_eq!(green, expected, "for:\n{src}");
        }

        check(
            "use a; /* one\ntwo */\n? doc\nalphabet b { '0' }\n",
            &[doc_item(true, "doc", Span::new(3, 1, 3, 6))],
        );
        check(
            "use a; /* one */ /* two\nthree */\n? doc\nalphabet b { '0' }\n",
            &[doc_item(false, "doc", Span::new(3, 1, 3, 6))],
        );
        check(
            "alphabet a { '0' } /* one\ntwo */\n? doc\nalphabet b { '0' }\n",
            &[doc_item(false, "doc", Span::new(3, 1, 3, 6))],
        );
    }

    /// Sanity check on the fixtures above: the green reparse shims are
    /// exercised over CONSTRUCTS the parser actually accepts, not
    /// hypothetical shapes — a smoke test that the whole crate's own
    /// `Program` type still round-trips through `parse` on one of them
    /// AND that the fixture's every declared feature actually landed (a
    /// wrong implementation dropping, say, the `preserves` clause or the
    /// second sig param would still leave `routines.len() == 1`, so that
    /// alone proves nothing).

    #[test]
    fn fixture_smoke_check_parses_to_a_program() {
        let src = "routine r(tape t: ab writes { '0' } preserves { '1' }, state s) {\n  \
                   entry state a {\n    [*] -> stop;\n  }\n}\n";
        let program: Program = crate::parser::parse(src).unwrap();
        assert_eq!(program.routines.len(), 1);
        let r = &program.routines[0];
        assert_eq!(r.sig.params.len(), 2, "both sig params must survive");
        let SigParamKind::Tape {
            writes, preserves, ..
        } = &r.sig.params[0].kind
        else {
            panic!("expected a tape param");
        };
        assert!(writes.is_some(), "the writes clause must survive");
        assert!(preserves.is_some(), "the preserves clause must survive");
        assert_eq!(r.states.len(), 1, "the entry state must survive");
    }
}
