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
//! # Two comments a plain sibling walk cannot see
//!
//! Both were measured against the printer this module has to stay
//! byte-identical to, and each one is a DROPPED comment if unhandled.
//!
//! - **Between a `machine`/`routine`/`graph` header and its `{`.** WORLD
//!   opens AT the brace, so `machine /* x */ {` leaves that comment in
//!   the DECLARATION's stream, one level up. The old parser body-drains
//!   it and prints it as the body's first item; [`pre_world_comments`]
//!   reaches back for it.
//! - **Inside a `;`-terminated declaration.** `tape main /* c */: ab;`
//!   prints as `tape main: ab; /* c */`, because the old parser's
//!   `take_trailing` looks at whatever is still PENDING when it reaches
//!   the `;` — and a comment written inside the statement is pending
//!   there. [`unclaimed_inside`] names, per node kind, which of a node's
//!   own comments are still pending at its terminator.
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

use mtc_core::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, TextLineIndex};

use crate::lexer::Comment;
use crate::syntax::TmcKind;
// One converter, shared: a doc run's own items carry `Comment` VALUES,
// so `syntax::extract` needs the same green-token → `Comment` mapping
// this module does, and `syntax` cannot depend on `fmt`. Pinned against
// the lexer here, by `tests::comment_values_match_the_lexers_own`.
use crate::syntax::extract::comment_from;

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
///
/// **A comment written BEFORE the `{` suppresses the whole run** — see
/// [`open_run`].
pub(crate) fn open_trailing(brace_owner: &SyntaxNode, index: &TextLineIndex) -> Vec<Comment> {
    let elems: Vec<SyntaxElement> = brace_owner.children_with_tokens().collect();
    match open_run(brace_owner, &elems, index) {
        Some((_, _, run)) => run.iter().map(comment_from).collect(),
        None => Vec::new(),
    }
}

/// True for a direct child token that is neither whitespace nor a
/// comment — the position a declaration's own header starts at.
fn is_significant(e: &SyntaxElement) -> bool {
    match e {
        SyntaxElement::Token(t) => !is_whitespace(t.kind()) && !is_comment(t.kind()),
        SyntaxElement::Node(_) => false,
    }
}

/// The comments written between a declaration's header and its `{`.
///
/// The scan starts at the header's first significant token, not at the
/// node, because a comment written above that keyword belongs to the
/// bound DOC_RUN as the old parser reads it (`crate::syntax::extract`'s
/// `doc_run_tokens`) — the same position [`blank_before_decl`] measures
/// its gap at, so the two agree by construction. For MACHINE and REUSE
/// the `{` is WORLD's, so `node` is the WORLD, its own `elems[..brace]`
/// is empty, and the answer comes from [`pre_world_comments`] one level
/// up: exactly one of the two halves is ever non-empty.
fn pre_brace_comments(
    node: &SyntaxNode,
    elems: &[SyntaxElement],
    brace: usize,
) -> Vec<SyntaxToken> {
    let head_start = elems[..brace]
        .iter()
        .position(is_significant)
        .unwrap_or(brace);
    let mut out: Vec<SyntaxToken> = elems[head_start..brace]
        .iter()
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if is_comment(t.kind()) => Some(t.clone()),
            _ => None,
        })
        .collect();
    out.extend(pre_world_comments(node));
    out
}

