# C2 Plan 8 — `.tmc` typed views and extraction parity

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the `.tmc` green tree a typed-view layer and an extraction function that produces the same `Program` the CST path does, proven equal over the shipped corpus **and over generated programs** — so the plans that migrate the compiler front, the language service and the formatter have something to stand on.

**Architecture:** Views are thin, zero-copy accessors declared with the core `ast_node!` macro: each wraps a `SyntaxNode` of one kind and walks direct children. They own no grammar and store nothing. On top of them, `extract_program` rebuilds the existing `Program` AST from the tree. Its correctness is not argued — it is held to a **differential oracle**: `extract_program(tree) == lower_cst(parse_cst(tokens))`, struct-equal, over every `.tmc` the repo ships and over programs from the generator plan 7 built.

**Tech Stack:** Rust (toolchain pinned by `rust-toolchain.toml`), `mtc-core`'s `syntax` framework (`AstNode`, the `ast_node!` macro, red-tree navigation), `proptest` as a dev-dependency.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md` — read §4.3 (views), §4.4 (the owned compiler vocabulary) and §6 (oracles and gates) before Task 1.

## Global Constraints

- **`crates/core` gets no diff.** The framework is complete; six `.pmc` plans added nothing to it after the first. If you believe a navigation primitive is genuinely missing, **stop and report it** rather than adding one — that is a ruling for the coordinator, not a convenience.
- **`crates/post-machine` gets no diff.** Read it constantly; never edit it.
- **The parser is not modified.** Plan 7 finished the tree. If a view seems to need a node the tree does not have, that is a finding to report, not a parser change to make.
- **Nothing in production consumes the tree at the end of this plan either.** Views and extraction are built and proven; routing consumers onto them is plans 9-11. `parse_cst` remains every consumer's path.
- **The oracle is the deliverable, not the extraction.** An `extract_program` that passes over nine corpus files and nothing else has not been shown to work. Task 6 is what this plan is for.
- **`.tma` is out of scope** — `crates/core/src/asm/`, `lint/tma/`, `lsp/tma/` are a different tree.
- Conventional commits with scope. No AI/Claude attribution in any commit message or file.
- Code comments cite durable `docs/` pages by page plus a parenthetical lowercase keyword; open the page and confirm the keyword appears. Never `docs/superpowers/`, never `spec §N`.
- Gates every task: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p mtc-turing-machine`, and `git diff --stat -- crates/core crates/post-machine` printing nothing.

## What plan 7 leaves you, and the one sentence that matters most

`.tmc` has a lossless green tree. `crates/turing-machine/src/syntax/` holds `kinds.rs` (47 kinds — 28 significant tokens, 3 trivia, 15 nodes, `Root` last), `layout.rs`, `emit.rs`, and `mod.rs` whose module doc carries two rulings you must not re-derive: **a declaration retro-wraps its bound doc run**, so a documented node's extent starts at the run; and **WORLD sits between MACHINE/REUSE and their body items**, so walking a machine's children for its body items descends one level.

`crates/turing-machine/tests/tmc_property.rs` is the generator. **Its own module doc records its limit, independently confirmed by two reviews: it pins `text() == src` and nothing else.** A tree with correct text and wrong node kinds passes it today. **This plan is where that stops being acceptable** — Task 6's parity oracle is the first property that looks at structure rather than bytes, and it is the reason the generator was built four plans early.

## What plan 7 cost, and the four traps that produced it

Nine fix rounds, **every defect in test code, none in an implementation**. Do not spend them again:

1. **A test asserting over the wrong side of a mapping.** A map keyed on the input's discriminant makes a collision structurally unfindable. Assert over what the function *produces*.
2. **A test covering half its domain.** A fixture-driven check reaches only the shapes the fixture contains. Where a property is over a finite set, enumerate the set.
3. **A test comparing a compile-time constant against itself.** `assert_eq!(ARRAY_OF_28.len(), 28)` cannot fail.
4. **Nodes built correctly and their extents pinned by nothing.** Twice. `d.contains("STATE")` holds for a tree that puts STATE anywhere. Assert `text()`.

And three documentation sentences that survived review while being false — each time the code right at every site and only the prose wrong. **For every sentence you write in a doc comment, ask what would make it false, and check that.**

## File Structure

| File | Responsibility |
|---|---|
| `crates/turing-machine/src/syntax/views.rs` (new) | Typed views over the 15 node kinds, declared with `ast_node!`. Accessors walk direct children; no grammar, no storage. |
| `crates/turing-machine/src/syntax/extract.rs` (new) | `extract_program(root, source) -> Program`, rebuilding the existing AST from the tree. |
| `crates/turing-machine/src/syntax/mod.rs` (modified) | Declares and re-exports both. |
| `crates/turing-machine/tests/syntax_views.rs` (new) | View-level tests: what each accessor returns, and what it returns for the shapes that have no such child. |
| `crates/turing-machine/tests/tmc_property.rs` (modified, Task 6) | Gains the parity property over generated programs. |

---

### Task 1: the view types

**Files:**
- Create: `crates/turing-machine/src/syntax/views.rs`
- Modify: `crates/turing-machine/src/syntax/mod.rs`
- Test: `crates/turing-machine/tests/syntax_views.rs`

**Interfaces:**
- Consumes: `mtc_core::syntax::{ast_node, AstNode, SyntaxNode, SyntaxToken}`; `crate::syntax::kinds::TmcKind`.
- Produces: one view type per node kind — `RootView`, `UseView`, `UsePathView`, `AlphabetView`, `ReuseView`, `MachineView`, `NamespaceView`, `WorldView`, `TapeView`, `StateView`, `RuleView`, `GraftView`, `BindView`, `DocRunView`, `AttrView` — each with the `AstNode` contract, plus `TopView`, an enum over the kinds that can appear at file or namespace level.

- [ ] **Step 1: Write the failing test**

Create `crates/turing-machine/tests/syntax_views.rs`. The point of this first test is the contract, not the accessors: a view must accept exactly its own kind and refuse every other, because everything later assumes that.

```rust
//! Typed views over the `.tmc` green tree. These tests pin the view
//! CONTRACT — which node a view accepts and which it refuses — and each
//! accessor's answer for the shapes that do and do not have the child it
//! names. Extraction parity is a separate file; a view that returns the
//! wrong child still round-trips, so losslessness cannot catch it.

