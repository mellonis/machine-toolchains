//! Lossless concrete syntax tree (CST) node types for `.tmc` — the front-end
//! mirror of the `.pmc` CST in the sibling PM-1 crate.
//!
//! `parse_cst` builds a [`Cst`] from a token stream (a `WithComments` stream
//! for the formatter/language server that phase 7 adds, or the compiler's
//! comment-free stream); `lower_cst` copies it into the flat
//! [`crate::parser::Program`] AST that the rest of the front end consumes. The
//! pretty-printer/LSP (phase 7) walk the [`Cst`] directly and must attach
//! without reworking it — hence the lossless obligation below.
//!
//! # The lossless contract
//!
//! The AST flattens for the compiler's convenience (namespaces stamped as a
//! `ns` path, machine bodies split into tapes + behavior, doc runs reduced to
//! a [`crate::parser::Doc`]); the CST keeps the source shape a printer needs:
//!
//! - **Item order and block boundaries are kept as written**, including
//!   namespace reopening — two `namespace n { … }` blocks are two sibling
//!   [`TopKind::Namespace`] nodes, never merged.
//! - **World-body items interleave in source order.** A [`MachineCst`]'s
//!   `items` is one `Vec<WorldItem>` with tape declarations, states, grafts,
//!   binds, and own-line comments interleaved exactly as written; `lower_cst`
//!   splits them into the AST's separate lists.
//! - **Rule internals are reused, not redefined.** [`RuleCst`] embeds the
//!   parser's [`crate::parser::Rule`] verbatim, so `lower_cst` hands it
//!   straight to the AST with no rebuilding.
//! - **Comments are trivia at their real source position** (module-level
//!   own-line comments as [`TopKind::Comment`] items, same-line trailing
//!   comments riding the node they follow, brace-line comments on
//!   `open_trailing`/`close_trailing`) — position is the attachment; there is
//!   no attachment pass, save the one real one for `?`/`!` doc runs (see
//!   [`AlphabetCst::doc_run`]).
//! - **Blank-line presence is a bool** (`blank_before`): the printer collapses
//!   any run of blank lines to at most one, so a count is never needed.
//!
//! Container nodes deliberately do NOT carry the AST's computed fields (no
//! `ns` tag, no reduced `doc`, no tapes/behavior split) — a future `lower_cst`
//! computes those from the block structure; duplicating them would let the two
//! trees disagree.

use mtc_core::diagnostics::Span;

use crate::lexer::Comment;
use crate::parser::{AlphabetElem, BindingArg, QualName, Rule, Signature};

/// A whole `.tmc` file: top-level items in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cst {
    pub items: Vec<TopItem>,
}

/// One file/namespace-level item, plus whether a blank line precedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopItem {
    pub blank_before: bool,
    pub kind: TopKind,
}

/// A file/namespace-level item as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopKind {
    /// An own-line comment at file or namespace level.
    Comment(Comment),
    Import(UseCst),
    Alphabet(AlphabetCst),
    Namespace(NamespaceCst),
    /// A `routine` or a `graph` — one shape, discriminated by
    /// [`ReuseCst::carrier`].
    Reuse(ReuseCst),
    Machine(MachineCst),
}

/// One path within a `use` list, as written — mirrors
/// [`crate::parser::Import`] minus its lower-copy-computed `ns` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsePath {
    /// `IDENT (:: IDENT)*`, e.g. `use mylib::plusOne;` → `["mylib", "plusOne"]`.
    pub path: Vec<String>,
    /// `as NAME` rebinding; `None` if absent.
    pub alias: Option<String>,
    /// Line of this path's first token.
    pub line: u32,
    /// Path start → last segment end; an `as` alias is NOT included.
    pub span: Span,
}

/// One `use` declaration list — `use a, b;` is ONE node holding two
/// [`UsePath`] entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseCst {
    pub paths: Vec<UsePath>,
    /// Line of the `use` keyword.
    pub line: u32,
    /// First path's start → last path's end.
    pub span: Span,
    /// A comment on the same source line, after the `;`.
    pub trailing: Option<Comment>,
}

