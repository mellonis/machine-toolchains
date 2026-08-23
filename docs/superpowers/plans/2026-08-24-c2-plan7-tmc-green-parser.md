# C2 Plan 7 — the `.tmc` green parser foundation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `.tmc` a lossless green syntax tree built by the existing parser, so the four consumers that follow (views, compiler front, language service, fmt) have something to migrate onto — and build the program generator the PM side did not have until it had already shipped two Critical bugs.

**Architecture:** The parser is not rewritten. Green emission is **woven into** the existing recursive-descent `Parser` through its own `bump()` hook, exactly as the `.pmc` side did: the parser stays the single owner of every grammar decision and the sink only mirrors token consumption and node boundaries, so the tree and the parser's errors cannot disagree. Token text and the trivia between tokens are reconstructed from the unchanged lexer's output plus the source, and their concatenation is the source byte for byte — the lossless law. Nothing consumes the tree yet; this plan ends with the tree, its oracle, and a generator.

**Tech Stack:** Rust (toolchain pinned by `rust-toolchain.toml`), `mtc-core`'s `syntax` framework (`TreeBuilder`, green/red model, `SyntaxKind`), `proptest` as a dev-dependency.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md` — read §4 (per-language adoption), §6 (oracles and gates) and §7 (sequencing) before Task 1. This plan is step 3's first half: "TM: same order".

## Global Constraints

- **`crates/core` gets no diff.** The syntax framework it provides is complete — PM's six plans added nothing to it after plan 1. A `crates/core` diff here is a design smell; raise it as a ruling instead of taking it.
- **The lexer is not modified.** `LexMode::WithComments` already exists on the TM lexer and already emits `TokenKind::Comment`. PM's plan 2 needed no lexer change either, and for the same reason: the layout pass reconstructs everything from the existing token stream plus the source text.
- **The parser's grammar decisions do not change.** Error text, error spans, and acceptance/rejection must be identical before and after. The sink observes; it never decides. Any place you feel tempted to change a production to make emission easier is a place to stop and report instead.
- **Nothing consumes the tree in this plan.** No compiler, lint, LSP or fmt change. Those are plans 8-11. Code that produces the tree and code that tests it is the whole scope.
- **`.tma` is out of scope.** The assembly CST in `crates/core/src/asm/` and everything under `lint/tma/` and `lsp/tma/` belongs to a different tree and the spec excludes it. If a search leads you there, you have left the task.
- **The `.tmc` CST survives this plan and every plan up to the arc's last one.** It is the differential oracle. Do not delete it, do not deprecate it, do not route anything away from it.
- PM only touches PM; this plan only touches `crates/turing-machine`. Do not edit `crates/post-machine`.
- Conventional commits with scope, e.g. `feat(turing-machine):`, `test(turing-machine):`.
- No AI/Claude attribution in any commit message or file.
- Code comments cite durable `docs/` pages by page plus a parenthetical lowercase keyword — `docs/core.md (syntax trees)`, `docs/tmt/language.md (rules)`. Open the page and confirm the keyword appears. Never cite `docs/superpowers/`, never write `spec §N` in a doc comment.
- Gates for every task: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p mtc-turing-machine`, and `git diff --stat -- crates/core crates/post-machine` printing nothing.

## What the PM side learned, stated once so this plan does not relearn it

Six plans built the `.pmc` green tree and its consumers. Two things from that are worth more than any individual step here:

1. **A differential oracle is only as strong as the inputs fed to it.** The `.pmc` migration compared two printers byte-for-byte on hand-written fixtures for six plans and eight mutation-armed reviews, and still shipped two Critical bugs — an unbounded comment duplication that corrupted files on format-on-save, and a wrong `label_break` present since the very first task. Both lived in shapes no fixture contained. The generator that would have caught them in minutes was added afterwards. **That is why Task 7 of this plan builds a `.tmc` generator now, four plans before anything needs it.**
2. **Mutation-armed review proves a test CAN fail; it cannot prove the implementation MATCHES the reference on shapes no fixture reaches.** Both disciplines are needed and neither substitutes for the other.

## The tree shape, and where the PM side's surprises were

Verified against the `.pmc` implementation, and expected to hold here because the framework and the weaving technique are the same:

- **Trivia flushes into the current node before a child opens**, so a node starts at its first significant token and trivia sits between a parent's children. That single fact is what later makes comment classification a local sibling query rather than a stored field.
- **A comment after a closing `}` is the next sibling token in the PARENT's stream**, not a child of the node it follows. It cost the `.pmc` side a review cycle to notice.
- **A node that binds a doc run retro-wraps it**, so its extent starts before its first keyword. On `.pmc` this was true of functions and false of namespaces, and the asymmetry produced a silent blank-line bug. `.tmc` has doc runs on more shapes; **Task 5 must establish, by dumping real trees, which `.tmc` nodes retro-wrap and which do not, and write the answer down.**

## File Structure

| File | Responsibility |
|---|---|
| `crates/turing-machine/src/syntax/mod.rs` (new) | Module root and the crate's public green surface: `TmcKind`, `kind_name`, `parse_green`. |
| `crates/turing-machine/src/syntax/kinds.rs` (new) | The `.tmc` kind space over core's opaque `SyntaxKind`: one kind per significant `TokenKind`, three trivia kinds, one per grammar container. |
| `crates/turing-machine/src/syntax/layout.rs` (new) | `SigLayout`: verbatim per-token text and the trivia between tokens, reconstructed from the unchanged lexer output plus the source. The lossless law starts here. |
| `crates/turing-machine/src/syntax/emit.rs` (new) | `GreenSink`: a `TreeBuilder` fed from the layout schedule. Mirrors token consumption and node boundaries; owns no grammar. |
| `crates/turing-machine/src/parser.rs` (modified) | The sink woven in at `bump()` and at each container production's boundaries. No grammar change. |
| `crates/turing-machine/tests/syntax_green.rs` (new) | Golden tree dumps derived BY HAND from the shape rules, plus the corpus-wide `text() == source` law. |
| `crates/turing-machine/tests/tmc_property.rs` (new) | The generator and its properties. Serves plans 8-11. |

---

### Task 1: the `.tmc` kind space

**Files:**
- Create: `crates/turing-machine/src/syntax/kinds.rs`, `crates/turing-machine/src/syntax/mod.rs`
- Modify: `crates/turing-machine/src/lib.rs` (add `pub mod syntax;`)

**Interfaces:**
- Consumes: `mtc_core::syntax::SyntaxKind`; `crate::lexer::TokenKind`.
- Produces: `pub enum TmcKind` (`#[repr(u16)]`, `Copy`), `impl From<TmcKind> for SyntaxKind`, `pub fn kind_name(k: SyntaxKind) -> &'static str`, and `pub(crate) fn token_kind(t: &TokenKind) -> TmcKind`.

