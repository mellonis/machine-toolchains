//! `.pmc` recursive-descent parser (docs/language.md): tokens → AST.

use std::collections::HashSet;

use mtc_core::diagnostics::{Pos, Span};

use crate::compiler::{CompileError, CompileErrorKind};
use crate::cst::{
    AttrCst, BodyItem, BodyKind, CommaItem, Cst, DocRunItem, DocRunKind, FunctionCst, NamespaceCst,
    StatementCst, TopItem, TopKind, TrailingComment, UseCst, UsePath,
};
use crate::lexer::{Comment, Token, TokenKind};

/// docs/language.md: words that cannot name a function.
pub const RESERVED: [&str; 8] = [
    "goto", "check", "left", "right", "mark", "unmark", "halt", "debugger",
];

/// The `.pmc` language acceptance-contract version (docs/language.md):
/// pre-1.0 the version is 0.N and N bumps on ANY grammar change; at a
/// declared 1.0 the axes activate (major = breaking, minor = additive).
/// No patch digit — spec-text corrections are errata;
/// implementation-conformance fixes live in the crate changelog. The
/// sigil-adjacency, reserved-path, and empty-builtin-parens tightenings
/// made this 0.2 (the v1 grammar is retroactively 0.1).
pub const PMC_LANG_VERSION: &str = "0.2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub functions: Vec<Function>,
    pub imports: Vec<Import>,
}

/// One `use` list item: `use a, std::b as c;` yields two of these.
/// Every import declares an external symbol by its FULL `::`-joined
/// path and binds ONE bare name in its declaring scope (alias if
/// present, else the path tail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// `IDENT (:: IDENT)*` — `use std::goToEnd;` → `["std", "goToEnd"]`.
    pub path: Vec<String>,
    /// `as NAME` rebinds the bare name (the declared symbol is unchanged).
    pub alias: Option<String>,
    pub line: u32,
    /// The declaring namespace block's path; empty = file level. The
    /// binding is visible in that block and nested scopes only.
    pub ns: Vec<String>,
    /// Path start → last segment end; an `as` alias is NOT included.
    pub span: Span,
}

impl Import {
    /// The bare name this import binds in its scope.
    pub fn binding(&self) -> &str {
        self.alias.as_deref().unwrap_or_else(|| {
            self.path
                .last()
                .expect("parser: import paths are non-empty")
        })
    }

    /// The full `::`-joined external symbol this import declares.
    pub fn full_path(&self) -> String {
        self.path.join("::")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub line: u32,
    pub col: u32,
    pub name_span: Span,
    pub body: Vec<Statement>,
    /// `export` (contextual keyword) or `main` (always exported).
    pub exported: bool,
    /// Nesting is always local; flatten computes this for top-level
    /// functions as `!exported`.
    pub local: bool,
    /// Nested function definitions (docs/language.md (visibility)), hoisted and visible to
    /// their own siblings and enclosing scope's body; emptied by flatten.
    pub nested: Vec<Function>,
    /// Enclosing namespace path (parser-set on top-level definitions;
    /// nested functions inherit through their top-level ancestor). The
    /// full symbol joins namespaces with `::` and nesting with `.` —
    /// `std::api.helper`.
    pub ns: Vec<String>,
    /// The bound `?`/`!` run (`docs/superpowers/specs/
    /// 2026-07-12-pmc-doc-lines-attributes-design.md`), reduced from
    /// [`crate::cst::FunctionCst::doc_run`] by [`lower_cst`]. `None` for
    /// an undocumented function (an empty `doc_run`); every compiler
    /// pass past `lower_cst` ignores this field — `flatten` copies it
    /// into `Analysis.docs`, keyed by the same fully-qualified name it
    /// already computes, and nothing downstream reads it off `Function`
    /// again.
    pub doc: Option<FnDoc>,
}

/// One function's reduced doc/attention run (docs/superpowers/specs/
/// 2026-07-12-pmc-doc-lines-attributes-design.md): paragraphs from `?`
/// lines, bare-prose `!` lines, and the `[deprecated]` attribute's
/// message, with spans and raw sigil/attribute text dropped — a future
/// hover/lint consumer reads this shape, not the CST's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnDoc {
    /// `?` lines, reduced: consecutive lines join into one paragraph
    /// separated by a single space; an empty `?` line splits paragraphs;
    /// leading/trailing empty `?` lines produce no empty paragraph.
    pub paragraphs: Vec<String>,
    /// Bare-prose `!` lines (no `[attr]` prefix), verbatim, in source
    /// order. The `[deprecated]` line is NOT included here — it is
    /// reduced into `deprecated` instead.
    pub attention: Vec<String>,
    /// `Some(message)` when a `! [deprecated] …` line is present (`""`
    /// when the line carries no message past the attribute); `None`
    /// otherwise. At most one such line can exist — a second is a parse
    /// error (`DuplicateAttribute`) before an AST is ever built.
    pub deprecated: Option<String>,
}

/// A label prefix `N:` — the span runs from the number's start to the
/// colon's END, spanning any interior whitespace (spaced `1 :` is legal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub value: u32,
    pub span: Span,
    /// The number as WRITTEN — digits only, leading zeros preserved.
    /// The printer emits this verbatim instead of re-deriving text from
    /// `value` (docs/fmt.md: fmt never touches a token).
    pub written: String,
}

/// One `;`-terminated statement: an optional run of labels, then one or
/// more comma-separated items. `items.len() > 1` only for comma groups,
/// whose position rules the parser has enforced: `check`/`halt` only
/// last, a successor only on the last item, `goto` never grouped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub labels: Vec<Label>,
    pub items: Vec<Item>,
    pub line: u32,
    /// First token of the statement (label or item) through the `;` end.
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Left,
    Right,
    Mark,
    Unmark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Successor {
    FallThrough,
    Label(u32),
    Return,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckArm {
    Label(u32),
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Builtin {
        which: Builtin,
        succ: Successor,
        /// The `(`…`)` range including both parens; None without parens.
        succ_span: Option<Span>,
        /// The successor's number token span alone (inside the parens,
        /// number only); `Some` iff `succ` is `Successor::Label`.
        succ_label_span: Option<Span>,
        /// The successor's number as WRITTEN (leading zeros preserved);
        /// `Some` iff `succ` is `Successor::Label` — parallels
        /// `succ_label_span`. The printer emits this verbatim instead of
        /// re-deriving text from the `Successor::Label` payload
        /// (docs/fmt.md: fmt never touches a token).
        succ_label_written: Option<String>,
        line: u32,
    },
    Debugger {
        line: u32,
    },
    Call {
        name: String,
        /// Name start → last `::` segment end.
        name_span: Span,
        succ: Successor,
        /// The `(`…`)` range; calls always have parens, so always Some.
        succ_span: Option<Span>,
        /// The successor's number token span alone (inside the parens,
        /// number only); `Some` iff `succ` is `Successor::Label`.
        succ_label_span: Option<Span>,
        /// The successor's number as WRITTEN (leading zeros preserved);
        /// `Some` iff `succ` is `Successor::Label` — parallels
        /// `succ_label_span`.
        succ_label_written: Option<String>,
        line: u32,
    },
    Check {
        marked: CheckArm,
        blank: CheckArm,
        /// `check` keyword start → `)` end.
        span: Span,
        /// The `marked` arm's own token span (a number or `!`).
        marked_span: Span,
        /// The `blank` arm's own token span (a number or `!`).
        blank_span: Span,
        /// The `marked` arm's number as WRITTEN (leading zeros
        /// preserved); `Some` iff `marked` is `CheckArm::Label`.
        marked_written: Option<String>,
        /// The `blank` arm's number as WRITTEN; `Some` iff `blank` is
        /// `CheckArm::Label`.
        blank_written: Option<String>,
        line: u32,
    },
    Halt {
        line: u32,
    },
    Goto {
        label: u32,
        /// The target number token's span.
        label_span: Span,
        /// The target number as WRITTEN (leading zeros preserved). The
        /// printer emits this verbatim instead of re-deriving text from
        /// `label` (docs/fmt.md: fmt never touches a token).
        label_written: String,
        line: u32,
    },
}

fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(n) => format!("`{n}`"),
        TokenKind::Number(v, _) => format!("`{v}`"),
        TokenKind::At => "`@`".into(),
        TokenKind::Bang => "`!`".into(),
        TokenKind::Comma => "`,`".into(),
        TokenKind::Semi => "`;`".into(),
        TokenKind::Colon => "`:`".into(),
        TokenKind::ColonColon => "`::`".into(),
        TokenKind::LParen => "`(`".into(),
        TokenKind::RParen => "`)`".into(),
        TokenKind::LBrace => "`{`".into(),
        TokenKind::RBrace => "`}`".into(),
        TokenKind::Eof => "end of file".into(),
        // Exhaustiveness only: the parser is always fed `lex()` (==
        // `lex_with(_, LexMode::WithoutComments)`), which never emits
        // this variant, so this arm is unreachable in practice.
        TokenKind::Comment(_) => "a comment".into(),
        // Doc/attention lines are semantic tokens the lexer emits on
        // BOTH modes (docs/language.md (doc lines)), so — unlike
        // Comment above — this parser DOES see them. At item position
        // (top level or in a body) a `?`/`!` line starts a run, handled
        // by `Parser::doc_run` before this ever runs; one reaching HERE
        // means it surfaced somewhere a run cannot start — mid-statement
        // (e.g. a successor split across lines inside an open paren
        // group) — where it's just an unexpected token like any other.
        TokenKind::DocLine(_) => "a doc line".into(),
        TokenKind::AttentionLine(_) => "an attention line".into(),
    }
}

/// tokens → AST, via the lossless CST
/// (docs/superpowers/specs/2026-07-07-pmc-fmt-design.md, "Architecture:
/// one unified lossless CST"). The compiler consumes the `Program`; fmt
/// reads the CST directly through [`parse_cst`]. The signature is
/// unchanged from the pre-C1 parser — verified byte-identical, for the
/// whole CST migration, against a frozen pre-C1 reference
/// implementation; that reference parser and its parity harness were
/// removed once the CST-based parser was confirmed a sound replacement.
pub fn parse(tokens: &[Token]) -> Result<Program, CompileError> {
    parse_cst(tokens).map(|cst| lower_cst(&cst))
}

/// tokens → lossless CST. Accepts either a `WithoutComments` stream (the
/// compiler's path, no trivia) or a `WithComments` stream (fmt's path,
/// comments interleaved). Comment tokens are split off up front so the
/// grammar walk over the significant tokens is identical to the pre-C1
/// parser — spans, control flow, and the duplicate-name/-label checks all
/// carry over verbatim. The dropped-in-lowering trivia (`blank_before`,
/// `label_break`, comment nodes, `trailing`, `CommaItem::leading`) is
/// attached from the split-off comments by source position;
/// `CommaItem::newline_before` is attached instead from the significant
/// tokens' own line numbers (it records a source newline, not a comment).
pub fn parse_cst(tokens: &[Token]) -> Result<Cst, CompileError> {
    let mut sig: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut comments: Vec<CommentAt> = Vec::new();
    for t in tokens {
        if let TokenKind::Comment(c) = &t.kind {
            comments.push(CommentAt {
                comment: c.clone(),
                line: t.line,
                col: t.col,
                sig_index: sig.len(),
            });
        } else {
            sig.push(t.clone());
        }
    }
    let items = Parser {
        tokens: &sig,
        pos: 0,
        namespaces: HashSet::new(),
        declared_fns: HashSet::new(),
        comments,
        cpos: 0,
        prev_end_line: 0,
    }
    .file()?;
    Ok(Cst { items })
}

/// Copy a CST into the flat `Program` the compiler consumes — exactly the
/// namespace-flattening + nested-function hoisting the pre-C1 parser did
/// inline. Stamps each definition's enclosing `ns` path, hoists nested
/// functions out of body order into `Function::nested`, and drops all
/// trivia. `local` is left `false` (flatten computes it), matching the
/// pre-C1 parser. Spans/lines/cols are copied verbatim.
pub fn lower_cst(cst: &Cst) -> Program {
    let mut functions = Vec::new();
    let mut imports = Vec::new();
    lower_items(&cst.items, &[], &mut functions, &mut imports);
    Program { functions, imports }
}

