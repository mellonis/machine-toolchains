# C2 Plan 6 — the `.pmc` formatter onto the green tree

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move `pmt fmt`'s `.pmc` printer off the C1 CST and onto the green
tree, so that after this plan `parse_cst` has zero production callers and
survives only as half the differential oracle.

**Architecture:** The green printer is built as a **parallel implementation**
in a new `fmt/print.rs`, growing surface by surface, with the C1 printer left
untouched as the differential reference — the same shape the parser migration
used in plan 2 (green emission woven alongside C1, oracle comparing, C1 deleted
at the end). Each task widens the set of sources on which
`print::format_green(src) == fmt::format(src)` holds; the final task swaps
`format()` over and deletes the C1 printer in one commit. Everything C1 stored
as a derived CST field — `blank_before`, `trailing`, `leading`,
`newline_before`, `open_trailing`, `close_trailing`, `label_break`,
`interior` — is re-derived from green trivia tokens by a new `fmt/trivia.rs`;
everything C1 stored as a parsed VALUE comes from `syntax::extract_statement`
and the typed views, unchanged.

**Tech Stack:** Rust (workspace pinned by `rust-toolchain.toml`), `mtc-core`'s
`syntax` framework (green/red model, `children_with_tokens`, `TextLineIndex`),
`mtc-post-machine`'s `syntax` module (`PmcKind`, views, `extract_statement`).

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md`
(§5.1 fmt; §6 oracles and gates). Read §5.1 and §6 before Task 1.

## Global Constraints

- **Byte-identical output is THE gate.** Every existing fmt fixture, the
  corpus, and the property tests must produce output identical to today's,
  byte for byte. If a green walk would render something *better*, it still
  renders it the old way in this plan. Behavior fixes are a separate,
  deliberate change — mixing one in destroys the only oracle that proves this
  rewrite faithful.
- **The compiled-stdlib byte-identity gate applies to this plan**: compile the
  embedded stdlib before and after at both opt levels and byte-compare the
  object. This is the standing gate for any text-only or formatter change.
- **`crates/core` gets no diff in this plan** — check it at every commit
  with `git diff --stat master -- crates/core`, which must print nothing. Everything the printer needs
  (`children_with_tokens`, `prev_sibling_or_token`, `next_sibling_or_token`,
  `first_token`, `last_token`, `TextLineIndex`) already exists and was verified
  present before this plan was written. A `crates/core` diff here is a design
  smell, not a convenience — raise it as a ruling instead of taking it.
- **PM only.** `.tmc` fmt is a later plan. Do not touch
  `crates/turing-machine`.
- **The C1 printer stays working until Task 7.** Do not edit
  `fmt/mod.rs`'s existing printer functions in Tasks 1–6; they are the
  reference the differential tests compare against.
- **The transient duplication is intended.** Two printers coexist from Task 2
  to Task 7 and the second one deletes the first. Do not "DRY" them together
  mid-plan.
- **FUNCTION green nodes retro-wrap their bound doc run; NAMESPACE nodes do
  NOT.** A `FUNCTION` node's `text_range()` starts at its `DOC_RUN` child, a
  line or more before the C1 `FunctionCst.span` did. Blank-line logic must key
  off the *unit* start (the item together with any leading comment run), never
  off a raw node extent, or one of the two kinds emits a silently wrong blank
  line. A doc run before `namespace` is a `DanglingDocRun` fatal, so the
  namespace side of the asymmetry cannot arise — but do not rely on that
  accidentally.
- **Statement values and statement trivia come from different places.**
  `syntax::extract_statement(&StatementView, &TextLineIndex) -> Statement`
  gives `{ labels, items, line, span }` — the value half. `label_break` and
  `trailing`, which `StatementCst` also carried, are trivia and come from
  `fmt/trivia.rs`. The Nth `ITEM` node under a `STATEMENT` corresponds to the
  Nth entry of `Statement::items`; that index correspondence is how per-item
  trivia is joined to per-item values.
- Conventional commits with scope, e.g. `feat(post-machine):`,
  `test(post-machine):`, `docs:`.
- No AI/Claude attribution in any commit message or artifact.
- Code comments cite durable `docs/` pages by page + parenthetical keyword
  (`docs/pmt/fmt.md (own-line labels)`). Never cite a `docs/superpowers/`
  spec or plan from code, and never let `spec §N` notation reach a doc
  comment. Published docs are forge-agnostic: no issue/PR numbers, no
  hosting-provider URLs.

## The tree shape this plan is built on

Verified against the real parser before this plan was written (`parse_green`
on `tests/syntax/rich.pmc` and on a hand-built comment-rich source). Trivia
flushes into the current node before a child opens, so **a node starts at its
first significant token and trivia sits between a parent's children**:

```
FILE
  tok   LINE_COMMENT  "// standalone"
  tok   WHITESPACE    "\n\n"
  NODE  USE_DECL
  tok   WHITESPACE    "\n\n"
  NODE  FUNCTION            <- retro-wrapped: starts at its DOC_RUN
  tok   WHITESPACE    " "
  tok   LINE_COMMENT  "// close trailing"    <- lives in the PARENT stream
```

```
FUNCTION
  NODE  DOC_RUN
  tok   WHITESPACE " "     <- the gap between the doc run and the declaration
  tok   IDENT      "volatile"
  tok   IDENT      "main"
  tok   L_PAREN / R_PAREN
  tok   L_BRACE    "{"
  tok   WHITESPACE " "
  tok   LINE_COMMENT "// open trailing"
  tok   WHITESPACE "\n    "
  NODE  STATEMENT | FUNCTION (nested)
  tok   WHITESPACE "\n"
  tok   R_BRACE    "}"