- [ ] **Step 1: Write the failing test**

The point of this test is not that the mapping is right — it is that the mapping is **exhaustive by construction**, so adding a lexer token later cannot silently fall through. Put it in `kinds.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};

    /// Every significant token the lexer can produce maps to a kind, and
    /// distinct token kinds never collapse onto one syntax kind. `Eof`
    /// carries no kind — the tree holds trailing trivia instead of a
    /// zero-length sentinel — and `Comment` becomes trivia, not a
    /// significant kind.
    #[test]
    fn every_significant_token_kind_maps_to_a_distinct_kind() {
        let src = "? doc\n! attention\nalphabet ab { '_', 'a' }\n\
                   machine {\n  tape main: ab;\n  entry state s {\n\
                     ['a'] -> write ['_'] move [>] goto s;\n\
                     [*] -> stop;\n  }\n}\n";
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let mut seen = std::collections::BTreeMap::new();
        for t in &tokens {
            if matches!(t.kind, crate::lexer::TokenKind::Eof) {
                continue;
            }
            let k = token_kind(&t.kind);
            let prev = seen.insert(std::mem::discriminant(&t.kind), k);
            if let Some(p) = prev {
                assert_eq!(p, k, "one token kind mapped to two syntax kinds");
            }
        }
        assert!(seen.len() >= 12, "fixture exercised only {} kinds", seen.len());
    }

    /// `kind_name` answers for every kind the enum defines, so a tree
    /// dump can never print a bare number.
    #[test]
    fn kind_name_answers_for_every_kind() {
        for raw in 0u16..=(TmcKind::Root as u16) {
            let name = kind_name(SyntaxKind(raw));
            assert!(!name.is_empty(), "kind {raw} has no name");
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-turing-machine --lib syntax::kinds`
Expected: FAIL to compile — `TmcKind`, `token_kind` and `kind_name` do not exist.

