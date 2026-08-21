# C2 Plan 5: The `.pmc` Language Service on Typed Views Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the `.pmc` language service's six CST walks onto typed views over the green tree, then retire `StagedAnalysis.cst` and the interim double parse that plan 4 left behind.

**Architecture:** Every one of the six walks is a **node-level containment descent** — "which function/namespace/use-path contains this position" — never a token-level query. So the whole migration needs exactly one new core primitive: `TextLineIndex::offset`, the inverse of `line_col`, converting the LSP's 1-based char `Pos` into the byte offset the green tree indexes by. Container structure comes from views; a statement's *item internals* (label references, a call's name) come from `syntax::extract_statement`, the same retokenization-through-the-real-parser path extraction already uses — never re-implemented, per the arc's binding reuse ruling.

**Tech Stack:** Rust; `mtc_core::syntax` (plan 1) + PM `syntax::{views, extract, parse_green}` (plans 2–4); no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md` — this plan implements §5.2's LSP clause. Deferred: §5.1 fmt (plan 6), the §6.1 cutover deletions (plan 7), the TM mirror (after).

## Global Constraints

- Branch `feat/c2-green-tree`. **Commits are authorized for this run** — commit at the end of each task with the message that task gives. One commit per task; no squashing, amending, or rebasing.
- **A deviation from the line above was accepted once, in Task 4, as a one-off** — an implementer amended an unreviewed, unpushed commit whose SHA had not yet been recorded, so neither of the rule's purposes (a reviewer's base cannot shift; ledger SHAs keep resolving) was at risk. That acceptance does NOT widen the rule. Rewriting a constraint after a violation so the violation becomes legal is how constraints turn decorative; the reviewer who flagged it was right to, and the ground should not have moved under the finding. Ask before deviating again.
- Every commit green on: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build -p mtc-core --no-default-features`. **The toolchain is pinned** in `rust-toolchain.toml` to 1.98.0, so plain `cargo clippy` IS the gate CI runs — there is no side-toolchain step any more.
- Zero new dependencies.
- **`crates/core` gets exactly one additive method** — `TextLineIndex::offset` in Task 1. Any other `crates/core` diff in this plan is a design smell: report BLOCKED.
- **Zero observable behavior change.** Same completions, same definitions, same hovers, same semantic tokens, same document symbols, same degradation tiers, same spans. The LSP suite passing unmodified is the gate.
- **No version space moves.** `PMC_LANG_VERSION`, `PM1_PMA_DIALECT_VERSION`, `IR_VERSION`, MO/MX/MT, the `pmt.json` project schema — all untouched.
- **Reuse over duplication (binding arc ruling, plan 3).** Item internals, check arms, doc-run reduction and attention attributes are NEVER re-implemented. They go through the existing parser code — here, through `syntax::extract_statement`, which already wraps `reparse_item` and the `in_group` rule. Adding a `label_refs`-like enumeration to `views.rs` would violate this: do not.
- Do not delete anything on the C1 path beyond what Task 7 names. `parse_cst`, `lower_cst`, `parse` and `cst.rs` all survive this plan — fmt still uses `parse_cst`, and `parse`/`lower_cst` are still the differential oracle. Their removal is plan 7's.
- **The `.pma` language service is out of scope entirely.** `lsp/pma/` walks core's *asm* CST, a different tree, untouched by this arc. `overlay.rs` likewise: its only CST is `mtc_core::asm::cst` for `.pma` exports.
- Commit style: conventional with scope. **Never** any AI/Claude attribution in a commit message, comment, or doc.
- Code comments cite durable pages by page + parenthetical keyword (`docs/core.md (syntax trees)`, `docs/lsp.md (staged analysis)`). Never a `docs/superpowers/` spec or plan from code; never "spec §N".

## What is already migration-free (do not touch)

Confirmed by reading every call site — stated so no task wastes effort re-deriving it:

- **`walk::span_contains`** is pure `Pos` arithmetic. It keeps working unchanged and every migrated walk still uses it at the `Span` boundaries.
- **`walk::label_refs`** operates on `crate::parser::Item`, the AST type — not the CST. The label-reference enumeration feeding both `navigate.rs` and `tokens.rs` needs no migration at all.
- **`complete.rs`'s `state.tokens` use** (`complete.rs:67`) is the lexer token stream, not the CST. Untouched.
- **`token_at_offset` and a preorder `descendants`** — both deferred from plan 3 "for the consumer plan" — turn out **not to be needed**. All six walks descend children testing node containment; none asks "what token is under the cursor". Do not add speculative core surface.

---

### Task 1: `TextLineIndex::offset` — the inverse of `line_col`

**Files:**
- Modify: `crates/core/src/syntax/line_index.rs`
- Test: `crates/core/src/syntax/line_index.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::diagnostics::Pos` (already imported in this file's `Span` import — add `Pos` to it).
- Produces: `pub fn offset(&self, pos: Pos) -> u32` on `TextLineIndex`. Every later task converts an incoming LSP `Pos` to a byte offset with it, exactly once per request, then compares against `TextRange`s.

**Why it is total (returns `u32`, not `Option<u32>`).** By the time a `Pos` reaches a `LanguageService` it has already been clamped by `crate::lsp::position::pos_from_lsp` (`crates/core/src/lsp/position.rs:95`): a line past end-of-file becomes the last line's end, a column past a line's end clamps to that line's end, and a UTF-16 offset inside a surrogate pair snaps to the character's start. So the inverse never sees garbage — and the commonest cursor position an editor sends, one column past a line's last character, must resolve to that line's end rather than fail. This method mirrors that clamping so the two conversions cannot disagree.

- [ ] **Step 1: Write the failing tests** (append to `line_index.rs`'s `mod tests`)

```rust
    #[test]
    fn offset_inverts_line_col_at_every_char_boundary() {
        // Round-trip over a text with a trailing newline, a blank line,
        // and a multi-byte character — the three shapes where a naive
        // byte/char mix-up shows.
        let text = "ab\n\nжc\n";
        let idx = TextLineIndex::new(text);
        for (offset, _) in text.char_indices().chain(std::iter::once((text.len(), '\0'))) {
            let offset = offset as u32;
            let (line, col) = idx.line_col(offset);
            assert_eq!(
                idx.offset(Pos { line, col }),
                offset,
                "round trip at byte {offset} ({line}:{col})"
            );
        }
    }

    #[test]
    fn offset_clamps_a_column_past_the_line_end_to_that_line_end() {
        // The end-of-line cursor: `pos_from_lsp` hands us col == chars+1,
        // and anything beyond must land on the same place — the newline's
        // own offset, never the next line.
        let idx = TextLineIndex::new("ab\ncd\n");
        assert_eq!(idx.offset(Pos { line: 1, col: 3 }), 2, "one past 'b' is the \\n");
        assert_eq!(idx.offset(Pos { line: 1, col: 99 }), 2, "far past clamps the same");
        assert_eq!(idx.offset(Pos { line: 2, col: 3 }), 5);
    }

    #[test]
    fn offset_clamps_a_line_past_the_end_to_end_of_text() {
        let idx = TextLineIndex::new("ab\ncd\n");
        assert_eq!(idx.offset(Pos { line: 99, col: 1 }), 6);
        // Line 3 exists (the empty line after the trailing newline).
        assert_eq!(idx.offset(Pos { line: 3, col: 1 }), 6);
    }

    #[test]
    fn offset_handles_an_empty_document() {
        let idx = TextLineIndex::new("");
        assert_eq!(idx.offset(Pos { line: 1, col: 1 }), 0);
        assert_eq!(idx.offset(Pos { line: 9, col: 9 }), 0);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-core --lib syntax::line_index`
Expected: FAIL to compile — `no method named 'offset' found for struct 'TextLineIndex'`.

- [ ] **Step 3: Implement**

Add `Pos` to the diagnostics import at the top of `crates/core/src/syntax/line_index.rs`:

```rust
use crate::diagnostics::{Pos, Span};
```

Then add the method to `impl TextLineIndex`, directly after `line_col`:

```rust
    /// Byte offset of a 1-based (line, char-column) position — the
    /// inverse of [`TextLineIndex::line_col`], and total where
    /// `line_col` is partial.
    ///
    /// It clamps exactly the way `crate::lsp::position::pos_from_lsp`
    /// clamps, because that function is what produces every `Pos` a
    /// language service ever sees: a line past the last one yields the
    /// end-of-text offset, and a column at or past a line's last
    /// character yields that line's end — the newline's own offset, or
    /// end-of-text on the final line. The end-of-line column (one past
    /// the last character) is the commonest cursor position an editor
    /// sends, so it resolves rather than failing; that is why this
    /// returns `u32` and not `Option<u32>`.
    pub fn offset(&self, pos: Pos) -> u32 {
        let text_len = self.text.len() as u32;
        if pos.line == 0 || pos.line as usize > self.line_starts.len() {
            return text_len;
        }
        let line_ix = (pos.line - 1) as usize;
        let line_start = self.line_starts[line_ix];
        // The line's end excludes its own `\n`; the last line ends at
        // end-of-text. `new` splits on `\n` only, so a `\r` stays part
        // of the line content — matching `line_col`'s own view.
        let line_end = self
            .line_starts
            .get(line_ix + 1)
            .map_or(text_len, |&next| next - 1);
        let mut offset = line_start;
        let mut col = 1u32;
        for ch in self.text[line_start as usize..line_end as usize].chars() {
            if col >= pos.col {
                return offset;
            }
            offset += ch.len_utf8() as u32;
            col += 1;
        }
        offset
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-core --lib syntax::line_index`
Expected: PASS — the four new tests plus the file's existing ones.

- [ ] **Step 5: Document it on the durable page**

`docs/core.md`'s syntax-trees section describes `TextLineIndex` as offset → line/column. Add one sentence naming the inverse and its clamping contract, in that section's existing voice. Verify the section exists first (`grep -n "TextLineIndex" docs/core.md`); if the page does not describe `TextLineIndex` at all, say so in your report and skip this step rather than inventing a section.

- [ ] **Step 6: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p mtc-core --no-default-features`
Expected: all green. This is the plan's ONLY `crates/core` change — check `git diff --stat -- crates/core` shows just `line_index.rs` (and `docs/core.md` outside the crate).

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/syntax/line_index.rs docs/core.md
git commit -m "feat(core): TextLineIndex::offset, the clamping inverse of line_col"
```

---

### Task 2: `walk.rs` on views, and the document retains its green tree

The shared enumeration layer, plus the state change every later task depends on: `StagedAnalysis` and `DocState` start carrying the green tree.

**Files:**
- Modify: `crates/post-machine/src/lsp/walk.rs`
- Modify: `crates/post-machine/src/compiler.rs` (`StagedAnalysis`, `analyze_staged`)
- Modify: `crates/post-machine/src/lsp/mod.rs` (`DocState`, the `analyze_staged` call site around line 552)
- Modify: `crates/post-machine/src/syntax/extract.rs` (visibility only)
- Modify: `crates/post-machine/src/lsp/navigate.rs` (`label_reference_at` ~278 and `label_span` ~297 move here whole; the `definition` call site at ~74 gets the adapter)
- Modify: `crates/post-machine/src/lsp/complete.rs` (two call-site adapters, ~614 and ~752)
- Modify: `crates/post-machine/src/syntax/mod.rs` (re-export)
- Test: `crates/post-machine/src/lsp/walk.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `TextLineIndex::offset` (Task 1); `crate::syntax::{FileView, NamespaceView, FunctionView, TopView, StatementView, LabelView}`; `mtc_core::syntax::{AstNode, SyntaxNode, TextLineIndex}`.
- Produces:
  - `pub(super) fn enclosing_function_chain(file: &FileView, offset: u32) -> Vec<FunctionView>` — outermost first, same contract as before: namespaces are walked but never added, empty when `offset` is in no function.
  - `pub(super) fn function_labels(function: &FunctionView, index: &TextLineIndex) -> Vec<Label>` — the function's OWN labels in source order, never descending into nested children. Returns owned `Label`s (not an iterator of references) because each is built through extraction.
  - `pub(crate) fn extract_statement(view: &StatementView, index: &TextLineIndex) -> Statement` — **a visibility change only**, re-exported as `crate::syntax::extract_statement`.
  - `StagedAnalysis.green: Option<Rc<GreenNode>>` and `DocState.green: Option<Rc<GreenNode>>` — `Some` exactly when `cst` is, i.e. whenever the parse succeeded.

**The `Rc` question, settled before it bites.** `Rc<GreenNode>` is `!Send`/`!Sync`, and `DocState` lives in the language server's document map. Step 1 verifies the server loop is single-threaded before anything else in this task — if it is not, the whole retention design is wrong and the task is BLOCKED, not worked around.

- [ ] **Step 1: Verify nothing requires `DocState` to be `Send`**

The failure mode is NOT a spawned thread — it is a `Send` bound on the trait or on
the collection the server stores services in. A thread-spawn grep would pass a
codebase that still refuses to compile, so check the bound directly:

```bash
grep -n "trait LanguageService" -A 4 crates/core/src/lsp/mod.rs
grep -rn "dyn LanguageService\|: Send\|+ Send" crates/core/src/lsp/mod.rs crates/core/src/lsp/server.rs
```

Expected, and already verified when this plan was written: `pub trait LanguageService`
carries **no** `Send` bound, and the server passes `&mut [&mut dyn LanguageService]` —
plain trait objects. The only `Send` in those files is
`panic_message(payload: &(dyn std::any::Any + Send))`, which is `catch_unwind`'s
payload type and unrelated. So an `Rc<GreenNode>` field is sound.

Record the actual output in your report. **If a `Send` bound has appeared on the
trait or the service collection since, STOP and report BLOCKED** — retention would
not compile, and the tree would have to be rebuilt per request instead.

- [ ] **Step 2: Expose the statement extraction**

In `crates/post-machine/src/syntax/extract.rs`, change `fn extract_statement` to `pub(crate) fn extract_statement` (the `extract` module itself is private — the re-export below is what makes it reachable) and extend its doc comment with a sentence saying why it is crate-visible:

```rust
/// …existing doc text…
///
/// `pub(crate)` because the `.pmc` language service needs one
/// statement's item internals — label references, a call's name — and
/// must get them the same way extraction does, through the parser's own
/// production (docs/lsp.md (semantic tokens)). Re-deriving that
/// enumeration over views instead would duplicate grammar the parser
/// already owns.
```

In `crates/post-machine/src/syntax/mod.rs`, add it to the existing `pub use extract::…` line:

```rust
pub use extract::{extract_program, extract_statement};
```

- [ ] **Step 3: Write the failing tests** (append to `walk.rs`'s `mod tests`)

```rust
    use crate::parser::parse_green;
    use mtc_core::syntax::{AstNode, SyntaxNode, TextLineIndex};

    use crate::syntax::FileView;

    fn file(src: &str) -> (FileView, TextLineIndex) {
        let root = SyntaxNode::new_root(parse_green(src).expect("parses"));
        (
            FileView::cast(root).expect("root is FILE"),
            TextLineIndex::new(src),
        )
    }

    #[test]
    fn chain_is_outermost_first_and_descends_into_nested() {
        let src = "ns() {\n    right;\n}\nouter() {\n    inner() {\n        left;\n    }\n    mark;\n}\n";
        let (file, index) = file(src);
        // Byte offset of the `left;` inside `inner`.
        let at = index.offset(mtc_core::diagnostics::Pos { line: 6, col: 9 });
        let chain = enclosing_function_chain(&file, at);
        let names: Vec<String> = chain.iter().map(|f| f.header().name.text().to_string()).collect();
        assert_eq!(names, vec!["outer".to_string(), "inner".to_string()]);
    }

    #[test]
    fn chain_is_empty_outside_every_function() {
        let src = "use std::goToEnd;\nmain() {\n    right;\n}\n";
        let (file, index) = file(src);
        let at = index.offset(mtc_core::diagnostics::Pos { line: 1, col: 3 });
        assert!(enclosing_function_chain(&file, at).is_empty());
    }

    #[test]
    fn namespaces_are_walked_but_never_enter_the_chain() {
        let src = "namespace ns {\n    inside() {\n        right;\n    }\n}\n";
        let (file, index) = file(src);
        let at = index.offset(mtc_core::diagnostics::Pos { line: 3, col: 9 });
        let chain = enclosing_function_chain(&file, at);
        let names: Vec<String> = chain.iter().map(|f| f.header().name.text().to_string()).collect();
        assert_eq!(names, vec!["inside".to_string()]);
    }

    #[test]
    fn function_labels_are_own_scope_only() {
        let src = "outer() {\n    1: right;\n    inner() {\n        2: left;\n    }\n    3: mark;\n}\n";
        let (file, index) = file(src);
        let at = index.offset(mtc_core::diagnostics::Pos { line: 6, col: 5 });
        let chain = enclosing_function_chain(&file, at);
        let outer = chain.first().expect("inside outer");
        let values: Vec<u32> = function_labels(outer, &index).iter().map(|l| l.value).collect();
        assert_eq!(values, vec![1, 3], "inner's label 2 is a separate scope");
    }
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p mtc-post-machine --lib lsp::walk`
Expected: FAIL to compile — `enclosing_function_chain` still takes `&[TopItem]` and `Pos`.

- [ ] **Step 5: Rewrite the two walkers**

Replace `enclosing_function_chain`, `push_deepest_nested` and `function_labels` in `crates/post-machine/src/lsp/walk.rs`. Keep `span_contains` and `label_refs` exactly as they are — they are already CST-free. Update the module doc's `use` list accordingly.

```rust
/// The enclosing function CHAIN at `offset`, outermost first: the
/// top-level function containing `offset`, then its nested descendant
/// containing `offset`, as deep as `offset` still lands inside one.
/// Namespace blocks are walked but never themselves added to the chain —
/// only a function's own extent does that. Empty when `offset` isn't
/// inside any function at all. A caller that only wants the innermost
/// enclosing function takes `.pop()`; a caller that needs every
/// enclosing level (qualified-name reconstruction, hoisted nested defs)
/// walks the whole `Vec`.
///
/// Offsets, not `Pos`: the green tree indexes by byte range
/// (docs/core.md (syntax trees)), so a request's position is converted
/// once by `TextLineIndex::offset` and every containment test below is a
/// range comparison.
pub(super) fn enclosing_function_chain(file: &FileView, offset: u32) -> Vec<FunctionView> {
    fn descend(items: impl Iterator<Item = TopView>, offset: u32) -> Vec<FunctionView> {
        for item in items {
            match item {
                TopView::Namespace(ns) => {
                    if ns.syntax().text_range().contains(offset) {
                        let chain = descend(ns.items(), offset);
                        if !chain.is_empty() {
                            return chain;
                        }
                    }
                }
                TopView::Function(f) => {
                    if f.syntax().text_range().contains(offset) {
                        let mut chain = vec![f.clone()];
                        push_deepest_nested(&f, offset, &mut chain);
                        return chain;
                    }
                }
                TopView::Use(_) => {}
            }
        }
        Vec::new()
    }
    descend(file.items(), offset)
}

/// Descends into `f`'s own nested definitions as long as `offset` stays
/// inside one, pushing each one reached onto `chain`.
fn push_deepest_nested(f: &FunctionView, offset: u32, chain: &mut Vec<FunctionView>) {
    for nested in f.nested() {
        if nested.syntax().text_range().contains(offset) {
            chain.push(nested.clone());
            push_deepest_nested(&nested, offset, chain);
            return;
        }
    }
}

/// Every label `function` declares in its OWN statements, in source
/// order — function-scoped, never descending into nested children (a
/// nested function's labels are a separate scope, reached only by
/// walking to that nested function's own chain entry first, via
/// [`enclosing_function_chain`]).
///
/// Owned `Label`s, not borrowed: each is built through
/// `crate::syntax::extract_statement`, the parser's own production, so a
/// label's `value` and `span` are the exact ones the compiler sees —
/// never a re-derivation over token text (a leading-zero label like
/// `007` would not survive one).
pub(super) fn function_labels(function: &FunctionView, index: &TextLineIndex) -> Vec<Label> {
    function
        .statements()
        .flat_map(|stmt| extract_statement(&stmt, index).labels)
        .collect()
}
```

Then, in `crates/post-machine/src/lsp/navigate.rs`, migrate the two functions that
consume those walkers directly. They cannot be adapted by signature alone — both bodies
read CST fields a `FunctionView` does not have — so they move here WHOLE rather than
being half-changed and left for Task 3:

```rust
/// The label value referenced at `pos`, plus the reference's own span
/// (the origin), if `pos` sits on one of `function`'s own reference
/// spans — `walk::label_refs`' shared enumeration over each comma-group
/// item, first hit wins. Only `function`'s OWN statements are examined —
/// its nested children are a separate label scope, reached only by
/// `walk::enclosing_function_chain` descending into them for a `pos`
/// that lands there.
///
/// The items come from `crate::syntax::extract_statement`, the parser's
/// own production, so `label_refs` sees exactly the `Item` the compiler
/// sees.
fn label_reference_at(
    function: &FunctionView,
    index: &TextLineIndex,
    pos: Pos,
) -> Option<(u32, Span)> {
    for stmt in function.statements() {
        for item in extract_statement(&stmt, index).items {
            for (value, span) in label_refs(&item).into_iter().flatten() {
                if span_contains(span, pos) {
                    return Some((value, span));
                }
            }
        }
    }
    None
}

/// `value`'s label declaration span within `function`'s OWN statements
/// (labels are function-scoped — never searched in nested children or
/// enclosing scopes), via `walk::function_labels`' shared scan.
fn label_span(function: &FunctionView, index: &TextLineIndex, value: u32) -> Option<Span> {
    function_labels(function, index)
        .into_iter()
        .find(|label| label.value == value)
        .map(|label| label.span)
}
```

Fix `walk.rs`'s imports: drop `crate::cst::{BodyKind, FunctionCst, TopItem, TopKind}`, keep `crate::parser::{CheckArm, Item, Label, Successor}`, and add `crate::syntax::extract_statement`, `crate::syntax::{FileView, FunctionView, TopView}`, `mtc_core::syntax::{AstNode, TextLineIndex}`.

- [ ] **Step 6: Adapt the four existing call sites so the crate compiles**

Changing `walk.rs`'s signatures breaks exactly **four** call sites, in **two** files.
`tokens.rs` is not among them — it imports only `label_refs` and `span_contains`,
neither of which changes. Verify that claim before you start:
`grep -rn "enclosing_function_chain\|function_labels" crates/post-machine/src/lsp/ | grep -v walk.rs`

This task leaves every one of them compiling and green with a mechanical adapter;
Tasks 3 and 5 then replace the surrounding logic. The adapter is the same shape each
time — build the view and the index from the retained tree, convert the position once:

```rust
    let green = state.green.as_ref()?;              // or `else { return … }` at a non-Option site
    let root = SyntaxNode::new_root(Rc::clone(green));
    let file = FileView::cast(root).expect("root is FILE");
    let index = TextLineIndex::new(&state.text);
    let offset = index.offset(pos);
```

then `enclosing_function_chain(&file, offset)` and `function_labels(&function, &index)`.

The four sites, each keeping its own existing fallback behavior:

- `navigate.rs:74` (`definition`) — `state.cst.as_ref()?` becomes the block above, then
  `.pop()` yields a `FunctionView` that the two functions migrated in Step 5 already
  accept. Nothing is left half-changed here.
- `complete.rs:614` (the chain leg) — `if let Some(cst) = &state.cst` becomes
  `if let Some(green) = &state.green`, then the block above.
- `complete.rs:752` (`label_candidates`) — `let Some(cst) = &state.cst else { return Vec::new(); }`
  becomes the same on `state.green`, preserving the empty-list fallback exactly.

**Do not commit a non-compiling tree, and do not use `todo!()`.** If an adapter turns
out to need more than the block above, that is a finding: report it rather than
inventing a larger change here.

- [ ] **Step 6b: Run the walk tests**

Run: `cargo test -p mtc-post-machine --lib lsp::walk && cargo build -p mtc-post-machine`
Expected: PASS and a clean build.

- [ ] **Step 7: Retain the green tree in the document state**

In `crates/post-machine/src/compiler.rs`, add the field to `StagedAnalysis` next to `cst`:

```rust
    /// Green syntax tree of the current text (docs/core.md (syntax
    /// trees)); `None` when lexing or parsing failed — the same tier as
    /// `cst`. The `.pmc` language service's position walks index by byte
    /// range against this tree.
    pub green: Option<Rc<GreenNode>>,
```

Add `use std::rc::Rc;` and `use mtc_core::syntax::GreenNode;` to `compiler.rs` if absent (check first — `SyntaxNode` is already imported there).

In `analyze_staged`, the green tree is currently consumed by `SyntaxNode::new_root(green)` and dropped. Clone the `Rc` before consuming it and thread it into all four `StagedAnalysis` constructions that follow the parse — the lex-failure and parse-failure returns keep `green: None`:

```rust
    let green_retained = Some(Rc::clone(&green));
```

placed immediately after the successful `parse_green_from_tokens` match, then `green: green_retained` (or `green: green_retained.clone()` where a later branch also needs it — the returns diverge, so a plain move works in each, exactly as `cst` does).

In `crates/post-machine/src/lsp/mod.rs`, add the matching `DocState` field with the same doc, and populate it from `staged.green` at the `analyze_staged` call site (around line 552, beside `cst: staged.cst`).

- [ ] **Step 8: Confirm nothing moved**

Run: `cargo build -p mtc-post-machine && cargo test -p mtc-post-machine --lib compiler && cargo test -p mtc-post-machine --lib lsp`
Expected: builds; all `analyze_staged` tier tests pass **unmodified** — adding a field must not disturb them — and the whole LSP suite passes **unmodified** too. The adapters of Step 6 changed how each site reaches the tree, never what it answers.

- [ ] **Step 9: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p mtc-core --no-default-features`
Expected: all green, the whole LSP suite included and unmodified.

- [ ] **Step 10: Commit**

```bash
git add crates/post-machine/src/lsp/walk.rs crates/post-machine/src/lsp/mod.rs crates/post-machine/src/compiler.rs crates/post-machine/src/syntax/extract.rs crates/post-machine/src/syntax/mod.rs
git commit -m "feat(post-machine): lsp walk primitives on views, document retains its green tree"
```

---

### Task 3: `navigate.rs` — use-path resolution and the hover target

Task 2 already migrated `label_reference_at` and `label_span` (they consume the walkers
it rewrote, and could not be adapted by signature alone). What remains here is
`use_path_at` — the one walk with a span subtlety of its own — and the hover call site.

**Files:**
- Modify: `crates/post-machine/src/lsp/navigate.rs` (`use_path_at` ~318, the hover target site ~392)
- Test: `crates/post-machine/src/lsp/navigate.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `walk::{enclosing_function_chain, function_labels, span_contains, label_refs}` (Task 2), `crate::syntax::extract_statement`, `DocState.green`, `TextLineIndex::offset`.
- Produces: no new public API. `use_path_at(file: &FileView, index: &TextLineIndex, offset: u32) -> Option<(String, Span)>`.

**Why `use_path_at` takes an offset where Task 2's `label_reference_at` takes a `Pos`.** `label_reference_at` matches a cursor against *reference spans* carried by AST `Item`s, which are `Span`s, not byte ranges, so it keeps `span_contains(span, pos)`. `use_path_at` tests node containment against the tree, so an offset is its natural currency.

- [ ] **Step 1: Write the failing test** (append to `navigate.rs`'s `mod tests`)

```rust
    /// Go-to-definition on a label reference still finds the label's own
    /// declaration in the same function — the walk that moved from the
    /// C1 CST to views. `007` also pins that the label VALUE survives
    /// extraction: a token-text re-derivation would either hand back
    /// `007` (unparseable as a value) or lose the written form.
    #[test]
    fn label_reference_resolves_to_its_declaration_after_the_view_migration() {
        const SRC: &str = "main() {\n    007: right;\n    goto 007;\n}\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, SRC);

        // The `007` inside `goto 007;` — skip past `"goto "`.
        let pos = pos_after(SRC, "goto 007", 5);
        let target = service
            .definition(URI, pos)
            .expect("resolves to the label declaration");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span.start, Pos { line: 2, col: 5 });
    }
```

This module's test fixture idiom, already in the file: `PmcLanguageService::new()`,
then `service.did_update(URI, SRC)`, then a position built with `pos_after` /
`pos_at`. Copy the shape from `local_call_resolves_to_the_top_level_definitions_name_span`
(`navigate.rs`, in the same `mod tests`).

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mtc-post-machine --lib lsp::navigate`
Expected: FAIL to compile, because Task 2 changed `enclosing_function_chain`'s signature and this file still calls the old one.

- [ ] **Step 3: Rewrite `use_path_at`**

For `use_path_at`, keep its long doc comment verbatim — every word of it is still true — and replace only the body and signature:

```rust
fn use_path_at(file: &FileView, index: &TextLineIndex, offset: u32) -> Option<(String, Span)> {
    fn descend(
        items: impl Iterator<Item = TopView>,
        index: &TextLineIndex,
        offset: u32,
    ) -> Option<(String, Span)> {
        for item in items {
            match item {
                TopView::Namespace(ns) => {
                    if ns.syntax().text_range().contains(offset)
                        && let Some(result) = descend(ns.items(), index, offset)
                    {
                        return Some(result);
                    }
                }
                TopView::Use(use_decl) => {
                    for path in use_decl.paths() {
                        let range = path.syntax().text_range();
                        if range.contains(offset) {
                            let joined = path
                                .segments()
                                .iter()
                                .map(|t| t.text().to_string())
                                .collect::<Vec<_>>()
                                .join("::");
                            return Some((joined, index.span(range)));
                        }
                    }
                }
                TopView::Function(_) => {}
            }
        }
        None
    }
    descend(file.items(), index, offset)
}
```

**One span subtlety to get right:** the C1 `UsePath.span` is alias-EXCLUSIVE — first segment start through last segment end, never covering an `as` alias (`extract.rs::extract_import` documents this). `UsePathView::syntax().text_range()` covers the whole USE_PATH node, alias included. Build the span from the segments the way `extract_import` does instead:

```rust
                            let segments = path.segments();
                            let first = segments.first().expect("USE_PATH always carries at least one segment");
                            let last = segments.last().expect("USE_PATH always carries at least one segment");
                            let span = index.span(mtc_core::syntax::TextRange::new(
                                first.text_range().start,
                                last.text_range().end,
                            ));
```

and test containment against that same alias-exclusive range, not the node's, so a cursor on the alias does not resolve to the path.

- [ ] **Step 4: Update the two call sites**

At `definition` (~line 72) and the hover target site (~line 392), replace `state.cst.as_ref()?` with the green tree, converting the position once:

```rust
    let green = state.green.as_ref()?;
    let root = SyntaxNode::new_root(Rc::clone(green));
    let file = FileView::cast(root).expect("root is FILE");
    let index = TextLineIndex::new(&state.text);
    let offset = index.offset(pos);
```

then `enclosing_function_chain(&file, offset).pop()`, `label_reference_at(&function, &index, pos)`, `label_span(&function, &index, value)`, and `use_path_at(&file, &index, offset)`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p mtc-post-machine --lib lsp::navigate`
Expected: PASS — the new test and every pre-existing `navigate` test, **unmodified**. If a pre-existing test needs editing to go green, the migration changed behavior: STOP and report it rather than adjusting the test.

- [ ] **Step 6: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/post-machine/src/lsp/navigate.rs
git commit -m "feat(post-machine): navigate walks the green tree"
```

---

### Task 4: `tokens.rs` — semantic tokens

**Files:**
- Modify: `crates/post-machine/src/lsp/tokens.rs` (`semantic_tokens` ~42, `walk_items` ~71, `walk_function` ~98, `walk_statement` ~122, `walk_item` ~150, `emit_use_path`)
- Test: `crates/post-machine/src/lsp/tokens.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `DocState.green`, `TextLineIndex`, `crate::syntax::extract_statement`, `crate::syntax::{FileView, FunctionView, TopView, UsePathView}`, `walk::label_refs`.
- Produces: no new API. `walk_items(items: impl Iterator<Item = TopView>, index: &TextLineIndex, resolutions: &BTreeMap<Span, &Resolution>, state: &DocState, out: &mut Vec<SemToken>)` and the matching `walk_function`/`walk_statement` over views.

**Two facts that make this simpler than it looks.** First, `semantic_tokens` already sorts its output (`out.sort_by_key(|token| token.span.start)`), so the interleaving of statements and nested definitions that the C1 `body: Vec<BodyItem>` preserved is NOT load-bearing here — emitting all statements then all nested functions produces the same sorted stream. Do not add an interleaved view accessor. Second, the `BTreeMap<Span, &Resolution>` is keyed by a call's `name_span`, and extraction is span-identical to the C1 lowering by construction (pinned by `corpus_extraction_parity`), so the existing key match keeps working untouched.

- [ ] **Step 1: Write the failing test** (append to `tokens.rs`'s `mod tests`)

```rust
    /// A nested definition's own name surfaces and the whole stream
    /// stays sorted — the property `semantic_tokens` asserts internally.
    /// Pins that emitting statements and nested definitions from two
    /// separate view iterators, rather than one interleaved body list,
    /// changes nothing, because the output is sorted.
    ///
    /// `inner` declares its OWN `2:` and gotos it: a `goto` across the
    /// function boundary would fatal `ir::lower` with an undefined
    /// label, `analysis` would be `None`, and `semantic_tokens` would
    /// return `None` before reaching anything this test is about.
    #[test]
    fn nested_definition_tokens_survive_the_view_migration() {
        const SRC: &str = "outer() {\n    1: right;\n    inner() {\n        2: left;\n        goto 2;\n    }\n    goto 1;\n}\nmain() { @outer(); }\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, SRC);

        let tokens = service.semantic_tokens(URI).expect("analysis succeeds");
        assert!(
            tokens.windows(2).all(|p| p[0].span.start <= p[1].span.start),
            "stream must be sorted by span start"
        );
        let declared: Vec<Pos> = tokens
            .iter()
            .filter(|t| t.token_type == TOKEN_TYPE_FUNCTION)
            .map(|t| t.span.start)
            .collect();
        assert!(
            declared.contains(&Pos { line: 3, col: 5 }),
            "inner's declaration name must be emitted, got {declared:?}"
        );
    }
```

Copy the service/fixture shape from `rich_fixture_yields_the_exact_absolute_token_stream`
(`tokens.rs`, same `mod tests`): `PmcLanguageService::new()`, `did_update(URI, SRC)`,
then `service.semantic_tokens(URI)`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mtc-post-machine --lib lsp::tokens`
Expected: FAIL to compile — `walk_items` still takes `&[TopItem]`.

- [ ] **Step 3: Rewrite the walkers**

```rust
pub(super) fn semantic_tokens(state: &DocState) -> Option<Vec<SemToken>> {
    let analysis = state.analysis.as_ref()?;
    let green = state.green.as_ref()?;
    let root = SyntaxNode::new_root(Rc::clone(green));
    let file = FileView::cast(root).expect("root is FILE");
    let index = TextLineIndex::new(&state.text);
    // …the existing `resolutions` BTreeMap construction, unchanged…
    let mut out = Vec::new();
    walk_items(file.items(), &index, &resolutions, state, &mut out);
    out.sort_by_key(|token| token.span.start);
    // …the existing non-overlap debug_assert, unchanged…
```

```rust
fn walk_items(
    items: impl Iterator<Item = TopView>,
    index: &TextLineIndex,
    resolutions: &BTreeMap<Span, &Resolution>,
    state: &DocState,
    out: &mut Vec<SemToken>,
) {
    for item in items {
        match item {
            TopView::Use(use_decl) => {
                for path in use_decl.paths() {
                    emit_use_path(&path, index, state, out);
                }
            }
            TopView::Namespace(ns) => {
                out.push(SemToken {
                    span: index.span(ns.name_token().text_range()),
                    token_type: TOKEN_TYPE_NAMESPACE,
                    modifiers: MODIFIER_DECLARATION,
                });
                walk_items(ns.items(), index, resolutions, state, out);
            }
            TopView::Function(f) => walk_function(&f, index, resolutions, state, out),
        }
    }
}

/// One function definition — top-level or nested — plus its own body.
/// Statements and nested definitions come from two separate view
/// iterators rather than one interleaved list; the caller sorts the
/// finished stream, so source interleaving is not load-bearing here.
fn walk_function(
    f: &FunctionView,
    index: &TextLineIndex,
    resolutions: &BTreeMap<Span, &Resolution>,
    state: &DocState,
    out: &mut Vec<SemToken>,
) {
    out.push(SemToken {
        span: index.span(f.header().name.text_range()),
        token_type: TOKEN_TYPE_FUNCTION,
        modifiers: MODIFIER_DECLARATION,
    });
    for stmt in f.statements() {
        let extracted = extract_statement(&stmt, index);
        for label in &extracted.labels {
            out.push(SemToken {
                span: label_def_span(label.span, &state.text),
                token_type: TOKEN_TYPE_NUMBER,
                modifiers: MODIFIER_DECLARATION,
            });
        }
        for item in &extracted.items {
            walk_item(item, resolutions, state, out);
        }
    }
    for nested in f.nested() {
        walk_function(&nested, index, resolutions, state, out);
    }
}
```

`walk_item`, `number_reference`, `label_def_span` and `emit_call_name` are unchanged — they already take AST `Item`s and `Span`s. `walk_statement` disappears, its two loops folded into `walk_function` above; delete it.

`emit_use_path` reads two things off the C1 `UsePath`: `path.path` (the segment
strings) and `path.span.start` (the first segment's position). Both come off the view:

```rust
fn emit_use_path(
    path: &UsePathView,
    index: &TextLineIndex,
    state: &DocState,
    out: &mut Vec<SemToken>,
) {
    let segments = path.segments();
    let texts: Vec<String> = segments.iter().map(|t| t.text().to_string()).collect();
    let full_path = texts.join("::");
    let default_library = texts.first().map(String::as_str) == Some("std")
        && std_enabled(state)
        && !overlay_owns(state, &full_path);
    let borrowed: Vec<&str> = texts.iter().map(String::as_str).collect();
    let start = index
        .span(
            segments
                .first()
                .expect("USE_PATH always carries at least one segment")
                .text_range(),
        )
        .start;
    emit_path_segments(&borrowed, start, default_library, out);
}
```

`emit_path_segments`, `std_enabled` and `overlay_owns` are unchanged.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mtc-post-machine --lib lsp::tokens`
Expected: PASS — the new test plus every pre-existing `tokens` test, **unmodified**. These include the legend drift guard; if it moves, something structural broke.

- [ ] **Step 5: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/post-machine/src/lsp/tokens.rs
git commit -m "feat(post-machine): semantic tokens walk the green tree"
```

---

### Task 5: `complete.rs` — completion contexts

**Files:**
- Modify: `crates/post-machine/src/lsp/complete.rs` (`enclosing_ns_path` ~380, the `ns_path`/chain block ~599–625, `label_candidates` ~749)
- Test: `crates/post-machine/src/lsp/complete.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `walk::{enclosing_function_chain, function_labels}`, `DocState.green`, `TextLineIndex`, `crate::syntax::{FileView, TopView}`.
- Produces: no new API. `enclosing_ns_path(file: &FileView, offset: u32) -> Vec<String>`.

**The degradation contract to preserve exactly.** Each of the three sites has a documented no-tree fallback and they are NOT the same: `ns_path` falls back to `[]` (top-level scope), the nested-defs leg is *skipped, not substituted*, and `label_candidates` returns an empty list rather than a hardcoded one. Keep each fallback as written, now keyed on `state.green` instead of `state.cst`.

- [ ] **Step 1: Write the failing test** (append to `complete.rs`'s `mod tests`)

```rust
    /// A nested function's own labels are the `goto` candidates, and
    /// outer's are not — the two walks that moved to views, exercised
    /// together in one namespaced, nested fixture.
    ///
    /// The fixture PARSES: `label_candidates` reads the tree, so a
    /// half-written `goto ` with no label would fail the parse and the
    /// feature would correctly return nothing, testing the fallback
    /// rather than the walk. The cursor instead sits on the `2` of a
    /// complete `goto 2;`, which is where a real client asks.
    #[test]
    fn goto_candidates_come_from_the_innermost_function_after_the_view_migration() {
        const SRC: &str = "namespace ns {\n    outer() {\n        1: right;\n        inner() {\n            2: left;\n            goto 2;\n        }\n        goto 1;\n    }\n}\nmain() { @ns::outer(); }\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, SRC);

        let pos = pos_after(SRC, "goto 2", 5);
        let labels: Vec<String> = service
            .completion(URI, pos)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert!(
            labels.contains(&"2".to_string()),
            "inner's own label, got {labels:?}"
        );
        assert!(
            !labels.contains(&"1".to_string()),
            "outer's label is a different scope, got {labels:?}"
        );
    }

    /// The enclosing namespace path qualifies the chain's names — the
    /// walk THIS task migrates. The test above exercises
    /// `enclosing_function_chain`/`function_labels`, both migrated in an
    /// earlier task; without this one, nothing here would cover
    /// `enclosing_ns_path` at all.
    ///
    /// At a call position inside `inner`, the candidate for the enclosing
    /// `outer` must carry the namespace-qualified form `ns::outer` —
    /// which is `full_name(&ns_path, name)` with `ns_path == ["ns"]`, so
    /// an `enclosing_ns_path` that returned `[]` would produce a bare
    /// `outer` and fail here.
    #[test]
    fn enclosing_namespace_qualifies_the_chain_after_the_view_migration() {
        const SRC: &str = "namespace ns {\n    outer() {\n        inner() {\n            @\n        }\n    }\n}\nmain() { @ns::outer(); }\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, SRC);

        let pos = pos_after(SRC, "            @", 13);
        let qualified: Vec<String> = service
            .completion(URI, pos)
            .into_iter()
            .filter_map(|c| c.detail.or(Some(c.label)))
            .collect();
        assert!(
            qualified.iter().any(|q| q.contains("ns::outer")),
            "the chain's qualified name must carry the namespace path, got {qualified:?}"
        );
    }
```

Read the module's `Candidate` shape before transcribing this one: if the qualified form
lives on a field other than `detail`/`label`, assert against that field instead. The
POINT of the test is that `ns::outer` appears where `enclosing_ns_path` fed it, so keep
the point and adapt the accessor — and say in your report which field carried it.

Copy the service/fixture shape from `call_position_top_level_offers_defs_imports_and_std_paths`
(`complete.rs`, same `mod tests`).

- [ ] **Step 2: Run the tests and record the baseline**

Run: `cargo test -p mtc-post-machine --lib lsp::complete`

**Expect them to PASS on arrival, and do not manufacture a red phase.** Both tests
exercise the public `completion` API, whose behavior this plan preserves exactly — so
they cannot fail before the migration any more than after it. They are regression pins
over a behavior-preserving change, not a red-green cycle. Two earlier tasks in this plan
hit the same thing and documented it honestly; do the same.

What they DO catch is the migration going wrong in Step 3–4: if `enclosing_ns_path`
returns `[]` where it used to return `["ns"]`, the second test fails, and that is the
whole reason it exists.

- [ ] **Step 3: Rewrite `enclosing_ns_path`**

```rust
/// The namespace path enclosing `offset` — the `::` segments of the
/// `namespace { }` blocks it sits inside, outermost first (a function's
/// own extent never changes it; only namespace blocks add a segment),
/// recursively, innermost match wins. Unlike
/// `walk::enclosing_function_chain`, this walks a DIFFERENT node kind
/// (namespace blocks, never function extents) toward a different result
/// shape (a path of names, not a chain of views) — its own walk, not a
/// duplicate of the shared one.
fn enclosing_ns_path(file: &FileView, offset: u32) -> Vec<String> {
    fn descend(items: impl Iterator<Item = TopView>, offset: u32) -> Vec<String> {
        for item in items {
            if let TopView::Namespace(ns) = item
                && ns.syntax().text_range().contains(offset)
            {
                let mut path = vec![ns.name()];
                path.extend(descend(ns.items(), offset));
                return path;
            }
        }
        Vec::new()
    }
    descend(file.items(), offset)
}
```

- [ ] **Step 4: Update the three call sites**

Build the view and offset once near the top of `completion`, beside the existing `state.tokens` read, and thread them down:

```rust
    let view = state.green.as_ref().map(|green| {
        let root = SyntaxNode::new_root(Rc::clone(green));
        (
            FileView::cast(root).expect("root is FILE"),
            TextLineIndex::new(&state.text),
        )
    });
```

- `ns_path` (~599): `view.as_ref().map(|(file, _)| enclosing_ns_path(file, offset)).unwrap_or_default()` — the `[]` fallback preserved.
- The chain leg (~613): `if let Some((file, index)) = &view { let chain = enclosing_function_chain(file, offset); … }` — each level's qualified name still rebuilt with `full_name(&ns_path, &name)` then a `.` segment per level, where `name` is `f.header().name.text()`. The skipped-not-substituted behavior is preserved by the `if let`.
- `label_candidates` (~749): takes the view pair, returns `Vec::new()` when it is `None`; then `enclosing_function_chain(file, offset).last()` and `function_labels(f, index)`, deduping by value into `Value` candidates exactly as now.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p mtc-post-machine --lib lsp::complete`
Expected: PASS — the new test plus every pre-existing `complete` test, **unmodified**. This module has the largest test surface in the LSP; treat any pre-existing failure as a behavior change to report, not to fix by editing the test.

- [ ] **Step 6: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/post-machine/src/lsp/complete.rs
git commit -m "feat(post-machine): completion contexts walk the green tree"
```

---

### Task 6: `mod.rs` — document symbols

The last CST reader, and the one with a lower tier than the rest: document symbols answer as long as parsing succeeded, even when a later stage fatals. It needs no item internals — only names and spans — so it becomes pure views and keeps its tier exactly.

**Files:**
- Modify: `crates/post-machine/src/lsp/mod.rs` (`cst_symbols` ~452, `function_symbol` ~472, `document_symbols` ~702)
- Test: `crates/post-machine/src/lsp/mod.rs` (its existing `#[cfg(test)] mod tests`)

**A boundary divergence Task 2 uncovered, and the reason this task cannot take
`f.syntax().text_range()` whole.** A green FUNCTION node **retro-wraps its bound doc
run**: for `"? doc\nmain() {\n1: right;\n}\n"` the committed golden shows
`FUNCTION@0..26` — starting at the doc line, byte 0 — while the C1 `FunctionCst.span`
started at `main`, byte 6. That `span`'s own doc comment says it exists "for hit-testing
and document-symbol ranges", and this task is the document-symbol half. Taking the node's
range unchanged would widen every doc-commented function's symbol range by a line or
more, visible in an editor's outline — a behavior change this plan forbids. Reconstruct
the C1 extent instead (Step 3), and pin it (Step 1's second test).

**Interfaces:**
- Consumes: `DocState.green`, `TextLineIndex`, `crate::syntax::{FileView, FunctionView, TopView}`, `crate::syntax::PmcKind`.
- Produces: no new API. `tree_symbols(items: impl Iterator<Item = TopView>, index: &TextLineIndex) -> Vec<SymbolNode>` (renamed from `cst_symbols` — the name says CST and would be a lie).

- [ ] **Step 1: Write the failing test** (append to `mod.rs`'s `mod tests`)

```rust
    /// Document symbols still answer on a document whose LATER stages
    /// fail — the tier this feature is documented to hold. `goto 99;`
    /// references an undefined label, which fatals `ir::lower`, so
    /// `analysis` is `None` while the parse itself succeeded. The same
    /// degradation shape `NAV_FIXTURE_BROKEN` uses in `navigate.rs`.
    #[test]
    fn document_symbols_answer_below_the_analysis_tier() {
        const SRC: &str = "outer() {\n    inner() { right; }\n    goto 99;\n}\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, SRC);

        let symbols = service
            .document_symbols(URI)
            .expect("parsing succeeded, so symbols answer");
        let names: Vec<String> = symbols.iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, vec!["outer".to_string()]);
        let nested: Vec<String> = symbols[0].children.iter().map(|s| s.name.clone()).collect();
        assert_eq!(nested, vec!["inner".to_string()]);
    }
```

`mod.rs`'s `mod tests` has no `URI` const of its own — declare one in the test
(`const URI: &str = "untitled:Symbols-1";`) or reuse whichever the neighbouring
tests use; read them and follow. Copy the service shape from
`parse_failure_yields_exactly_one_error_with_its_fatal_code`.

Add a second test in the same module, pinning the extent against the divergence above.
Derivation-first, the way this repo's goldens work — the expected spans are reasoned out
from the source, never pasted from a run:

```rust
    /// A doc-commented function's symbol range starts at its HEADER, not
    /// at its doc run. The green FUNCTION node retro-wraps the doc run,
    /// so the node's own range begins a line earlier; the C1 extent this
    /// feature has always reported does not, and an editor's outline
    /// shows the difference.
    #[test]
    fn doc_commented_function_symbol_range_excludes_its_doc_run() {
        // "? doc\n"      -> line 1
        // "main() {\n"   -> line 2, the header
        // "    right;\n" -> line 3
        // "}\n"          -> line 4, the closing brace
        const SRC: &str = "? doc\nmain() {\n    right;\n}\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, SRC);

        let symbols = service.document_symbols(URI).expect("parses");
        assert_eq!(symbols.len(), 1);
        assert_eq!(
            symbols[0].span.start,
            Pos { line: 2, col: 1 },
            "range starts at the header, not at the doc line"
        );
        assert_eq!(symbols[0].span.end, Pos { line: 4, col: 2 }, "one past `}`");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mtc-post-machine --lib lsp::tests`
Expected: FAIL to compile, or FAIL because `document_symbols` still reads `state.cst`.

- [ ] **Step 3: Rewrite both walkers**

```rust
/// A function's C1 extent — its header start through `}`, EXCLUDING a
/// bound doc run. The green FUNCTION node retro-wraps its doc run, so
/// `syntax().text_range()` starts a line or more earlier than the CST's
/// own `span` did; document-symbol ranges are one of the two things that
/// span was for (docs/lsp.md (document symbols)), so taking the node's
/// range whole would widen every doc-commented function's outline entry.
fn function_extent(f: &FunctionView) -> TextRange {
    let full = f.syntax().text_range();
    let start = f
        .syntax()
        .children_with_tokens()
        .find(|e| {
            e.kind() != PmcKind::DocRun.into()
                && e.kind() != PmcKind::Whitespace.into()
                && e.kind() != PmcKind::LineComment.into()
                && e.kind() != PmcKind::BlockComment.into()
        })
        .map_or(full.start, |e| e.text_range().start);
    TextRange::new(start, full.end)
}

fn tree_symbols(
    items: impl Iterator<Item = TopView>,
    index: &TextLineIndex,
) -> Vec<SymbolNode> {
    items
        .filter_map(|item| match item {
            TopView::Use(_) => None,
            TopView::Namespace(ns) => Some(SymbolNode {
                name: ns.name(),
                kind: SymbolNodeKind::Namespace,
                span: index.span(ns.syntax().text_range()),
                selection_span: index.span(ns.name_token().text_range()),
                children: tree_symbols(ns.items(), index),
            }),
            TopView::Function(f) => Some(function_symbol(&f, index)),
        })
        .collect()
}

/// One function's symbol (top-level or nested). Children are its nested
/// definitions, recursively; labels and statements are never emitted as
/// symbols.
fn function_symbol(f: &FunctionView, index: &TextLineIndex) -> SymbolNode {
    SymbolNode {
        name: f.header().name.text().to_string(),
        kind: SymbolNodeKind::Function,
        span: index.span(function_extent(f)),
        selection_span: index.span(f.header().name.text_range()),
        children: f.nested().map(|n| function_symbol(&n, index)).collect(),
    }
}
```

And `document_symbols`:

```rust
    fn document_symbols(&mut self, uri: &str) -> Option<Vec<SymbolNode>> {
        // Parse-tier: answered as long as parsing succeeded, even if a
        // later stage (duplicate-binding check, `ir::lower`) fatals.
        let state = self.docs.get(uri)?;
        let green = state.green.as_ref()?;
        let root = SyntaxNode::new_root(Rc::clone(green));
        let file = FileView::cast(root).expect("root is FILE");
        let index = TextLineIndex::new(&state.text);
        Some(tree_symbols(file.items(), &index))
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mtc-post-machine --lib lsp`
Expected: PASS — the new test plus the entire LSP suite, **unmodified**.

- [ ] **Step 5: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/post-machine/src/lsp/mod.rs
git commit -m "feat(post-machine): document symbols walk the green tree"
```

---

### Task 7: Retire `StagedAnalysis.cst` and the interim double parse

Plan 4 left `parse_cst` running alongside the green parse on every document analysis, because the language service still read the C1 CST. Tasks 2–6 removed the last such reader. Now the field goes, and with it the second parse.

**Files:**
- Modify: `crates/post-machine/src/compiler.rs` (`StagedAnalysis`, `analyze_staged`)
- Modify: `crates/post-machine/src/lsp/mod.rs` (`DocState`)
- Modify: `docs/lsp.md` (the staged-analysis tier description)
- Modify: `CLAUDE.md` (the two-parse-paths paragraph)
- Test: `crates/post-machine/src/compiler.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: everything Tasks 2–6 produced.
- Produces: `StagedAnalysis` without `cst`; `DocState` without `cst`. `analyze_staged` parses exactly once.

- [ ] **Step 1: Prove nothing reads it**

Run: `grep -rn "\.cst\b" crates/post-machine/src/ --include=*.rs | grep -v "lsp/pma/" | grep -v "asm"`
Expected: hits only in `compiler.rs` (the field and its constructions) and `lsp/mod.rs` (the `DocState` field and its assignment). **Any other hit is a reader Tasks 2–6 missed** — report it and migrate it before continuing.

`lsp/pma/` is excluded on purpose: it walks core's asm CST, a different tree entirely.

- [ ] **Step 2: Delete the field and the second parse**

In `compiler.rs`: remove `StagedAnalysis.cst` and its doc; remove the `let cst = crate::parser::parse_cst(&lexed).ok();` line and the `debug_assert!` beside it; remove `cst` from all four `StagedAnalysis` constructions. Rewrite the doc comment's interim paragraph — the double parse it describes is gone:

```rust
/// lex (WithComments) → green parse → extract → duplicate-binding check
/// → flatten → ir::lower, retaining each stage's outcome instead of
/// stopping at the first failure. Extraction and `flatten` are
/// infallible, so the only post-parse fatals are `DuplicateBinding` (the
/// binding check) and `UndefinedLabel` (`ir::lower`) — the pipeline
/// always runs through `ir::lower`, never stopping at `flatten`. The
/// `IrProgram` itself is discarded once `ir::lower` has had its say: the
/// LSP's tiers only need the flattened `Analysis`, not the CFG.
```

In `lsp/mod.rs`: remove `DocState.cst` and its assignment.

Drop `use crate::cst::Cst;` from `compiler.rs` if nothing else there needs it — `clippy -D warnings` will say.

- [ ] **Step 3: Run the tier tests, unmodified**

Run: `cargo test -p mtc-post-machine --lib compiler`
Expected: PASS. Two pre-existing tests name `cst` in their assertions — `analyze_staged_parse_failure_keeps_tokens_but_not_cst` and `analyze_staged_duplicate_binding_keeps_cst_but_not_analysis` — and Task 5 of plan 4 added `staged_cst_is_present_whenever_the_green_parse_succeeded`.

These three **must** be updated, and that is not a contract change: the tiers they pin are unchanged, only the field they observe them through is gone. Rewrite each to assert the same tier through `green` instead of `cst`, and rename them accordingly (`..._keeps_tokens_but_not_the_tree`, `..._keeps_the_tree_but_not_analysis`). Delete `staged_cst_is_present_whenever_the_green_parse_succeeded` outright — it existed only to pin the interim double parse's invariant, which no longer exists. Say all of this explicitly in your report.

The other three tier tests must pass **untouched**.

- [ ] **Step 4: Correct `docs/lsp.md`**

Find the staged-analysis section's tier description (`grep -n "cst\|CST" docs/lsp.md`) and correct any sentence that describes the `.pmc` service as reading a CST or the analysis as producing one. The `.pma`/`.tma` services genuinely still do — leave those alone. Record what you found and what you changed; if nothing needed changing, say so explicitly.

- [ ] **Step 5: Update `CLAUDE.md`**

The `### Pipeline and key types` paragraph beginning "**Two parse paths coexist until the C2 cutover.**" lists the `.pmc` language service as still on the C1 path. It no longer is. Rewrite that clause so the remaining C1 consumers are exactly: `fmt` (via `parse_cst`), and the optimizer/IR/codegen unit tests (via `parse`). Keep the rest of the paragraph — the oracle description and the cutover note — intact.

- [ ] **Step 6: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p mtc-core --no-default-features`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/post-machine/src/compiler.rs crates/post-machine/src/lsp/mod.rs docs/lsp.md CLAUDE.md
git commit -m "feat(post-machine): retire the staged CST and the interim double parse"
```

---

## Two costs this plan accepts on purpose

Both are plan-mandated and fine at this repo's scale (the largest `.pmc` in the corpus
is 99 lines). Stated so a reviewer does not raise either as a defect:

- **A go-to-definition on a label extracts each statement twice.** `label_reference_at`
  runs `extract_statement` over the function's statements to find the reference, then
  `label_span` → `function_labels` runs it again over the same statements to find the
  declaration. Collapsing them would mean threading one extraction through two
  independent walkers whose only shared caller is `definition`; the duplication buys
  keeping each walker readable on its own.
- **Each entry point builds a fresh `TextLineIndex` per request**, which copies the
  document text (the index owns it by design — `line_index.rs`'s own module doc records
  that tradeoff). Caching it on `DocState` would be the obvious next step if this ever
  measured; it does not today.

## Completion criteria for Plan 5

Named gates, not "the suite is green":

- **No `.pmc` LSP code reads a C1 CST.** Verify: `grep -rn "crate::cst::" crates/post-machine/src/lsp/` returns nothing.
- **`analyze_staged` parses once.** Verify: `grep -n "parse_cst" crates/post-machine/src/compiler.rs` returns nothing.
- **`crates/core` gained exactly one method** — `TextLineIndex::offset`. Verify: `git diff <plan-base>..HEAD --stat -- crates/core` shows only `syntax/line_index.rs`.
- **The whole LSP suite passes**, with only the three named `analyze_staged` tests changed and only to observe `green` where they observed `cst`.
- **`token_at_offset` and preorder `descendants` were not added.** They were deferred from plan 3 for this plan and turned out unnecessary; adding them speculatively is scope creep. Verify: `grep -n "token_at_offset\|fn descendants" crates/core/src/syntax/red.rs` returns nothing.
- **Views gained no item-internals accessor.** The binding reuse ruling holds: item contents come from `extract_statement`. Verify: `grep -n "label_refs\|call_name" crates/post-machine/src/syntax/views.rs` returns nothing.
- **No version space moved**, and PM-1 byte-identity is untouched (this plan changes no compiler path — `golden_programs`, `asm_volatile`, `opt_equivalence` should not even be at risk, but they run in the workspace suite anyway).
- Quality gates: `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build -p mtc-core --no-default-features`. The toolchain is pinned, so these are exactly what CI runs.

## What this plan deliberately does not do

- **fmt** keeps calling `parse_cst`. Plan 6, gated on byte-identical output over every fmt fixture.
- **The cutover** — deleting `cst.rs`, `lower_cst`, `parse_cst`, `parse`, the `WithoutComments` lex mode and the extraction oracle — is plan 7. Its known debt: ~14 `#[cfg(test)]` modules in `optimizer/*`, `ir.rs`, `codegen.rs`, `parser.rs` call `parse` directly; `extract.rs`'s doc comments cite `parser.rs` line numbers that die with the functions.
- **The TM mirror** of all of this comes after PM is finished.
- **Lowering the statement-level features' tier.** With items now coming from the tree rather than from `Analysis`, `navigate` and `tokens` could in principle answer below the analysis tier. Both keep gating on `analysis` exactly as today; changing that is a behavior change and belongs to its own decision, not to a migration plan.
