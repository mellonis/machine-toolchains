# C2 Plan 2: PM Green-Parser Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `.pmc` front end gains a green syntax tree: a PM kind space, a source-layout pass reconstructing verbatim token text and whitespace gaps over the *unchanged* lexer, and green emission woven into the existing `Parser` — proven by `tree.text() == source` and error parity over the full `.pmc` corpus.

**Architecture:** No new parser and no lexer change. A `Layout` pre-pass slices the source into per-significant-token verbatim text plus trivia schedules (whitespace runs + comment tokens), using token start positions (reliable char-based line/col) and validating with a concat-equals-source invariant. The existing recursive-descent `Parser` gains an optional `GreenSink` (a `TreeBuilder` + the layout): `bump()` emits each consumed token with its preceding trivia; the same productions that build C1 CST nodes bracket green nodes. Error parity is by construction — same code path, same `CompileError`s. The compiler's `parse()` path never sets the sink, so PM-1 byte-identity is untouched.

**Tech Stack:** Rust; `mtc_core::syntax` (plan 1's framework: `TreeBuilder`, `SyntaxKind`, `debug_dump`, `TextLineIndex`); no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md` — this plan implements §4.1–§4.2 for PM (with the §4.1 amendment in Task 7). Views (§4.3), consumers (§5), and cutover follow in later plans; core navigation primitives are deliberately deferred to the views plan, where their real call sites live (final-review guidance: shape them from call sites, not guesses).

## Global Constraints

- Branch: `feat/c2-green-tree` (already checked out; per-task commits authorized). Every commit: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build -p mtc-core --no-default-features` all green.
- Zero new dependencies. Zero behavior change on every existing path: `parse()`, `parse_cst()`, compile, fmt, lint, LSP are untouched observables — the sink is `None` everywhere except the new `parse_green`.
- The lossless law: `parse_green(source)?.text() == source`, byte for byte, over every corpus file.
- Offsets are bytes; the lexer's line/col are 1-based with **char** columns (`Token.len` is chars). Only token START positions are trusted; ends are derived and validated (Task 3).
- Trivia placement rule (binding for every instrumented production): an upcoming token's trivia is flushed into the CURRENT node before any child node opens — nodes begin at their first significant token; trailing same-line comments land in the parent, and views derive attachment later (spec §4.3).
- Goldens are derivation-first (house rule): expected `debug_dump` strings are derived by hand from this plan's tree-shape rules, never pasted from output. If the parser's real structure contradicts a golden's assumption, re-derive the golden by hand from the rules and record the correction in the report — or go BLOCKED if the rules themselves are contradicted.
- Commit style: conventional with scope; never any AI/Claude attribution.

---

### Task 1: Core polish — the parked plan-1 nits

**Files:**
- Modify: `crates/core/src/syntax/builder.rs` (checkpoint doc clause)
- Modify: `crates/core/src/syntax/line_index.rs` (citation reflow)

**Interfaces:**
- Consumes: nothing. Produces: nothing new — doc-only corrections ruled at plan 1's final review.

- [ ] **Step 1: Correct the checkpoint doc's over-generalized clause**

In `builder.rs`, the `checkpoint()` doc currently illustrates stale-checkpoint misuse as "just wraps nothing". The re-review traced the reachable outcomes; replace that illustrative sentence (keep the surrounding doc) with:

```rust
    /// [...existing first sentences stay...]
    /// A stale checkpoint — one whose position was folded past by a
    /// later `finish_node` — is only partially detected: positions past
    /// the current children length panic loudly, but a position that
    /// happens to still be in range silently wraps whatever now sits
    /// there (the folded node, or nothing when it lands exactly at the
    /// current end). Consume a checkpoint before the frame it was taken
    /// in is finished.
```

- [ ] **Step 2: Reflow `line_index.rs`'s citation onto one line**

The module doc's `docs/core.md (syntax tree)` citation is split across two `//!` lines; rewrap that paragraph so the citation string sits unbroken on one line (match how `green.rs` does it).

- [ ] **Step 3: Verify**

Run: `cargo test -p mtc-core syntax` (all pass), `cargo fmt --check`, `cargo clippy -p mtc-core --all-targets -- -D warnings`.
Then: `grep -rn "docs/core.md (syntax tree)" crates/core/src/syntax/ | wc -l` — Expected: 6 (all six module docs, each greppable).

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/syntax/
git commit -m "polish(core): checkpoint-doc accuracy + citation reflow (final-review parked items)"
```

---

### Task 2: PM kind space (`PmcKind`)

**Files:**
- Create: `crates/post-machine/src/syntax/mod.rs`
- Create: `crates/post-machine/src/syntax/kinds.rs`
- Modify: `crates/post-machine/src/lib.rs` (add `pub mod syntax;` next to the existing module list)

**Interfaces:**
- Consumes: `mtc_core::syntax::SyntaxKind`.
- Produces: `PmcKind` (`#[repr(u16)]`, Copy/Eq/Debug) with `impl From<PmcKind> for SyntaxKind`; `pub fn kind_name(kind: SyntaxKind) -> &'static str` (exhaustive, `"?"` for unknown). Every later task references kinds as `PmcKind::Ident.into()` etc. and passes `kind_name` to `debug_dump`.

- [ ] **Step 1: Write the failing test** (in `kinds.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use mtc_core::syntax::SyntaxKind;

    #[test]
    fn kinds_convert_and_name_round_trip() {
        let k: SyntaxKind = PmcKind::Ident.into();
        assert_eq!(kind_name(k), "IDENT");
        assert_eq!(kind_name(PmcKind::File.into()), "FILE");
        assert_eq!(kind_name(PmcKind::CheckArm.into()), "CHECK_ARM");
        assert_eq!(kind_name(SyntaxKind(u16::MAX)), "?");
    }

    #[test]
    fn kind_values_are_distinct() {
        // The discriminants are the kind space — a duplicate would alias
        // two kinds. Collect and compare counts.
        let all = [
            PmcKind::Ident,
            PmcKind::Number,
            PmcKind::At,
            PmcKind::Bang,
            PmcKind::Comma,
            PmcKind::Semi,
            PmcKind::Colon,
            PmcKind::ColonColon,
            PmcKind::LParen,
            PmcKind::RParen,
            PmcKind::LBrace,
            PmcKind::RBrace,
            PmcKind::DocLine,
            PmcKind::AttentionLine,
            PmcKind::LineComment,
            PmcKind::BlockComment,
            PmcKind::Whitespace,
            PmcKind::File,
            PmcKind::UseDecl,
            PmcKind::UsePath,
            PmcKind::Namespace,
            PmcKind::Function,
            PmcKind::DocRun,
            PmcKind::Statement,
            PmcKind::Label,
            PmcKind::Item,
            PmcKind::CheckArm,
        ];
        let mut vals: Vec<u16> = all.iter().map(|k| *k as u16).collect();
        vals.sort_unstable();
        vals.dedup();
        assert_eq!(vals.len(), all.len());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine syntax::kinds`
Expected: compile error — module/types not found.

- [ ] **Step 3: Implement**

`crates/post-machine/src/syntax/kinds.rs`:

```rust
//! The `.pmc` syntax-kind space over the core framework's opaque
//! `SyntaxKind` (docs/core.md (syntax tree)): token kinds mirror the
//! lexer's `TokenKind` (plus the two trivia kinds the token stream
//! carries only implicitly — whitespace runs and comments), node kinds
//! mirror the grammar's containers. `Eof` has no kind: the green tree
//! carries trailing trivia instead of a zero-length sentinel.
//!
//! Node granularity: containers down to `CHECK_ARM`. Successor arrows
//! and check internals stay as tokens inside `ITEM`/`CHECK_ARM` — a
//! view derives them; a finer node is an additive kind if a later
//! plan wants one.

use mtc_core::syntax::SyntaxKind;

/// `.pmc` kinds. Token kinds first, then trivia, then nodes. The
/// discriminant IS the wire value inside `SyntaxKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum PmcKind {
    // Significant tokens (mirror lexer::TokenKind, minus Eof/Comment).
    Ident = 0,
    Number = 1,
    At = 2,
    Bang = 3,
    Comma = 4,
    Semi = 5,
    Colon = 6,
    ColonColon = 7,
    LParen = 8,
    RParen = 9,
    LBrace = 10,
    RBrace = 11,
    DocLine = 12,
    AttentionLine = 13,
    // Trivia tokens.
    LineComment = 14,
    BlockComment = 15,
    Whitespace = 16,
    // Nodes.
    File = 32,
    UseDecl = 33,
    UsePath = 34,
    Namespace = 35,
    Function = 36,
    DocRun = 37,
    Statement = 38,
    Label = 39,
    Item = 40,
    CheckArm = 41,
}

impl From<PmcKind> for SyntaxKind {
    fn from(k: PmcKind) -> SyntaxKind {
        SyntaxKind(k as u16)
    }
}

/// Debug name for a `.pmc` kind — the `kind_name` callback for
/// `mtc_core::syntax::debug_dump`. Unknown values render as `"?"`.
pub fn kind_name(kind: SyntaxKind) -> &'static str {
    match kind {
        k if k == PmcKind::Ident.into() => "IDENT",
        k if k == PmcKind::Number.into() => "NUMBER",
        k if k == PmcKind::At.into() => "AT",
        k if k == PmcKind::Bang.into() => "BANG",
        k if k == PmcKind::Comma.into() => "COMMA",
        k if k == PmcKind::Semi.into() => "SEMI",
        k if k == PmcKind::Colon.into() => "COLON",
        k if k == PmcKind::ColonColon.into() => "COLON_COLON",
        k if k == PmcKind::LParen.into() => "L_PAREN",
        k if k == PmcKind::RParen.into() => "R_PAREN",
        k if k == PmcKind::LBrace.into() => "L_BRACE",
        k if k == PmcKind::RBrace.into() => "R_BRACE",
        k if k == PmcKind::DocLine.into() => "DOC_LINE",
        k if k == PmcKind::AttentionLine.into() => "ATTENTION_LINE",
        k if k == PmcKind::LineComment.into() => "LINE_COMMENT",
        k if k == PmcKind::BlockComment.into() => "BLOCK_COMMENT",
        k if k == PmcKind::Whitespace.into() => "WHITESPACE",
        k if k == PmcKind::File.into() => "FILE",
        k if k == PmcKind::UseDecl.into() => "USE_DECL",
        k if k == PmcKind::UsePath.into() => "USE_PATH",
        k if k == PmcKind::Namespace.into() => "NAMESPACE",
        k if k == PmcKind::Function.into() => "FUNCTION",
        k if k == PmcKind::DocRun.into() => "DOC_RUN",
        k if k == PmcKind::Statement.into() => "STATEMENT",
        k if k == PmcKind::Label.into() => "LABEL",
        k if k == PmcKind::Item.into() => "ITEM",
        k if k == PmcKind::CheckArm.into() => "CHECK_ARM",
        _ => "?",
    }
}
```

`crates/post-machine/src/syntax/mod.rs`:

```rust
//! The `.pmc` green-syntax layer over the core framework
//! (docs/core.md (syntax tree)): the kind space, the source-layout
//! pass, and green emission for the existing parser. Views arrive in
//! a later migration step.

mod kinds;

pub use kinds::{kind_name, PmcKind};
```

`lib.rs`: add `pub mod syntax;` in the module list (alphabetical position).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-post-machine syntax::kinds` — Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/syntax/ crates/post-machine/src/lib.rs
git commit -m "feat(post-machine): pmc syntax-kind space"
```

---

### Task 3: Source layout — verbatim token text + trivia schedules

**Files:**
- Create: `crates/post-machine/src/syntax/layout.rs`
- Modify: `crates/post-machine/src/syntax/mod.rs` (add `mod layout;` + re-export)

**Interfaces:**
- Consumes: `crate::lexer::{Token, TokenKind, CommentKind}`; `PmcKind` from Task 2.
- Produces:

```rust
pub struct SigLayout {
    /// Verbatim source text of this significant token ("" for Eof).
    pub text: String,
    /// Trivia pieces between the previous significant token and this
    /// one, in source order: whitespace runs and comment tokens.
    pub trivia_before: Vec<(PmcKind, String)>,
}
/// One entry per SIGNIFICANT token (comments stripped), aligned with
/// the comment-free stream the parser walks — Eof included as the last
/// entry, whose `trivia_before` is the file's trailing trivia.
pub fn layout(source: &str, tokens: &[Token]) -> Vec<SigLayout>
```

`tokens` is the `LexMode::WithComments` stream. Task 4's sink consumes `Vec<SigLayout>` indexed by the parser's `pos`.

- [ ] **Step 1: Write the failing tests** (in `layout.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{lex_with, LexMode};
    use crate::syntax::PmcKind;

    fn layout_of(src: &str) -> Vec<SigLayout> {
        layout(src, &lex_with(src, LexMode::WithComments).expect("lexes"))
    }

    /// The foundation invariant: trivia + token texts concatenate back
    /// to the source, byte for byte.
    fn concat(entries: &[SigLayout]) -> String {
        let mut out = String::new();
        for e in entries {
            for (_, t) in &e.trivia_before {
                out.push_str(t);
            }
            out.push_str(&e.text);
        }
        out
    }

    #[test]
    fn concat_reproduces_source_with_comments_and_multibyte() {
        let src = "use std::goToEnd; // λ note\nmain() {\n  right; /* b\nlock */ left;\n}\n";
        let entries = layout_of(src);
        assert_eq!(concat(&entries), src);
    }

    #[test]
    fn trivia_pieces_are_typed_and_verbatim() {
        let src = "// c\nmain() { right; }\n";
        let entries = layout_of(src);
        // First significant token is `main`; its trivia is the comment
        // then the newline.
        assert_eq!(
            entries[0].trivia_before,
            vec![
                (PmcKind::LineComment, "// c".to_string()),
                (PmcKind::Whitespace, "\n".to_string()),
            ]
        );
        assert_eq!(entries[0].text, "main");
    }

    #[test]
    fn doc_lines_span_to_end_of_line() {
        // DocLine payloads are normalized (sigil + one space stripped),
        // so verbatim text comes from the end-of-line rule, never the
        // payload.
        let src = "? doc  text\nmain() { right; }\n";
        let entries = layout_of(src);
        assert_eq!(entries[0].text, "? doc  text");
        assert_eq!(
            entries[1].trivia_before,
            vec![(PmcKind::Whitespace, "\n".to_string())]
        );
    }

    #[test]
    fn eof_entry_carries_trailing_trivia() {
        let src = "main() { right; }\n// tail\n";
        let entries = layout_of(src);
        let eof = entries.last().expect("eof entry");
        assert_eq!(eof.text, "");
        assert_eq!(
            eof.trivia_before,
            vec![
                (PmcKind::Whitespace, "\n".to_string()),
                (PmcKind::LineComment, "// tail".to_string()),
                (PmcKind::Whitespace, "\n".to_string()),
            ]
        );
        assert_eq!(concat(&entries), src);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine syntax::layout`
Expected: compile error — `layout`/`SigLayout` not found.

- [ ] **Step 3: Implement**

The algorithm — trust STARTS, derive ENDS, validate gaps:

```rust
//! Source layout for the green tree: verbatim per-token text and the
//! trivia (whitespace + comments) between tokens, reconstructed from
//! the UNCHANGED lexer's output plus the source text. Token start
//! positions (1-based line, 1-based char column) are trusted; ends are
//! derived per kind and validated by the invariant that everything
//! between two tokens is whitespace. The concatenation of all pieces
//! is the source, byte for byte — the green tree's lossless law starts
//! here.

use crate::lexer::{CommentKind, Token, TokenKind};

use super::kinds::PmcKind;

pub struct SigLayout {
    pub text: String,
    pub trivia_before: Vec<(PmcKind, String)>,
}

/// Byte offset of each (line, col) token start, computed by one pass
/// over the source tracking 1-based line and char column.
fn start_offsets(source: &str, tokens: &[Token]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(tokens.len());
    let mut ti = 0;
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    for (byte, ch) in source.char_indices() {
        while ti < tokens.len() && tokens[ti].line == line && tokens[ti].col == col {
            offsets.push(byte);
            ti += 1;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    // Eof (and any token starting exactly at end-of-text).
    while ti < tokens.len() {
        assert!(
            matches!(tokens[ti].kind, TokenKind::Eof),
            "unplaced non-Eof token at {}:{}",
            tokens[ti].line,
            tokens[ti].col
        );
        offsets.push(source.len());
        ti += 1;
    }
    offsets
}

/// End byte of token `i`: start + `len` chars for ordinary tokens and
/// comments (`Comment::text` is verbatim, `len` counts its chars); for
/// doc/attention lines the payload is normalized, so the token runs to
/// the end of its source line instead.
fn end_offset(source: &str, token: &Token, start: usize) -> usize {
    match &token.kind {
        TokenKind::DocLine(_) | TokenKind::AttentionLine(_) => source[start..]
            .find('\n')
            .map(|nl| start + nl)
            .unwrap_or(source.len()),
        TokenKind::Eof => start,
        _ => {
            let mut it = source[start..].char_indices();
            for _ in 0..token.len {
                it.next();
            }
            it.next().map(|(o, _)| start + o).unwrap_or(source.len())
        }
    }
}

pub fn layout(source: &str, tokens: &[Token]) -> Vec<SigLayout> {
    let starts = start_offsets(source, tokens);
    let mut entries = Vec::new();
    let mut pending: Vec<(PmcKind, String)> = Vec::new();
    let mut cursor = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        let start = starts[i];
        let gap = &source[cursor..start];
        assert!(
            gap.chars().all(char::is_whitespace),
            "non-whitespace between tokens at byte {cursor}: {gap:?}"
        );
        if !gap.is_empty() {
            pending.push((PmcKind::Whitespace, gap.to_string()));
        }
        let end = end_offset(source, t, start);
        let text = &source[start..end];
        cursor = end;
        match &t.kind {
            TokenKind::Comment(c) => {
                debug_assert_eq!(text, c.text, "comment slice vs lexer text");
                let kind = match c.kind {
                    CommentKind::Line => PmcKind::LineComment,
                    CommentKind::Block => PmcKind::BlockComment,
                };
                pending.push((kind, text.to_string()));
            }
            _ => {
                entries.push(SigLayout {
                    text: text.to_string(),
                    trivia_before: std::mem::take(&mut pending),
                });
            }
        }
    }
    assert!(pending.is_empty(), "trivia after Eof");
    assert_eq!(cursor, source.len(), "source tail not covered");
    entries
}
```

Add `mod layout;` + `pub use layout::{layout, SigLayout};` to `syntax/mod.rs`.

Note: the asserts are internal-invariant panics (lexer and layout disagreeing is a bug, not an input error) — same philosophy as `TreeBuilder`. If `concat_reproduces_source…` fails because a token kind's `len` doesn't cover its source text (the reason doc lines already get the end-of-line rule), extend the per-kind rule in `end_offset` for that kind, derive the correct rule from the lexer's code (read it), and record the finding in your report — do NOT weaken the test.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-post-machine syntax::layout` — Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/syntax/
git commit -m "feat(post-machine): source layout for the pmc green tree"
```

---

### Task 4: GreenSink + container emission (FILE, USE_DECL, USE_PATH, NAMESPACE)

**Files:**
- Create: `crates/post-machine/src/syntax/emit.rs`
- Modify: `crates/post-machine/src/syntax/mod.rs` (wire `mod emit;` + re-export `GreenSink`)
- Modify: `crates/post-machine/src/parser.rs` (Parser gains `sink: Option<GreenSink>`; `bump()` hook; `g_*` helpers; instrumentation of the file/use/namespace productions; new `pub fn parse_green`)
- Test: `crates/post-machine/tests/syntax_green.rs` (new; first goldens)

**Interfaces:**
- Consumes: Task 3's `layout`/`SigLayout`, Task 2's kinds, core's `TreeBuilder`/`Checkpoint`/`GreenNode`/`SyntaxNode`/`debug_dump`.
- Produces:

```rust
// syntax/emit.rs
pub struct GreenSink { /* builder, entries: Vec<SigLayout>, flushed_upto: usize */ }
impl GreenSink {
    pub fn new(entries: Vec<SigLayout>) -> GreenSink
    /// Flush trivia_before[pos] into the current node (idempotent).
    pub fn flush(&mut self, pos: usize)
    /// flush(pos) then emit significant token `pos` verbatim.
    pub fn token(&mut self, pos: usize, kind: PmcKind)
    pub fn start(&mut self, kind: PmcKind)
    pub fn finish(&mut self)
    pub fn checkpoint(&self) -> mtc_core::syntax::Checkpoint
    pub fn start_at(&mut self, cp: mtc_core::syntax::Checkpoint, kind: PmcKind)
    pub fn into_tree(self, eof_pos: usize) -> std::rc::Rc<mtc_core::syntax::GreenNode>
}
// parser.rs
pub fn parse_green(source: &str) -> Result<std::rc::Rc<mtc_core::syntax::GreenNode>, CompileError>
```

Parser helper methods (used by Task 5's instrumentation too): `g_flush_start(&mut self, kind: PmcKind)` (flush upcoming trivia, then open node), `g_finish(&mut self)`, `g_checkpoint(&mut self) -> Option<Checkpoint>` (flush upcoming trivia first), `g_start_at(&mut self, cp: Option<Checkpoint>, kind: PmcKind)` — all no-ops when `sink` is `None`.

- [ ] **Step 1: Implement `GreenSink`** (`syntax/emit.rs`)

```rust
//! Green emission for the existing `.pmc` parser: a `TreeBuilder` fed
//! from a [`super::SigLayout`] schedule. The parser stays the single
//! owner of grammar decisions — the sink only mirrors token
//! consumption and node boundaries, so the green tree and the parser's
//! errors can never disagree (docs/core.md (syntax tree)).

use std::rc::Rc;

use mtc_core::syntax::{Checkpoint, GreenNode, TreeBuilder};

use super::kinds::PmcKind;
use super::layout::SigLayout;

pub struct GreenSink {
    builder: TreeBuilder,
    entries: Vec<SigLayout>,
    /// First significant-token index whose trivia is not yet emitted.
    flushed_upto: usize,
}

impl GreenSink {
    pub fn new(entries: Vec<SigLayout>) -> GreenSink {
        GreenSink {
            builder: TreeBuilder::new(),
            entries,
            flushed_upto: 0,
        }
    }

    /// Emit `trivia_before[pos]` into the currently open node, once.
    pub fn flush(&mut self, pos: usize) {
        if self.flushed_upto > pos {
            return;
        }
        debug_assert_eq!(self.flushed_upto, pos, "trivia flushed out of order");
        for (kind, text) in &self.entries[pos].trivia_before {
            self.builder.token((*kind).into(), text);
        }
        self.flushed_upto = pos + 1;
    }

    /// Flush, then emit significant token `pos` verbatim.
    pub fn token(&mut self, pos: usize, kind: PmcKind) {
        self.flush(pos);
        let text = std::mem::take(&mut self.entries[pos].text);
        self.builder.token(kind.into(), &text);
    }

    pub fn start(&mut self, kind: PmcKind) {
        self.builder.start_node(kind.into());
    }

    pub fn finish(&mut self) {
        self.builder.finish_node();
    }

    pub fn checkpoint(&self) -> Checkpoint {
        self.builder.checkpoint()
    }

    pub fn start_at(&mut self, cp: Checkpoint, kind: PmcKind) {
        self.builder.start_node_at(cp, kind.into());
    }

    /// Emit the trailing trivia (the Eof entry's schedule) and close.
    /// Call with the FILE node still open; this finishes it.
    pub fn into_tree(mut self, eof_pos: usize) -> Rc<GreenNode> {
        self.flush(eof_pos);
        self.builder.finish_node();
        self.builder.finish()
    }
}
```

(Adjust `into_tree`'s division of labor if the FILE open/close reads more naturally in `parse_green` — the binding requirement is: trailing trivia INSIDE `FILE`, exactly one root, all text emitted.)

- [ ] **Step 2: Wire the Parser** (`parser.rs`)

1. Add `sink: Option<GreenSink>` to `struct Parser` and `sink: None` at the existing two construction sites (`parse_cst`'s and any test constructors).
2. Hook `bump()`:

```rust
    fn bump(&mut self) {
        if !matches!(self.tokens[self.pos].kind, TokenKind::Eof) {
            if let Some(sink) = &mut self.sink {
                sink.token(self.pos, sig_kind(&self.tokens[self.pos].kind));
            }
            self.pos += 1;
        }
    }
```

with a small mapper next to it:

```rust
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
```

3. Add the `g_*` helpers on `Parser`:

```rust
    fn g_flush_start(&mut self, kind: PmcKind) {
        if let Some(sink) = &mut self.sink {
            sink.flush(self.pos);
            sink.start(kind);
        }
    }
    fn g_finish(&mut self) {
        if let Some(sink) = &mut self.sink {
            sink.finish();
        }
    }
    fn g_checkpoint(&mut self) -> Option<mtc_core::syntax::Checkpoint> {
        self.sink.as_mut().map(|sink| {
            sink.flush(self.pos);
            sink.checkpoint()
        })
    }
    fn g_start_at(&mut self, cp: Option<mtc_core::syntax::Checkpoint>, kind: PmcKind) {
        if let (Some(sink), Some(cp)) = (&mut self.sink, cp) {
            sink.start_at(cp, kind);
        }
    }
```

`g_checkpoint` must be called where `self.pos` is the checkpointed construct's FIRST token (flushing that token's trivia into the parent first — the trivia placement rule).

4. Instrument the container productions — find where each construct's parsing begins/ends by reading the production (the C1 CST constructor sites mark the ends):
   - `file()`: `g_flush_start(PmcKind::File)` is NOT used for the root (there may be leading trivia before any token, which `flush` handles at the first bump; instead open the root directly on the sink before parsing: in `parse_green`, `sink.start(PmcKind::File)` before calling `.file()`, and close via `into_tree`).
   - `use` declaration: `g_flush_start(PmcKind::UseDecl)` immediately before consuming the `use` ident; `g_finish()` after the `;` is bumped. Each path: `g_flush_start(PmcKind::UsePath)` before its first ident, `g_finish()` after its last token (alias ident included when present, the separating comma excluded).
   - `namespace` block: `g_flush_start(PmcKind::Namespace)` before the `namespace` ident; `g_finish()` after the closing `}`.
   Functions/statements are Task 5 — for THIS task, a file whose content is only `use` declarations and empty namespaces must produce correct trees; instrument nothing else yet.

5. Add `parse_green`:

```rust
/// tokens+source → green syntax tree (docs/core.md (syntax tree)).
/// Runs the SAME grammar walk as [`parse_cst`] with a green sink
/// attached: identical acceptance, identical errors. The C1 CST built
/// alongside is discarded; the compiler's [`parse`] path never sets a
/// sink and is unaffected.
pub fn parse_green(source: &str) -> Result<Rc<GreenNode>, CompileError> {
    use crate::lexer::{lex_with, LexMode};
    let tokens = lex_with(source, LexMode::WithComments)?;
    let entries = crate::syntax::layout(source, &tokens);
    let mut sig: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut comments: Vec<CommentAt> = Vec::new();
    for t in &tokens {
        if let TokenKind::Comment(c) = &t.kind {
            comments.push(CommentAt { /* same fields as parse_cst builds */ });
        } else {
            sig.push(t.clone());
        }
    }
    let mut sink = GreenSink::new(entries);
    sink.start(PmcKind::File);
    let eof_pos = sig.len() - 1;
    let parser = Parser { tokens: &sig, pos: 0, sink: Some(sink), /* other fields as parse_cst builds them */ };
    let (_items, sink) = parser.file()?; // file()'s tail now hands back its sink — see below
    Ok(sink.expect("sink was set above").into_tree(eof_pos))
}
```

Mirror `parse_cst`'s exact sig/comment split (copy its loop body; the `CommentAt` fields are visible in `parse_cst` at parser.rs:315-341). `file()` consumes `self` by value (`fn file(mut self)`), so change its return type from `Result<Vec<TopItem>, CompileError>` to `Result<(Vec<TopItem>, Option<GreenSink>), CompileError>` — the tail returns `Ok((items, self.sink))` — and adjust `parse_cst`'s one call site to destructure and ignore the (always-`None`) sink. Keep `parse_cst`'s public behavior byte-identical.

- [ ] **Step 3: Write the golden test** (`crates/post-machine/tests/syntax_green.rs`)

```rust
//! Green-parser goldens and corpus oracles. Expected dumps are derived
//! BY HAND from the plan's tree-shape rules (derivation-first: never
//! pasted from output).

use mtc_core::syntax::{debug_dump, SyntaxNode};
use mtc_post_machine::parser::parse_green;
use mtc_post_machine::syntax::kind_name;

fn dump(source: &str) -> String {
    let tree = parse_green(source).expect("parses");
    let root = SyntaxNode::new_root(tree);
    assert_eq!(root.text(), source, "lossless law");
    debug_dump(&root, &|k| kind_name(k).to_string())
}

#[test]
fn use_decl_golden() {
    // "use std::goToEnd;\n" — bytes: use=0..3, ws=3..4, std=4..7,
    // ::=7..9, goToEnd=9..16, ;=16..17, \n=17..18.
    let src = "use std::goToEnd;\n";
    let expected = "\
FILE@0..18
  USE_DECL@0..17
    IDENT@0..3 \"use\"
    WHITESPACE@3..4 \" \"
    USE_PATH@4..16
      IDENT@4..7 \"std\"
      COLON_COLON@7..9 \"::\"
      IDENT@9..16 \"goToEnd\"
    SEMI@16..17 \";\"
  WHITESPACE@17..18 \"\\n\"";
    assert_eq!(dump(src), expected);
}
```

(Adjust the exact expected string to `debug_dump`'s actual line format — nodes `NAME@start..end`, tokens `NAME@start..end "text"`, two-space indent, no trailing newline; the byte offsets above are the hand derivation. If `use`'s path list keeps the `;` outside `USE_DECL` in the real production shape, or the alias/`,` handling differs, re-derive by hand per the plan's boundary rules and record the correction.)

Add `pub` visibility for `parse_green` re-export if `mtc_post_machine::parser` is not a public module — check `lib.rs`; if the parser module is private, export `parse_green` (and nothing else new) through `lib.rs` or the `syntax` module instead, and note which in the report.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-post-machine --test syntax_green` — Expected: golden passes.
Run: `cargo test -p mtc-post-machine` — Expected: ALL existing tests still pass (the sink is None on every old path).

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/ crates/post-machine/tests/syntax_green.rs
git commit -m "feat(post-machine): green emission woven into the parser — containers"
```

---

### Task 5: Statement-level emission (FUNCTION, DOC_RUN, STATEMENT, LABEL, ITEM, CHECK_ARM)

**Files:**
- Modify: `crates/post-machine/src/parser.rs` (instrument the remaining productions)
- Test: `crates/post-machine/tests/syntax_green.rs` (two more goldens + error-parity tests)

**Interfaces:**
- Consumes: Task 4's `g_*` helpers and `parse_green`.
- Produces: full-coverage green trees for every `.pmc` construct — Task 6's corpus oracle depends on it.

- [ ] **Step 1: Instrument the remaining productions** (read each production; the C1 constructor sites mark the extents)

- **Function with a doc run**: `g_checkpoint()` BEFORE the doc run's first token is consumed (or, when no run, before the `volatile`/`export`/name token — mirror `FunctionCst::span`'s start rules); wrap the run itself in `g_flush_start(PmcKind::DocRun)` … `g_finish()`; when the function header is confirmed, `g_start_at(cp, PmcKind::Function)`; `g_finish()` after the closing `}`. Ordinary comments INSIDE a doc run are trivia tokens in the schedule — they need no handling. A dangling doc run still errors exactly as today (parity by construction).
- **Nested functions**: same instrumentation (the production is shared or parallel — mirror whichever shape the code has).
- **Statement**: `g_flush_start(PmcKind::Statement)` before its first token (label number or item start); `g_finish()` after the `;`.
- **Label**: `g_flush_start(PmcKind::Label)` before the number; `g_finish()` after the `:`.
- **Item**: `g_flush_start(PmcKind::Item)` before the item's first token; `g_finish()` after its last (successor tokens included; a comma between comma-group items stays OUTSIDE both items, at statement level).
- **Check arm**: `g_flush_start(PmcKind::CheckArm)` before the arm's first token; `g_finish()` after its last. Arms live inside the check `ITEM`'s extent.

Trailing same-line comments after `;` are trivia of the NEXT token's schedule and land in the enclosing node (body/namespace/file), NOT inside `STATEMENT` — that is the trivia placement rule working as designed; views re-derive trailing attachment later.

- [ ] **Step 2: Add two goldens** (hand-derived; same derivation-first caveat as Task 4)

```rust
#[test]
fn function_statement_golden() {
    // "main() {\n  right;\n}\n" — m=0..4 ((=4 )=5 sp=6 {=7 \n+2sp=8..11
    // right=11..16 ;=16 \n=17 }=18 \n=19; total 20.
    let src = "main() {\n  right;\n}\n";
    let expected = "\
FILE@0..20
  FUNCTION@0..19
    IDENT@0..4 \"main\"
    L_PAREN@4..5 \"(\"
    R_PAREN@5..6 \")\"
    WHITESPACE@6..7 \" \"
    L_BRACE@7..8 \"{\"
    WHITESPACE@8..11 \"\\n  \"
    STATEMENT@11..17
      ITEM@11..16
        IDENT@11..16 \"right\"
      SEMI@16..17 \";\"
    WHITESPACE@17..18 \"\\n\"
    R_BRACE@18..19 \"}\"
  WHITESPACE@19..20 \"\\n\"";
    assert_eq!(dump(src), expected);
}

#[test]
fn doc_run_label_golden() {
    // "? doc\nmain() {\n1: right;\n}\n" — ?-line=0..5 \n=5..6
    // main=6..10 (=10 )=11 sp=12 {=13 \n=14 1=15 :=16 sp=17
    // right=18..23 ;=23 \n=24 }=25 \n=26; total 27.
    let src = "? doc\nmain() {\n1: right;\n}\n";
    let expected = "\
FILE@0..27
  FUNCTION@0..26
    DOC_RUN@0..5
      DOC_LINE@0..5 \"? doc\"
    WHITESPACE@5..6 \"\\n\"
    IDENT@6..10 \"main\"
    L_PAREN@10..11 \"(\"
    R_PAREN@11..12 \")\"
    WHITESPACE@12..13 \" \"
    L_BRACE@13..14 \"{\"
    WHITESPACE@14..15 \"\\n\"
    STATEMENT@15..24
      LABEL@15..17
        NUMBER@15..16 \"1\"
        COLON@16..17 \":\"
      WHITESPACE@17..18 \" \"
      ITEM@18..23
        IDENT@18..23 \"right\"
      SEMI@23..24 \";\"
    WHITESPACE@24..25 \"\\n\"
    R_BRACE@25..26 \"}\"
  WHITESPACE@26..27 \"\\n\"";
    assert_eq!(dump(src), expected);
}
```

- [ ] **Step 3: Add error-parity tests**

```rust
#[test]
fn error_parity_with_parse_cst() {
    use mtc_post_machine::lexer::{lex, lex_with, LexMode};
    use mtc_post_machine::parser::parse_cst;
    // A few invalid inputs; extend freely. Same code path ⇒ same error,
    // but assert it so a future divergence (a green-only early return,
    // say) cannot slip in.
    for src in ["main() {", "use ;", "main() { right }", "? dangling\n"] {
        let old = lex_with(src, LexMode::WithComments)
            .and_then(|t| parse_cst(&t).map(|_| ()))
            .unwrap_err();
        let new = mtc_post_machine::parser::parse_green(src).map(|_| ()).unwrap_err();
        assert_eq!(old, new, "error parity for {src:?}");
    }
}
```

(If `CompileError` lacks `PartialEq`, compare `format!("{old:?}")` against `format!("{new:?}")` instead — do not add derives to the error type in this plan. If any sample turns out to be VALID `.pmc`, replace it with a genuinely invalid one and note it.)

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-post-machine --test syntax_green` — Expected: all goldens + parity pass.
Run: `cargo test -p mtc-post-machine` — Expected: full crate still green.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/parser.rs crates/post-machine/tests/syntax_green.rs
git commit -m "feat(post-machine): green emission — functions, statements, doc runs"
```

---

### Task 6: Corpus oracle — the lossless law over every `.pmc` in the repo

**Files:**
- Modify: `crates/post-machine/tests/syntax_green.rs`

**Interfaces:**
- Consumes: `parse_green`, the corpus on disk.
- Produces: the standing oracle later plans extend (views parity will join this file).

- [ ] **Step 1: Add the corpus walker + oracle**

```rust
/// Every .pmc file in the crate (test programs, lint fixtures, the
/// embedded stdlib) — the corpus for the lossless law. Walked
/// explicitly so a future fixture is picked up automatically.
fn corpus() -> Vec<(std::path::PathBuf, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "pmc") {
                let text = std::fs::read_to_string(&path).expect("readable .pmc");
                files.push((path, text));
            }
        }
    }
    assert!(
        files.len() >= 9,
        "corpus unexpectedly small: {} files — did the walk break?",
        files.len()
    );
    files
}