use mtc_core::syntax::{AstNode, SyntaxNode};
use mtc_turing_machine::parser::parse_green;
use mtc_turing_machine::syntax::{AlphabetView, MachineView, RootView, TmcKind};

fn tree(src: &str) -> SyntaxNode {
    SyntaxNode::new_root(parse_green(src).expect("parses"))
}

fn first_of(root: &SyntaxNode, kind: TmcKind) -> SyntaxNode {
    fn go(n: &SyntaxNode, k: TmcKind, out: &mut Option<SyntaxNode>) {
        for c in n.children() {
            if out.is_none() && c.kind() == k.into() {
                *out = Some(c.clone());
            }
            go(&c, k, out);
        }
    }
    let mut out = None;
    go(root, kind, &mut out);
    out.expect("node present")
}

/// A view casts from its own kind and refuses every other. This is the
/// whole contract the rest of the layer rests on: an accessor that
/// silently accepted a foreign node would return plausible nonsense.
#[test]
fn a_view_accepts_its_own_kind_and_refuses_others() {
    let root = tree("alphabet ab { '_' }\n\nmachine {\n  tape main: ab;\n}\n");
    let alphabet = first_of(&root, TmcKind::Alphabet);
    let machine = first_of(&root, TmcKind::Machine);

    assert!(AlphabetView::cast(alphabet.clone()).is_some());
    assert!(MachineView::cast(machine.clone()).is_some());
    assert!(RootView::cast(root.clone()).is_some());

    assert!(AlphabetView::cast(machine).is_none(), "AlphabetView took a MACHINE");
    assert!(MachineView::cast(alphabet).is_none(), "MachineView took an ALPHABET");
    assert!(RootView::cast(first_of(&root, TmcKind::Tape)).is_none());
}

/// Every view kind casts from the node it names. Enumerated rather than
/// sampled, and the enumeration is itself checked against the kind
/// space: node kinds occupy the contiguous discriminant run `32..=46`,
/// spelled here as literals — `TmcKind::Root as u16` would be the table
/// comparing itself to itself. A node kind inserted anywhere but the
/// very end of that run renumbers every kind after it, so a table left
/// stale for the new kind produces a set that diverges from the run.
/// What this catches: a listed view that stops casting, and a table
/// left stale across a mid-run insertion. What it does NOT catch: a
/// node kind appended after `Root` with no view and no entry here,
/// since nothing already listed moves. Same blind spot, by the same
/// argument, as the significant-token census in `syntax::kinds`.
#[test]
fn every_node_kind_has_a_view_that_casts_from_it() {
    // Verified against the real parser before this plan shipped: it parses,
    // and it yields all fifteen node kinds with none missing. Three things
    // it must keep — `use` accepts NO doc run (`DanglingDocRun` otherwise;
    // only export/alphabet/routine/graph/machine/namespace do), a signature
    // parameter writes its keyword FIRST (`tape t: ab`, `state done`), and
    // a namespace must be present or NAMESPACE has no instance to cast from.
    let src = "use a::b;\n\n? doc\n! [deprecated] old\nalphabet ab { '_', 'a' }\n\n\
               namespace n {\n  alphabet inner { '_' }\n}\n\n\
               graph g(tape t: ab, state done) {\n\
               \x20 entry state gs { [*] -> done; }\n}\n\n\
               routine r(tape t: ab) {\n\
               \x20 entry graft g(t = t, done = return);\n}\n\n\
               machine {\n  tape main: ab;\n  bind r() as hh;\n\
               \x20 entry state s {\n    ['a'] -> write ['_'] move [>] goto s;\n\
               \x20   [*] -> stop;\n  }\n}\n";
    let root = tree(src);
    // Each entry: the kind, and a closure proving a view casts from it.
    let checks: [(TmcKind, fn(SyntaxNode) -> bool); 15] = [
        (TmcKind::Root, |n| RootView::cast(n).is_some()),
        (TmcKind::Use, |n| mtc_turing_machine::syntax::UseView::cast(n).is_some()),
        (TmcKind::UsePath, |n| mtc_turing_machine::syntax::UsePathView::cast(n).is_some()),
        (TmcKind::Alphabet, |n| AlphabetView::cast(n).is_some()),
        (TmcKind::Reuse, |n| mtc_turing_machine::syntax::ReuseView::cast(n).is_some()),
        (TmcKind::Machine, |n| MachineView::cast(n).is_some()),
        (TmcKind::Namespace, |n| mtc_turing_machine::syntax::NamespaceView::cast(n).is_some()),
        (TmcKind::World, |n| mtc_turing_machine::syntax::WorldView::cast(n).is_some()),
        (TmcKind::Tape, |n| mtc_turing_machine::syntax::TapeView::cast(n).is_some()),
        (TmcKind::State, |n| mtc_turing_machine::syntax::StateView::cast(n).is_some()),
        (TmcKind::Rule, |n| mtc_turing_machine::syntax::RuleView::cast(n).is_some()),
        (TmcKind::Graft, |n| mtc_turing_machine::syntax::GraftView::cast(n).is_some()),
        (TmcKind::Bind, |n| mtc_turing_machine::syntax::BindView::cast(n).is_some()),
        (TmcKind::DocRun, |n| mtc_turing_machine::syntax::DocRunView::cast(n).is_some()),
        (TmcKind::Attr, |n| mtc_turing_machine::syntax::AttrView::cast(n).is_some()),
    ];
    // The table IS the node half of the kind space, not a sample of it.
    let mut listed: Vec<u16> = checks.iter().map(|(k, _)| *k as u16).collect();
    listed.sort_unstable();
    let expected: Vec<u16> = (32..=46).collect();
    assert_eq!(
        listed, expected,
        "the view table is not the contiguous node run 32..=46"
    );
    for (kind, casts) in checks {
        let node = if kind == TmcKind::Root {
            root.clone()
        } else {
            first_of(&root, kind)
        };
        assert!(casts(node), "no view casts from {kind:?}");
    }
}
```

**That fixture was validated before this plan shipped**, and three things about it are not guessable from the grammar's shape:

- **`use` accepts no doc run.** A `? doc` before it is a `DanglingDocRun` error; only `export`, `alphabet`, `routine`, `graph`, `machine` and `namespace` accept one.
- **A signature parameter writes its keyword first** — `tape t: ab`, `state done` — not `t: ab` or `done: state`.
- **A namespace must actually appear**, or `NAMESPACE` has no instance for the enumerated test to cast from.

An earlier draft of this plan got all three wrong. If you change the fixture, re-run it and re-count the kinds rather than assuming.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-turing-machine --test syntax_views`
Expected: FAIL to compile — no view type exists.