/// The first `L_BRACE` child's position, the position the body's own
/// elements start at, and the comment tokens riding that brace. `None`
/// when the container has no brace of its own — ROOT, whose items begin
/// at its first element.
///
/// # A pre-brace comment suppresses the run entirely
///
/// The old parser's `capture_open_trailing` pops from a GLOBAL comment
/// cursor and keeps only comments whose next significant token is past
/// the `{` it has just consumed. A comment written BEFORE the brace is
/// still pending, sits at the head of that cursor, and fails that test —
/// so the loop breaks on its FIRST iteration and the run comes back
/// EMPTY, however many comments ride the brace's own line. All of them
/// fall to the body's drain instead, printed as items in source order
/// behind the pre-brace one, and the near edge stays on the brace.
///
/// Measured, on every brace owner: `machine /* x */ { // open` prints
/// `/* x */` and `// open` as the first two body items, not `// open` on
/// the brace line. The `namespace`, `state` and `routine`/`graph` twins
/// behave identically, and an `alphabet`'s pair goes to slot 0 of its
/// element list rather than to the body.
fn open_run(
    node: &SyntaxNode,
    elems: &[SyntaxElement],
    index: &TextLineIndex,
) -> Option<(usize, usize, Vec<SyntaxToken>)> {
    let brace = elems
        .iter()
        .position(|e| e.kind() == TmcKind::LBrace.into())?;
    if !pre_brace_comments(node, elems, brace).is_empty() {
        return Some((brace, brace + 1, Vec::new()));
    }
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

/// The comments written between a `machine`/`routine`/`graph` header
/// and the `{` its WORLD opens at — empty for every other container.
///
/// **The scan runs BACKWARDS from the WORLD and stops at the first
/// significant token**, which is what keeps it disjoint from the
/// declaration's own lists. A forward scan from the declaring keyword
/// would look right and be wrong: a comment written anywhere in a
/// REUSE's HEADER — `routine /* c */ r(tape t: ab)` — is already slot
/// 0 of the SIGNATURE's interior list, because the printer's
/// `delimited_interior` folds everything from a list's first
/// significant token up to its `(` into that slot, and printing it here
/// as well would print it twice. Backwards-to-the-first-significant
/// token is exactly "past the last list": the `)` for a REUSE, the
/// `machine` keyword for a MACHINE.
fn pre_world_comments(world: &SyntaxNode) -> Vec<SyntaxToken> {
    if world.kind() != TmcKind::World.into() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = world.prev_sibling_or_token();
    while let Some(SyntaxElement::Token(t)) = cur {
        if is_comment(t.kind()) {
            out.push(t.clone());
        } else if !is_whitespace(t.kind()) {
            break;
        }
        cur = t.prev_sibling_or_token();
    }
    out.reverse();
    out
}

/// Where a rule's own interior lists end, as source offsets — the
/// partition that decides which of a RULE's comments belongs to which
/// claimant.
///
/// The claimants tile the rule's text with no gaps: each list claims
/// every comment at or before its own end, so a comment's region is
/// decided by which boundary it falls short of. A comment starting
/// in:
///
/// | region | is claimed by |
/// |---|---|
/// | `[node.start, pattern_end)` | the pattern's interior list |
/// | `[pattern_end, write_end)` | the `write` vector's list |
/// | `[write_end, move_end)` | the `move` vector's list |
/// | `[move_end, pending)` | the `call` binding list, or one of its maps |
/// | `[pending, node.end)` | nothing — still pending at the `;` |
///
/// A vector's region therefore holds not only what was written between
/// its brackets but also the HEADER ahead of it — `write /* c */ [` and
/// `-> /* c */ write [` both land in the write vector's list
/// (docs/tmt/fmt.md (comments inside a list); this crate's `fmt` module
/// doc). An absent vector collapses its region to nothing by taking the
/// previous boundary as its own end.
pub(crate) struct RuleRegions {
    pub pattern_end: u32,
    pub write_end: u32,
    pub move_end: u32,
    pub pending: u32,
}

/// [`RuleRegions`] for one RULE node. `node` must be a RULE; asking any
/// other kind answers a degenerate partition, not an error.
pub(crate) fn rule_regions(node: &SyntaxNode) -> RuleRegions {
    // The pattern is not a node, so its `]` is one of RULE's own direct
    // children — and the only one, since each glyph vector's brackets
    // belong to its own WRITE_VEC/MOVE_VEC node.
    let pattern_end = node
        .children_with_tokens()
        .find(|e| e.kind() == TmcKind::RBracket.into())
        .map_or(node.text_range().start, |e| e.text_range().end);
    let vec_end = |kind: TmcKind, fallback: u32| {
        node.children()
            .find(|n| n.kind() == kind.into())
            .map_or(fallback, |n| n.text_range().end)
    };
    let write_end = vec_end(TmcKind::WriteVec, pattern_end);
    let move_end = vec_end(TmcKind::MoveVec, write_end);
    // A `call`'s binding list drains immediately before its `)`, which
    // is a direct child of TRANSITION (the arguments are BINDING_ARG
    // nodes, and a nested `with map` uses braces) — so the LAST `)`
    // there is the rule's last drain. Every other transition runs none.
    let pending = node
        .children()
        .find(|n| n.kind() == TmcKind::Transition.into())
        .and_then(|t| {
            t.children_with_tokens()
                .filter(|e| e.kind() == TmcKind::RParen.into())
                .map(|e| e.text_range().end)
                .last()
        })
        .unwrap_or(move_end);
    RuleRegions {
        pattern_end,
        write_end,
        move_end,
        pending,
    }
}

/// The comment tokens inside `node` that no interior list has claimed
/// by the time its terminator is reached — the ones the trailing-comment
/// slot and then the container's own item stream get to claim, in that
/// order.
///
/// A comment is unclaimed exactly while no list (or doc run) has taken
/// it. So this is a per-KIND question, and the chain below is
/// deliberately accounted for in prose rather than left to a silent
/// default:
///
/// - **TAPE** — no doc run, no list, so every comment inside it is
///   unclaimed. `tape /* c */ main: ab;` prints `tape main: ab; /* c */`.
/// - **GRAFT / BIND** — the binding list reaches the `)` and claims
///   everything at or before it, so only what is written AFTER the `)`
///   (around an `as NAME`) is left; the scan below starts at the LAST
///   `)` for exactly that reason.
/// - **USE** — its list reaches the `;`, so nothing inside it is ever
///   left over; the empty answer here is a fact about the shape of a
///   `use` declaration, not a gap.
/// - **RULE** — `;`-terminated like the three above, and it carries
///   several interior lists of its own: the pattern's, each glyph
///   vector's, and — for a `call` — its binding list's and every nested
///   map's. [`RuleRegions`] names where the last of them ends, and
///   everything past that offset is left over: `[*] -> stop /* c */;`
///   prints as `[*] -> stop; /* c */`, and so does the same comment
///   written anywhere in a `call`'s tail (`) /* c */ then`,
///   `then stop /* c */`). That region can open one level down, inside
///   the TRANSITION node, which is why this arm alone walks DESCENDANT
///   tokens rather than direct children.
/// - **Everything else** is `}`-terminated, and answers empty by
///   design: a trailing comment on such a declaration has to sit AFTER
///   the `}`, where the container's own sibling walk already sees it.
///   Comments written INSIDE one are not lost either — they belong to
///   its body items, to its head scan ([`pre_brace_comments`]) or to
///   one of its lists.
fn unclaimed_inside(node: &SyntaxNode) -> Vec<SyntaxToken> {
    let comments = |elems: &[SyntaxElement]| -> Vec<SyntaxToken> {
        elems
            .iter()
            .filter_map(|e| match e {
                SyntaxElement::Token(t) if is_comment(t.kind()) => Some(t.clone()),
                _ => None,
            })
            .collect()
    };
    let elems: Vec<SyntaxElement> = node.children_with_tokens().collect();
    if node.kind() == TmcKind::Tape.into() {
        comments(&elems)
    } else if node.kind() == TmcKind::Graft.into() || node.kind() == TmcKind::Bind.into() {
        let after = elems
            .iter()
            .rposition(|e| e.kind() == TmcKind::RParen.into())
            .map_or(elems.len(), |i| i + 1);
        comments(&elems[after..])
    } else if node.kind() == TmcKind::Rule.into() {
        // Not `elems`: a rule's pending region can begin INSIDE its
        // TRANSITION (`call r(…) /* c */ then stop`), one level down.
        let from = rule_regions(node).pending;
        node.descendant_tokens()
            .filter(|t| is_comment(t.kind()) && t.text_range().start >= from)
            .collect()
    } else {
        Vec::new()
    }
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
    let (head_end, body_start, mut near_edge) = match open_run(container, &elems, index) {
        // ROOT has no opener at all, so there is no `{` line to gap
        // the first item against; the near edge starts at line zero,
        // which is what makes a file whose first item sits on line 2 or
        // later lead with a blank line.
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

    // Comments written before the brace. The slot this walk is FOR is
    // the header — between the declaring keyword (or an `entry`/
    // `export` prefix) and the `{`, one level up for a WORLD. No
    // production holds a comment there, so the old parser's body drain
    // took it and it printed as the body's first item
    // ([`pre_brace_comments`] explains where each half lives, and why
    // the scan starts at the keyword rather than at the node).
    //
    // These lead the body, and the near edge starts on the `{`, never
    // past an open run: [`open_run`] returns an EMPTY run the moment
    // `pre_brace_comments` is non-empty, before it looks at the brace
    // line at all. The two are therefore never both non-empty by
    // construction, and the order between them cannot be observed.
    for t in pre_brace_comments(container, &elems, head_end) {
        push_comment(&mut out, &mut near_edge, &t);
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
                let node_last_line = end_line(index, &last);
                // A comment still pending INSIDE the node is ahead of
                // everything the container's own stream holds, and the
                // old parser's `take_trailing` inspects only the FIRST
                // pending one — so when there is an inside comment, no
                // comment written after the `;` can be the trailing,
                // whichever line it sits on.
                let inside = unclaimed_inside(n);
                let (trailing, next, taken) = match inside.first() {
                    None => {
                        let (trailing, next) = claim_trailing(&elems, i + 1, index, node_last_line);
                        (trailing, next, 0)
                    }
                    Some(t)
                        if !comment_from(t).own_line && start_line(index, t) == node_last_line =>
                    {
                        (Some((comment_from(t), end_line(index, t))), i + 1, 1)
                    }
                    Some(_) => (None, i + 1, 0),
                };
                // A `}`-terminated declaration's trailing comment moves
                // the near edge past itself; a `;`-terminated one's does
                // not. That asymmetry is not about the token — it is
                // which capture helper the old parser ran, and the two
                // partition the declaration kinds exactly.
                near_edge = match &trailing {
                    Some((_, comment_end)) if last.kind() == TmcKind::RBrace.into() => *comment_end,
                    _ => node_last_line,
                };
                out.push(Unit {
                    blank_before,
                    kind: UnitKind::Node(n.clone()),
                    trailing: trailing.map(|(c, _)| c),
                });
                // Whatever the trailing did not take drains next, as
                // items of the container — before any comment written
                // after the terminator, which is where they already sit
                // in source order.
                for t in &inside[taken..] {
                    push_comment(&mut out, &mut near_edge, t);
                }
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

/// A node's direct children strictly between its first `[` and its last
/// `]` — the element stream [`interior`] reads for a rule's pattern and
/// for each of its `write`/`move` vectors.
///
/// The `{`/`}` twin this once had beside it is GONE: every brace-
/// delimited list (an `alphabet` body, a `with map` pair list) reaches
/// its own interior through the printer's `delimited_interior`, which
/// has to slice the delimiters itself anyway — it folds in the
/// declaration's header ahead of the `{` and skips whatever the brace's
/// open run already claimed, neither of which a plain between-the-braces
/// slice expresses.
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
    /// so ONE rule answers what the retired parser needed a separate
    /// `leads_with_blank` branch for.
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
    ///
    /// Driven through a glyph vector rather than an `alphabet` body, which
    /// is where this was first measured: [`interior`] takes an iterator and
    /// is delimiter-agnostic, and [`between_brackets`] is the one slicer a
    /// production caller actually hands it — the brace surfaces build their
    /// own stream in the printer, header half included. The same 0/1/2
    /// keying is pinned on an `alphabet` body through printed BYTES by
    /// `fmt::print`'s `interior_list_comments_agree_on_every_surface`.
    #[test]
    fn interior_keys_a_trailing_comment_to_the_entry_count() {
        let src = "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s {\n    \
                   [*, *] -> write [\n      // zero\n      '_', // one\n      'a'\n      \
                   // two\n    ] stop;\n  }\n}\n";
        let (root, _index) = tree(src);
        let vec_node = descendants(&root)
            .find(|n| n.kind() == TmcKind::WriteVec.into())
            .expect("a WRITE_VEC");
        let found = interior(between_brackets(&vec_node));
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

    /// A trailing comment is claimed exactly once, and only by a NODE:
    /// [`claim_trailing`] fires once, right after a declaration's
    /// terminator, and every later comment becomes an item of the
    /// container's own stream — even one written on the same physical
    /// line. So "no newline before it ⇒ trailing" is wrong twice over,
    /// and both halves print differently.
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

        // The same-line boundary itself: the comment nearest a
        // declaration is its trailing only when it was written on that
        // declaration's OWN last line. Drop that test and this comment is
        // hoisted onto the `use`'s line — a silently relocated comment,
        // which is what whitespace-only exists to forbid.
        let (root, index) = tree("use a::b;\n// own line\nuse c::d;\n");
        let separated = units(&root, &index);
        assert_eq!(
            separated.len(),
            3,
            "an own-line comment is an item of its own, not the previous \
             declaration's trailing"
        );
        assert!(separated[0].trailing.is_none());
        assert!(matches!(&separated[1].kind, UnitKind::Comment(c) if c.text == "// own line"));
        assert!(
            !separated[2].blank_before,
            "and the gap after it is measured from the comment, not the `;`"
        );
    }

    /// The gap after a `;`-terminated declaration is measured from the `;`,
    /// not from the comment riding it: [`units`] advances the near edge
    /// past a trailing comment only when the declaration's last token is
    /// a `}`. The two fixtures differ only in the terminator, and only a
    /// MULTI-LINE trailing comment can show it.
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
    /// [`units`] seeds its near edge at line zero, so a file whose first
    /// item sits on line 2 or later leads with a blank. (`flush` ignores the first item's flag,
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
    /// and its name, or after an `entry` prefix — belongs to no
    /// production of the grammar, so [`pre_brace_comments`] leads the
    /// body with it and it prints as the body's FIRST item
    /// (docs/tmt/fmt.md (comments)). It also moves the
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

    /// A comment written between a `machine`/`routine`/`graph` header and
    /// its `{` is a child of the DECLARATION — WORLD opens at the brace —
    /// so the head scan that catches the NAMESPACE/STATE twin cannot see
    /// it. It still leads the body, and it still moves the near edge onto
    /// its own line, which for a comment written ABOVE the brace is where
    /// the printer's unwritten blank line comes from.
    ///
    /// The REUSE case is the one that says why the scan runs BACKWARDS:
    /// `routine /* c */ r(…)` — a comment earlier in the header — belongs
    /// to the signature's interior list, and the scan must stop at the
    /// `)` rather than sweep the whole header.
    #[test]
    fn a_comment_before_a_worlds_brace_is_the_bodys_first_unit() {
        let src = "machine /* x */ {\n  tape t: ab;\n}\n";
        let (root, index) = tree(src);
        let world = descendants(&root)
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        let items = units(&world, &index);
        assert!(
            matches!(&items[0].kind, UnitKind::Comment(c) if c.text == "/* x */"),
            "the pre-brace comment leads the body"
        );
        assert!(!items[1].blank_before);

        let src = "machine // why\n{\n  tape t: ab;\n}\n";
        let (root, index) = tree(src);
        let world = descendants(&root)
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        let above = units(&world, &index);
        assert!(matches!(&above[0].kind, UnitKind::Comment(c) if c.text == "// why"));
        assert!(
            above[1].blank_before,
            "the comment's line is ABOVE the brace's, so the tape reads two lines away"
        );

        let src = "routine r(tape /* sig */ t: ab) /* body */ {\n  state s {\n  }\n}\n";
        let (root, index) = tree(src);
        let world = descendants(&root)
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        let reuse = units(&world, &index);
        assert!(
            matches!(&reuse[0].kind, UnitKind::Comment(c) if c.text == "/* body */"),
            "only what is written past the signature's `)` reaches the body"
        );
        assert!(
            !reuse
                .iter()
                .any(|u| matches!(&u.kind, UnitKind::Comment(c) if c.text == "/* sig */")),
            "a comment inside the signature belongs to the signature's own list"
        );
    }

    /// A comment written BEFORE the `{` suppresses the open run
    /// ENTIRELY: `capture_open_trailing` pops from a global cursor and
    /// keeps only comments past the brace, so the still-pending
    /// pre-brace one sits at the head of that cursor, fails the test on
    /// the loop's first iteration, and takes nothing with it. Every
    /// brace-line comment then falls to the body's drain instead, and
    /// the near edge stays on the `{`.
    ///
    /// The rule lives in `open_run`, so it holds for every brace owner.
    /// The two named here are the ones the green printer cannot yet put
    /// through its differential harness: STATE renders with a later
    /// surface, and an ALPHABET's pre-brace pair lands in its element
    /// list's interior, later still. Asserted directly so neither waits
    /// for the surface that would otherwise be the first to notice.
    #[test]
    fn a_pre_brace_comment_suppresses_the_open_run() {
        let src =
            "machine {\n  tape t: ab;\n  state /* x */ s { // open\n    [*] -> stop;\n  }\n}\n";
        let (root, index) = tree(src);
        let state = descendants(&root)
            .find(|n| n.kind() == TmcKind::State.into())
            .expect("a STATE");
        assert!(
            open_trailing(&state, &index).is_empty(),
            "a pending pre-brace comment takes the whole run with it"
        );
        let items = units(&state, &index);
        assert!(matches!(&items[0].kind, UnitKind::Comment(c) if c.text == "/* x */"));
        assert!(
            matches!(&items[1].kind, UnitKind::Comment(c) if c.text == "// open"),
            "the brace-line comment is a body item, not part of the run"
        );
        assert!(!items[1].blank_before);

        let src = "alphabet ab /* x */ { // open\n  '_'\n}\n";
        let (root, index) = tree(src);
        let alphabet = root
            .children()
            .find(|n| n.kind() == TmcKind::Alphabet.into())
            .expect("an ALPHABET");
        assert!(open_trailing(&alphabet, &index).is_empty());

        let src = "machine /* x */ { // open\n  tape t: ab;\n}\n";
        let (root, index) = tree(src);
        let world = descendants(&root)
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        assert!(
            open_trailing(&world, &index).is_empty(),
            "the WORLD twin, whose pre-brace comment lives one level up"
        );
    }

    /// A comment written INSIDE a `;`-terminated declaration is still
    /// PENDING when the old parser reaches the `;`, so `take_trailing`
    /// relocates it to after the terminator — and what it declines
    /// (another line, an own-line comment, a second comment) drains as an
    /// item of the container instead, ahead of anything written after the
    /// `;`.
    #[test]
    fn a_comment_inside_a_semicolon_declaration_is_relocated_past_it() {
        let (root, index) = tree("machine {\n  tape main /* c */: ab;\n}\n");
        let world = descendants(&root)
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        let taken = units(&world, &index);
        assert_eq!(taken.len(), 1, "the comment rides the declaration");
        assert_eq!(
            taken[0].trailing.as_ref().map(|c| c.text.as_str()),
            Some("/* c */")
        );

        let (root, index) = tree("machine {\n  tape main /* a */: ab; /* b */\n}\n");
        let world = descendants(&root)
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        let both = units(&world, &index);
        assert_eq!(
            both[0].trailing.as_ref().map(|c| c.text.as_str()),
            Some("/* a */"),
            "the INSIDE comment is ahead of the container's own stream"
        );
        assert!(
            matches!(&both[1].kind, UnitKind::Comment(c) if c.text == "/* b */"),
            "so the one after the `;` is an item, not a second trailing"
        );

        // Declined on the line test, and declined on `own_line` — both
        // fall through to the container's drain.
        for src in [
            "machine {\n  tape main /* c */\n    : ab;\n}\n",
            "machine {\n  tape main\n    /* c */: ab;\n}\n",
        ] {
            let (root, index) = tree(src);
            let world = descendants(&root)
                .find(|n| n.kind() == TmcKind::World.into())
                .expect("a WORLD");
            let declined = units(&world, &index);
            assert!(declined[0].trailing.is_none(), "for:\n{src}");
            assert!(
                matches!(&declined[1].kind, UnitKind::Comment(c) if c.text == "/* c */"),
                "for:\n{src}"
            );
        }

        // A GRAFT claims everything at or before its binding list's `)`
        // into that list's interior, so only the tail region is pending.
        let (root, index) =
            tree("machine {\n  entry graft n::g(/* in list */ t = main) /* after */ as inst;\n}\n");
        let world = descendants(&root)
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        let graft = units(&world, &index);
        assert_eq!(
            graft[0].trailing.as_ref().map(|c| c.text.as_str()),
            Some("/* after */")
        );
        assert_eq!(graft.len(), 1, "the in-list comment is not a unit here");
    }

    /// A rule's own drains partition its comments: what one of its
    /// interior lists claims, and what is still PENDING when the walk
    /// reaches the `;`. This test asserts the pending half directly.
    ///
    /// The claimed half used to be asserted here too, off a
    /// `rule_vector_comments` helper that walked the tree a second time.
    /// That helper is gone: the printer decides a rule's off-grid status
    /// from the very buckets its row renderer prints with
    /// (`fmt::print`'s `breaks_the_grid`), deliberately rather than from
    /// a second walk, so a rival tree-walking answer was drift waiting to
    /// happen. Every boundary it covered is now pinned on printed BYTES —
    /// a vector's header half by `a_glyph_vectors_header_comment_belongs_to_that_vector`,
    /// the pattern's by `the_grid_measures_a_pattern_with_its_spliced_comment`,
    /// the call and map edges by `the_binding_list_boundaries_agree` — and
    /// the predicate's own boundary by
    /// `a_line_or_own_line_comment_in_a_glyph_vector_takes_the_rule_off_the_grid`.
    ///
    /// Each row below still moves ONE comment across one boundary; the
    /// `vectors` column names, in prose, which side it lands on, and the
    /// assertion checks the pending side answers accordingly:
    ///
    /// - inside the pattern's brackets vs. just past its `]`.
    /// - inside a `write` vector vs. between it and the following
    ///   `move` (still the move vector's, by the header rule) vs. past
    ///   the last vector with no `move` written at all.
    /// - inside a `call`'s parens, or inside a nested `with map`, vs.
    ///   past the `)` — and, the case a `move_end`-only cut gets wrong,
    ///   BETWEEN the last vector and the call's `(`, which the binding
    ///   list's own drain sweeps up.
    #[test]
    fn a_rules_comments_are_partitioned_by_its_own_drains() {
        let head = "alphabet ab { '_', 'a' }\nnamespace n {\n  routine r(tape t: ab) {\n    \
                    entry state q {\n      [*] -> stop;\n    }\n  }\n}\nmachine {\n  \
                    tape main: ab;\n  entry state s {\n    ";
        // (rule text, comments the vectors claim, comments pending at `;`)
        for (rule, _claimed_by_a_list, pending) in [
            ("[*] -> stop /* c */;", vec![], vec!["/* c */"]),
            ("[*] /* c */ -> stop;", vec![], vec!["/* c */"]),
            ("[/* c */ *] -> stop;", vec!["/* c */"], vec![]),
            ("[*] -> write /* c */ ['a'] stop;", vec!["/* c */"], vec![]),
            ("[*] -> write ['a'] /* c */ stop;", vec![], vec!["/* c */"]),
            (
                "[*] -> write ['a'] /* c */ move [>] stop;",
                vec!["/* c */"],
                vec![],
            ),
            (
                "[*] -> call n::r(/* c */ t = main) then stop;",
                vec![],
                vec![],
            ),
            (
                "[*] -> call n::r(t = main with map { /* c */ '_' -> '_' }) then stop;",
                vec![],
                vec![],
            ),
            (
                "[*] -> move [>] /* c */ call n::r(t = main) then stop;",
                vec![],
                vec![],
            ),
            (
                "[*] -> move [/* c */ >] call n::r(t = main) then stop;",
                vec!["/* c */"],
                vec![],
            ),
            (
                "[*] -> call n::r(t = main) /* c */ then stop;",
                vec![],
                vec!["/* c */"],
            ),
            (
                "[*] -> call n::r(t = main) then stop /* c */;",
                vec![],
                vec!["/* c */"],
            ),
        ] {
            let src = format!("{head}{rule}\n  }}\n}}\n");
            let (root, _index) = tree(&src);
            // The LAST rule in the file: the fixture's `routine` filler
            // carries one of its own, ahead of the rule under test.
            let rule_node = descendants(&root)
                .filter(|n| n.kind() == TmcKind::Rule.into())
                .last()
                .expect("a RULE");
            let found: Vec<String> = unclaimed_inside(&rule_node)
                .iter()
                .map(|t| t.text().to_string())
                .collect();
            assert_eq!(found, pending, "pending comments for:\n{rule}");
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
