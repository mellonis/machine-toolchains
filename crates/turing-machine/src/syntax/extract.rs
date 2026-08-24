//! The retokenization bridge (docs/core.md (syntax trees)): turning a
//! green subtree's own tokens back into real lexer [`Token`]s, so
//! extraction can hand them to the parser's OWN productions
//! (`crate::parser::reparse_transition`/`reparse_binding_arg`/
//! `reparse_sym_map`/`reparse_sig_param`/`reparse_doc_items`) instead of
//! re-deriving their grammar decisions from the tree shape. The
//! assembly half — walking views to build [`crate::parser::Program`]
//! itself — is a later change; this module holds only the bridge.

use mtc_core::syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextLineIndex};

use super::kinds::TmcKind;
use crate::lexer::{GLYPH_ESCAPES, Token, TokenKind, normalize_doc_payload};

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
///
/// `#[allow(dead_code)]`: exercised today only by this module's own
/// fidelity tests (`#[cfg(test)] mod tests`) — the assembly half that
/// walks views and calls this as real (non-test) code is a later
/// change, not yet written, so a plain `cargo build`/`clippy` of the
/// library target sees no caller outside `#[cfg(test)]`. The four
/// functions this one transitively calls — `is_trivia`,
/// `token_from_syntax`, and `token_from_syntax`'s own callees
/// `sigil_len` and `decode_glyph_body` — need no allow of their own:
/// rustc's dead-code pass treats an allow-marked item as a live root,
/// so anything reachable through it counts as used too. That is a
/// statement about this specific call graph, not a general rule —
/// a fifth function added later without a call path from here would
/// still need its own allow.
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use mtc_core::syntax::AstNode;
    use proptest::prelude::*;

    use super::*;
    use crate::cst::{DocRunKind, TopKind, WorldKind};
    use crate::lexer::{LexMode, lex, lex_with};
    use crate::parser::{
        BindingValue, Program, SigParamKind, Transition, lower_cst, parse_cst, parse_green,
        reparse_binding_arg, reparse_doc_items, reparse_sig_param, reparse_sym_map,
        reparse_transition,
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
    /// through `Parser::transition` reproduces the exact C1
    /// `Transition`, across every shape the production itself branches
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
    #[test]
    fn reparsed_transition_equals_the_c1_transition_across_every_reachable_shape() {
        let src = "routine r() {\n  entry state a {\n    \
                   ['0'] -> goto a;\n    ['1'] -> a;\n    ['2'] -> return;\n    \
                   ['3'] -> stop;\n    ['4'] -> halt;\n    \
                   ['5'] -> call sub(t = a) then halt;\n  }\n}\n";

        let cst = parse_cst(&lex(src).unwrap()).unwrap();
        let TopKind::Reuse(reuse) = &cst.items[0].kind else {
            panic!("expected a routine");
        };
        let WorldKind::State(state) = &reuse.items[0].kind else {
            panic!("expected a state");
        };
        let c1_transitions: Vec<Transition> = state
            .rules
            .iter()
            .map(|ri| {
                let crate::cst::RuleKind::Rule(rule_cst) = &ri.kind else {
                    panic!("expected a rule");
                };
                rule_cst.rule.transition.clone()
            })
            .collect();
        assert_eq!(
            c1_transitions.len(),
            6,
            "fixture must carry exactly six rules, one per reachable shape"
        );
        // Pin the SHAPE each fixture rule actually produced, independent
        // of the green-side comparison below — a fixture that silently
        // drifted (e.g. a `goto` that stopped being `explicit: true`)
        // would otherwise still "pass" by comparing two equally-wrong
        // sides against each other.
        assert!(matches!(
            c1_transitions[0],
            Transition::Goto { explicit: true, .. }
        ));
        assert!(matches!(
            c1_transitions[1],
            Transition::Goto {
                explicit: false,
                ..
            }
        ));
        assert!(matches!(c1_transitions[2], Transition::Return { .. }));
        assert!(matches!(c1_transitions[3], Transition::Stop { .. }));
        assert!(matches!(c1_transitions[4], Transition::Halt { .. }));
        assert!(matches!(c1_transitions[5], Transition::Call { .. }));

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

        assert_eq!(green_transitions, c1_transitions);
    }

    /// Retokenizing a GRAFT's own BINDING_ARG node and reparsing it
    /// through `Parser::binding_arg` reproduces the exact C1
    /// `BindingArg`, `with map { … }` included — the unit
    /// `crate::syntax::kinds`'s own module doc names as what a caller
    /// "retokenizes and hands back to `Parser::binding_arg`".
    #[test]
    fn reparsed_binding_arg_equals_the_c1_binding_arg() {
        let src = "routine r() {\n  entry state a {\n    [*] -> stop;\n  }\n  \
                   graft a(x = y with map { '0'->'1' }) as inst;\n}\n";

        let program = lower_cst(&parse_cst(&lex(src).unwrap()).unwrap());
        let c1_arg = program.routines[0].grafts[0].args[0].clone();

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

        assert_eq!(green_arg, c1_arg);
    }

    /// Retokenizing a BINDING_ARG's own SYM_MAP node and reparsing it
    /// through `Parser::sym_map` reproduces the exact C1 `SymMap`, both
    /// arrow flavors included.
    #[test]
    fn reparsed_sym_map_equals_the_c1_sym_map() {
        let src = "routine r() {\n  entry state a {\n    [*] -> stop;\n  }\n  \
                   graft a(x = y with map { '0'->'1', '2'=>'3' }) as inst;\n}\n";

        let program = lower_cst(&parse_cst(&lex(src).unwrap()).unwrap());
        let BindingValue::Named { map, .. } = &program.routines[0].grafts[0].args[0].value else {
            panic!("expected a Named binding value");
        };
        let c1_map = map.clone().expect("binding arg carries a map");

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

        assert_eq!(green_map, c1_map);
    }

    /// Retokenizing a REUSE's own SIG_PARAM node and reparsing it
    /// through `Parser::sig_param` reproduces the exact C1 `SigParam`,
    /// across BOTH shapes the production itself branches on: `Tape`
    /// (`writes`/`preserves` clauses included) and the plain `State`
    /// parameter — a fixture with only the `Tape` shape would leave
    /// the `State` arm of `Parser::sig_param` entirely unpinned.
    #[test]
    fn reparsed_sig_param_equals_the_c1_sig_param_for_both_shapes() {
        let src = "routine r(tape t: ab writes { '0' } preserves { '1' }, state s) {\n  \
                   entry state a {\n    [*] -> stop;\n  }\n}\n";

        let program = lower_cst(&parse_cst(&lex(src).unwrap()).unwrap());
        let c1_params = program.routines[0].sig.params.clone();
        assert_eq!(c1_params.len(), 2, "fixture must carry both shapes");
        assert!(matches!(c1_params[0].kind, SigParamKind::Tape { .. }));
        assert!(matches!(c1_params[1].kind, SigParamKind::State));

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

        assert_eq!(green_params, c1_params);
    }

    /// Retokenizing an ALPHABET's own bound DOC_RUN and reparsing it
    /// through `Parser::doc_run` reproduces the exact C1 `Vec<DocRunItem>`
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
    /// exercises that arithmetic at all. Its result, `AttrCst.span`,
    /// carries no assertion of its own — coverage comes transitively
    /// from the whole-value `assert_eq!(green_doc_run, c1_doc_run)`
    /// below, which compares every field, span included.
    #[test]
    fn reparsed_doc_items_equal_the_c1_doc_run() {
        let src = "? doc line\n! [deprecated] gone\nalphabet ab { '0' }\n";

        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let TopKind::Alphabet(alphabet) = &cst.items[0].kind else {
            panic!("expected an alphabet");
        };
        let c1_doc_run = alphabet.doc_run.clone();
        assert!(
            !c1_doc_run.is_empty(),
            "fixture must actually bind a doc run, or this test proves nothing"
        );
        assert!(
            !c1_doc_run
                .iter()
                .any(|i| matches!(i.kind, DocRunKind::Comment(_))),
            "this fixture must stay comment-free — a comment-interleaved run is a \
             different case, covered separately below"
        );
        assert!(
            c1_doc_run.iter().any(|i| matches!(
                &i.kind,
                DocRunKind::Attention {
                    attr: Some(a),
                    ..
                } if a.name == "deprecated"
            )),
            "fixture must actually carry a [deprecated] attribute, or the `len` \
             arithmetic `parse_attr` depends on is never exercised"
        );

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let TopView::Alphabet(alphabet_view) = root.items().next().expect("one item") else {
            panic!("expected an ALPHABET");
        };
        let doc_run_view = alphabet_view.doc_run().expect("alphabet carries a doc run");
        // `0`: this run is the file's very first construct, so the real
        // parse's own `prev_end_line` was still its initial `0` too —
        // see `reparsed_doc_items_agree_with_lower_cst_when_the_run_abuts_a_preceding_declaration`
        // below for the case where that is NOT `0` and the caller must
        // supply the real value.
        let green_doc_run = reparse_doc_items(&sig_tokens(doc_run_view.syntax(), &index), 0);

        assert_eq!(green_doc_run, c1_doc_run);
    }

    /// A comment interleaved inside a `DOC_RUN` (`(Doc, Comment,
    /// Attention)` on the C1 side) cannot survive retokenization —
    /// `sig_tokens` drops it as trivia, so the green side's raw
    /// `Vec<DocRunItem>` is strictly SHORTER than C1's. What a caller
    /// actually needs is `reduce_doc_run` equality, not raw item
    /// equality — this proves that holds anyway, because
    /// `DocRunKind::Comment` is fully inert in `.tmc`'s own
    /// `reduce_doc_run` regardless of position (`DocRunKind::Comment(_)
    /// => {}` — no paragraph split, no attention/`deprecated` effect):
    /// dropping the comment can never change the reduced [`Doc`].
    /// Mirrors the PM sibling's own
    /// `reparsed_doc_items_reduce_to_the_same_fndoc_when_comments_interleave`.
    #[test]
    fn reparsed_doc_items_reduce_to_the_same_doc_when_comments_interleave() {
        let src = "? doc line\n// interleaved comment\n? more doc\nalphabet ab { '0' }\n";

        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let TopKind::Alphabet(alphabet) = &cst.items[0].kind else {
            panic!("expected an alphabet");
        };
        assert!(
            alphabet
                .doc_run
                .iter()
                .any(|i| matches!(i.kind, DocRunKind::Comment(_))),
            "fixture must actually interleave a comment, or this test proves nothing: {:?}",
            alphabet.doc_run
        );
        let c1_doc = crate::parser::reduce_doc_run(&alphabet.doc_run);

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
            green_doc_run.len() < alphabet.doc_run.len(),
            "the comment must actually be dropped (strictly shorter, not merely \
             different), or this test proves nothing"
        );
        let green_doc = crate::parser::reduce_doc_run(&green_doc_run);

        assert_eq!(green_doc, c1_doc);
    }

    /// `reparse_doc_items`'s `prev_end_line` parameter agrees with
    /// `lower_cst` on the FIRST item's `blank_before`, the one field
    /// the two comment-free tests above cannot exercise: both sit at
    /// the very start of the file, where "the real preceding-line
    /// value" and "an isolated slice's own fresh start" both happen to
    /// be `0`, hiding a real divergence when they are NOT the same
    /// value. This fixture puts the doc run immediately after (no
    /// blank line) a PRECEDING top-level declaration, so the two
    /// values genuinely differ, and passes the real one — read off the
    /// tree the same way a caller (extraction) would, from the
    /// preceding sibling's own end line — proving full agreement, item
    /// for item and field for field, including `blank_before` on the
    /// first item.
    #[test]
    fn reparsed_doc_items_agree_with_lower_cst_when_the_run_abuts_a_preceding_declaration() {
        let src = "alphabet a { '0' }\n? doc line\n! [deprecated] gone\nalphabet b { '0' }\n";

        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let TopKind::Alphabet(second) = &cst.items[1].kind else {
            panic!("expected the second item to be an alphabet");
        };
        let c1_doc_run = second.doc_run.clone();
        assert!(
            !c1_doc_run.is_empty() && !c1_doc_run[0].blank_before,
            "fixture must actually abut the preceding declaration (no blank line \
             before the run's first item), or this test proves nothing about the \
             divergence it exists to catch: {:?}",
            c1_doc_run
        );

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

        assert_eq!(green_doc_run, c1_doc_run);
    }

    /// Sanity check on the fixtures above: `lower_cst` and the green
    /// reparse shims are exercised over CONSTRUCTS the parser actually
    /// accepts, not hypothetical shapes — a smoke test that the whole
    /// crate's own `Program` type still round-trips through `parse` on
    /// one of them AND that the fixture's every declared feature actually
    /// landed (a wrong implementation dropping, say, the `preserves`
    /// clause or the second sig param would still leave `routines.len()
    /// == 1`, so that alone proves nothing).
    #[test]
    fn fixture_smoke_check_parses_to_a_program() {
        let src = "routine r(tape t: ab writes { '0' } preserves { '1' }, state s) {\n  \
                   entry state a {\n    [*] -> stop;\n  }\n}\n";
        let program: Program = crate::parser::parse(&lex(src).unwrap()).unwrap();
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