- [ ] **Step 3: Write the implementation**

Model it on `crates/post-machine/src/syntax/views.rs`. Declare each view with the core macro, one line each:

```rust
ast_node!(pub struct RootView: TmcKind::Root.into());
```

`TopView` is an enum over what can appear at file or namespace level — `Use`, `Alphabet`, `Reuse`, `Machine`, `Namespace` — with a `cast` that dispatches on kind. The sibling's `TopView` is the template.

No accessors yet; Tasks 2-4 add them. This task is the type layer and its cast contract.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-turing-machine --test syntax_views`
Expected: PASS, 2 tests.

Run: `cargo test -p mtc-turing-machine`
Expected: all green — nothing consumes views yet.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/syntax crates/turing-machine/tests/syntax_views.rs
git commit -m "feat(turing-machine): typed views over the .tmc green tree"
```

---

### Task 2: container accessors — file, namespace, use, alphabet

**Files:**
- Modify: `crates/turing-machine/src/syntax/views.rs`, `crates/turing-machine/tests/syntax_views.rs`

**Interfaces:**
- Consumes: Task 1's view types.
- Produces: `RootView::items()`, `NamespaceView::{name_token, name, items}`, `UseView::paths()`, `UsePathView::{segments, alias_token}`, `AlphabetView::{name_token, exported, glyph_tokens, doc_run}`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/turing-machine/tests/syntax_views.rs`:

```rust
    // -- container accessors ------------------------------------------------

/// `items()` yields the top-level declarations in source order, and
/// nothing else — not the trivia between them, not the doc runs folded
/// into them.
#[test]
fn root_items_are_the_declarations_in_source_order() {
    let root = tree("use a::b;\n\nalphabet ab { '_' }\n\nmachine {\n  tape main: ab;\n}\n");
    let view = RootView::cast(root).expect("root");
    let kinds: Vec<TmcKind> = view.items().map(|i| i.kind()).collect();
    assert_eq!(kinds, vec![TmcKind::Use, TmcKind::Alphabet, TmcKind::Machine]);
}

/// A namespace's own items, not the file's.
#[test]
fn namespace_items_are_scoped_to_it() {
    let root = tree("alphabet outer { '_' }\n\nnamespace n {\n  alphabet inner { '_' }\n}\n");
    let ns = NamespaceView::cast(first_of(&root, TmcKind::Namespace)).expect("ns");
    assert_eq!(ns.name(), "n");
    let names: Vec<String> = ns
        .items()
        .filter_map(|i| AlphabetView::cast(i.node().clone()))
        .map(|a| a.name_token().text().to_string())
        .collect();
    assert_eq!(names, vec!["inner"], "the outer alphabet is not this namespace's");
}

/// A path's segments, and its alias when written.
#[test]
fn use_paths_expose_segments_and_alias() {
    let root = tree("use a::b::c, d::e as f;\n");
    let u = UseView::cast(first_of(&root, TmcKind::Use)).expect("use");
    let paths: Vec<UsePathView> = u.paths().collect();
    assert_eq!(paths.len(), 2);

    let segs: Vec<String> = paths[0].segments().iter().map(|t| t.text().to_string()).collect();
    assert_eq!(segs, vec!["a", "b", "c"]);
    assert!(paths[0].alias_token().is_none(), "no alias was written");

    assert_eq!(
        paths[1].alias_token().map(|t| t.text().to_string()),
        Some("f".to_string())
    );
}

