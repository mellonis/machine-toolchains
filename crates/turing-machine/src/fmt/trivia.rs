//! Trivia re-derived from the `.tmc` green tree — the half of the
//! formatter's input that carries no parsed value: which comments are
//! items of a container's own stream, which ride a declaration's last
//! line, which sit on an opening brace, which were written inside a
//! comma-separated list, and where the author left a blank line
//! (docs/tmt/fmt.md (blank lines), docs/tmt/fmt.md (comments)).
//!
//! # Where a comment lives, and what claims it
//!
//! Trivia flushes into the current node before a child opens, so a
//! comment written after a declaration's terminator is the next SIBLING
//! token in the container's stream, never a child of the declaration it
//! follows (this crate's `syntax` module doc). Three things can claim
//! one, and the container walk decides between them in one pass:
//!
//! - **A node's trailing comment** — the first comment after a
//!   declaration, on that declaration's own last line. Exactly one, and
//!   only ever after a NODE; a second comment on the same line, or one
//!   after a standalone comment, is an item of its own.
//! - **The open run** — the comments riding a `{`, which belong to the
//!   node that owns the brace: the declaration itself for ALPHABET,
//!   NAMESPACE and STATE, the interposed WORLD for MACHINE and REUSE.
//! - **An item of the container's stream** — everything else, printed
//!   at the block's indent on a line of its own.
//!
//! # The near edge of a gap
//!
//! `blank_before` is `line(unit) > near_edge + 1`, where the near edge
//! walks forward with the stream: the container's opener to begin with,
//! then each unit's own last line. That is the old parser's
//! `prev_end_line` arithmetic, reproduced rather than approximated,
//! because two shapes make the local alternative — counting newlines in
//! the whitespace immediately before the unit — print differently:
//!
//! - A `;`'s trailing comment does NOT move the near edge (the `}` twin
//!   does), so a multi-line one leaves the edge on the `;`.
//! - A comment written before a declaration's `{` prints as the body's
//!   first item and moves the near edge to its own line, which can sit
//!   ABOVE the brace — the source of the one blank line the formatter
//!   emits that nobody wrote.
//!
//! Every rule here was measured against the printer this module has to
//! stay byte-identical to; the tests below name each shape.

// Every primitive here is trivia for the green printer, which takes
// them one surface at a time; until the last surface is wired, the ones
// still waiting are reachable only from this module's own tests.
#![allow(dead_code)]

use mtc_core::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, TextLineIndex};

use crate::lexer::{Comment, CommentKind};
use crate::syntax::TmcKind;

/// One item of a container's stream: a declaration or a standalone
/// comment, the blank line written before it, and the comment riding
/// its last line.
pub(crate) struct Unit {
    pub blank_before: bool,
    pub kind: UnitKind,
    pub trailing: Option<Comment>,
}

pub(crate) enum UnitKind {
    Comment(Comment),
    Node(SyntaxNode),
}

// ---------------------------------------------------------------------------
// Token-level primitives
// ---------------------------------------------------------------------------

fn is_whitespace(kind: SyntaxKind) -> bool {
    kind == TmcKind::Whitespace.into()
}

fn is_comment(kind: SyntaxKind) -> bool {
    kind == TmcKind::LineComment.into() || kind == TmcKind::BlockComment.into()
}

/// The 1-based line a token starts on, and the one it ends on. A green
/// token's `end` is one past its last character and no significant or
/// comment token ends in a newline, so `line_col(end)` names the token's
/// LAST line — which for a multi-line block comment is not its first.
fn start_line(index: &TextLineIndex, t: &SyntaxToken) -> u32 {
    index.line_col(t.text_range().start).0
}

fn end_line(index: &TextLineIndex, t: &SyntaxToken) -> u32 {
    index.line_col(t.text_range().end).0
}

