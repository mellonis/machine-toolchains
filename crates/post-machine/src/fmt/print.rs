//! The green-tree `.pmc` printer (`docs/pmt/fmt.md`,
//! `docs/core.md` (syntax tree)) — a SEPARATE, parallel implementation
//! of [`super::format`] built directly on the lossless green tree
//! instead of the hand-typed C1 CST. It grows surface by surface across
//! several plans; the C1 printer in [`super`] stays untouched as the
//! differential oracle every widened surface is checked against (this
//! module's own `tests`), until the corpus-wide cutover retires it.
//!
//! **Scope of this module today**: the file itself, standalone/leading
//! comments between top-level items, `use` declarations (paths and
//! aliases), namespaces (including their same-line open/close-brace
//! comments and nested/reopened namespaces), functions: headers
//! (`volatile`/`export`, doc runs), nesting, and statement bodies
//! (labels, command-column alignment, comma-group layout with the
//! greedy-fill width fallback), every own-line comment inside a body or a
//! doc run (leading, standalone, trailing the body, a comment run's own
//! internal blank line, a block comment spanning lines, and a comment
//! interleaved inside or immediately after a bound doc run), and — new
//! this plan — a statement's own same-line trailing comment, the
//! column-alignment runs several such comments form together
//! ([`compute_trailing_spacing`]), and a function's own close-brace
//! comment. Every comment position this module does not yet cover hits
//! an explicit `unreachable!` naming the plan that owns it, so a test
//! that strays outside the covered surface fails loudly instead of
//! silently printing something wrong:
//!
//! - A use list's own interior comments and a comma group's interior
//!   comments are task 6's surface (`print_use`, [`print_body`]). A
//!   comment between a statement's label and its first item is also
//!   task 6's, guarded the same way [`print_body`]'s own
//!   `descendant_tokens` scan guards a comma group's interior comment
//!   (both live inside `STATEMENT`, so one scan catches both shapes).
//! - A function's same-line open-brace comment is task 6's surface
//!   ([`print_function`]) — a different task from its close-brace
//!   comment, see that function's own doc for why.
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

