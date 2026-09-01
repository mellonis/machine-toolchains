//! The `.pmc` printer (`docs/pmt/fmt.md`, `docs/core.md` (syntax
//! trees)) — the whole implementation behind [`super::format`], built
//! directly on the lossless green tree. It replaced a printer that
//! walked a hand-typed CST, and was grown surface by surface against
//! that printer as a differential oracle before being cut over
//! wholesale.
//!
//! **"C1" throughout this file names that retired pair — the hand-typed
//! CST and the printer that walked it. Both are deleted, so every
//! mention of C1 below records how that implementation behaved, never
//! anything still in the tree or code to go and open.** Notes saying a
//! rule is *ported* from `render_items`, `command_column`, `print_use`
//! and the like name C1's own functions, and record that the rule
//! crossed over unchanged. This module's `tests` are the surviving
//! trace of that oracle: every expected literal in them was captured
//! from the C1 printer while it still existed, and those literals are
//! what pin the behaviour now.
//!
//! **Scope of this module today**: the whole `.pmc` language — the file
//! itself, `use` declarations (paths, aliases, and every interior
//! comment position inside the path list), namespaces (including their
//! same-line open/close-brace comments and nested/reopened namespaces),
//! functions: headers (`volatile`/`export`, doc runs), nesting, and
//! statement bodies (labels, command-column alignment, comma-group
//! layout with the greedy-fill width fallback and its own mid-group
//! interior comments), and every comment position the language admits:
//! standalone/leading comments at any level, a comment run's own
//! internal blank line, a block comment spanning lines, a comment
//! interleaved inside or immediately after a bound doc run, a
//! statement's own same-line trailing comment (relocated forward from a
//! comment written before the terminating `;`, when C1 did the same —
//! [`statement_trailing_and_leftovers`]'s own doc), the column-alignment
//! runs several such comments form together
//! ([`compute_trailing_spacing`]), a function's own open- and
//! close-brace comments, and a comma group's/use list's own interior
//! comments ([`item_leading_comments`], [`use_decl_interior_comments`]).
//! No comment position hits a coverage-stub `unreachable!` any more —
//! the ones still in this file are ordinary exhaustiveness panics for
//! grammar shapes the parser cannot produce (an unexpected node/token
//! kind at a position only USE_DECL/NAMESPACE/FUNCTION/STATEMENT/comment
//! tokens can occupy), a different category entirely from a coverage
//! stub naming a future task.
//!
//! ## Comment placement, re-derived from trivia
//!
//! The C1 CST stored a leading comment run, an open/close-brace trailing
//! comment, and a blank-line flag as fields the parser filled in. The
//! green tree carries none of that as dedicated storage — every one of
//! those decisions is re-derived here from raw sibling trivia tokens via
//! [`super::trivia`] (`docs/pmt/fmt.md` (comments)). Two shapes drive the
//! top-level walk in [`print_items`]:
//!
//! - A comment immediately (no blank line) before an item belongs to
//!   that item's leading run ([`super::trivia::leading_comments`]) and
//!   prints above it; a comment that is not part of any item's leading
//!   run is its own standalone unit, printed exactly the same way
//!   ([`print_comment`]) — content printing never distinguishes leading
//!   from standalone, only the blank-line decision does.
//! - Some comments are already owned outright by ANOTHER printer and
//!   must never be reprinted as part of an item's leading run OR as a
//!   standalone token: a namespace's same-line open-brace comment
//!   ([`super::trivia::open_trailing`], consumed by [`print_namespace`]
//!   before its interior items print, and handed down as `reserved`),
//!   and any item's own same-line trailing comment
//!   ([`super::trivia::trailing_comment`] — a namespace's close-brace
//!   comment, or a use declaration's own comment after `;`). Both sit
//!   with no blank line before whatever comes next, so
//!   `leading_comments` (which walks back through every raw token until
//!   a NODE sibling, not merely up to the previous item's own last
//!   token) picks them up as the NEXT item's leading run too —
//!   [`print_items`]' `consumed` set is exactly this "owned elsewhere"
//!   set, filtering that leading-run print; its broader `claimed` set
//!   (which also folds in every item's OWN leading run) additionally
//!   stops a raw comment token from printing standalone when the walk
//!   reaches it directly as a plain sibling.

use mtc_core::syntax::{AstNode, SyntaxElement, SyntaxNode, SyntaxToken, TextLineIndex, token};

use crate::compiler::CompileError;
use crate::lexer::{LexMode, lex_with, normalize_doc_payload};
use crate::parser::{
    Builtin, CheckArm, Item, Label, Statement, Successor, parse_green_from_tokens,
};
use crate::syntax::{
    DocRunView, FunctionView, ItemView, NamespaceView, PmcKind, StatementView, UseDeclView,
    UsePathView, extract_statement,
};

use super::trivia;

/// Spaces per block level (`docs/pmt/fmt.md` (indentation) — 4 spaces,
/// never tabs).
const INDENT_UNIT: usize = 4;

/// Line width limit (`docs/pmt/fmt.md` (comma groups) — 80 characters,
/// matching lint's `line-too-long`; char count, not bytes).
const LINE_WIDTH: usize = 80;

/// Normalizes one comment's raw trivia text for printing
/// (`docs/pmt/fmt.md` (comments)): every line's TRAILING whitespace
/// stripped, joined back with LF only. A comment token's text is raw
/// trivia, captured character-for-character from source — a line
/// comment's trailing spaces, or a `\r` immediately before the closing
/// `\n` of a CRLF source line, survive into the token verbatim unless
/// stripped here; nothing else in the pipeline ever touches comment text.
/// Only each line's END is touched: a block comment's interior LEADING
/// whitespace is preserved verbatim, so `trim_end` must not reach it.
fn normalize_comment_text(text: &str) -> String {
    text.split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `.pmc` source → canonical text (`docs/core.md` (syntax trees)).
/// Lexes `WithComments`, parses straight to the green tree, and prints
/// from typed views and raw trivia queries over it. A lex/parse error is
/// returned as `Err`, never printed (thin renderer) — [`super::format`]
/// is the public wrapper and adds nothing but its own signature.
pub(crate) fn format(source: &str) -> Result<String, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    let root = SyntaxNode::new_root(green);
    // Threaded through every print function from here down:
    // `compute_trailing_spacing`'s aligned/ragged verdict reads each
    // trailing comment's own SOURCE column (`docs/pmt/fmt.md`
    // (comments)), which only a line index can answer.
    let line_index = TextLineIndex::new(source);
    let mut out = String::new();
    print_items(
        &mut out,
        root.children_with_tokens(),
        0,
        &[],
        &[],
        &line_index,
    );
    // Edge case (`docs/pmt/fmt.md` (indentation), mirrored from C1's
    // `print_cst`): an empty or whitespace-only file still reprints as
    // exactly one newline.
    if out.is_empty() {
        out.push('\n');
    }
    #[cfg(debug_assertions)]
    assert_comment_conservation(source, &out);
    Ok(out)
}

/// The conservation gate behind every debug-build render: each comment
/// in the source reprints in the output exactly once, text intact up to
/// the per-line trailing-whitespace trim [`normalize_comment_text`] is
/// allowed (docs/pmt/fmt.md (comments) — never moved, never dropped).
/// The printer coordinates independent claimants — leading runs,
/// trailing slots, brace rides, `use`-list splices, in-place header and
/// label-region walks — by per-surface boundaries with no structural
/// guarantee that every comment is claimed exactly once, and a missed
/// surface loses a comment SILENTLY: `pmt fmt` rewrites in place, and
/// no other property can see the loss (token equivalence lexes
/// `WithoutComments`; idempotence holds on the lossy output). This
/// printer shipped exactly that failure on header-interior comments. A
/// MULTISET is compared, not a sequence, so this gate is layout-blind —
/// the never-move rule itself is pinned position-by-position in
/// `tests/comment_positions.rs`.
#[cfg(debug_assertions)]
fn assert_comment_conservation(source: &str, formatted: &str) {
    fn multiset(text: &str) -> std::collections::BTreeMap<String, usize> {
        let mut m = std::collections::BTreeMap::new();
        for t in lex_with(text, LexMode::WithComments)
            .expect("both sides lexed already: the source above, the output by idempotence tests")
        {
            if let crate::lexer::TokenKind::Comment(c) = t.kind {
                *m.entry(normalize_comment_text(&c.text)).or_insert(0usize) += 1;
            }
        }
        m
    }
    let before = multiset(source);
    let after = multiset(formatted);
    assert_eq!(
        before, after,
        "fmt dropped or duplicated a comment\n--- input ---\n{source}\n--- output ---\n{formatted}"
    );
}

/// The elements strictly between a brace-delimited node's `{` and `}` —
/// the window [`print_namespace`] hands to [`print_items`] (mirroring the
/// C1 `NamespaceCst::items` field this replaces) and [`print_body`] walks
/// directly for a `FUNCTION`'s own body, in source order.
fn brace_interior(node: &SyntaxNode) -> impl Iterator<Item = SyntaxElement> + '_ {
    node.children_with_tokens()
        .skip_while(|e| e.kind() != PmcKind::LBrace.into())
        .skip(1)
        .take_while(|e| e.kind() != PmcKind::RBrace.into())
}

/// One level's item list, at `indent` — the file level (`indent == 0`,
/// called on the FILE root's own children) and a namespace's interior
/// (one level deeper, called on [`brace_interior`]) share this walk,
/// so nesting a namespace inside a namespace recurses "for free" the
/// same way C1's `print_top_items` did.
///
/// `reserved` is the caller's own already-printed comment tokens (a
/// namespace's [`super::trivia::open_trailing`] run) — see the module
/// doc's second bullet for why the shared walk needs to know about them.
fn print_items(
    out: &mut String,
    elements: impl Iterator<Item = SyntaxElement>,
    indent: usize,
    reserved: &[SyntaxToken],
    relocated: &[SyntaxToken],
    line_index: &TextLineIndex,
) {
    let elements: Vec<SyntaxElement> = elements.filter(|e| !trivia::is_ws(e.kind())).collect();

    // Comments some OTHER printer already owns outright: the caller's own
    // reserved (open-brace) run, and each item's same-line trailing
    // comment (printed inline by the item's own printer, e.g. a
    // namespace's close-brace comment or a use declaration's own
    // trailing comment after `;`). `trivia::leading_comments` walks back
    // through every raw token until it hits a NODE sibling — not just up
    // to the previous item's own last token — so a `consumed` comment
    // like that can ALSO surface as the NEXT item's own "leading run"
    // (`docs/pmt/fmt.md` (comments)): filtering an item's leading-run
    // print by `consumed` (not merely `reserved`) is what stops it
    // reprinting a second time, above the wrong item, there.
    let mut consumed: Vec<SyntaxToken> = reserved.to_vec();
    for e in &elements {
        if let SyntaxElement::Node(n) = e {
            consumed.extend(trivia::trailing_comment(n));
        }
    }

    // The superset used only to decide whether a raw comment TOKEN
    // encountered below is already accounted for: `consumed` plus every
    // item's own leading run (printed above that item, when the walk
    // reaches the item node itself rather than the token in isolation).
    let mut claimed: Vec<SyntaxToken> = consumed.clone();
    for e in &elements {
        if let SyntaxElement::Node(n) = e {
            claimed.extend(trivia::leading_comments(n));
        }
    }

    // The caller's relocated run (a namespace's header-interior comments
    // plus, with them, its open-brace run — [`header_interior_comments`])
    // prints first, at this level's indent, blanks measured between the
    // comments' ORIGINAL source lines; `baseline` then hands the last
    // one's line to the first thing printed below, whose real sibling
    // gap (against the `{`, not against the comment that used to sit
    // before it) would answer the blank question one line too late. A
    // relocated open-brace comment is also reached below as its own raw
    // token — skipped as `claimed`, which deliberately leaves `baseline`
    // in place for whatever prints after the skip.
    let mut baseline: Option<u32> = None;
    for c in relocated {
        if let Some(prev) = baseline
            && line_index.line_col(c.text_range().start).0 > prev + 1
        {
            out.push('\n');
        }
        print_comment(out, c, indent);
        baseline = Some(line_index.line_col(c.text_range().end).0);
    }

    let mut first = relocated.is_empty();
    for e in &elements {
        match e {
            SyntaxElement::Node(node) => {
                // The `i > 0`-equivalent brace-edge suppression: the
                // first printed unit at this level never gets a forced
                // blank line, regardless of what precedes it in source.
                // The other brace edge, "immediately before `}`", cannot
                // arise — no unit follows the last one to carry a
                // blank.
                let blank = match baseline.take() {
                    Some(prev) => unit_start_line(node, line_index) > prev + 1,
                    None => trivia::blank_before_unit(node),
                };
                if !first && blank {
                    out.push('\n');
                }
                first = false;
                for c in trivia::leading_comments(node) {
                    if !consumed.contains(&c) {
                        print_comment(out, &c, indent);
                    }
                }
                print_item(out, node, indent, line_index);
            }
            SyntaxElement::Token(tok) if trivia::is_comment(tok.kind()) => {
                if claimed.contains(tok) {
                    continue;
                }
                let blank = match baseline.take() {
                    Some(prev) => line_index.line_col(tok.text_range().start).0 > prev + 1,
                    None => blank_immediately_before(tok),
                };
                if !first && blank {
                    out.push('\n');
                }
                first = false;
                print_comment(out, tok, indent);
            }
            SyntaxElement::Token(t) => unreachable!(
                "unexpected token {:?} between top-level items; only USE_DECL/NAMESPACE/FUNCTION \
                 nodes, comments, and whitespace can appear here",
                t.kind()
            ),
        }
    }
}

/// Whether an empty line sits immediately before `tok` — a standalone
/// comment's own blank-line decision. An item's decision instead comes
/// from [`super::trivia::blank_before_unit`], which looks past the
/// item's own leading comment run to the gap before the WHOLE unit; a
/// standalone comment token IS its own unit, so the immediately
/// preceding sibling is already the right place to look.
fn blank_immediately_before(tok: &SyntaxToken) -> bool {
    matches!(
        tok.prev_sibling_or_token(),
        Some(SyntaxElement::Token(t))
            if trivia::is_ws(t.kind()) && t.text().matches('\n').count() >= 2
    )
}

/// The source line a node-backed body/top-level unit's OWN print begins
/// on — past its leading comment run, if any, same "whole unit" scope as
/// [`trivia::blank_before_unit`], but a line number instead of a gap
/// verdict. [`print_body`]'s own `override_prev_line` reads this
/// (`statement_trailing_and_leftovers`'s own doc: a leftover tail
/// comment relocates C1's `prev_end_line` baseline forward to ITS OWN
/// line, so the blank-before decision for whatever follows a leftover
/// can no longer be answered by the immediate REAL sibling gap alone —
/// the leftover isn't a real sibling of what follows it, it's nested
/// inside the PREVIOUS statement).
fn unit_start_line(node: &SyntaxNode, line_index: &TextLineIndex) -> u32 {
    let lead = trivia::leading_comments(node);
    let start = match lead.first() {
        Some(first) => first.text_range().start,
        None => node.text_range().start,
    };
    line_index.line_col(start).0
}

/// The canonical header line with its interior comments spliced in
/// place — the never-move rendering a declaration header takes when a
/// comment sits between its tokens (`main /* c */ () {`,
/// `namespace /* c */ n {`). Walks the node's direct header tokens
/// from the first significant one through the opening `{`: significant
/// tokens reproduce the canonical spacing (no space before `(`/`)` or
/// after `(`, one space elsewhere), a comment gets one space before and
/// after it, and a LINE comment — which nothing can follow on its
/// physical line — continues the header on the next line at the
/// declaration's own indent. Comments BEFORE the first significant
/// token belong to [`doc_run_trailing_comments`], the same claim
/// boundary [`header_interior_comments`] holds.
fn render_header_tokens(node: &SyntaxNode, indent: usize) -> String {
    let mut out = String::new();
    let mut seen_significant = false;
    let mut after_comment = false;
    let mut at_line_start = false;
    let mut prev: Option<String> = None;
    for e in node.children_with_tokens() {
        let SyntaxElement::Token(t) = e else {
            continue; // a bound DOC_RUN — before the header by construction
        };
        if trivia::is_ws(t.kind()) {
            continue;
        }
        if trivia::is_comment(t.kind()) {
            if !seen_significant {
                continue;
            }
            if !at_line_start {
                out.push(' ');
            }
            out.push_str(&normalize_comment_text(t.text()));
            if is_line_comment(&t) {
                out.push('\n');
                out.push_str(&" ".repeat(indent));
                at_line_start = true;
            } else {
                at_line_start = false;
            }
            after_comment = !at_line_start;
            continue;
        }
        let text = t.text();
        let space = if !seen_significant || at_line_start {
            false
        } else if after_comment {
            !matches!(text, "(" | ")")
        } else {
            !matches!(text, "(" | ")") && !matches!(prev.as_deref(), Some("("))
        };
        if space {
            out.push(' ');
        }
        out.push_str(text);
        seen_significant = true;
        prev = Some(text.to_string());
        after_comment = false;
        at_line_start = false;
        if t.kind() == PmcKind::LBrace.into() {
            break;
        }
    }
    out
}

/// Comment tokens written between a declaration's header tokens — after
/// its FIRST header token and before its opening `{` — in source order:
/// between a function's name and its `(`, inside the `()`, between `)`
/// and `{`, after `volatile`/`export`, or between a namespace's
/// keyword/name and its `{`. Any such comment takes the header to
/// [`render_header_tokens`], which prints it in place (the never-move
/// rule).
///
/// The after-the-first-header-token bound is the claim boundary against
/// [`doc_run_trailing_comments`]: a comment between a bound `DOC_RUN`
/// and the header sits BEFORE the first header token and is that walk's
/// to print, never this one's. A comment before the declaration
/// entirely is a sibling OUTSIDE the node (`FUNCTION` retro-wraps only
/// its doc run; `NAMESPACE` opens at its keyword), so it cannot be
/// collected here.
fn header_interior_comments(node: &SyntaxNode) -> Vec<SyntaxToken> {
    let mut out = Vec::new();
    let mut seen_header_token = false;
    for e in node.children_with_tokens() {
        match e {
            // A bound DOC_RUN — before the header by construction.
            SyntaxElement::Node(_) => {}
            SyntaxElement::Token(t) => {
                if t.kind() == PmcKind::LBrace.into() {
                    break;
                }
                if trivia::is_comment(t.kind()) {
                    if seen_header_token {
                        out.push(t);
                    }
                } else if !trivia::is_ws(t.kind()) {
                    seen_header_token = true;
                }
            }
        }
    }
    out
}

/// Dispatch one top-level (or namespace-level) item node to its printer.
/// Every kind this plan does not yet cover hits a loud, named panic
/// instead of silently printing something wrong — the module doc's own
/// rule, and the brief's for every later plan's surface.
fn print_item(out: &mut String, node: &SyntaxNode, indent: usize, line_index: &TextLineIndex) {
    if node.kind() == PmcKind::UseDecl.into() {
        let view = UseDeclView::cast(node.clone()).expect("kind checked by the caller");
        print_use(out, &view, indent, line_index);
    } else if node.kind() == PmcKind::Namespace.into() {
        let view = NamespaceView::cast(node.clone()).expect("kind checked by the caller");
        print_namespace(out, &view, indent, line_index);
    } else if node.kind() == PmcKind::Function.into() {
        let view = FunctionView::cast(node.clone()).expect("kind checked by the caller");
        print_function(out, &view, indent, line_index);
    } else {
        unreachable!(
            "unexpected node kind {:?} at item level; only USE_DECL, NAMESPACE and FUNCTION \
             can appear here",
            node.kind()
        )
    }
}

