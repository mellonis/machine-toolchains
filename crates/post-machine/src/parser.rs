//! `.pmc` recursive-descent parser (docs/pmt/language.md): tokens → AST.

use std::collections::HashSet;
use std::rc::Rc;

use mtc_core::diagnostics::Span;
use mtc_core::syntax::{Checkpoint, GreenNode, SyntaxNode};

use crate::compiler::{CompileError, CompileErrorKind};
use crate::lexer::{Comment, LexMode, Token, TokenKind, lex_with};
use crate::syntax::{self, GreenSink, PmcKind};

/// docs/pmt/language.md: words that cannot name a function.
pub const RESERVED: [&str; 8] = [
    "goto", "check", "left", "right", "mark", "unmark", "halt", "debugger",
];

/// True for every [`RESERVED`] command word AND for `volatile` — unlike
/// the `RESERVED` vocabulary, `volatile` is not a statement keyword (it
/// never starts a top-level statement the way `goto`/`left`/… do); it is
/// reserved from naming a function or namespace ONLY. This is the check
/// the two definition-name sites (`function`'s own name, a
/// `namespace NAME {`'s name) run; it is never used for the modifier's
/// own contextual-keyword lookahead (`Parser::peek_is_volatile_modifier`),
/// which disambiguates the modifier from the (rejected) literal name in
/// the first place.
fn is_reserved_definition_name(name: &str) -> bool {
    RESERVED.contains(&name) || name == "volatile"
}

/// The `.pmc` language acceptance-contract version (docs/pmt/language.md):
/// pre-1.0 the version is 0.N and N bumps on ANY grammar change; at a
/// declared 1.0 the axes activate (major = breaking, minor = additive).
/// No patch digit — spec-text corrections are errata;
/// implementation-conformance fixes live in the crate changelog. The
/// sigil-adjacency, reserved-path, and empty-builtin-parens tightenings
/// made this 0.2 (the v1 grammar is retroactively 0.1). Doc lines (`?`)
/// and attention lines (`!`) — plus the accompanying acceptance change
/// that a line-leading `!` is always an attention line, never a
/// successor — made this 0.3 (docs/pmt/language.md "Doc lines and
/// attention lines"). Reserving `volatile` and accepting it as the
/// leading modifier of the un-namespaced top-level `main` — everywhere
/// else it is either a reserved name or `VolatileNotOnMain` — made this
/// 0.4.
pub const PMC_LANG_VERSION: &str = "0.4";

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
    /// The alias NAME's own span — the declaration a call written with
    /// the alias navigates to (docs/lsp.md (go-to-definition)).
    pub alias_span: Option<Span>,
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
    /// The `volatile` modifier: `VolatileNotOnMain` rejects it everywhere
    /// except the un-namespaced top-level `main`, so by the time a
    /// `Function` exists this is `true` for at most that one definition
    /// and `false` for every other. Unlike `exported`, nothing folds into
    /// it — the parser only ever copies the written token through.
    pub volatile: bool,
    /// Nesting is always local; flatten computes this for top-level
    /// functions as `!exported`.
    pub local: bool,
    /// Nested function definitions (docs/pmt/language.md (visibility)), hoisted and visible to
    /// their own siblings and enclosing scope's body; emptied by flatten.
    pub nested: Vec<Function>,
    /// Enclosing namespace path (parser-set on top-level definitions;
    /// nested functions inherit through their top-level ancestor). The
    /// full symbol joins namespaces with `::` and nesting with `.` —
    /// `std::api.helper`.
    pub ns: Vec<String>,
    /// The bound `?`/`!` run — `docs/pmt/language.md (doc lines and attention
    /// lines)` — reduced by [`reduce_doc_run`] from the [`DocRunItem`]s
    /// extraction reads back off the green tree's `DOC_RUN` node. `None`
    /// for an undocumented function (an empty `doc_run`); every compiler
    /// pass past extraction ignores this field — `flatten` copies it
    /// into `Analysis.docs`, keyed by the same fully-qualified name it
    /// already computes, and nothing downstream reads it off `Function`
    /// again.
    pub doc: Option<FnDoc>,
}

/// One function's reduced doc/attention run (`docs/pmt/language.md`, doc lines
/// and attention lines): paragraphs from `?`
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
    /// `value` (docs/pmt/fmt.md: fmt never touches a token).
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
        /// (docs/pmt/fmt.md: fmt never touches a token).
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
        /// `label` (docs/pmt/fmt.md: fmt never touches a token).
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
        // BOTH modes (docs/pmt/language.md (doc lines)), so — unlike
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

/// Source → AST, through the one parse path this crate has: a
/// `WithComments` lex, the green syntax tree, then extraction
/// (docs/core.md (syntax trees)).
///
/// The convenience wrapper for callers that want only the `Program` —
/// it keeps nothing else, and it is the only parse function here that
/// yields one. A caller needing the token stream alongside it uses
/// `compiler::analyze`; a caller needing the green tree uses
/// `compiler::analyze_staged`, which is the one that retains it.
pub fn parse(source: &str) -> Result<Program, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    Ok(crate::syntax::extract_program(
        &SyntaxNode::new_root(green),
        source,
    ))
}

/// Every token that is not comment trivia — the stream `Parser` walks,
/// since comments reach the green tree as trivia through
/// [`crate::syntax::layout`] rather than through the walk. Equal,
/// element for element, to a
/// `LexMode::WithoutComments` lex of the same source: the lexer's mode
/// switch decides only whether a `Comment` token is pushed. That law is
/// checked corpus-wide by
/// `tests/syntax_green.rs::corpus_token_provenance_law` — which
/// re-derives the filter inline rather than calling this function — and
/// against this function directly by
/// `parse_green_from_tokens_matches_parse_green`. Together they are what
/// lets `compiler::analyze` fill `AnalysisOutput.tokens` from the one
/// `WithComments` lex the green parse already needs.
pub fn significant_tokens(tokens: &[Token]) -> Vec<Token> {
    tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
        .cloned()
        .collect()
}

/// source → green syntax tree (docs/core.md (syntax trees)). Lexes
/// `WithComments` and hands both halves to
/// [`parse_green_from_tokens`].
pub fn parse_green(source: &str) -> Result<Rc<GreenNode>, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    parse_green_from_tokens(source, &tokens)
}

/// Already-lexed tokens → green syntax tree, for callers that need to
/// keep the token stream even when the parse fails (the staged
/// pipeline's degradation tiers, docs/lsp.md (staged analysis)).
///
/// `tokens` MUST be a `LexMode::WithComments` lex of `source`:
/// `crate::syntax::layout` reconstructs verbatim token text and trivia
/// from the two together, so a comment-free stream would lose every
/// comment's own text and break the `text() == source` law. An empty
/// `tokens` slice panics — every real lex result is EOF-terminated.
///
/// Runs the same grammar walk `Parser::file` always has, with a green
/// sink attached: the sink mirrors token consumption and node
/// boundaries alongside the unchanged parser logic, and the tree it
/// builds is the walk's whole product (see [`Parser::sink`]'s own doc).
pub fn parse_green_from_tokens(
    source: &str,
    tokens: &[Token],
) -> Result<Rc<GreenNode>, CompileError> {
    let entries = syntax::layout(source, tokens);
    let sig = significant_tokens(tokens);
    let eof_pos = sig.len() - 1;
    let mut sink = GreenSink::new(entries);
    sink.start(PmcKind::File);
    let sink = Parser {
        tokens: &sig,
        pos: 0,
        namespaces: HashSet::new(),
        declared_fns: HashSet::new(),
        sink: Some(sink),
        recovered: None,
    }
    .file()?;
    Ok(sink
        .expect("parse_green_from_tokens always seeds a sink before calling file()")
        .finish_tree(eof_pos))
}

/// A resilient parse's product: the tree is ALWAYS built — broken
/// regions wrapped in [`PmcKind::Error`] nodes — and lossless
/// (`text() == source`); `errors` carries each recovery's error in
/// source order, empty on a clean document, where the tree is
/// byte-identical to [`parse_green_from_tokens`]'s.
pub struct ResilientParse {
    pub green: Rc<GreenNode>,
    pub errors: Vec<CompileError>,
}

/// [`parse_green_from_tokens`]'s error-recovering twin (docs/core.md
/// (syntax trees), error recovery) — the language service's entry, so
/// the editor keeps a CURRENT tree mid-edit. The batch pipeline never
/// calls this: its fatal contract (first error, no tree) is unchanged.
/// The first recorded error is always the same error, kind and span,
/// that the fatal entry reports on the same input.
pub fn parse_green_resilient(source: &str, tokens: &[Token]) -> ResilientParse {
    let entries = syntax::layout(source, tokens);
    let sig = significant_tokens(tokens);
    let eof_pos = sig.len() - 1;
    let mut sink = GreenSink::new(entries);
    sink.start(PmcKind::File);
    let parser = Parser {
        tokens: &sig,
        pos: 0,
        namespaces: HashSet::new(),
        declared_fns: HashSet::new(),
        sink: Some(sink),
        recovered: Some(Vec::new()),
    };
    let (sink, errors) = parser
        .file_resilient()
        .expect("resilient mode recovers every item error");
    ResilientParse {
        green: sink
            .expect("parse_green_resilient always seeds a sink")
            .finish_tree(eof_pos),
        errors,
    }
}

/// One line of a function's bound `?`/`!` run, plus whether a blank
/// line precedes it in source. Built by [`reparse_doc_items`] over a
/// retokenized `DOC_RUN` node, which is the one route a run reaches
/// [`reduce_doc_run`] by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRunItem {
    pub blank_before: bool,
    pub kind: DocRunKind,
}

