//! Lossless concrete syntax tree (CST) node types for `.pmc` — one unified
//! lossless CST for the parser, formatter, and LSP.
//!
//! **Types only — nothing here is built or consumed yet.** A future
//! `parse_cst` produces a [`Cst`] from a `WithComments` token stream, and
//! a future `lower_cst` copies it into the existing [`crate::parser::Program`]
//! AST (the compiler's and lint's unchanged path); the pretty-printer
//! walks the [`Cst`] directly. Both are out of scope for this module.
//!
//! # The lossless contract
//!
//! Where the AST (`docs/language.md` (program structure); `Program` in
//! [`crate::parser`]) flattens for the compiler's convenience, the CST
//! keeps the source shape a pretty-printer needs to reprint faithfully:
//!
//! - **Top-level item order and namespace-block boundaries are kept as
//!   written**, including reopening: the AST tags every declaration with
//!   a flattened `ns: Vec<String>` path and merges same-path blocks by
//!   scope; the CST instead nests a [`NamespaceCst`]'s own
//!   `items: Vec<TopItem>`, one node per `namespace NAME { … }` block as
//!   the author wrote it — two blocks reopening the same name are two
//!   sibling [`TopKind::Namespace`] entries, not one.
//! - **Statements and nested function definitions interleave in source
//!   order.** The AST hoists nested definitions into `Function::nested`,
//!   losing their position relative to the surrounding statements; the
//!   CST's [`FunctionCst::body`] is one `Vec<BodyItem>` with
//!   [`BodyKind::Statement`] and [`BodyKind::Nested`] interleaved exactly
//!   as written.
//! - **Statement internals are reused, not redefined.** [`StatementCst`]
//!   embeds [`crate::parser::Label`] and [`crate::parser::Item`]
//!   (in turn built from [`crate::parser::Builtin`],
//!   [`crate::parser::Successor`], [`crate::parser::CheckArm`]) verbatim,
//!   so [`crate::parser::lower_cst`] hands them straight to
//!   [`crate::parser::Statement`] with no rebuilding. Each comma-group
//!   entry is a [`CommaItem`] pairing the parser's [`Item`] with any
//!   comment trivia that precedes it INSIDE the group (`a, /* x */ b;`),
//!   so a formatter never loses a mid-group comment; `lower_cst` maps
//!   `items.iter().map(|ci| ci.item.clone())` to the AST's flat
//!   `Vec<Item>` and drops the trivia.
//! - **`label_break`** records whether the author put a newline after a
//!   statement's final label `:` (`docs/language.md`'s own-line-label
//!   shape; the design doc's Formatting rules section "Own-line labels")
//!   — the printer needs this to preserve the author's choice; it never
//!   infers or overrides it.
//! - **Comments are trivia nodes at their real source position**, not a
//!   side-channel list — see "Comment placement" below.
//! - **Blank-line presence between items is a bool, not a count.** Each
//!   [`TopItem`]/[`BodyItem`] carries `blank_before: bool` — "does at
//!   least one blank line precede this element in source" — because the
//!   printer's blank-line policy collapses any run to at most one
//!   (design doc, Decisions → Blank-line policy), so a count is never
//!   needed.
//!
//! Container nodes ([`NamespaceCst`], [`FunctionCst`]) deliberately do
//! NOT carry the AST's lower-copy-computed fields — no `ns` tag, no
//! separate `nested` list, no `local` flag. Those are computed once, by
//! a future `lower_cst`, from the CST's block/interleaving structure;
//! duplicating them here would let the two trees disagree.
//!
//! # Comment placement (trivia)
//!
//! Comments live in the token stream at their real position
//! (`LexMode::WithComments`, [`crate::lexer::Comment`]) and are carried
//! into the CST the same way: a leading own-line comment sits before its
//! node as a [`TopKind::Comment`]/[`BodyKind::Comment`] item in the same
//! `Vec`; a same-line trailing comment rides directly on the node it
//! follows ([`UseCst::trailing`], [`StatementCst::trailing`]). There
//! is no attachment pass — position IS the attachment (the one
//! exception is [`FunctionCst::doc_run`], a REAL attachment pass over
//! `?`/`!` lines — see that field's own doc). A future
//! pretty-printer classifies each comment purely from this structure
//! (design doc, "Comments = trivia-tokens native in the CST"):
//!
//! - **Leading** — a run of `Comment` items with `blank_before: false`
//!   immediately before a non-comment item.
//! - **Trailing** — carried on the preceding node's `trailing` field
//!   (same physical line, after its last token).
//! - **Standalone** — a `Comment` item with `blank_before: true`, itself
//!   followed by another blank before the next item.
//! - **Dangling** — one or more trailing `Comment` items at the end of a
//!   `Vec` with no following node (end of a body/namespace/file).
//! - **c-brace** — a comment on the SAME line as a [`FunctionCst`]'s or
//!   [`NamespaceCst`]'s opening `{` or closing `}` rides that brace's own
//!   line instead of becoming a leading/dangling body item
//!   ([`FunctionCst::open_trailing`] / [`FunctionCst::close_trailing`],
//!   [`NamespaceCst::open_trailing`] / [`NamespaceCst::close_trailing`])
//!   — the general rule "a comment stays on the line where it started"
//!   applies to the brace lines too, not just statement lines.