/// One `alphabet NAME { … }` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlphabetCst {
    pub name: String,
    pub name_span: Span,
    /// Line of the name token.
    pub line: u32,
    /// Column of the `export`/`alphabet` keyword (the header's first token).
    pub col: u32,
    /// The literal `export` keyword was written.
    pub exported: bool,
    /// Elements in source order.
    pub elems: Vec<AlphabetElem>,
    /// Header first token → closing `}` end.
    pub span: Span,
    /// The `?`/`!` run bound to this declaration, in source order; empty when
    /// undocumented. Unlike every other trivia field, this IS an attachment
    /// pass — `parse_cst` binds a run to the NEXT doc-accepting declaration at
    /// its scope (a run with anything else next is a `DanglingDocRun` error).
    /// `lower_cst` reduces it to [`crate::parser::Doc`].
    pub doc_run: Vec<DocRunItem>,
    /// Comment(s) on the same physical line as the opening `{`.
    pub open_trailing: Vec<Comment>,
    /// A comment on the same physical line as the closing `}`.
    pub close_trailing: Option<Comment>,
}

/// `routine` vs `graph` — a `ReuseCst`'s carrier kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReuseCarrier {
    Routine,
    Graph,
}

/// One `routine`/`graph NAME(sig) { … }` declaration — the two share a shape
/// (signature + world body); [`ReuseCst::carrier`] tells them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseCst {
    pub carrier: ReuseCarrier,
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    /// Column of the header's first token (`export`/`routine`/`graph`).
    pub col: u32,
    pub exported: bool,
    pub sig: Signature,
    /// World-body items in source order (states, grafts, binds, comments).
    pub items: Vec<WorldItem>,
    /// Header first token → closing `}` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub open_trailing: Vec<Comment>,
    pub close_trailing: Option<Comment>,
}

/// The single `machine { … }` block (a program has one; a library has none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineCst {
    /// Line of the `machine` keyword.
    pub line: u32,
    /// Column of the `machine` keyword.
    pub col: u32,
    /// World-body items in source order (tape decls, states, grafts, binds,
    /// comments).
    pub items: Vec<WorldItem>,
    /// `machine` keyword start → closing `}` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub open_trailing: Vec<Comment>,
    pub close_trailing: Option<Comment>,
}

/// One `namespace NAME { … }` block exactly as written — a reopened namespace
/// is a SEPARATE sibling node, never merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceCst {
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    /// `namespace` keyword start → closing `}` end.
    pub span: Span,
    /// Body items in source order; may itself nest [`TopKind::Namespace`].
    pub items: Vec<TopItem>,
    pub doc_run: Vec<DocRunItem>,
    pub open_trailing: Vec<Comment>,
    pub close_trailing: Option<Comment>,
}

/// One world-body item, plus whether a blank line precedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldItem {
    pub blank_before: bool,
    pub kind: WorldKind,
}

/// A world-body item as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldKind {
    /// An own-line comment inside a world body.
    Comment(Comment),
    /// `tape NAME: ALPHABET;` — grammatical only in a `machine` block.
    Tape(TapeCst),
    State(StateCst),
    Graft(GraftCst),
    Bind(BindCst),
}

/// A `tape NAME: ALPHABET;` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeCst {
    pub name: String,
    pub name_span: Span,
    pub alphabet: String,
    pub alphabet_span: Span,
    pub line: u32,
    /// `tape` keyword start → `;` end.
    pub span: Span,
    pub trailing: Option<Comment>,
}

/// A `[entry] state NAME { rules }` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCst {
    pub entry: bool,
    pub name: String,
    pub name_span: Span,
    pub line: u32,
    /// Column of the header's first token (`entry`/`state`).
    pub col: u32,
    /// Rules and own-line comments interleaved in source order.
    pub rules: Vec<RuleItem>,
    /// Header first token → closing `}` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub open_trailing: Vec<Comment>,
    pub close_trailing: Option<Comment>,
}

/// One state-body item, plus whether a blank line precedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleItem {
    pub blank_before: bool,
    pub kind: RuleKind,
}

/// A state-body item as written: an own-line comment or a rule. The rule is
/// boxed — a [`RuleCst`] dwarfs a [`Comment`], so an unboxed variant would
/// bloat every `RuleItem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleKind {
    Comment(Comment),
    Rule(Box<RuleCst>),
}

/// One `pattern -> action ;` rule, embedding the parser's [`Rule`] verbatim
/// plus a same-line trailing comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleCst {
    pub rule: Rule,
    pub trailing: Option<Comment>,
}