#[test]
fn corpus_lossless_law() {
    for (path, source) in corpus() {
        let tree = parse_green(&source)
            .unwrap_or_else(|e| panic!("{}: green parse failed: {e:?}", path.display()));
        let root = SyntaxNode::new_root(tree);
        assert_eq!(root.text(), source, "{}: text law", path.display());
    }
}

#[test]
fn corpus_acceptance_parity() {
    use mtc_post_machine::lexer::{lex_with, LexMode};
    use mtc_post_machine::parser::parse_cst;
    for (path, source) in corpus() {
        let old_ok = lex_with(&source, LexMode::WithComments)
            .and_then(|t| parse_cst(&t).map(|_| ()))
            .is_ok();
        let new_ok = parse_green(&source).is_ok();
        assert_eq!(old_ok, new_ok, "{}: acceptance parity", path.display());
    }
}
```

(The `>= 9` floor: 7 golden programs + the lint fixture + the stdlib at the time of writing — adjust to the actual count found, with the count stated in a comment; the floor exists so a broken walk fails loudly instead of vacuously passing on zero files.)

- [ ] **Step 2: Run to verify pass**

Run: `cargo test -p mtc-post-machine --test syntax_green` — Expected: all pass. A corpus failure here is a REAL emission bug (a missing production instrumentation, a layout gap): fix the emission — never the oracle. If a corpus file uses a construct whose golden shape is ambiguous under the plan's boundary rules, derive its intended shape from the rules, fix, and record in the report.

- [ ] **Step 3: Full gates**

Run all four workspace gates (Global Constraints). Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/post-machine/tests/syntax_green.rs
git commit -m "test(post-machine): green-tree lossless law + parity over the pmc corpus"
```