/// `namespace NAME { … }` (`docs/pmt/fmt.md` (indentation)),
/// mirroring C1's `print_namespace` decisions: one space before
/// `{`, the closing `}` alone at the header's own indent, and its
/// `items` printed one level deeper via the same [`print_items`] walk
/// the file level uses, so nesting recurses for free. The open/close
/// same-line comments ride their brace's own line exactly like C1's
/// `open_trailing`/`close_trailing` fields — see the module doc.
fn print_namespace(
    out: &mut String,
    ns: &NamespaceView,
    indent: usize,
    line_index: &TextLineIndex,
) {
    let node = ns.syntax();
    let pad = " ".repeat(indent);
    out.push_str(&pad);
    if header_interior_comments(node).is_empty() {
        out.push_str("namespace ");
        out.push_str(&ns.name());
        out.push_str(" {");
    } else {
        // A header-interior comment prints in place, on the header line
        // (the never-move rule) — same as a function header's.
        out.push_str(&render_header_tokens(node, indent));
    }
    let brace =
        token(node, PmcKind::LBrace.into()).expect("NAMESPACE always carries an L_BRACE token");
    let open = trivia::open_trailing(&brace);
    if open.is_empty() {
        out.push('\n');
    } else {
        out.push(' ');
        let texts: Vec<String> = open
            .iter()
            .map(|c| normalize_comment_text(c.text()))
            .collect();
        out.push_str(&texts.join(" "));
        out.push('\n');
    }
    print_items(
        out,
        brace_interior(node),
        indent + INDENT_UNIT,
        &open,
        &[],
        line_index,
    );
    out.push_str(&pad);
    out.push('}');
    if let Some(c) = trivia::trailing_comment(node) {
        out.push(' ');
        out.push_str(&normalize_comment_text(c.text()));
    }
    out.push('\n');
}

/// Every interior comment token in `node`'s own `use` list, indexed by
/// which path it precedes — the green re-derivation of C1's
/// `Parser::interior_comments`, ported for [`print_use`] to read
/// (`docs/pmt/fmt.md` (comments inside a use list)). `USE_PATH` is the
/// only node kind `USE_DECL` carries, so a forward walk of
/// `children_with_tokens` tracking how many have been seen so far IS
/// the same index C1 threaded: a comment that is a direct sibling gets
/// the CURRENT count; one nested a level down, inside a `USE_PATH`
/// itself (`std::/* c */goToEnd`), drains at the exact same point as
/// one written just after that path (C1's `Parser::interior_comments`
/// was only ever called between paths, never mid-path — a nested comment
/// simply stays pending until the drain that follows), so it takes the
/// index the path's OWN closing bumps the count to, one step ahead of
/// scanning past the path node — never a reason to guard the shape
/// away as somebody else's surface, unlike the C1-CST-field precedent
/// this reproduces (`UseCst::interior`, drained by C1's `print_use`):
/// there `Parser::interior_comments` and the mid-path drain already
/// collapsed onto that same index for the same reason, just recorded as
/// one flat `Vec<(usize, Comment)>` instead of walked live off tokens.
fn use_decl_interior_comments(node: &SyntaxNode) -> Vec<(usize, SyntaxToken)> {
    let mut out = Vec::new();
    let mut index = 0usize;
    for e in node.children_with_tokens() {
        match e {
            SyntaxElement::Node(n) if n.kind() == PmcKind::UsePath.into() => {
                index += 1;
                // A comment NESTED inside the path (`a /* c */ ::b`)
                // is not a slot comment: it prints in place inside the
                // path's own rendered text ([`render_use_path`], the
                // never-move rule).
            }
            SyntaxElement::Token(t) if trivia::is_comment(t.kind()) => out.push((index, t)),
            _ => {}
        }
    }
    out
}

/// Whether only whitespace precedes `t` on its own physical line — the
/// green re-derivation of the C1 lexer's `Comment::own_line` field
/// (`docs/pmt/fmt.md` (comments)): walk backward over sibling
/// tokens/nodes, skipping any run of same-line whitespace, until either
/// a newline-carrying whitespace token is found (own line: true), any
/// other element is found (false — real content precedes it on this
/// line, whitespace or not), or the walk runs out of siblings entirely
/// (true — nothing precedes it at all).
fn comment_own_line(t: &SyntaxToken) -> bool {
    let mut cur = t.prev_sibling_or_token();
    loop {
        match cur {
            None => return true,
            Some(SyntaxElement::Token(p)) if trivia::is_ws(p.kind()) => {
                if p.text().contains('\n') {
                    return true;
                }
                cur = p.prev_sibling_or_token();
            }
            _ => return false,
        }
    }
}

/// One `use` list (`docs/pmt/fmt.md` (spacing, comments inside a use
/// list)), mirroring C1's `print_use` decisions in full — paths
/// in source order, `::`-joined, `as`-aliased, one canonical space
/// after `use` and after each comma, AND interior comments printed in
/// place rather than relocated below the statement: a comment trailing
/// entry `i` is drained before entry `i+1` parses, so it keys to the
/// FOLLOWING index ([`use_decl_interior_comments`]), and
/// [`comment_own_line`] decides whether it rides the preceding line or
/// opens its own — ported unchanged, reading a `(usize, SyntaxToken)`
/// pair instead of C1's `(usize, Comment)`.
fn print_use(out: &mut String, u: &UseDeclView, indent: usize, _line_index: &TextLineIndex) {
    let node = u.syntax();
    let interior = use_decl_interior_comments(node);
    let rendered: Vec<String> = u.paths().map(|p| render_use_path(&p)).collect();
    out.push_str(&" ".repeat(indent));
    out.push_str("use");
    if interior.is_empty() {
        out.push(' ');
        out.push_str(&rendered.join(", "));
    } else {
        // Continuation lines align under the first path, clearing `use `.
        let cont = " ".repeat(indent + 4);
        let slot = |ix: usize, own_line: bool| -> Vec<&SyntaxToken> {
            interior
                .iter()
                .filter(move |(i, c)| *i == ix && comment_own_line(c) == own_line)
                .map(|(_, c)| c)
                .collect()
        };
        // A same-line comment directly after the `use` keyword, before the
        // first path (index 0's OWN slot — distinct from index 1's, which
        // the loop below reads as "the following index"). It rides the
        // `use` line itself; since it may be a LINE comment eating the
        // rest of that line, the first path always moves to its own line
        // when this slot is non-empty.
        let use_line_trailing = slot(0, false);
        if !use_line_trailing.is_empty() {
            // One space before EACH comment, mirroring the slot(i + 1)
            // loop below — a single space before the whole run would glue
            // adjacent block comments into `/* a *//* b */`.
            for c in &use_line_trailing {
                out.push(' ');
                out.push_str(&normalize_comment_text(c.text()));
            }
        } else if slot(0, true).is_empty() {
            // No comment rides the `use` line at all — the first path
            // follows the usual one space, same as the no-interior case.
            out.push(' ');
        }
        // Else: an own-line comment leads the first path (`slot(0, true)`
        // below); `use` itself takes no trailing space, since nothing
        // shares its line — the loop's own-line branch opens with the
        // newline that comment needs.
        for (i, path) in rendered.iter().enumerate() {
            for c in slot(i, true) {
                out.push('\n');
                out.push_str(&cont);
                out.push_str(&normalize_comment_text(c.text()));
                out.push('\n');
                out.push_str(&cont);
            }
            if (i > 0 || !use_line_trailing.is_empty()) && slot(i, true).is_empty() {
                out.push('\n');
                out.push_str(&cont);
            }
            out.push_str(path);
            if i + 1 < rendered.len() {
                out.push(',');
            }
            // The NEXT slot's same-line comments belong to THIS line: a
            // comment after `a,` is drained before `b` parses, so it keys
            // to the following index — this reads even on the LAST entry,
            // whose "next" slot (`rendered.len()`) is the tail: a comment
            // between the last path and the `;` that stayed on the path's
            // own line (docs/pmt/fmt.md (comments inside a use list)).
            for c in slot(i + 1, false) {
                out.push(' ');
                out.push_str(&normalize_comment_text(c.text()));
            }
        }
        // A tail-slot own-line comment sits on its own line before the `;`.
        let tail_own_line = slot(rendered.len(), true);
        for c in &tail_own_line {
            out.push('\n');
            out.push_str(&cont);
            out.push_str(&normalize_comment_text(c.text()));
        }
        // Either tail kind can be a LINE comment, which eats the rest of
        // its physical line — the `;` can never follow directly on that
        // line, so once ANY tail comment printed, it moves the `;` onto
        // its own continuation line instead of letting it ride the last
        // one.
        if !tail_own_line.is_empty() || !slot(rendered.len(), false).is_empty() {
            out.push('\n');
            out.push_str(&cont);
        }
    }
    out.push(';');
    if let Some(tc) = trivia::trailing_comment(node) {
        out.push(' ');
        out.push_str(&normalize_comment_text(tc.text()));
    }
    out.push('\n');
}

/// One `use`-list path (`docs/pmt/fmt.md` (spacing)): `::` tight,
/// ` as ALIAS` one space each side if present — C1's `render_use_path`
/// decision, ported to read segments/alias off [`UsePathView`] instead
/// of a parsed `UsePath`.
fn render_use_path(p: &UsePathView) -> String {
    let node = p.syntax();
    if node
        .descendant_tokens()
        .any(|t| trivia::is_comment(t.kind()))
    {
        // An interior comment prints in place inside the path (the
        // never-move rule): walk the path's own tokens, canonical `::`
        // joins, one space before a comment and one after it unless a
        // `::` follows; a LINE comment continues the path on the next
        // line at the list's own continuation indent.
        let mut s = String::new();
        let mut after_comment = false;
        let mut at_line_start = false;
        let mut prev_as = false;
        for t in node.descendant_tokens() {
            if trivia::is_ws(t.kind()) {
                continue;
            }
            if trivia::is_comment(t.kind()) {
                if !at_line_start {
                    s.push(' ');
                }
                s.push_str(&normalize_comment_text(t.text()));
                if is_line_comment(&t) {
                    s.push('\n');
                    s.push_str("    ");
                    at_line_start = true;
                } else {
                    at_line_start = false;
                }
                after_comment = !at_line_start;
                continue;
            }
            let text = t.text();
            let space = if at_line_start {
                false
            } else if after_comment {
                text != "::"
            } else {
                text == "as" || prev_as
            };
            if space {
                s.push(' ');
            }
            s.push_str(text);
            prev_as = text == "as";
            after_comment = false;
            at_line_start = false;
        }
        return s;
    }
    let mut s = p
        .segments()
        .iter()
        .map(SyntaxToken::text)
        .collect::<Vec<_>>()
        .join("::");
    if let Some(alias) = p.alias_token() {
        s.push_str(" as ");
        s.push_str(alias.text());
    }
    s
}

/// One own-line comment — leading, standalone, or a namespace's
/// open/close-brace comment reprinted by their own callers via
/// [`normalize_comment_text`] directly. Content printing is
/// IDENTICAL regardless of the comment's relationship to its neighbors;
/// only the blank-line decision made by [`print_items`]'s caller
/// differs — C1's `print_comment` rule unchanged, ported to a raw
/// [`SyntaxToken`].
fn print_comment(out: &mut String, comment: &SyntaxToken, indent: usize) {
    out.push_str(&" ".repeat(indent));
    out.push_str(&normalize_comment_text(comment.text()));
    out.push('\n');
}

/// Header + doc run + body + closing brace (`docs/pmt/fmt.md`
/// (indentation)), mirroring C1's `print_function` decisions —
/// used for both top-level and nested functions, a nested `FUNCTION`
/// being the same shape one indent level deeper. A bound doc run prints
/// via [`print_doc_run`], then any comment sitting between the run's
/// last line and the header prints the same way a standalone comment
/// does ([`doc_run_trailing_comments`] — see that function's own doc for
/// why such a comment is never one of `DOC_RUN`'s own children). Unlike
/// the C1 side, `blank_before_decl` (the gap between whichever of the
/// run's last line or its trailing comment sits closest to the header)
/// is never threaded in from the caller: it is computed locally, by that
/// same walk, since — unlike the C1 CST's `blank_before` field, spent by
/// the wrapping `TopItem`/`BodyItem` — nothing else needs that gap. The
/// blank line before the whole unit (run included) is still the
/// generic [`trivia::blank_before_unit`] the caller already applies to
/// every kind of item, top-level or body: a FUNCTION node retro-wraps
/// its own bound DOC_RUN as a child, so walking back from the FUNCTION
/// node already walks back from the run's own first line.
///
/// **Open- and close-brace comments**: a same-line comment after the
/// opening `{` (`trivia::open_trailing`) rides the header line exactly
/// like [`print_namespace`]'s own open-brace comment — mirrored down
/// into [`print_body`] as `reserved`, so the body's own leading-run scan
/// (which otherwise walks back through every raw sibling token, not
/// merely up to the previous item) never reprints it as the first body
/// element's own leading comment. A same-line comment after the closing
/// `}` (`trivia::trailing_comment` — the same query a statement's
/// trailing comment uses, see [`trivia::trailing_comment`]'s own doc for
/// why one function serves both shapes) prints here exactly like
/// [`print_namespace`]'s own close-brace comment, one canonical space
/// before it.
fn print_function(
    out: &mut String,
    func: &FunctionView,
    indent: usize,
    line_index: &TextLineIndex,
) {
    let node = func.syntax();
    if let Some(dr) = func.doc_run() {
        print_doc_run(out, &dr, indent);
        let (trailing, blank_before_header) = doc_run_trailing_comments(dr.syntax());
        for c in &trailing {
            if blank_immediately_before(c) {
                out.push('\n');
            }
            print_comment(out, c, indent);
        }
        if blank_before_header {
            out.push('\n');
        }
    }
    let pad = " ".repeat(indent);
    out.push_str(&pad);
    if header_interior_comments(node).is_empty() {
        // Fixed order — `volatile` precedes `export` when both are written
        // (mirrors `FnHeader`'s own contextual-keyword decode order).
        let header = func.header();
        if header.has_volatile {
            out.push_str("volatile ");
        }
        if header.has_export {
            out.push_str("export ");
        }
        out.push_str(header.name.text());
        out.push_str("() {");
    } else {
        // A header-interior comment prints in place, on the header line
        // (the never-move rule).
        out.push_str(&render_header_tokens(node, indent));
    }
    let brace =
        token(node, PmcKind::LBrace.into()).expect("FUNCTION always carries an L_BRACE token");
    let open = trivia::open_trailing(&brace);
    if open.is_empty() {
        out.push('\n');
    } else {
        out.push(' ');
        let texts: Vec<String> = open
            .iter()
            .map(|c| normalize_comment_text(c.text()))
            .collect();
        out.push_str(&texts.join(" "));
        out.push('\n');
    }
    print_body(out, node, indent + INDENT_UNIT, &open, &[], line_index);
    out.push_str(&pad);
    out.push('}');
    if let Some(c) = trivia::trailing_comment(node) {
        out.push(' ');
        out.push_str(&normalize_comment_text(c.text()));
    }
    out.push('\n');
}

/// Comments sitting between a bound `DOC_RUN` and its declaration's own
/// header token — real, but with no C1 CST field to port from. C1's
/// `Parser::doc_run` drained pending comments only on an iteration that
/// ALSO consumed one more `?`/`!` line first (its `for (comment, cline)
/// in self.drain_pending()` call sat at the BOTTOM of the `loop`, after
/// the `DocLine`/`AttentionLine` match arm, and was never reached once
/// the match fell to `_ => break`), so a comment after the run's LAST
/// line was captured there too, landing in the SAME `Vec<DocRunItem>`
/// as a `DocRunKind::Comment`. Green emission is a separate mechanism
/// entirely (`GreenSink::flush` is lazy: a token's
/// leading trivia is only emitted once THAT token itself is bumped, into
/// whichever node happens to be open at that moment). By the time the
/// declaration's own header token is finally bumped, `g_finish()` has
/// already closed `DOC_RUN` — confirmed directly against the green tree
/// (`debug_dump`): this comment prints as `DOC_RUN`'s own NEXT SIBLING
/// inside `FUNCTION`, never one of `DOC_RUN`'s children. The printed
/// TEXT is identical either way, since [`print_comment`] doesn't care
/// which parent walked to it.
///
/// Returns every such trailing comment in source order, plus whether a
/// blank line precedes the header itself — the whitespace immediately
/// before whichever of `DOC_RUN` or the last trailing comment sits
/// closest to it, [`print_function`]'s replacement for C1's
/// `blank_before_decl`. Each comment's OWN blank-before decision (the
/// gap before IT, not before the header) is left to the caller
/// ([`blank_immediately_before`] — the same query a standalone
/// top-level/body comment uses).
fn doc_run_trailing_comments(dr: &SyntaxNode) -> (Vec<SyntaxToken>, bool) {
    let mut out = Vec::new();
    let mut cur = dr.next_sibling_or_token();
    loop {
        match cur {
            Some(SyntaxElement::Token(t)) if trivia::is_ws(t.kind()) => {
                cur = t.next_sibling_or_token();
            }
            Some(SyntaxElement::Token(t)) if trivia::is_comment(t.kind()) => {
                cur = t.next_sibling_or_token();
                out.push(t);
            }
            Some(SyntaxElement::Token(t)) => return (out, blank_immediately_before(&t)),
            _ => unreachable!(
                "a DOC_RUN always precedes its bound declaration's own header token — a \
                 dangling run (nothing left to bind to) is DanglingDocRun, a parse error caught \
                 long before this printer ever runs"
            ),
        }
    }
}

/// A `DOC_RUN`'s own `?`/`!` lines PLUS any ordinary comment interleaved
/// between them (`docs/pmt/fmt.md` (doc and attention runs): "An
/// ordinary comment interleaved inside a run prints under the Comments
/// rule above, at the run's own indent" —
/// `comment_inside_a_doc_run_prints_under_existing_comment_rules` in
/// [`super`]'s own tests is the C1 fixture this ports), mirroring
/// C1's `print_doc_run` decisions: each at the bound declaration's
/// own `indent`, blank lines between run items collapsed to one (index
/// 0's own leading blank is the caller's `blank_before_unit` decision,
/// not this loop's — same split as the C1 side). `DOC_RUN`'s own
/// children are flat tokens (no sub-nodes: `Parser::doc_run` bumps
/// `DocLine`/`AttentionLine`/comment tokens directly into the node it
/// opens), so this walks tokens, not `children_with_tokens`-over-nodes
/// the way [`print_items`] does.
///
/// **A comment AFTER the run's last line is a different shape**, printed
/// by the caller instead ([`doc_run_trailing_comments`]) — see that
/// function's own doc for why it never reaches a `DOC_RUN`'s own
/// children, and so never reaches this loop.
fn print_doc_run(out: &mut String, dr: &DocRunView, indent: usize) {
    let pad = " ".repeat(indent);
    let tokens: Vec<SyntaxToken> = dr
        .syntax()
        .children_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if !trivia::is_ws(t.kind()) => Some(t),
            _ => None,
        })
        .collect();
    let mut first = true;
    for t in &tokens {
        if !first && blank_immediately_before(t) {
            out.push('\n');
        }
        first = false;
        if trivia::is_comment(t.kind()) {
            print_comment(out, t, indent);
        } else if t.kind() == PmcKind::DocLine.into() {
            print_doc_run_line(out, &pad, '?', &doc_line_payload(t));
        } else if t.kind() == PmcKind::AttentionLine.into() {
            print_doc_run_line(out, &pad, '!', &doc_line_payload(t));
        } else {
            unreachable!("unexpected token kind {:?} inside a doc run", t.kind())
        }
    }
}

/// A `DOC_LINE`/`ATTENTION_LINE` token's semantic payload: the raw
/// source text minus its own sigil (`?`/`!`, always one ASCII byte, but
/// sliced by `char::len_utf8` rather than a bare `1` on principle — same
/// discipline as `crate::syntax::extract`'s own `sigil_len`), then
/// [`normalize_doc_payload`]'s one-canonical-leading-space strip — the
/// same normalization a real lexer token carries.
fn doc_line_payload(t: &SyntaxToken) -> String {
    let text = t.text();
    let sigil_len = text
        .chars()
        .next()
        .expect(
            "DOC_LINE/ATTENTION_LINE token text is never empty — it always carries its own sigil",
        )
        .len_utf8();
    normalize_doc_payload(&text[sigil_len..])
}

/// One `?`/`!` line's canonical form: `sigil` alone when `text` is
/// empty, else `sigil` + one space + `text` verbatim — ported unchanged
/// from C1's `print_doc_run_line` (pure text, no green-tree input).
fn print_doc_run_line(out: &mut String, pad: &str, sigil: char, text: &str) {
    out.push_str(pad);
    out.push(sigil);
    if !text.is_empty() {
        out.push(' ');
        out.push_str(text);
    }
    out.push('\n');
}

