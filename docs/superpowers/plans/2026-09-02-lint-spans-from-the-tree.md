# `.tmc` Lint Spans From the Green Tree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `.tmc` lint layer's quickfix span helpers stop
reconstructing declaration extents from token adjacency and read them
as node ranges off the green tree the front end already builds — the
C2 whole-branch review follow-up tracked as "tmt lint: retain the green
tree in Analysis and make the quickfix span helpers node-range
queries".

**Architecture:** `compiler::Analysis` retains the `Rc<GreenNode>`
`analyze` builds (the staged analysis already does). `LintContext`
carries the tree's root and a `TextLineIndex` over the source and
DROPS the comment-free `tokens` field: every one of its six readers
becomes a range-containment query — the innermost node of the wanted
kind containing the anchor position, then that node's own range (a
declaration or statement to delete) or a token pair inside it (an `as`
clause, a `debugger` marker, a map arrow). The `significant_tokens`
filter both entry paths ran to fill `tokens` goes with the field: no
helper indexes off a neighbouring token any more, so a comment can no
longer void or truncate a span — the comment guard in `run_rules` is
the one thing that decides a comment-bearing shape, and it decides it
the same way as before (finding kept, fix withheld).

**Tech Stack:** `crates/turing-machine/src/syntax/views.rs` (the
`.tmc` typed views: `AlphabetView`, `ReuseView`, `GraftView`,
`BindView`, `RuleView`, `SymMapView`), core `TextLineIndex::{offset,
span}`, the red-tree `children_with_tokens`/`descendant_tokens`
primitives.

**Spec:** the tracker issue's own text is the spec; the precedent is
`lsp/quickfix.rs::enclosing_body` (a range query over `RootView`).

## Global Constraints

- Behavior-preserving on every existing pin: `tests/lint_quickfix_comments.rs`
  (applied texts + withheld fixes), `tests/lint_fix_comment_guard.rs`,
  every rule's own unit tests, and the LSP quickfix tests stay green
  UNCHANGED — they are the behavioral gate.
- Both entry paths — batch `lint()` and the editor's `did_update` —
  build the SAME context shape; CLI≡editor parity is structural.
- No `tokens`-style reader survives: after this arc `LintContext` has
  no comment-free token field, and `grep ctx.tokens` is empty.
- Gates: workspace tests, clippy `-D warnings`, `cargo fmt --check`,
  the no_std core build (untouched by this arc, run anyway).

## Design

**The query.** `spans.rs` keeps one generic finder:
`innermost<V: AstNode>(root: &SyntaxNode, offset: u32) -> Option<V>` —
descend from the root through the child whose range contains `offset`,
remembering the last node that casts to `V`. Anchors are the spans the
resolved module already keeps (a name, a target, a rule, a pair
literal), converted with `TextLineIndex::offset`.

**The six helpers, as tree queries** (all take `&LintContext`):
- `decl_span::<AlphabetView>` / `::<ReuseView>` / `::<GraftView>` /
  `::<BindView>` — ONE function, `decl_span<V>(ctx, anchor: Span) ->
  Option<Span>`: the innermost `V` containing the anchor's start, its
  whole range. A retro-wrapped doc run, `export`/`entry`, and the
  terminating `}`/`;` are all inside the node by construction
  (`syntax/mod.rs`'s module doc), so "the doc goes with what it
  documents" is no longer a walk-back — it is the node.
- `as_clause_span(ctx, graft_span)`: the innermost `GraftView`, its
  `as_name()` token, and the `)` reached by stepping back over trivia
  and the `as` ident from it: `Span { start: rparen.end, end: name.end }`.
- `marker_span(ctx, rule_span)`: the innermost `RuleView`, its direct
  IDENT child with text `debugger`, and the next non-trivia sibling's
  start: `Span { start: marker.start, end: next.start }`.
- `arrow_span(ctx, pair)`: the innermost `SymMapView` containing the
  pair's source literal, and among its descendant tokens the ARROW
  between `pair.src.span().end` and `pair.dst.span().start`.

**What the port removes.** `Analysis.tokens`' "rules that walk by
adjacency must filter first" contract, the `significant_tokens` call
in both entry paths, the 40-line load-bearing comment in `lint()`, the
`LintContext.tokens` field and its doc, `back_over_doc_run`, the brace
counting, and the pin file's "the filter is load-bearing" argument.
The pin FIXTURES stay: they now pin the tree-derived spans directly.

---

### Task 1: the helpers as tree queries (RED first)

**Files:** modify `crates/turing-machine/src/lint/rules/spans.rs`
(rewrite), `crates/turing-machine/src/compiler.rs` (`Analysis.green`),
`crates/turing-machine/src/lint/mod.rs` (`LintContext { root, index }`,
no `tokens`; `lint()` builds both), `crates/turing-machine/src/lsp/mod.rs`
(the context construction; hoist `line_index`).

**Produces:**
```rust
pub(crate) struct LintContext<'a> { resolved, diagnostics, program,
    pub root: &'a SyntaxNode, pub index: &'a TextLineIndex, pub comment_tokens: &'a [Token] }
pub(crate) fn decl_span<V: AstNode>(ctx: &LintContext, anchor: Span) -> Option<Span>
pub(crate) fn as_clause_span(ctx: &LintContext, graft_span: Span) -> Option<Span>
pub(crate) fn marker_span(ctx: &LintContext, rule_span: Span) -> Option<Span>
pub(crate) fn arrow_span(ctx: &LintContext, pair: &MapPair) -> Option<Span>
```

- [x] Tests in `spans.rs` (a `context(src)` helper over `analyze`):
  `decl_span_is_the_declaration_node_comment_included` — an alphabet
  with a comment between `alphabet` and its name still yields the FULL
  declaration span (the adjacency helper returned `None` here: this is
  the discriminating case); `decl_span_takes_the_bound_doc_run`;
  `as_clause_span_runs_from_the_paren_to_the_name`;
  `marker_span_ends_at_the_next_token`; `arrow_span_is_the_pairs_arrow`.
- [x] Run `cargo test -p mtc-turing-machine spans` — FAILS to compile
  (no `green`, no `root`, no generic `decl_span`).
- [x] Implement: `Analysis.green`; `LintContext` reshaped; `spans.rs`
  rewritten; both entry paths build `root`/`index`; the eight rule
  call sites switch to the new helpers (`unused_alphabet`,
  `unused_graph`, `unused_routine`, `unused_binding`,
  `unused_graft_instance`, `unused_graft_name`, `leftover_debugger`,
  `dead_map_pair`), their local helpers deleted.
- [x] Run `cargo test -p mtc-turing-machine` — green, every existing
  pin unchanged.

### Task 2: the words follow the code

**Files:** `tests/lint_quickfix_comments.rs` (header), `lint/mod.rs`
(`lint()` comment, `Analysis.tokens` doc), `lsp/mod.rs` (step-4
comment), `tests/lint_fix_comment_guard.rs` (last header paragraph),
`CLAUDE.md` (the "FIVE quickfix helpers" sentence), `docs/tmt/lint.md`
(quickfix availability: one sentence on where spans come from).

- [x] Rewrite each to state the new mechanism; no `Task N`/`spec §`
  notation; forge-agnostic in the published pages.
- [x] Full gates.
