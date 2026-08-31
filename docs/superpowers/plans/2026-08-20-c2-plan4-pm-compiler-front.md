# C2 Plan 4: PM Compiler Front on the Green Tree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move every `.pmc` **compiler-front** consumer off the C1 path (`lex → parse_cst → lower_cst`) and onto the green tree (`lex → parse_green → extract_program`), so that compiling, linting and building a `.pmc` program runs through the C2 front end while `lower_cst` survives only as the differential oracle.

**Architecture:** Extraction (plan 3) already rebuilds the *identical* `parser::Program` the C1 path builds — proven struct-equal over the whole corpus. So this plan is a **substitution, not a rewrite**: four production call sites swap which function produces the `Program`, and everything downstream (`check_duplicate_bindings`, `flatten`, `ir::lower`, all eleven lint rules, `compile()`) is untouched by construction. Lint rides free because `LintContext` carries `{source, tokens, ast, scopes, docs}` and never held a CST. The `.pmc` LSP is deliberately NOT migrated here — `StagedAnalysis.cst` stays populated for it, at the cost of one extra `parse_cst` per document analysis until plan 5 retires it.

**Tech Stack:** Rust; `mtc_core::syntax` (plan 1); PM `syntax::{PmcKind, layout, parse_green, extract_program}` + `syntax::views` (plans 2–3); no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md` — this plan implements the compiler-front half of §5.3 and, transitively, §5.2's lint clause. Explicitly deferred: §5.2's LSP clause (plan 5), §5.1 fmt (plan 6), the §6.1 cutover deletions (plan 7).

## Global Constraints

- Branch `feat/c2-green-tree`. **Commits are authorized for this run** — commit at the end of each task, with the message given in that task's commit step. Do not squash, amend, or rebase; one commit per task.
- Every commit must be green on: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build -p mtc-core --no-default-features`.
- **Zero new dependencies.** Zero `crates/core` changes — this plan touches `crates/post-machine` only. A `crates/core` diff here is a design smell: report BLOCKED instead.
- **Zero observable behavior change — Tasks 1–7.** Same objects, same diagnostics, same spans, same lint findings, same exit codes. The strongest statement of this is byte-identity, and it is gate-tested (see Completion criteria). **Task 8 is the one deliberate exemption** and is sequenced last, after every C2 gate has been run, so it can never be confused with a substitution regression.
- **No version space moves.** `PMC_LANG_VERSION`, `PM1_PMA_DIALECT_VERSION`, `IR_VERSION`, MO/MX/MT container versions, and the `pmt.json` `project` schema all stay exactly where they are. A struct-equal `Program` lowers to an identical `IrProgram`; nothing in this plan is an acceptance-contract change.
- **The oracle stays authoritative and stays alive.** `corpus_extraction_parity` (`extract_program == lower_cst ∘ parse_cst`) must keep passing. On a mismatch, fix `crates/post-machine/src/syntax/extract.rs` — never the oracle, never `lower_cst`. If you believe `lower_cst` is wrong, STOP and report BLOCKED.
- **Do not delete anything C1.** `cst.rs`, `parse_cst`, `lower_cst`, `parse`, the `LexMode::WithoutComments` path and every `#[cfg(test)]` `use crate::parser::parse;` in `optimizer/*`, `ir.rs`, `codegen.rs` all stay. Their removal is the cutover plan's deliverable, not this one's.
- Commit style: conventional with scope (`feat(post-machine):`, `test(post-machine):`, `docs:`). **Never** any AI/Claude attribution in a commit message, comment, or doc.
- Code comments cite durable pages by page + topic keyword (`docs/core.md (syntax trees)`). Never `spec §N` in a doc comment; never an issue/PR number in published content.

---

### Task 1: No-silent-drop hardening in `views.rs` + the retokenization parity fixture

Three view accessors currently drop or ignore an unexpected shape without a word. Today nothing produces those shapes, so the drops are invisible — but from Task 4 onward `analyze()` depends on extraction, and an invisible drop becomes a silently-miscompiled program instead of a failed test. Harden them **before** the dependency exists.

**Files:**
- Modify: `crates/post-machine/src/syntax/views.rs` (`top_items` ~line 45, `NamespaceView::name_token` ~line 58, `FunctionView::header` ~line 152)
- Modify: `crates/post-machine/src/parser.rs` (doc comment on `Parser::item`, line 1813)
- Create: `crates/post-machine/tests/syntax/retok.pmc`
- Test: `crates/post-machine/src/syntax/views.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `mtc_core::syntax::{TreeBuilder, SyntaxNode}`, `super::kinds::PmcKind`, plan-3 views.
- Produces: no new public API. Three `debug_assert!`s and one new corpus fixture file that `tests/syntax_green.rs::corpus()` picks up automatically (it walks the crate for `*.pmc`).

- [ ] **Step 1: Write the failing tests** (append to the `mod tests` at the bottom of `crates/post-machine/src/syntax/views.rs`)

The three tests below assert on `debug_assert!`, which compiles out under
`--release`. `cargo test` and CI's `cargo nextest run` both build in debug,
so they run normally there; the `#[cfg(debug_assertions)]` guard keeps a
release-mode run from reporting three confusing "did not panic" failures.

```rust
    // Gated the same way the three tests below are: under `--release`
    // they compile out, and an ungated import would then be unused and
    // fail `clippy -D warnings`.
    #[cfg(debug_assertions)]
    use mtc_core::syntax::TreeBuilder;

    /// A FILE whose child node is not one of the three top-level kinds.
    /// `top_items` filter-maps such a child away; the assertion makes
    /// the drop loud in debug builds instead of yielding a short list.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "unexpected node kind at top level")]
    fn top_items_refuses_an_unexpected_child_kind() {
        let mut b = TreeBuilder::new();
        b.start_node(PmcKind::File.into());
        b.start_node(PmcKind::Statement.into());
        b.token(PmcKind::Ident.into(), "right");
        b.token(PmcKind::Semi.into(), ";");
        b.finish_node();
        b.finish_node();
        let root = SyntaxNode::new_root(b.finish());
        let file = FileView::cast(root).expect("root is FILE");
        let _ = file.items().count();
    }

    /// A NAMESPACE header is exactly `namespace <name>`: two IDENTs
    /// before the block. `.nth(1)` would happily take the second of
    /// three and hand back a wrong name.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "NAMESPACE header is exactly")]
    fn name_token_refuses_a_three_ident_header() {
        let mut b = TreeBuilder::new();
        b.start_node(PmcKind::Namespace.into());
        b.token(PmcKind::Ident.into(), "namespace");
        b.token(PmcKind::Ident.into(), "a");
        b.token(PmcKind::Ident.into(), "b");
        b.token(PmcKind::LBrace.into(), "{");
        b.token(PmcKind::RBrace.into(), "}");
        b.finish_node();
        let root = SyntaxNode::new_root(b.finish());
        let ns = NamespaceView::cast(root).expect("root is NAMESPACE");
        let _ = ns.name();
    }

    /// The only two modifier IDENTs the parser accepts before a
    /// function name are `export` and `volatile`. A third would be
    /// silently ignored by the catch-all arm.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "unexpected modifier IDENT")]
    fn header_refuses_an_unknown_modifier_ident() {
        let mut b = TreeBuilder::new();
        b.start_node(PmcKind::Function.into());
        b.token(PmcKind::Ident.into(), "inline");
        b.token(PmcKind::Ident.into(), "f");
        b.token(PmcKind::LParen.into(), "(");
        b.token(PmcKind::RParen.into(), ")");
        b.token(PmcKind::LBrace.into(), "{");
        b.token(PmcKind::RBrace.into(), "}");
        b.finish_node();
        let root = SyntaxNode::new_root(b.finish());
        let f = FunctionView::cast(root).expect("root is FUNCTION");
        let _ = f.header();
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-post-machine --lib syntax::views -- --nocapture`
Expected: three FAILs — `should_panic` tests that did not panic ("test did not panic as expected"). The `top_items` case returns 0 items, `name_token` returns `"a"`, `header` returns a name with no complaint.

- [ ] **Step 3: Add the three assertions**

In `top_items`, replace the body:

```rust
/// Direct top-level-shaped children of `node`, in document order —
/// shared by `FileView::items` and `NamespaceView::items`. A child that
/// casts to none of the three kinds is a tree the parser cannot
/// produce: FILE/NAMESPACE children are only USE_DECL / NAMESPACE /
/// FUNCTION, and a bound DOC_RUN is retro-wrapped into its own FUNCTION
/// rather than sitting at this level. Asserted rather than silently
/// filtered, because every consumer of this iterator treats a short
/// list as "the file had fewer items".
fn top_items(node: &SyntaxNode) -> impl Iterator<Item = TopView> + '_ {
    node.children().filter_map(|child| {
        let kind = child.kind();
        let top = cast_top(child);
        debug_assert!(
            top.is_some(),
            "unexpected node kind at top level: {:?}",
            kind
        );
        top
    })
}
```

In `NamespaceView::name_token`, collect first so the shape can be checked:

```rust
    /// The second significant token — the IDENT after the `namespace`
    /// keyword IDENT.
    pub fn name_token(&self) -> SyntaxToken {
        let idents: Vec<SyntaxToken> = self
            .syntax()
            .children_with_tokens()
            .filter_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == PmcKind::Ident.into() => Some(t),
                _ => None,
            })
            .collect();
        debug_assert_eq!(
            idents.len(),
            2,
            "NAMESPACE header is exactly `namespace <name>`: 2 IDENTs, got {}",
            idents.len()
        );
        idents
            .into_iter()
            .nth(1)
            .expect("NAMESPACE always carries a name IDENT after the keyword IDENT")
    }
```

In `FunctionView::header`, replace the catch-all arm:

```rust
        for t in &idents[..idents.len() - 1] {
            match t.text() {
                "volatile" => has_volatile = true,
                "export" => has_export = true,
                other => debug_assert!(
                    false,
                    "unexpected modifier IDENT {other:?} before the function name — \
                     the parser accepts only `export` and `volatile`"
                ),
            }
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-post-machine --lib syntax::views`
Expected: PASS, all three.

- [ ] **Step 5: Add the retokenization parity fixture**

Create `crates/post-machine/tests/syntax/retok.pmc` with exactly this content:

```pmc
// Retokenization parity fixture (docs/core.md (syntax trees)): the four
// shapes where extraction's retokenization half could plausibly diverge
// from the original lex, all in one file the corpus oracles pick up
// automatically.
//
//   - A NON-ASCII identifier. `Token.len` counts CHARACTERS while a
//     green token's `TextRange` counts BYTES, so a multi-byte name is
//     the case where `sig_tokens`/`token_from_syntax` would disagree
//     with the lexer if either mixed the two units up. The `.pmc`
//     identifier rule is Unicode-aware (`char::is_alphabetic`), so this
//     is a legal program, not a torture case.
//   - A multi-byte BLOCK COMMENT sharing a line with code, so trivia
//     reconstruction has to land a non-ASCII span between two
//     significant tokens on one line.
//   - LEADING ZEROS on a label and its `goto`, whose written form is
//     carried by the token text rather than a parsed value.
//   - `use as as as;` — the `as`-marker rule read three times over: the
//     path segment, the marker, and the alias are all the same word.
//     `as` is an ordinary IDENT in `.pmc`, so this is the sharpest
//     shape `UsePathView::segments`/`alias_token` can be handed.
use std::goToEnd as end;
use as as as;

шагВперёд() {
    right;
}

zeros() {
    007: left; /* ёжик */ right;
    goto 007;
}

main() {
    @шагВперёд();
    @zeros();
    @end();
    @as();
}
```

- [ ] **Step 6: Run the corpus oracles against the new fixture**

Run: `cargo test -p mtc-post-machine --test syntax_green`
Expected: PASS — 15 tests, including `corpus_lossless_law`, `corpus_acceptance_parity`, `corpus_extraction_parity`.

This fixture is a **regression pin, not a red test**: it was validated against the built parser while this plan was written and all three oracles already pass on it. If any of them FAILS, that is a genuine extraction bug found by the fixture — fix `extract.rs`, report what it was, and keep the fixture.

- [ ] **Step 7: Add the `in_group` back-reference to `Parser::item`**

`Parser::item` has no doc comment, yet it is now reached from two directions: the parser's own statement production, and `reparse_item`, which must supply `in_group` from outside because an isolated `ITEM` node carries no memory of its former comma-group position. Add above `fn item(&mut self, in_group: bool)` at `crates/post-machine/src/parser.rs:1813`:

```rust
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
```

- [ ] **Step 8: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p mtc-core --no-default-features`
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add crates/post-machine/src/syntax/views.rs crates/post-machine/src/parser.rs crates/post-machine/tests/syntax/retok.pmc
git commit -m "polish(post-machine): views refuse impossible shapes loudly + retokenization fixture"
```

---

### Task 2: The token-provenance law

Task 4 will read `AnalysisOutput.tokens` off the significant half of the *one* `WithComments` lex that the green parse already needs, instead of a second `WithoutComments` lex. That field is `LintContext.tokens` — the input to the `leading-zeros` rule — so the substitution is only safe if filtering a `WithComments` stream of its `Comment` tokens yields exactly the `WithoutComments` stream. It does, structurally: the lexer branches on mode in exactly two places (`lexer.rs:232`, `lexer.rs:262`), both of which only decide whether a `Comment` token is pushed, and the unterminated-block-comment error returns *before* either branch. Pin it, because it is load-bearing and invisible.

An existing test (`compiler.rs::analyze_staged_on_clean_source_matches_analyze_at_every_stage`) already asserts a weaker form of this — `TokenKind` only, one fixture. This generalizes it to whole-`Token` equality (kind + line + col + len) over the corpus.

**Files:**
- Modify: `crates/post-machine/tests/syntax_green.rs`

**Interfaces:**
- Consumes: `corpus()` (already in this file), `mtc_post_machine::lexer::{lex, lex_with, LexMode, TokenKind}`.
- Produces: two tests, no API.

- [ ] **Step 1: Write the tests** (append to `crates/post-machine/tests/syntax_green.rs`)

```rust
/// The token-provenance law: a `WithComments` stream filtered of its
/// `Comment` tokens is EXACTLY the `WithoutComments` stream — same
/// kinds, same line/col, same char lengths, same order. This is what
/// licenses `compiler::analyze` reading its `tokens` field off the
/// single `WithComments` lex the green parse needs, instead of lexing
/// a second time; that field is `LintContext.tokens`, so any drift here
/// would silently change lint's input. Structurally guaranteed by the
/// lexer (its two mode branches decide only whether a `Comment` token
/// is pushed), pinned here because nothing else would catch a
/// regression until a lint finding moved.
#[test]
fn corpus_token_provenance_law() {
    use mtc_post_machine::lexer::{LexMode, TokenKind, lex, lex_with};
    for (path, source) in corpus() {
        let significant: Vec<_> = lex_with(&source, LexMode::WithComments)
            .expect("lexes")
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
            .collect();
        let without = lex(&source).expect("lexes");
        assert_eq!(
            significant,
            without,
            "{}: token provenance",
            path.display()
        );
    }
}

/// The same law's error half: a source that fails to lex fails
/// IDENTICALLY in both modes. `parse_green` lexes `WithComments`
/// internally while `compiler::analyze` used to lex `WithoutComments`,
/// so a mode-dependent lex error would change which diagnostic a
/// malformed program reports.
#[test]
fn lex_modes_agree_on_errors() {
    use mtc_post_machine::lexer::{LexMode, lex, lex_with};
    for src in ["/* never closed\nmain() { right; }\n", "left(!) / right(!);"] {
        let without = lex(src).expect_err("sample must fail to lex");
        let with = lex_with(src, LexMode::WithComments).expect_err("sample must fail to lex");
        assert_eq!(without, with, "lex error parity for {src:?}");
    }
}
```

- [ ] **Step 2: Extend the existing parse-error parity test with lex-level cases**

In `error_parity_with_parse_cst`, replace the `for src in [...]` list and its comment:

```rust
    // Parse-level: unterminated function body; a bare `use` with no
    // path; a missing `;` after a command; a doc run with nothing bound
    // to it. Lex-level: an unterminated block comment, and a stray `/`.
    // The lex cases matter because `parse_green` lexes internally while
    // the C1 side lexes first — both must surface the same error.
    for src in [
        "main() {",
        "use ;",
        "main() { right }",
        "? dangling\n",
        "/* never closed\nmain() { right; }\n",
        "left(!) / right(!);",
    ] {
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p mtc-post-machine --test syntax_green`
Expected: PASS.

These are **law pins, not red-green cycles** — they should pass on the first run. If `corpus_token_provenance_law` or `lex_modes_agree_on_errors` FAILS, **STOP and report BLOCKED**: the provenance assumption behind Task 4 is false, and Task 4's design has to change (`analyze` would have to keep its own `WithoutComments` lex) before anything else proceeds.

- [ ] **Step 4: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/tests/syntax_green.rs
git commit -m "test(post-machine): token-provenance law + lex-level error parity"
```

---

### Task 3: `parse_green_from_tokens` and the shared comment split

`analyze_staged` must retain its token vector across a parse failure, so it lexes first and needs a parse entry that accepts an already-lexed stream. Split `parse_green` in two. While here, factor the significant/comment split that `parse_cst` and `parse_green` currently carry as two byte-identical copies.

**Files:**
- Modify: `crates/post-machine/src/parser.rs` (`parse_cst` line 318, `parse_green` line 353)
- Test: `crates/post-machine/tests/syntax_green.rs`

**Interfaces:**
- Consumes: `crate::lexer::{Token, TokenKind, LexMode, lex_with}`, `crate::syntax::{layout, GreenSink, PmcKind}`.
- Produces:
  - `pub fn parse_green_from_tokens(source: &str, tokens: &[Token]) -> Result<Rc<GreenNode>, CompileError>` — used by Tasks 4, 5.
  - `fn split_comments(tokens: &[Token]) -> (Vec<Token>, Vec<CommentAt>)` — private to `parser.rs`; `CommentAt` is already private there.
  - `pub fn significant_tokens(tokens: &[Token]) -> Vec<Token>` — used by Task 4 to fill `AnalysisOutput.tokens`. **`pub`, not `pub(crate)`:** its first caller lands in Task 4, so a crate-private version would fail Task 3's own `clippy -D warnings` gate as dead code, and `tests/syntax_green.rs` is an external crate that could not reach it either. It is also the honest visibility — any caller that pre-lexes and uses `parse_green_from_tokens` needs the same split to recover the significant stream.
  - `parse_green(source)` keeps its exact current signature and behavior.

- [ ] **Step 1: Write the failing test** (append to `crates/post-machine/tests/syntax_green.rs`)

```rust
/// The tokens-taking entry point on a pre-lexed stream produces a tree
/// that satisfies the lossless law, and the split's significant half is
/// the `WithoutComments` lex `compiler::analyze` will read its tokens
/// from — the split that lets the staged pipeline keep its tokens
/// across a parse failure without lexing twice.
///
/// The dump comparison against [`parse_green`] is a delegation pin, NOT
/// an independent oracle: `parse_green` IS a call to
/// `parse_green_from_tokens`, so the two sides are one function over
/// content-identical inputs and agree by construction. It is kept
/// because it would catch `parse_green` growing logic of its own. The
/// load-bearing assertions here are the lossless law and the
/// token-provenance one.
#[test]
fn parse_green_from_tokens_matches_parse_green() {
    use mtc_post_machine::lexer::{LexMode, lex_with};
    use mtc_post_machine::parser::parse_green_from_tokens;
    let src = "// lead\nuse std::goToEnd as end;\nexport main() {\n    1: right;\n}\n";
    let tokens = lex_with(src, LexMode::WithComments).expect("lexes");

    let a = SyntaxNode::new_root(parse_green(src).expect("parses"));
    let b = SyntaxNode::new_root(parse_green_from_tokens(src, &tokens).expect("parses"));

    assert_eq!(b.text(), src, "lossless law");
    assert_eq!(
        debug_dump(&a, &|k| kind_name(k).to_string()),
        debug_dump(&b, &|k| kind_name(k).to_string())
    );

    // The split's significant half is the `WithoutComments` lex, which
    // is what Task 4 relies on — asserted here so `significant_tokens`
    // has a caller from the commit that introduces it.
    assert_eq!(
        mtc_post_machine::parser::significant_tokens(&tokens),
        mtc_post_machine::lexer::lex(src).expect("lexes")
    );
}