```

```
USE_DECL: IDENT("use") · USE_PATH · COMMA · LINE_COMMENT · USE_PATH · SEMI
STATEMENT: LABEL* · ITEM · COMMA · ITEM · SEMI
ITEM(check): IDENT("check") · L_PAREN · CHECK_ARM · COMMA · CHECK_ARM · R_PAREN
```

Two consequences worth naming, because both are easy to get wrong:

1. **A comment after a closing `}` is not inside the node it follows.** It is
   the next sibling token in the parent's stream. So `close_trailing` and a
   statement's `trailing` are THE SAME primitive applied to different node
   kinds — one function, not two.
2. **A leading comment run is not part of the item either.** C1 attached it to
   the item, so `blank_before` measured the gap before the whole unit. In green
   the run is a sequence of sibling tokens, so the unit's start must be found
   by walking back over the run.

## File Structure

| File | Responsibility |
|---|---|
| `crates/post-machine/src/fmt/trivia.rs` (new) | Comment/blank-line classification over green trivia. Pure functions on `SyntaxNode`/`SyntaxToken`; no printing, no allocation of output text. |
| `crates/post-machine/src/fmt/print.rs` (new) | The green printer. Grows from Task 2 to Task 6; becomes the only printer at Task 7. |
| `crates/post-machine/src/fmt/mod.rs` (modified) | Declares the two new modules; keeps `format()` and the C1 printer until Task 7, when the C1 printer is deleted and `format()` delegates. |
| `crates/post-machine/tests/fmt_programs.rs` (modified, Task 7) | Gains the corpus-wide byte-identity check once `format()` itself is green. |
| `CLAUDE.md`, `docs/pmt/fmt.md` (modified, Task 8) | The "Still on the C1 path" claim and any C1-shaped description of how fmt reads source. |

Tasks 1–6 keep every new test **inside the crate** as `#[cfg(test)]` unit
tests, because `format_green` is `pub(crate)` and integration tests cannot see
it. Do not widen its visibility to make an integration test reachable; Task 7
is where the public entry point becomes green.

---

### Task 1: `fmt/trivia.rs` — classification over green trivia

**Files:**
- Create: `crates/post-machine/src/fmt/trivia.rs`
- Modify: `crates/post-machine/src/fmt/mod.rs` (add `mod trivia;`)

**Interfaces:**
- Consumes: `mtc_core::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken}`, `crate::syntax::PmcKind`.
- Produces, all `pub(crate)`:
  - `fn leading_comments(node: &SyntaxNode) -> Vec<SyntaxToken>`
  - `fn blank_before_unit(node: &SyntaxNode) -> bool`
  - `fn trailing_comment(node: &SyntaxNode) -> Option<SyntaxToken>`
  - `fn open_trailing(open: &SyntaxToken) -> Vec<SyntaxToken>`
  - `fn label_break(stmt: &SyntaxNode) -> bool`
  - `fn is_comment(k: SyntaxKind) -> bool`, `fn is_ws(k: SyntaxKind) -> bool`

- [ ] **Step 1: Write the failing test**

