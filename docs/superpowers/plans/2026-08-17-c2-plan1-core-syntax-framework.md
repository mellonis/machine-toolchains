# C2 Plan 1: Core Syntax Framework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The language-agnostic green/red syntax-tree framework in `crates/core/src/syntax/` — green value trees, a checkpointing `TreeBuilder`, red cursors with byte ranges, the `AstNode` typed-view contract, and `LineIndex` offset→`Span` conversion — fully tested against a fake kind space.

**Architecture:** Rowan-model lossless trees, hand-rolled and sized to what `.pmc`/`.tmc` need. Green layer = immutable `Rc`-shared nodes/tokens owning their text; red layer = parent+offset cursors created on demand; views = zero-copy casts over red nodes. Core interprets no kinds — each language crate owns its `u16` kind space, mirroring how `vm::Arch` owns opcodes.

**Tech Stack:** Rust, std-gated module in `mtc-core` (`no_std` vm gate untouched), `proptest` dev-dep only. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md` (this plan implements §3; plans 2 and 3 will cover §4–§7 for PM and TM.)

## Global Constraints

- Workspace deps stay `serde`/`serde_json` (std-only) + `proptest` (dev). This plan adds **zero** dependencies.
- `crates/core` carries **zero** `.pmc`/`.tmc` knowledge — all tests use locally-declared fake `SyntaxKind` constants (house convention: each test file defines its own local helpers; there is no shared test-support module).
- The `syntax` module is `#[cfg(feature = "std")]`-gated in `core/src/lib.rs` like the other std modules; `cargo build -p mtc-core --no-default-features` must stay green.
- Offsets and `text_len` are **bytes** (`u32`). Lines and columns are **1-based**; columns count **characters** (the lexers' convention — `Token.len` is chars; span parity in plans 2/3 depends on this).
- Handles are `Rc`-based and deliberately not `Send` (single-threaded front ends, same as rowan).
- The lossless law: `root.text() == source`, byte for byte.
- Every commit: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build -p mtc-core --no-default-features` all green.
- Commit style: conventional with scope, e.g. `feat(core): …` (house style).
- Module docs carry substance in prose until Task 7 lands the `docs/core.md` section; Task 7 then inserts the `docs/core.md (syntax tree)` citations (house documentation-authority rule: never cite a page that doesn't exist yet).

## Branch setup (before Task 1)

```bash
git checkout master && git pull
git checkout -b feat/c2-green-tree
```

(House git rule: branch from an updated default branch, never a stale base.)

---

### Task 1: Green layer (`SyntaxKind`, `GreenToken`, `GreenNode`)

**Files:**
- Create: `crates/core/src/syntax/green.rs`
- Create: `crates/core/src/syntax/mod.rs`
- Modify: `crates/core/src/lib.rs` (add the std-gated module line next to the existing ones)

**Interfaces:**
- Consumes: nothing (first task).
- Produces: `SyntaxKind(pub u16)` (Copy, Eq, Hash, Ord); `GreenToken::new(kind, text) -> Rc<GreenToken>` with `.kind()`, `.text() -> &str`, `.text_len() -> u32`; `GreenElement::{Node(Rc<GreenNode>), Token(Rc<GreenToken>)}` with `.kind()`, `.text_len()`; `GreenNode::new(kind, Vec<GreenElement>) -> Rc<GreenNode>` with `.kind()`, `.text_len()`, `.children() -> &[GreenElement]`, `.text() -> String`.

- [ ] **Step 1: Write the failing tests** (inside `green.rs` as a `#[cfg(test)] mod tests`; the file starts with just the test module and stubs absent so it fails to compile — write the full file in Step 3; for the TDD cycle, first add only the module wiring + tests, watch the build fail on missing types)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    const ROOT: SyntaxKind = SyntaxKind(0);
    const WS: SyntaxKind = SyntaxKind(1);
    const IDENT: SyntaxKind = SyntaxKind(2);
    const LIST: SyntaxKind = SyntaxKind(3);

    #[test]
    fn green_tree_reproduces_text_and_caches_len() {
        let inner = GreenNode::new(
            LIST,
            vec![
                GreenElement::Token(GreenToken::new(IDENT, "ab")),
                GreenElement::Token(GreenToken::new(WS, " ")),
                GreenElement::Token(GreenToken::new(IDENT, "cd")),
            ],
        );
        assert_eq!(inner.text_len(), 5);
        let root = GreenNode::new(
            ROOT,
            vec![
                GreenElement::Node(inner),
                GreenElement::Token(GreenToken::new(WS, "\n")),
            ],
        );
        assert_eq!(root.kind(), ROOT);
        assert_eq!(root.text_len(), 6);
        assert_eq!(root.text(), "ab cd\n");
        assert_eq!(root.children().len(), 2);
    }

    #[test]
    fn text_len_counts_bytes_not_chars() {
        // Offsets across the framework are byte offsets; `LineIndex`
        // owns the char-column conversion.
        let t = GreenToken::new(IDENT, "λ");
        assert_eq!(t.text_len(), 2);
    }

    #[test]
    fn structure_sharing_is_by_rc() {
        let shared = GreenToken::new(IDENT, "x");
        let a = GreenNode::new(LIST, vec![GreenElement::Token(shared.clone())]);
        let b = GreenNode::new(LIST, vec![GreenElement::Token(shared.clone())]);
        assert_eq!(a, b);
        assert_eq!(Rc::strong_count(&shared), 3);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core syntax::green -- --nocapture`
Expected: compile error — `SyntaxKind`/`GreenToken`/`GreenNode` not found.

- [ ] **Step 3: Implement `green.rs`, `mod.rs`, and the `lib.rs` wiring**

`crates/core/src/syntax/green.rs`:

```rust
//! Green layer of the syntax framework: immutable, structure-shared
//! value trees. A green tree owns its exact source text and knows
//! nothing about absolute positions — the red layer (`red.rs`) adds
//! offsets on top. Whitespace and comments are ordinary tokens; there
//! is no side-channel trivia storage, so the lossless contract is one
//! law: the root's `text()` equals the source, byte for byte.
//!
//! Language-agnostic: kinds are opaque `u16`s owned by the language
//! crates, mirroring how `vm::Arch` owns its opcodes.

use std::rc::Rc;

/// An opaque syntax-kind tag. Each language crate owns its kind space
/// (tokens and nodes share one space); core only ever compares kinds
/// for equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SyntaxKind(pub u16);

/// A leaf: one token and its exact source text.
#[derive(Debug, PartialEq, Eq)]
pub struct GreenToken {
    kind: SyntaxKind,
    text: String,
}

impl GreenToken {
    pub fn new(kind: SyntaxKind, text: impl Into<String>) -> Rc<GreenToken> {
        Rc::new(GreenToken {
            kind,
            text: text.into(),
        })
    }

    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Length in BYTES (offsets across the framework are byte offsets;
    /// line/col conversion happens in `LineIndex`).
    pub fn text_len(&self) -> u32 {
        self.text.len() as u32
    }
}

/// One child slot of a green node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreenElement {
    Node(Rc<GreenNode>),
    Token(Rc<GreenToken>),
}

impl GreenElement {
    pub fn kind(&self) -> SyntaxKind {
        match self {
            GreenElement::Node(n) => n.kind(),
            GreenElement::Token(t) => t.kind(),
        }
    }

    pub fn text_len(&self) -> u32 {
        match self {
            GreenElement::Node(n) => n.text_len(),
            GreenElement::Token(t) => t.text_len(),
        }
    }
}

/// An interior node: a kind plus children, with the subtree's total
/// text length cached so red-layer offset math is O(children), never
/// O(subtree).
#[derive(Debug, PartialEq, Eq)]
pub struct GreenNode {
    kind: SyntaxKind,
    text_len: u32,
    children: Vec<GreenElement>,
}

impl GreenNode {
    pub fn new(kind: SyntaxKind, children: Vec<GreenElement>) -> Rc<GreenNode> {
        let text_len = children.iter().map(GreenElement::text_len).sum();
        Rc::new(GreenNode {
            kind,
            text_len,
            children,
        })
    }

    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    pub fn text_len(&self) -> u32 {
        self.text_len
    }

    pub fn children(&self) -> &[GreenElement] {
        &self.children
    }

    /// The subtree's full text.
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(self.text_len as usize);
        self.write_text(&mut out);
        out
    }

    fn write_text(&self, out: &mut String) {
        for c in &self.children {
            match c {
                GreenElement::Token(t) => out.push_str(t.text()),
                GreenElement::Node(n) => n.write_text(out),
            }
        }
    }
}
```

`crates/core/src/syntax/mod.rs`:

```rust
//! Language-agnostic lossless syntax trees: an immutable green layer,
//! offset-carrying red cursors, a builder for recursive-descent
//! parsers, and the typed-view (`AstNode`) contract. Kinds are opaque
//! per-language `u16` spaces; core interprets none of them (the same
//! contract the VM core keeps for opcodes and the assembler for
//! dialects). Handles are `Rc`-based — single-threaded by design.

mod green;

pub use green::{GreenElement, GreenNode, GreenToken, SyntaxKind};
```

`crates/core/src/lib.rs` — add alongside the existing std-gated module lines:

```rust
#[cfg(feature = "std")]
pub mod syntax;
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-core syntax::green` — Expected: 3 passed.
Run: `cargo build -p mtc-core --no-default-features` — Expected: green (module is std-gated).

- [ ] **Step 5: Commit** (requires the user's standing commit authorization for this plan)

```bash
git add crates/core/src/syntax/ crates/core/src/lib.rs
git commit -m "feat(core): green layer of the syntax framework"
```

---

### Task 2: `TreeBuilder` with checkpoints

**Files:**
- Create: `crates/core/src/syntax/builder.rs`
- Modify: `crates/core/src/syntax/mod.rs` (add `mod builder;` + re-export)

**Interfaces:**
- Consumes: Task 1's `SyntaxKind`, `GreenElement`, `GreenNode`, `GreenToken`.
- Produces: `TreeBuilder::new()`, `.token(kind, &str)`, `.start_node(kind)`, `.finish_node()`, `.checkpoint() -> Checkpoint`, `.start_node_at(Checkpoint, kind)`, `.finish(self) -> Rc<GreenNode>`; `Checkpoint` (Copy).

- [ ] **Step 1: Write the failing tests** (in `builder.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{GreenElement, SyntaxKind};

    const ROOT: SyntaxKind = SyntaxKind(0);
    const WS: SyntaxKind = SyntaxKind(1);
    const IDENT: SyntaxKind = SyntaxKind(2);
    const COMMA: SyntaxKind = SyntaxKind(3);
    const LIST: SyntaxKind = SyntaxKind(4);

    #[test]
    fn builds_nested_tree_in_document_order() {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        b.start_node(LIST);
        b.token(IDENT, "a");
        b.token(COMMA, ",");
        b.token(WS, " ");
        b.token(IDENT, "b");
        b.finish_node();
        b.token(WS, "\n");
        b.finish_node();
        let root = b.finish();
        assert_eq!(root.text(), "a, b\n");
        assert_eq!(root.children().len(), 2);
        assert_eq!(root.children()[0].kind(), LIST);
    }

    #[test]
    fn checkpoint_wraps_an_already_emitted_prefix() {
        // The parser emits `a` before knowing a comma follows; the
        // checkpoint retro-wraps `a` into the LIST node — the standard
        // fix for "this is only a list once I see the comma".
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        let cp = b.checkpoint();
        b.token(IDENT, "a");
        b.start_node_at(cp, LIST);
        b.token(COMMA, ",");
        b.token(IDENT, "b");
        b.finish_node();
        b.finish_node();
        let root = b.finish();
        assert_eq!(root.text(), "a,b");
        let GreenElement::Node(list) = &root.children()[0] else {
            panic!("expected the LIST node first");
        };
        assert_eq!(list.kind(), LIST);
        assert_eq!(list.text(), "a,b");
    }

    #[test]
    #[should_panic(expected = "unfinished node")]
    fn finish_with_open_frame_panics() {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        let _ = b.finish();
    }

    #[test]
    #[should_panic(expected = "checkpoint predates")]
    fn checkpoint_predating_an_open_frame_panics() {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        let cp = b.checkpoint();
        b.token(IDENT, "a");
        b.start_node(LIST); // opens AFTER the checkpoint's position
        b.start_node_at(cp, LIST);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core syntax::builder`
Expected: compile error — `TreeBuilder` not found.

- [ ] **Step 3: Implement `builder.rs`**

```rust
//! `TreeBuilder`: the emission surface a recursive-descent parser
//! drives. `start_node`/`finish_node` bracket children in document
//! order; `checkpoint` + `start_node_at` retro-wrap an already-emitted
//! prefix once the parser knows the enclosing kind. Balance errors are
//! panics: an unbalanced build is a parser bug, never an input error.

use std::rc::Rc;

use super::green::{GreenElement, GreenNode, GreenToken, SyntaxKind};

/// A mark into the builder's pending children; see
/// [`TreeBuilder::start_node_at`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint(usize);

#[derive(Debug, Default)]
pub struct TreeBuilder {
    /// Open frames: (kind, index into `children` where the frame began).
    parents: Vec<(SyntaxKind, usize)>,
    /// Pending children of the innermost open frame(s), flat until a
    /// `finish_node` folds a suffix into a node.
    children: Vec<GreenElement>,
}

impl TreeBuilder {
    pub fn new() -> TreeBuilder {
        TreeBuilder::default()
    }

    pub fn token(&mut self, kind: SyntaxKind, text: &str) {
        self.children
            .push(GreenElement::Token(GreenToken::new(kind, text)));
    }

    pub fn start_node(&mut self, kind: SyntaxKind) {
        self.parents.push((kind, self.children.len()));
    }

    pub fn finish_node(&mut self) {
        let (kind, first) = self
            .parents
            .pop()
            .expect("finish_node without a matching start_node");
        let node = GreenNode::new(kind, self.children.split_off(first));
        self.children.push(GreenElement::Node(node));
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint(self.children.len())
    }

    /// Open a node RETROACTIVELY at `cp`: everything emitted since the
    /// checkpoint becomes the new node's leading children. The
    /// checkpoint must not predate the innermost open frame.
    pub fn start_node_at(&mut self, cp: Checkpoint, kind: SyntaxKind) {
        assert!(
            cp.0 <= self.children.len(),
            "checkpoint points past the emitted children"
        );
        if let Some(&(_, first)) = self.parents.last() {
            assert!(
                cp.0 >= first,
                "checkpoint predates the innermost open frame"
            );
        }
        self.parents.push((kind, cp.0));
    }

    /// Close the build. Exactly one root node must remain.
    pub fn finish(mut self) -> Rc<GreenNode> {
        assert!(self.parents.is_empty(), "unfinished node at finish()");
        assert!(
            self.children.len() == 1,
            "finish() requires exactly one root element"
        );
        match self.children.pop().expect("length checked above") {
            GreenElement::Node(n) => n,
            GreenElement::Token(_) => panic!("the root must be a node, not a token"),
        }
    }
}
```

`mod.rs` additions:

```rust
mod builder;

pub use builder::{Checkpoint, TreeBuilder};
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-core syntax::builder` — Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/syntax/
git commit -m "feat(core): TreeBuilder with retroactive checkpoints"
```

---

### Task 3: Red layer (`TextRange`, `SyntaxNode`, `SyntaxToken`)

**Files:**
- Create: `crates/core/src/syntax/red.rs`
- Modify: `crates/core/src/syntax/mod.rs` (add `mod red;` + re-export)

**Interfaces:**
- Consumes: Tasks 1–2 (`GreenNode` via `TreeBuilder` in tests).
- Produces: `TextRange { pub start: u32, pub end: u32 }` with `new`, `.len()`, `.is_empty()`, `.contains(u32) -> bool`; `SyntaxNode::new_root(Rc<GreenNode>)` with `.kind()`, `.text_range()`, `.parent() -> Option<SyntaxNode>`, `.green() -> &Rc<GreenNode>`, `.text() -> String`, `.children() -> impl Iterator<Item = SyntaxNode>`, `.children_with_tokens() -> impl Iterator<Item = SyntaxElement>`; `SyntaxToken` with `.kind()`, `.text() -> &str`, `.text_range()`, `.parent() -> SyntaxNode`; `SyntaxElement::{Node, Token}` with `.kind()`, `.text_range()`. Node/token equality = positional identity (same green `Rc` at the same offset).

- [ ] **Step 1: Write the failing tests** (in `red.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{SyntaxKind, TreeBuilder};

    const ROOT: SyntaxKind = SyntaxKind(0);
    const WS: SyntaxKind = SyntaxKind(1);
    const IDENT: SyntaxKind = SyntaxKind(2);
    const LIST: SyntaxKind = SyntaxKind(3);

    /// `f λx` with the two idents wrapped in a LIST: exercises offsets
    /// across a multi-byte char, node/token interleaving, and parents.
    fn sample() -> SyntaxNode {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        b.start_node(LIST);
        b.token(IDENT, "f");
        b.token(WS, " ");
        b.token(IDENT, "λx");
        b.finish_node();
        b.token(WS, "\n");
        b.finish_node();
        SyntaxNode::new_root(b.finish())
    }

    #[test]
    fn ranges_are_absolute_byte_ranges() {
        let root = sample();
        assert_eq!(root.text_range(), TextRange::new(0, 6)); // "f λx\n" = 1+1+3+1
        let list = root.children().next().expect("LIST child");
        assert_eq!(list.kind(), LIST);
        assert_eq!(list.text_range(), TextRange::new(0, 5));
        assert_eq!(list.text(), "f λx");

        let tokens: Vec<SyntaxToken> = list
            .children_with_tokens()
            .filter_map(|e| match e {
                SyntaxElement::Token(t) => Some(t),
                SyntaxElement::Node(_) => None,
            })
            .collect();
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[2].text(), "λx");
        assert_eq!(tokens[2].text_range(), TextRange::new(2, 5));
    }

    #[test]
    fn parent_links_go_back_up() {
        let root = sample();
        let list = root.children().next().expect("LIST child");
        assert_eq!(list.parent().expect("has parent"), root);
        assert!(root.parent().is_none());
        let first_tok = list
            .children_with_tokens()
            .next()
            .expect("has children");
        assert_eq!(first_tok.text_range(), TextRange::new(0, 1));
    }

    #[test]
    fn equality_is_positional_identity() {
        let root = sample();
        let a = root.children().next().expect("LIST");
        let b = root.children().next().expect("LIST again");
        assert_eq!(a, b); // same green node, same offset
        assert_ne!(a, root);
    }

    #[test]
    fn children_ranges_tile_the_parent() {
        let root = sample();
        let mut cursor = root.text_range().start;
        for e in root.children_with_tokens() {
            assert_eq!(e.text_range().start, cursor);
            cursor = e.text_range().end;
        }
        assert_eq!(cursor, root.text_range().end);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core syntax::red`
Expected: compile error — `SyntaxNode`/`TextRange` not found.

- [ ] **Step 3: Implement `red.rs`**

```rust
//! Red layer: position-carrying cursors over a green tree. A
//! `SyntaxNode` is a cheap-to-clone `Rc` handle knowing its absolute
//! byte range, parent, and children; red nodes are created on demand
//! while walking, never stored in the green tree. Equality is
//! positional identity: the same green node at the same offset.

use std::rc::Rc;

use super::green::{GreenElement, GreenNode, GreenToken, SyntaxKind};

/// A half-open byte range `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    pub start: u32,
    pub end: u32,
}

impl TextRange {
    pub fn new(start: u32, end: u32) -> TextRange {
        debug_assert!(start <= end);
        TextRange { start, end }
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn contains(&self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }
}

#[derive(Debug, Clone)]
pub struct SyntaxNode(Rc<NodeData>);

#[derive(Debug)]
struct NodeData {
    green: Rc<GreenNode>,
    parent: Option<SyntaxNode>,
    offset: u32,
}

impl PartialEq for SyntaxNode {
    fn eq(&self, other: &SyntaxNode) -> bool {
        self.0.offset == other.0.offset && Rc::ptr_eq(&self.0.green, &other.0.green)
    }
}

impl Eq for SyntaxNode {}

impl SyntaxNode {
    pub fn new_root(green: Rc<GreenNode>) -> SyntaxNode {
        SyntaxNode(Rc::new(NodeData {
            green,
            parent: None,
            offset: 0,
        }))
    }

    pub fn kind(&self) -> SyntaxKind {
        self.0.green.kind()
    }

    pub fn text_range(&self) -> TextRange {
        TextRange::new(self.0.offset, self.0.offset + self.0.green.text_len())
    }

    pub fn parent(&self) -> Option<SyntaxNode> {
        self.0.parent.clone()
    }

    pub fn green(&self) -> &Rc<GreenNode> {
        &self.0.green
    }

    pub fn text(&self) -> String {
        self.0.green.text()
    }

    /// All children — nodes and tokens — in document order.
    pub fn children_with_tokens(&self) -> impl Iterator<Item = SyntaxElement> + '_ {
        let mut offset = self.0.offset;
        self.0.green.children().iter().map(move |c| {
            let at = offset;
            offset += c.text_len();
            match c {
                GreenElement::Node(n) => SyntaxElement::Node(SyntaxNode(Rc::new(NodeData {
                    green: n.clone(),
                    parent: Some(self.clone()),
                    offset: at,
                }))),
                GreenElement::Token(t) => SyntaxElement::Token(SyntaxToken {
                    green: t.clone(),
                    parent: self.clone(),
                    offset: at,
                }),
            }
        })
    }

    /// Child NODES only, in document order.
    pub fn children(&self) -> impl Iterator<Item = SyntaxNode> + '_ {
        self.children_with_tokens().filter_map(|e| match e {
            SyntaxElement::Node(n) => Some(n),
            SyntaxElement::Token(_) => None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SyntaxToken {
    green: Rc<GreenToken>,
    parent: SyntaxNode,
    offset: u32,
}

impl PartialEq for SyntaxToken {
    fn eq(&self, other: &SyntaxToken) -> bool {
        self.offset == other.offset && Rc::ptr_eq(&self.green, &other.green)
    }
}

impl Eq for SyntaxToken {}

impl SyntaxToken {
    pub fn kind(&self) -> SyntaxKind {
        self.green.kind()
    }

    pub fn text(&self) -> &str {
        self.green.text()
    }

    pub fn text_range(&self) -> TextRange {
        TextRange::new(self.offset, self.offset + self.green.text_len())
    }

    pub fn parent(&self) -> SyntaxNode {
        self.parent.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxElement {
    Node(SyntaxNode),
    Token(SyntaxToken),
}

impl SyntaxElement {
    pub fn kind(&self) -> SyntaxKind {
        match self {
            SyntaxElement::Node(n) => n.kind(),
            SyntaxElement::Token(t) => t.kind(),
        }
    }

    pub fn text_range(&self) -> TextRange {
        match self {
            SyntaxElement::Node(n) => n.text_range(),
            SyntaxElement::Token(t) => t.text_range(),
        }
    }
}
```

`mod.rs` additions:

```rust
mod red;

pub use red::{SyntaxElement, SyntaxNode, SyntaxToken, TextRange};
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-core syntax::red` — Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/syntax/
git commit -m "feat(core): red-layer cursors with byte ranges"
```

---

### Task 4: Typed-view contract (`AstNode`, `ast_node!`, lookup helpers)

**Files:**
- Create: `crates/core/src/syntax/ast.rs`
- Modify: `crates/core/src/syntax/mod.rs` (add `mod ast;` + re-export)

**Interfaces:**
- Consumes: Tasks 1 and 3 (`SyntaxKind`, `SyntaxNode`, `SyntaxToken`, `SyntaxElement`).
- Produces: `trait AstNode { fn can_cast(SyntaxKind) -> bool; fn cast(SyntaxNode) -> Option<Self>; fn syntax(&self) -> &SyntaxNode; }`; crate-root macro `ast_node!(pub struct Name: KIND_EXPR)`; helpers `syntax::child::<N>(&SyntaxNode) -> Option<N>`, `syntax::children::<N>(&SyntaxNode) -> impl Iterator<Item = N>`, `syntax::token(&SyntaxNode, SyntaxKind) -> Option<SyntaxToken>`. Plans 2/3 build every language view with exactly these.

- [ ] **Step 1: Write the failing tests** (in `ast.rs`)

```rust
#[cfg(test)]
mod tests {
    use crate::ast_node;
    use crate::syntax::{self, AstNode, SyntaxKind, SyntaxNode, TreeBuilder};

    const ROOT: SyntaxKind = SyntaxKind(0);
    const IDENT: SyntaxKind = SyntaxKind(1);
    const COMMA: SyntaxKind = SyntaxKind(2);
    const LIST: SyntaxKind = SyntaxKind(3);
    const ENTRY: SyntaxKind = SyntaxKind(4);

    ast_node!(pub struct ListView: LIST);
    ast_node!(pub struct EntryView: ENTRY);

    /// `(x,y)`-shaped tree: ROOT > LIST > [ENTRY("x"), COMMA, ENTRY("y")].
    fn sample() -> SyntaxNode {
        let mut b = TreeBuilder::new();
        b.start_node(ROOT);
        b.start_node(LIST);
        b.start_node(ENTRY);
        b.token(IDENT, "x");
        b.finish_node();
        b.token(COMMA, ",");
        b.start_node(ENTRY);
        b.token(IDENT, "y");
        b.finish_node();
        b.finish_node();
        b.finish_node();
        SyntaxNode::new_root(b.finish())
    }

    #[test]
    fn cast_checks_the_kind() {
        let root = sample();
        assert!(ListView::cast(root.clone()).is_none());
        let list_node = root.children().next().expect("LIST child");
        let list = ListView::cast(list_node).expect("casts");
        assert_eq!(list.syntax().kind(), LIST);
    }

    #[test]
    fn child_and_children_find_by_view_type() {
        let root = sample();
        let list: ListView = syntax::child(&root).expect("LIST under ROOT");
        let entries: Vec<EntryView> = syntax::children(list.syntax()).collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].syntax().text(), "x");
        assert_eq!(entries[1].syntax().text(), "y");
    }

    #[test]
    fn token_finds_by_kind() {
        let root = sample();
        let list: ListView = syntax::child(&root).expect("LIST under ROOT");
        let comma = syntax::token(list.syntax(), COMMA).expect("comma token");
        assert_eq!(comma.text(), ",");
        assert!(syntax::token(list.syntax(), IDENT).is_none()); // idents sit inside ENTRY nodes
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core syntax::ast`
Expected: compile error — `AstNode`/`ast_node!` not found.

- [ ] **Step 3: Implement `ast.rs`**

```rust
//! The typed-view contract: a view is a zero-copy wrapper over a
//! `SyntaxNode` of a known kind. Concrete views live in the language
//! crates; core owns only the casting contract, the declaration macro,
//! and the child/token lookup helpers views are written with.

use super::green::SyntaxKind;
use super::red::{SyntaxElement, SyntaxNode, SyntaxToken};

pub trait AstNode: Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(node: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
}

/// Declare a typed view struct over one syntax kind:
///
/// ```ignore
/// ast_node!(pub struct FunctionNode: kinds::FUNCTION);
/// ```
#[macro_export]
macro_rules! ast_node {
    ($(#[$attr:meta])* pub struct $name:ident: $kind:expr) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name {
            syntax: $crate::syntax::SyntaxNode,
        }

        impl $crate::syntax::AstNode for $name {
            fn can_cast(kind: $crate::syntax::SyntaxKind) -> bool {
                kind == $kind
            }

            fn cast(node: $crate::syntax::SyntaxNode) -> Option<Self> {
                if <Self as $crate::syntax::AstNode>::can_cast(node.kind()) {
                    Some($name { syntax: node })
                } else {
                    None
                }
            }

            fn syntax(&self) -> &$crate::syntax::SyntaxNode {
                &self.syntax
            }
        }
    };
}

/// First child node castable to `N`.
pub fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
    parent.children().find_map(N::cast)
}

/// All child nodes castable to `N`, in document order.
pub fn children<'a, N: AstNode + 'a>(parent: &'a SyntaxNode) -> impl Iterator<Item = N> + 'a {
    parent.children().filter_map(N::cast)
}

/// First child TOKEN of the given kind (direct children only — a
/// view's own token, never a grandchild's).
pub fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    parent.children_with_tokens().find_map(|e| match e {
        SyntaxElement::Token(t) if t.kind() == kind => Some(t),
        _ => None,
    })
}
```

`mod.rs` additions:

```rust
mod ast;

pub use ast::{child, children, token, AstNode};
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-core syntax::ast` — Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/syntax/
git commit -m "feat(core): AstNode typed-view contract and ast_node! macro"
```

---

### Task 5: `LineIndex` (byte offset → 1-based line / char column `Span`)

**Files:**
- Create: `crates/core/src/syntax/line_index.rs`
- Modify: `crates/core/src/syntax/mod.rs` (add `mod line_index;` + re-export)

**Interfaces:**
- Consumes: Task 3's `TextRange`; `crate::diagnostics::Span` (existing, unchanged).
- Produces: `LineIndex::new(&str)`, `.line_col(u32) -> (u32, u32)` (1-based line, 1-based CHAR column), `.span(TextRange) -> Span`. Plans 2/3 route every diagnostic/view span through this; span parity with the old parsers hangs on the char-column convention.

- [ ] **Step 1: Write the failing tests** (in `line_index.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::TextRange;

    #[test]
    fn lines_and_columns_are_one_based() {
        let idx = LineIndex::new("ab\ncd\n");
        assert_eq!(idx.line_col(0), (1, 1));
        assert_eq!(idx.line_col(1), (1, 2));
        assert_eq!(idx.line_col(2), (1, 3)); // the '\n' itself: end-exclusive col
        assert_eq!(idx.line_col(3), (2, 1));
        assert_eq!(idx.line_col(6), (3, 1)); // end-of-text after trailing newline
    }

    #[test]
    fn columns_count_chars_not_bytes() {
        // "λx" — λ is 2 bytes, 1 char; the lexer's Token.len counts
        // chars, so span parity requires char columns.
        let idx = LineIndex::new("λx");
        assert_eq!(idx.line_col(0), (1, 1));
        assert_eq!(idx.line_col(2), (1, 2)); // byte offset 2 = after λ
        assert_eq!(idx.line_col(3), (1, 3)); // end of text
    }

    #[test]
    fn span_matches_the_lexers_end_exclusive_convention() {
        // Token "cd" on line 2 at col 1: the lexer builds
        // Span::new(2, 1, 2, 3); the byte range through LineIndex must
        // produce the identical Span.
        let idx = LineIndex::new("ab\ncd\n");
        let span = idx.span(TextRange::new(3, 5));
        assert_eq!(span, crate::diagnostics::Span::new(2, 1, 2, 3));
    }

    #[test]
    fn empty_text_has_one_line() {
        let idx = LineIndex::new("");
        assert_eq!(idx.line_col(0), (1, 1));
    }
}
```

(Note: this test compares `Span` values with `==` — check `Span`/`Pos` derive `PartialEq` in `crates/core/src/diagnostics.rs` first; if they don't, add `PartialEq` to their derive lists in the same commit — an additive change.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core syntax::line_index`
Expected: compile error — `LineIndex` not found.

- [ ] **Step 3: Implement `line_index.rs`**

```rust
//! Byte-offset → line/column conversion for diagnostics. Lines are
//! 1-based; columns are 1-based CHARACTER counts — the convention the
//! lexers' tokens already carry (`Token.len` is chars), so spans built
//! through a `LineIndex` are byte-identical to lexer-built spans. Span
//! parity across the front-end migrations is a test-pinned contract.

use crate::diagnostics::Span;

use super::red::TextRange;

pub struct LineIndex {
    text: String,
    /// Byte offset of each line's first byte; `line_starts[0] == 0`.
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        LineIndex {
            text: text.to_owned(),
            line_starts,
        }
    }

    /// 1-based (line, char-column) of a byte offset. The offset must
    /// lie on a char boundary; the end-of-text offset is valid.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let line_ix = self.line_starts.partition_point(|&s| s <= offset) - 1;
        let line_start = self.line_starts[line_ix] as usize;
        let col = self.text[line_start..offset as usize].chars().count() as u32 + 1;
        (line_ix as u32 + 1, col)
    }

    /// The `Span` of a byte range — end-exclusive columns, matching the
    /// lexer's `Token::span` convention.
    pub fn span(&self, range: TextRange) -> Span {
        let (sl, sc) = self.line_col(range.start);
        let (el, ec) = self.line_col(range.end);
        Span::new(sl, sc, el, ec)
    }
}
```

`mod.rs` additions:

```rust
mod line_index;

pub use line_index::LineIndex;
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p mtc-core syntax::line_index` — Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/syntax/ crates/core/src/diagnostics.rs
git commit -m "feat(core): LineIndex byte-offset to Span conversion"
```

---

### Task 6: Property tests over a fake kind space

**Files:**
- Create: `crates/core/tests/syntax_props.rs`

**Interfaces:**
- Consumes: everything Tasks 1–5 export from `mtc_core::syntax`.
- Produces: the framework's standing regression properties (these outlive the migration — spec §6.1 law (a) lives here).

- [ ] **Step 1: Write the property tests**

```rust
//! Property tests for the core syntax framework, over a fake kind
//! space — core keeps zero language knowledge (crate contract; the
//! VM core proves the same with its fake arch).

use std::rc::Rc;

use mtc_core::syntax::{
    GreenElement, GreenNode, GreenToken, LineIndex, SyntaxElement, SyntaxKind, SyntaxNode,
};
use proptest::prelude::*;

const NODE: SyntaxKind = SyntaxKind(100);
const TOKEN: SyntaxKind = SyntaxKind(101);

/// Token text: short runs incl. spaces, newlines, and a multi-byte
/// char, so offsets cross UTF-8 boundaries and line breaks.
fn arb_token_text() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[abλ \\n]{0,6}").expect("valid regex")
}

fn arb_green() -> impl Strategy<Value = Rc<GreenNode>> {
    let leaf = proptest::collection::vec(arb_token_text(), 0..5).prop_map(|texts| {
        GreenNode::new(
            NODE,
            texts
                .into_iter()
                .map(|t| GreenElement::Token(GreenToken::new(TOKEN, t)))
                .collect(),
        )
    });
    leaf.prop_recursive(3, 24, 4, |inner| {
        (
            proptest::collection::vec(
                prop_oneof![
                    inner.prop_map(GreenElement::Node),
                    arb_token_text().prop_map(|t| GreenElement::Token(GreenToken::new(TOKEN, t))),
                ],
                0..4,
            ),
        )
            .prop_map(|(children,)| GreenNode::new(NODE, children))
    })
}

/// Recursively assert that children's ranges tile the parent's range
/// exactly, in document order, at every level.
fn assert_tiling(node: &SyntaxNode) {
    let mut cursor = node.text_range().start;
    for e in node.children_with_tokens() {
        assert_eq!(e.text_range().start, cursor, "gap or overlap in tiling");
        cursor = e.text_range().end;
        if let SyntaxElement::Node(n) = e {
            assert_eq!(n.parent().expect("child has parent"), *node);
            assert_tiling(&n);
        }
    }
    assert_eq!(cursor, node.text_range().end, "children under-cover the node");
}

proptest! {
    /// The lossless law's structural half: cached lengths always equal
    /// real text lengths.
    #[test]
    fn text_len_is_the_text_byte_length(root in arb_green()) {
        prop_assert_eq!(root.text_len() as usize, root.text().len());
    }

    /// Red ranges tile parents exactly and parent links invert
    /// children, at every depth.
    #[test]
    fn red_ranges_tile_and_parents_invert(root in arb_green()) {
        assert_tiling(&SyntaxNode::new_root(root));
    }

    /// LineIndex agrees with a naive char-by-char scan at every char
    /// boundary plus the end-of-text offset.
    #[test]
    fn line_index_matches_a_naive_scan(text in "[abλ\\n]{0,40}") {
        let idx = LineIndex::new(&text);
        let mut line = 1u32;
        let mut col = 1u32;
        for (byte_off, ch) in text.char_indices() {
            prop_assert_eq!(idx.line_col(byte_off as u32), (line, col));
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        prop_assert_eq!(idx.line_col(text.len() as u32), (line, col));
    }
}
```

- [ ] **Step 2: Run to verify the properties hold**

Run: `cargo test -p mtc-core --test syntax_props`
Expected: 3 passed (256 cases each by default). If a property fails, the framework has a real bug — fix the framework, never weaken the property.

- [ ] **Step 3: Commit**

```bash
git add crates/core/tests/syntax_props.rs
git commit -m "test(core): syntax framework property tests (lossless law, tiling, LineIndex)"
```

---

### Task 7: `docs/core.md` section, citations, full gate sweep

**Files:**
- Modify: `docs/core.md` (new top-level section, placed after the existing assembler/linker material, before the LSP framework section — adjust to the page's actual section order when editing)
- Modify: `crates/core/src/syntax/{mod,green,builder,red,ast,line_index}.rs` (insert the durable-page citation into each module doc's first paragraph)

**Interfaces:**
- Consumes: the shipped framework (Tasks 1–6).
- Produces: the durable page anchor `docs/core.md (syntax tree)` that all plan-2/3 code will cite.

- [ ] **Step 1: Add the section to `docs/core.md`**

Append this section (verify each claim against the code as written — the house docs rule; adjust prose only if the code differs):

```markdown
## Syntax trees

The framework in `syntax/` provides language-agnostic lossless syntax
trees for the toolchains' source languages. It follows the green/red
model: an immutable **green tree** owns the exact source text — every
token, including whitespace and comments, is an ordinary leaf, so the
root's text equals the source byte for byte (the lossless law, held by
property test) — and **red cursors** materialize on demand while
walking, each knowing its absolute byte range and its parent.

Kinds are opaque: `SyntaxKind` is a `u16` newtype whose values each
language crate defines for itself, exactly as the VM core executes
micro-ops without knowing opcodes. Core compares kinds only for
equality; its own tests run against a fake kind space.

Parsers emit trees through `TreeBuilder` — `start_node`/`finish_node`
bracket children in document order, and a `checkpoint` lets a
recursive-descent parser wrap an already-emitted prefix once it knows
the enclosing kind. Balance errors are panics: an unbalanced build is
a parser bug, not an input error.

Typed access goes through the `AstNode` contract: a view is a
zero-copy wrapper over a node of a known kind, declared with the
`ast_node!` macro and written with the `child`/`children`/`token`
lookup helpers. Concrete views live in the language crates.

Positions are byte offsets (`TextRange`, half-open). Diagnostics keep
the toolchains' line/column `Span` as their currency: a `LineIndex`
built from the source converts byte offsets to 1-based lines and
1-based **character** columns — the same counting the lexers use, so
spans built either way are identical.

Handles are `Rc`-based and single-threaded, matching the front ends
that use them.
```

- [ ] **Step 2: Insert citations into the six module docs**

In each of `mod.rs`, `green.rs`, `builder.rs`, `red.rs`, `ast.rs`, `line_index.rs`, extend the module doc's first paragraph with the citation ` — docs/core.md (syntax tree)` (house citation form: page + parenthetical topic keyword). Example for `green.rs`:

```rust
//! Green layer of the syntax framework — docs/core.md (syntax tree).
```

(Keep the rest of each module doc's prose; the citation supplements, never replaces.)

- [ ] **Step 3: Full gate sweep**

Run each; all must be green:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build -p mtc-core --no-default-features
```

Expected: everything passes; the no_std gate proves the std-gating held.

- [ ] **Step 4: Verify the docs claims against the tools**

Re-read the new `docs/core.md` section next to the code; every sentence must be true of the shipped implementation (the house per-page claim-verification rule). Fix prose or code mismatches now.

- [ ] **Step 5: Commit**

```bash
git add docs/core.md crates/core/src/syntax/
git commit -m "docs(core): syntax-tree framework section + module citations"
```

---

## Completion criteria for Plan 1

- All 7 tasks committed on `feat/c2-green-tree`; every commit passed the full gate set.
- `mtc_core::syntax` exports exactly: `SyntaxKind`, `GreenToken`, `GreenNode`, `GreenElement`, `TreeBuilder`, `Checkpoint`, `SyntaxNode`, `SyntaxToken`, `SyntaxElement`, `TextRange`, `AstNode`, `child`, `children`, `token`, `LineIndex` (+ the crate-root `ast_node!` macro).
- No PM/TM code touched. Plan 2 (PM migration) is written next, against these exact interfaces.