/// `exported` reads the `export` keyword's presence, and `doc_run` finds
/// the run the declaration retro-wraps.
#[test]
fn alphabet_exposes_export_glyphs_and_its_doc_run() {
    let root = tree("? doc\nexport alphabet ab { '_', 'a' }\n");
    let a = AlphabetView::cast(first_of(&root, TmcKind::Alphabet)).expect("alphabet");
    assert!(a.exported());
    assert_eq!(a.name_token().text(), "ab");
    let glyphs: Vec<String> = a.glyph_tokens().iter().map(|t| t.text().to_string()).collect();
    assert_eq!(glyphs, vec!["'_'", "'a'"]);
    assert!(a.doc_run().is_some(), "the run is the alphabet's own first child");

    let plain = tree("alphabet ab { '_' }\n");
    let p = AlphabetView::cast(first_of(&plain, TmcKind::Alphabet)).expect("alphabet");
    assert!(!p.exported());
    assert!(p.doc_run().is_none());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test syntax_views`
Expected: FAIL to compile — the accessors do not exist.

- [ ] **Step 3: Write the implementation**

Accessors walk direct children of the view's own node. Read the sibling's for the idiom: `child::<T>(self.node())` for one, `children::<T>(self.node())` for many, `token(self.node(), kind)` for a token.

Two `.tmc` specifics:

- **`glyph_tokens` must not descend.** An alphabet's glyphs are its own direct `GLYPH` tokens; a nested structure is not possible here, but writing it as a descendant walk would silently start picking up glyphs from elsewhere if the grammar ever nests. Walk direct children.
- **`exported` is the presence of an `export` keyword token before the `alphabet` keyword.** It is an `IDENT` at the token level: this lexer has NO keyword token kind at all, so every word in the language — reserved or not — arrives as an ordinary identifier, and the parser is the one place that refuses reserved words where a name is expected. Match on the text, and say that in a comment. Do NOT call `export` a *contextual* keyword: it is one of the 27 fully-reserved words in `lexer::RESERVED`, and the language's only contextual word is the `deprecated` attribute name.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test syntax_views`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src crates/turing-machine/tests/syntax_views.rs
git commit -m "feat(turing-machine): container accessors on the .tmc views"
```

---

### Task 3: world accessors — machine, reuse, signatures, tapes

**Files:**
- Modify: `crates/turing-machine/src/syntax/views.rs`, `crates/turing-machine/tests/syntax_views.rs`

**Interfaces:**
- Consumes: Tasks 1-2.
- Produces: `MachineView::{world, doc_run}`, `ReuseView::{kind, name_token, exported, signature, world, doc_run}`, `WorldView::{tapes, states, grafts, binds}`, `TapeView::{name_token, alphabet_token, volatile}`, and a `ReuseKind` enum distinguishing `routine` from `graph`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/turing-machine/tests/syntax_views.rs`:

```rust
    // -- world accessors ----------------------------------------------------

/// WORLD sits between a machine and its body items, so `world()` is the
/// step a caller must take before reaching tapes or states. This test
/// exists because forgetting it yields an empty body rather than an
/// error.
#[test]
fn a_machines_body_is_reached_through_its_world() {
    let root = tree("machine {\n  tape main: ab;\n  tape work: ab;\n}\n");
    let m = MachineView::cast(first_of(&root, TmcKind::Machine)).expect("machine");
    let w = m.world().expect("every machine has a world");
    let names: Vec<String> = w
        .tapes()
        .map(|t| t.name_token().text().to_string())
        .collect();
    assert_eq!(names, vec!["main", "work"]);
}

/// `volatile` is a modifier on the tape, not a separate declaration.
#[test]
fn tapes_expose_their_alphabet_and_volatility() {
    let root = tree("machine {\n  tape main: ab;\n  volatile tape scratch: ab;\n}\n");
    let w = MachineView::cast(first_of(&root, TmcKind::Machine))
        .expect("machine")
        .world()
        .expect("world");
    let tapes: Vec<TapeView> = w.tapes().collect();
    assert_eq!(tapes.len(), 2);
    assert_eq!(tapes[0].alphabet_token().text(), "ab");
    assert!(!tapes[0].volatile());
    assert!(tapes[1].volatile(), "the modifier belongs to the second tape");
}

/// A routine and a graph are both REUSE nodes; the view distinguishes
/// them, because everything downstream treats them differently.
#[test]
fn reuse_distinguishes_a_routine_from_a_graph() {
    let root = tree(
        "routine r(tape t: ab) {\n  entry state s { [*] -> stop; }\n}\n\n\
         graph g(tape t: ab, state done) {\n  entry state s { [*] -> done; }\n}\n",
    );
    let reuses: Vec<ReuseView> = RootView::cast(root)
        .expect("root")
        .items()
        .filter_map(|i| ReuseView::cast(i.node().clone()))
        .collect();
    assert_eq!(reuses.len(), 2);
    assert_eq!(reuses[0].kind(), ReuseKind::Routine);
    assert_eq!(reuses[1].kind(), ReuseKind::Graph);
    assert_eq!(reuses[0].name_token().text(), "r");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test syntax_views`
Expected: FAIL to compile.

- [ ] **Step 3: Write the implementation**

`ReuseKind` is decided by the leading keyword — `routine` or `graph` — which arrives as an `IDENT` token, same as `export`, because this lexer has no keyword token kind. Both are fully reserved (`lexer::RESERVED`); neither is contextual. Match on the text. `world()` returns `Option<WorldView>` rather than panicking: a view's job is to answer what the tree holds, and reporting absence is an answer.

**Do not add a convenience that skips WORLD.** A `MachineView::tapes()` forwarding through the world would hide the very structure the module doc tells later plans to expect, and the first thing a formatter needs is the real shape.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test syntax_views`
Expected: PASS, 9 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src crates/turing-machine/tests/syntax_views.rs
git commit -m "feat(turing-machine): world and signature accessors on the .tmc views"
```

---

### Task 3b: derive the node-kind space from what extraction must rebuild

**Why this task exists.** Plan 7 chose the `.tmc` node kinds from "the
grammar's containers" — `kinds.rs` says so in its own module doc — and
nobody checked that choice against the one consumer whose requirements
are non-negotiable: task 6's oracle holds `extract_program(tree)`
**struct-equal** to `lower_cst(parse_cst(tokens))`. Task 3's review found
the first place the two disagree.

**The rationale first written here was wrong, and the correction is the
useful part.** It argued that a signature carrying a `writes { … }` clause
puts commas at brace depth, so extraction would have to reimplement
`Parser::sig_param()` to split the run. Extraction does no such thing: it
reparses a node's token EXTENT through the parser's own production, as the
sibling does with `reparse_item`/`reparse_doc_items` — which task 5's own
Step 1 already instructs, in this same plan, before task 3b was written.
`ReuseView::signature()` hands `Parser::signature()` exactly its input, so
nothing needs splitting and the oracle needs no new kind at all.

What survives is a stronger reason, and it belongs to the VIEW layer
rather than the oracle: a boundary must be a node when finding it means
re-encoding a decision the parser already makes. Discovering the rest at
task 5 would mean reopening the kind space mid-extraction; discovering it
here costs one task.

**Files:**
- Modify: `crates/turing-machine/src/syntax/kinds.rs`,
  `crates/turing-machine/src/syntax/views.rs`,
  `crates/turing-machine/src/parser.rs`,
  `crates/turing-machine/tests/syntax_views.rs`,
  `crates/turing-machine/tests/tmc_property.rs`

**Interfaces:**
- Consumes: tasks 1-3's views and the 15-kind space.
- Produces: whatever node kinds the derivation demands, their views, and a
  census literal that matches.

- [ ] **Step 1: Write the derivation table before touching any code**

Walk every field of `parser::Program` reachable from a REUSE or MACHINE
subtree — `Routine`/`Graph`/`Machine`, then `Signature`, `SigParam`,
`ContractClause`, `State`, `Rule`, `Pattern`, `PatternCell`, `WriteVec`,
`MoveVec`, `Transition`, `Graft`, `Bind`, `BindingArg`, `QualName`,
`Doc` — and for each ask ONE question:

> Can `extract_program` rebuild this field from the current green tree
> using only direct-child walks and splitting on punctuation that is
> unambiguous at depth zero — WITHOUT reimplementing a parser rule?

Three verdicts, and the middle one is the whole point:

- **YES, already** — the tree brackets it, or the flat token run splits
  unambiguously. `Pattern`'s cells are an example to check: commas inside
  `[ … ]` are all at depth zero, so splitting is safe and no new kind is
  needed. Record WHY, not just "yes".
- **NO — keyword-decided** — the construct's extent or its presence is
  decided by a reserved word, so locating it re-encodes an optionality
  decision the parser already makes. `Rule.write` is the clearest case:
  the bracket group is a write vector only because the word `write`
  precedes it, and the identical shape is a pattern or a move vector
  elsewhere. These demand a node kind.
- **NO — containment** — the field is unambiguous in isolation but its
  OWNER is not, so a consumer must count back to a keyword to decide
  which parent a child belongs to. `Signature.params` is this shape once
  the contract clauses are nodes. These demand a node kind too.

Do NOT use "punctuation recurring at depth" as a discriminator. Tracking
brace depth is mechanical, and that test would justify a node kind for
every comma-separated list in the language.

Write the table into
`.superpowers/sdd/2026-08-24-c2-plan8-tmc-views/task-3b-derivation.md`
before writing code, one row per field: field → verdict → the reason →
the node kind it demands, if any. **A row saying "YES" must name the
punctuation that makes it unambiguous or the node that already brackets
it.** Bare "yes" rows are how plan 7 produced a kind space nobody checked.

Do not guess at depth: for every construct you judge, write the smallest
`.tmc` program exercising it, run it through `./target/release/tmt fmt
--check`, and dump its green tree with `mtc_core::syntax::debug_dump`
(`kinds.rs` supplies the `kind_name` callback). Judge the dump, not the
grammar as you remember it. Probes live in the session scratchpad, never
inside the repo.

- [ ] **Step 2: Add the kinds the table demands, and nothing else**

Kinds go at the END of the node run in `TmcKind`, after `Root`. Appending
rather than inserting is deliberate: it keeps every existing discriminant
where it is, so no committed test that spells a literal has to move for a
reason unrelated to its subject.

Bracket each new kind at the parser's EXISTING call site — the function
that already parses that construct — with the same `g_start`/`g_finish`
pair the other kinds use. The parser owns every grammar decision; the
sink only observes. Do not add a parsing branch, do not move one, and do
not change what the parser accepts. If bracketing a construct seems to
require changing the walk, stop and report it: that is a finding about
the grammar, not a step to improvise through.

- [ ] **Step 3: Update the three mirrors that the kind space feeds**

Each of these is a drift guard that MUST fail before you fix it — run it,
watch it fail, then update it. A guard you updated without seeing it fail
is a guard you have not tested.

1. `crates/turing-machine/tests/syntax_views.rs`, the census in
   `every_node_kind_has_a_view_that_casts_from_it`: the literal run
   `(32..=46)` becomes the new run, and the `checks` table grows a row
   per new kind. Every new kind needs a view declared with `ast_node!`
   and a cast test, exactly as the 15 existing ones have.
2. `kind_name` in `kinds.rs`: it answers for every kind, and its own test
   enumerates them.
3. `crates/turing-machine/tests/tmc_property.rs`: plan 7's generator
   holds `text() == src` over generated programs. New brackets must not
   move a single byte of text — that is the whole claim of a sink that
   only observes. Run it.

- [ ] **Step 4: Prove the new brackets are real**

For each new node kind, one test that a flat-token consumer could not
pass: cast the new view from a real parsed program and assert its
contents by VALUE. Then mutate the parser to drop that kind's
`g_start`/`g_finish` pair and confirm the test fails — a bracket nothing
observes is not a bracket.

The signature case is the one that motivated this task, so it gets the
adversarial fixture explicitly: a two-parameter signature whose first
parameter carries `writes { … }` with a comma inside the braces. A
consumer splitting the old flat run on commas would find three
separators where there is one. Assert the parameter count and each
parameter's name.

- [ ] **Step 5: Run the whole suite and commit**

`cargo test -p mtc-turing-machine`, `cargo fmt --check`, `cargo clippy
--workspace --all-targets -- -D warnings`. `crates/core` and
`crates/post-machine` stay at zero diff.

```bash
git add crates/turing-machine
git commit -m "feat(turing-machine): bracket the .tmc constructs extraction must rebuild"
```

---


### Task 4: body accessors — states, rules, grafts, binds, doc runs

**Files:**
- Modify: `crates/turing-machine/src/syntax/views.rs`, `crates/turing-machine/tests/syntax_views.rs`

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: `StateView::{name_token, is_entry, rules, doc_run}`, `RuleView::{pattern_tokens, write_vec, move_vec, transition}`, `GraftView::{target_token, is_entry, as_name, bindings}`, `BindView::{target_token, as_name, bindings}`, `DocRunView::{doc_lines, attention_lines, attrs}`, `AttrView::line_token`.

**Task 3b changed this task's shape — read this before Step 1.** Three of the
constructs this task was going to expose as flat token runs are now nodes, and
one accessor it named cannot exist at all.

- `write_tokens`/`move_tokens`/`transition_tokens` become typed child-node
  accessors: `write_vec() -> Option<WriteVecView>`, `move_vec() -> Option<MoveVecView>`,
  `transition() -> Option<TransitionView>`. `pattern_tokens` STAYS token-based —
  a pattern is positionally first and mandatory, so the derivation gave it no
  node kind.
- **`transition()` returning `None` IS `Transition::Stay`**, not an error and not
  a missing feature. An omitted transition means "stay in the current state", and
  the absence of the node is the only thing in the tree that carries it. Say so at
  the definition site; a later reader will otherwise "fix" it into a panic.
- **`AttrView::name_token` cannot exist.** The lexer folds a whole `! [ident] …`
  line into ONE `AttentionLine` token, so `[deprecated]` is never its own token
  and ATTR wraps exactly that single token. Expose `line_token()` instead, and
  get the attribute NAME by calling the parser's own `parse_attr` over the
  payload — widen it to `pub(crate)` if needed. Do NOT do string surgery in the
  view: that is the duplication this whole layer exists to avoid.
- Some accessors already landed in task 3b, because its by-value assertions
  needed them: `ReuseView::params`, `SigParamView::{volatile, kind, name_token,
  alphabet_token, contract_clauses}`, `ContractClauseView::keyword_token`,
  `BindingArgView::{name_token, sym_map}`. Do not re-add them; do check they are
  tested by value.

- [ ] **Step 1: Write the failing tests**

Add to `crates/turing-machine/tests/syntax_views.rs`:

```rust
    // -- body accessors -----------------------------------------------------

/// `is_entry` reads the `entry` marker; a world has exactly one.
#[test]
fn states_expose_entry_and_their_rules() {
    let root = tree(
        "machine {\n  tape main: ab;\n\
         \x20 entry state s {\n    ['a'] -> write ['_'] move [>] goto t;\n\
         \x20   [*] -> stop;\n  }\n\
         \x20 state t { [*] -> halt; }\n}\n",
    );
    let w = MachineView::cast(first_of(&root, TmcKind::Machine))
        .expect("machine")
        .world()
        .expect("world");
    let states: Vec<StateView> = w.states().collect();
    assert_eq!(states.len(), 2);
    assert!(states[0].is_entry());
    assert!(!states[1].is_entry());
    assert_eq!(states[0].name_token().text(), "s");
    assert_eq!(states[0].rules().count(), 2);
    assert_eq!(states[1].rules().count(), 1);
}

/// A doc run's three direct-child shapes, which are NOT interchangeable:
/// `?` lines are DOC_LINE tokens; a `! [ident] …` line is folded by the
/// lexer into one ATTENTION_LINE token that ATTR then wraps; a bare-prose
/// `!` line carries no `[ident]`, so no ATTR is emitted and its token
/// stays a direct child. `attention_lines` means the bare ones — an
/// implementation returning every ATTENTION_LINE, tagged ones included,
/// must fail this test, which is why the fixture carries one of each.
#[test]
fn doc_runs_split_their_lines_and_expose_attributes() {
    let root = tree("? one\n? two\n! plain prose\n! [deprecated] use the other\nalphabet ab { '_' }\n");
    let run = DocRunView::cast(first_of(&root, TmcKind::DocRun)).expect("run");
    assert_eq!(run.doc_lines().len(), 2);
    assert_eq!(
        run.attention_lines().len(),
        1,
        "only the bare-prose line — the tagged one lives inside an ATTR"
    );
    let attrs: Vec<AttrView> = run.attrs().collect();
    assert_eq!(attrs.len(), 1);
    assert!(
        attrs[0].line_token().text().contains("[deprecated]"),
        "ATTR wraps the whole attention line, payload and all"
    );
}


/// A graft names its target and its instance; `as_name` is absent only
/// on an entry graft, which may omit it.
#[test]
fn grafts_expose_target_and_instance_name() {
    let root = tree(
        "graph g(tape t: ab, state done) {\n  entry state s { [*] -> done; }\n}\n\n\
         machine {\n  tape main: ab;\n\
         \x20 entry graft g(t = main, done = stop) as gg;\n}\n",
    );
    let g = GraftView::cast(first_of(&root, TmcKind::Graft)).expect("graft");
    assert!(g.is_entry());
    assert_eq!(g.target_token().text(), "g");
    assert_eq!(g.as_name().map(|t| t.text().to_string()), Some("gg".to_string()));
    assert_eq!(g.bindings().count(), 2);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test syntax_views`
Expected: FAIL to compile.

- [ ] **Step 3: Write the implementation**

`RuleView`'s accessors return token runs rather than parsed values: a rule's pattern, write vector, move vector and transition are token sequences inside the RULE node, and turning them into values is extraction's job in Task 5, not the view layer's. **A view that parsed would duplicate grammar the parser already owns** — the same reason the sibling's `extract_statement` goes through the parser's own production rather than re-deriving it from views.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test syntax_views`
Expected: PASS, 12 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src crates/turing-machine/tests/syntax_views.rs
git commit -m "feat(turing-machine): body accessors on the .tmc views"
```

---

### Task 5: extraction

**Files:**
- Create: `crates/turing-machine/src/syntax/extract.rs`
- Modify: `crates/turing-machine/src/syntax/mod.rs`

**Interfaces:**
- Consumes: Tasks 1-4; `crate::parser::Program` and the AST types it holds.
- Produces: `pub fn extract_program(root: &SyntaxNode, source: &str) -> Program`.

- [ ] **Step 1: Read the target before writing anything**

`Program` is `{ imports, alphabets, routines, graphs, machine }`. Read `lower_cst` in `crates/turing-machine/src/parser.rs` — that is the function whose output you must reproduce exactly, and reading it is cheaper than rediscovering its decisions one oracle failure at a time. Note in particular where it *normalises*: any place it computes something rather than copying it is a place extraction must compute the same way.

**The sibling's hardest-won lesson applies here.** `crates/post-machine/src/syntax/extract.rs` does not re-derive the grammar from views: for a statement's internals it re-runs the parser's own production over the node's tokens. Where a `.tmc` construct's internals are equally intricate — a rule's transition, a binding list, a symbol map — do the same rather than writing a second parser. Say in your report which constructs you routed through the parser and which you read directly from views.

- [ ] **Step 2: Write the failing test**

Put this in `extract.rs`'s own test module — it is the smallest honest check, and Task 6 is the real one:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};
    use crate::parser::{lower_cst, parse_cst};
    use mtc_core::syntax::SyntaxNode;

    #[track_caller]
    fn agrees(src: &str) {
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let cst = parse_cst(&tokens).expect("parses");
        let expected = lower_cst(&cst);
        let green = crate::parser::parse_green_from_tokens(src, &tokens).expect("parses");
        let actual = extract_program(&SyntaxNode::new_root(green), src);
        assert_eq!(actual, expected, "extraction diverged for:\n{src}");
    }

    #[test]
    fn extraction_agrees_with_the_cst_on_small_programs() {
        agrees("use a::b;\n");
        agrees("alphabet ab { '_', 'a' }\n");
        agrees("machine {\n  tape main: ab;\n  entry state s { [*] -> stop; }\n}\n");
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p mtc-turing-machine --lib syntax::extract`
Expected: FAIL to compile — `extract_program` does not exist.

- [ ] **Step 4: Write the implementation**

Build `Program` from `RootView::items()`, dispatching on `TopView`. Namespaces contribute their items with the namespace's name prefixed the way `lower_cst` does it — **read that code rather than assuming the separator or the order.**

`Program` must derive or already implement `PartialEq` for the oracle to work. If it does not, adding the derive is the one production change this task may make; say so in your report.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p mtc-turing-machine --lib syntax::extract`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src
git commit -m "feat(turing-machine): extract a Program from the .tmc green tree"
```

---

### Task 6: the parity oracle — the plan's actual deliverable

**Files:**
- Modify: `crates/turing-machine/tests/tmc_property.rs`
- Create: `crates/turing-machine/tests/syntax_parity.rs`

**Interfaces:**
- Consumes: Task 5's `extract_program`; plan 7's generator.
- Produces: parity over the shipped corpus and over generated programs.

**Why this task is the plan.** Everything before it is machinery. An `extract_program` that agrees with the CST on nine corpus files and three hand-written fixtures has been tested on the shapes someone thought of. The generator exists precisely because the sibling language shipped two Critical bugs living in shapes nobody thought of, and **this is its first structural consumer** — until now it has only ever asserted `text() == src`.

- [ ] **Step 1: Write the corpus parity test**

Create `crates/turing-machine/tests/syntax_parity.rs`:

```rust
//! Extraction parity: the `Program` built from the green tree equals the
//! one built from the CST, struct for struct, over every `.tmc` the repo
//! ships. The green tree's own lossless law cannot catch an extraction
//! bug — a tree can round-trip perfectly and still be read wrongly — so
//! this is the check that the two paths agree about MEANING rather than
//! about bytes.

use mtc_core::syntax::SyntaxNode;
use mtc_turing_machine::lexer::{LexMode, lex_with};
use mtc_turing_machine::parser::{lower_cst, parse_cst, parse_green_from_tokens};
use mtc_turing_machine::syntax::extract_program;

#[test]
fn the_shipped_corpus_extracts_identically_on_both_paths() {
    let mut checked = 0;
    for dir in ["tests/golden", "src/stdlib", "../../docs/examples"] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("readable entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("tmc") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable source");
            let tokens = lex_with(&src, LexMode::WithComments).expect("lexes");
            let expected = lower_cst(&parse_cst(&tokens).expect("parses"));
            let green = parse_green_from_tokens(&src, &tokens).expect("parses");
            let actual = extract_program(&SyntaxNode::new_root(green), &src);
            assert_eq!(actual, expected, "{} extracts differently", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 9, "expected the whole .tmc corpus, saw {checked}");
}
```

- [ ] **Step 2: Add the generated-program parity property**

Add to `crates/turing-machine/tests/tmc_property.rs`, beside the lossless property:

```rust
proptest! {
    /// Extraction parity over generated programs. The lossless property
    /// above pins the tree's TEXT; this pins what the tree MEANS, and it
    /// is the first property in this file to look at structure at all.
    #[test]
    fn generated_programs_extract_identically_on_both_paths(
        seed in prop::collection::vec(any::<u8>(), 1..64)
    ) {
        let src = generate_program(&seed);
        let tokens = mtc_turing_machine::lexer::lex_with(
            &src, mtc_turing_machine::lexer::LexMode::WithComments,
        ).expect("lexes");
        let expected = mtc_turing_machine::parser::lower_cst(
            &mtc_turing_machine::parser::parse_cst(&tokens).expect("parses"),
        );
        let green = mtc_turing_machine::parser::parse_green_from_tokens(&src, &tokens)
            .expect("parses");
        let actual = mtc_turing_machine::syntax::extract_program(
            &mtc_core::syntax::SyntaxNode::new_root(green), &src,
        );
        prop_assert_eq!(actual, expected);
    }
}
```

- [ ] **Step 3: Run both, and expect the generated one to find something**

Run: `cargo test -p mtc-turing-machine --test syntax_parity --test tmc_property`

**A green first run on the generated property is a result worth doubting, not celebrating.** The generator reaches shapes the corpus does not — nested and reopened namespaces, comments in every positional slot, both entry forms, arithmetic folds, omitted transitions. If extraction handles all of that first time, confirm the property can fail before believing it: **break extraction deliberately — drop the alias from a `use` path, or ignore a tape's `volatile` modifier — and confirm the property goes red**, then restore.

Report which break you used and how many cases it took to fail.

- [ ] **Step 4: Run the whole suite**

Run: `cargo test -p mtc-turing-machine`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/tests
git commit -m "test(turing-machine): extraction parity over the corpus and generated programs"
```

---

### Task 7: documentation

**Files:**
- Modify: `CLAUDE.md`, `crates/turing-machine/src/syntax/mod.rs`

**Interfaces:**
- Consumes: Tasks 1-6. No code changes.

- [ ] **Step 1: Record what is true now, and only that**

`.tmc` has views and extraction, held to the CST by a differential oracle over the corpus and over generated programs. **Nothing in production consumes them** — `parse_cst` is still every consumer's path, and routing them across is plans 9-11.

Update `CLAUDE.md`'s `### The `.tmc` front end` paragraph to say exactly that. Keep it at standing state; do not narrate this plan.

- [ ] **Step 2: Record the oracle in the module doc**

`syntax/mod.rs` already carries the retro-wrap and WORLD rulings. Add one paragraph: extraction is held equal to `lower_cst` over both the corpus and generated programs, and that oracle is what a later plan must keep green when it routes a consumer across.

**Check every sentence you write.** Three documentation claims in plan 7 survived review while being false. For each sentence, ask what would make it false and verify that specific thing.

- [ ] **Step 3: Verify**

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md crates/turing-machine/src/syntax/mod.rs
git commit -m "docs: .tmc has views and extraction, proven against the CST"
```

---

## Exit criteria

- Every `.tmc` node kind has a view; each view accepts its own kind and refuses others, proven by an enumerated test rather than a sampled one.
- `extract_program` produces a `Program` struct-equal to `lower_cst(parse_cst(...))` over the shipped corpus and over generated programs.
- The parity property has been shown to fail under a deliberate extraction break.
- No production consumer reads views or extraction; `parse_cst` remains every consumer's path.
- `crates/core` and `crates/post-machine` have a zero-line diff for the whole plan.