fn lower_items(
    items: &[TopItem],
    ns: &[String],
    functions: &mut Vec<Function>,
    imports: &mut Vec<Import>,
) {
    for item in items {
        match &item.kind {
            TopKind::Comment(_) => {}
            TopKind::Import(use_cst) => {
                for p in &use_cst.paths {
                    imports.push(Import {
                        path: p.path.clone(),
                        alias: p.alias.clone(),
                        line: p.line,
                        ns: ns.to_vec(),
                        span: p.span,
                    });
                }
            }
            TopKind::Namespace(nsc) => {
                let mut child = ns.to_vec();
                child.push(nsc.name.clone());
                lower_items(&nsc.items, &child, functions, imports);
            }
            TopKind::Function(f) => functions.push(lower_function(f, ns)),
        }
    }
}

/// Lower one function. Nested functions are hoisted into `nested` (out of
/// body order) and, like the pre-C1 parser, carry an EMPTY `ns` — flatten
/// resolves nesting through the top-level ancestor. `exported` is copied
/// from the CST (the caller stamped top-level `main`'s auto-export);
/// nested functions are never exported. `doc` is [`reduce_doc_run`] over
/// the CST's bound `doc_run`.
fn lower_function(f: &FunctionCst, ns: &[String]) -> Function {
    let mut body = Vec::new();
    let mut nested = Vec::new();
    for bi in &f.body {
        match &bi.kind {
            BodyKind::Comment(_) => {}
            BodyKind::Statement(s) => body.push(Statement {
                labels: s.labels.clone(),
                items: s.items.iter().map(|ci| ci.item.clone()).collect(),
                line: s.line,
                span: s.span,
            }),
            BodyKind::Nested(g) => nested.push(lower_function(g, &[])),
        }
    }
    Function {
        name: f.name.clone(),
        line: f.line,
        col: f.col,
        name_span: f.name_span,
        body,
        exported: f.exported,
        local: false,
        nested,
        ns: ns.to_vec(),
        doc: reduce_doc_run(&f.doc_run),
    }
}