---

### Task 7: Spec amendments + plan file commit

**Files:**
- Modify: `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md` (§4.1 rewrite; §3.2 stale word)
- Add: `docs/superpowers/plans/2026-08-17-c2-plan2-pm-green-parser.md` (this file)

**Interfaces:** none — internal artifacts (issue/PR refs allowed there, not that any are needed).

- [ ] **Step 1: Amend spec §4.1**

Replace the sentence "The **existing lexers are reused** — the green builder consumes the `WithComments` stream." with:

```markdown
The **existing lexers are unchanged**: a per-crate layout pass
reconstructs each token's verbatim text and the whitespace gaps
between tokens from the source (token start positions are exact;
ends are derived per kind and validated by a
concatenation-equals-source invariant), and green emission is woven
into the existing parser behind an optional sink — same grammar walk,
same errors, with the C1-CST-building half deleted at cutover.
```

- [ ] **Step 2: Fix spec §3.2's stale name**

`A \`LineIndex\` (built once per file from the source text)` → `A \`TextLineIndex\` (built once per file from the source text)`.

- [ ] **Step 3: Verify + commit**

Run: `cargo test --workspace` (unchanged — docs only), `git status` shows only the two docs paths.

```bash
git add docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md docs/superpowers/plans/2026-08-17-c2-plan2-pm-green-parser.md
git commit -m "docs(plan): C2 plan 2 + spec §4.1 layout-reconstruction amendment"
```

---

## Completion criteria for Plan 2

- `parse_green(source)` produces a green tree for every corpus `.pmc` with `text() == source`, acceptance parity with `parse_cst`, and error parity on the sampled invalid inputs; all goldens hand-derived.
- Every pre-existing test in the workspace passes untouched (the compiler/fmt/lint/LSP paths never see a sink).
- The lexer is unmodified; `parse()`/`parse_cst()` public behavior byte-identical.
- Next plan (PM views + extraction parity) builds on: `PmcKind`/`kind_name`, `parse_green`, and the corpus harness in `tests/syntax_green.rs` — plus the core navigation primitives it will add against its real call sites.