- [ ] **Step 3: Write the implementation**

Model it on `crates/post-machine/src/syntax/kinds.rs`, which is the same file for the other language — read it first. The differences are only in the space:

- The TM lexer has **30 `TokenKind` variants**; `Eof` gets no kind and `Comment` becomes trivia, so **28 significant kinds**.
- Then **three trivia kinds**: `LineComment`, `BlockComment`, `Whitespace`.
- Then one node kind per grammar container. Derive the list from `crates/turing-machine/src/cst.rs`'s own types — `Use`, `UsePath`, `Alphabet`, `Reuse`, `Machine`, `Namespace`, `World`, `Tape`, `State`, `Rule`, `Graft`, `Bind`, `DocRun`, `Attr` — plus a `Root` for the file itself. **Order the enum tokens, then trivia, then nodes, and keep `Root` last**, because the `kind_name` test walks `0..=Root`.

`token_kind` must be a `match` on `&TokenKind` **with no wildcard arm**. That is the drift guard: adding a lexer token later fails the build here rather than silently becoming the wrong kind. Do not write `_ =>`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-turing-machine --lib syntax::kinds`
Expected: PASS, 2 tests.

Run: `cargo test -p mtc-turing-machine`
Expected: all green — nothing consumes this yet.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/syntax crates/turing-machine/src/lib.rs
git commit -m "feat(turing-machine): the .tmc syntax-kind space"
```

---

### Task 2: source layout — verbatim text and trivia

**Files:**
- Create: `crates/turing-machine/src/syntax/layout.rs`
- Modify: `crates/turing-machine/src/syntax/mod.rs`

**Interfaces:**
- Consumes: Task 1's `TmcKind`; `crate::lexer::{Token, TokenKind, lex_with, LexMode}`.
- Produces: `pub struct SigLayout { pub text: String, pub trivia_before: Vec<(TmcKind, String)> }` and `pub fn layout(source: &str, tokens: &[Token]) -> Vec<SigLayout>`, one entry per token including `Eof` (whose `text` is empty and whose `trivia_before` carries everything after the last significant token).

- [ ] **Step 1: Write the failing test**

The whole plan rests on this one property, so test it as a law, not as examples. In `layout.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};

    /// The concatenation of every piece — each token's verbatim text and
    /// the trivia before it — is the source, byte for byte. Everything
    /// downstream inherits its losslessness from this.
    #[track_caller]
    fn round_trips(src: &str) {
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let entries = layout(src, &tokens);
        let mut out = String::new();
        for e in &entries {
            for (_, t) in &e.trivia_before {
                out.push_str(t);
            }
            out.push_str(&e.text);
        }
        assert_eq!(out, src, "layout is not lossless");
    }

    #[test]
    fn the_pieces_concatenate_to_the_source() {
        round_trips("alphabet ab { '_', 'a' }\n");
        round_trips("machine {\n  tape main: ab;\n}\n");
        round_trips("");
        round_trips("\n\n\n");
    }

    #[test]
    fn comments_and_doc_runs_are_preserved_verbatim() {
        round_trips("// leading\nalphabet ab { '_' }\n");
        round_trips("alphabet ab { '_' } // trailing\n");
        round_trips("/* block\n   spanning */\nalphabet ab { '_' }\n");
        round_trips("? doc line\n! attention\nalphabet ab { '_' }\n");
    }

    #[test]
    fn glyph_and_number_spellings_survive() {
        round_trips("alphabet ab { '_', '\\'', '\\\\' }\n");
        round_trips("machine {\n  tape main: ab;\n  entry state s { [*] -> stop; }\n}\n");
    }

    #[test]
    fn the_whole_shipped_corpus_round_trips() {
        for dir in ["tests/golden", "src/stdlib"] {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for entry in entries {
                let path = entry.expect("entry").path();
                if path.extension().and_then(|e| e.to_str()) != Some("tmc") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).expect("readable");
                let tokens = lex_with(&src, LexMode::WithComments).expect("lexes");
                let out: String = layout(&src, &tokens)
                    .iter()
                    .flat_map(|e| {
                        e.trivia_before
                            .iter()
                            .map(|(_, t)| t.clone())
                            .chain(std::iter::once(e.text.clone()))
                    })
                    .collect();
                assert_eq!(out, src, "{} is not lossless", path.display());
            }
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-turing-machine --lib syntax::layout`
Expected: FAIL to compile — `SigLayout` and `layout` do not exist.

