# C2 Plan 3: PM Views + Extraction Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Typed views over the `.pmc` green tree plus an extraction layer that rebuilds the C1 AST (`parser::Program`) from the tree — proven struct-equal to `lower_cst(parse_cst(...))` over the whole corpus, so later plans can migrate consumers onto views with the old path as a bit-for-bit oracle.

**Architecture:** Three layers. (1) Core red-tree navigation primitives (`ancestors`, sibling queries, `first_token`/`last_token`, `descendant_tokens`) — the call sites are this plan's own view derivations. (2) `views.rs`: `ast_node!`-declared typed views whose accessors mirror the parser's decisions (the contextual-keyword rule: the LAST `IDENT` before `(` in a function header is the name; earlier `IDENT`s are modifiers — `export() {}` is a function named `export`). (3) `extract.rs`: containers assembled from views exactly as `lower_cst` assembles them (ns stamping, hoisting, per-path imports, doc reduction), while item internals are NOT re-interpreted — each ITEM node's significant tokens are rebuilt into real `lexer::Token`s (exact line/col via `TextLineIndex`, so every span is identical by construction) and fed back through the existing `Parser::item()` via a `pub(crate)` shim. Reuse, never duplicate: `reduce_doc_run` and `parse_attr` are reused the same way. The corpus struct-equality oracle is the net.

**Tech Stack:** Rust; `mtc_core::syntax` (plan 1) + PM `syntax::{PmcKind, layout, parse_green}` (plan 2); no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md` — this plan implements §4.3 (views) and the extraction half of §4.4 for PM, plus spec §6.1's oracle (b). Consumers (§5) migrate in the next plan; cutover later still.

## Global Constraints

- Branch `feat/c2-green-tree` (commits authorized). Every commit: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build -p mtc-core --no-default-features` green.
- Zero new dependencies. Zero behavior change on existing paths — the ONE existing-code refactor (lexer doc-payload normalization extracted into a shared helper) must keep lexer output byte-identical, guarded by the full suite + corpus.
- **Reuse over duplication (binding ruling):** item internals, check arms, doc-run reduction, and attention attributes are NEVER re-implemented — they go through the existing parser code (`Parser::item` via shim, `reduce_doc_run`, `parse_attr`). Views interpret only what the C1 CST layer itself computed (headers, names, containers, trivia-derived facts).
- **The oracle is authoritative:** `extract_program(...)` must equal `lower_cst(parse_cst(...))` by `==` on `Program`, per corpus file. On mismatch, fix the extraction — never the oracle, never `lower_cst`. If you believe `lower_cst` itself is wrong, STOP and report BLOCKED (controller rules on it).
- The contextual-keyword trap is in scope: `export`, `use`, and `namespace` are legal function names; views mirror parser lookahead, never text-matching alone; the fixture set includes a function literally named `export`.
- Goldens/derivations remain derivation-first; commit style conventional with scope; never any AI/Claude attribution.

---

### Task 1: Core navigation primitives

**Files:**
- Modify: `crates/core/src/syntax/red.rs`
- Modify: `docs/core.md` (two sentences in the Syntax trees section)