/// One extracted statement plus the facts [`Statement`] itself does not
/// carry: `label_break` ([`trivia::label_break`] — a green-tree query,
/// not a CST field), each item's `newline_before` (whether the author
/// put a newline before it inside its comma group, computed below from
/// the `ITEM` nodes' own text ranges — `Statement::items` is a flat
/// `Vec<Item>` with no per-item position of its own), each item's
/// `item_leading` ([`item_leading_comments`] — mid-comma-group interior
/// comments, C1's `CommaItem::leading` re-derived), and `trailing` — the
/// ONE comment [`print_statement`] prints after the `;`
/// ([`statement_trailing_and_leftovers`]'s resolved verdict, which a
/// pre-`;` comment can win over an ordinary post-`;` one; see that
/// function's own doc). `node` is kept alongside for
/// [`trivia::blank_before_unit`]'s per-body-item query in [`print_body`].
struct StmtElem {
    node: SyntaxNode,
    stmt: Statement,
    label_break: bool,
    newline_before: Vec<bool>,
    item_leading: Vec<Vec<SyntaxToken>>,
    /// Each `ITEM`'s own green node, for the comment-splicing token
    /// renderer ([`render_item_tokens`]) — an item whose node carries an
    /// interior comment renders from its tokens, comments in place.
    item_nodes: Vec<SyntaxNode>,
    /// Per label: the comments inside the `LABEL` node and the run up to
    /// the next label ([`label_region_comments`]) — printed in place in
    /// the label prefix; any entry forces the label-break layout.
    label_region: Vec<(Vec<SyntaxToken>, Vec<SyntaxToken>)>,
    /// The tail slot: comments between the last item and the `;`
    /// ([`item_leading_comments`]'s popped last entry) — printed before
    /// the `;`, never past it (the never-move rule).
    pre_semi: Vec<SyntaxToken>,
    trailing: Option<SyntaxToken>,
}

/// A function-body element, in the SAME order [`brace_interior`] yields
/// — never rebuilt by concatenating [`crate::syntax::FunctionView::statements`]
/// and [`crate::syntax::FunctionView::nested`] separately, which would lose
/// a nested function's position relative to its neighbouring statements.
/// `Comment` is an own-line comment reached directly as a raw sibling
/// token — leading, standalone, or trailing the whole body — see
/// [`print_body`]'s own doc for how it's told apart from a comment
/// that's already part of some OTHER element's leading run. A
/// `Statement`/`Nested`'s own same-line TRAILING comment is never this
/// variant: [`print_body`]'s collection loop keeps that token out of
/// `body` entirely, since it prints inline with the element it follows,
/// not as a body element of its own.
///
/// `TailComment` is a DIFFERENT shape with no C1-CST-field counterpart
/// at all: a comment nested inside a statement's own last item, or
/// sitting directly between it and the terminating `;`, that
/// [`statement_trailing_and_leftovers`] did NOT resolve as that
/// statement's `trailing` — C1's `Parser` re-drains it as an ordinary
/// standalone comment at the top of its OWN next body-loop iteration,
/// landing as a body item positioned right after the statement it was
/// physically nested inside (see that function's own doc for why its
/// blank-before is always `false`, never [`blank_immediately_before`]'s
/// generic sibling-gap query — the real tree sibling of one of these
/// tokens sits INSIDE the statement, not at body level, so that query
/// would ask the wrong question entirely).
enum BodyElem {
    Statement(Box<StmtElem>),
    Nested(FunctionView),
    Comment(SyntaxToken),
    TailComment(SyntaxToken),
}

/// The [`SyntaxNode`] a [`BodyElem::Statement`]/[`BodyElem::Nested`]
/// wraps, or `None` for [`BodyElem::Comment`]/[`BodyElem::TailComment`]
/// (which carry a raw token, not a node) — the one query [`print_body`]
/// needs both to build its `claimed` set (every `STATEMENT`/nested-
/// `FUNCTION`'s own leading comment run) and to drive its print loop's
/// per-element `blank_before_unit`/`leading_comments` queries, without
/// duplicating that pair of calls once per node variant.
fn body_elem_node(elem: &BodyElem) -> Option<&SyntaxNode> {
    match elem {
        BodyElem::Statement(s) => Some(&s.node),
        BodyElem::Nested(fv) => Some(fv.syntax()),
        BodyElem::Comment(_) | BodyElem::TailComment(_) => None,
    }
}

/// The one comment that prints as `elem`'s own trailing comment, if
/// any — the single source of truth both [`compute_trailing_spacing`]
/// and [`print_body`]'s own collection loop read, so neither can ever
/// diverge from what [`print_statement`]/a nested [`print_function`]'s
/// own close-brace print actually emits (the DOUBLE-PRINT trap this
/// plan has hit before, applied here to a NEW source of divergence: a
/// pre-`;` comment C1 relocated to `trailing`, per
/// [`statement_trailing_and_leftovers`]'s own doc). `BodyElem::Statement`
/// reads the field that function already resolved; `BodyElem::Nested`
/// reads [`trivia::trailing_comment`] directly — a nested function's own
/// body has no analogous before-`}` relocation surface, so its
/// close-brace comment is exactly what it already was;
/// `BodyElem::Comment`/`BodyElem::TailComment` carry none.
fn resolved_trailing(elem: &BodyElem) -> Option<SyntaxToken> {
    match elem {
        BodyElem::Statement(s) => s.trailing.clone(),
        BodyElem::Nested(fv) => trivia::trailing_comment(fv.syntax()),
        BodyElem::Comment(_) | BodyElem::TailComment(_) => None,
    }
}

/// Every `ITEM`'s own leading comment run inside `stmt`, indexed like
/// [`use_decl_interior_comments`] — slot 0 is whatever precedes the
/// first item (past any `LABEL`s), slot `i` is whatever precedes item
/// `i`, and the LAST slot (popped by every caller) is the "tail": a
/// comment nested inside the last item, or sitting directly between it
/// and the terminating `;`, that C1's `CommaItem::leading` has no home
/// for at all — no item ever follows the last one — and
/// [`statement_trailing_and_leftovers`] resolves separately.
///
/// The same drain-point rule as [`use_decl_interior_comments`]: a
/// comment nested inside an `ITEM` (`check(1 /* c */, 2)`, a child of
/// `ITEM`'s own `CHECK_ARM` child) or inside a `LABEL` (`1/* c */:`,
/// between the number and the colon) drains at the exact point C1's
/// parser next drains pending comments — which is always AFTER that
/// node's own tokens finish, at the boundary that opens the CURRENT
/// slot — never a reason to guard either shape away as somebody else's
/// surface.
fn item_leading_comments(stmt: &SyntaxNode) -> Vec<Vec<SyntaxToken>> {
    let mut out: Vec<Vec<SyntaxToken>> = vec![Vec::new()];
    // Comments seen before it's known whether a LABEL follows: a comment
    // that precedes another label belongs to the LABEL REGION
    // ([`label_region_comments`]) and prints interleaved with the labels,
    // never in an item slot — only the run between the LAST label and the
    // first item is slot 0's (the comment sits between `:` and the first
    // command, and prints there).
    let mut pending: Vec<SyntaxToken> = Vec::new();
    let mut items_started = false;
    for e in stmt.children_with_tokens() {
        match e {
            SyntaxElement::Node(n) if n.kind() == PmcKind::Label.into() => {
                pending.clear();
            }
            SyntaxElement::Node(n) if n.kind() == PmcKind::Item.into() => {
                if !items_started {
                    items_started = true;
                    out.last_mut()
                        .expect("out is never empty")
                        .append(&mut pending);
                }
                // A comment NESTED inside the item (`check(1 /* c */, 2)`)
                // is not collected here: it prints in place inside the
                // item's own rendered text ([`render_item_tokens`], the
                // never-move rule), never in a slot.
                out.push(Vec::new());
            }
            SyntaxElement::Token(t) if trivia::is_comment(t.kind()) => {
                if items_started {
                    out.last_mut().expect("out is never empty").push(t);
                } else {
                    pending.push(t);
                }
            }
            _ => {}
        }
    }
    // A labels-only statement can end with pending comments and no item;
    // they sit between the last label and the `;` — the tail slot's.
    out.last_mut()
        .expect("out is never empty")
        .append(&mut pending);
    out
}

/// Per label, the comments the never-move rule keys to that label: the
/// run INSIDE the `LABEL` node (between its number and its `:`) and the
/// run AFTER it up to the NEXT label (between two stacked labels).
/// Comments after the LAST label belong to slot 0 of
/// [`item_leading_comments`] instead — they sit between the `:` and the
/// first command and print there. Any comment here takes the statement
/// to the label-break layout, with the comments printed in place in the
/// label prefix.
fn label_region_comments(stmt: &SyntaxNode) -> Vec<(Vec<SyntaxToken>, Vec<SyntaxToken>)> {
    let mut out: Vec<(Vec<SyntaxToken>, Vec<SyntaxToken>)> = Vec::new();
    let mut pending: Vec<SyntaxToken> = Vec::new();
    for e in stmt.children_with_tokens() {
        match e {
            SyntaxElement::Node(n) if n.kind() == PmcKind::Label.into() => {
                if let Some(prev) = out.last_mut() {
                    prev.1.append(&mut pending);
                }
                pending.clear();
                let inside = n
                    .descendant_tokens()
                    .filter(|t| trivia::is_comment(t.kind()))
                    .collect();
                out.push((inside, Vec::new()));
            }
            SyntaxElement::Node(_) => break,
            SyntaxElement::Token(t) if trivia::is_comment(t.kind()) => {
                pending.push(t);
            }
            _ => {}
        }
    }
    out
}

/// A `FUNCTION` node's own body — [`brace_interior`] between its `{` and
/// `}` — mirroring C1's `print_function` body loop: one pass
/// collects each `STATEMENT`/nested `FUNCTION`/own-line comment in
/// source order (the single ordered walk [`BodyElem`]'s own doc
/// explains), a second computes the shared command column from every
/// collected statement (a `BodyElem::Comment` plays no part —
/// [`max_inline_label_prefix_width`]'s own `filter_map` already ignores
/// anything that isn't `BodyElem::Statement`), a third renders every
/// statement's own code ONCE up front and derives the trailing-comment
/// spacing for the whole body from those rendered widths
/// ([`compute_trailing_spacing`] — mirrors C1's own pre-pass, since the
/// alignment column of an early run member can depend on a LATER one's
/// reformatted width), and a fourth prints them.
///
/// **Own-line comments** (`docs/pmt/fmt.md` (comments)) mirror
/// [`print_items`]'s own `consumed`/`claimed` split, one level down: a
/// comment immediately (no blank line) before a `STATEMENT`/nested
/// `FUNCTION` belongs to THAT element's leading run
/// ([`trivia::leading_comments`]) and prints above it; every other
/// comment is its own standalone unit, printed the same way
/// ([`print_comment`]) with its own blank-line decision
/// ([`blank_immediately_before`]) — content printing never distinguishes
/// leading from standalone, only the blank-line decision does, same rule
/// as the top level. A `STATEMENT`/nested-`FUNCTION`'s own same-line
/// trailing comment is ALSO already owned outright by another printer
/// ([`print_statement`], or a nested [`print_function`]'s own
/// close-brace print) — exactly the shape [`print_items`]'s `consumed`
/// exists for, one level down: `leading_comments` walks back through
/// every raw token until a NODE sibling, so a trailing comment like that
/// surfaces as the NEXT element's own "leading run" too, and `consumed`
/// filters that print the same way it does at the top level. `reserved`
/// is a nested function's own [`trivia::open_trailing`] run, threaded
/// down from [`print_function`] exactly the way [`print_namespace`]
/// threads its own into [`print_items`] — without it, the SAME
/// `leading_comments` walk would ALSO surface an open-brace comment as
/// the first body element's own leading run, printing it a second time.
fn print_body(
    out: &mut String,
    func_node: &SyntaxNode,
    indent: usize,
    reserved: &[SyntaxToken],
    relocated: &[SyntaxToken],
    line_index: &TextLineIndex,
) {
    let elements: Vec<SyntaxElement> = brace_interior(func_node)
        .filter(|e| !trivia::is_ws(e.kind()))
        .collect();

    let mut body: Vec<BodyElem> = Vec::with_capacity(elements.len());
    // Parallel to `body`: the green re-derivation of C1's evolving
    // `prev_end_line` cursor, ONLY where it disagrees with a plain
    // real-sibling-gap query — i.e. only right after a
    // `BodyElem::TailComment` leftover (`statement_trailing_and_leftovers`'s
    // own doc). C1 drained every pending comment through ONE loop that
    // updated `prev_end_line` to each comment's OWN line as it went, so a
    // leftover tail comment became the new baseline for whatever was
    // printed right after it — but that comment is nested INSIDE the
    // statement it trails, not a real body-level sibling of what follows,
    // so the ordinary `blank_before_unit`/`blank_immediately_before`
    // sibling-gap queries would ask the wrong question there. `None`
    // everywhere else means "the real sibling gap already answers this
    // correctly" (true whenever nothing was relocated).
    let mut override_prev_line: Vec<Option<u32>> = Vec::with_capacity(elements.len());
    let mut pending_prev_line: Option<u32> = None;
    // The caller's relocated run (header-interior comments plus, with
    // them, the open-brace run — [`header_interior_comments`]) prints
    // FIRST, ahead of everything `brace_interior` yields. Entering each
    // as a `BodyElem::TailComment` buys the exact blank-line semantics
    // C1 gave relocation for free: the first one takes no blank (its
    // override is `None` and that arm defaults to none), each later
    // element measures against the PREVIOUS relocated comment's own
    // source line, and — since an open-brace comment relocated here is
    // ALSO reached below as its own raw `brace_interior` token, skipped
    // as `consumed` — `carried_prev_line` hands the baseline past the
    // skip to whatever prints next.
    for c in relocated {
        override_prev_line.push(pending_prev_line.take());
        pending_prev_line = Some(line_index.line_col(c.text_range().end).0);
        body.push(BodyElem::TailComment(c.clone()));
    }
    for e in &elements {
        match e {
            SyntaxElement::Node(node) if node.kind() == PmcKind::Statement.into() => {
                let sv = StatementView::cast(node.clone()).expect("kind checked above");
                let label_break = trivia::label_break(sv.syntax());
                let item_views: Vec<ItemView> = sv.items().collect();
                let mut newline_before = vec![false; item_views.len()];
                for i in 1..item_views.len() {
                    // Item K's first token on a later line than item
                    // K-1's LAST token — mirrors `Parser::statement`'s
                    // own `last_item_end_line` comparison exactly, so a
                    // multi-line item (e.g. a `check` split across
                    // lines inside its own parens) is measured by its
                    // own last token, not its first.
                    let prev_end = item_views[i - 1].syntax().text_range().end;
                    let cur_start = item_views[i].syntax().text_range().start;
                    newline_before[i] =
                        line_index.line_col(cur_start).0 > line_index.line_col(prev_end).0;
                }
                let mut item_leading = item_leading_comments(sv.syntax());
                let tail = item_leading
                    .pop()
                    .expect("item_leading_comments always yields items.len() + 1 slots (the tail)");
                let stmt = extract_statement(&sv, line_index);
                let label_region = label_region_comments(sv.syntax());
                // A label-region comment forces the label-break layout:
                // the comment prints in place in the prefix, and the
                // prefix no longer competes for the shared inline label
                // column (`max_inline_label_prefix_width` keys on this
                // flag).
                let label_break = label_break
                    || label_region
                        .iter()
                        .any(|(i, a)| !i.is_empty() || !a.is_empty());
                // The tail slot prints BEFORE the `;` — the comment
                // precedes it in source, and the never-move rule keeps it
                // there. The statement's own trailing comment is only ever
                // a genuine post-`;` one now.
                let trailing = trivia::trailing_comment(sv.syntax());
                override_prev_line.push(pending_prev_line.take());
                body.push(BodyElem::Statement(Box::new(StmtElem {
                    node: node.clone(),
                    stmt,
                    label_break,
                    newline_before,
                    item_leading,
                    item_nodes: item_views.iter().map(|v| v.syntax().clone()).collect(),
                    label_region,
                    pre_semi: tail,
                    trailing,
                })));
            }
            SyntaxElement::Node(node) if node.kind() == PmcKind::Function.into() => {
                let fv = FunctionView::cast(node.clone()).expect("kind checked above");
                override_prev_line.push(pending_prev_line.take());
                body.push(BodyElem::Nested(fv));
            }
            SyntaxElement::Node(node) => unreachable!(
                "unexpected node kind {:?} inside a function body; only STATEMENT and FUNCTION \
                 can appear here",
                node.kind()
            ),
            SyntaxElement::Token(t) if trivia::is_comment(t.kind()) => {
                // A STATEMENT/nested-FUNCTION's own same-line trailing
                // comment is this same raw token, reached again here as
                // the element's own next sibling — `body` must NOT carry
                // it as a second, separate entry: a `BodyElem::Comment`
                // between two statements is supposed to mean a genuine
                // OWN-LINE comment, breaking adjacency for
                // [`compute_trailing_spacing`]'s run scan; a trailing
                // comment is not that; it already prints inline, riding
                // the SAME line as the element it follows
                // ([`print_statement`]/a nested [`print_function`]'s own
                // close-brace print). Read through [`resolved_trailing`],
                // not a bare `trivia::trailing_comment` recomputation: a
                // statement's OWN `trailing` field can be a pre-`;`
                // comment instead, in which case a genuine post-`;`
                // comment like this one is NOT already accounted for and
                // must NOT be mistaken for it.
                let already_trailing = body
                    .last()
                    .and_then(resolved_trailing)
                    .is_some_and(|tc| &tc == t);
                if !already_trailing {
                    override_prev_line.push(pending_prev_line.take());
                    body.push(BodyElem::Comment(t.clone()));
                }
            }
            SyntaxElement::Token(t) => unreachable!(
                "unexpected token {:?} inside a function body; only STATEMENT/FUNCTION nodes, \
                 comments, and whitespace can appear here",
                t.kind()
            ),
        }
    }
    debug_assert_eq!(
        override_prev_line.len(),
        body.len(),
        "override_prev_line is pushed exactly once per body element, in lockstep"
    );

    let command_col = command_column(max_inline_label_prefix_width(&body), indent);

    // Every statement's code (label + items, no `;`) is rendered ONCE up
    // front — the trailing-comment alignment pre-pass
    // ([`compute_trailing_spacing`]) needs every run member's rendered
    // width before any of them is printed, mirroring
    // the `codes` pre-pass C1's `print_function` ran. Non-statement
    // elements get an unused empty placeholder — `compute_trailing_spacing`
    // never indexes into one, since a run can only ever contain
    // `BodyElem::Statement` entries (see that function's own doc).
    let codes: Vec<String> = body
        .iter()
        .map(|elem| match elem {
            BodyElem::Statement(s) => render_statement_code(s, command_col),
            BodyElem::Nested(_) | BodyElem::Comment(_) | BodyElem::TailComment(_) => String::new(),
        })
        .collect();
    let trailing_spacing = compute_trailing_spacing(&body, &codes, line_index);

    // Comments some OTHER printer already owns outright: `reserved` (this
    // body's own open-brace run, already printed on the header line by
    // the caller — see this function's own doc) plus every
    // STATEMENT/nested-FUNCTION element's own same-line trailing comment
    // ([`print_statement`], or a nested `print_function`'s own
    // close-brace print) via [`resolved_trailing`]. It does the same two
    // jobs [`print_items`]'s `consumed` does, and is checked in both of
    // the same places:
    //
    // * against an element's own leading run — `leading_comments` walks
    //   back through every raw token until a NODE sibling, so an
    //   open-brace or trailing comment surfaces as the NEXT element's
    //   "leading run" too;
    // * against a standalone `BodyElem::Comment` — the walk also reaches
    //   an open-brace comment directly, as its own raw token, whenever
    //   nothing claims it as a leading run (an empty body, or a blank
    //   line between it and the first body element). Skipping only
    //   `claimed` there printed it a SECOND time at body indent, and
    //   since `pmt fmt` rewrites in place every further pass appended
    //   another copy without bound. The trailing half of this set needs
    //   no such arm — the collection loop above keeps a trailing
    //   comment's own token out of `body` (`already_trailing`) — but
    //   `reserved` is never filtered at collection time, so the arm is
    //   what covers it (`docs/pmt/fmt.md` (comments)).
    let mut consumed: Vec<SyntaxToken> = reserved.to_vec();
    for elem in &body {
        consumed.extend(resolved_trailing(elem));
    }

    // Every STATEMENT/nested-FUNCTION element's own leading comment run,
    // unioned — checked, together with `consumed` above, against a
    // standalone `BodyElem::Comment` below, so a comment already printed
    // as some OTHER element's leading run isn't printed a SECOND time
    // when the walk also reaches it directly as its own raw token.
    // [`print_items`] gets the same two-set guard by building its own
    // `claimed` as a superset of its `consumed`; here they are kept
    // disjoint and both named at the use site, because the leading-run
    // filter below wants `consumed` alone.
    let mut claimed: Vec<SyntaxToken> = Vec::new();
    for elem in &body {
        if let Some(node) = body_elem_node(elem) {
            claimed.extend(trivia::leading_comments(node));
        }
    }

    let mut first = true;
    // Carries a computed `blank_override` PAST a `BodyElem::Comment` the
    // print loop skips outright (`claimed`/`consumed` — such a comment
    // prints elsewhere: via the NEXT node's own `leading_comments` loop
    // below, or on the header line above, never as its own entry here).
    // `override_prev_line[i]` alone is blind to this: it was computed at
    // COLLECTION time, when whether THIS comment will end up claimed by a
    // later node isn't known yet (`claimed` itself is only built once
    // collection finishes). Without carrying it forward, the override
    // silently vanishes at exactly the element it was meant for, and
    // whatever prints next falls back to a real-sibling-gap query that
    // sees the preceding statement's `;` sitting between the leftover
    // comment and itself — measuring the gap from the `;` line rather
    // than from the leftover's own line, which is one line too early and
    // drops a blank line the author wrote.
    let mut carried_prev_line: Option<u32> = None;
    for (i, elem) in body.iter().enumerate() {
        // The green stand-in for C1's evolving `prev_end_line`
        // (`override_prev_line`'s own doc): `Some` only right after a
        // `BodyElem::TailComment` leftover, where the real sibling gap
        // would ask the wrong question — either freshly computed at THIS
        // index, or carried forward from an earlier index whose own
        // comment got skipped as claimed (`carried_prev_line`, above). At
        // most one of the two is ever `Some` in practice: a fresh
        // `override_prev_line[i]` only arises when NOTHING was pushed
        // between the leftover and this element, which precludes a
        // skipped comment (and its carry) from also reaching here.
        let blank_override = carried_prev_line.take().or(override_prev_line[i]);
        match elem {
            BodyElem::Comment(tok) => {
                if claimed.contains(tok) || consumed.contains(tok) {
                    carried_prev_line = blank_override;
                    continue;
                }
                let blank = match blank_override {
                    Some(prev) => line_index.line_col(tok.text_range().start).0 > prev + 1,
                    None => blank_immediately_before(tok),
                };
                if !first && blank {
                    out.push('\n');
                }
                first = false;
                print_comment(out, tok, indent);
                continue;
            }
            BodyElem::TailComment(tok) => {
                // No preceding leftover in the SAME tail
                // (`blank_override` is `None`) means always no blank line
                // — see `statement_trailing_and_leftovers`'s own doc for
                // why: a tail member's own source line can never exceed
                // its statement's `;`, so C1's `cline > prev_end_line + 1`
                // check can never fire for the FIRST one. A SECOND (or
                // later) leftover in the same tail instead measures
                // against the PRECEDING leftover's own line, exactly like
                // any other position `blank_override` covers.
                let blank = match blank_override {
                    Some(prev) => line_index.line_col(tok.text_range().start).0 > prev + 1,
                    None => false,
                };
                if !first && blank {
                    out.push('\n');
                }
                first = false;
                print_comment(out, tok, indent);
                continue;
            }
            _ => {}
        }
        let node = body_elem_node(elem).expect(
            "BodyElem::Comment/TailComment handled and skipped above; every other variant \
             carries a node",
        );
        let blank = match blank_override {
            Some(prev) => unit_start_line(node, line_index) > prev + 1,
            None => trivia::blank_before_unit(node),
        };
        if !first && blank {
            out.push('\n');
        }
        first = false;
        for c in trivia::leading_comments(node) {
            if !consumed.contains(&c) {
                print_comment(out, &c, indent);
            }
        }
        print_body_item(
            out,
            elem,
            indent,
            &codes[i],
            trailing_spacing[i],
            command_col,
            line_index,
        );
    }
}