/// A doc/attention run's own line shapes (docs/pmt/language.md (doc lines)):
/// a `?` doc line, a `!` attention line, or an ordinary comment
/// interleaved within/after the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocRunKind {
    /// A `?` line. `text` is the lexer's payload verbatim (raw text
    /// after the sigil, minus one canonical leading space if present) —
    /// unprocessed, so a pretty-printer reprints it byte-for-byte.
    Doc { text: String, span: Span },
    /// A `!` line. `attr` is `Some` when the payload opens with a valid
    /// `[ident]` attribute (v1: only `[deprecated]` is accepted —
    /// anything else is a parse-time `UnknownAttribute` error, so every
    /// `Some` that survives the parse already named
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
    /// declaration). Never produced by [`reparse_doc_items`], the one
    /// route a run reaches [`reduce_doc_run`] by — a retokenized
    /// `DOC_RUN` has already had its comments filtered out as trivia
    /// (that shim's own doc explains why dropping them is harmless).
    Comment(Comment),
}

/// An attention line's leading `[ident]` attribute (docs/pmt/language.md
/// (doc lines)): v1 accepts exactly `"deprecated"`. `span` covers the
/// identifier alone, not the surrounding brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrCst {
    pub name: String,
    pub span: Span,
}

/// Reduce a function's bound `?`/`!` run into an [`FnDoc`]. The run
/// arrives as the [`DocRunItem`]s [`reparse_doc_items`] rebuilds off an
/// emitted `DOC_RUN` node — the one route in. `pub(crate)` for
/// cross-module use by `crate::syntax::extract::extract_function`, the
/// green-tree extraction's own production caller, and by that module's
/// fidelity tests, which lean on the `DocRunKind::Comment` inertness
/// documented right below to prove two
/// RAW `Vec<DocRunItem>`s that differ only by a dropped comment still
/// reduce to the identical [`FnDoc`] (`reparse_doc_items`'s own doc
/// comment explains why they can differ in the first place). `None` for
/// an empty run (undocumented). `DocRunKind::Comment` items are transparent:
/// they contribute nothing and never split a paragraph, matching the
/// design doc's "comments/blanks don't participate" rule for the run's
/// own order check. A `?` line's text is the join key; an EMPTY `?` line
/// (the lexer's bare-sigil payload) closes the current paragraph without
/// emitting an empty one, so leading/trailing/repeated blanks are all
/// absorbed. An attention line with `attr.name == "deprecated"` (at most
/// one — a second is rejected at parse time) is excluded from
/// `attention`; its message is the FULL raw
/// payload's text after the attribute's closing `]`, trimmed — finding
/// `]` in `text` directly is equivalent to (and simpler than) mapping
/// `attr.span.end` back into the string, since `parse_attr` only
/// recognizes `[ident]` at the payload's very start. An EMPTY bare `!`
/// line (no attribute, no text) contributes nothing to `attention` — the
/// same "an empty line carries no content" rule the `?` side already
/// applies — so a lone bare `!` can't leave `FnDoc` with a single empty
/// attention entry and no other content.
pub(crate) fn reduce_doc_run(doc_run: &[DocRunItem]) -> Option<FnDoc> {
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
                _ if text.is_empty() => {}
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

/// What [`Parser::function`] still tells its callers: the name token's
/// own identity. Both call sites key their duplicate-name checks on it,
/// and the top-level one also needs the name to decide whether a leading
/// `volatile` is on `main` (`VolatileNotOnMain`) — everything else about
/// a function declaration is read back off the green tree.
struct FnName {
    name: String,
    line: u32,
    col: u32,
}

struct Parser<'a> {
    /// Significant (comment-free) tokens only — identical to the
    /// `WithoutComments` stream, so a `WithComments` lex changes what
    /// the green sink records as trivia and nothing about the grammar
    /// walk itself.
    tokens: &'a [Token],
    pos: usize,
    /// Every namespace path declared so far (reopened blocks insert the
    /// same path again, harmlessly). Namespace names share the name pool
    /// with function names per scope — a human-clarity rule: since `::`
    /// (namespaces) and `.` (nesting) are distinct separators, `a::x`
    /// and `a.x` cannot collide; the pool rule just stops both spellings
    /// coexisting confusingly in one file.
    namespaces: HashSet<Vec<String>>,
    /// Every `(ns, name)` function declared so far — the same-scope
    /// duplicate check is a membership test on this set, so it stays
    /// independent of how the walk happens to nest its blocks and of
    /// the order [`crate::syntax::extract_program`] later flattens
    /// them in.
    declared_fns: HashSet<(Vec<String>, String)>,
    /// Green-tree emission: `bump()` mirrors every consumed token into
    /// it, and the `g_*` helpers below bracket node boundaries. `Some`
    /// on [`parse_green_from_tokens`]'s walk — [`Parser::file`]'s only
    /// production caller; `None` on the isolated re-parses
    /// ([`reparse_item`], [`reparse_doc_items`]), which are handed a
    /// bare token slice with no source string to lay out and so cannot
    /// be given a sink at all; there the `g_*` helpers are all no-ops
    /// and the production's own return value is the whole result.
    sink: Option<GreenSink>,
    /// `Some` = resilient mode ([`parse_green_resilient`]): item-level
    /// errors are recorded here and recovered from at the loop seams;
    /// `None` = the fatal contract (every other entry).
    recovered: Option<Vec<CompileError>>,
}

/// What one [`Parser::top_item`] iteration decided: the level is done
/// (Eof at file level, or the terminator was consumed), or one item was
/// parsed and the loop continues.
enum Step {
    Done,
    Continue,
}

