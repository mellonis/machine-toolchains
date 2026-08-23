//! The green-tree `.pmc` printer (`docs/pmt/fmt.md`,
//! `docs/core.md` (syntax tree)) — a SEPARATE, parallel implementation
//! of [`super::format`] built directly on the lossless green tree
//! instead of the hand-typed C1 CST. It grows surface by surface across
//! several plans; the C1 printer in [`super`] stays untouched as the
//! differential oracle every widened surface is checked against (this
//! module's own `tests`), until the corpus-wide cutover retires it.
//!
//! **Scope of this module today**: the outermost shapes only — the file
//! itself, standalone/leading comments between top-level items, `use`
//! declarations (paths and aliases; a use list's own interior comments
//! are a later plan's surface), and namespaces (including their
//! same-line open/close-brace comments and nested/reopened namespaces).
//! A `FUNCTION` node, or any other shape this module does not yet cover,
//! hits an explicit `unreachable!` naming the plan that owns it — see
//! [`print_item`] — so a test that strays outside the covered surface
//! fails loudly instead of silently printing something wrong.
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
use crate::lexer::{LexMode, lex_with};
use crate::parser::parse_green_from_tokens;
use crate::syntax::{NamespaceView, PmcKind, UseDeclView, UsePathView};

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
    // Threaded through every print function from here down: no surface
    // this plan covers needs source columns yet, but a later plan's
    // trailing-comment alignment does, and it should not have to thread
    // a new parameter through this whole call chain to get one.
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

/// The elements strictly between a `NAMESPACE` node's `{` and `}` — the
/// window [`print_namespace`] hands to [`print_items`], mirroring the C1
/// `NamespaceCst::items` field this replaces.
fn namespace_interior(node: &SyntaxNode) -> impl Iterator<Item = SyntaxElement> + '_ {
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
        unreachable!("FUNCTION is task 3's surface; `format_green` must not be called on it yet")
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
        namespace_interior(node),
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
}