Create `crates/post-machine/src/fmt/trivia.rs` containing ONLY the test module
below (no implementation yet), so the first run fails to compile against
missing functions rather than passing vacuously. Every source string here was
run through the real parser before this plan shipped and parses.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_green;
    use crate::syntax::PmcKind;
    use mtc_core::syntax::SyntaxNode;

    fn f(src: &str) -> SyntaxNode {
        SyntaxNode::new_root(parse_green(src).expect("parses"))
    }

    fn functions(root: &SyntaxNode) -> Vec<SyntaxNode> {
        root.children()
            .filter(|c| c.kind() == PmcKind::Function.into())
            .collect()
    }

    fn statements(fun: &SyntaxNode) -> Vec<SyntaxNode> {
        fun.children()
            .filter(|c| c.kind() == PmcKind::Statement.into())
            .collect()
    }

    /// The gap C1 recorded as `blank_before` sits before the item's
    /// leading comment run, not before the item node — the run is a
    /// sequence of sibling tokens in green, not part of the item.
    #[test]
    fn blank_before_unit_looks_past_the_leading_run() {
        let r = f("main() {\n 1: left;\n}\n\n// lead\nother() {\n 1: left;\n}\n");
        let fns = functions(&r);
        assert_eq!(fns.len(), 2);
        assert!(!blank_before_unit(&fns[0]), "nothing precedes the first item");
        assert_eq!(leading_comments(&fns[1]).len(), 1);
        assert!(blank_before_unit(&fns[1]), "the gap is before the comment");
    }

    /// A blank line inside a comment run cuts it: only the part below
    /// the gap binds to the item.
    #[test]
    fn a_blank_line_cuts_the_leading_run() {
        let r = f("main() {\n 1: left;\n}\n\n// far\n\n// near\nother() {\n 1: left;\n}\n");
        let fns = functions(&r);
        let lead = leading_comments(&fns[1]);
        assert_eq!(lead.len(), 1, "only `// near` binds");
        assert_eq!(lead[0].text(), "// near");
    }

    /// A trailing comment rides the same source line; a newline ends it.
    #[test]
    fn trailing_comment_stops_at_the_line_end() {
        let r = f("main() { // open\n 1: left; // ride\n // not mine\n 2: left;\n}\n");
        let st = statements(&functions(&r)[0]);
        assert_eq!(
            trailing_comment(&st[0]).map(|t| t.text().to_string()),
            Some("// ride".to_string())
        );
        assert_eq!(trailing_comment(&st[1]), None);
    }

    /// `open_trailing` reads forward off the brace token itself.
    #[test]
    fn open_trailing_reads_off_the_brace() {
        let r = f("main() { // open\n 1: left;\n}\n");
        let brace = functions(&r)[0]
            .children_with_tokens()
            .find_map(|e| match e {
                SyntaxElement::Token(t) if t.kind() == PmcKind::LBrace.into() => Some(t),
                _ => None,
            })
            .expect("the body brace");
        let open = open_trailing(&brace);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].text(), "// open");
    }

    /// What C1 called `close_trailing` is `trailing_comment` applied to
    /// the closed node: the comment lives in the PARENT's child stream.
    #[test]
    fn close_trailing_is_trailing_comment_on_the_node() {
        let r = f("main() {\n 1: left;\n} // bye\n");
        let fun = &functions(&r)[0];
        assert_eq!(
            trailing_comment(fun).map(|t| t.text().to_string()),
            Some("// bye".to_string())
        );
    }

    /// `label_break` is a newline between the last label and the first item.
    #[test]
    fn label_break_sees_the_own_line_label() {
        let r = f("main() {\n 1:\n    left;\n 2: left;\n}\n");
        let st = statements(&functions(&r)[0]);
        assert!(label_break(&st[0]));
        assert!(!label_break(&st[1]));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-post-machine --lib fmt::trivia`
Expected: FAIL to compile — `cannot find function 'blank_before_unit' in this scope`, and the same for the other five.

- [ ] **Step 3: Write the implementation**

Put this above the test module in `fmt/trivia.rs`. This code was compiled and
behavior-checked against the real parser before the plan shipped; use it
verbatim.

```rust
//! Comment and blank-line classification, re-derived from green trivia.
//!
//! The C1 CST stored these as fields the parser filled in — `blank_before`,
//! `trailing`, `leading`, `newline_before`, `open_trailing`,
//! `close_trailing`, `label_break`. The green tree stores nothing derived:
//! trivia are ordinary tokens sitting between a node's children, so every
//! one of those classifications is a local query over `children_with_tokens`
//! or a sibling walk. Same decisions, new source of truth
//! (`docs/pmt/fmt.md` (comments)).
//!
//! Two shapes drive most of this file. A comment after a closing `}` is the
//! next sibling token in the PARENT's stream, not a child of the node it
//! follows — so what the CST called `close_trailing` and what it called a
//! statement's `trailing` are one function here. And a leading comment run
//! is a run of sibling tokens rather than part of the item, so the gap
//! before the whole unit has to be found by walking back over the run.

use crate::syntax::PmcKind;
use mtc_core::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

pub(crate) fn is_comment(k: SyntaxKind) -> bool {
    k == PmcKind::LineComment.into() || k == PmcKind::BlockComment.into()
}

pub(crate) fn is_ws(k: SyntaxKind) -> bool {
    k == PmcKind::Whitespace.into()
}

/// The tokens immediately before `node`, newest-first, stopping at the
/// first sibling that is a node.
fn preceding_tokens(node: &SyntaxNode) -> Vec<SyntaxToken> {
    let mut out = Vec::new();
    let mut cur = node.prev_sibling_or_token();
    while let Some(SyntaxElement::Token(t)) = cur {
        cur = t.prev_sibling_or_token();
        out.push(t);
    }
    out
}

/// The comment run bound to `node` as its leading block, in source order.
/// A blank line ends the run: comments above the gap belong to whatever
/// came before, exactly as the CST's attachment pass decided.
pub(crate) fn leading_comments(node: &SyntaxNode) -> Vec<SyntaxToken> {
    let mut out = Vec::new();
    for t in preceding_tokens(node) {
        if is_comment(t.kind()) {
            out.push(t);
        } else if is_ws(t.kind()) {
            if t.text().matches('\n').count() >= 2 {
                break;
            }
        } else {
            break;
        }
    }
    out.reverse();
    out
}

/// Whether the author left an empty line before the whole unit — the item
/// together with its leading comment run. Keyed off the unit's start, never
/// off `node.text_range()`: a FUNCTION node already retro-wraps its doc run
/// and a NAMESPACE node does not, so a raw extent is the wrong anchor for
/// one of the two.
pub(crate) fn blank_before_unit(node: &SyntaxNode) -> bool {
    let lead = leading_comments(node);
    let before = match lead.first() {
        Some(first) => first.prev_sibling_or_token(),
        None => node.prev_sibling_or_token(),
    };
    match before {
        Some(SyntaxElement::Token(t)) => is_ws(t.kind()) && t.text().matches('\n').count() >= 2,
        _ => false,
    }
}

/// A comment riding the same source line as `node`'s last token — what the
/// CST recorded as `trailing` on a statement and as `close_trailing` on a
/// namespace or function.
pub(crate) fn trailing_comment(node: &SyntaxNode) -> Option<SyntaxToken> {
    let mut cur = node.next_sibling_or_token();
    while let Some(SyntaxElement::Token(t)) = cur {
        if is_ws(t.kind()) {
            if t.text().contains('\n') {
                return None;
            }
            cur = t.next_sibling_or_token();
        } else if is_comment(t.kind()) {
            return Some(t);
        } else {
            return None;
        }
    }
    None
}

/// Comments after an opening `{` still on its line.
pub(crate) fn open_trailing(open: &SyntaxToken) -> Vec<SyntaxToken> {
    let mut out = Vec::new();
    let mut cur = open.next_sibling_or_token();
    while let Some(SyntaxElement::Token(t)) = cur {
        if is_ws(t.kind()) {
            if t.text().contains('\n') {
                break;
            }
        } else if is_comment(t.kind()) {
            out.push(t.clone());
        } else {
            break;
        }
        cur = t.next_sibling_or_token();
    }
    out
}

/// Whether the author broke the line between a statement's last label and
/// its first item (`docs/pmt/fmt.md` (own-line labels)). The printer
/// preserves this choice and never infers or overrides it.
pub(crate) fn label_break(stmt: &SyntaxNode) -> bool {
    let mut seen_label = false;
    for e in stmt.children_with_tokens() {
        match e {
            SyntaxElement::Node(n) if n.kind() == PmcKind::Label.into() => seen_label = true,
            SyntaxElement::Node(_) => return false,
            SyntaxElement::Token(t) if seen_label && is_ws(t.kind()) => {
                if t.text().contains('\n') {
                    return true;
                }
            }
            SyntaxElement::Token(_) => {}
        }
    }
    false
}
```

Add `mod trivia;` to `crates/post-machine/src/fmt/mod.rs` beside the existing
items (the file is currently a single module with no submodules; put the
declaration directly under the `//!` header block and above the `use`
statements).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-post-machine --lib fmt::trivia`
Expected: PASS, 6 tests.

Then confirm nothing else moved:
Run: `cargo test -p mtc-post-machine`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/fmt/trivia.rs crates/post-machine/src/fmt/mod.rs
git commit -m "feat(post-machine): re-derive fmt's comment classification from green trivia"
```