**Interfaces:**
- Consumes: plan-1 red layer.
- Produces (all on `SyntaxNode` unless noted): `ancestors(&self) -> impl Iterator<Item = SyntaxNode>` (self's parent chain, nearest first, root last — NOT including self); `prev_sibling_or_token(&self) -> Option<SyntaxElement>`; `next_sibling_or_token(&self) -> Option<SyntaxElement>` (both also on `SyntaxToken`); `first_token(&self) -> Option<SyntaxToken>`; `last_token(&self) -> Option<SyntaxToken>`; `descendant_tokens(&self) -> impl Iterator<Item = SyntaxToken>` (document order, all depths). Deferred by ruling (call sites live in the consumer/LSP plan): `token_at_offset`, preorder node `descendants`.

- [ ] **Step 1: Write the failing tests** (append to red.rs's test module; reuse its existing `sample()` fixture and kind consts)

```rust
    #[test]
    fn ancestors_walk_to_the_root() {
        let root = sample();
        let list = root.children().next().expect("LIST child");
        let chain: Vec<SyntaxKind> = list.ancestors().map(|n| n.kind()).collect();
        assert_eq!(chain, vec![ROOT]);
        assert!(root.ancestors().next().is_none());
    }

    #[test]
    fn sibling_queries_walk_both_ways() {
        let root = sample();
        let list = root.children().next().expect("LIST child");
        // sample(): ROOT > [LIST, WS("\n")]
        let next = list.next_sibling_or_token().expect("has next");
        assert_eq!(next.kind(), WS);
        assert!(matches!(
            next,
            SyntaxElement::Token(ref t) if t.text() == "\n"
        ));
        let SyntaxElement::Token(ws) = next else { unreachable!() };
        let prev = ws.prev_sibling_or_token().expect("has prev");
        assert_eq!(prev.kind(), LIST);
        assert!(list.prev_sibling_or_token().is_none());
    }

    #[test]
    fn token_edges_and_descendant_tokens() {
        let root = sample();
        // sample() text: "f λx\n" — tokens f, " ", "λx" inside LIST; "\n" in ROOT.
        assert_eq!(root.first_token().expect("first").text(), "f");
        assert_eq!(root.last_token().expect("last").text(), "\n");
        let texts: Vec<String> = root
            .descendant_tokens()
            .map(|t| t.text().to_string())
            .collect();
        assert_eq!(texts, vec!["f", " ", "λx", "\n"]);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core syntax::red`
Expected: compile error — `ancestors` etc. not found.

- [ ] **Step 3: Implement**

Add an `index: u32` field to `NodeData` and to `SyntaxToken` (the element's position among the parent's children); `new_root` uses 0; `children_with_tokens` sets it from the enumeration position. Then:

```rust
    /// Parent chain, nearest first; does not include `self`.
    pub fn ancestors(&self) -> impl Iterator<Item = SyntaxNode> + '_ {
        std::iter::successors(self.parent(), SyntaxNode::parent)
    }

    /// The element immediately before this node among its parent's
    /// children, tokens included.
    pub fn prev_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let idx = self.0.index as usize;
        if idx == 0 {
            return None;
        }
        parent.children_with_tokens().nth(idx - 1)
    }

    /// The element immediately after this node among its parent's
    /// children, tokens included.
    pub fn next_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        parent.children_with_tokens().nth(self.0.index as usize + 1)
    }

    /// First token of the subtree, in document order.
    pub fn first_token(&self) -> Option<SyntaxToken> {
        self.children_with_tokens().find_map(|e| match e {
            SyntaxElement::Token(t) => Some(t),
            SyntaxElement::Node(n) => n.first_token(),
        })
    }

    /// Last token of the subtree, in document order.
    pub fn last_token(&self) -> Option<SyntaxToken> {
        let mut result = None;
        for e in self.children_with_tokens() {
            let candidate = match e {
                SyntaxElement::Token(t) => Some(t),
                SyntaxElement::Node(n) => n.last_token(),
            };
            if candidate.is_some() {
                result = candidate;
            }
        }
        result
    }

    /// Every token of the subtree, document order, all depths.
    pub fn descendant_tokens(&self) -> impl Iterator<Item = SyntaxToken> {
        let mut stack: Vec<SyntaxElement> = self
            .children_with_tokens()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        std::iter::from_fn(move || loop {
            match stack.pop()? {
                SyntaxElement::Token(t) => return Some(t),
                SyntaxElement::Node(n) => {
                    let mut children: Vec<SyntaxElement> =
                        n.children_with_tokens().collect();
                    children.reverse();
                    stack.extend(children);
                }
            }
        })
    }
```

Mirror `prev_sibling_or_token`/`next_sibling_or_token` on `SyntaxToken` (same bodies over `self.parent` + `self.index`). `first_token`/`last_token` returning `None` only for a node with no tokens anywhere beneath (possible: all-empty children). Add two sentences to `docs/core.md`'s Syntax trees section: navigation (ancestors, sibling queries, token edges, descendant tokens) exists on the red layer; core still interprets no kinds.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-core syntax` — all green (old + 3 new). Then the four workspace gates.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/syntax/red.rs docs/core.md
git commit -m "feat(core): red-tree navigation primitives"
```

---

### Task 2: PM typed views (`views.rs`)

**Files:**
- Create: `crates/post-machine/src/syntax/views.rs`
- Modify: `crates/post-machine/src/syntax/mod.rs` (add `mod views;` + re-exports)

**Interfaces:**
- Consumes: core `ast_node!`, `AstNode`, `child`/`children`/`token` helpers, Task 1's navigation; `PmcKind`.
- Produces (each an `ast_node!` view over its kind): `FileView: File`, `UseDeclView: UseDecl`, `UsePathView: UsePath`, `NamespaceView: Namespace`, `FunctionView: Function`, `DocRunView: DocRun`, `StatementView: Statement`, `LabelView: Label`, `ItemView: Item`. Accessors (exact signatures later tasks call):
  - `FileView::items(&self) -> impl Iterator<Item = TopView>` where `pub enum TopView { Use(UseDeclView), Namespace(NamespaceView), Function(FunctionView) }` (document order; trivia skipped).
  - `NamespaceView::name(&self) -> String` (second significant token — the IDENT after the `namespace` keyword IDENT), `::name_token(&self) -> SyntaxToken`, `::items(&self) -> impl Iterator<Item = TopView>`.
  - `UseDeclView::paths(&self) -> impl Iterator<Item = UsePathView>`.
  - `UsePathView::segments(&self) -> Vec<SyntaxToken>` (the path IDENTs, `as`-alias excluded), `::alias_token(&self) -> Option<SyntaxToken>` (the IDENT after the `as` marker), with the `as`-marker rule: among the view's IDENT tokens, an IDENT with text `"as"` that is neither first nor path-separated — concretely: segments are the IDENTs joined by `COLON_COLON`; if two IDENTs follow the last `::`-joined segment, the first is the literal `as` marker and the second is the alias. (`use as as as;` is thereby unambiguous: path `as`, marker `as`, alias `as`.)
  - `FunctionView::header(&self) -> FnHeader` where `pub struct FnHeader { pub name: SyntaxToken, pub has_volatile: bool, pub has_export: bool }` — **the contextual-keyword rule**: collect the IDENT tokens that are DIRECT children of the FUNCTION node before its first `L_PAREN` token; the LAST one is the name; among the earlier ones, text `"volatile"` sets `has_volatile` and text `"export"` sets `has_export` (mirror the parser's modifier order; a lone IDENT is always the name, so `export() {}` names a function `export`).
  - `FunctionView::doc_run(&self) -> Option<DocRunView>`, `::statements(&self) -> impl Iterator<Item = StatementView>`, `::nested(&self) -> impl Iterator<Item = FunctionView>` (direct children only).
  - `StatementView::labels(&self) -> impl Iterator<Item = LabelView>`, `::items(&self) -> impl Iterator<Item = ItemView>`.
  - `LabelView::number_token(&self) -> SyntaxToken`, `::colon_token(&self) -> SyntaxToken`.
  - `DocRunView` and `ItemView` expose only `syntax()` (extraction walks their tokens).

- [ ] **Step 1: Write the failing tests** (in `views.rs`; parse real snippets through `parse_green`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_green;
    use mtc_core::syntax::{AstNode, SyntaxNode};

    fn file(src: &str) -> FileView {
        FileView::cast(SyntaxNode::new_root(parse_green(src).expect("parses")))
            .expect("root is FILE")
    }

    #[test]
    fn use_paths_segments_and_alias() {
        let f = file("use std::goToEnd, std::goToBegin as gb;\n");
        let TopView::Use(u) = f.items().next().expect("one item") else {
            panic!("expected use decl");
        };
        let paths: Vec<UsePathView> = u.paths().collect();
        assert_eq!(paths.len(), 2);
        let seg: Vec<String> = paths[0]
            .segments()
            .iter()
            .map(|t| t.text().to_string())
            .collect();
        assert_eq!(seg, vec!["std", "goToEnd"]);
        assert!(paths[0].alias_token().is_none());
        assert_eq!(paths[1].alias_token().expect("alias").text(), "gb");
    }

    #[test]
    fn function_header_contextual_keywords() {
        let f = file("export main() { right; }\nexport() { left; }\n");
        let headers: Vec<FnHeader> = f
            .items()
            .map(|i| match i {
                TopView::Function(func) => func.header(),
                _ => panic!("expected functions"),
            })
            .collect();
        assert_eq!(headers[0].name.text(), "main");
        assert!(headers[0].has_export);
        // A lone IDENT before `(` is always the NAME — `export` here is
        // a function named export, not a modifier.
        assert_eq!(headers[1].name.text(), "export");
        assert!(!headers[1].has_export);
    }

    #[test]
    fn statements_labels_and_nesting() {
        let f = file("main() {\n1: 2: right, left;\ng() { right; }\n}\n");
        let TopView::Function(func) = f.items().next().expect("fn") else {
            panic!("expected function");
        };
        let stmts: Vec<StatementView> = func.statements().collect();
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0].labels().count(), 2);
        assert_eq!(stmts[0].items().count(), 2);
        assert_eq!(func.nested().count(), 1);
    }
}
```

(Verify `export() { ... }` and the exact snippets are accepted by the real parser before relying on them — if one is rejected, substitute a valid equivalent and record it. `left` as an item name: if the grammar has no `left` builtin, use whatever the corpus uses — read `crates/post-machine/tests/golden/*.pmc` for real item vocabulary and adjust snippets accordingly; the test's structural assertions are what matter.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p mtc-post-machine syntax::views`; compile error expected.

- [ ] **Step 3: Implement.** Views via `ast_node!`; accessors via `children`/`token` helpers + Task 1 navigation. `TopView`/`FnHeader` as plain enums/structs in `views.rs`. `FileView::items`/`NamespaceView::items` share one helper over `children()` filtering the three kinds. Re-export the public names from `syntax/mod.rs`.

- [ ] **Step 4: Run to verify pass** — module tests + full crate + workspace gates.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/syntax/
git commit -m "feat(post-machine): typed views over the pmc green tree"
```

---

### Task 3: Retokenization + reuse shims

**Files:**
- Create: `crates/post-machine/src/syntax/extract.rs` (retokenization half)
- Modify: `crates/post-machine/src/lexer.rs` (extract `normalize_doc_payload`)
- Modify: `crates/post-machine/src/parser.rs` (add the `pub(crate)` reuse shims)
- Modify: `crates/post-machine/src/syntax/mod.rs` (wire `mod extract;`)

**Interfaces:**
- Consumes: Task 1's `descendant_tokens`; `TextLineIndex`; `lexer::{Token, TokenKind}`.
- Produces:
  - lexer.rs: `pub(crate) fn normalize_doc_payload(raw_after_sigil: &str) -> String` — strips ONE leading space if present, otherwise verbatim; the lexer's own DocLine/AttentionLine payload construction now calls it (behavior-identical refactor — find the payload-normalization site(s) in the lexer and route them through the helper; byte-identical output enforced by the full suite).
  - extract.rs: `fn sig_tokens(node: &SyntaxNode, index: &TextLineIndex) -> Vec<Token>` — every non-trivia descendant token rebuilt as a real `Token` (kind from `PmcKind` + text: `Ident(text)`, `Number(text.parse().expect("lexed digits"), text)`, punctuation kinds 1:1, `DocLine/AttentionLine(normalize_doc_payload(&text[1..]))`; `line`/`col` from `index.line_col(range.start)`; `len = text.chars().count() as u32`), with a synthetic `Eof` token appended at the slice end's line/col.
  - parser.rs shims (they live in parser.rs so private methods stay private): `pub(crate) fn reparse_item(tokens: &[Token], in_group: bool) -> Item` and `pub(crate) fn reparse_doc_items(tokens: &[Token]) -> Vec<DocRunItem>` — each constructs a `Parser` over the slice (`pos: 0`, empty sets/comments, `sink: None`) and calls the existing production (`item(in_group)`; for doc items, per-token conversion reusing `parse_attr` for attention attributes — mirror how `doc_run()` builds `DocRunKind::Doc`/`Attention` from DocLine/AttentionLine tokens, WITHOUT its binding logic). Both `expect(...)` on error: extraction runs only on trees that already parsed, so failure is a bug.

- [ ] **Step 1: Write the failing tests** (in `extract.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{lex, lex_with, LexMode};
    use crate::parser::{parse_cst, parse_green};
    use crate::syntax::{ItemView, StatementView};
    use mtc_core::syntax::{child, children, SyntaxNode, TextLineIndex};

    /// Retokenized items re-parse to the EXACT C1 Item — spans included.
    #[test]
    fn reparsed_item_equals_the_c1_item() {
        let src = "main() {\n1: right(2), goto 1;\n}\n";
        // C1 side: the statement's items straight from parse_cst.
        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let c1_items = /* navigate cst.items[0] FUNCTION → body[0] Statement → items,
                          mapping CommaItem::item — see cst.rs shapes */;
        // Green side: retokenize each ITEM node and re-parse.
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let index = TextLineIndex::new(src);
        /* walk FILE → FUNCTION → STATEMENT via views, then for each ItemView:
           crate::parser::reparse_item(&sig_tokens(view.syntax(), &index), in_group) */
        // assert_eq! each pair.
    }
}
```

Write the test fully (the comments above mark navigation you write out with the real view/CST APIs — both sides are available; `goto 1` and `right(2)` cover `Goto` + a parenthesised successor. If the snippet isn't valid grammar, substitute from the corpus vocabulary and record it).

- [ ] **Step 2: Run to verify failure** — compile error on missing functions.

- [ ] **Step 3: Implement** per the Produces block. The `TokenKind` inverse mapping is a match on `PmcKind` (trivia kinds and `File`+node kinds `unreachable!` — `sig_tokens` filters trivia before mapping).

- [ ] **Step 4: Run to verify pass** — `cargo test -p mtc-post-machine syntax::extract`, then FULL crate suite (the lexer refactor must change nothing) + workspace gates.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/
git commit -m "feat(post-machine): retokenization + parser reuse shims for extraction"
```

---

### Task 4: Extraction — functions, doc runs, statements

**Files:**
- Modify: `crates/post-machine/src/syntax/extract.rs`

**Interfaces:**
- Consumes: Tasks 2-3; `parser::{Function, Statement, Label, FnDoc}`; `reduce_doc_run` (make it `pub(crate)` in parser.rs if not already reachable).
- Produces: `fn extract_function(view: &FunctionView, index: &TextLineIndex) -> Function` — mirrors `lower_function` (parser.rs:395+) exactly: body statements in order (each `Statement { labels, items, line, span }` — labels from `LabelView` tokens (`value` = number text parsed, `written` = number text, `span` = number start → colon end via `index.span`), items via `reparse_item`, `line`/`span` per the C1 conventions — read `StatementCst::span`'s doc: first token through `;` end); nested functions hoisted into `nested` with empty ns (exactly as `lower_function` does); `doc` = `reduce_doc_run(&reparse_doc_items(...))` over the DOC_RUN's tokens; `name`/`name_span`/`line`/`col` from the header name token; `exported`/`volatile` — read the parser's `function()` production for the exact stamping rule (incl. top-level `main` auto-export) and mirror it; the ns field is stamped by Task 5's recursion, `local` stays `false` (flatten computes it) — both exactly as `lower_cst` behaves.

- [ ] **Step 1: Write the failing test**

```rust
    /// Green-extracted functions equal lower_cst's — the oracle at
    /// function granularity, on a snippet with every function feature.
    #[test]
    fn extracted_function_equals_lowered() {
        let src = "? doc line\n! caution\nexport main() {\n1: right;\nh() { right; }\n}\n";
        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let lowered = crate::parser::lower_cst(&cst);
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        let index = TextLineIndex::new(src);
        let f = /* FileView over root → first TopView::Function */;
        let extracted = extract_function(&f, &index);
        assert_eq!(extracted, lowered.functions[0]);
    }
```

(Fill the navigation; `Function` derives PartialEq. If the snippet trips grammar rules — e.g. attention-line placement — adjust from corpus vocabulary and record.)

- [ ] **Step 2: Run to verify failure** — `extract_function` missing.

- [ ] **Step 3: Implement** per Produces. Where a C1 convention is unclear, the authority order is: `lower_cst`/`lower_function`'s code → the C1 CST field docs (`cst.rs`) → the parser production. Never guess: read, mirror, cite the line in a code comment only where the rule is non-obvious from the mirror itself.

- [ ] **Step 4: Run to verify pass** + full crate + workspace gates.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/syntax/extract.rs
git commit -m "feat(post-machine): function-level extraction from views"
```

---

### Task 5: Extraction — program assembly

**Files:**
- Modify: `crates/post-machine/src/syntax/extract.rs`
- Modify: `crates/post-machine/src/syntax/mod.rs` (re-export `extract_program`)

**Interfaces:**
- Consumes: Task 4; `parser::{Program, Import}`.
- Produces: `pub fn extract_program(root: &SyntaxNode, source: &str) -> Program` — builds the `TextLineIndex` once, then mirrors `lower_items` (parser.rs:356+): walk `FileView::items` recursively through namespaces accumulating the ns path; per `UsePathView` push an `Import { path, alias, line, ns, span }` (`path` = segment texts, `alias` = alias token text, `line` = first segment's line, `span` = first segment start → LAST SEGMENT end — the alias-exclusive convention from `Import`'s doc); per function `extract_function` + ns stamp on the returned value (top-level only — nested keep empty ns, as `lower_items`/`lower_function` do).

- [ ] **Step 1: Write the failing test**

```rust
    /// Whole-program equality on a namespaced, aliased, nested snippet.
    #[test]
    fn extracted_program_equals_lowered() {
        let src = "use std::goToEnd as ge;\nnamespace n {\nf() { right; }\n}\nmain() { right; }\n";
        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let expected = crate::parser::lower_cst(&cst);
        let root = SyntaxNode::new_root(parse_green(src).unwrap());
        assert_eq!(extract_program(&root, src), expected);
    }
```

- [ ] **Step 2: Run to verify failure** — `extract_program` missing.

- [ ] **Step 3: Implement** per Produces.

- [ ] **Step 4: Run to verify pass** + full crate + workspace gates.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/syntax/
git commit -m "feat(post-machine): program extraction from the green tree"
```

---

### Task 6: The corpus struct-equality oracle + contextual fixtures

**Files:**
- Create: `crates/post-machine/tests/syntax/contextual.pmc`
- Modify: `crates/post-machine/tests/syntax_green.rs`

**Interfaces:**
- Consumes: `extract_program`, the existing corpus walker.
- Produces: spec §6.1 oracle (b) for PM — the standing gate every consumer-migration commit in the next plan runs against.

- [ ] **Step 1: Write the fixture** — `contextual.pmc`: a VALID program containing functions literally named `export` and `use` (and `namespace` if the grammar's reserved-name rules allow it — check `is_reserved_definition_name` in parser.rs; drop any name it reserves and record which), called or not as the grammar requires, plus an aliased import whose alias shadows nothing. Verify validity by parse before relying on it.

- [ ] **Step 2: Add the oracle test**

```rust
#[test]
fn corpus_extraction_parity() {
    use mtc_post_machine::lexer::{lex_with, LexMode};
    use mtc_post_machine::parser::parse_cst;
    use mtc_post_machine::syntax::extract_program;
    for (path, source) in corpus() {
        let expected = mtc_post_machine::parser::lower_cst(
            &parse_cst(&lex_with(&source, LexMode::WithComments).expect("lexes"))
                .expect("parses"),
        );
        let root = SyntaxNode::new_root(
            parse_green(&source).unwrap_or_else(|e| panic!("{}: {e:?}", path.display())),
        );
        assert_eq!(
            extract_program(&root, &source),
            expected,
            "{}: extraction parity",
            path.display()
        );
    }
}
```

Bump the corpus floor 10 → 11 with the comment updated. A parity failure is an extraction bug: fix extraction, never the oracle (BLOCKED if you believe `lower_cst` is wrong).

- [ ] **Step 3: Run** — `cargo test -p mtc-post-machine --test syntax_green` all green; then the four workspace gates.

- [ ] **Step 4: Commit**

```bash
git add crates/post-machine/tests/
git commit -m "test(post-machine): corpus extraction-parity oracle + contextual-keyword fixture"
```

---

### Task 7: Docs + plan commit

**Files:**
- Modify: `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md` (§4.2 hedge — the deferred plan-2 ruling)
- Add: `docs/superpowers/plans/2026-08-18-c2-plan3-pm-views-extraction.md` (this file)

- [ ] **Step 1: Amend §4.2's opening** — "`parse_cst` is reimplemented as the same recursive-descent logic emitting green nodes through `TreeBuilder`." → "`parse_cst`'s recursive descent emits green nodes through `TreeBuilder` (woven behind an optional sink during the migration; the C1-CST-building half is deleted at cutover, leaving green emission as the parser's only construction)."

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md docs/superpowers/plans/2026-08-18-c2-plan3-pm-views-extraction.md
git commit -m "docs(plan): C2 plan 3 + spec §4.2 cutover hedge"
```

---

## Completion criteria for Plan 3

- `extract_program(parse_green(src), src) == lower_cst(parse_cst(lex_with(src)))` for all 11 corpus files, by `==` on `Program` — spans, docs, ns paths, hoisting, aliases, items, everything.
- All navigation primitives shipped with call sites in this plan's own code; `token_at_offset`/`descendants` recorded as deferred to the consumer plan.
- Zero behavior change: lexer refactor byte-identical (full suite), parser shims additive, all pre-existing tests green.
- Next plan (consumer migration) can point flatten/lint/LSP at views/extraction with this oracle as the per-commit net.