/// Map a significant `TokenKind` to its green-tree kind — the sink's
/// counterpart to `TokenKind`, since `PmcKind`'s token variants mirror
/// it 1:1 (`crate::syntax::kinds` doc). Called only from `bump()`,
/// which never bumps `Eof`, and only over the sig stream, which never
/// carries `Comment` ([`significant_tokens`] filters it out up front) —
/// both are unreachable here.
fn sig_kind(kind: &TokenKind) -> PmcKind {
    match kind {
        TokenKind::Ident(_) => PmcKind::Ident,
        TokenKind::Number(_, _) => PmcKind::Number,
        TokenKind::At => PmcKind::At,
        TokenKind::Bang => PmcKind::Bang,
        TokenKind::Comma => PmcKind::Comma,
        TokenKind::Semi => PmcKind::Semi,
        TokenKind::Colon => PmcKind::Colon,
        TokenKind::ColonColon => PmcKind::ColonColon,
        TokenKind::LParen => PmcKind::LParen,
        TokenKind::RParen => PmcKind::RParen,
        TokenKind::LBrace => PmcKind::LBrace,
        TokenKind::RBrace => PmcKind::RBrace,
        TokenKind::DocLine(_) => PmcKind::DocLine,
        TokenKind::AttentionLine(_) => PmcKind::AttentionLine,
        TokenKind::Comment(_) | TokenKind::Eof => {
            unreachable!("comments are stripped from the significant stream; Eof is never bumped")
        }
    }
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        // Safe: the lexer always appends Eof and bump() never passes it.
        &self.tokens[self.pos]
    }

    fn bump(&mut self) {
        if !matches!(self.tokens[self.pos].kind, TokenKind::Eof) {
            if let Some(sink) = &mut self.sink {
                sink.token(self.pos, sig_kind(&self.tokens[self.pos].kind));
            }
            self.pos += 1;
        }
    }

    /// Open a green node, flushing the upcoming token's trivia into the
    /// PARENT first — the trivia-placement rule: a node starts at its
    /// first significant token, so whitespace/comments before it belong
    /// to whatever is still open. No-op when `sink` is `None`.
    fn g_flush_start(&mut self, kind: PmcKind) {
        if let Some(sink) = &mut self.sink {
            sink.flush(self.pos);
            sink.start(kind);
        }
    }

    /// Close the innermost open green node. No-op when `sink` is `None`.
    fn g_finish(&mut self) {
        if let Some(sink) = &mut self.sink {
            sink.finish();
        }
    }

    /// Flush the upcoming token's trivia into the parent, then mark a
    /// green checkpoint for a later [`Self::g_start_at`] — for
    /// productions that only learn their node kind after parsing a
    /// prefix. `None` when `sink` is `None`.
    fn g_checkpoint(&mut self) -> Option<Checkpoint> {
        self.sink.as_mut().map(|sink| {
            sink.flush(self.pos);
            sink.checkpoint()
        })
    }

    /// Open a green node retroactively at a checkpoint taken by
    /// [`Self::g_checkpoint`]. No-op when either is `None`.
    fn g_start_at(&mut self, cp: Option<Checkpoint>, kind: PmcKind) {
        if let (Some(sink), Some(cp)) = (&mut self.sink, cp) {
            sink.start_at(cp, kind);
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

    /// The whole file is the `ns == []` namespace level. Hands back the
    /// (possibly `None`) green sink, which is the walk's whole product:
    /// `self` is consumed by value, so this is the only place it can
    /// escape.
    fn file(mut self) -> Result<Option<GreenSink>, CompileError> {
        self.top_items(&[], None)?;
        Ok(self.sink)
    }

    /// [`Self::file`] in resilient mode: also hands back the recovered
    /// errors. `Err` is unreachable — every item error is recovered at
    /// the loop seams — but the signature keeps the walk's own `?`s.
    #[allow(clippy::type_complexity)]
    fn file_resilient(mut self) -> Result<(Option<GreenSink>, Vec<CompileError>), CompileError> {
        self.top_items(&[], None)?;
        Ok((
            self.sink,
            self.recovered
                .expect("file_resilient runs in resilient mode"),
        ))
    }

    /// Collects a doc/attention run (docs/pmt/language.md (doc lines))
    /// starting at the current position — the caller has already
    /// confirmed `self.peek()` is `DocLine`/`AttentionLine`. A run is one
    /// optional contiguous `?` block then one optional contiguous `!`
    /// block; a `?` reached after the run has entered its `!` block is
    /// `DocLineOrder` (covers both interleaving and whole-run wrong
    /// order — a single "have we seen `!` yet" flag catches both, since
    /// ANY `?` after the first `!` violates the fixed order). Blank
    /// lines and ordinary comments are tolerated within/after the run
    /// without affecting that order check (spec: "comments/blanks don't
    /// participate") — they are trivia the green sink attaches on its
    /// own, and this walk simply steps over them.
    /// An attention line's `[ident]` attribute (if any) is validated
    /// against the v1 vocabulary here, since this is the only place the
    /// run's lines are walked in order. Returns the run's OWN first
    /// line's span, for the caller's `DanglingDocRun` error if what
    /// follows isn't the declaration the run must bind to. The run's
    /// [`DocRunItem`]s are extraction's to rebuild off the emitted
    /// `DOC_RUN` node, through [`reparse_doc_items`].
    fn doc_run(&mut self) -> Result<Span, CompileError> {
        let first_span = self.peek().span();
        let mut seen_attention = false;
        let mut seen_deprecated: Option<Span> = None;
        loop {
            let t = self.peek().clone();
            match &t.kind {
                TokenKind::DocLine(_) => {
                    if seen_attention {
                        return Err(Self::err_at(&t, CompileErrorKind::DocLineOrder));
                    }
                    self.bump();
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
                }
                _ => break,
            }
        }
        Ok(first_span)
    }

    /// Parses a leading `[ident]` attribute off an attention line's raw
    /// payload (docs/pmt/language.md (doc lines)): the exact shape `[`,
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

    /// True iff the current position is the `volatile` contextual keyword
    /// used as a modifier — `volatile` followed by another identifier.
    /// Mirrors `export`'s own keyword-vs-name disambiguation (`export` +
    /// identifier = modifier, `export` + `(` = a function literally
    /// named `export`) — but unlike `export`, `volatile` + `(` is never
    /// a legal name (`is_reserved_definition_name` rejects it in
    /// `function`'s own name check), so no separate `KeywordNeedsName`
    /// hint case is needed the way `namespace {`/`use {`/`export {}`
    /// needed one. Shared by the top-level header's own consumption, the
    /// nested-function-start predicate below, and the nested dispatch's
    /// own consumption — all three need the identical lookahead. Peek
    /// only; never advances `self.pos`.
    fn peek_is_volatile_modifier(&self) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(w) if w == "volatile")
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::Ident(_))
            )
    }

    /// True iff the current position starts a nested function definition
    /// (`IDENT ( ) {` — visibility-only nesting), optionally preceded by
    /// the `volatile` modifier (`volatile IDENT ( ) {`). The modifier is
    /// syntactically legal here — nesting still counts as a
    /// nested-function start so parsing (and doc-run attachment)
    /// proceeds exactly as it would without it — but the body loop's own
    /// dispatch always rejects it afterward with `VolatileNotOnMain`,
    /// since a nested definition is never the top-level `main`. Shared
    /// by the doc-run dangling check and the body loop's own
    /// nested-definition dispatch. Read-only.
    fn next_is_nested_function_start(&self) -> bool {
        let offset = usize::from(self.peek_is_volatile_modifier());
        matches!(
            self.tokens.get(self.pos + offset).map(|t| &t.kind),
            Some(TokenKind::Ident(w)) if !RESERVED.contains(&w.as_str())
        ) && matches!(
            self.tokens.get(self.pos + offset + 1).map(|t| &t.kind),
            Some(TokenKind::LParen)
        ) && matches!(
            self.tokens.get(self.pos + offset + 2).map(|t| &t.kind),
            Some(TokenKind::RParen)
        ) && matches!(
            self.tokens.get(self.pos + offset + 3).map(|t| &t.kind),
            Some(TokenKind::LBrace)
        )
    }

    /// One namespace level's item loop, walking the level's items in
    /// source order and bracketing a green node around each. Handles
    /// `use` (legal at any namespace depth, never in function bodies),
    /// `namespace NAME { … }` (contextual; recurse with the extended
    /// path), `export`, and function definitions. `terminator` is
    /// `Some(RBrace)` inside a block, `None` at file level (ends at Eof).
    /// Duplicate-name checks run here, during the walk — never over the
    /// extracted `Program` afterwards.
    /// One namespace level's item loop — the recovery seam
    /// (docs/core.md (syntax trees), error recovery). In fatal mode
    /// (`recovered: None`) an item error propagates unchanged. In
    /// resilient mode the error is recorded, the sink unwinds to this
    /// loop's depth, everything since the iteration's checkpoint is
    /// retro-wrapped into an ERROR node together with the tokens
    /// skipped to the next sync point, and the loop continues — so one
    /// broken item never takes its neighbours' parse with it.
    fn top_items(
        &mut self,
        ns: &[String],
        terminator: Option<&TokenKind>,
    ) -> Result<(), CompileError> {
        loop {
            let cp_for_recovery = self.g_checkpoint();
            let depth = self.sink.as_ref().map(GreenSink::open_depth);
            match self.top_item(ns, terminator, cp_for_recovery) {
                Ok(Step::Done) => return Ok(()),
                Ok(Step::Continue) => {}
                Err(e) => {
                    if self.recovered.is_none() {
                        return Err(e);
                    }
                    self.recovered.as_mut().expect("checked above").push(e);
                    if let (Some(sink), Some(d)) = (&mut self.sink, depth) {
                        sink.finish_to(d);
                    }
                    self.g_start_at(cp_for_recovery, PmcKind::Error);
                    self.skip_to_sync(terminator);
                    self.g_finish(); // Error
                    if matches!(self.peek().kind, TokenKind::Eof) {
                        // Nothing left to resync on — an unclosed block
                        // ends at Eof with its error recorded.
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Skip-and-emit resynchronization: consumes tokens into the open
    /// ERROR node until something that can start the next item (a
    /// doc/attention line, a `use`/`namespace`/`export`/`volatile`
    /// keyword, or a name followed by `(` — a function header), the
    /// level's terminator (left for the loop to consume), or Eof. A
    /// `;` is consumed INTO the region and ends it. Always makes
    /// progress: the sync check is skipped on the first token, since a
    /// failed parse may have consumed nothing and its own first token
    /// may itself be a sync token.
    fn skip_to_sync(&mut self, terminator: Option<&TokenKind>) {
        let mut first = true;
        // Brace depth WITHIN the region — a broken declaration may carry
        // its own `{ … }`; only depth-0 tokens end the region or match
        // the terminator, so an interior `;` or `}` stays part of it.
        let mut depth = 0usize;
        loop {
            let t = self.peek().clone();
            if matches!(t.kind, TokenKind::Eof) {
                return;
            }
            if depth == 0
                && let Some(term) = terminator
                && &t.kind == term
            {
                return;
            }
            // STRONG sync — a shape that can only start a new item —
            // ends the region at ANY depth: an UNBALANCED `{` in the
            // broken region must not swallow the rest of the file (the
            // exact mid-edit shape resilience exists for).
            let sync = match &t.kind {
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_) => true,
                TokenKind::Ident(w)
                    if matches!(w.as_str(), "use" | "namespace" | "export" | "volatile") =>
                {
                    true
                }
                TokenKind::Ident(w)
                    if !RESERVED.contains(&w.as_str())
                        && matches!(
                            self.tokens.get(self.pos + 1).map(|t| &t.kind),
                            Some(TokenKind::LParen)
                        ) =>
                {
                    true
                }
                _ => false,
            };
            if sync && !first {
                return;
            }
            match t.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth = depth.saturating_sub(1),
                _ => {}
            }
            let semi = matches!(t.kind, TokenKind::Semi) && depth == 0;
            self.bump();
            first = false;
            if semi {
                return;
            }
        }
    }

    /// One iteration of [`Self::top_items`]'s loop — the former loop
    /// body, factored out so the recovery wrapper owns the seam.
    fn top_item(
        &mut self,
        ns: &[String],
        terminator: Option<&TokenKind>,
        cp_for_recovery: Option<Checkpoint>,
    ) -> Result<Step, CompileError> {
        {
            // Doc/attention run (docs/pmt/language.md (doc lines)): a `?`/`!`
            // line at item position starts a run that must bind to the
            // NEXT function declaration at this scope — anything else
            // next (`use`, `namespace`, the block terminator, Eof, a
            // top-level statement) is `DanglingDocRun` at the run's own
            // first line. `next_is_top_level_function_start` mirrors this
            // loop's own dispatch conditions exactly, so classifying
            // "true" here guarantees the fallthrough function-parsing
            // code below is what actually runs next.
            // The green checkpoint for the FUNCTION node this run (if any)
            // binds to, or — with no run — for the header token about to
            // be consumed (`volatile`/`export`/name): taken here, before
            // either, so `g_start_at` below retro-wraps whichever prefix
            // was actually present. Unused whenever this token turns out to start a
            // `use`/`namespace` item instead — harmless, a fresh
            // checkpoint is taken every loop iteration.
            let fn_cp = cp_for_recovery;
            if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                self.g_flush_start(PmcKind::DocRun);
                let first_span = self.doc_run()?;
                self.g_finish();
                if !self.next_is_top_level_function_start(terminator) {
                    return Err(CompileError {
                        span: first_span,
                        kind: CompileErrorKind::DanglingDocRun,
                    });
                }
            }
            let t = self.peek().clone();
            match (&t.kind, terminator) {
                (TokenKind::Eof, None) => return Ok(Step::Done),
                (TokenKind::Eof, Some(_)) => {
                    return Err(Self::expected(&t, "`}` to close the namespace block"));
                }
                (k, Some(term)) if k == term => {
                    self.bump();
                    return Ok(Step::Done);
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
                self.g_flush_start(PmcKind::UseDecl);
                self.bump();
                loop {
                    // path := IDENT (`::` IDENT)*  [ `as` IDENT ]
                    let t = self.peek().clone();
                    let TokenKind::Ident(name) = &t.kind else {
                        return Err(Self::expected(&t, "an imported function name"));
                    };
                    if RESERVED.contains(&name.as_str()) {
                        return Err(Self::expected(&t, "an imported function name"));
                    }
                    self.g_flush_start(PmcKind::UsePath);
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
                        self.bump();
                    }
                    if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "as") {
                        self.bump();
                        let t = self.peek().clone();
                        let TokenKind::Ident(_) = &t.kind else {
                            return Err(Self::expected(&t, "an alias after `as`"));
                        };
                        self.bump();
                    }
                    self.g_finish(); // UsePath — the alias, if any, is its last token
                    let sep = self.peek().clone();
                    match sep.kind {
                        TokenKind::Comma => {
                            self.bump();
                        }
                        TokenKind::Semi => {
                            self.bump();
                            break;
                        }
                        TokenKind::Colon => {
                            return Err(Self::err_at(&sep, CompileErrorKind::SingleColonInPath));
                        }
                        _ => return Err(Self::expected(&sep, "`,` or `;`")),
                    }
                }
                self.g_finish(); // UseDecl — closes right after the `;`
                return Ok(Step::Continue);
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
                self.g_flush_start(PmcKind::Namespace);
                self.bump(); // `namespace`
                let name_tok = self.peek().clone();
                let TokenKind::Ident(name) = &name_tok.kind else {
                    unreachable!("checked above");
                };
                let name = name.clone();
                if is_reserved_definition_name(&name) {
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
                self.bump(); // the name
                self.bump(); // `{`
                let mut child = ns.to_vec();
                child.push(name.clone());
                self.namespaces.insert(child.clone());
                self.top_items(&child, Some(&TokenKind::RBrace))?;
                // The recursive `top_items` call above already bumped the
                // closing `}` into the still-open NAMESPACE node; close it
                // now that its full span — including that `}` — is emitted.
                self.g_finish(); // Namespace
                return Ok(Step::Continue);
            }
            // Contextual keyword: `volatile` + identifier = the volatile
            // modifier; `volatile` + `(` is a function literally NAMED
            // `volatile` — `function`'s own name check rejects that as a
            // reserved name, so it is never treated as the modifier
            // here. Fixed order: `volatile` precedes `export` when both
            // are written.
            let volatile_tok = if self.peek_is_volatile_modifier() {
                let tok = self.peek().clone();
                self.bump();
                Some(tok)
            } else {
                None
            };
            // Contextual keyword: `export` + identifier = exported def;
            // `export` + `(` is a function NAMED export. Whether a
            // function is exported is extraction's own reading of the
            // green tree; this walk only consumes the keyword.
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
            {
                self.bump();
            }
            // The header is confirmed now (a function IS what follows):
            // retro-open FUNCTION at `fn_cp`, so it wraps the doc run
            // (if any) and/or the `volatile`/`export` tokens already
            // emitted above. `function` closes it after the `}`.
            self.g_start_at(fn_cp, PmcKind::Function);
            let f = self.function()?;
            // `volatile` is legal ONLY on the un-namespaced top-level
            // `main` — checked before the duplicate-name checks below so
            // the more specific rule is the one that surfaces.
            if let Some(tok) = &volatile_tok
                && !(ns.is_empty() && f.name == "main")
            {
                return Err(Self::err_at(
                    tok,
                    CompileErrorKind::VolatileNotOnMain(f.name.clone()),
                ));
            }
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
        }
        Ok(Step::Continue)
    }

    // Walks one function definition — its name, its empty parameter
    // parens, and its brace-delimited body — and hands back the name
    // token's own identity, which is all either call site still needs:
    // the duplicate-name and shared-name-pool checks in `top_items`, the
    // sibling-name check in this method's own nested-definition branch,
    // and `VolatileNotOnMain`'s message all key on it. A leading doc run
    // and a leading `volatile`/`export` are the CALLER's to consume and
    // to judge — this method never sees them, and never collects a run
    // itself (the caller owns the "what comes next" dispatch a run's
    // dangling check depends on).
    //
    // Green FUNCTION node: opened by the CALLER (`g_start_at` at a
    // checkpoint taken before either call site invokes this method) and
    // closed HERE, right after the closing `}` is bumped — the one
    // `Ok` exit this loop has. Both call sites call `g_start_at`
    // unconditionally, immediately before invoking this method, so the
    // open/close pair always balances; a future third call site (or a
    // second `Ok` exit added here) must preserve both halves of that
    // contract or the green builder mis-nests.
    fn function(&mut self) -> Result<FnName, CompileError> {
        let name_tok = self.peek().clone();
        let TokenKind::Ident(name) = &name_tok.kind else {
            return Err(Self::expected(&name_tok, "a function name"));
        };
        let name = name.clone();
        if is_reserved_definition_name(&name) {
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
        self.expect(&TokenKind::LBrace, "`{`")?;

        let mut nested_names: HashSet<String> = HashSet::new();
        let mut seen_labels: HashSet<u32> = HashSet::new();
        // The body loop is the INNER recovery seam (docs/core.md (syntax
        // trees), error recovery) — same wrapper as `top_items`': in
        // resilient mode a statement/nested-definition error is
        // recorded, the region since the iteration's checkpoint wraps
        // into an ERROR node, and the loop resyncs at the next
        // statement boundary — one broken statement never takes its
        // function (or the file) with it.
        loop {
            let cp = self.g_checkpoint();
            let depth = self.sink.as_ref().map(GreenSink::open_depth);
            match self.fn_body_item(&mut nested_names, &mut seen_labels, cp) {
                Ok(Step::Done) => break,
                Ok(Step::Continue) => {}
                Err(e) => {
                    if self.recovered.is_none() {
                        return Err(e);
                    }
                    self.recovered.as_mut().expect("checked above").push(e);
                    if let (Some(sink), Some(d)) = (&mut self.sink, depth) {
                        sink.finish_to(d);
                    }
                    self.g_start_at(cp, PmcKind::Error);
                    self.skip_to_stmt_sync();
                    self.g_finish(); // Error
                    if matches!(self.peek().kind, TokenKind::Eof) {
                        // An unclosed body ends at Eof with its error
                        // recorded — close FUNCTION the way the RBrace
                        // arm does.
                        self.g_finish(); // Function
                        break;
                    }
                }
            }
        }
        Ok(FnName {
            name,
            line: name_tok.line,
            col: name_tok.col,
        })
    }

    /// Statement-boundary resynchronization for [`Self::fn_body_item`]'s
    /// recovery: consumes tokens into the open ERROR node until
    /// something that can start the next body item — a label number, a
    /// command keyword, `@`, a doc/attention line, or a name followed by
    /// `(` (a nested definition) — or the body's `}` / Eof (left for the
    /// loop). A `;` is consumed INTO the region and ends it; the sync
    /// check skips the first token so recovery always makes progress.
    fn skip_to_stmt_sync(&mut self) {
        let mut first = true;
        // Brace depth WITHIN the region — a broken nested definition may
        // carry its own `{ … }`; only a depth-0 `;` ends the region and
        // only a depth-0 `}` is the body's closer.
        let mut depth = 0usize;
        loop {
            let t = self.peek().clone();
            if matches!(t.kind, TokenKind::Eof) {
                return;
            }
            if matches!(t.kind, TokenKind::RBrace) && depth == 0 {
                return;
            }
            // Weak sync (a label, a command, `@`) counts only at depth
            // 0 — those tokens legitimately occur inside a region's own
            // braces; STRONG shapes (doc lines, a name followed by `(`)
            // end the region at any depth, so an unbalanced `{` cannot
            // swallow the rest of the body.
            let sync = match &t.kind {
                TokenKind::Number(..) | TokenKind::At => depth == 0,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_) => true,
                TokenKind::Ident(w) if RESERVED.contains(&w.as_str()) => depth == 0,
                TokenKind::Ident(_)
                    if matches!(
                        self.tokens.get(self.pos + 1).map(|t| &t.kind),
                        Some(TokenKind::LParen)
                    ) =>
                {
                    true
                }
                _ => false,
            };
            if sync && !first {
                return;
            }
            match t.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth = depth.saturating_sub(1),
                _ => {}
            }
            let semi = matches!(t.kind, TokenKind::Semi) && depth == 0;
            self.bump();
            first = false;
            if semi {
                return;
            }
        }
    }

    /// One iteration of the function-body loop — the former loop body,
    /// factored out so the recovery wrapper owns the seam.
    fn fn_body_item(
        &mut self,
        nested_names: &mut HashSet<String>,
        seen_labels: &mut HashSet<u32>,
        cp: Option<Checkpoint>,
    ) -> Result<Step, CompileError> {
        {
            if matches!(self.peek().kind, TokenKind::Eof) {
                return Err(Self::expected(
                    self.peek(),
                    "`}` to close the function body",
                ));
            }
            // Doc/attention run (docs/pmt/language.md (doc lines)): a `?`/`!`
            // line at body item position starts a run that must bind to
            // the NEXT nested function definition — anything else next
            // (a statement, the closing `}`, `export` before a nested
            // def) is `DanglingDocRun` at the run's own first line.
            if matches!(
                self.peek().kind,
                TokenKind::DocLine(_) | TokenKind::AttentionLine(_)
            ) {
                self.g_flush_start(PmcKind::DocRun);
                let first_span = self.doc_run()?;
                self.g_finish();
                if !self.next_is_nested_function_start() {
                    return Err(CompileError {
                        span: first_span,
                        kind: CompileErrorKind::DanglingDocRun,
                    });
                }
            }
            // Nested definition: `[volatile] IDENT ( ) {` — visibility-only
            // nesting. A leading `volatile` is syntactically part of the
            // same shape but always illegal here — nesting can never be
            // the top-level `main` — so it is rejected immediately, the
            // same way `NestedExport` below rejects a leading `export`
            // without parsing the rest of the definition first.
            let is_nested_def = self.next_is_nested_function_start();
            if is_nested_def {
                if self.peek_is_volatile_modifier() {
                    let tok = self.peek().clone();
                    // The shape `next_is_nested_function_start` just
                    // confirmed guarantees an identifier right after
                    // `volatile` here.
                    let TokenKind::Ident(name) = &self.tokens[self.pos + 1].kind else {
                        unreachable!("next_is_nested_function_start confirmed an identifier here");
                    };
                    return Err(Self::err_at(
                        &tok,
                        CompileErrorKind::VolatileNotOnMain(name.clone()),
                    ));
                }
                self.g_start_at(cp, PmcKind::Function);
                let child = self.function()?;
                if nested_names.contains(&child.name) {
                    return Err(CompileError {
                        span: mtc_core::diagnostics::Span::point(child.line, child.col),
                        kind: CompileErrorKind::DuplicateName {
                            name: child.name.clone(),
                            what: "function",
                        },
                    });
                }
                nested_names.insert(child.name);
                return Ok(Step::Continue);
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
            let mut labels = Vec::new();
            loop {
                let tok = self.peek().clone();
                let TokenKind::Number(n, written) = &tok.kind else {
                    break;
                };
                let (n, written) = (*n, written.clone());
                self.g_flush_start(PmcKind::Label);
                self.bump();
                let colon = self.peek().clone();
                self.expect(&TokenKind::Colon, "`:` after a label number")?;
                self.g_finish();
                if !seen_labels.insert(n) {
                    return Err(Self::err_at(&tok, CompileErrorKind::DuplicateLabel(n)));
                }
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
                self.bump();
                // FUNCTION was retro-opened by the caller (top level:
                // `top_items`; nested: the `g_start_at` above) at a
                // checkpoint taken before this call — closing it here,
                // right after the `}` bump, is the one shared exit for
                // both call sites.
                self.g_finish();
                return Ok(Step::Done);
            }
            self.g_start_at(cp, PmcKind::Statement);
            self.statement()?;
            self.g_finish();
        }
        Ok(Step::Continue)
    }

    /// One labeled statement: a comma group of items and its closing
    /// `;`. The labels are the CALLER's — it announces them, checks them
    /// for duplicates and for dangling, and brackets the green STATEMENT
    /// node around this walk. The items are collected here only so the
    /// comma-group position rules below can look at the one that
    /// precedes each `,` (docs/pmt/language.md (comma groups)); nothing
    /// else reads them.
    fn statement(&mut self) -> Result<(), CompileError> {
        self.g_flush_start(PmcKind::Item);
        let first_item = self.item(false)?;
        self.g_finish();
        let mut items = vec![first_item];
        while matches!(self.peek().kind, TokenKind::Comma) {
            let comma = self.peek().clone();
            // Whatever precedes a `,` must be bare (docs/pmt/language.md).
            match items.last().expect("items is never empty") {
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
            // The comma just bumped above stays outside both ITEM nodes,
            // at STATEMENT level — `g_flush_start` opens ITEM only now,
            // at this item's own first token.
            self.g_flush_start(PmcKind::Item);
            let item = self.item(true)?;
            self.g_finish();
            items.push(item);
        }
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(())
    }

    /// One statement item. `in_group` selects the comma-group grammar
    /// path (docs/pmt/language.md (comma groups)): inside a group,
    /// `goto` is illegal and a successor may only be the trailing item.
    ///
    /// Reached from two places. The statement production passes its own
    /// group position. [`reparse_item`] — the retokenization reuse shim
    /// extraction calls — must be told: a green `ITEM` node retokenized
    /// on its own carries no record of the group it came from, so its
    /// caller in `crate::syntax::extract` recovers the flag from the
    /// node's position among its siblings. Any new branch on `in_group`
    /// here is therefore a change to extraction's contract too.
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
                    self.g_flush_start(PmcKind::CheckArm);
                    let (marked, marked_span, marked_written) = self.check_arm()?;
                    self.g_finish();
                    self.expect(&TokenKind::Comma, "`,` between check arms")?;
                    self.g_flush_start(PmcKind::CheckArm);
                    let (blank, blank_span, blank_written) = self.check_arm()?;
                    self.g_finish();
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
                            // docs/pmt/language.md: parens on a builtin, if
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

/// Retokenization reuse shim (`crate::syntax::extract::sig_tokens`'s
/// counterpart): re-parse one already-extracted `.pmc` item from a green
/// tree's own retokenized `ITEM` node through the SAME production the
/// original parse used, so an extraction and the original parse can
/// never disagree on what an item means. Lives here, not in
/// `crate::syntax::extract`, so it can build a bare `Parser` and reach
/// the private [`Parser::item`] production directly. `in_group` selects
/// the same grammar path `Parser::item` already branches on
/// (docs/pmt/language.md: `goto` is illegal, and a non-trailing
/// successor is illegal, only INSIDE a comma group) — the caller
/// supplies it because a lone `ITEM` node carries no memory of its own
/// former comma-group position. `expect`s on error: extraction only
/// ever runs on a tree that already parsed once, so a failure here is a
/// bug in the retokenization, not a malformed program.
pub(crate) fn reparse_item(tokens: &[Token], in_group: bool) -> Item {
    Parser {
        tokens,
        pos: 0,
        namespaces: HashSet::new(),
        declared_fns: HashSet::new(),
        sink: None,
        recovered: None,
    }
    .item(in_group)
    .expect("reparse_item: extraction only ever runs on an already-parsed tree")
}

/// Retokenization reuse shim for a `DOC_RUN` node's own `DocLine`/
/// `AttentionLine` tokens: converts each into a [`DocRunItem`]. This is
/// the ONE route a run reaches [`reduce_doc_run`] by —
/// [`Parser::doc_run`] walks the same lines during the parse but keeps
/// none of them, so the items are rebuilt here off the emitted node.
/// The run-binding logic stays where it was: the `DocLineOrder`
/// ordering check, the duplicate-`[deprecated]` check, and the "what
/// follows must be a declaration" rule are all properties of the
/// ORIGINAL parse, validated once by `doc_run` and its caller; an
/// isolated retokenized `DOC_RUN` slice has nothing left to validate,
/// only to convert. Attention attributes are still decoded through
/// [`Parser::parse_attr`], the exact helper `doc_run` itself calls, so
/// `[deprecated]` decodes identically either way. `blank_before` counts
/// from a fresh `prev_end_line` of 0 — the isolated slice's own start,
/// not the original file position (nothing downstream reads
/// `blank_before` off a reduced [`FnDoc`], so this is cosmetic
/// fidelity, not load-bearing).
///
/// **Interleaved comments are dropped, not reproduced.** `tokens` comes
/// from `crate::syntax::extract::sig_tokens`, which filters comments as
/// trivia before this ever runs — so a `DOC_RUN` with an interior
/// `//`/`/* */` comment yields a sequence strictly SHORTER than the
/// run as WRITTEN (no `DocRunKind::Comment` item ever appears here).
/// This is behavior-preserving where it matters: [`reduce_doc_run`]
/// treats every `DocRunKind::Comment` as fully inert
/// (`DocRunKind::Comment(_) => {}` — no paragraph split, no join, no
/// attention/`deprecated` effect, regardless of position), so a
/// comment-dropped run and a hand-built comment-ful one reduce to the
/// IDENTICAL [`FnDoc`] even though the two raw `Vec<DocRunItem>`s
/// differ. A caller wanting raw item-for-item parity (spans, `Comment`
/// entries included) has no such guarantee — only [`reduce_doc_run`]
/// equality holds.
pub(crate) fn reparse_doc_items(tokens: &[Token]) -> Vec<DocRunItem> {
    let mut p = Parser {
        tokens,
        pos: 0,
        namespaces: HashSet::new(),
        declared_fns: HashSet::new(),
        sink: None,
        recovered: None,
    };
    let mut items = Vec::new();
    // The isolated slice's own start, not the original file position.
    let mut prev_end_line = 0;
    loop {
        let t = p.peek().clone();
        match &t.kind {
            TokenKind::DocLine(text) => {
                let text = text.clone();
                p.bump();
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
                p.bump();
                let attr = Parser::parse_attr(&text, &t);
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
            TokenKind::Eof => break,
            _ => unreachable!(
                "reparse_doc_items: extraction only ever feeds a DOC_RUN's own tokens \
                 (DocLine/AttentionLine) plus the synthetic trailing Eof"
            ),
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CompileErrorKind;
    use crate::lexer::lex;

    /// The crate's one parse entry point: source in, `Program` out, with
    /// the `WithComments` lex it needs done internally. Pinned here
    /// because every other test module in the crate calls it and none of
    /// them assert its contract.
    #[test]
    fn parse_takes_source_and_lexes_with_comments_itself() {
        // The comment sits BETWEEN two significant tokens and on a line
        // it does not lengthen, so every span below it is unmoved: the
        // two programs must be equal field for field. A comment on its
        // own line would shift every following line and the comparison
        // would fail for a reason that has nothing to do with trivia —
        // measured, not assumed.
        let src = "main() { /* c */\n    right;\n}\n";
        let bare = "main() {\n    right;\n}\n";
        let p = parse(src).expect("parses");
        assert_eq!(p.functions.len(), 1);
        assert_eq!(p.functions[0].name, "main");
        assert_eq!(p.functions, parse(bare).expect("parses").functions);
    }

    /// A lex failure surfaces as the lexer's own error, unchanged by the
    /// mode `parse` picks internally.
    #[test]
    fn parse_reports_the_lexers_own_error() {
        let src = "/* never closed\nmain() { right; }\n";
        assert_eq!(
            parse(src).map(|_| ()).unwrap_err(),
            crate::lexer::lex(src).map(|_| ()).unwrap_err()
        );
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
        let p = parse(src).unwrap();
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
        let p = parse("f() { 1: right, right, mark(5); 5: left, check(1, !); }").unwrap();
        assert_eq!(p.functions[0].body[0].items.len(), 3);
        assert_eq!(p.functions[0].body[1].items.len(), 2);

        let e = parse("f() { left(1), left(2); 1: mark; 2: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("successor")));

        let e = parse("f() { check(1, 2), left; 1: mark; 2: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("check")));

        let e = parse("f() { halt, left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("halt")));

        let e = parse("f() { goto 1, left; 1: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("goto")));
        let e = parse("f() { left, goto 1; 1: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("goto")));
    }

    #[test]
    fn reserved_and_at_rules() {
        // At top level a reserved-word ident is now a `TopLevelStatement`
        // (docs/pmt/language.md) — the naming check runs only once a keyword
        // has consumed the leading token (e.g. `export <reserved>()`).
        let e = parse("check() { }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::TopLevelStatement(ref n) if n.contains("check"))
        );
        // `export` isn't reserved, so it slips past the top-level guard;
        // `function()` itself then sees the reserved name.
        let e = parse("export check() { }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::ReservedName { ref name, what } if name == "check" && what == "function")
        );

        let e = parse("f() { @left(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::BuiltinCalled(n) if n == "left"));

        let e = parse("f() { flip; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownCommand(n) if n == "flip"));

        // A user function called without `@` is the same error (docs/pmt/language.md).
        let e = parse("f() { goToEnd(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownCommand(n) if n == "goToEnd"));
    }

    #[test]
    fn empty_builtin_parens_are_a_syntax_error() {
        // docs/pmt/language.md: `()` on a tape builtin, if written, must carry
        // a successor — empty parens are no longer fall-through sugar.
        for name in ["left", "right", "mark", "unmark"] {
            let e = parse(&format!("f() {{ {name}(); }}")).unwrap_err();
            assert!(
                matches!(e.kind, CompileErrorKind::EmptyBuiltinParens { name: ref n } if n == name),
                "{name}(): got {:?}",
                e.kind
            );
        }

        // Bare, and both successor forms, stay legal.
        assert!(parse("f() { left; }").is_ok());
        assert!(parse("f() { left(5); }").is_ok());
        assert!(parse("f() { left(!); }").is_ok());

        // Scope limit: user calls keep mandatory-but-emptyable parens.
        assert!(parse("f() { @f(); }").is_ok());
    }

    #[test]
    fn goto_bang_is_a_dedicated_error() {
        let e = parse("f() { goto !; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GotoReturn));
    }

    #[test]
    fn duplicate_and_dangling_diagnostics() {
        let e = parse("f() { } f() { }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "f" && what == "function")
        );

        let e = parse("f() { 1: left; 1: right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateLabel(1)));

        let e = parse("f() { left; 2: }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DanglingLabel(2)));
    }

    #[test]
    fn empty_function_and_stacked_labels() {
        let p = parse("f() { }").unwrap();
        assert!(p.functions[0].body.is_empty());

        let p = parse("f() { 1: 2: left; }").unwrap();
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
        let p = parse("идиВКонец() { right(!); } main() { @идиВКонец(); }").unwrap();
        assert_eq!(p.functions[0].name, "идиВКонец");
        match &p.functions[1].body[0].items[0] {
            Item::Call { name, .. } => assert_eq!(name, "идиВКонец"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn export_is_contextual_and_main_auto_exports() {
        let p = parse("export api() { left; } helper() { right; } main() { mark; }").unwrap();
        assert!(p.functions[0].exported);
        assert!(!p.functions[1].exported);
        assert!(p.functions[2].exported); // main
        let p = parse("export() { left; } main() { @export(); }").unwrap();
        assert_eq!(p.functions[0].name, "export"); // a function NAMED export
    }

    #[test]
    fn nested_definitions_parse_recursively() {
        let p = parse("main() { walk() { step() { right; } @step(); } @walk(); }").unwrap();
        let main = &p.functions[0];
        assert_eq!(main.nested.len(), 1);
        assert_eq!(main.nested[0].name, "walk");
        assert_eq!(main.nested[0].nested[0].name, "step");
    }

    #[test]
    fn namespace_blocks_stamp_paths_and_nest() {
        let p = parse("namespace a { f() { left; } namespace b { g() { right; } } } h() { mark; }")
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
        let p = parse("namespace() { left; } main() { @namespace(); }").unwrap();
        assert_eq!(p.functions[0].name, "namespace");
    }

    #[test]
    fn import_paths_aliases_and_scopes_parse() {
        let p = parse("use a, std::b as c; namespace ns { use d::e; }").unwrap();
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
        let p = parse("main() { @std::api::run(); }").unwrap();
        match &p.functions[0].body[0].items[0] {
            Item::Call { name, .. } => assert_eq!(name, "std::api::run"),
            other => panic!("unexpected {other:?}"),
        }
        let e = parse("main() { @std::(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Expected { what, .. } if what.contains("::")));
    }

    #[test]
    fn namespace_name_pool_and_reopening_rules() {
        // Reopening the same namespace is legal (scopes merge by path).
        assert!(parse("namespace a { f() { left; } } namespace a { g() { right; } }").is_ok());
        // Same (path, name) across reopened blocks is a duplicate.
        let e = parse("namespace a { f() { left; } } namespace a { f() { right; } }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "f" && what == "function")
        );
        // The same bare name in different namespaces is legal.
        assert!(parse("namespace a { f() { left; } } namespace b { f() { right; } }").is_ok());
        // Namespace and function names share one pool per scope.
        let e = parse("namespace a { } a() { left; }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "a" && what == "namespace")
        );
        let e = parse("a() { left; } namespace a { }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "a" && what == "function")
        );
        // An unclosed block is an error, not silent Eof acceptance.
        let e = parse("namespace a { f() { left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Expected { .. }));
    }

    #[test]
    fn use_stays_illegal_inside_function_bodies() {
        let e = parse("main() { use go; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::KeywordInBody(kw) if kw == "use"));
    }

    #[test]
    fn nested_export_and_same_scope_duplicates_error() {
        let e = parse("main() { export inner() { left; } }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::NestedExport));
        let e = parse("main() { f() { left; } f() { right; } }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::DuplicateName { ref name, what } if name == "f" && what == "function")
        );
    }

    #[test]
    fn spans_are_retained_for_labels_names_and_items() {
        let p = parse("f() {\n  5 : right(7);\n7:  left;\n}").unwrap();
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
        let p = parse("f() { @a::b(); check(1, !); 1: left; }").unwrap();
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

    /// Character-precise reference spans: exact `Span::new(...)` values
    /// against the fixture's actual layout —
    /// `f() { 1: right(2); check(1, !); goto 1; left, mark(3); }`.
    #[test]
    fn reference_spans_on_goto_check_and_builtin_successors() {
        let p = parse("f() { 1: right(2); check(1, !); goto 1; left, mark(3); }").unwrap();
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
        let p = parse("f() { right; }").unwrap();
        let Item::Builtin {
            succ_label_span, ..
        } = &p.functions[0].body[0].items[0]
        else {
            panic!("expected builtin");
        };
        assert!(succ_label_span.is_none());

        // Parenthesised but a `!` (return) successor, not a label.
        let p = parse("f() { right(!); }").unwrap();
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
        let p = parse("main() { @g(7); }").unwrap();
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

    /// A green FUNCTION/NAMESPACE node's own `text_range()`, converted
    /// to a `line`/`col` `Span` via `TextLineIndex`. With no bound doc
    /// run this opens at the declaration's first significant token:
    /// `top_items`'s green checkpoint (`fn_cp`) is taken before
    /// `volatile`/`export`/the name token (docs/core.md (syntax
    /// trees)). A bound doc run retro-wraps in front of that, which is
    /// why the LSP trims the extent back down (`lsp::function_extent`).
    fn extent(src: &str) -> Span {
        use mtc_core::syntax::{AstNode, TextLineIndex};

        let index = TextLineIndex::new(src);
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let file = syntax::FileView::cast(root).expect("root is FILE");
        match file.items().next().expect("one top-level item") {
            syntax::TopView::Function(f) => index.span(f.syntax().text_range()),
            syntax::TopView::Namespace(ns) => index.span(ns.syntax().text_range()),
            syntax::TopView::Use(_) => panic!("expected a function or namespace item"),
        }
    }

    #[test]
    fn function_and_namespace_extent_spans() {
        // Two-line function: name token start → closing `}` end.
        assert_eq!(extent("f() {\n    left;\n}\n"), Span::new(1, 1, 3, 2));

        // Namespace block: `namespace` keyword start → closing `}` end.
        assert_eq!(
            extent("namespace ns {\n    f() { left; }\n}\n"),
            Span::new(1, 1, 3, 2)
        );

        // A leading `export` is consumed before the green FUNCTION node's
        // own checkpoint, but the checkpoint is taken ahead of it, so the
        // node's extent still starts at `export` — the header's true
        // first token (the name token's own line/col stay name-anchored;
        // only the extent reaches back). Pinned explicitly since it's the
        // one place the "header first token" reading is load-bearing.
        assert_eq!(
            extent("export f() {\n    left;\n}\n"),
            Span::new(1, 1, 3, 2) // starts at "export", not "f"
        );
        // The bare (non-exported) case above ("Two-line function") already
        // pins the name-token-anchored start for the un-exported path —
        // together the two assertions in this test cover both cases.
    }

    /// The green-tree counterpart to the `volatile`/`export` parser tests
    /// below: a FUNCTION node's extent starts at `volatile` (not `export`,
    /// not the name) when a leading `volatile` was written, mirroring how
    /// `function_and_namespace_extent_spans` pins `export`'s own extent
    /// start — and `FnHeader::has_volatile` records the token losslessly,
    /// the same way `has_export` does, for the formatter to read.
    #[test]
    fn volatile_starts_the_function_extent_and_is_recorded_on_the_header() {
        use mtc_core::syntax::AstNode;

        let src = "volatile main() {\n    mark;\n}\n";
        assert_eq!(extent(src), Span::new(1, 1, 3, 2)); // starts at "volatile", not "main"
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let file = syntax::FileView::cast(root).expect("root is FILE");
        let syntax::TopView::Function(f) = file.items().next().expect("one item") else {
            panic!("expected a function item");
        };
        let header = f.header();
        assert!(header.has_volatile);
        assert!(!header.has_export);

        // Fixed order: `volatile` still wins the extent start over
        // `export` when both are written.
        let src = "volatile export main() {\n    mark;\n}\n";
        assert_eq!(extent(src), Span::new(1, 1, 3, 2)); // starts at "volatile", not "export"
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let file = syntax::FileView::cast(root).expect("root is FILE");
        let syntax::TopView::Function(f) = file.items().next().expect("one item") else {
            panic!("expected a function item");
        };
        let header = f.header();
        assert!(header.has_volatile);
        assert!(header.has_export);
    }

    #[test]
    fn import_spans_exclude_the_alias() {
        let p = parse("use std::go as g;\nmain() { @g(); }").unwrap();
        let imp = &p.imports[0];
        assert_eq!((imp.span.start.col, imp.span.end.col), (5, 12)); // "std::go"
    }

    fn err_msg(src: &str) -> String {
        parse(src).unwrap_err().to_string()
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
        assert!(parse("main() { 1 : right; }").is_ok());
        assert!(parse("main() { 1: 2: right; }").is_ok());
        assert!(parse("use std :: goToEnd;\nmain() { @goToEnd(); }").is_ok());
    }

    #[test]
    fn empty_builtin_parens_message_names_the_builtin_and_the_fix() {
        let m = err_msg("main() { mark(); }");
        assert!(m.contains("`mark`"), "got: {m}");
        assert!(m.contains("successor"), "got: {m}");
        // Calls are unaffected: `@f()` stays legal, no error at all.
        assert!(parse("f() { } main() { @f(); }").is_ok());
    }

    // -- Doc/attention runs (docs/pmt/language.md (doc lines)) ----------------
    //
    // Grammar-fixed run order (`?` block, then `!` block), attachment to
    // the next declaration at the run's own scope, and the two
    // attention-line attribute checks.

    /// Retokenizes `src`'s `?`/`!` lines the way `syntax::extract::sig_tokens`
    /// feeds a bound `DOC_RUN` into [`reparse_doc_items`] — every other
    /// token (comments included) stripped, a synthetic `Eof` appended.
    /// Comment mode makes no difference to `DocLine`/`AttentionLine`
    /// tokens (only `TokenKind::Comment` is mode-gated), so plain `lex`
    /// already strips `//`/`/* */` trivia the same `reparse_doc_items`
    /// precondition requires — matching why its own doc calls an
    /// interleaved comment "dropped, not reproduced" by this route.
    fn doc_run_items(src: &str) -> Vec<DocRunItem> {
        let mut tokens: Vec<Token> = lex(src)
            .unwrap()
            .into_iter()
            .filter(|t| matches!(t.kind, TokenKind::DocLine(_) | TokenKind::AttentionLine(_)))
            .collect();
        tokens.push(Token {
            kind: TokenKind::Eof,
            line: 0,
            col: 0,
            len: 0,
        });
        reparse_doc_items(&tokens)
    }

    #[test]
    fn doc_run_collects_a_docs_only_run() {
        let doc_run = doc_run_items("? line one\n? line two\nmain() { right; }");
        assert_eq!(doc_run.len(), 2);
        let DocRunKind::Doc { text, .. } = &doc_run[0].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, "line one");
        assert!(!doc_run[0].blank_before);
        let DocRunKind::Doc { text, .. } = &doc_run[1].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, "line two");
        assert!(!doc_run[1].blank_before);
    }

    #[test]
    fn doc_run_collects_an_attention_only_run() {
        let doc_run = doc_run_items(
            "! bare prose line\n! [deprecated] use goToStart instead\nmain() { right; }",
        );
        assert_eq!(doc_run.len(), 2);
        let DocRunKind::Attention { attr, text, .. } = &doc_run[0].kind else {
            panic!("expected an attention line");
        };
        assert!(attr.is_none());
        assert_eq!(text, "bare prose line");
        let DocRunKind::Attention { attr, text, .. } = &doc_run[1].kind else {
            panic!("expected an attention line");
        };
        assert_eq!(attr.as_ref().expect("has an attribute").name, "deprecated");
        assert_eq!(text, "[deprecated] use goToStart instead");
    }

    #[test]
    fn doc_run_collects_docs_then_attention_in_order() {
        let doc_run = doc_run_items("? doc line\n! [deprecated] msg\nexport helper() { right; }");
        assert_eq!(doc_run.len(), 2);
        assert!(matches!(doc_run[0].kind, DocRunKind::Doc { .. }));
        assert!(matches!(doc_run[1].kind, DocRunKind::Attention { .. }));
    }

    #[test]
    fn doc_run_binds_to_a_nested_function_at_its_own_indent() {
        // Indentation before both the sigil and the nested function's
        // name — the run still lexes/attaches correctly (design doc:
        // "runs sit at the bound declaration's own indent"). Read through
        // `parse` (the binding decision itself is the parser's, not
        // `reparse_doc_items`'s — that helper only reduces an already-
        // bound run's own items).
        let prog =
            parse("main() {\n    ? step one\n    step() { right; }\n    @step();\n}").unwrap();
        let main = &prog.functions[0];
        assert!(main.doc.is_none(), "the run binds to `step`, not `main`");
        let step = &main.nested[0];
        assert_eq!(step.name, "step");
        let doc = step.doc.as_ref().expect("documented");
        assert_eq!(doc.paragraphs, vec!["step one"]);
    }

    /// `reparse_doc_items`'s `blank_before` is computed purely from the
    /// line-number GAP between the doc run's own SURVIVING tokens
    /// (`DocLine`/`AttentionLine` — comments are stripped before this
    /// ever runs, per `reparse_doc_items`'s own doc: "dropped, not
    /// reproduced"). Originally named as though it distinguished "a real
    /// blank line" from "a comment sitting in the gap"; renamed after
    /// measuring that it CANNOT — a probe fixture with a comment-only
    /// gap and no literal blank line (`"? first\n// mid comment\n?
    /// second\n..."`) read `blank_before: true` on the second `Doc`
    /// line, identically to the real-blank case below, because both
    /// leave the same >1 line-number gap between the two surviving
    /// `DocLine` tokens once the comment is stripped out. Matches
    /// `reparse_doc_items`'s own doc ("cosmetic fidelity, not
    /// load-bearing" — nothing downstream reads `blank_before` off a
    /// reduced `FnDoc`). Comment inertness in the REDUCED `FnDoc` —
    /// a comment contributing nothing and never splitting a paragraph —
    /// is pinned separately, directly against `reduce_doc_run`, by
    /// `fn_doc_comment_items_in_the_run_contribute_nothing_and_never_split_a_paragraph`.
    #[test]
    fn doc_run_items_blank_before_tracks_a_token_line_gap_not_a_real_blank_line() {
        let src = "\
? first
// mid comment

? second

// trailing comment before fn
main() { right; }
";
        let doc_run = doc_run_items(src);
        assert_eq!(doc_run.len(), 2);
        let DocRunKind::Doc { text, .. } = &doc_run[0].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, "first");
        assert!(!doc_run[0].blank_before);
        let DocRunKind::Doc { text, .. } = &doc_run[1].kind else {
            panic!("expected the second doc line");
        };
        assert_eq!(text, "second");
        assert!(doc_run[1].blank_before, "a blank line precedes it");

        // Same shape, but the gap is a lone comment with NO literal
        // blank line — reads `true` too, the same as the real-blank
        // case above: the mechanism cannot tell them apart.
        let src_no_blank = "? first\n// mid comment\n? second\nmain() { right; }\n";
        let doc_run = doc_run_items(src_no_blank);
        assert_eq!(doc_run.len(), 2);
        assert!(
            doc_run[1].blank_before,
            "a comment-only gap reads as blank too, with no literal blank line present"
        );
    }

    #[test]
    fn doc_run_before_a_nested_function_amid_sibling_statements() {
        // Read through `parse`, same reasoning as
        // `doc_run_binds_to_a_nested_function_at_its_own_indent`: the AST
        // hoists `helper` out of body order, so `main.body` holds the
        // THREE statements (`left; @helper(); left;`) and `main.nested`
        // the one documented nested function, separately.
        let prog = parse(
            "main() {\n    left;\n    ? helper doc\n    helper() { right; }\n    @helper();\n    left;\n}",
        )
        .unwrap();
        let main = &prog.functions[0];
        assert_eq!(main.body.len(), 3);
        assert_eq!(main.nested.len(), 1);
        let helper = &main.nested[0];
        assert_eq!(helper.name, "helper");
        let doc = helper.doc.as_ref().expect("documented");
        assert_eq!(doc.paragraphs, vec!["helper doc"]);
    }

    /// The `doc_run` → `FnDoc` reduction is the ONLY thing a doc run
    /// changes about the lowered `Program`. Both sides here run `parse`,
    /// so this is not a claim about which recipe `parse` uses; it
    /// isolates one question — "does the reduction leak anything ELSE
    /// into the rest of the AST?" — by stripping `doc` back off the
    /// documented function and requiring the two programs to match
    /// exactly (the twin is padded with blank lines so `main`'s own
    /// line/col line up too).
    #[test]
    fn documented_function_lowers_to_its_undocumented_twin_plus_a_doc() {
        let doc = parse("? doc\n! [deprecated] msg\nmain() { right; }").unwrap();
        let bare = parse("\n\nmain() { right; }").unwrap();
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
        // doc-run-item layer too, plus verbatim internal spacing in an
        // attention line's full payload — no extra normalization happens
        // here.
        let src = "?text\n?  text\n! [deprecated] msg with  double  spaces\nmain() { right; }";
        let doc_run = doc_run_items(src);
        assert_eq!(
            doc_run.clone(),
            doc_run,
            "lossless round-trip: clone() == self"
        );

        let DocRunKind::Doc { text, .. } = &doc_run[0].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, "text");
        let DocRunKind::Doc { text, .. } = &doc_run[1].kind else {
            panic!("expected a doc line");
        };
        assert_eq!(text, " text"); // one space consumed, one remains
        let DocRunKind::Attention { attr, text, .. } = &doc_run[2].kind else {
            panic!("expected an attention line");
        };
        assert_eq!(attr.as_ref().expect("has an attribute").name, "deprecated");
        assert_eq!(text, "[deprecated] msg with  double  spaces");
    }

    #[test]
    fn doc_line_order_rejects_interleave_and_wrong_order() {
        let e = parse("? doc\n! attn\n? doc2\nmain() { right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DocLineOrder));
        assert_eq!(e.kind.code(), "doc-line-order");
        assert_eq!((e.span.start.line, e.span.start.col), (3, 1));

        let e = parse("! attn only\n? doc after\nmain() { right; }").unwrap_err();
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
            let e = parse(src).unwrap_err();
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
            let e = parse(src).unwrap_err();
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
        let e = parse("! [depercated] old api\nmain() { right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownAttribute(ref n) if n == "depercated"));
        assert_eq!(e.kind.code(), "unknown-attribute");
        assert_eq!((e.span.start.line, e.span.start.col), (1, 4));
    }

    /// `parse_attr`'s column math is char-counted throughout
    /// (`Token::len`, `text.chars().count()`),
    /// never byte-counted — a non-ASCII payload AFTER the attribute
    /// (`café`, where `é` is one `char` but two UTF-8 bytes) must not
    /// perturb the attribute name's own span, since nothing about
    /// `[xx]`'s position depends on what follows it.
    #[test]
    fn unknown_attribute_span_is_char_counted_past_a_non_ascii_payload() {
        let e = parse("! [xx] café\nmain() { right; }").unwrap_err();
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
        let e =
            parse("! [deprecated] first\n! [deprecated] second\nmain() { right; }").unwrap_err();
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
        let prog = parse("? line one\n? line two\n?\n? second para\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.paragraphs, vec!["line one line two", "second para"]);
        assert!(doc.attention.is_empty());
        assert_eq!(doc.deprecated, None);
    }

    #[test]
    fn fn_doc_leading_and_trailing_empty_doc_lines_produce_no_empty_paragraphs() {
        let prog = parse("?\n?\n? doc\n?\n?\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.paragraphs, vec!["doc"]);
    }

    #[test]
    fn fn_doc_run_of_only_empty_lines_yields_content_empty_doc() {
        // A lone empty `?` line and a lone empty `!` line, together: the
        // `?` side already drops an empty doc line without emitting an
        // empty paragraph; this pins the mirrored rule for `!` — an empty
        // attention line contributes nothing either, so a run built from
        // nothing but blanks reduces to a content-empty `FnDoc` (no
        // paragraphs, no attention, no deprecation) rather than an
        // `attention: [""]` entry that would otherwise render as a
        // note-only hover popup with nothing in it.
        let prog = parse("?\n!\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("run is non-empty");
        assert!(doc.paragraphs.is_empty());
        assert!(doc.attention.is_empty());
        assert_eq!(doc.deprecated, None);
    }

    #[test]
    fn fn_doc_attention_prose_is_captured_verbatim_in_order() {
        let prog = parse("! first note\n! second note\nmain() { right; }").unwrap();
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
        let src = "! see [deprecated] docs\nmain() { right; }";
        let doc_run = doc_run_items(src);
        let DocRunKind::Attention { attr, .. } = &doc_run[0].kind else {
            panic!("expected an attention line");
        };
        assert!(attr.is_none(), "bracket mid-prose is not an attribute");

        let prog = parse(src).unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.attention, vec!["see [deprecated] docs"]);
        assert_eq!(doc.deprecated, None);
    }

    #[test]
    fn fn_doc_deprecated_message_captured_with_and_without_a_message() {
        let prog = parse("! [deprecated] use goToStart instead\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.deprecated, Some("use goToStart instead".to_string()));
        assert!(doc.attention.is_empty());

        let prog = parse("! [deprecated]\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.deprecated, Some(String::new()));
    }

    #[test]
    fn fn_doc_deprecated_line_is_excluded_from_attention_while_bare_prose_survives() {
        let prog =
            parse("! note one\n! [deprecated] use bar instead\n! note two\nmain() { right; }")
                .unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.attention, vec!["note one", "note two"]);
        assert_eq!(doc.deprecated, Some("use bar instead".to_string()));
    }

    /// Builds the `Vec<DocRunItem>` by hand — `parse(src)` cannot pin
    /// this: the production path into `reduce_doc_run` is
    /// `extract_function`'s `reduce_doc_run(&reparse_doc_items(&tokens))`
    /// (`crate::syntax::extract`), where `tokens` comes from
    /// `sig_tokens`, which strips comment tokens unconditionally — by the
    /// time `reduce_doc_run` runs on that path, no `DocRunKind::Comment`
    /// item has ever existed to feed it, so a `parse(src)` round trip
    /// through a comment-bearing source proves nothing about the
    /// `DocRunKind::Comment(_) => {}` arm below. Only a hand-built run
    /// can exercise that arm directly.
    ///
    /// Proven to discriminate: changed `DocRunKind::Comment(_) => {}` to
    /// `DocRunKind::Comment(_) => paragraphs.push("BOGUS".to_string())`
    /// and confirmed this test — and only this one among the doc-run/
    /// fn-doc suite — failed
    /// (`paragraphs: ["first", "BOGUS", "second"]` vs. the expected
    /// `["first second"]`); reverted, confirmed green again.
    #[test]
    fn fn_doc_comment_items_in_the_run_contribute_nothing_and_never_split_a_paragraph() {
        use crate::lexer::CommentKind;

        let dummy_span = Span::new(1, 1, 1, 1);
        let doc_run = vec![
            DocRunItem {
                blank_before: false,
                kind: DocRunKind::Doc {
                    text: "first".to_string(),
                    span: dummy_span,
                },
            },
            DocRunItem {
                blank_before: false,
                kind: DocRunKind::Comment(Comment {
                    text: "// mid comment".to_string(),
                    kind: CommentKind::Line,
                    own_line: true,
                }),
            },
            DocRunItem {
                blank_before: false,
                kind: DocRunKind::Doc {
                    text: "second".to_string(),
                    span: dummy_span,
                },
            },
        ];
        let doc = reduce_doc_run(&doc_run).expect("documented");
        assert_eq!(doc.paragraphs, vec!["first second"]);
    }

    #[test]
    fn undocumented_function_has_no_doc() {
        let prog = parse("main() { right; }").unwrap();
        assert_eq!(prog.functions[0].doc, None);
    }

    // WARM-UP pins (T3 review carry-overs, Task 4): `reduce_doc_run`
    // edge shapes not exercised by any T3 test.

    /// A run made of nothing but empty `?` lines is still non-empty at
    /// the CST level (bare sigils were written), so `Function.doc` is
    /// `Some` — but every `FnDoc` field is empty, since an empty `?`
    /// line only ever closes a paragraph, never starts one.
    #[test]
    fn fn_doc_a_run_of_only_empty_doc_lines_is_some_with_every_field_empty() {
        let prog = parse("?\n?\nmain() { right; }").unwrap();
        assert_eq!(
            prog.functions[0].doc,
            Some(FnDoc {
                paragraphs: vec![],
                attention: vec![],
                deprecated: None,
            })
        );
    }

    /// TWO consecutive empty `?` lines between paragraphs collapse the
    /// same as one: the paragraph-flush check guards on `current` being
    /// non-empty, so the second empty line finds nothing left to flush
    /// and is a no-op — no stray empty paragraph appears between "a" and
    /// "b".
    #[test]
    fn fn_doc_a_double_empty_separator_still_yields_exactly_two_paragraphs() {
        let prog = parse("? a\n?\n?\n? b\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.paragraphs, vec!["a", "b"]);
    }

    /// Whitespace between `[deprecated]`'s closing `]` and its message is
    /// trimmed, not preserved verbatim: `reduce_doc_run` slices past `]`
    /// and calls `.trim()`, so two (or more) interior spaces collapse
    /// away just like a single one would.
    #[test]
    fn fn_doc_deprecated_message_trims_whitespace_after_the_attribute() {
        let prog = parse("! [deprecated]  two spaces\nmain() { right; }").unwrap();
        let doc = prog.functions[0].doc.as_ref().expect("documented");
        assert_eq!(doc.deprecated, Some("two spaces".to_string()));
    }

    // `volatile` (0.4): a contextual keyword-plus-reservation. Legal
    // only as the leading modifier of the un-namespaced top-level
    // `main`; a reserved word
    // everywhere it would otherwise name a function or namespace.

    #[test]
    fn volatile_main_parses_and_sets_the_flag() {
        let p = parse("volatile main() { mark; }").unwrap();
        assert!(p.functions[0].volatile);
        assert!(p.functions[0].exported);
    }

    #[test]
    fn volatile_export_main_parses_with_fixed_order() {
        let p = parse("volatile export main() { mark; }").unwrap();
        assert!(p.functions[0].volatile);
        assert!(p.functions[0].exported);

        // Fixed order: `volatile` precedes `export`. Written the other
        // way round, `top_items` consumes `export` as the export
        // modifier (it's followed by an identifier), leaving `function()`
        // to parse "volatile" itself as the name — which the reserved-name
        // check rejects.
        let e = parse("export volatile main() { mark; }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::ReservedName { ref name, what } if name == "volatile" && what == "function"),
            "got: {:?}",
            e.kind
        );
    }

    #[test]
    fn volatile_on_a_non_main_function_errors() {
        let e = parse("volatile foo() { mark; }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::VolatileNotOnMain(ref name) if name == "foo"),
            "got: {:?}",
            e.kind
        );
        assert_eq!(e.kind.code(), "volatile-not-on-main");
        let m = e.to_string();
        assert!(m.contains("volatile") && m.contains("foo"), "got: {m}");
    }

    #[test]
    fn volatile_on_a_nested_function_errors() {
        // A nested `main` is not top-level `main` — the flag never
        // survives nesting, so this fails the same way a nested
        // non-`main` name would.
        let e = parse("main() { volatile inner() { mark; } }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::VolatileNotOnMain(ref name) if name == "inner"),
            "got: {:?}",
            e.kind
        );
    }

    #[test]
    fn volatile_as_a_definition_name_is_reserved() {
        let e = parse("volatile() { mark; }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::ReservedName { ref name, what } if name == "volatile" && what == "function"),
            "got: {:?}",
            e.kind
        );
        let e = parse("namespace volatile { }").unwrap_err();
        assert!(
            matches!(e.kind, CompileErrorKind::ReservedName { ref name, what } if name == "volatile" && what == "namespace"),
            "got: {:?}",
            e.kind
        );
    }
}