/// `.pmc` source → canonical text, the green-tree path
/// (`docs/core.md` (syntax tree)). Lexes `WithComments`, parses straight
/// to the green tree (no C1 CST involved), and prints from typed views
/// and raw trivia queries over it. A lex/parse error is returned as
/// `Err`, never printed (thin renderer, same discipline as
/// [`super::format`]).
///
/// `#[allow(dead_code)]`: nothing outside this module's own tests calls
/// it yet — later plans widen coverage and, eventually, `cli/fmt.rs`
/// switches over to it. Every function below is reached only through
/// this one, so this single attribute covers the whole file.
#[allow(dead_code)]
pub(crate) fn format_green(source: &str) -> Result<String, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    let root = SyntaxNode::new_root(green);
    // Threaded through every print function from here down:
    // `compute_trailing_spacing`'s aligned/ragged verdict reads each
    // trailing comment's own SOURCE column (`docs/pmt/fmt.md`
    // (comments)), which only a line index can answer.
    let line_index = TextLineIndex::new(source);
    let mut out = String::new();
    print_items(&mut out, root.children_with_tokens(), 0, &[], &line_index);
    // Edge case (module doc's "Edge cases", mirrored from
    // `super::print_cst`): an empty or whitespace-only file still
    // reprints as exactly one newline.
    if out.is_empty() {
        out.push('\n');
    }
    Ok(out)
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
/// (one level deeper, called on [`namespace_interior`]) share this walk,
/// so nesting a namespace inside a namespace recurses "for free" the
/// same way [`super::print_top_items`] does for C1.
///
/// `reserved` is the caller's own already-printed comment tokens (a
/// namespace's [`super::trivia::open_trailing`] run) — see the module
/// doc's second bullet for why the shared walk needs to know about them.
fn print_items(
    out: &mut String,
    elements: impl Iterator<Item = SyntaxElement>,
    indent: usize,
    reserved: &[SyntaxToken],
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

    let mut first = true;
    for e in &elements {
        match e {
            SyntaxElement::Node(node) => {
                // The `i > 0`-equivalent brace-edge suppression
                // (`super::top_wants_blank_before`'s doc): the first
                // printed unit at this level never gets a forced blank
                // line, regardless of what precedes it in source.
                if !first && trivia::blank_before_unit(node) {
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
                if !first && blank_immediately_before(tok) {
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
/// mirroring [`super::print_namespace`]'s decisions: one space before
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
    out.push_str("namespace ");
    out.push_str(&ns.name());
    out.push_str(" {");
    let brace =
        token(node, PmcKind::LBrace.into()).expect("NAMESPACE always carries an L_BRACE token");
    let open = trivia::open_trailing(&brace);
    if open.is_empty() {
        out.push('\n');
    } else {
        out.push(' ');
        let texts: Vec<String> = open
            .iter()
            .map(|c| super::normalize_comment_text(c.text()))
            .collect();
        out.push_str(&texts.join(" "));
        out.push('\n');
    }
    print_items(
        out,
        brace_interior(node),
        indent + super::INDENT_UNIT,
        &open,
        line_index,
    );
    out.push_str(&pad);
    out.push('}');
    if let Some(c) = trivia::trailing_comment(node) {
        out.push(' ');
        out.push_str(&super::normalize_comment_text(c.text()));
    }
    out.push('\n');
}

/// One `use` list (`docs/pmt/fmt.md` (spacing)), mirroring
/// [`super::print_use`]'s decisions for the surface this plan covers:
/// paths in source order, `::`-joined, `as`-aliased, one canonical space
/// after `use` and after each comma. A comment positioned INSIDE the
/// path list is a later plan's surface — see the module doc — and hits
/// a loud panic rather than being silently dropped. Scanned via
/// `descendant_tokens`, not `children_with_tokens`: a comment written
/// between `::` and a segment (`std::/* c */goToEnd`) lexes as a child
/// of the `USE_PATH` node one level down, not of the `UseDecl` node
/// itself — `render_use_path` only reads `UsePathView`'s own
/// IDENT/`::` tokens, so a shallower scan would let such a comment slip
/// past undetected and get silently dropped instead of panicking.
fn print_use(out: &mut String, u: &UseDeclView, indent: usize, _line_index: &TextLineIndex) {
    let node = u.syntax();
    if node
        .descendant_tokens()
        .any(|t| trivia::is_comment(t.kind()))
    {
        unreachable!("interior use-list comments are task 6's surface")
    }
    let rendered: Vec<String> = u.paths().map(|p| render_use_path(&p)).collect();
    out.push_str(&" ".repeat(indent));
    out.push_str("use ");
    out.push_str(&rendered.join(", "));
    out.push(';');
    if let Some(tc) = trivia::trailing_comment(node) {
        out.push(' ');
        out.push_str(&super::normalize_comment_text(tc.text()));
    }
    out.push('\n');
}

/// One `use`-list path (`docs/pmt/fmt.md` (spacing)): `::` tight,
/// ` as ALIAS` one space each side if present — [`super::render_use_path`]'s
/// decision, ported to read segments/alias off [`UsePathView`] instead
/// of a parsed `UsePath`.
fn render_use_path(p: &UsePathView) -> String {
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
/// [`super::normalize_comment_text`] directly. Content printing is
/// IDENTICAL regardless of the comment's relationship to its neighbors;
/// only the blank-line decision made by [`print_items`]'s caller
/// differs — [`super::print_comment`]'s same rule, ported to a raw
/// [`SyntaxToken`].
fn print_comment(out: &mut String, comment: &SyntaxToken, indent: usize) {
    out.push_str(&" ".repeat(indent));
    out.push_str(&super::normalize_comment_text(comment.text()));
    out.push('\n');
}

/// Header + doc run + body + closing brace (`docs/pmt/fmt.md`
/// (indentation)), mirroring [`super::print_function`]'s decisions —
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
/// **Open-brace comment deferred, close-brace comment implemented**: a
/// same-line comment after the opening `{` (`trivia::open_trailing`) is
/// task 6's surface, still guarded; a same-line comment after the
/// closing `}` (`trivia::trailing_comment` — the same query a
/// statement's trailing comment uses, see [`trivia::trailing_comment`]'s
/// own doc for why one function serves both shapes) prints here exactly
/// like [`print_namespace`]'s own close-brace comment, one canonical
/// space before it.
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
    let brace =
        token(node, PmcKind::LBrace.into()).expect("FUNCTION always carries an L_BRACE token");
    if !trivia::open_trailing(&brace).is_empty() {
        unreachable!("a function's open-brace trailing comment is task 6's surface")
    }
    out.push('\n');
    print_body(out, node, indent + super::INDENT_UNIT, line_index);
    out.push_str(&pad);
    out.push('}');
    if let Some(c) = trivia::trailing_comment(node) {
        out.push(' ');
        out.push_str(&super::normalize_comment_text(c.text()));
    }
    out.push('\n');
}

/// Comments sitting between a bound `DOC_RUN` and its declaration's own
/// header token — real, but with no C1 CST field to port from.
/// `Parser::doc_run`'s own comment-draining loop only runs on an
/// iteration that ALSO consumes one more `?`/`!` line first (the `for
/// (comment, cline) in self.drain_pending()` call sits at the BOTTOM of
/// the `loop`, after a `DocLine`/`AttentionLine` match arm, never
/// reached once the match falls to `_ => break`), so a comment after the
/// run's LAST line is captured there too — landing in the SAME
/// `Vec<DocRunItem>` as `DocRunKind::Comment` — but green emission is a
/// separate mechanism entirely (`GreenSink::flush` is lazy: a token's
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
/// [`super::print_doc_run`]'s decisions: each at the bound declaration's
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
/// from [`super::print_doc_run_line`] (pure text, no green-tree input).
fn print_doc_run_line(out: &mut String, pad: &str, sigil: char, text: &str) {
    out.push_str(pad);
    out.push(sigil);
    if !text.is_empty() {
        out.push(' ');
        out.push_str(text);
    }
    out.push('\n');
}

/// One extracted statement plus the two facts [`Statement`] itself does
/// not carry: `label_break` ([`trivia::label_break`] — a green-tree
/// query, not a CST field) and each item's `newline_before` (whether the
/// author put a newline before it inside its comma group, computed
/// below from the `ITEM` nodes' own text ranges — `Statement::items` is
/// a flat `Vec<Item>` with no per-item position of its own). `node` is
/// kept alongside for [`trivia::blank_before_unit`]'s per-body-item
/// query in [`print_body`].
struct StmtElem {
    node: SyntaxNode,
    stmt: Statement,
    label_break: bool,
    newline_before: Vec<bool>,
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
enum BodyElem {
    Statement(StmtElem),
    Nested(FunctionView),
    Comment(SyntaxToken),
}

/// The [`SyntaxNode`] a [`BodyElem::Statement`]/[`BodyElem::Nested`]
/// wraps, or `None` for [`BodyElem::Comment`] (which carries a raw
/// token, not a node) — the one query [`print_body`] needs both to build
/// its `claimed` set (every `STATEMENT`/nested-`FUNCTION`'s own leading
/// comment run) and to drive its print loop's per-element
/// `blank_before_unit`/`leading_comments` queries, without duplicating
/// that pair of calls once per node variant.
fn body_elem_node(elem: &BodyElem) -> Option<&SyntaxNode> {
    match elem {
        BodyElem::Statement(s) => Some(&s.node),
        BodyElem::Nested(fv) => Some(fv.syntax()),
        BodyElem::Comment(_) => None,
    }
}

/// A `FUNCTION` node's own body — [`brace_interior`] between its `{` and
/// `}` — mirroring [`super::print_function`]'s body loop: one pass
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
/// filters that print the same way it does at the top level.
fn print_body(out: &mut String, func_node: &SyntaxNode, indent: usize, line_index: &TextLineIndex) {
    let elements: Vec<SyntaxElement> = brace_interior(func_node)
        .filter(|e| !trivia::is_ws(e.kind()))
        .collect();

    let mut body: Vec<BodyElem> = Vec::with_capacity(elements.len());
    for e in &elements {
        match e {
            SyntaxElement::Node(node) if node.kind() == PmcKind::Statement.into() => {
                let sv = StatementView::cast(node.clone()).expect("kind checked above");
                // Scanned with `descendant_tokens`, not the item list
                // alone: a comment between the label and the first
                // item, or between two comma-group items, is nested
                // inside STATEMENT either way and must not slip past a
                // shallower scan undetected (mirrors `print_use`'s own
                // `descendant_tokens` guard).
                if sv
                    .syntax()
                    .descendant_tokens()
                    .any(|t| trivia::is_comment(t.kind()))
                {
                    unreachable!("interior list comments are task 6's surface")
                }
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
                let stmt = extract_statement(&sv, line_index);
                body.push(BodyElem::Statement(StmtElem {
                    node: node.clone(),
                    stmt,
                    label_break,
                    newline_before,
                }));
            }
            SyntaxElement::Node(node) if node.kind() == PmcKind::Function.into() => {
                let fv = FunctionView::cast(node.clone()).expect("kind checked above");
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
                // close-brace print).
                let already_trailing = body
                    .last()
                    .and_then(body_elem_node)
                    .and_then(trivia::trailing_comment)
                    .is_some_and(|tc| &tc == t);
                if !already_trailing {
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

    let command_col = command_column(max_inline_label_prefix_width(&body), indent);

    // Every statement's code (label + items, no `;`) is rendered ONCE up
    // front — the trailing-comment alignment pre-pass
    // ([`compute_trailing_spacing`]) needs every run member's rendered
    // width before any of them is printed, mirroring
    // [`super::print_function`]'s own `codes` pre-pass. Non-statement
    // elements get an unused empty placeholder — `compute_trailing_spacing`
    // never indexes into one, since a run can only ever contain
    // `BodyElem::Statement` entries (see that function's own doc).
    let codes: Vec<String> = body
        .iter()
        .map(|elem| match elem {
            BodyElem::Statement(s) => render_statement_code(
                &s.stmt.labels,
                s.label_break,
                &s.stmt.items,
                &s.newline_before,
                command_col,
            ),
            BodyElem::Nested(_) | BodyElem::Comment(_) => String::new(),
        })
        .collect();
    let trailing_spacing = compute_trailing_spacing(&body, &codes, line_index);

    // Comments some OTHER printer already owns outright: a
    // `STATEMENT`/nested `FUNCTION`'s own same-line trailing comment
    // ([`print_statement`], or a nested `print_function`'s own
    // close-brace print) — mirrors [`print_items`]'s `consumed`, one
    // difference: the collection loop above already keeps a trailing
    // comment's OWN token out of `body` (`already_trailing`), so unlike
    // `print_items` this set is never checked against a `BodyElem::Comment`
    // arm. It exists for the OTHER direction only: `leading_comments`
    // walks back through every raw token until a NODE sibling, so a
    // trailing comment like that surfaces as the NEXT element's own
    // "leading run" too — filtering it there is this set's one job.
    let mut consumed: Vec<SyntaxToken> = Vec::new();
    for elem in &body {
        if let Some(node) = body_elem_node(elem) {
            consumed.extend(trivia::trailing_comment(node));
        }
    }

    // Every STATEMENT/nested-FUNCTION element's own leading comment run,
    // unioned — the set a standalone `BodyElem::Comment` is checked
    // against below, so a comment already printed as some OTHER
    // element's leading run isn't printed a SECOND time when the walk
    // also reaches it directly as its own raw token (mirrors
    // [`print_items`]'s `claimed`, minus its `consumed` half — see the
    // comment above for why body level's `consumed` has nothing to add
    // here).
    let mut claimed: Vec<SyntaxToken> = Vec::new();
    for elem in &body {
        if let Some(node) = body_elem_node(elem) {
            claimed.extend(trivia::leading_comments(node));
        }
    }

    let mut first = true;
    for (i, elem) in body.iter().enumerate() {
        if let BodyElem::Comment(tok) = elem {
            if claimed.contains(tok) {
                continue;
            }
            if !first && blank_immediately_before(tok) {
                out.push('\n');
            }
            first = false;
            print_comment(out, tok, indent);
            continue;
        }
        let node = body_elem_node(elem).expect(
            "BodyElem::Comment handled and skipped above; every other variant carries a node",
        );
        if !first && trivia::blank_before_unit(node) {
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
            line_index,
        );
    }
}

/// Dispatches one node-backed [`BodyElem`] to its printer — ported from
/// [`super::print_body_item`]. [`BodyElem::Comment`] never reaches this
/// dispatcher: [`print_body`]'s own loop prints it directly, mirroring
/// how [`print_items`] prints a standalone comment token itself rather
/// than routing it through [`print_item`]. `code`/`trailing_spacing` are
/// [`print_body`]'s pre-pass outputs, meaningful only for
/// [`BodyElem::Statement`] (mirrors [`super::print_body_item`]'s own
/// `code`/`trailing_spacing` parameters, unused by its `Nested`/`Comment`
/// arms).
fn print_body_item(
    out: &mut String,
    elem: &BodyElem,
    indent: usize,
    code: &str,
    trailing_spacing: usize,
    line_index: &TextLineIndex,
) {
    match elem {
        BodyElem::Statement(s) => print_statement(out, s, code, trailing_spacing),
        BodyElem::Nested(fv) => print_function(out, fv, indent, line_index),
        BodyElem::Comment(_) => unreachable!(
            "print_body's own loop handles BodyElem::Comment directly and never reaches this \
             dispatcher — see print_body_item's own doc"
        ),
    }
}

/// Label prefix width: the smallest multiple of [`super::INDENT_UNIT`]
/// that is `>= max(base_body_indent, P + 2)`, where `P` is the widest
/// INLINE labeled statement's label-prefix width in the body — ported
/// unchanged from [`super::command_column`] (pure `usize` arithmetic, no
/// green-tree input).
fn command_column(p: usize, base_body_indent: usize) -> usize {
    let min = base_body_indent.max(p + 2);
    min.div_ceil(super::INDENT_UNIT) * super::INDENT_UNIT
}

/// `P`: the max label-prefix width among `body`'s own INLINE labeled
/// statements — ported from [`super::max_inline_label_prefix_width`],
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
/// Ported unchanged from [`super::label_prefix_text`] (`&[Label]` is the
/// same [`crate::parser::Label`] on both sides).
fn label_prefix_text(labels: &[Label]) -> String {
    labels
        .iter()
        .map(|l| format!("{}:", l.written))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Char width of [`label_prefix_text`] — ported unchanged from
/// [`super::label_prefix_width`].
fn label_prefix_width(labels: &[Label]) -> usize {
    label_prefix_text(labels).chars().count()
}

/// Left margin for a `prefix_width`-wide label prefix at `command_col`,
/// or `None` if that would leave less than the mandatory 1-space margin
/// — ported unchanged from [`super::label_margin`].
fn label_margin(command_col: usize, prefix_width: usize) -> Option<usize> {
    command_col
        .checked_sub(prefix_width + 1)
        .filter(|&margin| margin >= 1)
}

/// One statement's code up to but NOT including the final `;` — ported
/// from [`super::render_statement_code`], reading a `Statement`'s own
/// `labels`/`items` plus the separately-derived `label_break` and
/// `newline_before` instead of a `StatementCst`'s fields directly. See
/// that function's own doc for the unlabeled / inline-labeled /
/// own-line-labeled shapes; the decisions are unchanged. Called once per
/// statement, up front, by [`print_body`]'s own pre-pass — see that
/// function's own doc for why [`compute_trailing_spacing`] needs every
/// statement's code rendered before any of them prints.
fn render_statement_code(
    labels: &[Label],
    label_break: bool,
    items: &[Item],
    newline_before: &[bool],
    command_col: usize,
) -> String {
    let mut out = String::new();
    if labels.is_empty() {
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
    out.push_str(&render_items(items, newline_before, command_col));
    out
}

/// One statement's final line(s): the precomputed `code`
/// ([`render_statement_code`], rendered once by [`print_body`]'s own
/// pre-pass), the `;`, then a same-line trailing comment if any — spaced
/// per `trailing_spacing` ([`compute_trailing_spacing`],
/// `docs/pmt/fmt.md` (comments)) — then the newline. Ported from
/// [`super::print_statement`], reading the trailing comment straight off
/// the green tree ([`trivia::trailing_comment`]) instead of a
/// `StatementCst::trailing` field.
fn print_statement(out: &mut String, s: &StmtElem, code: &str, trailing_spacing: usize) {
    out.push_str(code);
    out.push(';');
    if let Some(tc) = trivia::trailing_comment(&s.node) {
        out.push_str(&" ".repeat(trailing_spacing));
        out.push_str(&super::normalize_comment_text(tc.text()));
    }
    out.push('\n');
}

/// Char width of `code`'s LAST physical line, `+ 1` for the `;` that
/// follows it (a statement's trailing comment always rides the line
/// carrying the final `;`, even when a multi-line own-line label or
/// comma-group spreads the rest of the statement across earlier lines) —
/// ported unchanged from [`super::code_line_width_incl_semi`] (pure
/// text/`usize`, no green-tree input).
fn code_line_width_incl_semi(code: &str) -> usize {
    let last_line = code.rsplit('\n').next().unwrap_or(code);
    last_line.chars().count() + 1
}

/// Trailing-comment alignment (`docs/pmt/fmt.md` (comments)) — ported
/// from [`super::compute_trailing_spacing`], reading `body`'s
/// [`BodyElem`]s instead of C1's `&[BodyItem]` and each member's
/// trailing comment via [`trivia::trailing_comment`] instead of a
/// `StatementCst::trailing` field. Returns, per `body` index, the number
/// of spaces to place between the `;` and a trailing `//`/`/* */` —
/// meaningful only where that [`BodyElem`] is a [`BodyElem::Statement`]
/// with a trailing comment; other entries are unused filler.
///
/// **The run-boundary rule**, derived by reading
/// [`super::compute_trailing_spacing`]'s own scan (not restated from any
/// plan brief) and confirmed against the real C1 formatter on several
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
/// stable under repeated formatting (`docs/pmt/fmt.md` (comments), same
/// reasoning C1 documents on [`super::compute_trailing_spacing`] under
/// "Idempotence note"). When `aligned`, every non-overflowing member gets
/// `align_col - code_w[off]` spaces (which lands its `//` exactly at
/// `align_col`); when not, every member (overflowing or not) gets one
/// space.
///
/// **A note on `blank_before`**: C1 reads `BodyItem::blank_before` — the
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
    let has_trailing = |elem: &BodyElem| match elem {
        BodyElem::Statement(s) => trivia::trailing_comment(&s.node).is_some(),
        BodyElem::Nested(_) | BodyElem::Comment(_) => false,
    };
    // Only ever called on a `body[j]` the caller has already confirmed
    // `has_trailing` for, so `elem` is always a `BodyElem::Statement`
    // here — an unreachable panic rather than a silent `false` arm for
    // `Nested`/`Comment`, since those variants never actually reach this
    // closure (mirrors `comment_w`/`source_cols`'s own `let … else`
    // idiom below).
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
                let tc = trivia::trailing_comment(&s.node).expect("has_trailing guarantees Some");
                // Measured on the NORMALIZED text: a raw trailing
                // `\r`/space in the token must not inflate the column
                // math for a width nothing will actually print.
                super::normalize_comment_text(tc.text()).chars().count()
            })
            .collect();

        if run_len >= 2 {
            let max_code_w = *code_w.iter().max().expect("run_len >= 2");
            let align_col = max_code_w + 1;
            let overflow: Vec<bool> = (0..run_len)
                .map(|off| align_col + comment_w[off] > super::LINE_WIDTH)
                .collect();
            let source_cols: Vec<u32> = (run_start..run_end)
                .map(|k| {
                    let BodyElem::Statement(s) = &body[k] else {
                        unreachable!("has_trailing guarantees a Statement");
                    };
                    let tc =
                        trivia::trailing_comment(&s.node).expect("has_trailing guarantees Some");
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
/// fallback — ported from [`super::render_items`] WITHOUT its
/// mid-comma-group comment handling (`CommaItem::leading` has no
/// counterpart here: [`print_body`]'s own `descendant_tokens` guard
/// already refuses a STATEMENT carrying one, so grouping here is driven
/// by `newline_before` alone, never a forced comment break). `items` and
/// `newline_before` are parallel, index 0's `newline_before` always
/// `false`. See [`super::render_items`]'s own doc for the per-group
/// fit-or-greedy-fill decision this reproduces unchanged.
fn render_items(items: &[Item], newline_before: &[bool], command_col: usize) -> String {
    let texts: Vec<String> = items.iter().map(render_item).collect();
    let mut groups: Vec<Vec<usize>> = vec![vec![0]];
    for (i, &nb) in newline_before.iter().enumerate().skip(1) {
        if nb {
            groups.push(vec![i]);
        } else {
            groups.last_mut().expect("groups is never empty").push(i);
        }
    }
    let last_group_idx = groups.len() - 1;
    let mut out = String::new();
    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            out.push('\n');
            out.push_str(&" ".repeat(command_col));
        }
        let group_texts: Vec<&str> = group.iter().map(|&i| texts[i].as_str()).collect();
        let joined = group_texts.join(", ");
        // `+ 1` reserved for the trailing `,`/`;` folded into
        // `< LINE_WIDTH` (clippy::int_plus_one), same as C1.
        if command_col + joined.chars().count() < super::LINE_WIDTH {
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

/// The true output column after appending `text` at the point where the
/// cursor sits at `base` — ported unchanged from [`super::line_width_after`]
/// (pure text/`usize`, no green-tree input). No `text` this task ever
/// passes through here embeds a `\n` (comment-free items), so the `rsplit_once`
/// branch is dead for now — kept anyway, since [`greedy_fill_group`] is
/// otherwise a byte-for-byte port and diverging it would defeat the
/// point of porting rather than reimplementing.
fn line_width_after(base: usize, text: &str) -> usize {
    match text.rsplit_once('\n') {
        Some((_, last_line)) => last_line.chars().count(),
        None => base + text.chars().count(),
    }
}

/// Rule 2's greedy-fill, applied to one group's items — ported unchanged
/// from [`super::greedy_fill_group`] (pure text/`usize`, no green-tree
/// input).
fn greedy_fill_group(out: &mut String, texts: &[&str], command_col: usize) {
    let mut items = texts.iter();
    let first = items.next().expect("a comma group is never empty");
    out.push_str(first);
    let mut col = line_width_after(command_col, first);
    for text in items {
        let w = text.chars().count();
        if col + 2 + w < super::LINE_WIDTH {
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

/// Canonical item text — ported unchanged from [`super::render_item`]
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

/// Ported unchanged from [`super::builtin_name`].
fn builtin_name(which: Builtin) -> &'static str {
    match which {
        Builtin::Left => "left",
        Builtin::Right => "right",
        Builtin::Mark => "mark",
        Builtin::Unmark => "unmark",
    }
}

/// Ported unchanged from [`super::render_builtin_successor`].
fn render_builtin_successor(succ: Successor, written: Option<&str>) -> String {
    match succ {
        Successor::FallThrough => String::new(),
        _ => format!("({})", render_successor(succ, written)),
    }
}

/// Ported unchanged from [`super::render_successor`].
fn render_successor(succ: Successor, written: Option<&str>) -> String {
    match succ {
        Successor::FallThrough => String::new(),
        Successor::Label(_) => written
            .expect("succ_label_written is Some whenever succ is Successor::Label")
            .to_string(),
        Successor::Return => "!".to_string(),
    }
}

/// Ported unchanged from [`super::render_check_arm`].
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

    /// The differential oracle this plan is built on: for every shape the
    /// green printer already covers, its output is byte-identical to the
    /// C1 printer's. Tasks 3-6 widen the set of sources this is called on;
    /// Task 7 makes it corpus-wide and retires the C1 side.
    #[track_caller]
    fn same_as_c1(src: &str) {
        let green = format_green(src).expect("green printer accepts it");
        let c1 = crate::fmt::format(src).expect("C1 printer accepts it");
        assert_eq!(green, c1, "green output diverged from C1 for:\n{src}");
    }

    #[test]
    fn empty_and_whitespace_only_files() {
        same_as_c1("");
        same_as_c1("\n");
        same_as_c1("\n\n\n");
    }

    #[test]
    fn a_single_use_declaration() {
        same_as_c1("use std::goToEnd;\n");
        same_as_c1("use   std::goToEnd  ;\n");
        same_as_c1("use std::goToEnd as far;\n");
    }

    /// A comment lexed inside a `USE_PATH` node (`std::/* c */goToEnd`,
    /// one level below `UseDecl`) must hit the guard's loud panic, not
    /// slip past a shallower `children_with_tokens`-only scan and get
    /// silently dropped by `render_use_path`. Pins `print_use`'s
    /// `descendant_tokens` scan — reverting it to direct children only
    /// makes this test the one that goes red (see the fix report).
    #[test]
    #[should_panic(expected = "interior use-list comments are task 6's surface")]
    fn a_comment_nested_inside_a_use_path_is_not_silently_dropped() {
        let _ = format_green("use std::/* c */goToEnd;\n");
    }

    #[test]
    fn a_multi_path_use_declaration() {
        same_as_c1("use std::goToEnd, std::goToBegin;\n");
        same_as_c1("use std::goToEnd,\n    std::goToBegin as backToStart;\n");
    }

    #[test]
    fn standalone_comments_between_declarations() {
        same_as_c1("// leading\nuse std::goToEnd;\n");
        same_as_c1("// far\n\n// near\nuse std::goToEnd;\n");
        same_as_c1("use std::goToEnd;\n\n// trailing file comment\n");
        // A same-line trailing comment on one `use` immediately followed
        // (no blank line) by another `use`: `leading_comments` walks
        // back through every raw token until a NODE sibling, so without
        // the `consumed`/`claimed` split in `print_items` this same
        // comment token surfaces as BOTH the first use's trailing
        // comment AND the second use's leading run, printing it twice.
        same_as_c1("use std::goToEnd; // t\nuse std::goToBegin;\n");
        // The same shape, but with a blank line and a genuine leading
        // comment of its own between the trailing comment and the next
        // `use` — the blank line already cuts `leading_comments`' run
        // before it reaches the trailing comment, so this pins that the
        // fix above doesn't disturb the already-correct case.
        same_as_c1("use std::goToEnd; // t\n\n// lead\nuse std::goToBegin;\n");
    }

    #[test]
    fn an_empty_namespace() {
        same_as_c1("namespace n {\n}\n");
        same_as_c1("namespace n { // open\n}\n");
        same_as_c1("namespace n {\n} // close\n");
    }

    #[test]
    fn nested_and_reopened_namespaces() {
        same_as_c1("namespace a {\n    namespace b {\n    }\n}\n");
        same_as_c1("namespace a {\n}\n\nnamespace a {\n}\n");
        // Same overlap as the `use`-trailing-comment case above, one
        // level up: a namespace's same-line close-brace comment,
        // immediately followed (no blank line) by another namespace,
        // would otherwise also print as that next namespace's leading
        // comment.
        same_as_c1("namespace a {\n} // close\nnamespace b {\n}\n");
    }

    #[test]
    fn a_minimal_function() {
        same_as_c1("main() {\n 1: left;\n}\n");
        same_as_c1("main(){1:left;}\n");
        same_as_c1("main() {\n}\n");
    }

    #[test]
    fn function_header_modifiers() {
        same_as_c1("volatile main() {\n 1: left;\n}\n");
        same_as_c1("export helper() {\n 1: left;\n}\n");
    }

    #[test]
    fn a_doc_run_binds_to_its_function() {
        same_as_c1("? doc line\nmain() {\n 1: left;\n}\n");
        same_as_c1("? one\n? two\n! attention\nmain() {\n 1: left;\n}\n");
        same_as_c1("? doc\n\nmain() {\n 1: left;\n}\n");
    }

    #[test]
    fn nested_functions() {
        same_as_c1("main() {\n    step() {\n 1: left;\n    }\n\n    @step();\n}\n");
    }

    /// A nested function BETWEEN two statements. This is the fixture that
    /// catches the one Task 3 mistake byte-identity would otherwise only
    /// surface at Task 7, as a corpus-wide diff to bisect: building body
    /// order from `statements()` and `nested()` separately instead of from
    /// `children_with_tokens()` hoists the nested function out of place,
    /// and every other nested-function fixture happens to put it first.
    #[test]
    fn a_nested_function_between_statements_keeps_its_position() {
        same_as_c1("main() {\n 1: left;\n    step() {\n 1: left;\n    }\n 2: left;\n}\n");
        same_as_c1("main() {\n 1: left;\n\n    step() {\n 1: left;\n    }\n\n 2: left;\n}\n");
    }

    #[test]
    fn labels_stacked_and_own_line() {
        same_as_c1("main() {\n 1: 2: right, mark;\n 3: left;\n}\n");
        same_as_c1("main() {\n 1:\n    left;\n}\n");
        same_as_c1("main() {\n 1: left;\n 10: left;\n 100: left;\n}\n");
    }

    #[test]
    fn statement_shapes() {
        same_as_c1("main() {\n 1: check(1, 2);\n 2: goto 1;\n 3: halt;\n}\n");
        same_as_c1("main() {\n 1: @callee();\n 2: @callee(!);\n 3: debugger;\n}\n");
        same_as_c1(
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
        same_as_c1("main() {\n 1: left,\n    right;\n}\n");
        same_as_c1(
            "main() {\n 1: unmark, unmark, unmark, unmark, unmark, unmark, unmark, unmark, \
             unmark, unmark;\n}\n",
        );
    }

    #[test]
    fn blank_lines_between_body_items() {
        same_as_c1("main() {\n 1: left;\n\n 2: left;\n}\n");
        same_as_c1("main() {\n 1: left;\n\n\n\n 2: left;\n}\n");
    }

    #[test]
    fn a_single_trailing_comment() {
        same_as_c1("main() {\n 1: left; // note\n}\n");
        same_as_c1("main() {\n 1: left;    // over-indented note\n}\n");
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
        same_as_c1("main() {\n 1: left; // a\n 2: right; // b\n 3: mark; // c\n}\n");
        // An uncommented statement in the middle ends the run after
        // statement 1 and starts a new one at statement 3 — TWO lone
        // runs, not one of length 2 skipping the gap. Proven by the
        // column-equal counterpart below
        // (`a_run_only_ever_spans_consecutive_members`), where a wrong
        // "skip the gap" rule would visibly misalign 1 and 3.
        same_as_c1("main() {\n 1: left; // a\n 2: right;\n 3: mark; // c\n}\n");
        // A blank line before statement 2 ends the run after statement 1:
        // `!body[j].blank_before` is one of the two conditions extending a
        // run (`has_trailing` is the other), so statement 2 starts its own
        // lone run rather than joining statement 1's. With only two
        // statements, both readings (one broken run vs. two lone ones)
        // print identically (single space each) — the column-equal
        // counterpart below (`a_blank_line_ends_a_run`) is where the two
        // readings diverge visibly.
        same_as_c1("main() {\n 1: left; // a\n\n 2: right; // b\n}\n");
        // A statement long enough to push its own comment past the run's
        // column would decide the run's column for everyone IF this run
        // were aligned — it isn't (columns 11 vs 44), so both statements
        // still get one space. The real "decides the column" case is
        // `an_aligned_run_shares_the_widest_reformatted_line` below.
        same_as_c1(
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
        same_as_c1("main() {\n 1: left;        // a\n 2: left, right; // b\n}\n");
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
        same_as_c1(
            "main() {\n 1: left;        // a\n\n 2: left, right; // b\n 3: mark;        // c\n}\n",
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
        same_as_c1("main() {\n 1: left;        // a\n 2: right;\n 3: mark;        // c\n}\n");
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
        same_as_c1(
            "main() {\n 1: right;       // a\n    check(1, 3); // b\n 3: left; // a comment \
             long enough that keeping it aligned would overflow eighty columns\n}\n",
        );
        same_as_c1(
            "volatile main() {\n 1: right;       // a\n    check(1, 3); // b\n 3: left; // a \
             comment long enough that keeping it aligned would overflow eighty columns\n}\n",
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
        same_as_c1("main() {\n 999999999:\n    left;    // a\n 2: right;   // b\n}\n");
    }

    /// `comment_w` is measured on [`super::normalize_comment_text`]'s
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
        same_as_c1(
            "main() {\n 1: left;  // xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx  \
             \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\n 2: right; // b\n}\n",
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
        same_as_c1(
            "main() {\n 1: left;  // xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n 2: right; // b\n}\n",
        );
        same_as_c1(
            "main() {\n 1: left;  // xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n 2: right; // b\n}\n",
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
        same_as_c1("main() {\n 1: left;  // a\n 2: right; // b\n 3: mark;       // c\n}\n");
    }

    #[test]
    fn trailing_comments_on_declarations() {
        same_as_c1("use std::goToEnd; // note\n");
        same_as_c1("main() {\n 1: left;\n} // after the brace\n");
        same_as_c1("namespace n {\n} // after the namespace\n");
    }

    #[test]
    fn an_own_line_comment_is_not_a_trailing_one() {
        same_as_c1("main() {\n 1: left;\n // own line\n 2: left; // trailing\n}\n");
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
        same_as_c1("main() {\n 1: left; // t\n // own line after trailing\n 2: right;\n}\n");
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
        same_as_c1("main() {\n 1: left;    // a\n // own line\n 2: right;   // b\n}\n");
    }

    /// Pins the two comment-surface guards still standing after this task
    /// (mirrors `a_comment_nested_inside_a_use_path_is_not_silently_dropped`'s
    /// role for task 2's own guard): each fixture is otherwise a valid,
    /// parseable comment-bearing program that a shallower scan — or a
    /// guard mixed up with a neighboring one — could let slip past
    /// silently instead of panicking loudly. Two guards this list used
    /// to pin — a statement's trailing comment and a function's
    /// close-brace comment — are GONE: this task implements both (see
    /// `an_alignment_run_and_its_boundaries` and
    /// `trailing_comments_on_declarations` above).
    #[test]
    #[should_panic(expected = "interior list comments are task 6's surface")]
    fn interior_comma_comment_guard_fires() {
        let _ = format_green("main() {\n 1: left, /* x */ right;\n}\n");
    }

    #[test]
    #[should_panic(expected = "a function's open-brace trailing comment is task 6's surface")]
    fn function_open_brace_comment_guard_fires() {
        let _ = format_green("main() { // open\n 1: left;\n}\n");
    }

    /// The middle fixture is also this task's DOUBLE-PRINT guard: `//
    /// between` is BOTH statement 2's own leading run
    /// ([`trivia::leading_comments`]) AND a raw comment token
    /// `print_body`'s own element walk reaches directly — mutating
    /// `claimed.contains(tok)` to always `false` (skipping the check)
    /// makes this exact fixture the one that turns red, since C1 prints
    /// `// between` once but an un-filtered green walk would print it
    /// twice.
    #[test]
    fn own_line_comments_inside_a_body() {
        same_as_c1("main() {\n    // leading\n 1: left;\n}\n");
        same_as_c1("main() {\n 1: left;\n    // between\n 2: left;\n}\n");
        same_as_c1("main() {\n 1: left;\n    // trailing the body\n}\n");
        // A standalone (unclaimed) comment with a blank line before it —
        // `blank_immediately_before(tok)`'s own branch in `print_body`'s
        // Comment arm, not `blank_before_unit`'s (that one only fires for
        // a NODE, and this comment has no following node to attach to).
        same_as_c1("main() {\n 1: left;\n\n    // standalone with blank\n}\n");
    }

    #[test]
    fn a_comment_run_keeps_its_internal_gap() {
        same_as_c1("main() {\n    // far\n\n    // near\n 1: left;\n}\n");
    }

    #[test]
    fn block_comments() {
        same_as_c1("main() {\n    /* one line */\n 1: left;\n}\n");
        same_as_c1(
            "main() {\n    /* a block comment\n       spanning two lines */\n 1: left;\n}\n",
        );
        // Empty comments — `normalize_comment_text`'s per-line `trim_end`
        // has nothing to trim either way, but these are real lexed
        // shapes, not merely untested `text` values.
        same_as_c1("main() {\n    //\n 1: left;\n}\n");
        same_as_c1("main() {\n    /**/\n 1: left;\n}\n");
    }

    #[test]
    fn comments_around_a_nested_function() {
        same_as_c1(
            "main() {\n    // about step\n    step() {\n 1: left;\n    }\n\n    @step();\n}\n",
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
        same_as_c1("main() {\n    step() {\n 1: left;\n    } // after nested\n 2: left;\n}\n");
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
        same_as_c1("? one\n// interloper\n? two\nmain() {\n 1: left;\n}\n");
        // After a gap following the run's only line — DOC_RUN's own
        // next sibling, not a child.
        same_as_c1("? one\n\n// after a gap\nmain() {\n 1: left;\n}\n");
        // Directly between the run's last line and the declaration,
        // no blank either side.
        same_as_c1("? one\n// trailing before fn\nmain() {\n 1: left;\n}\n");
        // The same shape, with a blank line before the trailing comment.
        same_as_c1("? one\n\n// trailing before fn\nmain() {\n 1: left;\n}\n");
        // More than one trailing comment in a row, still DOC_RUN's own
        // siblings, not its children.
        same_as_c1("? one\n// c1\n// c2\nmain() {\n 1: left;\n}\n");
        // A blank line AFTER the trailing comment, before the header —
        // `blank_before_header`'s own branch, computed off the LAST
        // trailing comment rather than off `DOC_RUN` itself.
        same_as_c1("? one\n// c1\n\nmain() {\n 1: left;\n}\n");
        // A blank line BETWEEN two run items, INSIDE `print_doc_run`'s
        // own loop — distinct from every gap above, which all sit
        // outside the run (before it, or after its last line). One doc
        // line's own kind, one comment token's, so both branches of the
        // dispatch below the blank check get their own blank-before
        // proof.
        same_as_c1("? one\n\n? two\nmain() {\n 1: left;\n}\n");
        same_as_c1("? one\n\n// mid\n? two\nmain() {\n 1: left;\n}\n");
    }

    /// A comment immediately before a bound doc run — `leading_comments`
    /// on the `FUNCTION` node (which retro-wraps the run as its own
    /// first child) sees this comment as its OWN preceding sibling, one
    /// level up from anything [`print_doc_run`]/[`doc_run_trailing_comments`]
    /// walk. Exercises `print_body`'s `claimed` set against a
    /// doc-run-bound nested function, at body indent.
    #[test]
    fn a_comment_leads_a_bound_doc_run() {
        same_as_c1("// lead\n? doc\nmain() {\n 1: left;\n}\n");
        same_as_c1(
            "main() {\n    // about step\n    ? step doc\n    step() {\n 1: left;\n    }\n}\n",
        );
    }

    // Ported branches that no fixture above reaches, each pinned by a
    // C1 unit test in `fmt/mod.rs` that Task 7 deletes along with the
    // copy it pins — this module's own duplicate needs its own
    // coverage before that happens (mirrors `comma_group_layout`'s own
    // rationale, applied to the branches review flagged as untouched
    // by mutation).

    /// `command_column`'s `base_body_indent.max(p + 2)` only differs from
    /// a `p + 1` mutant at the residue class `p ≡ 3 (mod 4)`: `p = 3`
    /// (from a two-digit inline label `"12:"`) makes `p + 2 == 5` round
    /// up to command column 8, while `p + 1 == 4` would round up to 4
    /// instead — every other fixture in this module uses a `p` that
    /// rounds to the same multiple of 4 either way.
    #[test]
    fn command_column_rounds_up_from_the_plus_two_margin() {
        same_as_c1("main() {\n 12: left;\n}\n");
    }

    /// A `?`/`!` line with an empty payload prints as the bare sigil — a
    /// real lexed shape (a doc paragraph break), not merely a `text`
    /// value this printer happens never to receive. C1 pins the same
    /// shape in `fmt/mod.rs`'s `empty_doc_line_prints_bare_sigil_as_a_paragraph_break`.
    #[test]
    fn a_bare_doc_line_is_a_paragraph_break() {
        same_as_c1("? one\n?\n? two\nmain() {\n 1: left;\n}\n");
    }

    /// `render_check_arm`'s `CheckArm::Return` arm (`check(..., !)`) —
    /// every other `check` fixture in this module uses two `Label` arms.
    #[test]
    fn check_arm_return() {
        same_as_c1("main() {\n 1: check(!, 1);\n}\n");
        same_as_c1("main() {\n 1: check(!, !);\n}\n");
    }

    /// `render_builtin_successor`'s non-`FallThrough` branch (the
    /// `(...)` wrapping a written label or `!`) — no fixture above gives
    /// a builtin its own successor.
    #[test]
    fn a_builtin_with_a_successor() {
        same_as_c1("main() {\n 1: left(5);\n}\n");
        same_as_c1("main() {\n 1: mark(!);\n}\n");
    }

    /// `label_margin`'s `None` arm, reached under `label_break` (an
    /// own-line label wide enough that even the strict 1-space margin
    /// doesn't fit) — the `labels_stacked_and_own_line` fixtures above
    /// only exercise the `Some` arm.
    #[test]
    fn label_margin_overflow_under_label_break() {
        same_as_c1("main() {\n 999999999:\n    left;\n}\n");
    }
}