/// A `[entry] graft TARGET(args) [as NAME];` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraftCst {
    pub entry: bool,
    pub target: QualName,
    pub args: Vec<BindingArg>,
    /// `as NAME` instance name (name, span); required unless `entry`.
    pub as_name: Option<(String, Span)>,
    pub line: u32,
    /// Header first token → `;` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub trailing: Option<Comment>,
}

/// A `bind TARGET(args) as NAME;` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindCst {
    pub target: QualName,
    pub args: Vec<BindingArg>,
    /// `as NAME` — always present for a bind.
    pub as_name: (String, Span),
    pub line: u32,
    /// `bind` keyword start → `;` end.
    pub span: Span,
    pub doc_run: Vec<DocRunItem>,
    pub trailing: Option<Comment>,
}

/// One line of a doc/attention run, plus whether a blank line precedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRunItem {
    pub blank_before: bool,
    pub kind: DocRunKind,
}

/// A doc/attention run's line shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocRunKind {
    /// A `?` line. `text` is the lexer's payload verbatim.
    Doc { text: String, span: Span },
    /// A `!` line. `attr` is `Some` when the payload opens with a valid
    /// `[ident]` attribute (v1: only `[deprecated]`). `text` is the FULL raw
    /// payload verbatim, attribute prefix included.
    Attention {
        attr: Option<AttrCst>,
        text: String,
        span: Span,
    },
    /// An ordinary comment inside the run.
    Comment(Comment),
}

/// An attention line's leading `[ident]` attribute; `span` covers the
/// identifier alone, not the brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrCst {
    pub name: String,
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::CommentKind;
    use crate::parser::{
        MoveCell, MoveDir, MoveVec, Pattern, PatternCell, PatternCellKind, Transition,
    };

    /// Hand-builds a machine with one state and one rule, plus a leading
    /// comment and a trailing comment, and asserts the lossless round-trip
    /// contract: `clone() == self` over the whole derived tree.
    #[test]
    fn hand_built_cst_round_trips_through_clone_and_eq() {
        let sp = Span::new(1, 1, 1, 1);
        let rule = Rule {
            pattern: Pattern {
                cells: vec![PatternCell {
                    kind: PatternCellKind::Wildcard,
                    binding: None,
                    span: sp,
                }],
                span: sp,
            },
            debugger: false,
            write: None,
            mov: Some(MoveVec {
                cells: vec![MoveCell {
                    dir: MoveDir::Right,
                    span: sp,
                }],
                span: sp,
            }),
            transition: Transition::Goto {
                name: "scan".into(),
                explicit: true,
                span: sp,
            },
            line: 4,
            span: sp,
        };
        let state = StateCst {
            entry: true,
            name: "scan".into(),
            name_span: sp,
            line: 3,
            col: 3,
            rules: vec![
                RuleItem {
                    blank_before: false,
                    kind: RuleKind::Comment(Comment {
                        text: "// leading".into(),
                        kind: CommentKind::Line,
                        own_line: true,
                    }),
                },
                RuleItem {
                    blank_before: false,
                    kind: RuleKind::Rule(Box::new(RuleCst {
                        rule,
                        trailing: Some(Comment {
                            text: "// trailing".into(),
                            kind: CommentKind::Line,
                            own_line: false,
                        }),
                    })),
                },
            ],
            span: sp,
            doc_run: vec![],
            open_trailing: vec![],
            close_trailing: None,
        };
        let machine = MachineCst {
            line: 2,
            col: 1,
            items: vec![WorldItem {
                blank_before: false,
                kind: WorldKind::State(state),
            }],
            span: sp,
            doc_run: vec![],
            open_trailing: vec![],
            close_trailing: None,
        };
        let cst = Cst {
            items: vec![TopItem {
                blank_before: false,
                kind: TopKind::Machine(machine),
            }],
        };

        let TopKind::Machine(m) = &cst.items[0].kind else {
            panic!("expected a machine item");
        };
        let WorldKind::State(s) = &m.items[0].kind else {
            panic!("expected a state item");
        };
        assert!(s.entry);
        assert_eq!(s.name, "scan");
        assert_eq!(s.rules.len(), 2);
        assert!(matches!(s.rules[0].kind, RuleKind::Comment(_)));
        assert!(matches!(s.rules[1].kind, RuleKind::Rule(_)));

        // The lossless round-trip contract: cloning reproduces an equal tree.
        assert_eq!(cst.clone(), cst);
    }
}