---

### Task 2: `fmt/print.rs` — the file/use/namespace skeleton

**Files:**
- Create: `crates/post-machine/src/fmt/print.rs`
- Modify: `crates/post-machine/src/fmt/mod.rs` (add `mod print;`)

**Interfaces:**
- Consumes: Task 1's `trivia::{blank_before_unit, leading_comments, trailing_comment, open_trailing, is_comment, is_ws}`.
- Produces:
  - `pub(crate) fn format_green(source: &str) -> Result<String, CompileError>` — the green printer's entry point. Handles FILE, USE_DECL, USE_PATH and NAMESPACE containers plus standalone comments; any other node kind hits `unreachable!` (see Step 3).
  - `pub(crate) const INDENT_UNIT: usize` is NOT redefined — reuse `super::INDENT_UNIT`.

- [ ] **Step 1: Write the failing test**

Add to `crates/post-machine/src/fmt/print.rs`:

```rust
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
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: FAIL to compile — `cannot find function 'format_green'`.

- [ ] **Step 3: Write the implementation**

Build `format_green` as the mirror of `fmt/mod.rs`'s `format` +
`print_cst` + `print_top_items` + `print_top_item` + `print_namespace` +
`print_use` + `render_use_path` + `print_comment` + `normalize_comment_text`,
reading each of those functions and porting its DECISIONS unchanged. The
differences are only in where the inputs come from:

- Entry: `lex_with(source, LexMode::WithComments)` then
  `crate::parser::parse_green_from_tokens(&tokens)` then
  `SyntaxNode::new_root(...)`, mirroring `analyze`'s front end. Build one
  `TextLineIndex::new(source)` and thread it through — later tasks need it for
  source columns.
- Top-level iteration: walk `root.children_with_tokens()`. A `Node` whose kind
  is `UseDecl`, `Namespace` or `Function` is an item; a `LineComment` or
  `BlockComment` token that is not part of any item's leading run is a
  standalone comment; `Whitespace` tokens are consumed, never printed.
- `blank_before` becomes `trivia::blank_before_unit(&node)`; an item's leading
  comments become `trivia::leading_comments(&node)` and print above it.
- A namespace's `open_trailing` becomes `trivia::open_trailing(&brace)` where
  `brace` is the namespace node's `L_BRACE` child token; its `close_trailing`
  becomes `trivia::trailing_comment(&namespace_node)`.
- A use declaration's paths are `UseDeclView::paths()`; each path's segments
  and alias come from `UsePathView::segments()` / `alias_token()`. Its
  `interior` comments are Task 6's surface — for now, assert none are present
  (see the `unreachable!` rule below) so a mis-scoped test fails loudly.

Guard every surface this task does not yet cover with an explicit panic
carrying the task that owns it, so a test fed an out-of-scope shape fails
loudly instead of silently printing something wrong:

```rust
unreachable!("FUNCTION is task 3's surface; `format_green` must not be called on it yet")
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: PASS, 6 tests.