use mtc_core::diagnostics::Span;

use crate::lexer::Comment;
use crate::parser::{Item, Label};

/// A whole `.pmc` file: top-level items in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cst {
    pub items: Vec<TopItem>,
}

/// One file/namespace-level item, plus whether a blank line precedes it
/// in source (module doc's "Blank-line presence").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopItem {
    pub blank_before: bool,
    pub kind: TopKind,
}

/// A file/namespace-level item as written: an own-line comment, an
/// import, a namespace block, or a function definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopKind {
    /// An own-line comment at file or namespace level (module doc's
    /// "Comment placement").
    Comment(Comment),
    Import(UseCst),
    Namespace(NamespaceCst),
    Function(FunctionCst),
}

/// A same-line trailing comment plus its SOURCE column (`docs/fmt.md`,
/// comments). The column is needed to detect whether the author aligned
/// a RUN of trailing `//`s in source — it plays no role in the AST:
/// `lower_cst` ignores this whole type, same as it ignores [`Comment`]
/// itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrailingComment {
    pub comment: Comment,
    /// 1-based source column of the comment's first character (mirrors
    /// [`crate::lexer::Token::col`]).
    pub col: u32,
}

/// One path within a `use` list, as written (`docs/language.md`
/// (imports)) — mirrors [`crate::parser::Import`] minus its
/// lower-copy-computed `ns` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsePath {
    /// `IDENT (:: IDENT)*`, e.g. `use std::goToEnd;` → `["std", "goToEnd"]`.
    pub path: Vec<String>,
    /// `as NAME` rebinding; `None` if absent.
    pub alias: Option<String>,
    /// Line of this path's first token — matches [`crate::parser::Import::line`]
    /// (a `use` list's paths need not share a line; the grammar puts no
    /// restriction on splitting the list across lines).
    pub line: u32,
    /// Path start → last segment end; an `as` alias is NOT included
    /// (matches [`crate::parser::Import::span`]).
    pub span: Span,
}

/// One `use` declaration list, as the author wrote it — `use a, b;` is
/// ONE node holding two [`UsePath`] entries (fixes the formerly per-path
/// `ImportCst`, which made `use a, b;` and `use a; use b;` indistinguishable
/// and lost the list's grouping — see the design doc's "Imports" rule:
/// fmt "neither reorders nor merges/splits `use` statements"). Each path
/// keeps its own line/span so [`crate::parser::lower_cst`]'s per-path
/// flattening is unaffected by the grouping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseCst {
    /// The list's paths, in source order.
    pub paths: Vec<UsePath>,
    /// Line of the `use` keyword.
    pub line: u32,
    /// First path's start → last path's end; an `as` alias is NOT
    /// included (mirrors each [`UsePath::span`]'s own convention).
    pub span: Span,
    /// A comment on the same source line, after the `;`.
    pub trailing: Option<TrailingComment>,
}