- [ ] **Step 3: Write the implementation**

Read `crates/post-machine/src/syntax/layout.rs` first and port its method: one pass over `source.char_indices()` tracking 1-based line and char column produces each token's start byte offset; each token's end is derived per kind; everything between two tokens is trivia, split into whitespace runs and comment tokens.

Three `.tmc` specifics the `.pmc` file does not have, and each is a place to get the end offset wrong:

- **`Glyph(String)` carries the DECODED content**, so its source length is not its payload length — `'\''` is four source characters and one decoded one. Derive the end by scanning the source from the start quote for the closing quote, honouring `\'` and `\\`, rather than by measuring the payload.
- **`Number(u32, String)` carries the digits as written**, leading zeros included. Use the spelling's length, never the value's.
- **Two-character operators** (`..`, `->`, `=>`, `::`) are lexed greedily and their one-character siblings (`.`, `-`, `=`, `:`) exist as separate kinds. Length follows the kind.

Assert, do not assume: everything between the end of one token and the start of the next must be whitespace or a comment. If it is not, the offsets are wrong and a `debug_assert!` naming the position is what will tell you.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-turing-machine --lib syntax::layout`
Expected: PASS, 4 tests, including the 9-file corpus.

Run: `cargo test -p mtc-turing-machine`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/syntax
git commit -m "feat(turing-machine): reconstruct .tmc source layout for the green tree"
```

---

### Task 3: the green sink

**Files:**
- Create: `crates/turing-machine/src/syntax/emit.rs`
- Modify: `crates/turing-machine/src/syntax/mod.rs`