Run: `cargo test -p mtc-post-machine`
Expected: all green — the C1 printer is untouched, so every existing fmt test
still passes.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/fmt/print.rs crates/post-machine/src/fmt/mod.rs
git commit -m "feat(post-machine): green-tree fmt printer for the file/use/namespace skeleton"
```

---

### Task 3: functions, doc runs and statement bodies

**Files:**
- Modify: `crates/post-machine/src/fmt/print.rs`

**Interfaces:**
- Consumes: Task 2's `format_green` and its `same_as_c1` test helper; Task 1's trivia primitives; `crate::syntax::{FunctionView, StatementView, extract_statement}`; `crate::parser::Statement`.
- Produces: `format_green` now covers FUNCTION (header, doc run, nesting) and STATEMENT (labels, items, alignment, greedy fill) for comment-free bodies.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `fmt/print.rs`. Every source below parses —
verified against the real parser before this plan shipped.

```rust
    #[test]
    fn a_minimal_function() {
        same_as_c1("main() {\n 1: left;\n}\n");
        same_as_c1("main(){1:left;}\n");
        same_as_c1("main() {\n}\n");
    }

    #[test]
    fn function_header_modifiers() {
        same_as_c1("volatile main() {\n 1: left;\n}\n");
        same_as_c1("export helper() {\n 1: left;\n}\n");
    }

    #[test]
    fn a_doc_run_binds_to_its_function() {
        same_as_c1("? doc line\nmain() {\n 1: left;\n}\n");
        same_as_c1("? one\n? two\n! attention\nmain() {\n 1: left;\n}\n");
        same_as_c1("? doc\n\nmain() {\n 1: left;\n}\n");
    }

    #[test]
    fn nested_functions() {
        same_as_c1("main() {\n    step() {\n 1: left;\n    }\n\n    @step();\n}\n");
    }

    /// A nested function BETWEEN two statements. This is the fixture that
    /// catches the one Task 3 mistake byte-identity would otherwise only
    /// surface at Task 7, as a corpus-wide diff to bisect: building body
    /// order from `statements()` and `nested()` separately instead of from
    /// `children_with_tokens()` hoists the nested function out of place,
    /// and every other nested-function fixture happens to put it first.
    #[test]
    fn a_nested_function_between_statements_keeps_its_position() {
        same_as_c1("main() {\n 1: left;\n    step() {\n 1: left;\n    }\n 2: left;\n}\n");
        same_as_c1("main() {\n 1: left;\n\n    step() {\n 1: left;\n    }\n\n 2: left;\n}\n");
    }

    #[test]
    fn labels_stacked_and_own_line() {
        same_as_c1("main() {\n 1: 2: right, mark;\n 3: left;\n}\n");
        same_as_c1("main() {\n 1:\n    left;\n}\n");
        same_as_c1("main() {\n 1: left;\n 10: left;\n 100: left;\n}\n");
    }

    #[test]
    fn statement_shapes() {
        same_as_c1("main() {\n 1: check(1, 2);\n 2: goto 1;\n 3: halt;\n}\n");
        same_as_c1("main() {\n 1: @callee();\n 2: @callee(!);\n 3: debugger;\n}\n");
        same_as_c1("main() {\n 1: left, right, mark, unmark, left, right, mark, unmark, left;\n}\n");
    }

    #[test]
    fn blank_lines_between_body_items() {
        same_as_c1("main() {\n 1: left;\n\n 2: left;\n}\n");
        same_as_c1("main() {\n 1: left;\n\n\n\n 2: left;\n}\n");
    }
}
```

Note the closing brace: this block ends the `tests` module opened in Task 2 —
delete the old closing brace when you paste it, do not leave two.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: FAIL — the tests panic on the `unreachable!("FUNCTION is task 3's surface…")` guard from Task 2.

- [ ] **Step 3: Write the implementation**

Port `print_function`, `print_doc_run`, `print_doc_run_line`,
`print_body_item`, `command_column`, `max_inline_label_prefix_width`,
`label_prefix_text`, `label_prefix_width`, `label_margin`,
`render_statement_code`, `print_statement`, `code_line_width_incl_semi`,
`render_items`, `line_width_after`, `greedy_fill_group`, `builtin_name`,
`render_builtin_successor`, `render_successor` and `render_check_arm` from
`fmt/mod.rs`. The decisions are unchanged; only the inputs move:

- A function's header (name, `volatile`, `export`, attributes) comes from
  `FunctionView::header()`, its doc run from `FunctionView::doc_run()`, its
  statements from `FunctionView::statements()` and its nested functions from
  `FunctionView::nested()`. Body order — statements interleaved with nested
  functions — must come from `children_with_tokens()`, not from the two
  filtered iterators, or a nested function moves relative to its neighbours.
- A statement's VALUES come from `extract_statement(&view, &index)`, which
  returns `Statement { labels, items, line, span }`. Its `label_break` comes
  from `trivia::label_break(&node)` — `Statement` does not carry it.
- The functions that take only AST values (`render_successor`,
  `render_check_arm`, `builtin_name`, `render_builtin_successor`) are copied
  across unchanged; do not re-derive them from tokens.
- `blank_before` on a body item is `trivia::blank_before_unit(&node)`, the same
  primitive the top level uses.
- The gap between a doc run and its declaration is the whitespace token
  BETWEEN the `DOC_RUN` child and the first header token, inside the FUNCTION
  node — not a gap in the parent's stream. Read `print_function`'s
  `blank_before_decl` parameter for what the C1 side did with it.

Keep the `unreachable!` guards for comment surfaces this task does not cover
(a statement's trailing comment, interior list comments, brace comments) and
name Task 4, 5 or 6 in each message.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: PASS, 13 tests.

Run: `cargo test -p mtc-post-machine`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/fmt/print.rs
git commit -m "feat(post-machine): green-tree fmt printer for functions, doc runs and bodies"
```

---

### Task 4: leading and standalone comments inside bodies

**Files:**
- Modify: `crates/post-machine/src/fmt/print.rs`

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces: `format_green` covers comments that occupy their own line inside a function body or between top-level items, including a comment run's internal blank lines and a block comment spanning lines.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `fmt/print.rs`:

```rust
    #[test]
    fn own_line_comments_inside_a_body() {
        same_as_c1("main() {\n    // leading\n 1: left;\n}\n");
        same_as_c1("main() {\n 1: left;\n    // between\n 2: left;\n}\n");
        same_as_c1("main() {\n 1: left;\n    // trailing the body\n}\n");
    }

    #[test]
    fn a_comment_run_keeps_its_internal_gap() {
        same_as_c1("main() {\n    // far\n\n    // near\n 1: left;\n}\n");
    }

    #[test]
    fn block_comments() {
        same_as_c1("main() {\n    /* one line */\n 1: left;\n}\n");
        same_as_c1("main() {\n    /* a block comment\n       spanning two lines */\n 1: left;\n}\n");
    }

    #[test]
    fn comments_around_a_nested_function() {
        same_as_c1("main() {\n    // about step\n    step() {\n 1: left;\n    }\n\n    @step();\n}\n");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: FAIL — either on a Task 3 `unreachable!` guard for comments, or on an output mismatch against the C1 printer.

- [ ] **Step 3: Write the implementation**

Port `print_comment`, `normalize_comment_text`, `layout_leading`,
`emit_forced_break` and the `LeadingLayout` struct from `fmt/mod.rs`. The
classification inputs come from `trivia::leading_comments`, and the
`blank_before` inside a run is the whitespace between two adjacent comment
tokens carrying two or more newlines. Everything about how a comment's TEXT is
normalized and indented is unchanged — read the C1 functions and port them.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: PASS, 17 tests.

Run: `cargo test -p mtc-post-machine`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/fmt/print.rs
git commit -m "feat(post-machine): green-tree fmt printer for own-line comments"
```

---

### Task 5: trailing comments and their alignment runs

**Files:**
- Modify: `crates/post-machine/src/fmt/print.rs`

**Interfaces:**
- Consumes: Tasks 1–4, in particular `trivia::trailing_comment`; `mtc_core::syntax::TextLineIndex` for source columns.
- Produces: `format_green` covers same-line trailing comments and the column-alignment runs they form.

This is the densest surface in the plan and the one most likely to pass by
luck. Alignment is a RUN property: consecutive statements each carrying a
trailing comment are aligned together, and a statement without one ends the
run. The C1 side computed this in `compute_trailing_spacing(body, codes)`
over a `Vec<BodyItem>`; the green side computes it over the body's node
sequence. **Write the run-boundary rule down in a doc comment** — it was
implicit in the CST's structure and must be explicit here.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `fmt/print.rs`. The middle test deliberately puts
a gap in the middle of a run, so a wrong boundary rule shows up as a column
shift rather than passing by luck.

```rust
    #[test]
    fn a_single_trailing_comment() {
        same_as_c1("main() {\n 1: left; // note\n}\n");
        same_as_c1("main() {\n 1: left;    // over-indented note\n}\n");
    }

    #[test]
    fn an_alignment_run_and_its_boundaries() {
        // Three commented statements in a row align together.
        same_as_c1("main() {\n 1: left; // a\n 2: right; // b\n 3: mark; // c\n}\n");
        // An uncommented statement in the middle breaks the run in two,
        // so the two halves align independently.
        same_as_c1("main() {\n 1: left; // a\n 2: right;\n 3: mark; // c\n}\n");
        // A blank line does not by itself end a run of comments.
        same_as_c1("main() {\n 1: left; // a\n\n 2: right; // b\n}\n");
        // A statement long enough to push its own comment past the run's
        // column decides the run's column for everyone.
        same_as_c1("main() {\n 1: left; // a\n 2: left, right, mark, unmark, left, right; // b\n}\n");
    }

    #[test]
    fn trailing_comments_on_declarations() {
        same_as_c1("use std::goToEnd; // note\n");
        same_as_c1("main() {\n 1: left;\n} // after the brace\n");
        same_as_c1("namespace n {\n} // after the namespace\n");
    }

    #[test]
    fn an_own_line_comment_is_not_a_trailing_one() {
        same_as_c1("main() {\n 1: left;\n // own line\n 2: left; // trailing\n}\n");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: FAIL — a Task 3/4 guard, or misaligned columns against the C1 output.

- [ ] **Step 3: Write the implementation**

Port `compute_trailing_spacing` and the trailing half of `print_statement`.
Read `compute_trailing_spacing` in `fmt/mod.rs` closely: it is the function
whose behavior this task must reproduce exactly. The green input is the
sequence of body child nodes; for each, `trivia::trailing_comment(&node)`
gives the comment or `None`, and the comment's source column comes from
`index.line_col(token.text_range().start)`.

State the run-boundary rule in the new function's doc comment, in prose, as
the C1 code's behavior dictates it — do not restate it from this plan, derive
it from the function you are porting and write down what you found.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: PASS, 21 tests.

Run: `cargo test -p mtc-post-machine`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/fmt/print.rs
git commit -m "feat(post-machine): green-tree fmt printer for trailing comments and alignment runs"
```

---

### Task 6: interior list comments and brace comments

**Files:**
- Modify: `crates/post-machine/src/fmt/print.rs`

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces: `format_green` covers every remaining comment position — between `use` paths, between comma-group items, and after an opening `{`. After this task no `unreachable!` guard remains in `print.rs`.

The spec's payoff lands here: an interior comment is an ordinary token between
two entry nodes, so per-entry trivia is automatic and the C1
re-attachment defect class is structurally unrepresentable. The output must
still be byte-identical to C1's — this task proves the new representation
reproduces the old decisions, it does not change them.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `fmt/print.rs`:

```rust
    #[test]
    fn interior_comments_between_use_paths() {
        same_as_c1("use a::b, // note\n    c::d;\n");
        same_as_c1("use a::b,\n    // own line\n    c::d;\n");
        same_as_c1("use std::goToEnd,\n    // pulled in for the return leg\n    std::goToBegin as backToStart;\n");
    }

    #[test]
    fn interior_comments_between_comma_items() {
        same_as_c1("main() {\n 1: left, // note\n    right;\n}\n");
        same_as_c1("main() {\n 1: left,\n    // own line\n    right;\n}\n");
    }

    #[test]
    fn comments_after_an_opening_brace() {
        same_as_c1("main() { // open\n 1: left;\n}\n");
        same_as_c1("namespace n { // open\n}\n");
        same_as_c1("main() { // open\n    step() { // nested open\n 1: left;\n    }\n}\n");
    }

    #[test]
    fn every_comment_position_at_once() {
        same_as_c1(concat!(
            "// file leader\n\n",
            "use a::b, // interior\n    c::d;\n\n",
            "? doc\n",
            "main() { // open\n",
            "    // leading standalone\n",
            " 1: left, right; // trailing\n\n",
            " 2: check(1, 2);\n",
            "} // close\n"
        ));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: FAIL — on the interior-comment guard left in Task 2, or on an output mismatch.

- [ ] **Step 3: Write the implementation**

Port the interior-comment handling from `print_use` and `render_items`. The
green input is the token sequence between two entry nodes: walk
`children_with_tokens()` of the `USE_DECL` (entries are `USE_PATH` nodes) or of
the `STATEMENT` (entries are `ITEM` nodes) and collect the comment tokens
between them. `newline_before` — which C1 stored on `CommaItem` — is whether
the whitespace before the entry contains a newline.

Remove the last `unreachable!` guard; add a `debug_assert!` in its place only
if you can name a shape it would catch, otherwise leave it out.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-post-machine --lib fmt::print`
Expected: PASS, 25 tests.

Run: `cargo test -p mtc-post-machine`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/fmt/print.rs
git commit -m "feat(post-machine): green-tree fmt printer for interior and brace comments"
```

---

### Task 7: swap `format()` over and delete the C1 printer

**Files:**
- Modify: `crates/post-machine/src/fmt/mod.rs` (delete the C1 printer, delegate)
- Modify: `crates/post-machine/src/fmt/print.rs` (rename the entry point)
- Modify: `crates/post-machine/tests/fmt_programs.rs` (corpus-wide check)

**Interfaces:**
- Consumes: Tasks 1–6 — `format_green` covering every `.pmc` shape.
- Produces: `pub fn format(source: &str) -> Result<String, CompileError>` unchanged in signature and behavior, now green-backed. `fmt/mod.rs` no longer imports anything from `crate::cst`, and `parse_cst` has zero production callers.

- [ ] **Step 1: Write the regression pin**

This task has no red phase and does not pretend to. The swap's real gate is
that every test already in the repo — the 91 inline tests in `fmt/mod.rs`,
`fmt_programs.rs`, `fmt_property.rs`, the goldens and the compiled stdlib —
stays green while the printer underneath them is replaced. The test below is
a **pin**: it passes on arrival (verified against today's C1-backed `format`
before this plan shipped) and exists so that a corpus file the green printer
mishandles fails loudly rather than silently reformatting.

Add to `crates/post-machine/tests/fmt_programs.rs`:

```rust
/// The corpus-wide gate for the green printer: every `.pmc` file the
/// repo ships formats, formats again to the same bytes, and comes back
/// with an unchanged token stream — the whitespace-only contract, checked
/// on real sources rather than generated ones. `fmt_property.rs` proves
/// the same two properties over generated programs, but its generator
/// deliberately emits no namespaces, imports or comments; this covers
/// exactly those.
///
/// Idempotence rather than fmt-cleanliness is the property here because
/// only five of the shipped files are committed in canonical form. The
/// three that are load-bearing already have a stronger pin in
/// `dogfood_stdlib_and_goldens_are_already_fmt_clean`, which stays.
///
/// Verified green against the C1-backed printer before this plan shipped:
/// 13 files, both properties.
#[test]
fn every_shipped_pmc_file_formats_idempotently() {
    use mtc_post_machine::lexer::{LexMode, TokenKind, lex_with};

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex_with(src, LexMode::WithoutComments)
            .expect("lexes")
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    let mut checked = 0;
    for dir in ["tests/golden", "tests/syntax", "tests/lint", "src/stdlib"] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("readable entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("pmc") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable source");
            let once = mtc_post_machine::format(&src)
                .unwrap_or_else(|e| panic!("{} failed to format: {e:?}", path.display()));
            let twice = mtc_post_machine::format(&once)
                .unwrap_or_else(|e| panic!("{} failed to re-format: {e:?}", path.display()));
            assert_eq!(once, twice, "{} is not idempotent", path.display());
            assert_eq!(kinds(&src), kinds(&once), "{} changed tokens", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 13, "expected the whole .pmc corpus, saw {checked}");
}
```

- [ ] **Step 2: Run the pin against the C1 printer**

Run: `cargo test -p mtc-post-machine --test fmt_programs every_shipped_pmc_file`
Expected: PASS, with `checked` reaching at least 13. A pin that fails here is
a broken pin, not a discovery — fix the pin before touching the printer, and
say so in the report.

- [ ] **Step 3: Write the implementation**

1. In `fmt/print.rs`, rename `format_green` to `format` (still `pub(crate)`).
2. In `fmt/mod.rs`, replace the body of the public `format` with a single
   delegation to `print::format(source)`.
3. Delete every C1 printer function from `fmt/mod.rs`: `print_cst`,
   `print_top_items`, `top_wants_blank_before`, `top_item_leads_with_blank`,
   `body_wants_blank_before`, `body_item_leads_with_blank`, `print_top_item`,
   `print_namespace`, `print_use`, `render_use_path`, `normalize_comment_text`,
   `print_comment`, `print_function`, `print_doc_run`, `print_doc_run_line`,
   `print_body_item`, `command_column`, `max_inline_label_prefix_width`,
   `label_prefix_text`, `label_prefix_width`, `label_margin`,
   `render_statement_code`, `print_statement`, `code_line_width_incl_semi`,
   `compute_trailing_spacing`, `render_items`, `render_item`, `LeadingLayout`,
   `layout_leading`, `emit_forced_break`, `line_width_after`,
   `greedy_fill_group`, `builtin_name`, `render_builtin_successor`,
   `render_successor`, `render_check_arm` — together with the
   `use crate::cst::{...}` import and the `parse_cst` import.
4. `fmt/mod.rs`'s inline test module holds 91 tests. **88 of them go through
   `format` and need no change at all.** Exactly three call a deleted helper
   directly, all three calling `command_column`:
   `command_column_namespaced_base_indent`, `command_column_worked_values`,
   and `t_namespace_body_feeds_the_deeper_base_indent_into_command_column`.
   Move those three into `print.rs`'s test module beside the ported
   `command_column`. That count was measured against the file before this
   plan shipped — if you find a fourth, it is one a Task 1-6 commit added.
   Do not delete a test to make the build pass; if one cannot be kept, say so
   in the report and explain why.
5. **`same_as_c1` becomes vacuous the moment `format` delegates** — it would
   compare the green printer against itself and 25 tests would silently
   assert `x == x`. Delete the helper and, with it, the per-surface
   differential tests from Tasks 2-6: their job was to gate the migration
   surface by surface, and that job ends here. The behavior they guarded is
   covered from now on by the 91 inline tests, `fmt_programs.rs`,
   `fmt_property.rs` and the corpus pin. If you believe a specific surface
   loses its only coverage that way, keep THAT test and rewrite it to assert
   an expected output literal — but say which, and why, in the report.
6. Move `INDENT_UNIT` and `LINE_WIDTH` to `print.rs`. After the deletion
   `mod.rs` holds exactly three production items — `format`, and those two
   consts if you leave them — so moving both is the tidier end state. The
   deletion list above plus these two consts plus `format` account for
   **every** production item in `fmt/mod.rs`; that inventory was checked
   against the file before this plan shipped, so anything else you find
   there is something a Task 1-6 commit added, not something the list
   missed.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-post-machine`