/// Reduce a [`FunctionCst::doc_run`] into an [`FnDoc`] — `None` for an
/// empty run (undocumented). `DocRunKind::Comment` items are transparent:
/// they contribute nothing and never split a paragraph, matching the
/// design doc's "comments/blanks don't participate" rule for the run's
/// own order check. A `?` line's text is the join key; an EMPTY `?` line
/// (the lexer's bare-sigil payload) closes the current paragraph without
/// emitting an empty one, so leading/trailing/repeated blanks are all
/// absorbed. An attention line with `attr.name == "deprecated"` (at most
/// one — a second is rejected at parse time, before any `FunctionCst`
/// exists) is excluded from `attention`; its message is the FULL raw
/// payload's text after the attribute's closing `]`, trimmed — finding
/// `]` in `text` directly is equivalent to (and simpler than) mapping
/// `attr.span.end` back into the string, since `parse_attr` only
/// recognizes `[ident]` at the payload's very start.
fn reduce_doc_run(doc_run: &[DocRunItem]) -> Option<FnDoc> {
    if doc_run.is_empty() {
        return None;
    }
    let mut paragraphs = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut attention = Vec::new();
    let mut deprecated = None;
    for item in doc_run {
        match &item.kind {
            DocRunKind::Doc { text, .. } => {
                if text.is_empty() {
                    if !current.is_empty() {
                        paragraphs.push(current.join(" "));
                        current.clear();
                    }
                } else {
                    current.push(text.as_str());
                }
            }
            DocRunKind::Attention { attr, text, .. } => match attr {
                Some(a) if a.name == "deprecated" => {
                    let close = text.find(']').expect(
                        "parser: a `deprecated`-tagged attention line always has a closing `]`",
                    );
                    deprecated = Some(text[close + 1..].trim().to_string());
                }
                _ => attention.push(text.clone()),
            },
            DocRunKind::Comment(_) => {}
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    Some(FnDoc {
        paragraphs,
        attention,
        deprecated,
    })
}

/// A comment token lifted out of the stream during [`parse_cst`]'s split,
/// remembering where it sat relative to the significant tokens.
struct CommentAt {
    comment: Comment,
    /// The comment's own start line (for `blank_before` gaps).
    line: u32,
    /// The comment's own start column (`docs/superpowers/specs/
    /// 2026-07-07-pmc-fmt-design.md`, "Trailing comments" — the
    /// alignment rule's source-column detection; brief §A).
    col: u32,
    /// Number of significant tokens preceding this comment — the `pos`
    /// the significant-token walk is at when the comment is "pending".
    sig_index: usize,
}

/// [`Parser::top_items`]'s return shape: the items, the block's
/// `close_trailing` comment, and the closing `}` token's own span.
type TopItemsResult = Result<(Vec<TopItem>, Option<Comment>, Option<Span>), CompileError>;

struct Parser<'a> {
    /// Significant (comment-free) tokens only — identical to the
    /// `WithoutComments` stream, so the grammar walk matches the pre-C1
    /// parser exactly.
    tokens: &'a [Token],
    pos: usize,
    /// Every namespace path declared so far (reopened blocks insert the
    /// same path again, harmlessly). Namespace names share the name pool
    /// with function names per scope — a human-clarity rule: since `::`
    /// (namespaces) and `.` (nesting) are distinct separators, `a::x`
    /// and `a.x` cannot collide; the pool rule just stops both spellings
    /// coexisting confusingly in one file.
    namespaces: HashSet<Vec<String>>,
    /// Every `(ns, name)` function declared so far — the pre-C1 parser
    /// scanned its flat `functions` vec for the same-scope duplicate
    /// check; a set keyed on `(ns, name)` is the equivalent membership
    /// test and independent of the CST's block nesting.
    declared_fns: HashSet<(Vec<String>, String)>,
    /// Comments split out of the stream, in source order.
    comments: Vec<CommentAt>,
    /// Cursor into `comments`: everything before it is already attached.
    cpos: usize,
    /// End line of the last emitted CST element, for `blank_before`.
    prev_end_line: u32,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        // Safe: the lexer always appends Eof and bump() never passes it.
        &self.tokens[self.pos]
    }

    fn bump(&mut self) {
        if !matches!(self.tokens[self.pos].kind, TokenKind::Eof) {
            self.pos += 1;
        }
    }

    fn err_at(t: &Token, kind: CompileErrorKind) -> CompileError {
        CompileError {
            span: t.span(),
            kind,
        }
    }

    fn expected(t: &Token, what: &'static str) -> CompileError {
        Self::err_at(
            t,
            CompileErrorKind::Expected {
                what,
                found: describe(&t.kind),
            },
        )
    }

    fn expect(&mut self, kind: &TokenKind, what: &'static str) -> Result<(), CompileError> {
        if &self.peek().kind == kind {
            self.bump();
            Ok(())
        } else {
            Err(Self::expected(self.peek(), what))
        }
    }

    /// Attach every pending comment at or before the current sig position,
    /// returning `(comment, start_line)` in source order. Never drops.
    fn drain_pending(&mut self) -> Vec<(Comment, u32)> {
        let mut out = Vec::new();
        while self.cpos < self.comments.len() && self.comments[self.cpos].sig_index <= self.pos {
            let ca = &self.comments[self.cpos];
            out.push((ca.comment.clone(), ca.line));
            self.cpos += 1;
        }
        out
    }

    /// Like [`Self::drain_pending`] but dropping line info — for
    /// mid-comma-group leading trivia, which carries no `blank_before`.
    fn drain_pending_comments(&mut self) -> Vec<Comment> {
        self.drain_pending().into_iter().map(|(c, _)| c).collect()
    }

    /// Take the one same-line trailing comment after a `;` (the pending
    /// comment that follows code on `end_line`), if any. Carries the
    /// comment's source column (brief §A) alongside it.
    fn take_trailing(&mut self, end_line: u32) -> Option<TrailingComment> {
        if self.cpos < self.comments.len() {
            let ca = &self.comments[self.cpos];
            if ca.sig_index <= self.pos && !ca.comment.own_line && ca.line == end_line {
                let out = TrailingComment {
                    comment: ca.comment.clone(),
                    col: ca.col,
                };
                self.cpos += 1;
                return Some(out);
            }
        }
        None
    }

    /// The whole file is the `ns == []` namespace level.
    fn file(mut self) -> Result<Vec<TopItem>, CompileError> {
        self.top_items(&[], None).map(|(items, _, _)| items)
    }

    /// Collects a doc/attention run (docs/language.md (doc lines))
    /// starting at the current position — the caller has already
    /// confirmed `self.peek()` is `DocLine`/`AttentionLine`. A run is one
    /// optional contiguous `?` block then one optional contiguous `!`
    /// block; a `?` reached after the run has entered its `!` block is
    /// `DocLineOrder` (covers both interleaving and whole-run wrong
    /// order — a single "have we seen `!` yet" flag catches both, since
    /// ANY `?` after the first `!` violates the fixed order). Blank
    /// lines and ordinary comments are tolerated within/after the run
    /// without affecting that order check (spec: "comments/blanks don't
    /// participate") — `drain_pending` after each consumed line picks up
    /// anything sitting between it and whatever comes next, including
    /// comments between the run's last line and the bound declaration.
    /// An attention line's `[ident]` attribute (if any) is validated
    /// against the v1 vocabulary here, since this is the only place the
    /// run's lines are walked in order. Returns the run's items plus the
    /// run's OWN first line's span, for the caller's `DanglingDocRun`
    /// error if what follows isn't the declaration the run must bind to.
    fn doc_run(&mut self) -> Result<(Vec<DocRunItem>, Span), CompileError> {
        let first_span = self.peek().span();
        let mut items: Vec<DocRunItem> = Vec::new();
        let mut seen_attention = false;
        let mut seen_deprecated: Option<Span> = None;
        let mut prev_end_line = self.prev_end_line;
        loop {
            let t = self.peek().clone();
            match &t.kind {
                TokenKind::DocLine(text) => {
                    if seen_attention {
                        return Err(Self::err_at(&t, CompileErrorKind::DocLineOrder));
                    }
                    let text = text.clone();
                    self.bump();
                    let blank_before = t.line > prev_end_line + 1;
                    prev_end_line = t.line;
                    items.push(DocRunItem {
                        blank_before,
                        kind: DocRunKind::Doc {
                            text,
                            span: t.span(),
                        },
                    });
                }
                TokenKind::AttentionLine(text) => {
                    let text = text.clone();
                    self.bump();
                    seen_attention = true;
                    let attr = Self::parse_attr(&text, &t);
                    if let Some(a) = &attr {
                        if a.name == "deprecated" {
                            if seen_deprecated.is_some() {
                                return Err(CompileError {
                                    span: a.span,
                                    kind: CompileErrorKind::DuplicateAttribute,
                                });
                            }
                            seen_deprecated = Some(a.span);
                        } else {
                            return Err(CompileError {
                                span: a.span,
                                kind: CompileErrorKind::UnknownAttribute(a.name.clone()),
                            });
                        }
                    }
                    let blank_before = t.line > prev_end_line + 1;
                    prev_end_line = t.line;
                    items.push(DocRunItem {
                        blank_before,
                        kind: DocRunKind::Attention {
                            attr,
                            text,
                            span: t.span(),
                        },
                    });
                }
                _ => break,
            }
            for (comment, cline) in self.drain_pending() {
                let blank_before = cline > prev_end_line + 1;
                prev_end_line = cline + comment.text.matches('\n').count() as u32;
                items.push(DocRunItem {
                    blank_before,
                    kind: DocRunKind::Comment(comment),
                });
            }
        }
        self.prev_end_line = prev_end_line;
        Ok((items, first_span))
    }

    /// Parses a leading `[ident]` attribute off an attention line's raw
    /// payload (docs/language.md (doc lines)): the exact shape `[`,
    /// ident, `]` at the payload's very start — anything else means "no
    /// attribute", the whole line is free prose (`None`). `token` is the
    /// `AttentionLine` token the payload came from, needed to translate
    /// the identifier's position WITHIN the payload string into a real
    /// source `Span`: `token.len` counts the sigil plus the RAW payload
    /// (before the lexer's canonical one-leading-space strip), while
    /// `text.chars().count()` counts the STORED (possibly one shorter)
    /// payload — the difference is exactly the 0-or-1 leading space that
    /// strip removed, and `[` sits right after it.
    fn parse_attr(text: &str, token: &Token) -> Option<AttrCst> {
        let chars: Vec<char> = text.chars().collect();
        if chars.first() != Some(&'[') {
            return None;
        }
        let close = chars.iter().position(|&c| c == ']')?;
        let ident_chars = &chars[1..close];
        let (first, rest) = ident_chars.split_first()?;
        if !(first.is_alphabetic() || *first == '_') {
            return None;
        }
        if !rest.iter().all(|c| c.is_alphanumeric() || *c == '_') {
            return None;
        }
        let name: String = ident_chars.iter().collect();
        let stripped = token.len - 1 - text.chars().count() as u32;
        let bracket_col = token.col + 1 + stripped;
        let start_col = bracket_col + 1; // past the `[`
        let end_col = start_col + name.chars().count() as u32;
        Some(AttrCst {
            name,
            span: Span::new(token.line, start_col, token.line, end_col),
        })
    }

    /// True iff, ignoring any doc run just collected, the current
    /// position starts a top-level (or namespace-level) function
    /// declaration in `top_items`'s own grammar — i.e. none of the
    /// OTHER shapes at this level (`use IDENT`, `namespace IDENT {`, the
    /// `namespace|use|export {` needs-a-name hint, a top-level
    /// statement, or the scope's own terminator/Eof) claim the token
    /// first. Read-only (peeks only, never advances `self.pos`) and
    /// mirrors `top_items`'s dispatch conditions exactly, in the same
    /// order, so a doc run's attachment decision matches what the rest
    /// of that loop would do with the same token.
    fn next_is_top_level_function_start(&self, terminator: Option<&TokenKind>) -> bool {
        let t = self.peek();
        if matches!(t.kind, TokenKind::Eof) {
            return false;
        }
        if let Some(term) = terminator
            && &t.kind == term
        {
            return false;
        }
        if let TokenKind::Ident(w) = &t.kind {
            let next_is_ident = matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::Ident(_))
            );
            let next_is_lbrace = matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::LBrace)
            );
            if w == "namespace" && next_is_ident {
                let next2_is_lbrace = matches!(
                    self.tokens.get(self.pos + 2).map(|t| &t.kind),
                    Some(TokenKind::LBrace)
                );
                if next2_is_lbrace {
                    return false; // `namespace NAME { … }`
                }
            }
            if w == "use" && next_is_ident {
                return false; // `use NAME, …;` import list
            }
            if matches!(w.as_str(), "namespace" | "use" | "export") && next_is_lbrace {
                return false; // `namespace {` / `use {` / `export {}` hint case
            }
            if RESERVED.contains(&w.as_str()) {
                return false; // top-level statement
            }
        }
        if matches!(t.kind, TokenKind::At) {
            return false; // top-level statement (`@f();`)
        }
        true
    }

    /// True iff the current position starts a nested function definition
    /// (`IDENT ( ) {` — visibility-only nesting): shared by the doc-run
    /// dangling check and the body loop's own nested-definition
    /// dispatch, so both read the identical shape. Read-only.
    fn next_is_nested_function_start(&self) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(w)
                if !RESERVED.contains(&w.as_str()))
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::LParen)
            )
            && matches!(
                self.tokens.get(self.pos + 2).map(|t| &t.kind),
                Some(TokenKind::RParen)
            )
            && matches!(
                self.tokens.get(self.pos + 3).map(|t| &t.kind),
                Some(TokenKind::LBrace)
            )
    }

    /// One namespace level's item loop, building `TopItem`s in source
    /// order. Handles `use` (legal at any namespace depth, never in
    /// function bodies), `namespace NAME { … }` (contextual; recurse with
    /// the extended path), `export`, and function definitions, and
    /// interleaves own-line comments as [`TopKind::Comment`] items.
    /// `terminator` is `Some(RBrace)` inside a block, `None` at file level
    /// (ends at Eof). Duplicate-name checks run here, exactly as the pre-C1
    /// parser did. Returns the items, the block's `close_trailing` comment
    /// (c-brace fix, mirrors `function`'s close_trailing), and the closing
    /// `}` token's own span (for the caller's `NamespaceCst::span` extent)
    /// — both always `None` when `terminator` is `None` (a file has no
    /// closing brace).
    fn top_items(&mut self, ns: &[String], terminator: Option<&TokenKind>) -> TopItemsResult {
        let mut items: Vec<TopItem> = Vec::new();
        loop {
            // Own-line comments (leading/standalone/dangling) become their
            // own items at this level, in source position.
            for (comment, cline) in self.drain_pending() {
                let blank_before = cline > self.prev_end_line + 1;
                self.prev_end_line = cline + comment.text.matches('\n').count() as u32;
                items.push(TopItem {
                    blank_before,
                    kind: TopKind::Comment(comment),
                });
            }
            // Doc/attention run (docs/language.md (doc lines)): a `?`/`!`
            // line at item position starts a run that must bind to the
            // NEXT function declaration at this scope — anything else
            // next (`use`, `namespace`, the block terminator, Eof, a
            // top-level statement) is `DanglingDocRun` at the run's own
            // first line. `next_is_top_level_function_start` mirrors this
            // loop's own dispatch conditions exactly, so classifying
            // "true" here guarantees the fallthrough function-parsing
            // code below is what actually runs next.
            let doc_run = if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                let (run, first_span) = self.doc_run()?;
                if !self.next_is_top_level_function_start(terminator) {
                    return Err(CompileError {
                        span: first_span,
                        kind: CompileErrorKind::DanglingDocRun,
                    });
                }
                run
            } else {
                Vec::new()
            };
            let t = self.peek().clone();
            match (&t.kind, terminator) {
                (TokenKind::Eof, None) => return Ok((items, None, None)),
                (TokenKind::Eof, Some(_)) => {
                    return Err(Self::expected(&t, "`}` to close the namespace block"));
                }
                (k, Some(term)) if k == term => {
                    let close_line = t.line;
                    self.prev_end_line = close_line;
                    self.bump();
                    // c-brace fix, symmetric to the namespace's own
                    // `open_trailing` capture below `top_items`'s caller:
                    // a comment on the SAME line as `}` rides the closing
                    // brace instead of becoming the next sibling's
                    // leading own-line comment. The top-of-loop
                    // `drain_pending()` above already caught up
                    // `self.cpos` to the pre-`}` `self.pos`, so nothing
                    // is pending here except a comment genuinely
                    // following `}` (`sig_index == self.pos`, the
                    // position `}` just advanced to).
                    let mut close_trailing: Option<Comment> = None;
                    if self.cpos < self.comments.len() {
                        let ca = &self.comments[self.cpos];
                        if ca.sig_index == self.pos && ca.line == close_line {
                            self.prev_end_line =
                                close_line + ca.comment.text.matches('\n').count() as u32;
                            close_trailing = Some(ca.comment.clone());
                            self.cpos += 1;
                        }
                    }
                    return Ok((items, close_trailing, Some(t.span())));
                }
                _ => {}
            }
            // `namespace {` / `use {` / `export {`: the contextual keyword
            // has no name; without this check it parses as a function
            // named `namespace` and the error blames the `{`.
            if let TokenKind::Ident(w) = &t.kind
                && matches!(w.as_str(), "namespace" | "use" | "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::LBrace)
                )
            {
                let kw: &'static str = match w.as_str() {
                    "use" => "use",
                    "export" => "export",
                    _ => "namespace",
                };
                return Err(Self::err_at(&t, CompileErrorKind::KeywordNeedsName(kw)));
            }
            // A command or call at top level: `left;`, `goto 1;`, `@f();`.
            // Without this, reserved words blame naming rules and `@`
            // blames a missing function name.
            let top_level_stmt = match &t.kind {
                TokenKind::At => true,
                TokenKind::Ident(w) => RESERVED.contains(&w.as_str()),
                _ => false,
            };
            if top_level_stmt {
                return Err(Self::err_at(
                    &t,
                    CompileErrorKind::TopLevelStatement(describe(&t.kind)),
                ));
            }
            // Contextual keyword: `use` + identifier = import declaration;
            // `use` + `(` is a function NAMED use.
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "use")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
            {
                let use_line = t.line;
                self.bump();
                let mut paths: Vec<UsePath> = Vec::new();
                let semi_line;
                loop {
                    // path := IDENT (`::` IDENT)*  [ `as` IDENT ]
                    let t = self.peek().clone();
                    let TokenKind::Ident(name) = &t.kind else {
                        return Err(Self::expected(&t, "an imported function name"));
                    };
                    if RESERVED.contains(&name.as_str()) {
                        return Err(Self::expected(&t, "an imported function name"));
                    }
                    let mut path = vec![name.clone()];
                    let path_start = t.span().start;
                    let mut path_end = t.span().end;
                    let path_line = t.line;
                    self.bump();
                    while matches!(self.peek().kind, TokenKind::ColonColon) {
                        self.bump();
                        let t = self.peek().clone();
                        let TokenKind::Ident(seg) = &t.kind else {
                            return Err(Self::expected(&t, "a name after `::`"));
                        };
                        if RESERVED.contains(&seg.as_str()) {
                            return Err(Self::err_at(
                                &t,
                                CompileErrorKind::ReservedName {
                                    name: seg.clone(),
                                    what: "path segment",
                                },
                            ));
                        }
                        path.push(seg.clone());
                        path_end = t.span().end;
                        self.bump();
                    }
                    let alias = if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "as") {
                        self.bump();
                        let t = self.peek().clone();
                        let TokenKind::Ident(a) = &t.kind else {
                            return Err(Self::expected(&t, "an alias after `as`"));
                        };
                        self.bump();
                        Some(a.clone())
                    } else {
                        None
                    };
                    paths.push(UsePath {
                        path,
                        alias,
                        line: path_line,
                        span: Span {
                            start: path_start,
                            end: path_end,
                        },
                    });
                    let sep = self.peek().clone();
                    match sep.kind {
                        TokenKind::Comma => {
                            self.bump();
                        }
                        TokenKind::Semi => {
                            semi_line = sep.line;
                            self.bump();
                            break;
                        }
                        TokenKind::Colon => {
                            return Err(Self::err_at(&sep, CompileErrorKind::SingleColonInPath));
                        }
                        _ => return Err(Self::expected(&sep, "`,` or `;`")),
                    }
                }
                // The whole `use` list's trailing comment rides the node.
                let trailing = self.take_trailing(semi_line);
                let use_span = Span {
                    start: paths
                        .first()
                        .expect("a use list has at least one path")
                        .span
                        .start,
                    end: paths
                        .last()
                        .expect("a use list has at least one path")
                        .span
                        .end,
                };
                // One TopItem for the whole grouped list (fmt design doc §C
                // "Imports: grouping fix") — `blank_before` reads the `use`
                // keyword's own line, matching what the FIRST path would
                // have reported under the old per-path scheme.
                let blank_before = use_line > self.prev_end_line + 1;
                self.prev_end_line = paths.last().expect("a use list has at least one path").line;
                items.push(TopItem {
                    blank_before,
                    kind: TopKind::Import(UseCst {
                        paths,
                        line: use_line,
                        span: use_span,
                        trailing,
                    }),
                });
                continue;
            }
            // Contextual keyword: `namespace NAME {` opens a (reopenable)
            // block; `namespace` + `(` stays a function NAMED namespace.
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "namespace")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
                && matches!(
                    self.tokens.get(self.pos + 2).map(|t| &t.kind),
                    Some(TokenKind::LBrace)
                )
            {
                let ns_saved = self.prev_end_line;
                let ns_line = t.line;
                self.bump(); // `namespace`
                let name_tok = self.peek().clone();
                let TokenKind::Ident(name) = &name_tok.kind else {
                    unreachable!("checked above");
                };
                let name = name.clone();
                if RESERVED.contains(&name.as_str()) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::ReservedName {
                            name,
                            what: "namespace",
                        },
                    ));
                }
                // Shared name pool: a namespace may not reuse a sibling
                // function's name (reopening the same namespace is fine).
                if self.declared_fns.contains(&(ns.to_vec(), name.clone())) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::DuplicateName {
                            name,
                            what: "function",
                        },
                    ));
                }
                let name_span = name_tok.span();
                self.bump(); // the name
                let brace = self.peek().clone();
                self.bump(); // `{`
                let mut child = ns.to_vec();
                child.push(name.clone());
                self.namespaces.insert(child.clone());
                self.prev_end_line = brace.line;
                // c-brace fix (`cst.rs`'s "Comment placement" doc,
                // mirrors `function`'s `open_trailing` capture): comment(s)
                // riding the SAME physical line as the namespace's `{`,
                // before the first body item, are captured here instead
                // of falling into `top_items`'s ordinary leading-comment
                // drain (which would print them as their own body item,
                // moving them off the header line). `sig_index ==
                // self.pos` (not `<=`) excludes a comment that sits
                // BEFORE `{` even when it shares `{`'s physical line.
                let mut open_trailing: Vec<Comment> = Vec::new();
                while self.cpos < self.comments.len() {
                    let ca = &self.comments[self.cpos];
                    if ca.sig_index == self.pos && ca.line == brace.line {
                        open_trailing.push(ca.comment.clone());
                        self.cpos += 1;
                    } else {
                        break;
                    }
                }
                if let Some(last) = open_trailing.last() {
                    self.prev_end_line = brace.line + last.text.matches('\n').count() as u32;
                }
                let (child_items, close_trailing, close_span) =
                    self.top_items(&child, Some(&TokenKind::RBrace))?;
                // `top_items` set `prev_end_line` to the closing `}` line
                // (or its close_trailing comment's last line).
                let blank_before = ns_line > ns_saved + 1;
                items.push(TopItem {
                    blank_before,
                    kind: TopKind::Namespace(NamespaceCst {
                        name,
                        name_span,
                        line: ns_line,
                        span: Span {
                            start: t.span().start,
                            end: close_span
                                .expect(
                                    "top_items with Some(terminator) always returns a close span",
                                )
                                .end,
                        },
                        items: child_items,
                        open_trailing,
                        close_trailing,
                    }),
                });
                continue;
            }
            // Contextual keyword: `export` + identifier = exported def;
            // `export` + `(` is a function NAMED export.
            let fn_saved = self.prev_end_line;
            let fn_line = self.peek().line;
            let export_start = if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                ) {
                let export_tok = self.peek().clone();
                self.bump();
                Some(export_tok.span().start)
            } else {
                None
            };
            let exported = export_start.is_some();
            // Threaded through so `FunctionCst::span` starts at `export`
            // (the header's true first token) rather than the name — see
            // `cst.rs`'s `FunctionCst::span` doc. `doc_run` is empty
            // unless the loop above just collected and validated one.
            let mut f = self.function(export_start, doc_run)?;
            // The literal keyword presence (fmt design doc §D "Export
            // keyword verbatim") — unlike `exported` below, this does NOT
            // fold in `main`'s auto-export.
            f.has_export = exported;
            // Only the un-namespaced top-level `main` auto-exports (and is
            // the entry); a namespaced `main` is an ordinary function.
            f.exported = exported || (ns.is_empty() && f.name == "main");
            if self.declared_fns.contains(&(ns.to_vec(), f.name.clone())) {
                return Err(CompileError {
                    span: mtc_core::diagnostics::Span::point(f.line, f.col),
                    kind: CompileErrorKind::DuplicateName {
                        name: f.name.clone(),
                        what: "function",
                    },
                });
            }
            // Shared name pool: a function may not reuse a sibling
            // namespace's name.
            let mut as_ns = ns.to_vec();
            as_ns.push(f.name.clone());
            if self.namespaces.contains(&as_ns) {
                return Err(CompileError {
                    span: mtc_core::diagnostics::Span::point(f.line, f.col),
                    kind: CompileErrorKind::DuplicateName {
                        name: f.name.clone(),
                        what: "namespace",
                    },
                });
            }
            self.declared_fns.insert((ns.to_vec(), f.name.clone()));
            // `function` set `prev_end_line` to the closing `}` line.
            let blank_before = fn_line > fn_saved + 1;
            items.push(TopItem {
                blank_before,
                kind: TopKind::Function(f),
            });
        }
    }

    // `export_start`: the `export` keyword's span start when the caller
    // already consumed a leading `export` for this function (top-level
    // only — a nested definition passes `None`, `NestedExport` bars a
    // nested `export` before this is ever called). Threaded in rather
    // than re-detected here because `top_items` already consumed the
    // token; `FunctionCst::span` starts here when present, at the name
    // token otherwise (cst.rs's `FunctionCst::span` doc). `doc_run`: the
    // run the caller already collected and validated as bound to THIS
    // declaration (empty when undocumented) — this function only stores
    // it, it never collects one itself (the caller owns the "what comes
    // next" dispatch a run's dangling check depends on).
    fn function(
        &mut self,
        export_start: Option<Pos>,
        doc_run: Vec<DocRunItem>,
    ) -> Result<FunctionCst, CompileError> {
        let name_tok = self.peek().clone();
        let TokenKind::Ident(name) = &name_tok.kind else {
            return Err(Self::expected(&name_tok, "a function name"));
        };
        let name = name.clone();
        if RESERVED.contains(&name.as_str()) {
            return Err(Self::err_at(
                &name_tok,
                CompileErrorKind::ReservedName {
                    name,
                    what: "function",
                },
            ));
        }
        self.bump();
        self.expect(&TokenKind::LParen, "`(` after the function name")?;
        self.expect(&TokenKind::RParen, "`)` (functions take no parameters)")?;
        let brace = self.peek().clone();
        self.expect(&TokenKind::LBrace, "`{`")?;

        let mut body: Vec<BodyItem> = Vec::new();
        let mut nested_names: HashSet<String> = HashSet::new();
        let mut seen_labels: HashSet<u32> = HashSet::new();
        self.prev_end_line = brace.line;
        // c-brace fix (`cst.rs`'s "Comment placement" doc): comment(s)
        // riding the SAME physical line as `{`, before the first body
        // item, are captured here instead of falling into the ordinary
        // leading-comment drain below (which would print them as their
        // own body item, moving them off the header line). `sig_index ==
        // self.pos` (not `<=`) is deliberate — it excludes a comment that
        // sits BEFORE `{` (e.g. `f() /* x */ { ... }`, sig_index one
        // token earlier) even when that comment happens to share `{`'s
        // physical line; only a comment genuinely AFTER `{` has
        // `sig_index` equal to the position `{` just advanced `self.pos`
        // to.
        let mut open_trailing: Vec<Comment> = Vec::new();
        while self.cpos < self.comments.len() {
            let ca = &self.comments[self.cpos];
            if ca.sig_index == self.pos && ca.line == brace.line {
                open_trailing.push(ca.comment.clone());
                self.cpos += 1;
            } else {
                break;
            }
        }
        if let Some(last) = open_trailing.last() {
            self.prev_end_line = brace.line + last.text.matches('\n').count() as u32;
        }
        let mut close_trailing: Option<Comment> = None;
        // Assigned exactly once, in the `RBrace`-closing branch below,
        // right before the `break` that is this loop's only non-error
        // exit — the closing `}` token's own span, for
        // `FunctionCst::span`'s extent end.
        let close_span: Span;
        loop {
            // Own-line comments (leading/standalone/dangling) become body
            // items in source position.
            for (comment, cline) in self.drain_pending() {
                let blank_before = cline > self.prev_end_line + 1;
                self.prev_end_line = cline + comment.text.matches('\n').count() as u32;
                body.push(BodyItem {
                    blank_before,
                    kind: BodyKind::Comment(comment),
                });
            }
            if matches!(self.peek().kind, TokenKind::Eof) {
                return Err(Self::expected(
                    self.peek(),
                    "`}` to close the function body",
                ));
            }
            // Doc/attention run (docs/language.md (doc lines)): a `?`/`!`
            // line at body item position starts a run that must bind to
            // the NEXT nested function definition — anything else next
            // (a statement, the closing `}`, `export` before a nested
            // def) is `DanglingDocRun` at the run's own first line.
            let doc_run = if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                let (run, first_span) = self.doc_run()?;
                if !self.next_is_nested_function_start() {
                    return Err(CompileError {
                        span: first_span,
                        kind: CompileErrorKind::DanglingDocRun,
                    });
                }
                run
            } else {
                Vec::new()
            };
            // Nested definition: IDENT ( ) {  — visibility-only nesting.
            let is_nested_def = self.next_is_nested_function_start();
            if is_nested_def {
                let nested_saved = self.prev_end_line;
                let nested_line = self.peek().line;
                // Nested definitions can never carry a leading `export`
                // (`NestedExport` bars it above), so the extent always
                // starts at the name token.
                let child = self.function(None, doc_run)?;
                if nested_names.contains(&child.name) {
                    return Err(CompileError {
                        span: mtc_core::diagnostics::Span::point(child.line, child.col),
                        kind: CompileErrorKind::DuplicateName {
                            name: child.name.clone(),
                            what: "function",
                        },
                    });
                }
                nested_names.insert(child.name.clone());
                // `function` set `prev_end_line` to the nested `}` line.
                let blank_before = nested_line > nested_saved + 1;
                body.push(BodyItem {
                    blank_before,
                    kind: BodyKind::Nested(child),
                });
                continue;
            }
            // `export` before a nested definition is an error.
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
            {
                let t = self.peek().clone();
                return Err(Self::err_at(&t, CompileErrorKind::NestedExport));
            }
            // Labels announced before the next statement (possibly stacked).
            let stmt_saved = self.prev_end_line;
            let stmt_line = self.peek().line;
            let mut labels = Vec::new();
            let mut last_colon_line: u32 = 0;
            loop {
                let tok = self.peek().clone();
                let TokenKind::Number(n, written) = &tok.kind else {
                    break;
                };
                let (n, written) = (*n, written.clone());
                self.bump();
                let colon = self.peek().clone();
                self.expect(&TokenKind::Colon, "`:` after a label number")?;
                if !seen_labels.insert(n) {
                    return Err(Self::err_at(&tok, CompileErrorKind::DuplicateLabel(n)));
                }
                last_colon_line = colon.line;
                labels.push(Label {
                    value: n,
                    span: Span {
                        start: tok.span().start,
                        end: colon.span().end,
                    },
                    written,
                });
            }
            if matches!(self.peek().kind, TokenKind::RBrace) {
                if let Some(label) = labels.first() {
                    let t = self.peek().clone();
                    return Err(Self::err_at(
                        &t,
                        CompileErrorKind::DanglingLabel(label.value),
                    ));
                }
                let close_tok = self.peek().clone();
                let close_line = close_tok.line;
                close_span = close_tok.span();
                self.prev_end_line = close_line;
                self.bump();
                // c-brace fix, symmetric to `open_trailing` above: a
                // comment on the SAME line as `}` rides the closing
                // brace instead of becoming the next sibling's leading
                // own-line comment. The top-of-loop `drain_pending()`
                // above already caught up `self.cpos` to the pre-`}`
                // `self.pos`, so nothing is pending here except a
                // comment genuinely following `}` (`sig_index ==
                // self.pos`, the position `}` just advanced to).
                if self.cpos < self.comments.len() {
                    let ca = &self.comments[self.cpos];
                    if ca.sig_index == self.pos && ca.line == close_line {
                        self.prev_end_line =
                            close_line + ca.comment.text.matches('\n').count() as u32;
                        close_trailing = Some(ca.comment.clone());
                        self.cpos += 1;
                    }
                }
                break;
            }
            let stmt = self.statement(labels, last_colon_line)?;
            // `statement` set `prev_end_line` to the `;` line.
            let blank_before = stmt_line > stmt_saved + 1;
            body.push(BodyItem {
                blank_before,
                kind: BodyKind::Statement(stmt),
            });
        }
        Ok(FunctionCst {
            name,
            name_span: name_tok.span(),
            line: name_tok.line,
            col: name_tok.col,
            span: Span {
                start: export_start.unwrap_or_else(|| name_tok.span().start),
                end: close_span.end,
            },
            exported: false,
            has_export: false,
            body,
            open_trailing,
            close_trailing,
            doc_run,
        })
    }

    fn statement(
        &mut self,
        labels: Vec<Label>,
        last_colon_line: u32,
    ) -> Result<StatementCst, CompileError> {
        let start = labels
            .first()
            .map(|l| l.span.start)
            .unwrap_or_else(|| self.peek().span().start);
        let line = self.peek().line;
        // The author put a newline after the final label `:` (own-line
        // label) iff the first command sits on a later line.
        let label_break = !labels.is_empty() && line > last_colon_line;
        // A comment between the label and the first command (rare) rides
        // the first item's leading; the common case leaves it empty.
        let leading = self.drain_pending_comments();
        let mut items = vec![CommaItem {
            item: self.item(false)?,
            leading,
            // The first entry's `newline_before` is always false (fmt
            // design doc, "Comma-group layout").
            newline_before: false,
        }];
        // `pos` has advanced past the item just parsed; `pos - 1` is its
        // last significant token, whose line is the "item K-1's last
        // token" side of the next entry's newline comparison.
        let mut last_item_end_line = self.tokens[self.pos - 1].line;
        while matches!(self.peek().kind, TokenKind::Comma) {
            let comma = self.peek().clone();
            // Whatever precedes a `,` must be bare (docs/language.md).
            match &items.last().expect("items is never empty").item {
                Item::Check { .. } => {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition(
                            "check must be the last command in a comma group",
                        ),
                    ));
                }
                Item::Halt { .. } => {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition(
                            "halt must be the last command in a comma group",
                        ),
                    ));
                }
                Item::Goto { .. } => {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition("goto cannot appear in a comma group"),
                    ));
                }
                Item::Builtin { succ, .. } | Item::Call { succ, .. }
                    if *succ != Successor::FallThrough =>
                {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition(
                            "only the last command in a comma group may take a successor",
                        ),
                    ));
                }
                _ => {}
            }
            self.bump();
            // A mid-group comment attaches to the following item's leading.
            let leading = self.drain_pending_comments();
            // The comments are a side channel (split off before the
            // significant-token walk, see `parse_cst`), so `self.peek()`
            // here already sits on this item's real first token, whatever
            // comments were just drained.
            let item_start_line = self.peek().line;
            let newline_before = item_start_line > last_item_end_line;
            items.push(CommaItem {
                item: self.item(true)?,
                leading,
                newline_before,
            });
            last_item_end_line = self.tokens[self.pos - 1].line;
        }
        let semi = self.peek().clone();
        self.expect(&TokenKind::Semi, "`;`")?;
        let trailing = self.take_trailing(semi.line);
        self.prev_end_line = semi.line;
        Ok(StatementCst {
            labels,
            items,
            line,
            span: Span {
                start,
                end: semi.span().end,
            },
            label_break,
            trailing,
        })
    }

    fn item(&mut self, in_group: bool) -> Result<Item, CompileError> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::At => {
                self.bump();
                let name_tok = self.peek().clone();
                let TokenKind::Ident(name) = &name_tok.kind else {
                    return Err(Self::expected(&name_tok, "a function name after `@`"));
                };
                let mut name = name.clone();
                if RESERVED.contains(&name.as_str()) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::BuiltinCalled(name),
                    ));
                }
                let mut name_end = name_tok.span().end;
                self.bump();
                // Qualified call: `@ns::path::f()` — ABSOLUTE (flatten
                // skips the scope chain), `::` segments only (nested
                // functions stay unnameable — the grammar has no `.`).
                while matches!(self.peek().kind, TokenKind::ColonColon) {
                    self.bump();
                    let t = self.peek().clone();
                    let TokenKind::Ident(seg) = &t.kind else {
                        return Err(Self::expected(&t, "a name after `::`"));
                    };
                    if RESERVED.contains(&seg.as_str()) {
                        return Err(Self::err_at(
                            &t,
                            CompileErrorKind::ReservedName {
                                name: seg.clone(),
                                what: "path segment",
                            },
                        ));
                    }
                    name.push_str("::");
                    name.push_str(seg);
                    name_end = t.span().end;
                    self.bump();
                }
                if matches!(self.peek().kind, TokenKind::Colon) {
                    let t = self.peek().clone();
                    return Err(Self::err_at(&t, CompileErrorKind::SingleColonInPath));
                }
                let lparen = self.peek().clone();
                self.expect(&TokenKind::LParen, "`(` (user calls are written `@name()`)")?;
                let (succ, succ_label_span, succ_label_written) = self.successor()?;
                let rparen = self.peek().clone();
                self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Item::Call {
                    name,
                    name_span: Span {
                        start: name_tok.span().start,
                        end: name_end,
                    },
                    succ,
                    succ_span: Some(Span {
                        start: lparen.span().start,
                        end: rparen.span().end,
                    }),
                    succ_label_span,
                    succ_label_written,
                    line: tok.line,
                })
            }
            TokenKind::Ident(word) => match word.as_str() {
                "goto" => {
                    if in_group {
                        return Err(Self::err_at(
                            &tok,
                            CompileErrorKind::GroupPosition("goto cannot appear in a comma group"),
                        ));
                    }
                    self.bump();
                    let target = self.peek().clone();
                    let target_span = target.span();
                    match target.kind {
                        TokenKind::Number(n, written) => {
                            self.bump();
                            Ok(Item::Goto {
                                label: n,
                                label_span: target_span,
                                label_written: written,
                                line: tok.line,
                            })
                        }
                        TokenKind::Bang => Err(Self::err_at(&target, CompileErrorKind::GotoReturn)),
                        _ => Err(Self::expected(&target, "a numeric label after `goto`")),
                    }
                }
                "check" => {
                    self.bump();
                    self.expect(&TokenKind::LParen, "`(` after `check`")?;
                    let (marked, marked_span, marked_written) = self.check_arm()?;
                    self.expect(&TokenKind::Comma, "`,` between check arms")?;
                    let (blank, blank_span, blank_written) = self.check_arm()?;
                    let rparen = self.peek().clone();
                    self.expect(&TokenKind::RParen, "`)`")?;
                    Ok(Item::Check {
                        marked,
                        blank,
                        span: Span {
                            start: tok.span().start,
                            end: rparen.span().end,
                        },
                        marked_span,
                        blank_span,
                        marked_written,
                        blank_written,
                        line: tok.line,
                    })
                }
                "halt" => {
                    self.bump();
                    Ok(Item::Halt { line: tok.line })
                }
                "debugger" => {
                    self.bump();
                    Ok(Item::Debugger { line: tok.line })
                }
                "left" | "right" | "mark" | "unmark" => {
                    let which = match word.as_str() {
                        "left" => Builtin::Left,
                        "right" => Builtin::Right,
                        "mark" => Builtin::Mark,
                        _ => Builtin::Unmark,
                    };
                    self.bump();
                    let (succ, succ_span, succ_label_span, succ_label_written) =
                        if matches!(self.peek().kind, TokenKind::LParen) {
                            let lparen = self.peek().clone();
                            self.bump();
                            // docs/language.md: parens on a builtin, if
                            // present, must carry a successor — empty `()` is
                            // no longer fall-through sugar. Builtins-only:
                            // `successor()` (shared with calls) is untouched,
                            // so `@f()` stays legal.
                            if matches!(self.peek().kind, TokenKind::RParen) {
                                let rparen = self.peek().clone();
                                return Err(CompileError {
                                    span: Span {
                                        start: lparen.span().start,
                                        end: rparen.span().end,
                                    },
                                    kind: CompileErrorKind::EmptyBuiltinParens {
                                        name: word.clone(),
                                    },
                                });
                            }
                            let (succ, succ_label_span, succ_label_written) = self.successor()?;
                            let rparen = self.peek().clone();
                            self.expect(&TokenKind::RParen, "`)`")?;
                            (
                                succ,
                                Some(Span {
                                    start: lparen.span().start,
                                    end: rparen.span().end,
                                }),
                                succ_label_span,
                                succ_label_written,
                            )
                        } else {
                            (Successor::FallThrough, None, None, None)
                        };
                    Ok(Item::Builtin {
                        which,
                        succ,
                        succ_span,
                        succ_label_span,
                        succ_label_written,
                        line: tok.line,
                    })
                }
                "use" => Err(Self::err_at(&tok, CompileErrorKind::KeywordInBody("use"))),
                "namespace" => Err(Self::err_at(
                    &tok,
                    CompileErrorKind::KeywordInBody("namespace"),
                )),
                other => Err(Self::err_at(
                    &tok,
                    CompileErrorKind::UnknownCommand(other.to_string()),
                )),
            },
            _ => Err(Self::expected(&tok, "a command")),
        }
    }

    /// Inside `( … )`: empty → fall through, `N` → label, `!` → return.
    /// The second element of the result is the number token's own span,
    /// the third is its WRITTEN text — both `Some` iff the successor is
    /// `Successor::Label`.
    fn successor(&mut self) -> Result<(Successor, Option<Span>, Option<String>), CompileError> {
        let t = self.peek().clone();
        let t_span = t.span();
        match t.kind {
            TokenKind::Number(n, written) => {
                self.bump();
                Ok((Successor::Label(n), Some(t_span), Some(written)))
            }
            TokenKind::Bang => {
                self.bump();
                Ok((Successor::Return, None, None))
            }
            _ => Ok((Successor::FallThrough, None, None)), // the caller checks the `)`
        }
    }

    /// The second element of the result is the arm's own token span (the
    /// number or the `!`), regardless of which arm shape it is; the
    /// third is the number's WRITTEN text, `Some` iff the arm is
    /// `CheckArm::Label`.
    fn check_arm(&mut self) -> Result<(CheckArm, Span, Option<String>), CompileError> {
        let t = self.peek().clone();
        let t_span = t.span();
        match t.kind {
            TokenKind::Number(n, written) => {
                self.bump();
                Ok((CheckArm::Label(n), t_span, Some(written)))
            }
            TokenKind::Bang => {
                self.bump();
                Ok((CheckArm::Return, t.span(), None))
            }
            _ => Err(Self::expected(&t, "a label number or `!`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CompileErrorKind;
    use crate::lexer::lex;

    fn parse_src(src: &str) -> Result<Program, CompileError> {
        parse(&lex(src).unwrap())
    }

    /// `parse_cst` on a `WithComments` stream must retain every comment as
    /// trivia and record the layout signals (`blank_before`,
    /// `label_break`, per-item `leading`, `trailing`) that `lower_cst`
    /// drops. Reads each of those fields, and confirms no comment is lost.
    #[test]
    fn parse_cst_captures_comment_trivia_and_layout() {
        use crate::cst::{BodyKind, TopKind};
        use crate::lexer::{LexMode, lex_with};

        let src = "\
// top comment
use std::goToEnd; // import trailing

f() {
    1:
        left; // trailing
    right, /* mid */ mark;
}
";
        let tokens = lex_with(src, LexMode::WithComments).unwrap();
        let cst = parse_cst(&tokens).unwrap();

        // An own-line comment is lifted to its own top-level Comment item.
        let TopKind::Comment(c0) = &cst.items[0].kind else {
            panic!(
                "expected a leading comment item, got {:?}",
                cst.items[0].kind
            );
        };
        assert_eq!(c0.text, "// top comment");
        assert!(c0.own_line);

        // The import keeps its same-line trailing comment.
        let TopKind::Import(use_cst) = &cst.items[1].kind else {
            panic!("expected an import item");
        };
        assert_eq!(use_cst.paths.len(), 1);
        assert_eq!(use_cst.paths[0].path, vec!["std", "goToEnd"]);
        assert_eq!(
            use_cst.trailing.as_ref().map(|tc| tc.comment.text.as_str()),
            Some("// import trailing")
        );

        // A blank line precedes the function in source.
        assert!(cst.items[2].blank_before, "blank line precedes f()");
        let TopKind::Function(f) = &cst.items[2].kind else {
            panic!("expected a function item");
        };

        // Own-line label => `label_break`; same-line `;` trailing comment.
        let BodyKind::Statement(s0) = &f.body[0].kind else {
            panic!("expected the first body statement");
        };
        assert!(s0.label_break, "the label sits on its own line");
        assert_eq!(
            s0.trailing.as_ref().map(|tc| tc.comment.text.as_str()),
            Some("// trailing")
        );
        assert_eq!(s0.items.len(), 1);
        assert!(s0.items[0].leading.is_empty());

        // A mid-group comment rides the FOLLOWING comma item's `leading`.
        // Both items sit on the same source line, so `newline_before` is
        // false for both (the comment alone doesn't count as a break).
        let BodyKind::Statement(s1) = &f.body[1].kind else {
            panic!("expected the second body statement");
        };
        assert_eq!(s1.items.len(), 2);
        assert!(s1.items[0].leading.is_empty());
        assert!(!s1.items[0].newline_before);
        assert!(!s1.items[1].newline_before);
        assert_eq!(
            s1.items[1]
                .leading
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            vec!["/* mid */"]
        );

        // Nothing dropped: every comment token is placed somewhere.
        let comment_count = tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Comment(_)))
            .count();
        assert_eq!(comment_count, 4);
    }

    /// `CommaItem::newline_before` (fmt design doc, "Comma-group
    /// layout"): the first entry is always `false`; a later entry is
    /// `true` iff the author put a newline before it, compared by token
    /// line — not by whether a comment happens to sit between the items.
    #[test]
    fn parse_cst_records_comma_group_newline_before() {
        use crate::cst::BodyKind;
        use crate::lexer::{LexMode, lex_with};

        let src = "f() {\n1: left, right,\nmark, unmark;\n}\n";
        let tokens = lex_with(src, LexMode::WithComments).unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(f) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        let BodyKind::Statement(s) = &f.body[0].kind else {
            panic!("expected the body statement");
        };
        assert_eq!(s.items.len(), 4);
        // `left` (first item), never a break by contract.
        assert!(!s.items[0].newline_before);
        // `right` shares `left`'s source line.
        assert!(!s.items[1].newline_before);
        // `mark` sits on a new source line — the author's break.
        assert!(s.items[2].newline_before);
        // `unmark` shares `mark`'s source line.
        assert!(!s.items[3].newline_before);
    }

    #[test]
    fn parses_the_spec_sample() {
        let src = r#"
// Move right until the first blank cell.
goToEnd() {
1:  right;
    check(1, 2);      // cell marked -> goto 1, blank -> goto 2
2:  left;             // last command - implicit return
}

goToBegin() {
1:  left(2);
2:  check(1, 3);
3:  right(!);
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
"#;
        let p = parse_src(src).unwrap();
        assert_eq!(
            p.functions
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["goToEnd", "goToBegin", "main"]
        );
        let main = &p.functions[2];
        assert_eq!(main.body.len(), 5);
        assert_eq!(main.body[0].items.len(), 1);
        match &main.body[0].items[0] {
            Item::Call {
                name,
                succ: Successor::FallThrough,
                line,
                ..
            } => {
                assert_eq!(name, "goToEnd");
                assert_eq!(*line, main.body[0].line);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            main.body[3]
                .labels
                .iter()
                .map(|l| l.value)
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(main.body[3].items.len(), 1);
        match &main.body[3].items[0] {
            Item::Builtin {
                which: Builtin::Unmark,
                succ: Successor::Return,
                line,
                ..
            } => {
                assert_eq!(*line, main.body[3].line);
            }
            other => panic!("unexpected {other:?}"),
        }
        match &main.body[2].items[0] {
            Item::Check {
                marked: CheckArm::Label(3),
                blank: CheckArm::Label(4),
                ..
            } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn comma_groups_parse_and_enforce_positions() {
        let p = parse_src("f() { 1: right, right, mark(5); 5: left, check(1, !); }").unwrap();
        assert_eq!(p.functions[0].body[0].items.len(), 3);
        assert_eq!(p.functions[0].body[1].items.len(), 2);

        let e = parse_src("f() { left(1), left(2); 1: mark; 2: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("successor")));

        let e = parse_src("f() { check(1, 2), left; 1: mark; 2: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("check")));

        let e = parse_src("f() { halt, left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("halt")));

        let e = parse_src("f() { goto 1, left; 1: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("goto")));
        let e = parse_src("f() { left, goto 1; 1: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("goto")));
    }

    #[test]
    fn reserved_and_at_rules() {
        // At top level a reserved-word ident is now a `TopLevelStatement`
        // (docs/language.md) — the naming check runs only once a keyword
        // has consumed the leading token (e.g. `export <reserved>()`).
        let e = parse_src("check() { }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::TopLevelStatement(ref n) if n.contains("check"))
        );
        // `export` isn't reserved, so it slips past the top-level guard;
        // `function()` itself then sees the reserved name.
        let e = parse_src("export check() { }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::ReservedName { ref name, what } if name == "check" && what == "function")
        );

        let e = parse_src("f() { @left(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::BuiltinCalled(n) if n == "left"));

        let e = parse_src("f() { flip; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownCommand(n) if n == "flip"));

        // A user function called without `@` is the same error (docs/language.md).
        let e = parse_src("f() { goToEnd(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownCommand(n) if n == "goToEnd"));
    }

    #[test]
    fn empty_builtin_parens_are_a_syntax_error() {
        // docs/language.md: `()` on a tape builtin, if written, must carry
        // a successor — empty parens are no longer fall-through sugar.
        for name in ["left", "right", "mark", "unmark"] {
            let e = parse_src(&format!("f() {{ {name}(); }}")).unwrap_err();
            assert!(
                matches!(e.kind, CompileErrorKind::EmptyBuiltinParens { name: ref n } if n == name),
                "{name}(): got {:?}",
                e.kind
            );
        }

        // Bare, and both successor forms, stay legal.
        assert!(parse_src("f() { left; }").is_ok());
        assert!(parse_src("f() { left(5); }").is_ok());
        assert!(parse_src("f() { left(!); }").is_ok());

        // Scope limit: user calls keep mandatory-but-emptyable parens.
        assert!(parse_src("f() { @f(); }").is_ok());
    }

    #[test]
    fn goto_bang_is_a_dedicated_error() {
        let e = parse_src("f() { goto !; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GotoReturn));
    }

    #[test]
    fn duplicate_and_dangling_diagnostics() {
        let e = parse_src("f() { } f() { }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "f" && what == "function")
        );

        let e = parse_src("f() { 1: left; 1: right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateLabel(1)));

        let e = parse_src("f() { left; 2: }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DanglingLabel(2)));
    }

    #[test]
    fn empty_function_and_stacked_labels() {
        let p = parse_src("f() { }").unwrap();
        assert!(p.functions[0].body.is_empty());

        let p = parse_src("f() { 1: 2: left; }").unwrap();
        assert_eq!(
            p.functions[0].body[0]
                .labels
                .iter()
                .map(|l| l.value)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn unicode_function_names_and_calls() {
        let p = parse_src("идиВКонец() { right(!); } main() { @идиВКонец(); }").unwrap();
        assert_eq!(p.functions[0].name, "идиВКонец");
        match &p.functions[1].body[0].items[0] {
            Item::Call { name, .. } => assert_eq!(name, "идиВКонец"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn export_is_contextual_and_main_auto_exports() {
        let p = parse_src("export api() { left; } helper() { right; } main() { mark; }").unwrap();
        assert!(p.functions[0].exported);
        assert!(!p.functions[1].exported);
        assert!(p.functions[2].exported); // main
        let p = parse_src("export() { left; } main() { @export(); }").unwrap();
        assert_eq!(p.functions[0].name, "export"); // a function NAMED export
    }

    #[test]
    fn nested_definitions_parse_recursively() {
        let p = parse_src("main() { walk() { step() { right; } @step(); } @walk(); }").unwrap();
        let main = &p.functions[0];
        assert_eq!(main.nested.len(), 1);
        assert_eq!(main.nested[0].name, "walk");
        assert_eq!(main.nested[0].nested[0].name, "step");
    }

    #[test]
    fn namespace_blocks_stamp_paths_and_nest() {
        let p =
            parse_src("namespace a { f() { left; } namespace b { g() { right; } } } h() { mark; }")
                .unwrap();
        let tagged: Vec<(&str, Vec<&str>)> = p
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f.ns.iter().map(String::as_str).collect()))
            .collect();
        assert_eq!(
            tagged,
            vec![("f", vec!["a"]), ("g", vec!["a", "b"]), ("h", vec![])]
        );
        // `namespace` + `(` stays a function NAMED namespace.
        let p = parse_src("namespace() { left; } main() { @namespace(); }").unwrap();
        assert_eq!(p.functions[0].name, "namespace");
    }

    #[test]
    fn import_paths_aliases_and_scopes_parse() {
        let p = parse_src("use a, std::b as c; namespace ns { use d::e; }").unwrap();
        assert_eq!(p.imports.len(), 3);
        assert_eq!(p.imports[0].path, vec!["a"]);
        assert_eq!(p.imports[0].alias, None);
        assert_eq!(p.imports[0].binding(), "a");
        assert!(p.imports[0].ns.is_empty());
        assert_eq!(p.imports[1].path, vec!["std", "b"]);
        assert_eq!(p.imports[1].alias.as_deref(), Some("c"));
        assert_eq!(p.imports[1].binding(), "c");
        assert_eq!(p.imports[1].full_path(), "std::b");
        assert_eq!(p.imports[2].path, vec!["d", "e"]);
        assert_eq!(p.imports[2].ns, vec!["ns"]);
    }

    #[test]
    fn qualified_calls_parse_to_joined_names() {
        let p = parse_src("main() { @std::api::run(); }").unwrap();
        match &p.functions[0].body[0].items[0] {
            Item::Call { name, .. } => assert_eq!(name, "std::api::run"),
            other => panic!("unexpected {other:?}"),
        }
        let e = parse_src("main() { @std::(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Expected { what, .. } if what.contains("::")));
    }

    #[test]
    fn namespace_name_pool_and_reopening_rules() {
        // Reopening the same namespace is legal (scopes merge by path).
        assert!(parse_src("namespace a { f() { left; } } namespace a { g() { right; } }").is_ok());
        // Same (path, name) across reopened blocks is a duplicate.
        let e =
            parse_src("namespace a { f() { left; } } namespace a { f() { right; } }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "f" && what == "function")
        );
        // The same bare name in different namespaces is legal.
        assert!(parse_src("namespace a { f() { left; } } namespace b { f() { right; } }").is_ok());
        // Namespace and function names share one pool per scope.
        let e = parse_src("namespace a { } a() { left; }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "a" && what == "namespace")
        );
        let e = parse_src("a() { left; } namespace a { }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "a" && what == "function")
        );
        // An unclosed block is an error, not silent Eof acceptance.
        let e = parse_src("namespace a { f() { left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Expected { .. }));
    }

    #[test]
    fn use_stays_illegal_inside_function_bodies() {
        let e = parse_src("main() { use go; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::KeywordInBody(kw) if kw == "use"));
    }

    #[test]
    fn nested_export_and_same_scope_duplicates_error() {
        let e = parse_src("main() { export inner() { left; } }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::NestedExport));
        let e = parse_src("main() { f() { left; } f() { right; } }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "f" && what == "function")
        );
    }

    #[test]
    fn spans_are_retained_for_labels_names_and_items() {
        let p = parse_src("f() {\n  5 : right(7);\n7:  left;\n}").unwrap();
        let f = &p.functions[0];
        assert_eq!(
            (f.name_span.start.col, f.name_span.end.col),
            (1, 2) // "f" at 1:1, end-exclusive
        );
        let s0 = &f.body[0];
        let label = &s0.labels[0];
        assert_eq!(label.value, 5);
        // "5 : …": number at col 3, colon at col 5 → span 3..6 (spans the gap)
        assert_eq!((label.span.start.col, label.span.end.col), (3, 6));
        // statement span: from the label through the `;`
        assert_eq!(s0.span.start.col, 3);
        assert_eq!(s0.span.end.col, 16); // after `;` of "right(7);"
        let Item::Builtin { succ_span, .. } = &s0.items[0] else {
            panic!("expected builtin");
        };
        let ss = succ_span.expect("right(7) has parens");
        assert_eq!((ss.start.col, ss.end.col), (12, 15)); // "(7)"
    }

    #[test]
    fn call_and_check_spans() {
        let p = parse_src("f() { @a::b(); check(1, !); 1: left; }").unwrap();
        let f = &p.functions[0];
        let Item::Call {
            name,
            name_span,
            succ_span,
            ..
        } = &f.body[0].items[0]
        else {
            panic!("expected call");
        };
        assert_eq!(name, "a::b");
        assert_eq!((name_span.start.col, name_span.end.col), (8, 12)); // "a::b"
        assert!(succ_span.is_some()); // "()" always parenthesised
        let Item::Check { span, .. } = &f.body[1].items[0] else {
            panic!("expected check");
        };
        assert_eq!((span.start.col, span.end.col), (16, 27)); // "check(1, !)"
    }

    /// docs/superpowers/plans/2026-07-10-lsp-plan2-pmc-service.md (Task
    /// 2): character-precise reference spans, exact `Span::new(...)`
    /// values against the fixture's actual layout —
    /// `f() { 1: right(2); check(1, !); goto 1; left, mark(3); }`.
    #[test]
    fn reference_spans_on_goto_check_and_builtin_successors() {
        let p = parse_src("f() { 1: right(2); check(1, !); goto 1; left, mark(3); }").unwrap();
        let f = &p.functions[0];

        // `1: right(2);` — the successor's number token alone, inside the
        // parens.
        let Item::Builtin {
            succ_label_span, ..
        } = &f.body[0].items[0]
        else {
            panic!("expected builtin");
        };
        assert_eq!(
            succ_label_span.expect("right(2) has a label successor"),
            Span::new(1, 16, 1, 17)
        );

        // `check(1, !);` — each arm's own token.
        let Item::Check {
            marked_span,
            blank_span,
            ..
        } = &f.body[1].items[0]
        else {
            panic!("expected check");
        };
        assert_eq!(*marked_span, Span::new(1, 26, 1, 27));
        assert_eq!(*blank_span, Span::new(1, 29, 1, 30));

        // `goto 1;` — the target number token.
        let Item::Goto { label_span, .. } = &f.body[2].items[0] else {
            panic!("expected goto");
        };
        assert_eq!(*label_span, Span::new(1, 38, 1, 39));

        // `left, mark(3);` — the bare (no-successor) first item has no
        // label span at all.
        let Item::Builtin {
            which: Builtin::Left,
            succ_label_span,
            ..
        } = &f.body[3].items[0]
        else {
            panic!("expected bare left");
        };
        assert!(
            succ_label_span.is_none(),
            "a bare (successor-less) builtin has no succ_label_span"
        );
    }

    #[test]
    fn succ_label_span_is_none_without_a_label_successor() {
        // Bare, no parens at all.
        let p = parse_src("f() { right; }").unwrap();
        let Item::Builtin {
            succ_label_span, ..
        } = &p.functions[0].body[0].items[0]
        else {
            panic!("expected builtin");
        };
        assert!(succ_label_span.is_none());

        // Parenthesised but a `!` (return) successor, not a label.
        let p = parse_src("f() { right(!); }").unwrap();
        let Item::Builtin {
            succ_label_span, ..
        } = &p.functions[0].body[0].items[0]
        else {
            panic!("expected builtin");
        };
        assert!(succ_label_span.is_none());
    }

    #[test]
    fn call_succ_label_span_covers_the_number() {
        let p = parse_src("main() { @g(7); }").unwrap();
        let Item::Call {
            succ_label_span, ..
        } = &p.functions[0].body[0].items[0]
        else {
            panic!("expected call");
        };
        assert_eq!(
            succ_label_span.expect("@g(7) has a label successor"),
            Span::new(1, 13, 1, 14)
        );
    }

    #[test]
    fn function_and_namespace_extent_spans() {
        use crate::cst::TopKind;

        // Two-line function: name token start → closing `}` end.
        let tokens = lex("f() {\n    left;\n}\n").unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(f) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        assert_eq!(f.span, Span::new(1, 1, 3, 2));

        // Namespace block: `namespace` keyword start → closing `}` end.
        let tokens = lex("namespace ns {\n    f() { left; }\n}\n").unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Namespace(ns) = &cst.items[0].kind else {
            panic!("expected a namespace item");
        };
        assert_eq!(ns.span, Span::new(1, 1, 3, 2));

        // A leading `export` is consumed by `top_items` before
        // `function()` ever sees it, but its span start is threaded
        // through so `FunctionCst::span` still starts at `export` — the
        // header's true first token (`f.name_span`/`.line`/`.col` stay
        // name-token-anchored; only the extent `span` reaches back).
        // Pinned explicitly since it's the one place the doc comment's
        // "header first token" reading is load-bearing.
        let tokens = lex("export f() {\n    left;\n}\n").unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(f) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        assert_eq!(f.span, Span::new(1, 1, 3, 2)); // starts at "export", not "f"
        // The bare (non-exported) case above ("Two-line function") already
        // pins the name-token-anchored start for the un-exported path —
        // together the two assertions in this test cover both cases.
    }

    #[test]
    fn import_spans_exclude_the_alias() {
        let p = parse_src("use std::go as g;\nmain() { @g(); }").unwrap();
        let imp = &p.imports[0];
        assert_eq!((imp.span.start.col, imp.span.end.col), (5, 12)); // "std::go"
    }

    fn err_msg(src: &str) -> String {
        parse_src(src).unwrap_err().to_string()
    }

    #[test]
    fn reserved_words_are_barred_in_every_path_segment() {
        let m = err_msg("main() { @std::goto(); }");
        assert!(m.contains("reserved word"), "got: {m}");
        let m = err_msg("use std::goto;\nmain() { right; }");
        assert!(m.contains("reserved word"), "got: {m}");
    }

    #[test]
    fn keyword_followed_by_brace_gets_a_hint() {
        let m = err_msg("namespace {\n}");
        assert!(
            m.contains("did you mean `namespace <name> { … }`"),
            "got: {m}"
        );
        let m = err_msg("use {}");
        assert!(m.contains("did you mean `use <name>;`"), "got: {m}");
        let m = err_msg("export {}");
        assert!(
            m.contains("did you mean `export <name>() { … }`"),
            "got: {m}"
        );
    }

    #[test]
    fn use_and_namespace_inside_a_body_say_the_real_rule() {
        let m = err_msg("main() { use go; }");
        assert!(m.contains("not allowed inside a function body"), "got: {m}");
        let m = err_msg("main() { namespace x; }");
        assert!(m.contains("not allowed inside a function body"), "got: {m}");
    }

    #[test]
    fn single_colon_in_a_path_hints_double_colon() {
        let m = err_msg("use std:b;\nmain() { right; }");
        assert!(m.contains("did you mean `::`"), "got: {m}");
        let m = err_msg("main() { @f:g(); }");
        assert!(m.contains("did you mean `::`"), "got: {m}");
    }

    #[test]
    fn namespace_naming_errors_say_namespace() {
        let m = err_msg("namespace goto { }");
        assert!(m.contains("namespace"), "got: {m}");
        let m = err_msg("namespace a { } a() { right; }");
        assert!(m.contains("namespace"), "got: {m}");
    }

    #[test]
    fn unclosed_function_body_mentions_the_brace() {
        let m = err_msg("f() { left;");
        assert!(m.contains("`}` to close the function body"), "got: {m}");
    }

    #[test]
    fn top_level_statements_state_the_rule() {
        for src in ["left;\nmain() { right; }", "goto 1;", "@foo();"] {
            let m = err_msg(src);
            assert!(m.contains("not allowed at top level"), "{src} got: {m}");
        }
    }

    #[test]
    fn spaced_label_colons_and_paths_stay_legal() {
        assert!(parse_src("main() { 1 : right; }").is_ok());
        assert!(parse_src("main() { 1: 2: right; }").is_ok());
        assert!(parse_src("use std :: goToEnd;\nmain() { @goToEnd(); }").is_ok());
    }

    #[test]
    fn empty_builtin_parens_message_names_the_builtin_and_the_fix() {
        let m = err_msg("main() { mark(); }");
        assert!(m.contains("`mark`"), "got: {m}");
        assert!(m.contains("successor"), "got: {m}");
        // Calls are unaffected: `@f()` stays legal, no error at all.
        assert!(parse_src("f() { } main() { @f(); }").is_ok());
    }

    // -- Doc/attention runs (docs/language.md (doc lines)) ----------------
    //
    // Grammar-fixed run order (`?` block, then `!` block), attachment to
    // the next `FunctionCst` at the run's own scope, and the two
    // attention-line attribute checks. `lower_cst` still ignores
    // `doc_run` entirely in this task (Task 3's job) — these tests read
    // `parse_cst`'s `Cst` directly.

    #[test]
    fn doc_run_collects_a_docs_only_run() {
        let tokens = lex("? line one\n? line two\nmain() { right; }").unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(f) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        assert!(!cst.items[0].blank_before);
        assert_eq!(f.doc_run.len(), 2);
        let DocRunKind::Doc { text, .. } = &f.doc_run[0].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, "line one");
        assert!(!f.doc_run[0].blank_before);
        let DocRunKind::Doc { text, .. } = &f.doc_run[1].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, "line two");
        assert!(!f.doc_run[1].blank_before);
    }

    #[test]
    fn doc_run_collects_an_attention_only_run() {
        let tokens =
            lex("! bare prose line\n! [deprecated] use goToStart instead\nmain() { right; }")
                .unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(f) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        assert_eq!(f.doc_run.len(), 2);
        let DocRunKind::Attention { attr, text, .. } = &f.doc_run[0].kind else {
            panic!("expected an attention line");
        };
        assert!(attr.is_none());
        assert_eq!(text, "bare prose line");
        let DocRunKind::Attention { attr, text, .. } = &f.doc_run[1].kind else {
            panic!("expected an attention line");
        };
        assert_eq!(attr.as_ref().expect("has an attribute").name, "deprecated");
        assert_eq!(text, "[deprecated] use goToStart instead");
    }

    #[test]
    fn doc_run_collects_docs_then_attention_in_order() {
        let tokens = lex("? doc line\n! [deprecated] msg\nexport helper() { right; }").unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(f) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        assert!(f.exported, "export threads through unaffected by the run");
        assert_eq!(f.doc_run.len(), 2);
        assert!(matches!(f.doc_run[0].kind, DocRunKind::Doc { .. }));
        assert!(matches!(f.doc_run[1].kind, DocRunKind::Attention { .. }));
    }

    #[test]
    fn doc_run_binds_to_a_nested_function_at_its_own_indent() {
        // Indentation before both the sigil and the nested function's
        // name — the run still lexes/attaches correctly (design doc:
        // "runs sit at the bound declaration's own indent").
        let tokens =
            lex("main() {\n    ? step one\n    step() { right; }\n    @step();\n}").unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(main) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        assert!(
            main.doc_run.is_empty(),
            "the run binds to `step`, not `main`"
        );
        let BodyKind::Nested(step) = &main.body[0].kind else {
            panic!("expected the nested function first");
        };
        assert_eq!(step.name, "step");
        assert_eq!(step.doc_run.len(), 1);
        let DocRunKind::Doc { text, .. } = &step.doc_run[0].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, "step one");
        assert!(matches!(main.body[1].kind, BodyKind::Statement(_)));
    }

    #[test]
    fn doc_run_tolerates_blanks_and_comments_within_and_after() {
        use crate::lexer::{LexMode, lex_with};

        let src = "\
? first
// mid comment

? second

// trailing comment before fn
main() { right; }
";
        let tokens = lex_with(src, LexMode::WithComments).unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(f) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        assert_eq!(f.doc_run.len(), 4);
        let DocRunKind::Doc { text, .. } = &f.doc_run[0].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, "first");
        assert!(!f.doc_run[0].blank_before);
        let DocRunKind::Comment(c) = &f.doc_run[1].kind else {
            panic!("expected the mid-run comment");
        };
        assert_eq!(c.text, "// mid comment");
        assert!(!f.doc_run[1].blank_before);
        let DocRunKind::Doc { text, .. } = &f.doc_run[2].kind else {
            panic!("expected the second doc line");
        };
        assert_eq!(text, "second");
        assert!(f.doc_run[2].blank_before, "a blank line precedes it");
        let DocRunKind::Comment(c) = &f.doc_run[3].kind else {
            panic!("expected the trailing comment");
        };
        assert_eq!(c.text, "// trailing comment before fn");
        assert!(f.doc_run[3].blank_before, "a blank line precedes it");
        // No blank between the run's last line and the bound function.
        assert!(!cst.items[0].blank_before);
    }

    #[test]
    fn doc_run_before_a_nested_function_amid_sibling_statements() {
        let tokens = lex(
            "main() {\n    left;\n    ? helper doc\n    helper() { right; }\n    @helper();\n    left;\n}",
        )
        .unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(main) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        assert_eq!(main.body.len(), 4);
        assert!(matches!(main.body[0].kind, BodyKind::Statement(_)));
        let BodyKind::Nested(helper) = &main.body[1].kind else {
            panic!("expected the nested function");
        };
        assert_eq!(helper.name, "helper");
        assert_eq!(helper.doc_run.len(), 1);
        assert!(matches!(main.body[2].kind, BodyKind::Statement(_)));
        assert!(matches!(main.body[3].kind, BodyKind::Statement(_)));
    }

    /// The C1 parity guard (`parse == lower_cst ∘ parse_cst`) exercised on
    /// an actual documented program, not just argued from `parse`'s own
    /// definition. Task 3 lands the `doc_run` → `FnDoc` reduction, so a
    /// documented function no longer lowers to the exact same `Program`
    /// as its undocumented twin — `doc` is now the one field that
    /// differs. Isolates the comparison to "does the reduction leak
    /// anything ELSE into the rest of the AST": strip `doc` back off the
    /// documented function and the two programs must match exactly (the
    /// twin is padded with blank lines so `main`'s own line/col line up
    /// too).
    #[test]
    fn documented_function_lowers_to_its_undocumented_twin_plus_a_doc() {
        let doc = parse_src("? doc\n! [deprecated] msg\nmain() { right; }").unwrap();
        let bare = parse_src("\n\nmain() { right; }").unwrap();
        assert_eq!(bare.functions[0].doc, None);
        assert_eq!(
            doc.functions[0].doc,
            Some(FnDoc {
                paragraphs: vec!["doc".to_string()],
                attention: vec![],
                deprecated: Some("msg".to_string()),
            })
        );
        let mut doc_stripped = doc;
        doc_stripped.functions[0].doc = None;
        assert_eq!(doc_stripped, bare);
    }

    #[test]
    fn doc_run_round_trips_and_keeps_text_verbatim() {
        // Pins the WARM-UP lexer contract (minus-ONE-space rule) at the
        // CST layer too, plus verbatim internal spacing in an attention
        // line's full payload — no extra normalization happens here.
        let src = "?text\n?  text\n! [deprecated] msg with  double  spaces\nmain() { right; }";
        let tokens = lex(src).unwrap();
        let cst = parse_cst(&tokens).unwrap();
        assert_eq!(cst.clone(), cst, "lossless round-trip: clone() == self");

        let TopKind::Function(f) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        let DocRunKind::Doc { text, .. } = &f.doc_run[0].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, "text");
        let DocRunKind::Doc { text, .. } = &f.doc_run[1].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, " text"); // one space consumed, one remains
        let DocRunKind::Attention { attr, text, .. } = &f.doc_run[2].kind else {
            panic!("expected an attention line");
        };
        assert_eq!(attr.as_ref().expect("has an attribute").name, "deprecated");
        assert_eq!(text, "[deprecated] msg with  double  spaces");
    }

    #[test]
    fn doc_line_order_rejects_interleave_and_wrong_order() {
        let e = parse_src("? doc\n! attn\n? doc2\nmain() { right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DocLineOrder));
        assert_eq!(e.kind.code(), "doc-line-order");
        assert_eq!((e.span.start.line, e.span.start.col), (3, 1));

        let e = parse_src("! attn only\n? doc after\nmain() { right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DocLineOrder));
        assert_eq!(e.kind.code(), "doc-line-order");
        assert_eq!((e.span.start.line, e.span.start.col), (2, 1));
    }

    #[test]
    fn dangling_doc_run_at_top_level_and_in_body() {
        // Each source's run starts at col 1, on the line it's actually
        // written — the run's own first line, not wherever the parser
        // gave up.
        let top_level = [
            "? orphan doc\nuse std::goToEnd;\n",
            "? orphan doc\nnamespace ns { }\n",
            "? orphan doc\n",
        ];
        for src in top_level {
            let e = parse_src(src).unwrap_err();
            assert!(
                matches!(e.kind, CompileErrorKind::DanglingDocRun),
                "{src:?} got {:?}",
                e.kind
            );
            assert_eq!(e.kind.code(), "dangling-doc-run");
            assert_eq!((e.span.start.line, e.span.start.col), (1, 1), "{src:?}");
        }

        let in_body = [
            ("main() {\n? orphan\nright;\n}", 2), // dangling before a statement
            ("main() {\nright;\n? orphan\n}", 3), // dangling before the close brace
        ];
        for (src, want_line) in in_body {
            let e = parse_src(src).unwrap_err();
            assert!(
                matches!(e.kind, CompileErrorKind::DanglingDocRun),
                "{src:?} got {:?}",
                e.kind
            );
            assert_eq!(e.kind.code(), "dangling-doc-run");
            assert_eq!(
                (e.span.start.line, e.span.start.col),
                (want_line, 1),
                "{src:?}"
            );
        }
    }

    #[test]
    fn unknown_attribute_is_rejected_with_the_attr_span() {
        let e = parse_src("! [depercated] old api\nmain() { right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownAttribute(ref n) if n == "depercated"));
        assert_eq!(e.kind.code(), "unknown-attribute");
        assert_eq!((e.span.start.line, e.span.start.col), (1, 4));
    }

    /// WARM-UP pin (T2 review carry-over): `parse_attr`'s column math
    /// (`docs/superpowers/specs/2026-07-12-pmc-doc-lines-attributes-design.md`)
    /// is char-counted throughout (`Token::len`, `text.chars().count()`),
    /// never byte-counted — a non-ASCII payload AFTER the attribute
    /// (`café`, where `é` is one `char` but two UTF-8 bytes) must not
    /// perturb the attribute name's own span, since nothing about
    /// `[xx]`'s position depends on what follows it.
    #[test]
    fn unknown_attribute_span_is_char_counted_past_a_non_ascii_payload() {
        let e = parse_src("! [xx] café\nmain() { right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownAttribute(ref n) if n == "xx"));
        assert_eq!(e.kind.code(), "unknown-attribute");
        assert_eq!(
            (
                e.span.start.line,
                e.span.start.col,
                e.span.end.line,
                e.span.end.col
            ),
            (1, 4, 1, 6)
        );
    }

    #[test]
    fn duplicate_deprecated_attribute_is_rejected_at_the_second_occurrence() {
        let e = parse_src("! [deprecated] first\n! [deprecated] second\nmain() { right; }")
            .unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateAttribute));
        assert_eq!(e.kind.code(), "duplicate-attribute");
        assert_eq!((e.span.start.line, e.span.start.col), (2, 4));
    }

    // Task 3: the `doc_run` → `FnDoc` reduction lowered onto
    // `Function::doc`. `Analysis.docs`'s qualification (top-level,
    // nested dot-mangled, namespaced) is covered in `compiler.rs`'s
    // tests; these pin the CST -> AST reduction itself.

    #[test]
    fn fn_doc_paragraphs_join_with_a_single_space_and_split_on_an_empty_doc_line() {
        let prog =
            parse_src("? line one\n? line two\n?\n? second para\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.paragraphs, vec!["line one line two", "second para"]);
        assert!(doc.attention.is_empty());
        assert_eq!(doc.deprecated, None);
    }

    #[test]
    fn fn_doc_leading_and_trailing_empty_doc_lines_produce_no_empty_paragraphs() {
        let prog = parse_src("?\n?\n? doc\n?\n?\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.paragraphs, vec!["doc"]);
    }

    #[test]
    fn fn_doc_attention_prose_is_captured_verbatim_in_order() {
        let prog = parse_src("! first note\n! second note\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert!(doc.paragraphs.is_empty());
        assert_eq!(doc.attention, vec!["first note", "second note"]);
        assert_eq!(doc.deprecated, None);
    }

    // WARM-UP pin (1) (T2 review carry-over): a bracket that doesn't sit
    // at the payload's very start is NOT an attribute — `parse_attr`
    // already returns `None` for it (first char isn't `[`), and the
    // whole line is bare prose that lands verbatim in `attention`.
    #[test]
    fn fn_doc_attention_bracket_mid_prose_has_no_attr_and_lands_verbatim() {
        let tokens = lex("! see [deprecated] docs\nmain() { right; }").unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let TopKind::Function(f) = &cst.items[0].kind else {
            panic!("expected a function item");
        };
        let DocRunKind::Attention { attr, .. } = &f.doc_run[0].kind else {
            panic!("expected an attention line");
        };
        assert!(attr.is_none(), "bracket mid-prose is not an attribute");

        let prog = lower_cst(&cst);
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.attention, vec!["see [deprecated] docs"]);
        assert_eq!(doc.deprecated, None);
    }

    #[test]
    fn fn_doc_deprecated_message_captured_with_and_without_a_message() {
        let prog = parse_src("! [deprecated] use goToStart instead\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.deprecated, Some("use goToStart instead".to_string()));
        assert!(doc.attention.is_empty());

        let prog = parse_src("! [deprecated]\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.deprecated, Some(String::new()));
    }

    #[test]
    fn fn_doc_deprecated_line_is_excluded_from_attention_while_bare_prose_survives() {
        let prog =
            parse_src("! note one\n! [deprecated] use bar instead\n! note two\nmain() { right; }")
                .unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.attention, vec!["note one", "note two"]);
        assert_eq!(doc.deprecated, Some("use bar instead".to_string()));
    }

    #[test]
    fn fn_doc_comment_items_in_the_run_contribute_nothing_and_never_split_a_paragraph() {
        use crate::lexer::{LexMode, lex_with};
        let src =
            "? first\n// mid comment\n? second\n// trailing comment before fn\nmain() { right; }";
        let tokens = lex_with(src, LexMode::WithComments).unwrap();
        let cst = parse_cst(&tokens).unwrap();
        let prog = lower_cst(&cst);
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.paragraphs, vec!["first second"]);
    }

    #[test]
    fn undocumented_function_has_no_doc() {
        let prog = parse_src("main() { right; }").unwrap();
        assert_eq!(prog.functions[0].doc, None);
    }
}