/// One `namespace NAME { … }` block exactly as the author wrote it — a
/// reopened namespace is a SEPARATE sibling `NamespaceCst`, never merged
/// (module doc's "namespace-block boundaries + reopening").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceCst {
    pub name: String,
    /// Span of the `NAME` token alone.
    pub name_span: Span,
    /// Line of the `namespace` keyword.
    pub line: u32,
    /// Extent: the `namespace` keyword's start → the closing `}`'s end.
    /// For hit-testing and document-symbol ranges.
    pub span: Span,
    /// Body items in source order; may itself contain nested
    /// [`TopKind::Namespace`] blocks.
    pub items: Vec<TopItem>,
    /// Comment(s) on the SAME physical line as the opening `{`, before
    /// the first body item — e.g. `namespace ns { // note` (module doc's
    /// "Comment placement", "c-brace" fix, mirrors
    /// [`FunctionCst::open_trailing`]). Empty when no such comment
    /// exists. Any comment here forces the body onto its own line below.
    pub open_trailing: Vec<Comment>,
    /// A comment on the SAME physical line as the closing `}` — e.g.
    /// `} // t` (mirrors [`FunctionCst::close_trailing`]). `None` when
    /// absent.
    pub close_trailing: Option<Comment>,
}

/// One function definition (top-level or nested) exactly as written —
/// no `ns` tag, no `nested` list, no `local` flag (module doc's
/// container-node note; a future `lower_cst` computes all three from
/// this node's position in the tree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCst {
    pub name: String,
    /// Span of the name token alone.
    pub name_span: Span,
    /// Line of the name token.
    pub line: u32,
    /// Column of the name token.
    pub col: u32,
    /// Extent: the header's first token → the closing `}`'s end — the
    /// `export` keyword's start when present (`export name() {` is one
    /// header form),
    /// otherwise the name token's start. A nested function is never
    /// exported, so its extent always starts at its name token. For
    /// hit-testing and document-symbol ranges.
    pub span: Span,
    /// `export` (contextual keyword) or `main` at top level. A nested
    /// function is never exported.
    pub exported: bool,
    /// Whether the literal `export` keyword was WRITTEN in source — unlike
    /// `exported`, this does NOT fold in top-level `main`'s auto-export
    /// (`docs/language.md`: `main` is always the entry regardless of
    /// spelling). The printer reads this, never `exported`, to decide
    /// whether to emit the token: `export main() { … }` keeps `export`,
    /// bare `main() { … }` stays bare, both compile identically.
    /// [`crate::parser::lower_cst`] ignores this field — the AST's
    /// `exported` is computed exactly as before.
    pub has_export: bool,
    /// Body items in source order — statements, own-line comments, and
    /// nested function definitions interleaved as written (module doc's
    /// "Statements and nested function definitions interleave").
    pub body: Vec<BodyItem>,
    /// Comment(s) on the SAME physical line as the opening `{`, before
    /// the first body item — e.g. `f() { // note` or
    /// `f() { /* c */ right; }` (module doc's "Comment placement",
    /// "c-brace" fix). Empty when no such comment exists. Unlike a
    /// mid-comma-group [`CommaItem::leading`] run, ANY comment here
    /// (block or line) forces the body onto its own line below — a
    /// comment glued to `{` never shares its line with the first
    /// statement's code.
    pub open_trailing: Vec<Comment>,
    /// A comment on the SAME physical line as the closing `}` — e.g.
    /// `} // t`. `None` when absent.
    pub close_trailing: Option<Comment>,
    /// The `?`/`!` run bound to this declaration (docs/language.md (doc
    /// lines)), in source order; empty when the function is
    /// undocumented. Unlike every other trivia field in this module,
    /// this IS an attachment pass — `parse_cst` walks past blank lines,
    /// ordinary comments, and the run's own order/attribute rules to
    /// bind it to the NEXT `FunctionCst` at its scope (a run with
    /// anything else next is a `DanglingDocRun` compile error, not a
    /// silently dropped item). `lower_cst` ignores this field for now;
    /// a later reduction copies it onto the AST as `FnDoc`.
    pub doc_run: Vec<DocRunItem>,
}