/// A pre-lexed stream that fails to PARSE surfaces the error rather
/// than swallowing it — the staged pipeline reports this one as its
/// fatal while keeping the tokens it already has. As above, the
/// equality against [`parse_green`] holds by delegation, not by
/// independent derivation; what this pins is that the tokens-taking
/// entry returns `Err` at all on unparseable input.
#[test]
fn parse_green_from_tokens_reports_the_same_parse_error() {
    use mtc_post_machine::lexer::{LexMode, lex_with};
    use mtc_post_machine::parser::parse_green_from_tokens;
    let src = "main() { right }";
    let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
    assert_eq!(
        parse_green_from_tokens(src, &tokens).map(|_| ()).unwrap_err(),
        parse_green(src).map(|_| ()).unwrap_err()
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-post-machine --test syntax_green parse_green_from_tokens`
Expected: FAIL to compile — `no function or associated item named 'parse_green_from_tokens' found`.

- [ ] **Step 3: Factor the split and add the new entry point**

In `crates/post-machine/src/parser.rs`, add above `parse_cst`:

```rust
/// Split a token stream into its significant tokens and its comment
/// trivia — the shape both parse paths hand to `Parser`. `sig_index`
/// records how many significant tokens precede each comment, which is
/// the `pos` the significant-token walk sits at while that comment is
/// pending. A `LexMode::WithoutComments` stream yields an empty
/// `comments` and clones straight through.
fn split_comments(tokens: &[Token]) -> (Vec<Token>, Vec<CommentAt>) {
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
    (sig, comments)
}

/// The significant half of [`split_comments`] — every token that is not
/// comment trivia. Equal, element for element, to a
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
    split_comments(tokens).0
}
```

Rewrite `parse_cst`'s prologue to use it (the rest of the function is unchanged):

```rust
pub fn parse_cst(tokens: &[Token]) -> Result<Cst, CompileError> {
    let (sig, comments) = split_comments(tokens);
    let (items, _sink) = Parser {
        tokens: &sig,
        pos: 0,
        namespaces: HashSet::new(),
        declared_fns: HashSet::new(),
        comments,
        cpos: 0,
        prev_end_line: 0,
        sink: None,
    }
    .file()?;
    Ok(Cst { items })
}
```

Replace `parse_green` with the pair:

```rust
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
/// comment's own text and break the `text() == source` law.
///
/// Runs the SAME grammar walk as [`parse_cst`] with a green sink
/// attached: identical acceptance, identical errors — the sink only
/// mirrors token consumption and node boundaries alongside the
/// unchanged parser logic.
pub fn parse_green_from_tokens(
    source: &str,
    tokens: &[Token],
) -> Result<Rc<GreenNode>, CompileError> {
    let entries = syntax::layout(source, tokens);
    let (sig, comments) = split_comments(tokens);
    let eof_pos = sig.len() - 1;
    let mut sink = GreenSink::new(entries);
    sink.start(PmcKind::File);
    let (_items, sink) = Parser {
        tokens: &sig,
        pos: 0,
        namespaces: HashSet::new(),
        declared_fns: HashSet::new(),
        comments,
        cpos: 0,
        prev_end_line: 0,
        sink: Some(sink),
    }
    .file()?;
    Ok(sink
        .expect("parse_green_from_tokens always seeds a sink before calling file()")
        .into_tree(eof_pos))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-post-machine --test syntax_green`
Expected: PASS, all tests including the two new ones.

- [ ] **Step 5: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all green. The `parse_cst` refactor is behavior-preserving; if any test outside `syntax_green.rs` moved, the split is not byte-identical to what it replaced — revert and re-derive.

- [ ] **Step 6: Commit**

```bash
git add crates/post-machine/src/parser.rs crates/post-machine/tests/syntax_green.rs
git commit -m "feat(post-machine): parse_green_from_tokens + one shared comment split"
```

---

### Task 4: `analyze` on the green tree

The first production consumer. After this task, every `pmt` compile, build, and CLI lint of a `.pmc` file runs its front end on the green tree.

**Files:**
- Modify: `crates/post-machine/src/compiler.rs` (`analyze`, lines 417–437)
- Test: `crates/post-machine/src/compiler.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::parser::{parse_green_from_tokens, significant_tokens}` (Task 3), `crate::syntax::extract_program` (plan 3), `mtc_core::syntax::SyntaxNode`, `crate::lexer::{LexMode, lex_with}`.
- Produces: `analyze`'s signature is unchanged — `pub(crate) fn analyze(source: &str) -> Result<AnalysisOutput, CompileError>`. Every field of `AnalysisOutput` keeps its exact current meaning, `tokens` included (`WithoutComments`-equivalent).

- [ ] **Step 1: Write the failing test** (append to `compiler.rs`'s `mod tests`)

```rust
    /// `analyze` runs on the green tree, and the C1 path is the oracle:
    /// the AST it produces, the diagnostics, the scope summary and the
    /// token stream must all match what `lex + parse` produced. Written
    /// against the C1 functions directly rather than a recorded
    /// snapshot, so the oracle cannot drift.
    #[test]
    fn analyze_matches_the_c1_front_end() {
        let src = "// lead\nuse std::goToEnd as end;\nnamespace ns { export inner() { right; } }\n? documented\nexport main() {\n    helper() { left; }\n    007: @helper();\n    @ns::inner();\n    @end();\n    goto 007;\n}\n";

        let expected_ast = {
            let tokens = crate::lexer::lex(src).expect("lexes");
            let parsed = crate::parser::parse(&tokens).expect("parses");
            let Flattened { program, .. } = flatten(parsed);
            program
        };
        let expected_tokens = crate::lexer::lex(src).expect("lexes");

        let a = analyze(src).expect("analyzes");
        assert_eq!(a.ast, expected_ast, "flattened AST parity");
        assert_eq!(a.tokens, expected_tokens, "token provenance");
    }

    /// A source that fails to lex still fails at the lex stage, with
    /// the same error — `analyze` now lexes `WithComments`, and the
    /// mode must not change which diagnostic a malformed program gets.
    #[test]
    fn analyze_reports_the_same_lex_error_as_before() {
        let src = "/* never closed\nmain() { right; }\n";
        assert_eq!(
            analyze(src).map(|_| ()).unwrap_err(),
            crate::lexer::lex(src).map(|_| ()).unwrap_err()
        );
    }
```

- [ ] **Step 2: Run the tests to verify the first one fails**

Run: `cargo test -p mtc-post-machine --lib compiler::tests::analyze`
Expected: PASS on the current code. Both are **oracle harnesses, not red tests** — today `analyze` *is* the C1 path, so they compare it to itself and cannot fail. They become meaningful in Step 4, where the two sides are genuinely different implementations.

Note the coverage limit honestly: this fixture is hand-picked and is NOT in the corpus, so it is the corpus oracle (`corpus_extraction_parity`, 12 files) that carries the real extraction guarantee. This test's job is to catch a substitution mistake in `analyze` itself — wrong token provenance, a dropped `check_duplicate_bindings`, a reordered stage — not an extraction bug.

Record the baseline: run both, confirm green, then proceed.

- [ ] **Step 3: Substitute the front end**

Replace `analyze`'s body in `crates/post-machine/src/compiler.rs`:

```rust
/// lex → green parse → extract → duplicate-binding check → flatten →
/// lower. Stops before the optimizer; `compile()` composes this with the
/// back half.
///
/// The parse is the green one (docs/core.md (syntax trees)): one
/// `WithComments` lex feeds both `parse_green_from_tokens` and — through
/// `significant_tokens` — the `tokens` field, which stays exactly the
/// `WithoutComments` stream it has always been (pinned by
/// `tests/syntax_green.rs::corpus_token_provenance_law`). `extract_program`
/// rebuilds the same `Program` the C1 path built, held to it by
/// `tests/syntax_green.rs::corpus_extraction_parity`.
pub(crate) fn analyze(source: &str) -> Result<AnalysisOutput, CompileError> {
    let lexed = crate::lexer::lex_with(source, LexMode::WithComments)?;
    let green = crate::parser::parse_green_from_tokens(source, &lexed)?;
    let parsed = crate::syntax::extract_program(&SyntaxNode::new_root(green), source);
    let tokens = crate::parser::significant_tokens(&lexed);
    check_duplicate_bindings(&parsed)?;
    let Flattened {
        program,
        scopes,
        warnings: vis,
        resolutions: _,
        docs,
    } = flatten(parsed);
    let (ir, diagnostics) = lower_and_merge(&program, vis)?;
    Ok(AnalysisOutput {
        tokens,
        ast: program,
        ir,
        diagnostics,
        scopes,
        docs,
    })
}
```

Add `use mtc_core::syntax::SyntaxNode;` to `compiler.rs`'s imports if it is not already there. `LexMode` is already imported (`analyze_staged` uses it).

- [ ] **Step 4: Run the oracle tests**

Run: `cargo test -p mtc-post-machine --lib compiler`
Expected: PASS. `analyze_matches_the_c1_front_end` now genuinely compares two different implementations; a failure here is an extraction bug (fix `extract.rs`) or a provenance bug (fix `significant_tokens`), never a change to the oracle.

- [ ] **Step 5: Run the byte-identity gates**

Run:
```bash
cargo test -p mtc-post-machine --test golden_programs
cargo test -p mtc-post-machine --test asm_volatile
cargo test -p mtc-post-machine --test opt_equivalence
cargo test -p mtc-post-machine --test volatile_equivalence
cargo test -p mtc-post-machine --test compile_programs
cargo test -p mtc-post-machine --test lint_programs
cargo test -p mtc-post-machine --test sweep
```
Expected: all PASS. These are the standing regression gates, and they now exercise the green path end to end: the derivation-first `.pmt` goldens byte-compare committed snapshots, `asm_volatile` pins directive-free files assembling byte-identically, and `sweep` REPORTS per-program instruction counts and image sizes (its totals block is explicitly not an assertion — it asserts termination outcomes and the step-limit trap only, so a change in the totals must be caught by comparing runs, not by a red test). **A single byte moving here means the substitution was not behavior-preserving — STOP and report, do not regenerate a golden.**

- [ ] **Step 6: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p mtc-core --no-default-features`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/post-machine/src/compiler.rs
git commit -m "feat(post-machine): analyze runs the green front end"
```

---

### Task 5: `analyze_staged` on the green tree

The LSP's pipeline entry. Same substitution, plus one interim compromise: `StagedAnalysis.cst` stays populated, because the `.pmc` LSP still reads it. That costs one extra `parse_cst` per document analysis, until plan 5 retires the field.

**Files:**
- Modify: `crates/post-machine/src/compiler.rs` (`StagedAnalysis` doc comments lines 462–474, `analyze_staged` lines 477–541)
- Test: `crates/post-machine/src/compiler.rs` (its `#[cfg(test)] mod tests` — the four existing tier tests stay **unmodified**)

**Interfaces:**
- Consumes: everything Task 3 and Task 4 produced.
- Produces: `analyze_staged`'s signature and `StagedAnalysis`'s field set are unchanged. The degradation tiers are unchanged: `tokens` `None` only on lex failure; `cst` `None` on lex or parse failure; `analysis` `None` on any failure; `fatal` carries the single error.

- [ ] **Step 1: Confirm the tier tests are the specification, and leave them alone**

Run: `cargo test -p mtc-post-machine --lib compiler::tests::analyze_staged`
Expected: PASS — four tests (`..._on_clean_source_matches_analyze_at_every_stage`, `..._lex_failure_degrades_everything_to_none`, `..._parse_failure_keeps_tokens_but_not_cst`, `..._duplicate_binding_keeps_cst_but_not_analysis`) plus `..._lower_failure_proves_the_pipeline_reaches_ir_lower`.

**Do not edit these tests in this task.** They pin the observable contract, and the whole point of the substitution is that they keep passing untouched. If one needs editing to go green, the substitution is wrong.

- [ ] **Step 2: Write the additional failing test** (append to `compiler.rs`'s `mod tests`)

```rust
    /// The interim double-parse invariant: wherever the green parse
    /// succeeds, `parse_cst` succeeds too, so the not-yet-migrated
    /// `.pmc` LSP still gets its C1 CST. Acceptance parity makes this
    /// true by construction (both walks are the same `Parser`); pinned
    /// here because `analyze_staged` degrades rather than panics if it
    /// ever stops being true, which would otherwise be silent.
    #[test]
    fn staged_cst_is_present_whenever_the_green_parse_succeeded() {
        for src in [
            "main() { right; }\n",
            "// only a comment\n",
            "",
            "use std::goToEnd as end;\nmain() { @end(); }\n",
            "? doc\nexport main() {\n    1: left;\n}\n",
        ] {
            let staged = analyze_staged(src);
            if staged.fatal.is_none() {
                assert!(
                    staged.cst.is_some(),
                    "green parse succeeded but cst is None for {src:?}"
                );
            }
        }
    }
```

- [ ] **Step 3: Run it to verify it passes on the current code**

Run: `cargo test -p mtc-post-machine --lib compiler::tests::staged_cst_is_present`
Expected: PASS — today `cst` comes from the only parse there is. Like Task 4's Step 2 this is an oracle installed ahead of the change; it becomes meaningful in Step 5.

- [ ] **Step 4: Substitute the front end**

Replace the doc comment on `analyze_staged` and its lex/parse prologue in `crates/post-machine/src/compiler.rs`:

```rust
/// lex (WithComments) → green parse → extract → duplicate-binding check
/// → flatten → ir::lower, retaining each stage's outcome instead of
/// stopping at the first failure. Extraction and `flatten` are
/// infallible, so the only post-parse fatals are `DuplicateBinding` (the
/// binding check) and `UndefinedLabel` (`ir::lower`) — the pipeline
/// always runs through `ir::lower`, never stopping at `flatten`. The
/// `IrProgram` itself is discarded once `ir::lower` has had its say: the
/// LSP's tiers only need the flattened `Analysis`, not the CFG.
///
/// Interim double parse: the `.pmc` language service still reads the C1
/// CST, so `parse_cst` runs alongside the green parse until that service
/// moves onto views. The green parse is the authority — it produces the
/// `Program` and any parse fatal; `cst` is a side artifact built only
/// after it succeeded.
pub(crate) fn analyze_staged(source: &str) -> StagedAnalysis {
    let lexed = match crate::lexer::lex_with(source, LexMode::WithComments) {
        Ok(tokens) => tokens,
        Err(fatal) => {
            return StagedAnalysis {
                tokens: None,
                cst: None,
                analysis: None,
                fatal: Some(fatal),
            };
        }
    };
    let green = match crate::parser::parse_green_from_tokens(source, &lexed) {
        Ok(green) => green,
        Err(fatal) => {
            return StagedAnalysis {
                tokens: Some(lexed),
                cst: None,
                analysis: None,
                fatal: Some(fatal),
            };
        }
    };
    // `.ok()`, not `expect`: acceptance parity makes this `Some`
    // whenever the green parse succeeded, and if that ever broke, an
    // editor should lose one tier of features rather than take the
    // language server down.
    let cst = crate::parser::parse_cst(&lexed).ok();
    debug_assert!(
        cst.is_some(),
        "acceptance parity: parse_cst must succeed wherever parse_green did"
    );
    let program = crate::syntax::extract_program(&SyntaxNode::new_root(green), source);
    let tokens = lexed;
    if let Err(fatal) = check_duplicate_bindings(&program) {
        return StagedAnalysis {
            tokens: Some(tokens),
            cst,
            analysis: None,
            fatal: Some(fatal),
        };
    }
```

The remainder of the function — the `Flattened { .. } = flatten(program);` destructuring and the `match lower_and_merge(...)` tail — is unchanged **except** that its two `cst: Some(cst)` field initializers become `cst` (the value is already an `Option<Cst>`). Each `return` sits on a diverging path, so moving `cst` out in one branch does not affect the others.

Update `StagedAnalysis`'s `cst` field doc to record why it is still here:

```rust
    /// CST of the current text (`None` when lexing or parsing failed).
    /// Interim: the `.pmc` language service has not moved onto the green
    /// tree yet, so this is still built alongside it. It disappears when
    /// that service migrates.
    pub cst: Option<Cst>,
```

- [ ] **Step 5: Run the tier tests, unmodified**

Run: `cargo test -p mtc-post-machine --lib compiler`
Expected: PASS — all five tier tests plus the new one, with no edits to any of them.

- [ ] **Step 6: Run the LSP suite**

Run: `cargo test -p mtc-post-machine --lib lsp`

The `.pmc`/`.pma` language services are unit-tested in-crate; there is no LSP integration test file.

Expected: PASS. The LSP still reads `state.cst`, so nothing it does should move; a failure here means the `cst` side artifact is not what it used to be.

- [ ] **Step 7: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p mtc-core --no-default-features`
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add crates/post-machine/src/compiler.rs
git commit -m "feat(post-machine): staged analysis runs the green front end"
```

---

### Task 6: `stdlib::roster` and `cli::driver::scan_source`

The last two production callers of the C1 path outside fmt and the LSP.

`roster()` hand-walks the CST for the `std` namespace's exported functions. Rather than hand-walking views instead, it goes through `extract_program` and filters the `Program` — reusing the parity-proven path instead of writing a second walk that could drift from it.

`scan_source()` is the build driver's pre-pass: it extracts the volatile bit and the export list to plan cross-object resolution, and silently yields an empty scan on any failure (the real diagnostic comes later, when the unit is actually compiled).

**Files:**
- Modify: `crates/post-machine/src/stdlib/mod.rs` (`roster`, lines 75–105)
- Modify: `crates/post-machine/src/cli/driver.rs` (`scan_source`, lines 789–800)
- Test: `crates/post-machine/src/stdlib/mod.rs` and `crates/post-machine/src/cli/driver.rs` (their `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::parser::parse_green`, `crate::syntax::extract_program`, `mtc_core::syntax::SyntaxNode`.
- Produces: both functions keep their exact signatures and return values. `RosterEntry`'s fields and their order are unchanged.

- [ ] **Step 1: Write the failing tests**

Append to `crates/post-machine/src/stdlib/mod.rs`'s `mod tests` (line 205):

```rust
    /// The roster built from the green tree is the same roster the C1
    /// CST walk built — same entries, same order, same spans. The C1
    /// walk is reproduced inline as the oracle rather than recorded as
    /// a snapshot, so it cannot drift from the real one.
    #[test]
    fn roster_matches_the_c1_cst_walk() {
        use crate::cst::TopKind;
        use crate::lexer::lex;
        use crate::parser::parse_cst;

        let cst = parse_cst(&lex(SOURCE).expect("lexes")).expect("parses");
        let mut expected: Vec<(String, Span, u32)> = Vec::new();
        for top in &cst.items {
            let TopKind::Namespace(ns) = &top.kind else {
                continue;
            };
            if ns.name != "std" {
                continue;
            }
            for body in &ns.items {
                let TopKind::Function(f) = &body.kind else {
                    continue;
                };
                if !f.exported {
                    continue;
                }
                expected.push((format!("std::{}", f.name), f.name_span, f.line));
            }
        }

        let actual: Vec<(String, Span, u32)> = roster()
            .iter()
            .map(|e| (e.full_path.clone(), e.name_span, e.decl_line))
            .collect();

        assert_eq!(actual, expected);
        assert_eq!(actual.len(), 11, "the embedded stdlib exports 11 routines");
    }
```

Append to `crates/post-machine/src/cli/driver.rs`'s `mod tests` (line 1009):

```rust
    /// The driver's pre-pass scan agrees with the C1 front end on both
    /// of the facts it reports, and still degrades to an empty scan on
    /// a source that does not parse.
    #[test]
    fn scan_source_matches_the_c1_front_end() {
        let src = "use std::goToEnd as end;\nnamespace ns { export inner() { right; } }\nvolatile main() {\n    helper() { left; }\n    @helper();\n}\n";
        let expected = {
            let tokens = crate::lexer::lex(src).expect("lexes");
            crate::parser::parse(&tokens).expect("parses")
        };
        let scan = scan_source(src);
        assert_eq!(
            scan.volatile,
            expected.functions.iter().any(|f| f.volatile)
        );
        assert!(scan.exports.iter().any(|e| e == "main"));
        assert!(scan.exports.iter().any(|e| e == "ns::inner"));
    }

    #[test]
    fn scan_source_degrades_on_a_broken_source() {
        let scan = scan_source("main() { right");
        assert!(!scan.volatile);
        assert!(scan.exports.is_empty());
    }
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p mtc-post-machine --lib stdlib::tests::roster_matches && cargo test -p mtc-post-machine --lib cli::driver::tests::scan_source`
Expected: PASS on the current code — both are oracle harnesses installed before the substitution, like Tasks 4 and 5. Record the baseline, then change the implementations.

- [ ] **Step 3: Rewrite `roster`**

```rust
/// Parses `SOURCE` once (lex → green parse → extraction, no hand
/// parsing) into the roster of exported routines in the `std` namespace
/// block. Filters the extracted `Program` rather than walking the tree
/// a second way: extraction is held struct-equal to the C1 lowering by
/// the corpus oracle, so this cannot drift from what the compiler sees.
/// `ns == ["std"]` is an exact match, not a prefix — a namespace nested
/// inside `std` was never part of the roster.
///
/// The green tree and every view over it are `Rc`-based and die inside
/// this initializer; only owned `RosterEntry` fields (String/Span/u32)
/// reach the `OnceLock`, which requires `Sync`. Do not cache a
/// `SyntaxNode` or a view here.
pub(crate) fn roster() -> &'static [RosterEntry] {
    static ROSTER: OnceLock<Vec<RosterEntry>> = OnceLock::new();
    ROSTER.get_or_init(|| {
        let green = crate::parser::parse_green(SOURCE).expect("the embedded stdlib parses");
        let program =
            crate::syntax::extract_program(&SyntaxNode::new_root(green), SOURCE);
        program
            .functions
            .iter()
            .filter(|f| f.exported && f.ns == ["std"])
            .map(|f| RosterEntry {
                full_path: format!("std::{}", f.name),
                name_span: f.name_span,
                decl_line: f.line,
            })
            .collect()
    })
}
```

Fix `stdlib/mod.rs`'s imports: `use crate::parser::{FnDoc, parse_cst};` loses `parse_cst` if nothing else in the file uses it; add `use mtc_core::syntax::SyntaxNode;`. Drop the now-unused `use crate::cst::TopKind;` and `use crate::lexer::lex;` from the file's top-level imports **only if** the test module does not need them at that scope — the test above imports its own.

- [ ] **Step 4: Rewrite `scan_source`**

In `crates/post-machine/src/cli/driver.rs`:

```rust
fn scan_source(text: &str) -> SourceScan {
    let Some(program) = crate::parser::parse_green(text)
        .ok()
        .map(|green| crate::syntax::extract_program(&SyntaxNode::new_root(green), text))
    else {
        return SourceScan::default();
    };
```

The body below is unchanged. Add `use mtc_core::syntax::SyntaxNode;` to the file's imports.

- [ ] **Step 5: Run the tests to verify they still pass**

Run: `cargo test -p mtc-post-machine --lib stdlib && cargo test -p mtc-post-machine --lib cli::driver`
Expected: PASS. The oracle tests now compare two implementations.

- [ ] **Step 6: Run the stdlib byte-identity gate**

Run:
```bash
cargo test -p mtc-post-machine --test stdlib_programs
cargo test -p mtc-post-machine --test golden_programs
cargo test -p mtc-post-machine --test cli_programs
cargo test -p mtc-post-machine --test build_driver
cargo test -p mtc-post-machine --test sweep
```
Expected: all PASS. The compiled stdlib is byte-compared at both opt levels by these suites; the roster feeds hover and the `deprecated-call` lint, and `sweep` pins the stdlib's own image size.

- [ ] **Step 7: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p mtc-core --no-default-features`
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add crates/post-machine/src/stdlib/mod.rs crates/post-machine/src/cli/driver.rs
git commit -m "feat(post-machine): stdlib roster and driver scan on the green tree"
```

---

### Task 7: Stale-claim sweep and the plan commit

Four doc comments and one CLAUDE.md paragraph now assert things that stopped being true in Tasks 4–6. Published `docs/` pages are expected **unchanged** — behavior did not move — but that expectation must be checked, not assumed.

**Files:**
- Modify: `crates/post-machine/src/parser.rs` (doc comments on `parse` line ~296 and `parse_cst` line ~306)
- Modify: `crates/post-machine/src/cst.rs` (module doc, lines 5–6)
- Modify: `CLAUDE.md` (the `### Pipeline and key types` two-parse-paths paragraph)
- Create: `docs/superpowers/plans/2026-08-20-c2-plan4-pm-compiler-front.md` (this file — committed here)

**Interfaces:**
- Consumes: nothing. Produces: nothing. Prose only.

- [ ] **Step 0: Close the nested-namespace coverage hole**

Task 6's review turned up a real gap: **no `.pmc` fixture anywhere in the corpus contains a
nested namespace block.** So `extract_items`' `ns`-accumulation path — the code that stamps a
function's namespace path, now on the production parse path for every `.pmc` compile — has zero
corpus coverage. It is not broken (verified: the fixture below passes all four oracles), it is
simply unchecked. A ten-line fixture closes it, and the corpus oracles do the rest with no test
code to write.

Create `crates/post-machine/tests/syntax/nested_ns.pmc`:

```pmc
// Nested-namespace corpus fixture (docs/pmt/language.md (namespaces)):
// the one shape no other `.pmc` fixture carries. Extraction accumulates a
// function's namespace path across NAMESPACE recursion and stamps it onto
// the definition it encounters; without a nested block, that accumulation
// never runs more than one level deep in any oracle. This file exercises
// two levels (`outer::inner`), a definition at each level, and a
// RE-OPENED top-level namespace — three blocks whose functions must all
// land in document order with their own distinct `ns` paths.
namespace outer {
    namespace inner {
        export deep() { right; }
    }
    export shallow() { left; }
}
namespace outer {
    export second() { mark; }
}
main() {
    @outer::inner::deep();
    @outer::shallow();
    @outer::second();
}
```

Run: `cargo test -p mtc-post-machine --test syntax_green`
Expected: PASS — 19 tests, the corpus now 13 files. Like `retok.pmc` this is a **regression pin,
not a red test**: it was validated against the built tools before this step shipped. If
`corpus_extraction_parity` fails on it, that is a genuine `ns`-stamping bug the fixture caught —
fix `crates/post-machine/src/syntax/extract.rs`, never the oracle, and report it prominently.

Commit this on its own, before the prose edits:

```bash
git add crates/post-machine/tests/syntax/nested_ns.pmc
git commit -m "test(post-machine): nested-namespace corpus fixture"
```

- [ ] **Step 1: Verify the published pages really are unchanged**

Run: `grep -rn "parse_cst\|lower_cst\|C1\|CST" docs/ README.md`
Expected: hits only in `docs/core.md`'s syntax-trees section (which describes the green framework, not the C1 path) and in `docs/superpowers/` internal artifacts. **If a published page under `docs/pmt/` or `README.md` describes the C1 CST as the compiler's parse path, correct it in this task** — that is the spec's "any page found describing the C1 internals is corrected in the same commit" rule.

Record what you found either way, so the next plan does not re-run the search blind.

- [ ] **Step 2: Correct `parse`'s doc comment**

`crates/post-machine/src/parser.rs`, above `pub fn parse`:

```rust
/// tokens → AST, via the one unified lossless CST. No longer the
/// compiler's path — `compiler::analyze` extracts from the green tree
/// (docs/core.md (syntax trees)) — this survives as the differential
/// oracle that extraction is held equal to, and as the parse behind the
/// optimizer's and IR's own unit tests. The signature is unchanged from
/// the pre-C1 parser.
```

- [ ] **Step 3: Correct `parse_cst`'s doc comment**

Replace its opening sentence (`/// tokens → lossless CST. Accepts either a `WithoutComments` stream (the compiler's path, no trivia) or a `WithComments` stream (fmt's path, comments interleaved).`) with:

```rust
/// tokens → lossless CST. Accepts either a `WithoutComments` stream (no
/// trivia) or a `WithComments` stream (fmt's path and the staged
/// pipeline's, comments interleaved). Not the compiler's path any more:
/// `compiler::analyze` extracts from the green tree, and
/// `compiler::analyze_staged` builds this alongside it only for the
/// `.pmc` language service.
```

- [ ] **Step 4: Correct `cst.rs`'s module doc**

Lines 5–6 read `//! `parse_cst` produces a [`Cst`] from a `WithComments` token stream, and //! a future `lower_cst` copies it into the existing [`crate::parser::Program`]`. `lower_cst` is not future and is not the compiler's path. Replace those two lines with:

```rust
//! `parse_cst` produces a [`Cst`] from a `WithComments` token stream,
//! and [`crate::parser::lower_cst`] copies it into the
//! [`crate::parser::Program`] shape. That pair is the differential
//! oracle the green-tree extraction is held equal to, plus fmt's and
//! the `.pmc` language service's remaining input — the compiler front
//! end reads the green tree instead (docs/core.md (syntax trees)).
```

Leave every other `lower_cst` reference in `cst.rs` alone: they describe what `lower_cst` does with each field, and all of that is still accurate.

- [ ] **Step 5: Update CLAUDE.md**

In `### Pipeline and key types`, the paragraph beginning **"Two parse paths coexist until the C2 cutover."** currently says `parse` is "the live one and every consumer still goes through it". Replace the whole paragraph with:

```markdown
**Two parse paths coexist until the C2 cutover.** The compiler front end
is the green one: `analyze`/`analyze_staged` run `lex_with(WithComments)`
→ `parse_green_from_tokens` → `syntax::extract_program`, and so do the
embedded stdlib's roster and the build driver's source scan. Lint follows
for free — `LintContext` carries tokens and the flattened AST, never a
CST. Still on the C1 path: `fmt` (`parse_cst` directly), the `.pmc`
language service (`StagedAnalysis.cst`, built alongside the green parse
until it migrates), and the optimizer/IR/codegen unit tests. `parse` and
`lower_cst` survive as the differential oracle: `text() == source`, and
`extract_program` struct-equal to `lower_cst(parse_cst(...))` across the
corpus. **When the cutover lands, this paragraph and the `parse` clause
above both change** — `lower_cst`, `parse_cst` and the C1 CST all go away
together.
```

Also update the `### Pipeline and key types` opening sentence, which still reads `parser.rs` (recursive descent; `parse` = `lower_cst ∘ parse_cst` over one lossless CST shared with fmt/LSP): keep the parenthetical but append `— fmt's and the LSP's path; the compiler's is `parse_green` + extraction`.

- [ ] **Step 6: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p mtc-core --no-default-features`
Expected: all green. Doc-comment edits can still break `cargo doc` intra-doc links — if clippy flags a broken link, fix the path rather than dropping the link.

- [ ] **Step 7: Run the newer clippy before any push**

Run: `cargo +1.97.0 clippy --workspace --all-targets -- -D warnings`
Expected: green. CI's clippy is newer than local stable and has failed locally-green pushes; this is the standing pre-push check.

- [ ] **Step 8: Commit**

```bash
git add crates/post-machine/src/parser.rs crates/post-machine/src/cst.rs CLAUDE.md docs/superpowers/plans/2026-08-20-c2-plan4-pm-compiler-front.md
git commit -m "docs(plan): C2 plan 4 + stale C1-path claims corrected"
```

---

---

### Task 8: `non-camel-case` stops suggesting a rename to the same name

Unrelated to C2 — a defect surfaced by Task 1's non-ASCII fixture and folded
in here at the maintainer's request. Sequenced last, after every C2 gate has
run, so the behavior change it makes is never mistaken for a substitution
regression.

`is_lower_camel` is ASCII (`is_ascii_lowercase` / `is_ascii_alphanumeric`)
while the language's identifier rule is Unicode-aware (`char::is_alphabetic`),
so a Cyrillic name is reported — correctly, and the docs page already says so.
The defect is the *suggestion*: `to_camel` only drops `_`, re-cases around the
dropped separators, and lowercases the first character, none of which changes
a name whose offending characters are outside ASCII. All three message sites
then tell the author to rename a name to itself. Verified against the built
`pmt` while this plan was written:

```
function 'шагВперёд' is not camelCase — rename to 'шагВперёд'
namespace 'ыHelpers' is not camelCase — rename to 'ыHelpers'
import binding 'шаг' is not camelCase — alias it: 'use their::шаг as шаг'
```

The verdict stays (it is the documented convention). Only the useless
suggestion goes.

**Files:**
- Modify: `crates/post-machine/src/lint/rules/non_camel_case.rs`
- Modify: `docs/pmt/lint.md` (the `### non-camel-case` entry, lines 153–162)
- Test: `crates/post-machine/src/lint/rules/non_camel_case.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks — this is independent of the C2 work.
- Produces: `fn suggestion(name: &str) -> Option<String>`, private to
  `non_camel_case.rs`. `to_camel` keeps its signature and `pub(super)`
  visibility. No rule code, span, or `fix` changes: `non-camel-case` has
  `fix: None` at all three sites and keeps it.

- [ ] **Step 1: Write the failing tests** (append to the `mod tests` at the bottom of `crates/post-machine/src/lint/rules/non_camel_case.rs`, beside the existing `messages()` helper)

```rust
    /// A non-ASCII name is still reported — the convention is ASCII by
    /// definition (docs/pmt/lint.md) — but the mechanical derivation
    /// cannot improve it, so the message must not advise a rename to
    /// the name the author already wrote.
    #[test]
    fn non_ascii_function_fires_without_a_tautological_suggestion() {
        let m = messages("export шагВперёд() { right; }\nmain() { @шагВперёд(); }\n");
        assert_eq!(
            m,
            vec![
                "function 'шагВперёд' is not camelCase — camelCase names are ASCII [a-z][a-zA-Z0-9]*"
            ]
        );
    }

    #[test]
    fn non_ascii_namespace_fires_without_a_tautological_suggestion() {
        let m = messages(
            "namespace ыHelpers { export inner() { right; } }\nmain() { @ыHelpers::inner(); }\n",
        );
        assert_eq!(
            m,
            vec![
                "namespace 'ыHelpers' is not camelCase — camelCase names are ASCII [a-z][a-zA-Z0-9]*"
            ]
        );
    }

    /// The import site keeps its actionable advice — aliasing IS the fix
    /// for a binding — but stops naming an alias identical to the
    /// binding it would replace.
    #[test]
    fn non_ascii_import_binding_advises_an_alias_without_naming_one() {
        let m = messages("use their::шаг;\nmain() { @шаг(); }\n");
        assert_eq!(
            m,
            vec![
                "import binding 'шаг' is not camelCase — alias it to an ASCII [a-z][a-zA-Z0-9]* name"
            ]
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-post-machine --lib lint::rules::non_camel_case`
Expected: three FAILs with these exact left-hand values (verified against the built `pmt`):

- `"function 'шагВперёд' is not camelCase — rename to 'шагВперёд'"`
- `"namespace 'ыHelpers' is not camelCase — rename to 'ыHelpers'"`
- `"import binding 'шаг' is not camelCase — alias it: 'use their::шаг as шаг'"`

The three pre-existing tests (`snake_case_function_fires_with_suggestion`,
`violating_import_binding_suggests_an_alias`, and the namespace one) must
still PASS — ASCII names keep their derived suggestions unchanged. If any of
them goes red, the fix is over-broad.

- [ ] **Step 3: Add the `suggestion` helper**

In `crates/post-machine/src/lint/rules/non_camel_case.rs`, add below `to_camel`:

```rust
/// How the messages name this rule's alphabet when they have no
/// derivable rename to offer.
const ASCII_CAMEL: &str = "camelCase names are ASCII [a-z][a-zA-Z0-9]*";

/// The mechanical rename for `name`, or `None` when the derivation
/// cannot improve on what the author already wrote. [`to_camel`] drops
/// `_`, capitalizes after each dropped `_`, and lowercases the first
/// character — none of which changes a name whose offending characters
/// lie outside ASCII. The language's identifier rule is Unicode-aware
/// while this rule's convention is ASCII (docs/pmt/lint.md
/// (non-camel-case)), so that case is reachable, and advising "rename
/// 'шаг' to 'шаг'" is worse than offering nothing.
fn suggestion(name: &str) -> Option<String> {
    let camel = to_camel(name);
    (camel != name).then_some(camel)
}
```

- [ ] **Step 4: Branch the three messages**

Function site:

```rust
        if !is_lower_camel(last) {
            let message = match suggestion(last) {
                Some(camel) => {
                    format!("function '{last}' is not camelCase — rename to '{camel}'")
                }
                None => format!("function '{last}' is not camelCase — {ASCII_CAMEL}"),
            };
            out.push(Diagnostic {
                code: "non-camel-case",
                span: f.name_span,
                message,
                fix: None,
            });
        }
```

Namespace site:

```rust
            if !is_lower_camel(&segment) {
                let message = match suggestion(&segment) {
                    Some(camel) => {
                        format!("namespace '{segment}' is not camelCase — rename to '{camel}'")
                    }
                    None => format!("namespace '{segment}' is not camelCase — {ASCII_CAMEL}"),
                };
                out.push(Diagnostic {
                    code: "non-camel-case",
                    span: f.name_span,
                    message,
                    fix: None,
                });
            }
```

Import site — the alias advice survives, only the derived alias goes:

```rust
        if !is_lower_camel(binding) {
            let message = match suggestion(binding) {
                Some(camel) => format!(
                    "import binding '{binding}' is not camelCase — alias it: 'use {} as {camel}'",
                    imp.full_path()
                ),
                None => format!(
                    "import binding '{binding}' is not camelCase — \
                     alias it to an ASCII [a-z][a-zA-Z0-9]* name"
                ),
            };
            out.push(Diagnostic {
                code: "non-camel-case",
                span: imp.span,
                message,
                fix: None,
            });
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mtc-post-machine --lib lint::rules::non_camel_case`
Expected: PASS — the three new tests and the three pre-existing ones.

- [ ] **Step 6: Run the lint suites**

Run: `cargo test -p mtc-post-machine --test lint_programs && cargo test -p mtc-post-machine --test error_code_docs`
Expected: PASS with no edits. No committed fixture uses a non-ASCII name, so
no existing expected message moves; `error_code_docs` is the registry↔docs
drift guard and the rule's *code* is untouched.

- [ ] **Step 7: Correct the docs claim**

`docs/pmt/lint.md` currently states flatly that "The message carries a
mechanically derived rename suggestion". That is now conditional. Replace the
`### non-camel-case` body's second and third sentences:

```markdown
Definition names the user owns — functions, namespaces, import
bindings — should be lowerCamelCase, the project's house style. The
message carries a mechanically derived rename suggestion where one
exists; an import binding's suggestion is an `as` alias. A name whose
offending characters lie outside ASCII has no derivable rename — the
derivation would hand back the same name — so those messages name the
convention's alphabet instead of suggesting anything. Report-only: a
rename is a multi-site edit, and renaming an exported function changes
its symbol name. The most opinionated rule in the set — `--allow
non-camel-case` is the escape hatch (note that non-ASCII identifiers,
which the language permits, do not satisfy the ASCII convention).
```

- [ ] **Step 8: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p mtc-core --no-default-features`
Expected: all green.

- [ ] **Step 9: Run the newer clippy**

Run: `cargo +1.97.0 clippy --workspace --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 10: Commit**

```bash
git add crates/post-machine/src/lint/rules/non_camel_case.rs docs/pmt/lint.md
git commit -m "fix(post-machine): non-camel-case stops suggesting a rename to the same name"
```

## Completion criteria for Plan 4

Named gates, not "the suite is green":

- **Every `.pmc` compile runs the green front end.** No production call site of `crate::parser::parse` remains in `crates/post-machine/src` outside `#[cfg(test)]` modules. Verify: `grep -rn "parser::parse\b\|parse(&tokens)" crates/post-machine/src --include=*.rs` returns only test-module hits.
- **PM-1 byte-identity, now proven through the green path.** `golden_programs` (derivation-first `.pmt` snapshots byte-compared) and `asm_volatile` (directive-free files assemble byte-identically) pass with no regeneration. `sweep` asserts termination outcomes and the step-limit trap; its corpus-wide instruction-count and image-size totals are a REPORT, not an assertion, so confirming they did not move requires a before/after comparison of two runs.
- **Compiled-stdlib byte-identity at both opt levels.** Covered by `golden_programs` + `cli_programs` + `stdlib_programs`; the roster change in Task 6 is the reason this is called out separately.
- **`-O0` bit-identity** and **the `brk` barrier** hold: `opt_equivalence` and `volatile_equivalence` pass unmodified.
- **Both differential oracles are alive and green:** `corpus_lossless_law` (`text() == source`) and `corpus_extraction_parity` (`extract_program == lower_cst ∘ parse_cst`), now over 13 corpus files including `retok.pmc` and `nested_ns.pmc`.
- **The staged tiers are unmoved:** all five `analyze_staged_*` tests pass **with no edits**, and the whole LSP suite passes with no edits.
- **The provenance law holds:** `corpus_token_provenance_law` and `lex_modes_agree_on_errors` pass.
- **No version space moved.** `PMC_LANG_VERSION` and `PM1_PMA_DIALECT_VERSION` (`crates/post-machine/src/`), `IR_VERSION` (`crates/post-machine/src/ir.rs`), the MO/MX/MT container versions (`crates/core/src/formats/`), and the `pmt.json` `project` schema (`crates/post-machine/src/project.rs`) are untouched. Verify against **this plan's base**, not `master` — the branch already carries 27 commits from plans 1–3:

  ```bash
  BASE=$(git rev-parse HEAD~<number-of-plan-4-commits>)   # the commit Task 1 started from
  git diff $BASE..HEAD --stat -- crates/core                       # must be empty
  git diff $BASE..HEAD -- crates/post-machine/src/ir.rs | grep IR_VERSION   # must be empty
  git diff $BASE..HEAD --stat -- crates/post-machine/src/project.rs        # must be empty
  ```
- **`crates/core` has a zero-line diff** across this plan's commits (the first command above).
- **Task 8's one intended behavior change is isolated:** the only moved observable in the whole plan is the `non-camel-case` message text for names with no derivable rename, in its own final commit. Every ASCII name keeps its exact previous message, pinned by the rule's three pre-existing tests passing unmodified.
- Quality gates: `cargo clippy --workspace --all-targets -- -D warnings` on local stable **and** on `+1.97.0`; `cargo fmt --check`; `cargo build -p mtc-core --no-default-features`.

## What this plan deliberately does not do

- **The `.pmc` LSP** keeps reading `StagedAnalysis.cst` (7 sites across `lsp/{tokens,navigate,complete,mod}.rs` plus `lsp/walk.rs`). Plan 5. It needs core's `token_at_offset` and preorder `descendants` — both deferred from plan 3 for exactly this reason — plus a line/col → offset inverse.
- **fmt** keeps calling `parse_cst`. Plan 6, gated on byte-identical output over every fmt fixture.
- **Cutover** — deleting `cst.rs`, `lower_cst`, `parse_cst`, `parse`, the `WithoutComments` lex mode and oracle (b) — is plan 7. Its known debt, catalogued here so it is not rediscovered: ~14 `#[cfg(test)]` modules in `optimizer/*`, `ir.rs`, `codegen.rs` and `parser.rs` call `parse` directly and all need rewriting; `extract.rs`'s doc comments cite `parser.rs:395`/`parser.rs:402` by line number and those references die with the functions.
- **The TM mirror** of all of the above comes after PM is finished.