/// Dispatches one node-backed [`BodyElem`] to its printer — ported from
/// C1's `print_body_item`. [`BodyElem::Comment`]/[`BodyElem::TailComment`]
/// never reach this dispatcher: [`print_body`]'s own loop prints them
/// directly, mirroring how [`print_items`] prints a standalone comment
/// token itself rather than routing it through [`print_item`].
/// `code`/`trailing_spacing` are [`print_body`]'s pre-pass outputs,
/// meaningful only for [`BodyElem::Statement`] (mirrors
/// the `code`/`trailing_spacing` parameters C1's `print_body_item`
/// took, unused by its `Nested`/`Comment` arms).
fn print_body_item(
    out: &mut String,
    elem: &BodyElem,
    indent: usize,
    code: &str,
    trailing_spacing: usize,
    command_col: usize,
    line_index: &TextLineIndex,
) {
    match elem {
        BodyElem::Statement(s) => print_statement(out, s, code, trailing_spacing, command_col),
        BodyElem::Nested(fv) => print_function(out, fv, indent, line_index),
        BodyElem::Comment(_) | BodyElem::TailComment(_) => unreachable!(
            "print_body's own loop handles BodyElem::Comment/BodyElem::TailComment directly \
             and never reaches this dispatcher — see print_body_item's own doc"
        ),
    }
}

/// Label prefix width: the smallest multiple of [`INDENT_UNIT`]
/// that is `>= max(base_body_indent, P + 2)`, where `P` is the widest
/// INLINE labeled statement's label-prefix width in the body — ported
/// unchanged from C1's `command_column` (pure `usize` arithmetic, no
/// green-tree input).
fn command_column(p: usize, base_body_indent: usize) -> usize {
    let min = base_body_indent.max(p + 2);
    min.div_ceil(INDENT_UNIT) * INDENT_UNIT
}