**Interfaces:**
- Consumes: Tasks 1-2; `mtc_core::syntax::{TreeBuilder, Checkpoint, GreenNode}`.
- Produces: `pub struct GreenSink` with `new(entries: Vec<SigLayout>)`, `flush(&mut self, pos: usize)`, `token(&mut self, pos: usize, kind: TmcKind)`, `start(&mut self, kind: TmcKind)`, `finish(&mut self)`, `checkpoint(&self) -> Checkpoint`, `start_at(&mut self, cp: Checkpoint, kind: TmcKind)`, and `finish_tree(self, pos_after_last: usize) -> Rc<GreenNode>`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};
    use crate::syntax::kinds::TmcKind;
    use crate::syntax::layout::layout;
    use mtc_core::syntax::SyntaxNode;

    /// A sink driven by hand reproduces the source exactly. This is the
    /// lossless law one level up from `layout`: the builder must place
    /// every piece the schedule carries, in order, and lose none of it.
    #[test]
    fn a_hand_driven_sink_reproduces_the_source() {
        let src = "// lead\nalphabet ab { '_' }\n";
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let sig: Vec<usize> = (0..tokens.len()).collect();
        let mut sink = GreenSink::new(layout(src, &tokens));
        sink.start(TmcKind::Root);
        sink.start(TmcKind::Alphabet);
        for (i, _) in sig.iter().enumerate() {
            if matches!(tokens[i].kind, crate::lexer::TokenKind::Eof) {
                break;
            }
            sink.token(i, crate::syntax::kinds::token_kind(&tokens[i].kind));
        }
        sink.finish();
        let green = sink.finish_tree(tokens.len() - 1);
        let root = SyntaxNode::new_root(green);
        assert_eq!(root.text(), src, "the sink lost or reordered source");
    }

    /// A checkpoint started retroactively wraps tokens already emitted —
    /// the mechanism a doc run bound to a later declaration needs.
    #[test]
    fn a_retroactive_checkpoint_wraps_already_emitted_tokens() {
        let src = "? doc\nalphabet ab { '_' }\n";
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let mut sink = GreenSink::new(layout(src, &tokens));
        sink.start(TmcKind::Root);
        let cp = sink.checkpoint();
        sink.token(0, TmcKind::DocLine);
        sink.start_at(cp, TmcKind::DocRun);
        sink.finish();
        for i in 1..tokens.len() - 1 {
            sink.token(i, crate::syntax::kinds::token_kind(&tokens[i].kind));
        }
        let green = sink.finish_tree(tokens.len() - 1);
        let root = SyntaxNode::new_root(green);
        assert_eq!(root.text(), src);
        assert_eq!(
            root.children().next().map(|c| c.kind()),
            Some(TmcKind::DocRun.into()),
            "the checkpoint did not wrap the doc line"
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-turing-machine --lib syntax::emit`
Expected: FAIL to compile — `GreenSink` does not exist.

- [ ] **Step 3: Write the implementation**

Port `crates/post-machine/src/syntax/emit.rs` — it is 119 lines and the logic is language-independent. Its three load-bearing properties, which you must keep:

- `flush(pos)` is **idempotent** and asserts monotonic order, so a caller need not track whether some other helper already flushed that position.
- `token(pos, kind)` takes the text out of the schedule with `std::mem::take` and asserts it was not already empty — that is what catches a token emitted twice.
- `finish_tree(pos_after_last)` flushes the trailing trivia into the root before closing it, so text after the last significant token is not lost.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-turing-machine --lib syntax::emit`
Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/syntax
git commit -m "feat(turing-machine): a green sink for the .tmc parser"
```

---

### Task 4: weave the sink into the parser — the outer containers

**Files:**
- Modify: `crates/turing-machine/src/parser.rs`
- Modify: `crates/turing-machine/src/syntax/mod.rs`

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: `pub fn parse_green(source: &str) -> Result<Rc<GreenNode>, CompileError>` and `pub fn parse_green_from_tokens(source: &str, tokens: &[Token]) -> Result<Rc<GreenNode>, CompileError>`, covering `ROOT`, `USE`, `USE_PATH`, `ALPHABET`, `MACHINE` and `NAMESPACE`. Productions not yet wrapped still emit their tokens, so the tree is lossless from this task onward even where it is not yet structured.

- [ ] **Step 1: Write the failing test**

Create `crates/turing-machine/tests/syntax_green.rs`. **The expected dumps are derived BY HAND from the shape rules — never pasted from a run.** That is this repo's golden discipline and it is the only thing that makes the dump evidence rather than a photograph.

```rust
//! Green-parser goldens for `.tmc`: emission woven into the existing
//! parser for the outer container productions. Expected dumps are
//! derived by hand from the tree-shape rules — trivia flushes into the
//! current node before a child opens, so a node starts at its first
//! significant token — never pasted from a run.

use mtc_core::syntax::{SyntaxNode, debug_dump};
use mtc_turing_machine::parser::parse_green;
use mtc_turing_machine::syntax::kind_name;

fn dump(source: &str) -> String {
    let tree = parse_green(source).expect("parses");
    let root = SyntaxNode::new_root(tree);
    assert_eq!(root.text(), source, "lossless law");
    debug_dump(&root, &|k| kind_name(k).to_string())
}

/// `"alphabet ab { '_' }\n"` — ALPHABET spans `alphabet`..`}` inclusive.
/// The trailing newline belongs to ROOT, not to ALPHABET: the node
/// closes right after its `}`.
#[test]
fn an_alphabet_declaration() {
    let d = dump("alphabet ab { '_' }\n");
    assert!(d.contains("ALPHABET"), "{d}");
    assert!(d.trim_end().ends_with("WHITESPACE"), "trailing \\n is ROOT's: {d}");
}

/// A leading comment is a ROOT-level token before ALPHABET opens, not a
/// child of it — trivia flushes into the CURRENT node, and ALPHABET is
/// not open yet.
#[test]
fn a_leading_comment_belongs_to_the_root() {
    let d = dump("// lead\nalphabet ab { '_' }\n");
    let lead = d.find("LINE_COMMENT").expect("comment present");
    let alpha = d.find("ALPHABET").expect("alphabet present");
    assert!(lead < alpha, "comment must precede the node it leads: {d}");
}

#[test]
fn a_machine_with_one_tape() {
    let d = dump("machine {\n  tape main: ab;\n}\n");
    assert!(d.contains("MACHINE"), "{d}");
}

/// Namespaces nest, and a `machine` block may NOT sit inside one — that
/// is a language rule (`docs/tmt/language.md`, namespaces), so the
/// nesting fixture uses declarations, not a machine.
#[test]
fn nested_namespaces() {
    let d = dump("namespace a {\n  namespace b {\n    export alphabet ab { '_', 'a' }\n  }\n}\n");
    assert_eq!(d.matches("NAMESPACE").count(), 2, "{d}");
    let first_ns = d.find("NAMESPACE").expect("namespace");
    let alpha = d.find("ALPHABET").expect("alphabet");
    assert!(first_ns < alpha, "the outer namespace opens first: {d}");
}

#[test]
fn a_use_declaration_with_two_paths() {
    let d = dump("use std::binaryNumbers,\n    other::thing;\n");
    assert_eq!(d.matches("USE_PATH").count(), 2, "{d}");
}

/// The law over every `.tmc` the repo ships, including the flagship
/// brainfuck universal machine and the embedded stdlib.
#[test]
fn the_whole_shipped_corpus_is_lossless() {
    let mut checked = 0;
    for dir in ["tests/golden", "src/stdlib", "../../docs/examples"] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("tmc") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable");
            let tree = parse_green(&src)
                .unwrap_or_else(|e| panic!("{} failed to parse: {e:?}", path.display()));
            let root = SyntaxNode::new_root(tree);
            assert_eq!(root.text(), src, "{} is not lossless", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 9, "expected the whole .tmc corpus, saw {checked}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-turing-machine --test syntax_green`
Expected: FAIL to compile — `parse_green` does not exist.

- [ ] **Step 3: Write the implementation**

Read `crates/post-machine/src/parser.rs`'s weaving first — search it for `GreenSink` and `sink` to see every hook the `.pmc` side needed. The technique:

- The `Parser` gains an `Option<GreenSink>` field. When it is `None` the parser behaves exactly as today; when it is `Some` every `bump()` mirrors the consumed token into the sink. **`bump()` is at `parser.rs:916` — that one hook covers every token the parser consumes, which is why no production needs to know about the sink.**
- Each container production brackets its body with `sink.start(kind)` / `sink.finish()`. Only the six named in the Interfaces block this task; the rest emit tokens without a wrapping node and get theirs in Task 5.
- `parse_green_from_tokens` runs the same grammar walk as the existing `parse` over a `WithComments` token stream and returns the tree instead of the CST. `parse_green` is `lex_with(WithComments)` then that.

**Acceptance parity is not optional and not a matter of care — it must hold by construction.** The sink is the only new thing in the walk, and it makes no decisions. If you find yourself changing a production's control flow, stop and report it: that is the plan being wrong, not the code.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mtc-turing-machine --test syntax_green`
Expected: PASS, 6 tests.

Run: `cargo test -p mtc-turing-machine`
Expected: all green, including the existing parser tests — acceptance is unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src
git commit -m "feat(turing-machine): weave green emission into the .tmc parser"
```

---

### Task 5: the inner containers, and the retro-wrap question

**Files:**
- Modify: `crates/turing-machine/src/parser.rs`
- Modify: `crates/turing-machine/tests/syntax_green.rs`

**Interfaces:**
- Consumes: Task 4.
- Produces: `parse_green` covers every container — adds `WORLD`, `TAPE`, `STATE`, `RULE`, `GRAFT`, `BIND`, `REUSE`, `DOC_RUN` and `ATTR`. After this task no `.tmc` construct is unstructured.

- [ ] **Step 1: Establish the retro-wrap answer before writing any test**

On the `.pmc` side a node that binds a doc run **retro-wraps it**, so the node's extent starts a line or more before its first keyword — and this was true of functions but NOT of namespaces. That asymmetry produced a silent blank-line bug that took a plan to find. `.tmc` puts doc runs on more shapes than `.pmc` does.

So: before writing this task's tests, **dump real trees and find out which `.tmc` nodes retro-wrap their doc run and which do not.** A throwaway binary in a temp directory outside the repo, printing `debug_dump` for a doc-run-carrying `alphabet`, `machine`, `state`, `graph` and `routine`, answers it in one run. **Write the answer into `syntax/mod.rs`'s module doc**, in prose, as a fact later plans can rely on without re-deriving.

Nothing else in this task is safe to write until that is known.

- [ ] **Step 2: Write the failing tests**

Add to `crates/turing-machine/tests/syntax_green.rs`, inserting before the corpus test so it stays last:

```rust
#[test]
fn a_state_with_three_rules() {
    let d = dump(
        "machine {\n  tape main: ab;\n  entry state s {\n\
         ['b'] -> write ['a'] move [>] goto s;\n\
         ['a'] ->             move [>] goto s;\n\
         ['_'] -> stop;\n  }\n}\n",
    );
    assert_eq!(d.matches("RULE").count(), 3, "{d}");
    let state = d.find("STATE").expect("state");
    let first_rule = d.find("RULE").expect("rule");
    assert!(state < first_rule, "STATE opens before its rules: {d}");
}

#[test]
fn a_tape_declaration_is_its_own_node() {
    let d = dump("machine {\n  tape main: ab;\n  tape work: ab;\n}\n");
    assert_eq!(d.matches("TAPE").count(), 2, "{d}");
}

#[test]
fn a_doc_run_before_a_declaration() {
    let d = dump("? one\n? two\nalphabet ab { '_' }\n");
    assert!(d.contains("DOC_RUN"), "{d}");
    assert_eq!(d.matches("DOC_LINE").count(), 2, "{d}");
}

#[test]
fn an_attention_line_with_an_attribute() {
    let d = dump("? doc\n! [deprecated] use the other one\nalphabet ab { '_' }\n");
    assert!(d.contains("ATTENTION_LINE"), "{d}");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test syntax_green`
Expected: FAIL — the new container kinds do not appear in the dumps yet.

- [ ] **Step 4: Write the implementation**

Bracket each remaining container production the same way Task 4 did the outer ones. The doc run is the one that needs the checkpoint mechanism from Task 3: the run's tokens are consumed before the parser knows what declaration they bind to, so the node is opened retroactively with `start_at`.

Apply the answer you established in Step 1: where a `.tmc` node retro-wraps its bound doc run, open the node at the checkpoint taken before the run; where it does not, open it at its first keyword. **Do not make them uniform for tidiness** — reproduce what the grammar actually does, and let the module doc record it.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test syntax_green`
Expected: PASS, 10 tests.

Run: `cargo test -p mtc-turing-machine`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src crates/turing-machine/tests/syntax_green.rs
git commit -m "feat(turing-machine): green nodes for every .tmc container"
```

---

### Task 6: the `.tmc` program generator

**Files:**
- Create: `crates/turing-machine/tests/tmc_property.rs`

**Interfaces:**
- Consumes: Tasks 1-5.
- Produces: a deterministic seed-driven generator of grammar-valid `.tmc` programs, and the losslessness property over them. Plans 8-11 extend the property list; the generator itself is the durable asset.

**Why this task exists here rather than at the end.** The `.pmc` migration compared implementations on hand-written fixtures through six plans and eight mutation-armed reviews, then shipped two Critical bugs living in shapes no fixture contained. A generator was added afterwards and reproduced both within thirteen cases. This plan pays that cost up front, four plans before the first consumer needs it.

- [ ] **Step 1: Write the failing test**

```rust
//! `.tmc` property tests. A deterministic generator of GRAMMAR-VALID
//! programs, asserting the laws the green tree must hold over every one
//! of them. Hand-written fixtures test the shapes someone thought of;
//! this tests the ones nobody did.
//!
//! Every generated program is valid BY CONSTRUCTION — the generator's
//! job is to explore the space of accepted programs, not of rejected
//! ones; the parser's own tests cover rejection.

use mtc_core::syntax::SyntaxNode;
use mtc_turing_machine::parser::parse_green;
use proptest::prelude::*;

/// A deterministic cursor over a byte seed, cycling forever so the
/// generator never has to handle running out of randomness.
struct Cursor<'a> {
    bytes: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn next(&mut self) -> u8 {
        let b = self.bytes[self.i % self.bytes.len()];
        self.i += 1;
        b
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}

fn generate_program(seed: &[u8]) -> String {
    let mut c = Cursor { bytes: seed, i: 0 };
    let mut out = String::new();
    if c.pick(3) == 0 {
        out.push_str("? a doc line\n");
    }
    out.push_str("alphabet ab { '_', 'a', 'b' }\n\n");
    out.push_str("machine {\n  tape main: ab;\n\n");
    let states = 1 + c.pick(3);
    for s in 0..states {
        if c.pick(4) == 0 {
            out.push_str("  // a standalone comment\n");
        }
        let entry = if s == 0 { "entry " } else { "" };
        out.push_str(&format!("  {entry}state s{s} {{\n"));
        let rules = 1 + c.pick(3);
        for r in 0..rules {
            let pat = ["['a']", "['b']", "['_']", "[*]"][c.pick(4)];
            let target = format!("s{}", c.pick(states));
            let body = match c.pick(4) {
                0 => format!("write ['a'] move [>] goto {target}"),
                1 => format!("move [<] goto {target}"),
                2 => "stop".to_string(),
                _ => format!("goto {target}"),
            };
            let trail = if c.pick(5) == 0 { " // note" } else { "" };
            out.push_str(&format!("    {pat} -> {body};{trail}\n"));
            let _ = r;
        }
        out.push_str("  }\n\n");
    }
    out.push_str("}\n");
    out
}

proptest! {
    /// The lossless law over generated programs: the tree's text is the
    /// source, byte for byte.
    #[test]
    fn generated_programs_round_trip(seed in prop::collection::vec(any::<u8>(), 1..64)) {
        let src = generate_program(&seed);
        let tree = parse_green(&src)
            .unwrap_or_else(|e| panic!("generator emitted an invalid program: {e:?}\n{src}"));
        let root = SyntaxNode::new_root(tree);
        prop_assert_eq!(root.text(), src);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mtc-turing-machine --test tmc_property`
Expected: FAIL to compile if `parse_green` is not yet public from the crate root; otherwise PASS. **If it passes on the first run, say so in the report** — this is a law the previous tasks were built to satisfy, so a green first run is the expected outcome, not a skipped step. What must NOT happen is a green run with the generator rejecting everything.

- [ ] **Step 3: Prove the generator is not vacuous**

A generator whose programs all fail to parse, or which explores one shape, is worse than none — it reads as coverage. Prove otherwise, and put the evidence in your report:

- Run with `PROPTEST_CASES=2000` and confirm zero `proptest` global rejects.
- Dump 200 generated programs to a temp directory outside the repo and confirm every one parses and that they are not all identical — report the number of distinct programs.
- Break `layout`'s trivia emission (drop the comment pieces) and confirm the property goes red. Restore. **A property that cannot fail has not been shown to work.**

- [ ] **Step 4: Run the full suite**

Run: `cargo test -p mtc-turing-machine`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/tests/tmc_property.rs
git commit -m "test(turing-machine): a .tmc program generator and the lossless law over it"
```

---

### Task 7: documentation

**Files:**
- Modify: `CLAUDE.md` (the `### The `.tmc` front end` section)
- Modify: `crates/turing-machine/src/syntax/mod.rs` (module doc, if Task 5 left the retro-wrap answer incomplete)

**Interfaces:**
- Consumes: Tasks 1-6. No code changes.

- [ ] **Step 1: Correct the pipeline description**

`CLAUDE.md`'s `### The `.tmc` front end` section opens `lexer → lossless cst → parser → compiler …`. That is still true — the CST is still what every consumer runs — but it is now incomplete: a green tree is built alongside it and nothing consumes it yet.

State exactly that, in one or two sentences, at standing state. Do not narrate this plan. The shape to convey: `.tmc` now has a green syntax tree built by the same parser that builds the CST, held to `text() == source` over the corpus and over generated programs; the CST remains the path every consumer runs, and migrating them is plans 8-11.

- [ ] **Step 2: Check `docs/` for anything now false**

Run:

```bash
grep -rn "cst\|CST" docs/tmt/ docs/core.md --include='*.md'
```

`docs/` is published documentation: it describes what the tool does, not which internal tree it uses. Change a page only if it states something now false. **If every hit is about the `.tma` assembly CST or about behavior, change nothing and say so in your report** — "checked, nothing needed" is the expected outcome here and inventing an edit to look busy is not.

- [ ] **Step 3: Verify**

Run: `cargo test --workspace`
Expected: all green — `cli_docs.rs` quotes `tmt --help` verbatim and would catch help-text drift.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md crates/turing-machine/src/syntax
git commit -m "docs: .tmc builds a green tree alongside its CST"
```

---

## Exit criteria

- `parse_green` produces a tree for every `.tmc` the repo ships, and `root.text() == source` holds on all of them and on generated programs.
- The parser's acceptance, error text and error spans are unchanged — the existing parser tests pass untouched.
- No consumer reads the tree yet; the CST is still the only path in use.
- A `.tmc` generator exists, is proven non-vacuous, and is ready for plans 8-11 to hang properties on.
- `crates/core` and `crates/post-machine` have a zero-line diff for the whole plan.