/// One green comment token → the [`Comment`] the lexer would have built
/// for the same source. Not a second decoder: a comment has no decoded
/// payload — `Comment::text` is documented as the verbatim source text,
/// delimiters included, which is exactly the token's own text, and the
/// kind is the delimiter pair. Only `own_line` is derived, because it is
/// the one field that is contextual rather than a property of the token
/// (and therefore the one `syntax::extract`'s converter has no arm for).
/// Pinned against `lex_with` itself by
/// `tests::comment_values_match_the_lexers_own`.
fn comment_from(t: &SyntaxToken) -> Comment {
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
        Some(SyntaxElement::Token(p)) if is_whitespace(p.kind()) => {
            p.text().contains('\n') || p.prev_sibling_or_token().is_none()
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// A container's item stream
// ---------------------------------------------------------------------------

/// The comments riding a node's opening brace, in source order.
///
/// **Ask the node that owns the brace.** For ALPHABET, NAMESPACE and
/// STATE that is the declaration; for MACHINE and REUSE the braces
/// belong to the interposed WORLD, and asking the declaration yields an
/// empty vector rather than an error.
///
/// The run ends at the end of the brace's own LINE, not at the next
/// newline: a multi-line block comment riding the `{` is part of the
/// run, and anything after it has started a later line even with no
/// newline in between, so it is already a body item.
pub(crate) fn open_trailing(brace_owner: &SyntaxNode, index: &TextLineIndex) -> Vec<Comment> {
    let elems: Vec<SyntaxElement> = brace_owner.children_with_tokens().collect();
    match open_run(&elems, index) {
        Some((_, _, run)) => run.iter().map(comment_from).collect(),
        None => Vec::new(),
    }
}

/// The first `L_BRACE` child's position, the position the body's own
/// elements start at, and the comment tokens riding that brace. `None`
/// when the container has no brace of its own — ROOT, whose items begin
/// at its first element.
fn open_run(
    elems: &[SyntaxElement],
    index: &TextLineIndex,
) -> Option<(usize, usize, Vec<SyntaxToken>)> {
    let brace = elems
        .iter()
        .position(|e| e.kind() == TmcKind::LBrace.into())?;
    let brace_line = index.line_col(elems[brace].text_range().start).0;
    let mut body = brace + 1;
    let mut run = Vec::new();
    for (j, e) in elems.iter().enumerate().skip(brace + 1) {
        match e {
            SyntaxElement::Token(t) if is_whitespace(t.kind()) => {}
            SyntaxElement::Token(t)
                if is_comment(t.kind()) && start_line(index, t) == brace_line =>
            {
                run.push(t.clone());
                body = j + 1;
            }
            _ => break,
        }
    }
    Some((brace, body, run))
}

/// A container's items, in source order — the file (ROOT), a
/// `namespace` body, a `machine`/`routine`/`graph` body (the WORLD), or
/// a state's rule list.
///
/// The walk is one pass with a near edge that moves as it goes; see the
/// module doc for what claims each comment and why the edge is tracked
/// rather than read off the whitespace before each item.
pub(crate) fn units(container: &SyntaxNode, index: &TextLineIndex) -> Vec<Unit> {
    let elems: Vec<SyntaxElement> = container.children_with_tokens().collect();
    let (head_end, body_start, mut near_edge) = match open_run(&elems, index) {
        // ROOT has no opener at all, and the parser seeds its own
        // `prev_end_line` at zero — so a file whose first item sits on
        // line 2 or later leads with a blank line.
        None => (0, 0, 0),
        Some((brace, body, run)) => {
            let edge = match run.last() {
                Some(last) => end_line(index, last),
                None => index.line_col(elems[brace].text_range().start).0,
            };
            (brace, body, edge)
        }
    };

    let mut out: Vec<Unit> = Vec::new();
    let push_comment = |out: &mut Vec<Unit>, near_edge: &mut u32, t: &SyntaxToken| {
        out.push(Unit {
            blank_before: start_line(index, t) > *near_edge + 1,
            kind: UnitKind::Comment(comment_from(t)),
            trailing: None,
        });
        *near_edge = end_line(index, t);
    };

    // Comments written BEFORE the brace — between a declaration's
    // keyword and its name, or after an `entry` prefix. No production
    // holds them, so they lead the body.
    for e in &elems[..head_end] {
        if let SyntaxElement::Token(t) = e
            && is_comment(t.kind())
        {
            push_comment(&mut out, &mut near_edge, t);
        }
    }

    let mut i = body_start;
    while i < elems.len() {
        match &elems[i] {
            SyntaxElement::Token(t) if is_comment(t.kind()) => {
                push_comment(&mut out, &mut near_edge, t);
                i += 1;
            }
            // Whitespace, and the closing brace: neither is an item.
            SyntaxElement::Token(_) => i += 1,
            SyntaxElement::Node(n) => {
                let first = n
                    .first_token()
                    .expect("a container's item node carries at least one token");
                let last = n
                    .last_token()
                    .expect("a container's item node carries at least one token");
                let blank_before = start_line(index, &first) > near_edge + 1;
                let (trailing, next) = claim_trailing(&elems, i + 1, index, end_line(index, &last));
                // A `}`-terminated declaration's trailing comment moves
                // the near edge past itself; a `;`-terminated one's does
                // not. That asymmetry is not about the token — it is
                // which capture helper the old parser ran, and the two
                // partition the declaration kinds exactly.
                near_edge = match &trailing {
                    Some((_, comment_end)) if last.kind() == TmcKind::RBrace.into() => *comment_end,
                    _ => end_line(index, &last),
                };
                out.push(Unit {
                    blank_before,
                    kind: UnitKind::Node(n.clone()),
                    trailing: trailing.map(|(c, _)| c),
                });
                i = next;
            }
        }
    }
    out
}

/// The one comment that rides a node's last line, with its own end line
/// and the position the container walk resumes at. Scans past whitespace
/// only: the first comment decides, and a comment that has already
/// started a later line is an item instead.
fn claim_trailing(
    elems: &[SyntaxElement],
    from: usize,
    index: &TextLineIndex,
    node_last_line: u32,
) -> (Option<(Comment, u32)>, usize) {
    for (j, e) in elems.iter().enumerate().skip(from) {
        match e {
            SyntaxElement::Token(t) if is_whitespace(t.kind()) => {}
            SyntaxElement::Token(t) if is_comment(t.kind()) => {
                return if start_line(index, t) == node_last_line {
                    (Some((comment_from(t), end_line(index, t))), j + 1)
                } else {
                    (None, from)
                };
            }
            _ => break,
        }
    }
    (None, from)
}

/// The gap between a declaration's bound doc run and the declaration
/// itself — the second, smaller question a documented item asks, once
/// its own `blank_before` has been spent on the gap before the RUN
/// (this crate's `syntax` module doc: a declaration retro-wraps its doc
/// run, so the node starts at the run).
///
/// Measured at the declaration's first significant token, not right
/// after the DOC_RUN node: an ordinary comment written between the run's
/// last line and the keyword is part of the run as the old parser reads
/// it, but sits OUTSIDE the DOC_RUN node in the tree. `false` when the
/// declaration carries no run at all, which is also when the printer
/// never asks.
pub(crate) fn blank_before_decl(node: &SyntaxNode) -> bool {
    let elems: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let header = elems.iter().position(|e| match e {
        SyntaxElement::Token(t) => !is_whitespace(t.kind()) && !is_comment(t.kind()),
        SyntaxElement::Node(_) => false,
    });
    match header {
        Some(i) if i > 0 => match &elems[i - 1] {
            SyntaxElement::Token(t) if is_whitespace(t.kind()) => {
                t.text().matches('\n').count() >= 2
            }
            _ => false,
        },
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Comma-separated lists
// ---------------------------------------------------------------------------

/// The comments written inside one comma-separated list, each keyed to
/// the entry it sits against (docs/tmt/fmt.md (comments inside a list)).
///
/// The key is **entries STARTED so far**, never a comma count: a comment
/// after the last of `n` entries has `n-1` commas before it and must key
/// to `n`, which is how it finds a home before the closer.
///
/// No depth tracking, and none needed: every nestable construct a list
/// can hold — a `with map`, a signature clause, a bracketed vector — is
/// its own NODE in this tree, so a comma at this level is always a
/// separator. The one bare exception is a `{expr}` substitution cell,
/// whose braces are plain tokens; it can hold no comma either.
pub(crate) fn interior(elems: impl Iterator<Item = SyntaxElement>) -> Vec<(usize, Comment)> {
    let mut out = Vec::new();
    let mut entries_started = 0usize;
    let mut in_entry = false;
    for e in elems {
        match &e {
            SyntaxElement::Token(t) if is_whitespace(t.kind()) => {}
            SyntaxElement::Token(t) if is_comment(t.kind()) => {
                out.push((entries_started, comment_from(t)));
            }
            SyntaxElement::Token(t) if t.kind() == TmcKind::Comma.into() => in_entry = false,
            _ => {
                if !in_entry {
                    entries_started += 1;
                    in_entry = true;
                }
            }
        }
    }
    out
}

/// A node's direct children strictly between its first `{` and its last
/// `}` — the element stream [`interior`] reads for an `alphabet` body or
/// a `with map` pair list.
pub(crate) fn between_braces(node: &SyntaxNode) -> impl Iterator<Item = SyntaxElement> {
    between(node, TmcKind::LBrace, TmcKind::RBrace)
}

/// The `[`/`]` twin, for a rule's pattern and its `write`/`move`
/// vectors.
pub(crate) fn between_brackets(node: &SyntaxNode) -> impl Iterator<Item = SyntaxElement> {
    between(node, TmcKind::LBracket, TmcKind::RBracket)
}

fn between(
    node: &SyntaxNode,
    open: TmcKind,
    close: TmcKind,
) -> impl Iterator<Item = SyntaxElement> {
    let elems: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let from = elems
        .iter()
        .position(|e| e.kind() == open.into())
        .map_or(elems.len(), |i| i + 1);
    let to = elems
        .iter()
        .rposition(|e| e.kind() == close.into())
        .unwrap_or(elems.len());
    elems.into_iter().take(to).skip(from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};
    use crate::parser::parse_green_from_tokens;
    use crate::syntax::TmcKind;
    use mtc_core::syntax::SyntaxNode;

    fn tree(src: &str) -> (SyntaxNode, TextLineIndex) {
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let green = parse_green_from_tokens(src, &tokens).expect("parses");
        (SyntaxNode::new_root(green), TextLineIndex::new(src))
    }

    /// An own-line comment is a unit of its own; a same-line comment after a
    /// node rides that node. The two are told apart by the whitespace before
    /// the comment, never by a line number computed twice.
    #[test]
    fn own_line_comments_are_units_and_same_line_comments_are_trailing() {
        let src = "// standalone\nuse a::b; // trailing\n";
        let (root, index) = tree(src);
        let units = units(&root, &index);
        assert_eq!(units.len(), 2);
        assert!(matches!(&units[0].kind, UnitKind::Comment(c) if c.text == "// standalone"));
        assert!(units[0].trailing.is_none());
        assert!(matches!(&units[1].kind, UnitKind::Node(n) if n.kind() == TmcKind::Use.into()));
        assert_eq!(
            units[1].trailing.as_ref().map(|c| c.text.as_str()),
            Some("// trailing")
        );
    }

    /// A blank line is presence, not a count: two blank lines and three
    /// report the same, because the printer collapses any run to one.
    #[test]
    fn blank_before_is_presence_not_a_count() {
        for gap in ["\n\n", "\n\n\n", "\n\n\n\n"] {
            let src = format!("use a::b;{gap}use c::d;\n");
            let (root, index) = tree(&src);
            let units = units(&root, &index);
            assert!(units[1].blank_before, "gap {gap:?} must read as blank");
        }
        let (root, index) = tree("use a::b;\nuse c::d;\n");
        assert!(!units(&root, &index)[1].blank_before);
    }

    /// A documented declaration's unit starts at its retro-wrapped DOC_RUN,
    /// so ONE rule answers what C1 needed `leads_with_blank` to branch for.
    /// All seven doc-run kinds retro-wrap on this language — including
    /// `namespace`, which is where the PM sibling's rule does NOT apply.
    #[test]
    fn a_documented_declarations_unit_starts_at_its_doc_run() {
        let src = "use a::b;\n\n? doc\nnamespace n {\n}\n";
        let (root, index) = tree(src);
        let units = units(&root, &index);
        assert!(
            units[1].blank_before,
            "the blank sits before the DOC_RUN, and the unit starts there"
        );
        let UnitKind::Node(ns) = &units[1].kind else {
            panic!("expected a node")
        };
        assert_eq!(ns.kind(), TmcKind::Namespace.into());
        assert!(
            !blank_before_decl(ns),
            "no blank between the run and `namespace`"
        );

        let src = "use a::b;\n? doc\n\nnamespace n {\n}\n";
        let (root, index) = tree(src);
        // `super::` because the binding above already shadows the name.
        let units = super::units(&root, &index);
        assert!(!units[1].blank_before);
        let UnitKind::Node(ns) = &units[1].kind else {
            panic!("expected a node")
        };
        assert!(
            blank_before_decl(ns),
            "the run→declaration gap is its own query"
        );
    }

    /// Brace comments live in two different streams: the declaration's own
    /// for ALPHABET/NAMESPACE/STATE, the interposed WORLD's for
    /// MACHINE/REUSE. One primitive, two nodes to ask — asking the wrong one
    /// silently yields an empty vector, which is why this test names both.
    #[test]
    fn open_trailing_comes_from_the_node_that_owns_the_brace() {
        let src = "alphabet ab { // on the brace\n  '_'\n}\n";
        let (root, index) = tree(src);
        let alphabet = root
            .children()
            .find(|n| n.kind() == TmcKind::Alphabet.into())
            .expect("an ALPHABET");
        let comments = open_trailing(&alphabet, &index);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].text, "// on the brace");

        let src = "alphabet ab { '_' }\nmachine { // on the brace\n  tape t: ab;\n}\n";
        let (root, index) = tree(src);
        let machine = root
            .children()
            .find(|n| n.kind() == TmcKind::Machine.into())
            .expect("a MACHINE");
        assert!(
            open_trailing(&machine, &index).is_empty(),
            "MACHINE does not own its brace — WORLD does"
        );
        let world = machine
            .children()
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        assert_eq!(open_trailing(&world, &index)[0].text, "// on the brace");
    }

    /// Interior attribution counts ENTRIES STARTED, never commas: a comment
    /// after the last entry has `n-1` commas before it and must key to `n`.
    #[test]
    fn interior_keys_a_trailing_comment_to_the_entry_count() {
        let src = "alphabet ab {\n  // zero\n  '_', // one\n  'a'\n  // two\n}\n";
        let (root, _index) = tree(src);
        let alphabet = root
            .children()
            .find(|n| n.kind() == TmcKind::Alphabet.into())
            .expect("an ALPHABET");
        let elems = between_braces(&alphabet);
        let found = interior(elems);
        let keys: Vec<usize> = found.iter().map(|(i, _)| *i).collect();
        assert_eq!(keys, vec![0, 1, 2], "two entries, so the last key is 2");
    }

    /// A `{expr}` substitution cell carries braces but never a comma, so a
    /// flat scan over a vector's elements needs no depth tracking. Pinned
    /// because the alternative — assuming it — is exactly the assumption a
    /// nested map would break.
    #[test]
    fn a_substitution_cell_does_not_confuse_the_entry_scan() {
        let src = "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s {\n    [*] -> write [/* c */ {0 + 1}] stop;\n  }\n}\n";
        let (root, _index) = tree(src);
        let vec_node = descendants(&root)
            .find(|n| n.kind() == TmcKind::WriteVec.into())
            .expect("a WRITE_VEC");
        let found = interior(between_brackets(&vec_node));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 0, "one cell, comment before it");
    }

    // -----------------------------------------------------------------
    // Measured shapes the simplest plausible rules get wrong. Each was
    // read off `crate::fmt::format`, the printer this module has to keep
    // byte-identical, before it was written down here.
    // -----------------------------------------------------------------

    /// A trailing comment is claimed exactly once, and only by a NODE.
    /// C1's `take_trailing`/`capture_close_trailing` fire once each right
    /// after a declaration's terminator; every later comment falls to the
    /// pending drain and becomes an item of its own — even one written on
    /// the same physical line. So "no newline before it ⇒ trailing" is
    /// wrong twice over, and both halves print differently.
    #[test]
    fn a_trailing_comment_is_claimed_once_and_only_by_a_node() {
        let (root, index) = tree("use a::b; /* one */ /* two */\n");
        let after_semi = units(&root, &index);
        assert_eq!(
            after_semi.len(),
            2,
            "the second comment is not a second trailing"
        );
        assert_eq!(
            after_semi[0].trailing.as_ref().map(|c| c.text.as_str()),
            Some("/* one */")
        );
        assert!(matches!(&after_semi[1].kind, UnitKind::Comment(c) if c.text == "/* two */"));

        let (root, index) = tree("/* a */ /* b */\nuse a::b;\n");
        let chained = units(&root, &index);
        assert_eq!(chained.len(), 3, "a comment never carries a trailing");
        assert!(chained.iter().all(|u| u.trailing.is_none()));
    }

    /// The gap after a `;`-terminated declaration is measured from the `;`,
    /// not from the comment riding it — `take_trailing` is the one capture
    /// helper that does not advance C1's `prev_end_line`, while the `}`
    /// twin `capture_close_trailing` does. The two fixtures differ only in
    /// the terminator, and only a MULTI-LINE trailing comment can show it.
    #[test]
    fn the_gap_after_a_trailing_comment_is_measured_from_the_terminator() {
        let (root, index) = tree("use a::b; /* one\ntwo */\n/* three */\n");
        let semi = units(&root, &index);
        assert!(
            semi[1].blank_before,
            "the `;` line, not the comment's end line, is the gap's near edge"
        );

        let (root, index) = tree("alphabet ab { '_' } /* one\ntwo */\n/* three */\n");
        let braced = units(&root, &index);
        assert!(
            !braced[1].blank_before,
            "a `}}`'s trailing comment DOES advance the near edge"
        );
    }

    /// The brace's own run stops at the end of the brace's LINE, which is
    /// not the same as "up to the next newline": a multi-line block comment
    /// riding the `{` carries the run past that line, and whatever follows
    /// it — with no newline in between — is already a body item.
    #[test]
    fn open_trailing_stops_at_the_end_of_the_braces_own_line() {
        let src = "machine { /* a\nb */ /* c */\n  tape t: ab;\n}\n";
        let (root, index) = tree(src);
        let world = descendants(&root)
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        let open = open_trailing(&world, &index);
        assert_eq!(
            open.len(),
            1,
            "only the comment that STARTS on the `{{` line"
        );
        assert_eq!(open[0].text, "/* a\nb */");

        let units = units(&world, &index);
        assert!(
            matches!(&units[0].kind, UnitKind::Comment(c) if c.text == "/* c */"),
            "the comment past the brace line is the body's first item"
        );
    }

    /// A container's FIRST unit still has a gap to measure, and the near
    /// edge differs by container. A braced body measures against the `{`'s
    /// own line, so one newline is no blank; ROOT has no opener at all and
    /// C1 seeds it at line zero, so a file whose first item sits on line 2
    /// or later leads with a blank. (`flush` ignores the first item's flag,
    /// so this is fidelity rather than visible output — which is exactly
    /// why it would rot unwatched.)
    #[test]
    fn a_containers_first_unit_is_gapped_against_its_own_opener() {
        let (root, index) = tree("use a::b;\n");
        assert!(!units(&root, &index)[0].blank_before);
        let (root, index) = tree("\nuse a::b;\n");
        assert!(
            units(&root, &index)[0].blank_before,
            "ROOT's near edge is the virtual line 0"
        );

        for (src, expect) in [
            ("machine {\n  tape t: ab;\n}\n", false),
            ("machine {\n\n  tape t: ab;\n}\n", true),
            ("machine { tape t: ab;\n}\n", false),
        ] {
            let (root, index) = tree(src);
            let world = descendants(&root)
                .find(|n| n.kind() == TmcKind::World.into())
                .expect("a WORLD");
            assert_eq!(units(&world, &index)[0].blank_before, expect, "for:\n{src}");
        }
    }

    /// A comment written before a declaration's `{` — between its keyword
    /// and its name, or after an `entry` prefix — belongs to no production
    /// in C1, so it falls to the body's pending drain and prints as the
    /// body's FIRST item (docs/tmt/fmt.md (comments)). It also moves the
    /// near edge to its own end line, which can sit ABOVE the brace: that
    /// is where the printer's one unwritten blank line comes from, and
    /// reproducing it is a plan constraint, not a bug to fix.
    #[test]
    fn a_comment_before_the_brace_is_the_bodys_first_unit() {
        let src = "machine {\n  tape t: ab;\n  state /* mid */ s {\n    [*] -> stop;\n  }\n}\n";
        let (root, index) = tree(src);
        let state = descendants(&root)
            .find(|n| n.kind() == TmcKind::State.into())
            .expect("a STATE");
        let mid = units(&state, &index);
        assert!(
            matches!(&mid[0].kind, UnitKind::Comment(c) if c.text == "/* mid */"),
            "the pre-brace comment leads the body"
        );
        assert!(!mid[0].blank_before);
        assert!(
            !mid[1].blank_before,
            "the rule sits one line below the comment's own line"
        );

        let src =
            "machine {\n  tape t: ab;\n  entry // why\n  state s {\n    [*] -> stop;\n  }\n}\n";
        let (root, index) = tree(src);
        let state = descendants(&root)
            .find(|n| n.kind() == TmcKind::State.into())
            .expect("a STATE");
        let prefixed = units(&state, &index);
        assert!(matches!(&prefixed[0].kind, UnitKind::Comment(c) if c.text == "// why"));
        assert!(
            prefixed[1].blank_before,
            "the comment's line is ABOVE the brace's, so the rule reads two lines away"
        );
    }

    /// An ordinary comment written between a doc run's last line and the
    /// keyword it documents belongs to the RUN as the old parser reads it,
    /// but sits outside the DOC_RUN node in the tree — so the run→
    /// declaration gap is measured at the keyword, never at the node's own
    /// end. The two fixtures put the blank line on opposite sides of that
    /// comment; only the one written directly above the keyword counts.
    #[test]
    fn a_comment_after_the_doc_run_moves_the_run_to_declaration_gap() {
        for (src, expect) in [
            ("? doc\n/* c */\nalphabet ab { '_' }\n", false),
            ("? doc\n/* c */\n\nalphabet ab { '_' }\n", true),
            ("? doc\n\n/* c */\nalphabet ab { '_' }\n", false),
        ] {
            let (root, _index) = tree(src);
            let alphabet = root
                .children()
                .find(|n| n.kind() == TmcKind::Alphabet.into())
                .expect("an ALPHABET");
            assert_eq!(blank_before_decl(&alphabet), expect, "for:\n{src}");
        }
    }

    /// Every `Comment` this module hands the printer must be the one the
    /// lexer would have built for the same source — `text` verbatim, `kind`
    /// from the delimiter, and `own_line` true iff nothing but whitespace
    /// preceded it on its physical line. `own_line` is the one that cannot
    /// be read off the token itself, and the one the printer reads to
    /// decide whether a list can stay inline; the leading-indent fixture is
    /// the case a "previous whitespace holds a newline" rule alone gets
    /// wrong.
    #[test]
    fn comment_values_match_the_lexers_own() {
        for src in [
            "// first\nuse a::b; // trailing\n/* own */ /* riding */\n",
            "  // indented at the start of the file\nuse a::b;\n",
            "/* a */use a::b;/* b */\n",
            "alphabet ab { // brace\n  /* own */\n  '_', /* after */ 'a'\n} // close\n",
            "machine {\n  tape t: ab; /* multi\n  line */\n}\n",
        ] {
            let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
            let expected: Vec<_> = tokens
                .iter()
                .filter_map(|t| match &t.kind {
                    crate::lexer::TokenKind::Comment(c) => Some(c.clone()),
                    _ => None,
                })
                .collect();
            assert!(!expected.is_empty(), "fixture must carry comments: {src}");

            let (root, _index) = tree(src);
            let found: Vec<_> = root
                .descendant_tokens()
                .filter(|t| is_comment(t.kind()))
                .map(|t| comment_from(&t))
                .collect();
            assert_eq!(found, expected, "for:\n{src}");
        }
    }

    /// Every node of `root`'s subtree, document order. A local walk: the
    /// core red tree exposes `children`/`descendant_tokens` but no
    /// descendant-NODE iterator, and this crate cannot add one.
    fn descendants(root: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> {
        let mut stack: Vec<SyntaxNode> = root.children().collect();
        stack.reverse();
        std::iter::from_fn(move || {
            let n = stack.pop()?;
            let mut kids: Vec<SyntaxNode> = n.children().collect();
            kids.reverse();
            stack.extend(kids);
            Some(n)
        })
    }
}