Expected: all green, including the 91 kept inline tests, `fmt_programs.rs`,
`fmt_property.rs` and the new corpus test.

Then verify the standing gates, all of which this task can break:

```bash
# no production caller of the C1 parse path remains
grep -rn "parse_cst\|lower_cst" crates/post-machine/src --include='*.rs' | grep -v "^.*: *//" | grep -v "#\[cfg(test)\]"
```
Expected: hits only in `parser.rs` (the definitions and `parse`), `cst.rs`
(doc comments), and test modules — nothing in `fmt/`.

```bash
# compiled-stdlib byte identity, the standing gate for any formatter change
cargo test -p mtc-post-machine --test golden_programs
cargo test -p mtc-post-machine --test compile_programs
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build -p mtc-core --no-default-features
```
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine/src/fmt crates/post-machine/tests/fmt_programs.rs
git commit -m "feat(post-machine): fmt runs the green tree; the C1 printer is gone"
```

---

### Task 8: documentation

**Files:**
- Modify: `CLAUDE.md` (the `### Pipeline and key types` section)
- Modify: `docs/pmt/fmt.md` (only if it describes fmt's input in C1 terms)

**Interfaces:**
- Consumes: Task 7's finished state.
- Produces: no code. The repo's standing-state documentation matches what the code now does.

- [ ] **Step 1: Find every claim this plan falsified**

Run:

```bash
grep -n "parse_cst\|lower_cst\|C1\|CST" CLAUDE.md
grep -rn "parse_cst\|lower_cst\|CST" docs/ --include='*.md'
```

The known one is `CLAUDE.md`'s `### Pipeline and key types` paragraph, which
says fmt is "Still on the C1 path" and describes `parse` as
`lower_cst ∘ parse_cst` with "the CST is fmt's path". Both halves are now
false. Read every other hit and decide: a claim about the *cutover* being
incomplete is still true (the oracle and the `#[cfg(test)]` callers survive
until plan 7); a claim about fmt's input is not.

- [ ] **Step 2: Rewrite the falsified claims**

In `CLAUDE.md`, the paragraph must now say: the compiler front end, the `.pmc`
language service AND fmt all run the green tree; `parse_cst` and `lower_cst`
survive only as the differential oracle and in `#[cfg(test)]` modules of the
optimizer, IR, codegen and parser; the C1 CST goes away at the PM cutover.
Keep the file at standing state — do not narrate this plan's story there.

`docs/` is published documentation: it describes what the tool does, not which
internal tree it uses. Change a page only if it makes a claim that is now
false; if the only hits are about `.pma` (core's assembly CST, out of scope
for this plan), change nothing and say so in the report.

- [ ] **Step 3: Verify nothing else drifted**

Run: `cargo test --workspace`
Expected: all green — `cli_docs.rs` quotes `pmt --help` verbatim and would
catch a help-text drift.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs
git commit -m "docs: fmt runs the green tree"
```

---

## Exit criteria

- `fmt::format` parses through `parse_green_from_tokens` and prints from the
  green tree; `crates/post-machine/src/cst.rs` has no production consumer.
- `parse_cst` and `lower_cst` have **zero** production callers across the
  crate — the precondition plan 7 deletes them under.
- Every fmt fixture, the 13-file `.pmc` corpus, `fmt_property.rs`'s
  idempotence and token-equivalence properties, and the compiled-stdlib
  byte-identity gate are green.
- `crates/core` has a zero-line diff for the whole plan.
- `CLAUDE.md` describes fmt on the green tree.