/// `P`: the max label-prefix width among `body`'s own INLINE labeled
/// statements — ported from C1's `max_inline_label_prefix_width`,
/// reading `body`'s [`BodyElem`]s instead of C1's `&[BodyItem]`. Only
/// looks at THIS function's own body — a nested function's statements
/// belong to ITS OWN body/command-column, and never appear in `body`
/// (`FunctionView::nested`'s "direct children only" contract).
fn max_inline_label_prefix_width(body: &[BodyElem]) -> usize {
    body.iter()
        .filter_map(|elem| match elem {
            BodyElem::Statement(s) if !s.label_break && !s.stmt.labels.is_empty() => {
                Some(label_prefix_width(&s.stmt.labels))
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// A statement's label prefix as printed: each label `N:` — `N` is the
/// number as WRITTEN (leading zeros preserved, fmt never touches a
/// token), not re-derived from the parsed value — joined by one space,
/// e.g. `1:` or the stacked `1: 2:`. Empty for an unlabeled statement.
/// Ported unchanged from C1's `label_prefix_text` (`&[Label]` is the
/// same [`crate::parser::Label`] on both sides).
fn label_prefix_text(labels: &[Label]) -> String {
    labels
        .iter()
        .map(|l| format!("{}:", l.written))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Char width of [`label_prefix_text`] — ported unchanged from
/// C1's `label_prefix_width`.
fn label_prefix_width(labels: &[Label]) -> usize {
    label_prefix_text(labels).chars().count()
}

/// Left margin for a `prefix_width`-wide label prefix at `command_col`,
/// or `None` if that would leave less than the mandatory 1-space margin
/// — ported unchanged from C1's `label_margin`.
fn label_margin(command_col: usize, prefix_width: usize) -> Option<usize> {
    command_col
        .checked_sub(prefix_width + 1)
        .filter(|&margin| margin >= 1)
}

/// One statement's code up to but NOT including the final `;` — ported
/// from C1's `render_statement_code`, reading a `Statement`'s own
/// `labels`/`items` plus the separately-derived `label_break` and
/// `newline_before` instead of a `StatementCst`'s fields directly. See
/// that function's own doc for the unlabeled / inline-labeled /
/// own-line-labeled shapes; the decisions are unchanged. Called once per
/// statement, up front, by [`print_body`]'s own pre-pass — see that
/// function's own doc for why [`compute_trailing_spacing`] needs every
/// statement's code rendered before any of them prints.
fn render_statement_code(s: &StmtElem, command_col: usize) -> String {
    let labels = &s.stmt.labels;
    let label_break = s.label_break;
    let label_region = &s.label_region;
    let region_commented = label_region
        .iter()
        .any(|(i, a)| !i.is_empty() || !a.is_empty());
    let mut out = String::new();
    if labels.is_empty() {
        out.push_str(&" ".repeat(command_col));
    } else if region_commented {
        // Label-region comments print in place, interleaved with the
        // labels at a fixed one-space indent (the margin alignment is
        // meaningless once comments stretch the prefix), and the
        // statement always takes the label-break layout — `label_break`
        // was already forced true where this element was built.
        out.push(' ');
        out.push_str(&label_prefix_with_comments(labels, label_region));
        out.push('\n');
        out.push_str(&" ".repeat(command_col));
    } else {
        let prefix = label_prefix_text(labels);
        let width = prefix.chars().count();
        if label_break {
            match label_margin(command_col, width) {
                Some(margin) => out.push_str(&" ".repeat(margin)),
                None => out.push(' '),
            }
            out.push_str(&prefix);
            out.push('\n');
            out.push_str(&" ".repeat(command_col));
        } else {
            let margin = label_margin(command_col, width).expect(
                "max_inline_label_prefix_width guarantees a >=1 margin for every inline label",
            );
            out.push_str(&" ".repeat(margin));
            out.push_str(&prefix);
            out.push(' ');
        }
    }
    out.push_str(&render_items(
        &s.stmt.items,
        &s.newline_before,
        &s.item_leading,
        &s.item_nodes,
        command_col,
    ));
    // The tail slot: comments between the last item and the `;` print
    // BEFORE the `;`, in their written slot (the never-move rule) — an
    // own-line comment keeps its own line at the command column, a
    // same-line one rides the code line.
    for c in &s.pre_semi {
        if comment_own_line(c) {
            out.push('\n');
            out.push_str(&" ".repeat(command_col));
        } else {
            out.push(' ');
        }
        out.push_str(&normalize_comment_text(c.text()));
    }
    out
}

/// The label prefix with the label-region comments interleaved in their
/// written slots: `1 /* a */: /* b */ 2:` — an inside comment between
/// its number and `:`, an after comment between the `:` and the next
/// label. A LINE comment eats the rest of its physical line, so the
/// remainder continues on the next line at the same one-space indent.
fn label_prefix_with_comments(
    labels: &[Label],
    region: &[(Vec<SyntaxToken>, Vec<SyntaxToken>)],
) -> String {
    let mut s = String::new();
    for (i, l) in labels.iter().enumerate() {
        if i > 0 && !s.ends_with(' ') {
            s.push(' ');
        }
        s.push_str(&l.written);
        let (inside, after) = &region[i];
        for c in inside {
            s.push(' ');
            s.push_str(&normalize_comment_text(c.text()));
            if is_line_comment(c) {
                s.push('\n');
                s.push(' ');
            }
        }
        s.push(':');
        for c in after {
            s.push(' ');
            s.push_str(&normalize_comment_text(c.text()));
            if is_line_comment(c) {
                s.push('\n');
                s.push(' ');
            }
        }
    }
    s
}

/// Whether a comment token is the `//` kind — the kind nothing can
/// follow on its physical line.
fn is_line_comment(c: &SyntaxToken) -> bool {
    c.text().trim_start().starts_with("//")
}

/// One statement's final line(s): the precomputed `code`
/// ([`render_statement_code`], rendered once by [`print_body`]'s own
/// pre-pass), the `;`, then a same-line trailing comment if any — spaced
/// per `trailing_spacing` ([`compute_trailing_spacing`],
/// `docs/pmt/fmt.md` (comments)) — then the newline. The `;` abuts the
/// code — or, when the tail slot's last comment is a LINE comment
/// (nothing can follow it on its line), moves to its own line at
/// `command_col`.
fn print_statement(
    out: &mut String,
    s: &StmtElem,
    code: &str,
    trailing_spacing: usize,
    command_col: usize,
) {
    out.push_str(code);
    if s.pre_semi.last().is_some_and(is_line_comment) {
        out.push('\n');
        out.push_str(&" ".repeat(command_col));
    }
    out.push(';');
    if let Some(tc) = &s.trailing {
        out.push_str(&" ".repeat(trailing_spacing));
        out.push_str(&normalize_comment_text(tc.text()));
    }
    out.push('\n');
}

/// Char width of `code`'s LAST physical line, `+ 1` for the `;` that
/// follows it (a statement's trailing comment always rides the line
/// carrying the final `;`, even when a multi-line own-line label or
/// comma-group spreads the rest of the statement across earlier lines) —
/// ported unchanged from C1's `code_line_width_incl_semi` (pure
/// text/`usize`, no green-tree input).
fn code_line_width_incl_semi(code: &str) -> usize {
    let last_line = code.rsplit('\n').next().unwrap_or(code);
    last_line.chars().count() + 1
}

/// Trailing-comment alignment (`docs/pmt/fmt.md` (comments)) — ported
/// from C1's `compute_trailing_spacing`, reading `body`'s
/// [`BodyElem`]s instead of C1's `&[BodyItem]` and each member's
/// trailing comment via [`resolved_trailing`] instead of a
/// `StatementCst::trailing` field — the SAME resolved value
/// [`print_statement`] prints, never a fresh
/// `trivia::trailing_comment(&s.node)` recomputation (this plan's own
/// DOUBLE-PRINT/silent-divergence trap: a pre-`;` comment relocates to
/// `s.trailing`, so re-deriving it here independently could disagree
/// with what actually prints). Returns, per `body` index, the number
/// of spaces to place between the `;` and a trailing `//`/`/* */` —
/// meaningful only where that [`BodyElem`] is a [`BodyElem::Statement`]
/// with a trailing comment; other entries are unused filler.
///
/// **The run-boundary rule**, derived by reading
/// C1's own `compute_trailing_spacing` scan (not restated from any
/// plan brief) and confirmed against the C1 formatter on several
/// hand-built sources before this doc was written: a run is a MAXIMAL
/// sequence of CONSECUTIVE [`BodyElem::Statement`] entries, each carrying
/// a trailing comment. The scan that finds one has two distinct rules —
/// one for starting a run, a stricter one for extending it:
///
/// - **Starting**: any [`BodyElem::Statement`] with a trailing comment
///   can start a run, regardless of its OWN `blank_before` — the C1 scan
///   never reads `body[run_start].blank_before` at all, only every
///   CANDIDATE EXTENSION's. A statement preceded by a blank line is
///   still a perfectly good run of its own.
/// - **Extending**: the NEXT element joins the current run only if it is
///   ALSO a [`BodyElem::Statement`] carrying a trailing comment AND has
///   no blank line before it. Failing either test ends the run right
///   there — a nested function, an own-line comment, a statement with no
///   trailing comment, OR a statement that has one but sits after a
///   blank line, are all run-enders. The very next qualifying statement
///   (if any) starts a brand-new run, its own `blank_before` irrelevant
///   as above; nothing is skipped over to "reach past" a non-member.
///
/// A run of exactly one member always renders at the default one space
/// (`spacing` starts all-`1` and the `run_len >= 2` branch below is the
/// only place that changes it) — alignment as a visible effect exists
/// only for a run of two or more. For such a run: `align_col` is one
/// past the WIDEST reformatted code line in the run
/// ([`code_line_width_incl_semi`], last physical line only — an
/// own-line-labeled statement's earlier line(s) never count). A member
/// whose comment would cross column 80 at `align_col` is `overflow` and
/// falls back to one space on its own, REGARDLESS of what the rest of
/// the run does. The run is `aligned` only if every NON-overflowing
/// member's SOURCE `//` column (`index.line_col`) is identical to every
/// other non-overflowing member's — an overflowing member's own column
/// plays NO part in that verdict. This exclusion is for IDEMPOTENCE, not
/// merely to protect the verdict from one ill-fitting number: an
/// overflowing member renders at its OWN width-derived one-space column,
/// not `align_col`, so its SOURCE column on a second `pmt fmt` pass (which
/// re-derives every column from THIS pass's OUTPUT) would almost never
/// match the rest of the run — reading it into the verdict would flip an
/// aligned run to ragged on every other reformat. Excluding it is what
/// keeps a run's aligned/ragged verdict — and so its whole layout —
/// stable under repeated formatting (`docs/pmt/fmt.md` (comments) —
/// the same reasoning C1 recorded as its own "Idempotence note").
/// When `aligned`, every non-overflowing member gets
/// `align_col - code_w[off]` spaces (which lands its `//` exactly at
/// `align_col`); when not, every member (overflowing or not) gets one
/// space.
///
/// **A note on `blank_before`**: C1 read a `blank_before` field — the
/// gap immediately before that ITEM. Extension here reads
/// [`trivia::blank_before_unit`] instead, which looks PAST an item's own
/// leading comment run to the gap before the whole unit — a different
/// query in general. They agree at this one call site: a genuine blank
/// line strips `body[j]`'s leading run to empty (a blank cuts
/// `leading_comments`), so `blank_before_unit` looks straight at that
/// same blank and both queries read it identically; absent a blank line,
/// `body[j]`'s leading run instead picks up the PRECEDING member's own
/// trailing comment (same-line whitespace before it, never a blank), so
/// both queries again agree — `false`. The one shape where they COULD
/// diverge — a real own-line comment sitting between the two statements —
/// never reaches this call at all: `has_trailing` already ends the run at
/// that comment (`BodyElem::Comment` is never `Statement`), short-circuiting
/// `&&` before `blank_before` is evaluated.
fn compute_trailing_spacing(
    body: &[BodyElem],
    codes: &[String],
    line_index: &TextLineIndex,
) -> Vec<usize> {
    let mut spacing = vec![1usize; body.len()];
    // Statement-only, deliberately NOT `resolved_trailing(elem).is_some()`:
    // a nested function's own close-brace comment never joins an
    // alignment run — C1's own `has_trailing` matches `BodyKind::Statement`
    // alone (`fmt/mod.rs`), and a `BodyElem::Nested` with a real trailing
    // comment must reproduce that exclusion, not silently start
    // participating just because `resolved_trailing` also covers it.
    let has_trailing =
        |elem: &BodyElem| matches!(elem, BodyElem::Statement(s) if s.trailing.is_some());
    // Only ever called on a `body[j]` the caller has already confirmed
    // `has_trailing` for, so `elem` is always a `BodyElem::Statement`
    // here — an unreachable panic rather than a silent `false` arm for
    // `Nested`/`Comment`/`TailComment`, since those variants never
    // actually reach this closure (mirrors `comment_w`/`source_cols`'s
    // own `let … else` idiom below).
    let blank_before = |elem: &BodyElem| {
        let BodyElem::Statement(s) = elem else {
            unreachable!("has_trailing guarantees a Statement");
        };
        trivia::blank_before_unit(&s.node)
    };
    let mut i = 0;
    while i < body.len() {
        if !has_trailing(&body[i]) {
            i += 1;
            continue;
        }
        let run_start = i;
        let mut j = i + 1;
        while j < body.len() && has_trailing(&body[j]) && !blank_before(&body[j]) {
            j += 1;
        }
        let run_end = j;
        let run_len = run_end - run_start;

        let code_w: Vec<usize> = (run_start..run_end)
            .map(|k| code_line_width_incl_semi(&codes[k]))
            .collect();
        let comment_w: Vec<usize> = (run_start..run_end)
            .map(|k| {
                let BodyElem::Statement(s) = &body[k] else {
                    unreachable!("has_trailing guarantees a Statement");
                };
                let tc = s.trailing.as_ref().expect("has_trailing guarantees Some");
                // Measured on the NORMALIZED text: a raw trailing
                // `\r`/space in the token must not inflate the column
                // math for a width nothing will actually print.
                normalize_comment_text(tc.text()).chars().count()
            })
            .collect();

        if run_len >= 2 {
            let max_code_w = *code_w.iter().max().expect("run_len >= 2");
            let align_col = max_code_w + 1;
            let overflow: Vec<bool> = (0..run_len)
                .map(|off| align_col + comment_w[off] > LINE_WIDTH)
                .collect();
            let source_cols: Vec<u32> = (run_start..run_end)
                .map(|k| {
                    let BodyElem::Statement(s) = &body[k] else {
                        unreachable!("has_trailing guarantees a Statement");
                    };
                    let tc = s.trailing.as_ref().expect("has_trailing guarantees Some");
                    line_index.line_col(tc.text_range().start).1
                })
                .collect();
            let non_overflow_cols: Vec<u32> = source_cols
                .iter()
                .zip(&overflow)
                .filter(|&(_, ovf)| !ovf)
                .map(|(&c, _)| c)
                .collect();
            let aligned = non_overflow_cols.windows(2).all(|w| w[0] == w[1]);
            for off in 0..run_len {
                spacing[run_start + off] = if aligned && !overflow[off] {
                    align_col - code_w[off]
                } else {
                    1
                };
            }
        }
        // run_len == 1 (lone): leave the default 1.
        i = run_end;
    }
    spacing
}

/// Comma-group layout (`docs/pmt/fmt.md` (comma groups)): respect the
/// author's own line breaks (`newline_before`), with a greedy-fill width
/// fallback, PLUS `leading`'s mid-comma-group comments
/// (the green re-derivation of C1's own per-entry `leading` field,
/// [`item_leading_comments`]) — ported from C1's `render_items` in
/// full. `items`, `newline_before` and `leading` are parallel, index 0's
/// `newline_before` always `false`.
///
/// Items are first partitioned into groups at each `newline_before`
/// boundary — the first item always starts group 0, and an item with
/// `newline_before` set always starts a NEW group. When no item sets it,
/// this yields exactly one group holding every item, which collapses the
/// no-author-break rules onto the very same per-group logic as the
/// preserved-line one: each group is emitted as one line if it fits
/// (`command_col` + its comma-joined text + 1 for the trailing `,`
/// boundary or the statement's final `;`, both width 1, <= 80), else
/// [`greedy_fill_group`] repacks just that group. A non-last group's
/// line ends with a trailing `,` (the boundary to the next group); the
/// very last group carries none — [`print_statement`] appends the final
/// `;` itself.
///
/// See [`layout_leading`] for how a leading comment run resolves to an
/// inline prefix or a forced break, folded into the SAME group-boundary
/// machinery a `newline_before` break uses (a forced break behaves like
/// an author newline for grouping purposes).
fn render_items(
    items: &[Item],
    newline_before: &[bool],
    leading: &[Vec<SyntaxToken>],
    item_nodes: &[SyntaxNode],
    command_col: usize,
) -> String {
    let layouts: Vec<LeadingLayout> = leading.iter().map(|l| layout_leading(l)).collect();
    let texts: Vec<String> = items
        .iter()
        .enumerate()
        .zip(&layouts)
        .map(|((i, item), layout)| {
            // An item whose node carries an interior comment renders from
            // its own tokens with the comments in place (the never-move
            // rule); a comment-free item keeps the classic canonical
            // rendering (byte-identical for it).
            let body = match item_nodes
                .get(i)
                .filter(|n| n.descendant_tokens().any(|t| trivia::is_comment(t.kind())))
            {
                Some(node) => render_item_tokens(node, command_col + 4),
                None => render_item(item),
            };
            format!("{}{}", layout.inline_prefix, body)
        })
        .collect();
    let mut groups: Vec<Vec<usize>> = vec![vec![0]];
    for (i, &nb) in newline_before.iter().enumerate().skip(1) {
        if nb || layouts[i].forced_break {
            groups.push(vec![i]);
        } else {
            groups.last_mut().expect("groups is never empty").push(i);
        }
    }
    let last_group_idx = groups.len() - 1;
    let mut out = String::new();
    // Rare (`parser.rs`'s own leading-trivia doc: "a comment between the
    // label and the first command"): a forcing LINE comment on item 0
    // has no preceding `,` to attach to — emit it directly (the caller
    // already left `out`'s position at `command_col`, per
    // `render_statement_code`'s invariant).
    if layouts[0].forced_break {
        emit_forced_break(&mut out, &layouts[0], command_col);
    }
    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            let first_idx = group[0];
            if layouts[first_idx].forced_break {
                // The preceding group's trailing `,` (below) is already
                // in `out`; one space, then the comment(s), then break.
                out.push(' ');
                emit_forced_break(&mut out, &layouts[first_idx], command_col);
            } else {
                out.push('\n');
                out.push_str(&" ".repeat(command_col));
            }
        }
        let group_texts: Vec<&str> = group.iter().map(|&i| texts[i].as_str()).collect();
        let joined = group_texts.join(", ");
        // `+ 1` reserved for the trailing `,`/`;` folded into
        // `< LINE_WIDTH` (clippy::int_plus_one), same as C1.
        if command_col + joined.chars().count() < LINE_WIDTH {
            out.push_str(&joined);
        } else {
            greedy_fill_group(&mut out, &group_texts, command_col);
        }
        if gi != last_group_idx {
            out.push(',');
        }
    }
    out
}

/// How one [`item_leading_comments`] entry resolves
/// (`docs/pmt/fmt.md` (comments)) — ported unchanged from
/// C1's `LeadingLayout`, reading `&[SyntaxToken]` instead of
/// `&[Comment]`. A BLOCK comment with no LINE comment among it/its
/// siblings stays inline, prepended to the item's own text
/// (`inline_prefix`) — the join/greedy-fill logic downstream never has
/// to know a comment was there. A LINE comment forces a break: nothing
/// can follow `//` on its physical line, so everything up to and
/// including the first LINE comment becomes `break_inline` (emitted
/// right after the preceding separator, before the forced newline);
/// anything AFTER that first LINE comment (pathological, but still MUST
/// be reprinted per the brief — fidelity over layout) becomes
/// `pre_item_lines`, each on its own re-indented line ahead of the item.
struct LeadingLayout {
    inline_prefix: String,
    forced_break: bool,
    break_inline: String,
    pre_item_lines: Vec<String>,
}

/// Ported unchanged from C1's `layout_leading`, reading a
/// `SyntaxToken`'s own `kind()`/`text()` instead of a `Comment`'s
/// `kind`/`text` fields.
fn layout_leading(leading: &[SyntaxToken]) -> LeadingLayout {
    match leading
        .iter()
        .position(|c| c.kind() == PmcKind::LineComment.into())
    {
        Some(break_pos) => {
            let mut break_inline = String::new();
            for c in &leading[..break_pos] {
                break_inline.push_str(&normalize_comment_text(c.text()));
                break_inline.push(' ');
            }
            break_inline.push_str(&normalize_comment_text(leading[break_pos].text()));
            let pre_item_lines = leading[break_pos + 1..]
                .iter()
                .map(|c| normalize_comment_text(c.text()))
                .collect();
            LeadingLayout {
                inline_prefix: String::new(),
                forced_break: true,
                break_inline,
                pre_item_lines,
            }
        }
        None => {
            let mut inline_prefix = String::new();
            for c in leading {
                inline_prefix.push_str(&normalize_comment_text(c.text()));
                inline_prefix.push(' ');
            }
            LeadingLayout {
                inline_prefix,
                forced_break: false,
                break_inline: String::new(),
                pre_item_lines: Vec::new(),
            }
        }
    }
}

/// Emits a [`LeadingLayout`]'s forced break: `break_inline` on the
/// current line, a newline, each `pre_item_lines` entry on its own line
/// at `command_col`, then `command_col` spaces — leaving the cursor
/// ready for the item that follows. The caller has already placed
/// whatever separator (`,` + space, or nothing for item 0) belongs
/// before it. Ported unchanged from C1's `emit_forced_break`.
fn emit_forced_break(out: &mut String, layout: &LeadingLayout, command_col: usize) {
    out.push_str(&layout.break_inline);
    out.push('\n');
    for line in &layout.pre_item_lines {
        out.push_str(&" ".repeat(command_col));
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&" ".repeat(command_col));
}

/// The true output column after appending `text` at the point where the
/// cursor sits at `base` — ported unchanged from C1's `line_width_after`
/// (pure text/`usize`, no green-tree input). A [`LeadingLayout`] BLOCK
/// comment can embed real newlines (a multi-line `/* … */` leading an
/// item); when `text` contains one, everything up to its FIRST `\n`
/// continues from `base`, but every line after that is raw content
/// printed verbatim with no re-indent (`print_comment`'s doc: only a
/// comment's first line gets `command_col` prefixed) — so the cursor's
/// true resulting column is simply the width of the substring AFTER the
/// LAST `\n`, independent of `base` entirely. Text with no embedded
/// newline (the overwhelming common case: no comment, or a single-line
/// one) takes the plain `base + chars(text)` sum, unchanged.
fn line_width_after(base: usize, text: &str) -> usize {
    match text.rsplit_once('\n') {
        Some((_, last_line)) => last_line.chars().count(),
        None => base + text.chars().count(),
    }
}

/// Rule 2's greedy-fill, applied to one group's items — ported unchanged
/// from C1's `greedy_fill_group` (pure text/`usize`, no green-tree
/// input).
fn greedy_fill_group(out: &mut String, texts: &[&str], command_col: usize) {
    let mut items = texts.iter();
    let first = items.next().expect("a comma group is never empty");
    out.push_str(first);
    let mut col = line_width_after(command_col, first);
    for text in items {
        let w = text.chars().count();
        if col + 2 + w < LINE_WIDTH {
            out.push_str(", ");
            out.push_str(text);
            col = line_width_after(col + 2, text);
        } else {
            out.push(',');
            out.push('\n');
            out.push_str(&" ".repeat(command_col));
            out.push_str(text);
            col = line_width_after(command_col, text);
        }
    }
}

/// Canonical item text with the item's INTERIOR comments spliced in
/// place — the never-move rendering an item takes when its node carries
/// one (`check(1 /* c */, 2)`, `right( /* c */ 3)`, `@b( /* c */ !)`).
/// Walks the item's own tokens in source order: significant tokens
/// reproduce [`render_item`]'s canonical spacing (byte-identical on a
/// comment-free item, pinned by the fmt corpus), a comment gets one
/// space before it and one after unless a `,`/`)` follows, and a LINE
/// comment — which nothing can follow on its physical line — continues
/// the item on the next line at `cont_col`.
fn render_item_tokens(node: &SyntaxNode, cont_col: usize) -> String {
    let mut out = String::new();
    let mut prev: Option<String> = None; // last emitted SIGNIFICANT text
    let mut after_comment = false;
    let mut at_line_start = false;
    for t in node.descendant_tokens() {
        if trivia::is_ws(t.kind()) {
            continue;
        }
        if trivia::is_comment(t.kind()) {
            if !at_line_start {
                out.push(' ');
            }
            out.push_str(&normalize_comment_text(t.text()));
            if is_line_comment(&t) {
                out.push('\n');
                out.push_str(&" ".repeat(cont_col));
                at_line_start = true;
            } else {
                at_line_start = false;
            }
            after_comment = !at_line_start;
            continue;
        }
        let text = t.text();
        let space = if at_line_start {
            false
        } else if after_comment {
            !matches!(text, "," | ")")
        } else {
            matches!(prev.as_deref(), Some(",") | Some("goto"))
        };
        if space {
            out.push(' ');
        }
        out.push_str(text);
        prev = Some(text.to_string());
        after_comment = false;
        at_line_start = false;
    }
    out
}

/// Canonical item text — ported unchanged from C1's `render_item`
/// (reads only [`Item`], the same [`crate::parser`] type on both sides).
/// A number's WRITTEN spelling is the one thing NOT canonicalized here,
/// same discipline as everywhere else in this module: emitted verbatim
/// via `succ_label_written`/`marked_written`/`blank_written`/`label_written`.
fn render_item(item: &Item) -> String {
    match item {
        Item::Builtin {
            which,
            succ,
            succ_label_written,
            ..
        } => {
            format!(
                "{}{}",
                builtin_name(*which),
                render_builtin_successor(*succ, succ_label_written.as_deref())
            )
        }
        Item::Debugger { .. } => "debugger".to_string(),
        Item::Call {
            name,
            succ,
            succ_label_written,
            ..
        } => format!(
            "@{name}({})",
            render_successor(*succ, succ_label_written.as_deref())
        ),
        Item::Check {
            marked,
            blank,
            marked_written,
            blank_written,
            ..
        } => {
            format!(
                "check({}, {})",
                render_check_arm(*marked, marked_written.as_deref()),
                render_check_arm(*blank, blank_written.as_deref())
            )
        }
        Item::Halt { .. } => "halt".to_string(),
        Item::Goto { label_written, .. } => format!("goto {label_written}"),
    }
}

/// Ported unchanged from C1's `builtin_name`.
fn builtin_name(which: Builtin) -> &'static str {
    match which {
        Builtin::Left => "left",
        Builtin::Right => "right",
        Builtin::Mark => "mark",
        Builtin::Unmark => "unmark",
    }
}

/// Ported unchanged from C1's `render_builtin_successor`.
fn render_builtin_successor(succ: Successor, written: Option<&str>) -> String {
    match succ {
        Successor::FallThrough => String::new(),
        _ => format!("({})", render_successor(succ, written)),
    }
}

/// Ported unchanged from C1's `render_successor`.
fn render_successor(succ: Successor, written: Option<&str>) -> String {
    match succ {
        Successor::FallThrough => String::new(),
        Successor::Label(_) => written
            .expect("succ_label_written is Some whenever succ is Successor::Label")
            .to_string(),
        Successor::Return => "!".to_string(),
    }
}

/// Ported unchanged from C1's `render_check_arm`.
fn render_check_arm(arm: CheckArm, written: Option<&str>) -> String {
    match arm {
        CheckArm::Label(_) => written
            .expect("marked_written/blank_written is Some whenever the arm is CheckArm::Label")
            .to_string(),
        CheckArm::Return => "!".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Pure alignment arithmetic ---------------------------------------
    //
    // Moved here with `command_column` itself when the CST printer was
    // deleted; the rest of that printer's tests drive `format` and stayed
    // in [`super`].

    #[test]
    fn command_column_worked_values() {
        // P=0 (no labels): base indent alone.
        assert_eq!(command_column(0, 4), 4);
        // P=2 (`1:`): max(4, 4) = 4.
        assert_eq!(command_column(2, 4), 4);
        // P=6 (`11111:`): max(4, 8) = 8.
        assert_eq!(command_column(6, 4), 8);
        // P=3 (`12:`): max(4, 5) = 5, rounded up to 8.
        assert_eq!(command_column(3, 4), 8);
        // P=5, stacked labels (`1: 2:`): max(4, 7) = 7, rounded up to 8.
        assert_eq!(command_column(5, 4), 8);
    }

    #[test]
    fn command_column_namespaced_base_indent() {
        // base_body_indent 8 (one level deeper: a namespaced or nested
        // body). No label wide enough to push past it.
        assert_eq!(command_column(0, 8), 8);
        assert_eq!(command_column(2, 8), 8);
        // P=10 pushes past the deeper base: max(8, 12) = 12, already a
        // multiple of 4.
        assert_eq!(command_column(10, 8), 12);
    }

    /// The assertion every fixture in this module runs on: `src` prints
    /// to exactly `expected`.
    ///
    /// Each `expected` literal here was **captured from the C1 printer**
    /// — mechanically, one file per call site, while `fmt::format` still
    /// ran `parse_cst` + `print_cst` — and the converted suite was proven
    /// green before the C1 printer was deleted. They are therefore pins
    /// on the behavior this migration had to preserve, not restatements
    /// of whatever the green printer happens to do.
    ///
    /// Commit `63275fc` is the last one where that printer existed, so
    /// the capture is reproducible rather than merely asserted: check it
    /// out, feed it any `src` below, and its output is the `expected`
    /// beside it. Fixtures added since (and any whose expected value a
    /// deliberate behaviour change moves) say so at their own site.
    #[track_caller]
    fn formats_to(src: &str, expected: &str) {
        let out = format(src).expect("the printer accepts it");
        assert_eq!(out, expected, "output diverged for:\n{src}");
    }

    /// [`formats_to`] plus the fixed-point half: `expected` must also
    /// format to itself. A one-pass assertion cannot see a printer that
    /// GROWS its output — a comment emitted one extra time per pass reads
    /// as correct on pass one and corrupts the file on pass two — and
    /// `pmt fmt PATH` rewrites in place, so every pass is one a user
    /// actually gets (`docs/pmt/fmt.md` (`--check`, stdin, and exit
    /// codes)). Use this for any fixture whose shape decides where a
    /// comment prints.
    #[track_caller]
    fn formats_to_fixed_point(src: &str, expected: &str) {
        formats_to(src, expected);
        formats_to(expected, expected);
    }

    #[test]
    fn empty_and_whitespace_only_files() {
        formats_to("", "\n");
        formats_to("\n", "\n");
        formats_to("\n\n\n", "\n");
    }

    #[test]
    fn a_single_use_declaration() {
        formats_to("use std::goToEnd;\n", "use std::goToEnd;\n");
        formats_to("use   std::goToEnd  ;\n", "use std::goToEnd;\n");
        formats_to("use std::goToEnd as far;\n", "use std::goToEnd as far;\n");
    }

    /// A comment lexed inside a `USE_PATH` node (`std::/* c */goToEnd`)
    /// prints in place inside the path ([`render_use_path`]'s
    /// token-splicing branch, the never-move rule) — between the `::`
    /// and the segment it was written before.
    #[test]
    fn a_comment_nested_inside_a_use_path_prints_in_place() {
        formats_to("use std::/* c */goToEnd;\n", "use std:: /* c */ goToEnd;\n");
        formats_to(
            "use std::/* c */goToEnd, std::goToBegin;\n",
            "use std:: /* c */ goToEnd, std::goToBegin;\n",
        );
    }

    #[test]
    fn a_multi_path_use_declaration() {
        formats_to(
            "use std::goToEnd, std::goToBegin;\n",
            "use std::goToEnd, std::goToBegin;\n",
        );
        formats_to(
            "use std::goToEnd,\n    std::goToBegin as backToStart;\n",
            "use std::goToEnd, std::goToBegin as backToStart;\n",
        );
    }

    #[test]
    fn standalone_comments_between_declarations() {
        formats_to(
            "// leading\nuse std::goToEnd;\n",
            "// leading\nuse std::goToEnd;\n",
        );
        formats_to(
            "// far\n\n// near\nuse std::goToEnd;\n",
            "// far\n\n// near\nuse std::goToEnd;\n",
        );
        formats_to(
            "use std::goToEnd;\n\n// trailing file comment\n",
            "use std::goToEnd;\n\n// trailing file comment\n",
        );
        // A same-line trailing comment on one `use` immediately followed
        // (no blank line) by another `use`: `leading_comments` walks
        // back through every raw token until a NODE sibling, so without
        // the `consumed`/`claimed` split in `print_items` this same
        // comment token surfaces as BOTH the first use's trailing
        // comment AND the second use's leading run, printing it twice.
        formats_to(
            "use std::goToEnd; // t\nuse std::goToBegin;\n",
            "use std::goToEnd; // t\nuse std::goToBegin;\n",
        );
        // The same shape, but with a blank line and a genuine leading
        // comment of its own between the trailing comment and the next
        // `use` — the blank line already cuts `leading_comments`' run
        // before it reaches the trailing comment, so this pins that the
        // fix above doesn't disturb the already-correct case.
        formats_to(
            "use std::goToEnd; // t\n\n// lead\nuse std::goToBegin;\n",
            "use std::goToEnd; // t\n\n// lead\nuse std::goToBegin;\n",
        );
    }

    #[test]
    fn an_empty_namespace() {
        formats_to("namespace n {\n}\n", "namespace n {\n}\n");
        formats_to("namespace n { // open\n}\n", "namespace n { // open\n}\n");
        formats_to("namespace n {\n} // close\n", "namespace n {\n} // close\n");
    }

    #[test]
    fn nested_and_reopened_namespaces() {
        formats_to(
            "namespace a {\n    namespace b {\n    }\n}\n",
            "namespace a {\n    namespace b {\n    }\n}\n",
        );
        formats_to(
            "namespace a {\n}\n\nnamespace a {\n}\n",
            "namespace a {\n}\n\nnamespace a {\n}\n",
        );
        // Same overlap as the `use`-trailing-comment case above, one
        // level up: a namespace's same-line close-brace comment,
        // immediately followed (no blank line) by another namespace,
        // would otherwise also print as that next namespace's leading
        // comment.
        formats_to(
            "namespace a {\n} // close\nnamespace b {\n}\n",
            "namespace a {\n} // close\nnamespace b {\n}\n",
        );
    }

    #[test]
    fn a_minimal_function() {
        formats_to("main() {\n 1: left;\n}\n", "main() {\n 1: left;\n}\n");
        formats_to("main(){1:left;}\n", "main() {\n 1: left;\n}\n");
        formats_to("main() {\n}\n", "main() {\n}\n");
    }

    #[test]
    fn function_header_modifiers() {
        formats_to(
            "volatile main() {\n 1: left;\n}\n",
            "volatile main() {\n 1: left;\n}\n",
        );
        formats_to(
            "export helper() {\n 1: left;\n}\n",
            "export helper() {\n 1: left;\n}\n",
        );
    }

    #[test]
    fn a_doc_run_binds_to_its_function() {
        formats_to(
            "? doc line\nmain() {\n 1: left;\n}\n",
            "? doc line\nmain() {\n 1: left;\n}\n",
        );
        formats_to(
            "? one\n? two\n! attention\nmain() {\n 1: left;\n}\n",
            "? one\n? two\n! attention\nmain() {\n 1: left;\n}\n",
        );
        formats_to(
            "? doc\n\nmain() {\n 1: left;\n}\n",
            "? doc\n\nmain() {\n 1: left;\n}\n",
        );
    }

    #[test]
    fn nested_functions() {
        formats_to(
            "main() {\n    step() {\n 1: left;\n    }\n\n    @step();\n}\n",
            "main() {\n    step() {\n     1: left;\n    }\n\n    @step();\n}\n",
        );
    }

    /// A nested function BETWEEN two statements. This is the fixture that
    /// catches the one Task 3 mistake byte-identity would otherwise only
    /// surface at Task 7, as a corpus-wide diff to bisect: building body
    /// order from `statements()` and `nested()` separately instead of from
    /// `children_with_tokens()` hoists the nested function out of place,
    /// and every other nested-function fixture happens to put it first.
    #[test]
    fn a_nested_function_between_statements_keeps_its_position() {
        formats_to(
            "main() {\n 1: left;\n    step() {\n 1: left;\n    }\n 2: left;\n}\n",
            "main() {\n 1: left;\n    step() {\n     1: left;\n    }\n 2: left;\n}\n",
        );
        formats_to(
            "main() {\n 1: left;\n\n    step() {\n 1: left;\n    }\n\n 2: left;\n}\n",
            "main() {\n 1: left;\n\n    step() {\n     1: left;\n    }\n\n 2: left;\n}\n",
        );
    }

    #[test]
    fn labels_stacked_and_own_line() {
        formats_to(
            "main() {\n 1: 2: right, mark;\n 3: left;\n}\n",
            "main() {\n  1: 2: right, mark;\n     3: left;\n}\n",
        );
        formats_to(
            "main() {\n 1:\n    left;\n}\n",
            "main() {\n 1:\n    left;\n}\n",
        );
        formats_to(
            "main() {\n 1: left;\n 10: left;\n 100: left;\n}\n",
            "main() {\n     1: left;\n    10: left;\n   100: left;\n}\n",
        );
    }

    #[test]
    fn statement_shapes() {
        formats_to(
            "main() {\n 1: check(1, 2);\n 2: goto 1;\n 3: halt;\n}\n",
            "main() {\n 1: check(1, 2);\n 2: goto 1;\n 3: halt;\n}\n",
        );
        formats_to(
            "main() {\n 1: @callee();\n 2: @callee(!);\n 3: debugger;\n}\n",
            "main() {\n 1: @callee();\n 2: @callee(!);\n 3: debugger;\n}\n",
        );
        formats_to(
            "main() {\n 1: left, right, mark, unmark, left, right, mark, unmark, left;\n}\n",
            "main() {\n 1: left, right, mark, unmark, left, right, mark, unmark, left;\n}\n",
        );
    }

    /// `render_items`' two group-boundary paths, neither reached by
    /// `statement_shapes` above: an author line break with NO comment
    /// involved at all (`newline_before`'s own group-split branch,
    /// independent of any comment machinery — the same source
    /// `trivia::label_break`'s own `label_break_detects_inline_label`
    /// test uses, confirming it parses), and a comma group wide enough
    /// to cross the 80-column limit at `command_col` and fall through to
    /// `greedy_fill_group`'s own wrapping.
    #[test]
    fn comma_group_layout() {
        formats_to(
            "main() {\n 1: left,\n    right;\n}\n",
            "main() {\n 1: left,\n    right;\n}\n",
        );
        formats_to(
            "main() {\n 1: unmark, unmark, unmark, unmark, unmark, unmark, unmark, unmark, \
             unmark, unmark;\n}\n",
            "main() {\n 1: unmark, unmark, unmark, unmark, unmark, unmark, unmark, unmark, unmark,\n    unmark;\n}\n",
        );
    }

    #[test]
    fn blank_lines_between_body_items() {
        formats_to(
            "main() {\n 1: left;\n\n 2: left;\n}\n",
            "main() {\n 1: left;\n\n 2: left;\n}\n",
        );
        formats_to(
            "main() {\n 1: left;\n\n\n\n 2: left;\n}\n",
            "main() {\n 1: left;\n\n 2: left;\n}\n",
        );
    }

    #[test]
    fn a_single_trailing_comment() {
        formats_to(
            "main() {\n 1: left; // note\n}\n",
            "main() {\n 1: left; // note\n}\n",
        );
        formats_to(
            "main() {\n 1: left;    // over-indented note\n}\n",
            "main() {\n 1: left; // over-indented note\n}\n",
        );
    }

    /// The task-5 brief's own fixture set for run boundaries. Kept
    /// byte-for-byte as specified, but NONE of these four sources has its
    /// trailing `//`s sitting at a common source column (confirmed by
    /// running each through the real C1 formatter before writing this
    /// comment), so `compute_trailing_spacing`'s `aligned` check is
    /// `false` in every one of them and every statement here — including
    /// runs of 2 and 3 — falls back to the one-space default. That makes
    /// this test a REAL but WEAK proof: it pins boundary detection
    /// (`has_trailing`/the run-length split) without ever exercising the
    /// `align_col - code_w[off]` arithmetic, `overflow`, or the
    /// non-overflow column filter. Those branches get their own,
    /// deliberately column-equal fixtures below
    /// (`an_aligned_run_shares_the_widest_reformatted_line` on down).
    #[test]
    fn an_alignment_run_and_its_boundaries() {
        // Three commented statements in a row, source columns unequal
        // (11/12/11) — a run of >= 2, but ragged, so all three fall back
        // to a single space each.
        formats_to(
            "main() {\n 1: left; // a\n 2: right; // b\n 3: mark; // c\n}\n",
            "main() {\n 1: left; // a\n 2: right; // b\n 3: mark; // c\n}\n",
        );
        // An uncommented statement in the middle ends the run after
        // statement 1 and starts a new one at statement 3 — TWO lone
        // runs, not one of length 2 skipping the gap. Proven by the
        // column-equal counterpart below
        // (`a_run_only_ever_spans_consecutive_members`), where a wrong
        // "skip the gap" rule would visibly misalign 1 and 3.
        formats_to(
            "main() {\n 1: left; // a\n 2: right;\n 3: mark; // c\n}\n",
            "main() {\n 1: left; // a\n 2: right;\n 3: mark; // c\n}\n",
        );
        // A blank line before statement 2 ends the run after statement 1:
        // `!body[j].blank_before` is one of the two conditions extending a
        // run (`has_trailing` is the other), so statement 2 starts its own
        // lone run rather than joining statement 1's. With only two
        // statements, both readings (one broken run vs. two lone ones)
        // print identically (single space each) — the column-equal
        // counterpart below (`a_blank_line_ends_a_run`) is where the two
        // readings diverge visibly.
        formats_to(
            "main() {\n 1: left; // a\n\n 2: right; // b\n}\n",
            "main() {\n 1: left; // a\n\n 2: right; // b\n}\n",
        );
        // A statement long enough to push its own comment past the run's
        // column would decide the run's column for everyone IF this run
        // were aligned — it isn't (columns 11 vs 44), so both statements
        // still get one space. The real "decides the column" case is
        // `an_aligned_run_shares_the_widest_reformatted_line` below.
        formats_to(
            "main() {\n 1: left; // a\n 2: left, right, mark, unmark, left, right; // b\n}\n",
            "main() {\n 1: left; // a\n 2: left, right, mark, unmark, left, right; // b\n}\n",
        );
    }

    /// `align_col = max_code_w + 1`, then `spacing[off] = align_col -
    /// code_w[off]` for every non-overflowing member: two statements
    /// whose SOURCE `//` columns are hand-padded equal (both at column
    /// 17) but whose reformatted code widths differ (`left;` vs.
    /// `left, right;`) — verified against the real C1 formatter before
    /// writing this fixture. A mutant collapsing that arithmetic to the
    /// lone-run default `1` turns this red: the wider statement's own
    /// comment stays at one space (`align_col - code_w == 1` there too),
    /// but the narrower one's would drop from two spaces to one.
    #[test]
    fn an_aligned_run_shares_the_widest_reformatted_line() {
        formats_to(
            "main() {\n 1: left;        // a\n 2: left, right; // b\n}\n",
            "main() {\n 1: left;        // a\n 2: left, right; // b\n}\n",
        );
    }

    /// The column-equal counterpart to `an_alignment_run_and_its_boundaries`'s
    /// blank-line case: statements 1 and 3 share source column 17, with
    /// statement 2 (also at column 17) between them. `!body[j].blank_before`
    /// stops statement 2 from joining statement 1's run — the blank line
    /// sits immediately before it — so it starts a NEW run with statement
    /// 3, and 1 stays a lone run at the default one space, EVEN THOUGH its
    /// column matches. A mutant that let a blank line pass through would
    /// merge all three into one aligned run instead, visibly widening
    /// statement 1's spacing to match 2 and 3.
    #[test]
    fn a_blank_line_ends_a_run() {
        formats_to(
            "main() {\n 1: left;        // a\n\n 2: left, right; // b\n 3: mark;        // c\n}\n",
            "main() {\n 1: left; // a\n\n 2: left, right; // b\n 3: mark;        // c\n}\n",
        );
    }

    /// The column-equal counterpart to `an_alignment_run_and_its_boundaries`'s
    /// middle case: statements 1 and 3 again share column 17, with
    /// statement 2 (uncommented) between them. `has_trailing(body[2])` is
    /// false, so the run scan restarts at statement 3 rather than
    /// extending across the gap — statement 1 and statement 3 are each a
    /// lone run, both at the default one space, DESPITE sharing a column.
    /// A mutant that skipped over a non-trailing member instead of ending
    /// the run there would merge 1 and 3 into one run and align them.
    #[test]
    fn a_run_only_ever_spans_consecutive_members() {
        formats_to(
            "main() {\n 1: left;        // a\n 2: right;\n 3: mark;        // c\n}\n",
            "main() {\n 1: left; // a\n 2: right;\n 3: mark; // c\n}\n",
        );
    }

    /// The design doc's own worked example (`docs/pmt/fmt.md` (comments)):
    /// three commented statements where 1 and 2 share a source column and
    /// 3 does not, AND 3's comment is long enough to overflow 80 columns
    /// at the run's aligned column. Proves two branches at once: `overflow`
    /// (statement 3 falls back to one space instead of joining the
    /// alignment attempt) and the `non_overflow_cols` filter (statement
    /// 3's mismatched column is excluded from the aligned/ragged verdict,
    /// so 1 and 2 stay aligned instead of the whole run going ragged). The
    /// doc's own claim that the header line is invisible to this
    /// computation is pinned by the `volatile` variant. This source is
    /// already in the toolchain's canonical form (idempotence check: a
    /// second `pmt fmt` pass reproduces it byte for byte), confirmed
    /// against the real C1 formatter before writing this fixture.
    #[test]
    fn the_overflow_fallback_is_excluded_from_the_aligned_verdict() {
        formats_to(
            "main() {\n 1: right;       // a\n    check(1, 3); // b\n 3: left; // a comment \
             long enough that keeping it aligned would overflow eighty columns\n}\n",
            "main() {\n 1: right;       // a\n    check(1, 3); // b\n 3: left; // a comment long enough that keeping it aligned would overflow eighty columns\n}\n",
        );
        formats_to(
            "volatile main() {\n 1: right;       // a\n    check(1, 3); // b\n 3: left; // a \
             comment long enough that keeping it aligned would overflow eighty columns\n}\n",
            "volatile main() {\n 1: right;       // a\n    check(1, 3); // b\n 3: left; // a comment long enough that keeping it aligned would overflow eighty columns\n}\n",
        );
    }

    /// `code_line_width_incl_semi` measures only the code's LAST physical
    /// line — a statement with an own-line label spreads its code across
    /// two lines (`999999999:` then `    left`), and only the second one
    /// rides the `;`. Paired at an equal source column (17) with a short
    /// inline-labeled statement, verified against the real C1 formatter:
    /// a mutant measuring the WHOLE code string (own-line label included)
    /// would compute a much wider `align_col`, visibly over-padding
    /// statement 2's comment instead of the correct two-space/one-space
    /// split this fixture pins.
    #[test]
    fn alignment_measures_only_the_codes_last_line() {
        formats_to(
            "main() {\n 999999999:\n    left;    // a\n 2: right;   // b\n}\n",
            "main() {\n 999999999:\n    left;  // a\n 2: right; // b\n}\n",
        );
    }

    /// `comment_w` is measured on [`normalize_comment_text`]'s
    /// output, not the raw token — a raw trailing run of spaces before
    /// the newline must not inflate the overflow math for width nothing
    /// will actually print. Statement 1's comment carries 60 core
    /// characters plus 12 raw trailing spaces: `align_col` (11) plus the
    /// NORMALIZED width (63) is 74, under 80 — no overflow, stays
    /// aligned; `align_col` plus the RAW width (75) is 86, over 80 — a
    /// mutant reading the raw token width would wrongly fall statement 1
    /// back to one space instead of the two this fixture pins. Verified
    /// against the real C1 formatter before writing this fixture.
    #[test]
    fn normalized_width_not_raw_drives_the_overflow_check() {
        formats_to(
            "main() {\n 1: left;  // xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx  \
             \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\n 2: right; // b\n}\n",
            "main() {\n 1: left;  // xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n 2: right; // b\n}\n",
        );
    }

    /// The overflow test itself, `align_col + comment_w[off] >
    /// LINE_WIDTH`: exactly column 80 is NOT overflow (`>`, not `>=`),
    /// 81 is. Two statements share a source column (11 pre-comment);
    /// statement 1's comment is sized so `align_col (11) + comment_w`
    /// lands EXACTLY on 80 in the first source, 81 in the second —
    /// verified against the real C1 formatter before writing this
    /// fixture. Statement 2's own natural aligned spacing is 1 either
    /// way (its code is one character wider than statement 1's), so
    /// statement 1 is the only place a `>` vs. `>=` mutant is visible:
    /// two spaces at exactly 80, one space (fallen back) at 81.
    #[test]
    fn the_overflow_boundary_is_strictly_greater_than_80() {
        formats_to(
            "main() {\n 1: left;  // xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n 2: right; // b\n}\n",
            "main() {\n 1: left;  // xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n 2: right; // b\n}\n",
        );
        formats_to(
            "main() {\n 1: left;  // xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n 2: right; // b\n}\n",
            "main() {\n 1: left; // xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n 2: right; // b\n}\n",
        );
    }

    /// `aligned` requires EVERY consecutive pair of non-overflowing
    /// columns to match (`.all`), not merely some pair (`.any`):
    /// statements 1 and 2 share a source column, statement 3 sits at a
    /// different one, none overflow. A `.any`-style check would still
    /// call this "aligned" off the matching (1, 2) pair alone and hand
    /// every member its own `align_col - code_w[off]` — verified against
    /// the real C1 formatter, which instead falls the whole run back to
    /// one space per comment, since NOT every member shares the column.
    #[test]
    fn aligned_requires_every_pair_to_match_not_just_one() {
        formats_to(
            "main() {\n 1: left;  // a\n 2: right; // b\n 3: mark;       // c\n}\n",
            "main() {\n 1: left; // a\n 2: right; // b\n 3: mark; // c\n}\n",
        );
    }

    #[test]
    fn trailing_comments_on_declarations() {
        formats_to("use std::goToEnd; // note\n", "use std::goToEnd; // note\n");
        formats_to(
            "main() {\n 1: left;\n} // after the brace\n",
            "main() {\n 1: left;\n} // after the brace\n",
        );
        formats_to(
            "namespace n {\n} // after the namespace\n",
            "namespace n {\n} // after the namespace\n",
        );
    }

    #[test]
    fn an_own_line_comment_is_not_a_trailing_one() {
        formats_to(
            "main() {\n 1: left;\n // own line\n 2: left; // trailing\n}\n",
            "main() {\n 1: left;\n    // own line\n 2: left; // trailing\n}\n",
        );
    }

    /// A statement's trailing comment, immediately followed (same
    /// non-blank run of tokens) by a genuine own-line comment on its own
    /// line, before the next statement: both print, in order. This
    /// fixture alone does NOT distinguish the collection loop's
    /// `already_trailing` identity check (`&tc == t`) from a weaker
    /// `.is_some()` — statement 2 carries no trailing comment of its own,
    /// so [`trivia::leading_comments`]' independent backward walk off
    /// statement 2 finds and prints "// own line after trailing" via a
    /// DIFFERENT path regardless of whether it also survives into
    /// `body` as its own element; see
    /// `an_own_line_comment_wrongly_treated_as_trailing_corrupts_run_adjacency`
    /// below for the fixture that DOES distinguish them.
    #[test]
    fn a_trailing_comment_does_not_swallow_the_next_own_line_comment() {
        formats_to(
            "main() {\n 1: left; // t\n // own line after trailing\n 2: right;\n}\n",
            "main() {\n 1: left; // t\n    // own line after trailing\n 2: right;\n}\n",
        );
    }

    /// The discriminating counterpart: TWO commented statements (both
    /// run candidates, sharing a source column), separated by a genuine
    /// own-line comment. `already_trailing`'s identity check correctly
    /// keeps that own-line comment as its own `BodyElem::Comment`, which
    /// ends the run between the two statements (`has_trailing` is false
    /// for a `Comment` element) — both statements fall back to the
    /// default one space, matching the real C1 formatter (verified
    /// before writing this fixture). A mutant weakening the check to
    /// `.is_some()` (statement 1 has SOME trailing comment on record,
    /// regardless of identity) would wrongly omit the own-line comment
    /// from `body` entirely, making the two statements look ADJACENT to
    /// the run scan and aligning them — a visible column shift.
    #[test]
    fn an_own_line_comment_wrongly_treated_as_trailing_corrupts_run_adjacency() {
        formats_to(
            "main() {\n 1: left;    // a\n // own line\n 2: right;   // b\n}\n",
            "main() {\n 1: left; // a\n    // own line\n 2: right; // b\n}\n",
        );
    }

    /// The two `unreachable!` coverage stubs this list used to pin —
    /// a comma group's own interior comment and a function's open-brace
    /// comment — are GONE, replaced by real handling this task
    /// implements (see `an_alignment_run_and_its_boundaries` and
    /// `trailing_comments_on_declarations` above for the two stubs
    /// removed a task earlier). A BLOCK comment mid-group with no LINE
    /// comment among it/its siblings ([`layout_leading`]'s `None` arm)
    /// stays INLINE rather than forcing a break — the counterpart this
    /// module's own `interior_comments_between_comma_items` test (a LINE
    /// comment, `Some` arm) doesn't reach.
    #[test]
    fn interior_comma_block_comment_stays_inline() {
        formats_to(
            "main() {\n 1: left, /* x */ right;\n}\n",
            "main() {\n 1: left, /* x */ right;\n}\n",
        );
    }

    #[test]
    fn function_open_brace_comment_rides_the_header_line() {
        formats_to(
            "main() { // open\n 1: left;\n}\n",
            "main() { // open\n 1: left;\n}\n",
        );
    }

    /// The middle fixture is also this task's DOUBLE-PRINT guard: `//
    /// between` is BOTH statement 2's own leading run
    /// ([`trivia::leading_comments`]) AND a raw comment token
    /// `print_body`'s own element walk reaches directly — mutating
    /// `claimed.contains(tok)` to always `false` (skipping the check)
    /// makes this exact fixture the one that turns red: C1 printed
    /// `// between` once, and an un-filtered green walk prints it
    /// twice.
    #[test]
    fn own_line_comments_inside_a_body() {
        formats_to(
            "main() {\n    // leading\n 1: left;\n}\n",
            "main() {\n    // leading\n 1: left;\n}\n",
        );
        formats_to(
            "main() {\n 1: left;\n    // between\n 2: left;\n}\n",
            "main() {\n 1: left;\n    // between\n 2: left;\n}\n",
        );
        formats_to(
            "main() {\n 1: left;\n    // trailing the body\n}\n",
            "main() {\n 1: left;\n    // trailing the body\n}\n",
        );
        // A standalone (unclaimed) comment with a blank line before it —
        // `blank_immediately_before(tok)`'s own branch in `print_body`'s
        // Comment arm, not `blank_before_unit`'s (that one only fires for
        // a NODE, and this comment has no following node to attach to).
        formats_to(
            "main() {\n 1: left;\n\n    // standalone with blank\n}\n",
            "main() {\n 1: left;\n\n    // standalone with blank\n}\n",
        );
    }

    #[test]
    fn a_comment_run_keeps_its_internal_gap() {
        formats_to(
            "main() {\n    // far\n\n    // near\n 1: left;\n}\n",
            "main() {\n    // far\n\n    // near\n 1: left;\n}\n",
        );
    }

    #[test]
    fn block_comments() {
        formats_to(
            "main() {\n    /* one line */\n 1: left;\n}\n",
            "main() {\n    /* one line */\n 1: left;\n}\n",
        );
        formats_to(
            "main() {\n    /* a block comment\n       spanning two lines */\n 1: left;\n}\n",
            "main() {\n    /* a block comment\n       spanning two lines */\n 1: left;\n}\n",
        );
        // Empty comments — `normalize_comment_text`'s per-line `trim_end`
        // has nothing to trim either way, but these are real lexed
        // shapes, not merely untested `text` values.
        formats_to(
            "main() {\n    //\n 1: left;\n}\n",
            "main() {\n    //\n 1: left;\n}\n",
        );
        formats_to(
            "main() {\n    /**/\n 1: left;\n}\n",
            "main() {\n    /**/\n 1: left;\n}\n",
        );
    }

    #[test]
    fn comments_around_a_nested_function() {
        formats_to(
            "main() {\n    // about step\n    step() {\n 1: left;\n    }\n\n    @step();\n}\n",
            "main() {\n    // about step\n    step() {\n     1: left;\n    }\n\n    @step();\n}\n",
        );
    }

    /// A nested `FUNCTION`'s own close-brace comment, immediately
    /// followed (no blank line) by a statement at the OUTER body's own
    /// level: the same double-print shape `standalone_comments_between_declarations`
    /// pins at the top level, one level down — the nested function's
    /// `}`-comment is `print_body`'s own `consumed` set (built from
    /// `body_elem_node`, which covers `BodyElem::Nested`, not just
    /// `Statement`), so it does not ALSO reprint as statement 2's own
    /// leading run.
    #[test]
    fn a_nested_functions_close_brace_comment_does_not_double_print() {
        formats_to(
            "main() {\n    step() {\n 1: left;\n    } // after nested\n 2: left;\n}\n",
            "main() {\n    step() {\n     1: left;\n    } // after nested\n 2: left;\n}\n",
        );
    }

    /// The doc-run-comment ruling's own surface (module doc's second
    /// bullet): a comment interleaved BETWEEN two run lines
    /// ([`print_doc_run`]'s own loop) and one sitting AFTER the run's
    /// last line, before the bound declaration
    /// ([`doc_run_trailing_comments`] — a real green-tree shape with no
    /// direct C1-CST field, confirmed against `debug_dump` before
    /// writing this test: such a comment is `DOC_RUN`'s own NEXT
    /// SIBLING inside `FUNCTION`, never one of `DOC_RUN`'s children).
    /// Both parse and format identically today, verified against the
    /// real C1 formatter before this fixture was written.
    #[test]
    fn doc_run_interior_and_trailing_comments() {
        // Between two doc lines — a DOC_RUN child.
        formats_to(
            "? one\n// interloper\n? two\nmain() {\n 1: left;\n}\n",
            "? one\n// interloper\n? two\nmain() {\n 1: left;\n}\n",
        );
        // After a gap following the run's only line — DOC_RUN's own
        // next sibling, not a child.
        formats_to(
            "? one\n\n// after a gap\nmain() {\n 1: left;\n}\n",
            "? one\n\n// after a gap\nmain() {\n 1: left;\n}\n",
        );
        // Directly between the run's last line and the declaration,
        // no blank either side.
        formats_to(
            "? one\n// trailing before fn\nmain() {\n 1: left;\n}\n",
            "? one\n// trailing before fn\nmain() {\n 1: left;\n}\n",
        );
        // The same shape, with a blank line before the trailing comment.
        formats_to(
            "? one\n\n// trailing before fn\nmain() {\n 1: left;\n}\n",
            "? one\n\n// trailing before fn\nmain() {\n 1: left;\n}\n",
        );
        // More than one trailing comment in a row, still DOC_RUN's own
        // siblings, not its children.
        formats_to(
            "? one\n// c1\n// c2\nmain() {\n 1: left;\n}\n",
            "? one\n// c1\n// c2\nmain() {\n 1: left;\n}\n",
        );
        // A blank line AFTER the trailing comment, before the header —
        // `blank_before_header`'s own branch, computed off the LAST
        // trailing comment rather than off `DOC_RUN` itself.
        formats_to(
            "? one\n// c1\n\nmain() {\n 1: left;\n}\n",
            "? one\n// c1\n\nmain() {\n 1: left;\n}\n",
        );
        // A blank line BETWEEN two run items, INSIDE `print_doc_run`'s
        // own loop — distinct from every gap above, which all sit
        // outside the run (before it, or after its last line). One doc
        // line's own kind, one comment token's, so both branches of the
        // dispatch below the blank check get their own blank-before
        // proof.
        formats_to(
            "? one\n\n? two\nmain() {\n 1: left;\n}\n",
            "? one\n\n? two\nmain() {\n 1: left;\n}\n",
        );
        formats_to(
            "? one\n\n// mid\n? two\nmain() {\n 1: left;\n}\n",
            "? one\n\n// mid\n? two\nmain() {\n 1: left;\n}\n",
        );
    }

    /// A comment immediately before a bound doc run — `leading_comments`
    /// on the `FUNCTION` node (which retro-wraps the run as its own
    /// first child) sees this comment as its OWN preceding sibling, one
    /// level up from anything [`print_doc_run`]/[`doc_run_trailing_comments`]
    /// walk. Exercises `print_body`'s `claimed` set against a
    /// doc-run-bound nested function, at body indent.
    #[test]
    fn a_comment_leads_a_bound_doc_run() {
        formats_to(
            "// lead\n? doc\nmain() {\n 1: left;\n}\n",
            "// lead\n? doc\nmain() {\n 1: left;\n}\n",
        );
        formats_to(
            "main() {\n    // about step\n    ? step doc\n    step() {\n 1: left;\n    }\n}\n",
            "main() {\n    // about step\n    ? step doc\n    step() {\n     1: left;\n    }\n}\n",
        );
    }

    // Ported branches no fixture above reaches. Each was once covered
    // only by a C1 unit test in `fmt/mod.rs`, which went away with the
    // printer it pinned; these are what pins them now. Every one names
    // the branch it holds down and why the fixtures above cannot, so a
    // mutation of that branch fails here by name.

    /// `command_column`'s `base_body_indent.max(p + 2)` only differs from
    /// a `p + 1` mutant at the residue class `p ≡ 3 (mod 4)`: `p = 3`
    /// (from a two-digit inline label `"12:"`) makes `p + 2 == 5` round
    /// up to command column 8, while `p + 1 == 4` would round up to 4
    /// instead — every other fixture in this module uses a `p` that
    /// rounds to the same multiple of 4 either way.
    #[test]
    fn command_column_rounds_up_from_the_plus_two_margin() {
        formats_to("main() {\n 12: left;\n}\n", "main() {\n    12: left;\n}\n");
    }

    /// A `?`/`!` line with an empty payload prints as the bare sigil — a
    /// real lexed shape (a doc paragraph break), not merely a `text`
    /// value this printer happens never to receive. C1 pinned the same
    /// shape in `fmt/mod.rs`'s `empty_doc_line_prints_bare_sigil_as_a_paragraph_break`.
    #[test]
    fn a_bare_doc_line_is_a_paragraph_break() {
        formats_to(
            "? one\n?\n? two\nmain() {\n 1: left;\n}\n",
            "? one\n?\n? two\nmain() {\n 1: left;\n}\n",
        );
    }

    /// `render_check_arm`'s `CheckArm::Return` arm (`check(..., !)`) —
    /// every other `check` fixture in this module uses two `Label` arms.
    #[test]
    fn check_arm_return() {
        formats_to(
            "main() {\n 1: check(!, 1);\n}\n",
            "main() {\n 1: check(!, 1);\n}\n",
        );
        formats_to(
            "main() {\n 1: check(!, !);\n}\n",
            "main() {\n 1: check(!, !);\n}\n",
        );
    }

    /// `render_builtin_successor`'s non-`FallThrough` branch (the
    /// `(...)` wrapping a written label or `!`) — no fixture above gives
    /// a builtin its own successor.
    #[test]
    fn a_builtin_with_a_successor() {
        formats_to("main() {\n 1: left(5);\n}\n", "main() {\n 1: left(5);\n}\n");
        formats_to("main() {\n 1: mark(!);\n}\n", "main() {\n 1: mark(!);\n}\n");
    }

    /// `label_margin`'s `None` arm, reached under `label_break` (an
    /// own-line label wide enough that even the strict 1-space margin
    /// doesn't fit) — the `labels_stacked_and_own_line` fixtures above
    /// only exercise the `Some` arm.
    #[test]
    fn label_margin_overflow_under_label_break() {
        formats_to(
            "main() {\n 999999999:\n    left;\n}\n",
            "main() {\n 999999999:\n    left;\n}\n",
        );
    }

    // -- Task 6: use-list interior comments ------------------------------

    #[test]
    fn interior_comments_between_use_paths() {
        formats_to(
            "use a::b, // note\n    c::d;\n",
            "use a::b, // note\n    c::d;\n",
        );
        formats_to(
            "use a::b,\n    // own line\n    c::d;\n",
            "use a::b,\n    // own line\n    c::d;\n",
        );
        formats_to(
            "use std::goToEnd,\n    // pulled in for the return leg\n    std::goToBegin as \
             backToStart;\n",
            "use std::goToEnd,\n    // pulled in for the return leg\n    std::goToBegin as backToStart;\n",
        );
    }

    /// The full slot inventory, ported from `tests/fmt_programs.rs`'
    /// dedicated (structural-assertion) fixtures — real, C1-verified
    /// sources, now checked byte-for-byte against C1 instead of just
    /// "the comment survives somewhere sane": slot 0 own-line and
    /// same-line (before the first path), between two entries, the tail
    /// slot own-line and same-line (the LINE-comment-would-swallow-`;`
    /// case), and a trailing comment that must NOT migrate into the
    /// following `use`.
    #[test]
    fn interior_use_slots_from_the_integration_fixture_set() {
        formats_to(
            "use std::goToEnd, // walk right\n    std::goToBegin;\n\nmain() {\n 1: \
             @goToEnd();\n 2: halt;\n}\n",
            "use std::goToEnd, // walk right\n    std::goToBegin;\n\nmain() {\n 1: @goToEnd();\n 2: halt;\n}\n",
        );
        formats_to(
            "use\n// note\na::b, c::d;\n",
            "use\n    // note\n    a::b,\n    c::d;\n",
        );
        formats_to(
            "use // note\na::b, c::d;\n",
            "use // note\n    a::b,\n    c::d;\n",
        );
        formats_to(
            "use a::b, // note\nc::d;\n",
            "use a::b, // note\n    c::d;\n",
        );
        formats_to(
            "use a::b, c::d\n// note\n;\n",
            "use a::b,\n    c::d\n    // note\n    ;\n",
        );
        formats_to("use a::b // note\n;\n", "use a::b // note\n    ;\n");
        formats_to(
            "use a::b;\n// the fallback path\nuse c::d;\n\nmain() {\n 1: @b();\n 2: \
                    halt;\n}\n",
            "use a::b;\n// the fallback path\nuse c::d;\n\nmain() {\n 1: @b();\n 2: halt;\n}\n",
        );
    }

    // -- Task 6: comma-group interior comments ---------------------------

    #[test]
    fn interior_comments_between_comma_items() {
        formats_to(
            "main() {\n 1: left, // note\n    right;\n}\n",
            "main() {\n 1: left, // note\n    right;\n}\n",
        );
        formats_to(
            "main() {\n 1: left,\n    // own line\n    right;\n}\n",
            "main() {\n 1: left, // own line\n    right;\n}\n",
        );
    }

    /// `render_items`' item-0 special case: a forcing LINE comment
    /// between an own-line label's `:` and the first command has no
    /// preceding `,` to attach to, so `layouts[0].forced_break` is
    /// handled BEFORE the main group loop even starts — the ONE branch
    /// `interior_comments_between_comma_items` above never reaches
    /// (both its fixtures force a break at item 1, inside the loop).
    /// Ported from C1's own `m3_item0_leading_line_comment_forces_a_comma_group_break`.
    #[test]
    fn a_forcing_comment_before_the_first_item_breaks_before_the_loop() {
        formats_to(
            "main() { 1: // c\n left, right; }\n",
            "main() {\n 1:\n    // c\n    left, right;\n}\n",
        );
    }

    /// The `gi > 0` forced-break branch, distinct from item 0's: a THIRD
    /// item continues the group a forced break started, rather than
    /// forcing a break of its own — pins that `layouts[i].forced_break`
    /// only starts a NEW group, it doesn't propagate to every later
    /// member.
    #[test]
    fn items_after_a_forced_break_continue_the_new_group_until_their_own_break() {
        formats_to(
            "main() {\n 1: left, // note\n    right, mark;\n}\n",
            "main() {\n 1: left, // note\n    right, mark;\n}\n",
        );
    }

    /// A LINE comment nested inside an item's own parens stays inside
    /// them ([`render_item_tokens`], the never-move rule): the item
    /// continues on the next line at the continuation column, and the
    /// following item rides the SAME group unchanged — the comment no
    /// longer reattributes to the next item's leading slot, so it forces
    /// no group break.
    #[test]
    fn a_comment_nested_inside_the_previous_item_stays_inside_it() {
        formats_to(
            "main() {\n 1: @f( // c\n), left;\n}\n",
            "main() {\n 1: @f( // c\n        ), left;\n}\n",
        );
    }

    /// `layout_leading`'s `pre_item_lines` — a comment AFTER the first
    /// LINE comment in one item's leading run (pathological, but still
    /// reprinted per fidelity-over-layout): the LINE comment becomes
    /// `break_inline`, everything after it becomes its own re-indented
    /// line ahead of the item, both on `emit_forced_break`'s own
    /// `for line in &layout.pre_item_lines` loop — no fixture above ever
    /// gives an item's leading run more than one comment.
    #[test]
    fn a_comment_after_the_forcing_line_comment_reprints_as_a_pre_item_line() {
        formats_to(
            "main() { left, // note\n /* extra */\n right; }\n",
            "main() {\n    left, // note\n    /* extra */\n    right;\n}\n",
        );
    }

    /// `layout_leading`'s `break_inline` loop over `leading[..break_pos]`
    /// — a BLOCK comment BEFORE the forcing LINE comment in one item's
    /// leading run, space-joined onto the SAME `break_inline` line. No
    /// fixture above gives an item's leading run a comment before its
    /// own forcing one (every other forced-break fixture has `break_pos
    /// == 0`).
    #[test]
    fn a_comment_before_the_forcing_line_comment_joins_its_break_inline_line() {
        formats_to(
            "main() { left, /* x */ // note\n right; }\n",
            "main() {\n    left, /* x */ // note\n    right;\n}\n",
        );
    }

    /// `line_width_after`'s `rsplit_once` branch and `greedy_fill_group`'s
    /// cursor math, ported unchanged from C1's own M2 regression
    /// (`m2_multiline_comment_greedy_fill_uses_last_line_width`): a
    /// mid-comma-group BLOCK comment spanning two physical source lines
    /// advances `newline_before`'s line-number comparison for the
    /// FOLLOWING item regardless of source formatting, and the cursor's
    /// TRUE resulting column is the width of the comment's own LAST
    /// physical line, not the naive sum of both lines' widths.
    #[test]
    fn multiline_leading_comment_uses_last_line_width_for_greedy_fill() {
        let comment = format!("/* {}\ny */", "x".repeat(70));
        formats_to(
            &format!("main() {{ left, {comment} right, mark, mark, mark, mark, mark; }}\n"),
            "main() {\n    left,\n    /* xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\ny */ right, mark, mark, mark, mark, mark;\n}\n",
        );
    }

    /// The control for the fixture above, ported from C1's own
    /// `m2_control_single_line_comment_unaffected`: the SAME shape
    /// collapsed to one physical line — no embedded `\n`, so
    /// `line_width_after`'s `rsplit_once` branch is never taken and the
    /// plain `base + chars(text)` arithmetic is exercised instead.
    #[test]
    fn control_single_line_leading_comment_is_unaffected_by_the_multiline_fix() {
        let comment = format!("/* {} y */", "x".repeat(70));
        formats_to(
            &format!("main() {{ left, {comment} right, mark, mark, mark, mark, mark; }}\n"),
            "main() {\n    left,\n    /* xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx y */ right,\n    mark, mark, mark, mark, mark;\n}\n",
        );
    }

    /// A comment in the LABEL REGION — between a label's number and its
    /// colon, or between two stacked labels — prints in place in the
    /// label prefix ([`label_region_comments`], the never-move rule),
    /// and the statement takes the label-break layout.
    #[test]
    fn a_comment_in_the_label_region_prints_in_place() {
        formats_to(
            "main() { 1/* lbl */: left; }\n",
            "main() {\n 1 /* lbl */:\n    left;\n}\n",
        );
        formats_to(
            "main() { 1: 2/* between labels */: left; }\n",
            "main() {\n 1: 2 /* between labels */:\n    left;\n}\n",
        );
    }

    // -- The tail slot, under the never-move rule ------------------------
    //
    // A comment between the last item and the `;` prints BEFORE the `;`,
    // in its written slot — it precedes the `;` in source, and the rule
    // keeps it there. Only a genuine post-`;` same-line comment is the
    // statement's trailing comment now.

    /// A same-line block comment before the `;` stays before it; one
    /// nested inside the last item stays inside the item's own text
    /// ([`render_item_tokens`]).
    #[test]
    fn a_pre_semicolon_comment_prints_before_the_semicolon() {
        formats_to(
            "main() {\n 1: left /* c */;\n}\n",
            "main() {\n 1: left /* c */;\n}\n",
        );
        formats_to(
            "main() {\n 1: check(1 /* c */, 2);\n}\n",
            "main() {\n 1: check(1 /* c */, 2);\n}\n",
        );
        formats_to(
            "main() {\n 1: left, /* a */right /* b */;\n}\n",
            "main() {\n 1: left, /* a */ right /* b */;\n}\n",
        );
    }

    /// A pre-`;` comment and a genuine post-`;` one coexist: the first
    /// prints before the `;`, the second is the statement's trailing
    /// comment ([`trivia::trailing_comment`], the only trailing source
    /// now).
    #[test]
    fn a_pre_and_a_post_semicolon_comment_each_keep_their_side() {
        formats_to(
            "main() {\n 1: left /*a*/; // b\n}\n",
            "main() {\n 1: left /*a*/; // b\n}\n",
        );
    }

    /// An own-line tail comment keeps its own line at the command
    /// column, still BEFORE the `;` — and since a LINE comment eats the
    /// rest of its physical line, the `;` moves to its own line below
    /// ([`print_statement`]). Blank lines inside the tail are not
    /// preserved (the slot renders compactly).
    #[test]
    fn an_own_line_pre_semicolon_comment_keeps_its_line_before_the_semicolon() {
        formats_to(
            "main() {\n 1: left\n// own line\n;\n}\n",
            "main() {\n 1: left\n    // own line\n    ;\n}\n",
        );
        formats_to(
            "main() {\n 1: left\n\n// own line\n;\n}\n",
            "main() {\n 1: left\n    // own line\n    ;\n}\n",
        );
    }

    /// A LINE comment nested inside `check(...)`'s own multi-line arm
    /// list stays inside the arms ([`render_item_tokens`]): nothing can
    /// follow it on its physical line, so the item continues on the next
    /// line at the continuation column.
    #[test]
    fn a_line_comment_inside_check_arms_stays_inside_them() {
        formats_to(
            "main() {\n 1: check(1,\n // a\n 2);\n}\n",
            "main() {\n 1: check(1, // a\n        2);\n}\n",
        );
    }

    /// An own-line BLOCK comment directly before the `;` keeps its own
    /// line and the `;` follows it there — before-the-`;` in token
    /// order, exactly as written (its own-line flag re-derives true on
    /// the next parse, so this is a fixed point).
    #[test]
    fn an_own_line_block_comment_before_the_semicolon_stays_before_it() {
        formats_to(
            "main() {\n 1: left\n/* c */;\n}\n",
            "main() {\n 1: left\n    /* c */;\n}\n",
        );
    }

    /// A pre-`;` comment is NOT a trailing comment: it prints before its
    /// `;` and joins no alignment run — only the genuine post-`;`
    /// comment on the next statement does (a run of one, so it takes the
    /// single space).
    #[test]
    fn a_pre_semicolon_comment_joins_no_alignment_run() {
        formats_to(
            "main() {\n 1: left /* a */;\n 2: right;    // b\n}\n",
            "main() {\n 1: left /* a */;\n 2: right; // b\n}\n",
        );
    }

    /// Both sides of the `;` keep their comments with another statement
    /// following: the pre-`;` block prints before the `;`, the post-`;`
    /// line comment is statement 1's trailing comment, and statement 2
    /// is untouched.
    #[test]
    fn pre_and_post_semicolon_comments_hold_with_a_following_statement() {
        formats_to(
            "main() {\n 1: left /*a*/; // b\n 2: right;\n}\n",
            "main() {\n 1: left /*a*/; // b\n 2: right;\n}\n",
        );
    }

    /// An own-line tail comment stays before its `;`, and what follows
    /// the `;` is untouched by it: a post-`;` standalone comment prints
    /// as its own body element (no blank inserted — none separates it
    /// from `;` in the real tree).
    #[test]
    fn a_tail_comment_leaves_what_follows_the_semicolon_alone() {
        formats_to(
            "main() {\n 1: left\n// own line\n\n\n;\n// b\n}\n",
            "main() {\n 1: left\n    // own line\n    ;\n    // b\n}\n",
        );
        formats_to(
            "main() {\n 1: left\n// own line\n\n\n\n\n;\n 2: right;\n}\n",
            "main() {\n 1: left\n    // own line\n    ;\n 2: right;\n}\n",
        );
    }

    /// TWO comments nested inside the same item's arms both stay inside
    /// it, in order (blank lines inside an item's parens are not
    /// preserved — the arms render on one line once the comments are
    /// blocks).
    #[test]
    fn two_comments_inside_one_items_arms_both_stay_inside() {
        formats_to(
            "main() {\n 1: check(1 /* a */,\n\n\n 2 /* b */);\n}\n",
            "main() {\n 1: check(1 /* a */, 2 /* b */);\n}\n",
        );
    }

    /// A tail comment plus a claimed standalone after the `;`: each
    /// prints on its own side of the `;`, the following statement's
    /// leading run untouched.
    #[test]
    fn a_tail_comment_and_a_claimed_comment_each_keep_their_side() {
        formats_to(
            "main() {\n 1: left\n// own line\n;\n// c\n 2: right;\n}\n",
            "main() {\n 1: left\n    // own line\n    ;\n    // c\n 2: right;\n}\n",
        );
        formats_to(
            "main() {\n 1: left\n// own line\n;\n// c\n// d\n 2: right;\n}\n",
            "main() {\n 1: left\n    // own line\n    ;\n    // c\n    // d\n 2: right;\n}\n",
        );
    }

    /// The same shape reaching a NESTED FUNCTION's leading run instead
    /// of a statement's.
    #[test]
    fn a_tail_comment_before_a_nested_functions_leading_run() {
        formats_to(
            "main() {\n 1: left\n// own line\n;\n// c\n    step() {\n 1: left;\n    }\n}\n",
            "main() {\n 1: left\n    // own line\n    ;\n    // c\n    step() {\n     1: left;\n    }\n}\n",
        );
    }

    /// A LINE comment inside the last item's arms plus a claimed
    /// standalone after the `;` — the item keeps its comment inside, the
    /// standalone stays a body element.
    #[test]
    fn an_item_interior_comment_and_a_following_standalone_are_independent() {
        formats_to(
            "main() {\n 1: check(1,\n // a\n 2);\n// c\n 2: right;\n}\n",
            "main() {\n 1: check(1, // a\n        2);\n    // c\n 2: right;\n}\n",
        );
    }

    /// A REAL blank line between the `;` and a claimed standalone
    /// comment survives — the ordinary `blank_immediately_before`
    /// sibling-gap query, unaffected by the tail slot.
    #[test]
    fn a_real_blank_line_before_a_claimed_comment_survives() {
        formats_to(
            "main() {\n 1: left\n// own line\n;\n\n// c\n 2: right;\n}\n",
            "main() {\n 1: left\n    // own line\n    ;\n\n    // c\n 2: right;\n}\n",
        );
    }

    /// C1's own `has_trailing` matches `BodyKind::Statement` alone
    /// (`fmt/mod.rs`) — a nested function's own close-brace comment never
    /// joins an alignment run, even sitting directly adjacent to a
    /// commented statement. A mutant that instead used
    /// `resolved_trailing(elem).is_some()` (which also covers
    /// `BodyElem::Nested`) would wrongly start a run at the nested
    /// function's `} // a` — `code_line_width_incl_semi` reads
    /// `codes[k]`, an EMPTY placeholder for `BodyElem::Nested`
    /// (`print_body`'s own pre-pass), so `max_code_w`/`align_col` would
    /// be corrupted to `1`, visibly shrinking statement 2's own spacing.
    #[test]
    fn a_nested_functions_close_brace_comment_never_joins_an_alignment_run() {
        formats_to(
            "main() {\n    step() {\n 1: left;\n    } // a\n 2: right;    // b\n}\n",
            "main() {\n    step() {\n     1: left;\n    } // a\n 2: right; // b\n}\n",
        );
    }

    // -- Task 6: function open-brace comments ----------------------------

    #[test]
    fn comments_after_an_opening_brace() {
        formats_to(
            "main() { // open\n 1: left;\n}\n",
            "main() { // open\n 1: left;\n}\n",
        );
        formats_to("namespace n { // open\n}\n", "namespace n { // open\n}\n");
        formats_to(
            "main() { // open\n    step() { // nested open\n 1: left;\n    }\n}\n",
            "main() { // open\n    step() { // nested open\n     1: left;\n    }\n}\n",
        );
    }

    /// The DOUBLE-PRINT trap `print_function`'s open-brace comment shares
    /// with `print_namespace`'s: without threading `open` down into
    /// `print_body` as `reserved`, the SAME comment token would ALSO
    /// surface as the first body element's own leading run
    /// (`trivia::leading_comments` walks back through every raw sibling
    /// token, not merely up to `{`) and print a second time.
    #[test]
    fn an_open_brace_comment_does_not_double_print_as_the_first_statements_leading_run() {
        formats_to(
            "main() { // open\n 1: left;\n}\n",
            "main() { // open\n 1: left;\n}\n",
        );
        formats_to(
            "main() { // open\n    step() {\n 1: left;\n    }\n}\n",
            "main() { // open\n    step() {\n     1: left;\n    }\n}\n",
        );
    }

    /// The other half of that trap, and the one the fixtures above all
    /// miss: nothing has to CLAIM an open-brace comment for the body walk
    /// to reach it. It is a raw token between `{` and `}`, so it arrives
    /// as a standalone body element in its own right whenever no element
    /// takes it as a leading run — an empty body, a body of only
    /// comments, or a blank line between it and the first statement (a
    /// blank cuts the leading run, `trivia::leading_comments`). Printing
    /// it there put a second copy at body indent, and since `pmt fmt`
    /// rewrites in place, every later pass appended another: five passes,
    /// five copies. Hence the fixed point, not one pass.
    #[test]
    fn an_open_brace_comment_prints_once_when_nothing_claims_it() {
        // Empty body: nothing else is in the body at all.
        formats_to_fixed_point("main() { // open\n}\n", "main() { // open\n}\n");
        // A body of only comments: the walk reaches both as standalone.
        formats_to_fixed_point(
            "main() { // open\n    // dangling\n}\n",
            "main() { // open\n    // dangling\n}\n",
        );
        // A blank line cuts the leading run, so the statement below no
        // longer claims it — and the blank itself is stripped at the
        // brace edge (`docs/pmt/fmt.md` (blank lines)).
        formats_to_fixed_point(
            "main() { // open\n\n 1: left;\n}\n",
            "main() { // open\n 1: left;\n}\n",
        );
        // Any nesting depth: a nested function's own open-brace run is
        // threaded down the same way.
        formats_to_fixed_point(
            "main() {\n    step() { // nested open\n    }\n}\n",
            "main() {\n    step() { // nested open\n    }\n}\n",
        );
        formats_to_fixed_point(
            "namespace n {\n    f() { // open\n\n 1: left;\n    }\n}\n",
            "namespace n {\n    f() { // open\n     1: left;\n    }\n}\n",
        );
    }

    // -- Stacked labels written on separate lines -------------------------

    /// A newline BETWEEN two labels is not the own-line-label break: the
    /// break is measured from the last label to the command
    /// (`docs/pmt/fmt.md` (own-line labels)), so stacked labels the author
    /// spread over lines restack onto one, and their label prefix keeps
    /// counting as an INLINE label for the command column
    /// (`docs/pmt/fmt.md` (label and command alignment)). Reading the
    /// first newline after ANY label as the break instead put the command
    /// on its own line and dropped the prefix out of the width
    /// measurement — which, in a body whose column was set by that very
    /// statement, collapsed every command in it from column 12 to column
    /// 4.
    #[test]
    fn stacked_labels_written_on_separate_lines_restack() {
        formats_to_fixed_point(
            "main() {\n 1:\n 2: left;\n}\n",
            "main() {\n  1: 2: left;\n}\n",
        );
        // Same body, mixed with a wide inline label: both commands land
        // on the one command column the wide label set.
        formats_to_fixed_point(
            "main() {\n 11111: right;\n 1:\n 2: left;\n}\n",
            "main() {\n 11111: right;\n  1: 2: left;\n}\n",
        );
        // Three labels, two breaks — still one statement on one line.
        formats_to_fixed_point(
            "main() {\n 1:\n 2:\n 3: left;\n}\n",
            "main() {\n   1: 2: 3: left;\n}\n",
        );
        // A real break after the LAST label is still preserved, and
        // still keeps the prefix out of the inline-label measurement:
        // the command sits at the body indent, not at a column widened
        // by `1: 2:`.
        formats_to_fixed_point(
            "main() {\n 1:\n 2:\n    left;\n}\n",
            "main() {\n 1: 2:\n    left;\n}\n",
        );
    }

    /// The same rule with a comment between the two labels: the comment
    /// does not make the gap a break either. A LINE comment between two
    /// stacked labels prints between them — the second label continues
    /// on the next line — and the shape is a fixed point in ONE pass:
    /// this was fmt's single non-idempotent position before the
    /// never-move port ([`label_region_comments`]).
    #[test]
    fn a_comment_between_stacked_labels_stays_between_them() {
        formats_to(
            "main() {\n 1:\n // mid\n 2: left;\n}\n",
            "main() {\n 1: // mid\n 2:\n    left;\n}\n",
        );
    }

    // -- Task 6: everything at once ---------------------------------------

    #[test]
    fn every_comment_position_at_once() {
        formats_to(
            concat!(
                "// file leader\n\n",
                "use a::b, // interior\n    c::d;\n\n",
                "? doc\n",
                "main() { // open\n",
                "    // leading standalone\n",
                " 1: left, right; // trailing\n\n",
                " 2: check(1, 2);\n",
                "} // close\n"
            ),
            "// file leader\n\nuse a::b, // interior\n    c::d;\n\n? doc\nmain() { // open\n    // leading standalone\n 1: left, right; // trailing\n\n 2: check(1, 2);\n} // close\n",
        );
    }

    /// A pinned run over every real, multi-hundred-line `.pmc` program
    /// already in this repository — the embedded stdlib, the
    /// derivation-first golden fixtures, and `rich.pmc` (a syntax-tree
    /// fixture picked BECAUSE it is trivia-dense by construction). A
    /// hand-written 1-5 line fixture proves one shape at a time; only a
    /// real program proves the wiring holds together — [`print_body`]'s
    /// signature, both `consumed` builders, [`render_items`]'s wiring
    /// and [`StmtElem`]'s fields — the way the corpus-wide gates in
    /// `tests/fmt_programs.rs` do.
    ///
    /// Five of the twelve are committed in canonical form, so their own
    /// source IS the expected text — a stronger pin than a captured
    /// blob, since the expected side is a file other suites already
    /// read and would notice changing. The other seven are deliberately
    /// non-canonical (they are lexer and parser fixtures, not formatter
    /// ones), so their canonical form is committed beside them under
    /// `tests/fmt_expected/`. Each of those is exactly `pmt fmt` output
    /// for the fixture it names — reproduce one with
    /// `pmt fmt - < tests/<dir>/<name>.pmc` (the stdin form writes to
    /// stdout and leaves the fixture itself alone), and overwrite it only
    /// after deciding the change it would encode is intended.
    #[test]
    fn the_corpus_formats_to_its_pinned_canonical_form() {
        // Already canonical: the expected text is the committed source.
        formats_to(
            include_str!("../stdlib/std.pmc"),
            include_str!("../stdlib/std.pmc"),
        );
        formats_to(
            include_str!("../../tests/golden/sum.pmc"),
            include_str!("../../tests/golden/sum.pmc"),
        );
        formats_to(
            include_str!("../../tests/golden/ty.pmc"),
            include_str!("../../tests/golden/ty.pmc"),
        );
        formats_to(
            include_str!("../../tests/golden/ex000002.pmc"),
            include_str!("../../tests/golden/ex000002.pmc"),
        );
        formats_to(
            include_str!("../../tests/syntax/contextual.pmc"),
            include_str!("../../tests/syntax/contextual.pmc"),
        );
        // Deliberately non-canonical fixtures: canonical form beside them.
        formats_to(
            include_str!("../../tests/golden/sum2.pmc"),
            include_str!("../../tests/fmt_expected/sum2.pmc.expected"),
        );
        formats_to(
            include_str!("../../tests/golden/ty2.pmc"),
            include_str!("../../tests/fmt_expected/ty2.pmc.expected"),
        );
        formats_to(
            include_str!("../../tests/golden/ex000001.pmc"),
            include_str!("../../tests/fmt_expected/ex000001.pmc.expected"),
        );
        formats_to(
            include_str!("../../tests/golden/test1.pmc"),
            include_str!("../../tests/fmt_expected/test1.pmc.expected"),
        );
        formats_to(
            include_str!("../../tests/syntax/rich.pmc"),
            include_str!("../../tests/fmt_expected/rich.pmc.expected"),
        );
        formats_to(
            include_str!("../../tests/syntax/nested_ns.pmc"),
            include_str!("../../tests/fmt_expected/nested_ns.pmc.expected"),
        );
        formats_to(
            include_str!("../../tests/syntax/retok.pmc"),
            include_str!("../../tests/fmt_expected/retok.pmc.expected"),
        );
    }

    // -- Header-interior comments print in place ------------------------
    //
    // A comment written between a declaration's header tokens — between
    // the name and `(`, inside the parens, between `)` and `{`, after
    // `volatile`/`export`, or between a namespace's keyword/name and its
    // `{` — prints on the header line, between the same two tokens it
    // was written between ([`render_header_tokens`], the never-move
    // rule). A LINE comment continues the header on the next line at
    // the declaration's own indent. These fixtures replaced the old
    // relocate-to-body pins when the never-move port landed; the C1
    // relocation behavior they used to pin is retired.

    #[test]
    fn header_comment_between_parens_and_brace_stays() {
        formats_to_fixed_point(
            "main() /* keep me */ {\n  1: right;\n}\n",
            "main() /* keep me */ {\n 1: right;\n}\n",
        );
    }

    #[test]
    fn header_comment_between_name_and_parens_stays() {
        formats_to_fixed_point(
            "main /* x */ () {\n  1: right;\n}\n",
            "main /* x */() {\n 1: right;\n}\n",
        );
    }

    #[test]
    fn header_comment_inside_the_parens_stays() {
        formats_to_fixed_point(
            "main( /* b */ ) {\n  1: right;\n}\n",
            "main( /* b */) {\n 1: right;\n}\n",
        );
    }

    #[test]
    fn header_comments_after_volatile_and_export_stay() {
        formats_to_fixed_point(
            "export /* c */ f() {\n  1: right;\n}\nmain() {\n  1: @f();\n}\n",
            "export /* c */ f() {\n 1: right;\n}\nmain() {\n 1: @f();\n}\n",
        );
        formats_to_fixed_point(
            "volatile /* v */ main() {\n  1: right;\n}\n",
            "volatile /* v */ main() {\n 1: right;\n}\n",
        );
        formats_to_fixed_point(
            "volatile /* v */ export /* e */ main() {\n  1: right;\n}\n",
            "volatile /* v */ export /* e */ main() {\n 1: right;\n}\n",
        );
    }

    /// A `//` header comment consumes the rest of its line, so the rest
    /// of the header — here the `{` — continues on the next line at the
    /// declaration's indent.
    #[test]
    fn a_header_line_comment_continues_the_header_below() {
        formats_to_fixed_point(
            "main() // note\n{\n  1: right;\n}\n",
            "main() // note\n{\n 1: right;\n}\n",
        );
    }

    /// Blank lines inside a header do not survive — the header renders
    /// compactly, comments in their written token slots.
    #[test]
    fn header_blanks_render_compactly() {
        formats_to_fixed_point(
            "main() /* a */\n/* c */ {\n  1: right;\n}\n",
            "main() /* a */ /* c */ {\n 1: right;\n}\n",
        );
        formats_to_fixed_point(
            "main() /* a */\n\n/* c */ {\n  1: right;\n}\n",
            "main() /* a */ /* c */ {\n 1: right;\n}\n",
        );
        formats_to_fixed_point(
            "main() /* a */ {\n\n  1: right;\n}\n",
            "main() /* a */ {\n 1: right;\n}\n",
        );
        formats_to_fixed_point(
            "main() /* a */\n\n{\n  1: right;\n}\n",
            "main() /* a */ {\n 1: right;\n}\n",
        );
    }

    #[test]
    fn two_header_comments_stay_in_source_order() {
        formats_to_fixed_point(
            "main() /* a */ /* b */ {\n  1: right;\n}\n",
            "main() /* a */ /* b */ {\n 1: right;\n}\n",
        );
    }

    /// The open-brace comment rides the brace even with a header comment
    /// present — the two are independent surfaces now.
    #[test]
    fn a_header_comment_leaves_the_open_brace_comment_on_the_brace() {
        formats_to_fixed_point(
            "main() /* a */ { // t\n  1: right;\n}\n",
            "main() /* a */ { // t\n 1: right;\n}\n",
        );
    }

    #[test]
    fn header_comment_coexists_with_a_doc_run() {
        formats_to_fixed_point(
            "? doc\nmain() /* h */ {\n  1: right;\n}\n",
            "? doc\nmain() /* h */ {\n 1: right;\n}\n",
        );
    }

    #[test]
    fn nested_function_header_comment_stays_one_level_deeper() {
        formats_to_fixed_point(
            "main() {\n  step() /* n */ {\n    1: right;\n  }\n  1: @step();\n}\n",
            "main() {\n    step() /* n */ {\n     1: right;\n    }\n 1: @step();\n}\n",
        );
    }

    #[test]
    fn namespace_header_comments_stay() {
        // Between the name and `{`.
        formats_to_fixed_point(
            "namespace n /* nk */ {\n  export f() {\n    1: right;\n  }\n}\nmain() {\n  1: @n::f();\n}\n",
            "namespace n /* nk */ {\n    export f() {\n     1: right;\n    }\n}\nmain() {\n 1: @n::f();\n}\n",
        );
        // Between the keyword and the name.
        formats_to_fixed_point(
            "namespace /* k */ n {\n  export f() {\n    1: right;\n  }\n}\nmain() {\n  1: @n::f();\n}\n",
            "namespace /* k */ n {\n    export f() {\n     1: right;\n    }\n}\nmain() {\n 1: @n::f();\n}\n",
        );
        // Header blanks render compactly here too.
        formats_to_fixed_point(
            "namespace n /* k */\n\n{\n  export f() {\n    1: right;\n  }\n}\nmain() {\n  1: @n::f();\n}\n",
            "namespace n /* k */ {\n    export f() {\n     1: right;\n    }\n}\nmain() {\n 1: @n::f();\n}\n",
        );
        // And the open-brace comment stays on the brace here too.
        formats_to_fixed_point(
            "namespace n /* k */ { // t\n  export f() {\n    1: right;\n  }\n}\nmain() {\n  1: @n::f();\n}\n",
            "namespace n /* k */ { // t\n    export f() {\n     1: right;\n    }\n}\nmain() {\n 1: @n::f();\n}\n",
        );
    }
}