/// One line of a [`FunctionCst::doc_run`], plus whether a blank line
/// precedes it in source (same `blank_before` convention as every other
/// CST item list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRunItem {
    pub blank_before: bool,
    pub kind: DocRunKind,
}

/// A doc/attention run's own line shapes (docs/language.md (doc lines)):
/// a `?` doc line, a `!` attention line, or an ordinary comment
/// interleaved within/after the run (module doc's "Comment placement"
/// applies here too — position is still the attachment for a comment
/// INSIDE a run; only the run's own binding to its `FunctionCst` is a
/// real attachment pass, per [`FunctionCst::doc_run`]'s doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocRunKind {
    /// A `?` line. `text` is the lexer's payload verbatim (raw text
    /// after the sigil, minus one canonical leading space if present) —
    /// unprocessed, so a pretty-printer reprints it byte-for-byte.
    Doc { text: String, span: Span },
    /// A `!` line. `attr` is `Some` when the payload opens with a valid
    /// `[ident]` attribute (v1: only `[deprecated]` is accepted —
    /// anything else is a parse-time `UnknownAttribute` error, so by
    /// the time a `FunctionCst` exists, every `Some` here already named
    /// `"deprecated"`). `text` is the FULL raw payload verbatim,
    /// attribute prefix included when present — mirrors `Doc::text`'s
    /// unprocessed-token convention; a consumer that only wants the
    /// free-form message recovers it from `text` using `attr`'s own
    /// span.
    Attention {
        attr: Option<AttrCst>,
        text: String,
        span: Span,
    },
    /// An ordinary `//`/`/* */` comment inside the run (between run
    /// lines, or between the run's last line and the bound
    /// declaration).
    Comment(Comment),
}

/// An attention line's leading `[ident]` attribute (docs/language.md
/// (doc lines)): v1 accepts exactly `"deprecated"`. `span` covers the
/// identifier alone, not the surrounding brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrCst {
    pub name: String,
    pub span: Span,
}

/// One function-body item, plus whether a blank line precedes it in
/// source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyItem {
    pub blank_before: bool,
    pub kind: BodyKind,
}

/// A function-body item as written: an own-line comment, a statement, or
/// a nested function definition — in source order, never separated into
/// distinct lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyKind {
    /// An own-line comment inside a function body (module doc's
    /// "Comment placement").
    Comment(Comment),
    Statement(StatementCst),
    Nested(FunctionCst),
}

/// One comma-group entry: the parser's [`Item`] plus any comment trivia
/// that precedes it inside the group. The first entry's `leading` is
/// normally empty; a mid-group comment (`a, /* x */ b;`) attaches as the
/// following entry's `leading` so nothing is dropped. [`crate::parser::lower_cst`]
/// drops `leading` when copying to the AST's flat `Vec<Item>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommaItem {
    pub item: Item,
    pub leading: Vec<Comment>,
    /// Whether the author put a newline before this item, inside its
    /// comma group (`docs/fmt.md`, comma groups) — the first entry's is
    /// always `false`. Set
    /// from token line numbers (item K's first token on a later line than
    /// item K-1's last token), not from comment positions.
    /// [`crate::parser::lower_cst`] drops it too, like `leading`.
    pub newline_before: bool,
}

/// One `;`-terminated statement, reusing the parser's statement-internal
/// types verbatim ([`Label`], [`Item`] via [`CommaItem`]) so
/// [`crate::parser::lower_cst`] hands them straight to
/// [`crate::parser::Statement`] with no rebuilding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementCst {
    pub labels: Vec<Label>,
    pub items: Vec<CommaItem>,
    pub line: u32,
    /// First token of the statement (label or item) through the `;` end
    /// (matches [`crate::parser::Statement::span`]).
    pub span: Span,
    /// Whether the author put a newline after the statement's final
    /// label `:` (`docs/language.md`'s own-line-label shape; the design
    /// doc's Formatting rules → "Own-line labels"). The printer preserves
    /// this choice and never infers or overrides it.
    pub label_break: bool,
    /// A comment on the same source line, after the `;`.
    pub trailing: Option<TrailingComment>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::CommentKind;
    use crate::parser::{Builtin, Successor};

    /// Hand-constructs a namespace containing one exported function whose
    /// body interleaves: a leading comment, a labeled statement with a
    /// trailing comment, a nested function, and a blank-separated
    /// standalone comment. Documents the intended CST shape end to end,
    /// and asserts the lossless round-trip contract: `clone() == self`.
    #[test]
    fn hand_built_cst_round_trips_through_clone_and_eq() {
        let dummy_span = Span::new(1, 1, 1, 1);

        let leading = BodyItem {
            blank_before: false,
            kind: BodyKind::Comment(Comment {
                text: "// leading".into(),
                kind: CommentKind::Line,
                own_line: true,
            }),
        };
        let labeled_statement = BodyItem {
            blank_before: false,
            kind: BodyKind::Statement(StatementCst {
                labels: vec![Label {
                    value: 1,
                    span: dummy_span,
                    written: "1".into(),
                }],
                items: vec![CommaItem {
                    item: Item::Builtin {
                        which: Builtin::Right,
                        succ: Successor::FallThrough,
                        succ_span: None,
                        succ_label_span: None,
                        succ_label_written: None,
                        line: 3,
                    },
                    leading: vec![],
                    newline_before: false,
                }],
                line: 3,
                span: dummy_span,
                label_break: false,
                trailing: Some(TrailingComment {
                    comment: Comment {
                        text: "// trailing".into(),
                        kind: CommentKind::Line,
                        own_line: false,
                    },
                    col: 12,
                }),
            }),
        };
        let nested_fn = BodyItem {
            blank_before: false,
            kind: BodyKind::Nested(FunctionCst {
                name: "g".into(),
                name_span: dummy_span,
                line: 4,
                col: 5,
                span: dummy_span,
                exported: false,
                has_export: false,
                body: vec![],
                open_trailing: vec![],
                close_trailing: None,
                doc_run: vec![],
            }),
        };
        let standalone = BodyItem {
            blank_before: true,
            kind: BodyKind::Comment(Comment {
                text: "// standalone".into(),
                kind: CommentKind::Line,
                own_line: true,
            }),
        };

        let f = FunctionCst {
            name: "f".into(),
            name_span: dummy_span,
            line: 2,
            col: 5,
            span: dummy_span,
            exported: true,
            has_export: true,
            body: vec![leading, labeled_statement, nested_fn, standalone],
            open_trailing: vec![],
            close_trailing: None,
            doc_run: vec![],
        };

        let ns = NamespaceCst {
            name: "ns".into(),
            name_span: dummy_span,
            line: 1,
            span: dummy_span,
            items: vec![TopItem {
                blank_before: false,
                kind: TopKind::Function(f),
            }],
            open_trailing: vec![],
            close_trailing: None,
        };

        let cst = Cst {
            items: vec![TopItem {
                blank_before: false,
                kind: TopKind::Namespace(ns),
            }],
        };

        // Field access into the tree, per the shape above.
        let TopKind::Namespace(ns) = &cst.items[0].kind else {
            panic!("expected a namespace item");
        };
        assert_eq!(ns.name, "ns");
        let TopKind::Function(f) = &ns.items[0].kind else {
            panic!("expected a function item");
        };
        assert_eq!(f.name, "f");
        assert!(f.exported);
        assert_eq!(f.body.len(), 4);
        assert!(matches!(f.body[0].kind, BodyKind::Comment(_)));
        assert!(matches!(f.body[1].kind, BodyKind::Statement(_)));
        assert!(matches!(f.body[2].kind, BodyKind::Nested(_)));
        assert!(f.body[3].blank_before);

        // The lossless round-trip contract: cloning must reproduce an
        // equal tree (derived Clone + PartialEq/Eq on every node).
        assert_eq!(cst.clone(), cst);
    }
}
