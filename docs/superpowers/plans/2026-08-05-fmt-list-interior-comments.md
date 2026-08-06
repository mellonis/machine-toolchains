# List Interior Comments Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A comment written inside a comma-separated list prints where its author put it, instead of being relocated below the enclosing item.

**Architecture:** Every affected CST node gains one sparse field — `Vec<(usize, Comment)>`, each comment keyed by the index of the entry it precedes, with `index == len` meaning "after the last entry". No element type changes, no AST type changes, so `lower_cst` and every existing reader are untouched. The printer buckets that vector per slot and honours each comment's `own_line` flag.

**Tech Stack:** Rust, two crates (`mtc-turing-machine`, `mtc-post-machine`). No new dependencies.

Spec: `docs/superpowers/specs/2026-08-04-fmt-list-interior-comments-design.md`.

## Global Constraints

- **No new dependencies.** The workspace is `serde`/`serde_json` only, `proptest` as a dev-dep. No clap.
- **`crates/core` must not change.** Zero diff.
- **No AST type changes.** `crates/*/src/parser.rs`'s AST structs (`AlphabetElem`, `Signature`, `SigParam`, `BindingArg`, `SymMap`, `MapPair`, `Rule`, `Transition`) keep their exact current shape. The new fields live only on CST nodes. This is what preserves the assertion at `crates/turing-machine/src/compiler.rs:3577` that an AST parsed from a comment-carrying stream equals one parsed from a comment-free stream.
- **`lower_cst` must not change** in either crate.
- **The formatter stays whitespace-only and idempotent.** Re-lexing formatted output must yield the same token stream, comment text and `own_line` included.
- **The compiled stdlib must stay byte-identical** at `-O0` and `-O1`.
- **Committed goldens stay byte-identical** unless a task deliberately edits a fixture, in which case the edit must be provably text-only.
- Code comments cite durable pages by page + parenthetical keyword, e.g. `docs/tmt/fmt.md (interior comments)`. **Never cite anything under `docs/superpowers/`.** No issue numbers, no PR numbers, no hosting-provider URLs.
- **No Claude or Claude Code attribution** anywhere, including commit messages.
- Commit style: conventional commits with scope — `feat(turing-machine):`, `fix(post-machine):`, `docs(editors):`.
- **Gates, run in the FOREGROUND before every commit** (`run_in_background` false — a backgrounded run stalls the session):
  - `cargo test --workspace`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo fmt --check`

## Scope

**In:** TM `alphabet` bodies, `routine`/`graph` signature parameters, `graft`/`bind` argument lists, `call` binding argument lists, `with map` pair lists, and both crates' `use` path lists.

**Out, deferred to a follow-up issue:** TM pattern cells, write vectors, move vectors. These are positional glyph vectors walked per row by codegen and the optimizer; a mid-vector comment is vanishingly rare. They keep today's relocation behaviour, and Task 8 files the issue.

## File Structure

| File | Responsibility |
|---|---|
| `crates/turing-machine/src/cst.rs` | New `interior` fields on `AlphabetCst`, `ReuseCst`, `GraftCst`, `BindCst`, `UseCst`, `RuleCst` |
| `crates/turing-machine/src/parser.rs` | `interior_comments` helper; six list loops populate it |
| `crates/turing-machine/src/fmt.rs` | `Interior` bucketing helper; four renderers consume it; `sym_map_text` becomes column-aware |
| `crates/post-machine/src/cst.rs` | New `interior` field on `UseCst` |
| `crates/post-machine/src/parser.rs` | `interior_comments` helper; the `use` loop populates it |
| `crates/post-machine/src/fmt/mod.rs` | `print_use` consumes the interior comments |
| `crates/turing-machine/tests/golden/a5_call_across_alphabets.tmc` | Fixture gains interior comments reaching both seam surfaces |
| `docs/tmt/fmt.md`, `docs/pmt/fmt.md` | The exception section becomes a statement of the placement rule |

---

### Task 1: The TM interior-comment representation, capture, and printing

Give the five CST-owned TM list nodes their `interior` field, populate it, AND
teach the printer to consume it — in ONE commit.

**Why these are not separable.** Once the parser drains a comment into
`interior`, the old relocation path never sees it. A commit that captures
without printing therefore does not relocate the comment, it DROPS it — turning
the defect this plan fixes into outright data loss. Verified on a built binary
during execution: with capture wired and no printer reading it, an interior
comment vanishes from formatted output entirely. The repository's own guard
cannot catch that, because no corpus fixture carries an interior comment yet.

So Task 1 covers both halves, and the task is done only when a comment written
inside a list still appears in the formatted output, in place.

**Files:**
- Modify: `crates/turing-machine/src/cst.rs`
- Modify: `crates/turing-machine/src/parser.rs`

**Interfaces:**
- Consumes: nothing (first task).
- Produces:
  - `pub interior: Vec<(usize, Comment)>` on `AlphabetCst`, `GraftCst`, `BindCst`, `UseCst`; `pub sig_interior: Vec<(usize, Comment)>` on `ReuseCst`.
  - `Parser::interior_comments(&mut self, index: usize, out: &mut Vec<(usize, Comment)>)`.

- [ ] **Step 1: Write the failing test**

Add to `crates/turing-machine/src/parser/tests.rs`:

```rust
    /// A comment inside a comma-separated list is captured on the enclosing
    /// CST node, keyed by the index of the entry it precedes. An index equal
    /// to the entry count means "after the last entry, before the closer"
    /// (docs/tmt/fmt.md (interior comments)).
    #[test]
    fn interior_comments_are_captured_with_their_entry_index() {
        let src = "alphabet bits { '_', // the blank\n  '0', '1' }\n\n\
                   machine { tape t: bits; entry state s { [*] -> stop; } }\n";
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let cst = parse_cst(&tokens).expect("parses");
        let TopKind::Alphabet(a) = &cst.items[0].kind else {
            panic!("expected an alphabet item");
        };
        assert_eq!(a.elems.len(), 3, "the three glyphs still parse");
        assert_eq!(a.interior.len(), 1, "the comment is captured");
        let (index, comment) = &a.interior[0];
        assert_eq!(*index, 1, "it precedes entry 1 (`'0'`)");
        assert_eq!(comment.text.trim_end(), "// the blank");
        assert!(!comment.own_line, "it trails `'_',` on the same line");
    }

    /// The tail slot: a comment after the last entry has no following entry,
    /// so it is keyed by the entry count itself.
    #[test]
    fn a_comment_after_the_last_entry_is_keyed_by_the_entry_count() {
        let src = "alphabet bits { '_', '0', '1' // the last\n}\n\n\
                   machine { tape t: bits; entry state s { [*] -> stop; } }\n";
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let cst = parse_cst(&tokens).expect("parses");
        let TopKind::Alphabet(a) = &cst.items[0].kind else {
            panic!("expected an alphabet item");
        };
        assert_eq!(a.interior.len(), 1);
        assert_eq!(a.interior[0].0, 3, "keyed by the entry count, not an index");
    }

    /// An own-line comment before the first entry keys to index 0, and keeps
    /// `own_line` so the printer can put it back on its own line.
    #[test]
    fn a_comment_before_the_first_entry_keys_to_zero() {
        let src = "alphabet bits {\n  // leading note\n  '_', '0', '1' }\n\n\
                   machine { tape t: bits; entry state s { [*] -> stop; } }\n";
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let cst = parse_cst(&tokens).expect("parses");
        let TopKind::Alphabet(a) = &cst.items[0].kind else {
            panic!("expected an alphabet item");
        };
        assert_eq!(a.interior.len(), 1);
        assert_eq!(a.interior[0].0, 0);
        assert!(a.interior[0].1.own_line, "it sits on its own line");
    }
```

The test module already imports what these need; if `parse_cst`, `lex_with`, `LexMode`, or `TopKind` is not in scope, add it to the existing `use` block rather than fully-qualifying inline.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine parser::tests::interior -- --nocapture`

Expected: FAIL to compile — `no field 'interior' on type 'AlphabetCst'`.

- [ ] **Step 3: Add the CST fields**

In `crates/turing-machine/src/cst.rs`, add to `AlphabetCst`, `GraftCst`, `BindCst`, and `UseCst` (place it immediately after each node's entry `Vec`):

```rust
    /// Comments written INSIDE the list, in source order, each keyed by the
    /// index of the entry it precedes. An index equal to the entry count
    /// means "after the last entry, before the closer".
    ///
    /// Sparse and index-keyed rather than a per-entry wrapper, so the entry
    /// types are untouched and no AST-facing type carries trivia
    /// (docs/tmt/fmt.md (interior comments)).
    pub interior: Vec<(usize, Comment)>,
```

`ReuseCst` gets the same field under a distinct name, because its list lives one level down inside `sig`:

```rust
    /// Interior comments of the SIGNATURE's parameter list, keyed as
    /// [`AlphabetCst::interior`] is. Named apart from a plain `interior`
    /// because this node's list is `sig.params`, not a field of its own.
    pub sig_interior: Vec<(usize, Comment)>,
```

Also extend the module doc's lossless-contract list. After the bullet beginning "**Comments are trivia at their real source position**", add:

```
//! - **Interior list comments are index-keyed** (`interior`): a comment
//!   inside a comma-separated list is stored against the index of the entry
//!   it precedes, with the entry count meaning "before the closer". The
//!   entry types stay trivia-free, so `lower_cst` hands them to the AST
//!   unchanged.
```

- [ ] **Step 4: Add the parser helper**

In `crates/turing-machine/src/parser.rs`, add to the "comment trivia helpers" section, after `take_trailing`:

```rust
    /// Drain every pending comment written before entry `index` of the list
    /// being parsed, tagging each with that index. Called at the top of each
    /// list-loop iteration and once more before the closer with
    /// `index = entries.len()`, which is how a comment after the last entry
    /// gets a home (docs/tmt/fmt.md (interior comments)).
    fn interior_comments(&mut self, index: usize, out: &mut Vec<(usize, Comment)>) {
        while self.cpos < self.comments.len() && self.comments[self.cpos].sig_index <= self.pos {
            out.push((index, self.comments[self.cpos].comment.clone()));
            self.cpos += 1;
        }
    }
```

- [ ] **Step 5: Populate it in the alphabet body loop**

Replace the body loop in `parse_alphabet` (`crates/turing-machine/src/parser.rs`, currently around line 1305):

```rust
        let mut elems: Vec<AlphabetElem> = Vec::new();
        let mut interior: Vec<(usize, Comment)> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBrace) {
            loop {
                self.interior_comments(elems.len(), &mut interior);
                elems.push(self.alphabet_elem()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RBrace => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `}`")),
                }
            }
        }
        self.interior_comments(elems.len(), &mut interior);
```

and add `interior,` to the returned `AlphabetCst`.

Note the ordering: `capture_open_trailing` already ran before this loop, so a comment on the `{` line is claimed as `open_trailing` and never reaches `interior`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine parser::tests::interior -- --nocapture`

Expected: the three tests PASS.

- [ ] **Step 7: Populate the remaining four CST-owned lists**

`signature` — the caller stores the result on `ReuseCst.sig_interior`, so this returns it:

```rust
    fn signature(&mut self) -> Result<(Signature, Vec<(usize, Comment)>), CompileError> {
        let lp = self.expect(&TokenKind::LParen, "`(` to open the signature")?;
        let mut params: Vec<SigParam> = Vec::new();
        let mut interior: Vec<(usize, Comment)> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RParen) {
            loop {
                self.interior_comments(params.len(), &mut interior);
                params.push(self.sig_param()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RParen => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `)`")),
                }
            }
        }
        self.interior_comments(params.len(), &mut interior);
        let rp = self.expect(&TokenKind::RParen, "`)` to close the signature")?;
        Ok((
            Signature {
                params,
                span: join(lp.span(), rp.span()),
            },
            interior,
        ))
    }
```

`signature()` has exactly ONE call site (`parser.rs:1409`, building `ReuseCst`). Destructure the pair there and store the vector in `sig_interior`.

`binding_args` — serves graft, bind, AND call, so it returns its interior list and each caller decides where it lands:

```rust
    fn binding_args(&mut self) -> Result<(Vec<BindingArg>, Vec<(usize, Comment)>), CompileError> {
        self.expect(&TokenKind::LParen, "`(` to open the binding")?;
        let mut args: Vec<BindingArg> = Vec::new();
        let mut interior: Vec<(usize, Comment)> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RParen) {
            loop {
                self.interior_comments(args.len(), &mut interior);
                args.push(self.binding_arg()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RParen => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `)`")),
                }
            }
        }
        self.interior_comments(args.len(), &mut interior);
        self.expect(&TokenKind::RParen, "`)` to close the binding")?;
        Ok((args, interior))
    }
```

The graft and bind call sites store the vector on `GraftCst.interior` / `BindCst.interior`. **The call-transition call site discards it for now** — `let (args, _interior) = self.binding_args()?;` — because `RuleCst` does not have its field until Task 4. Leave a comment there saying so.

The `use` loop in the same file:

```rust
        let mut paths: Vec<UsePath> = Vec::new();
        let mut interior: Vec<(usize, Comment)> = Vec::new();
        loop {
            self.interior_comments(paths.len(), &mut interior);
            // ... existing path parsing, unchanged ...
```

with `self.interior_comments(paths.len(), &mut interior);` immediately after the loop and `interior,` added to the returned `UseCst`.

- [ ] **Step 8: Run the full gates**

Run, each in the foreground:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Expected: all pass. Behaviour is unchanged — the printer ignores the new fields — so every existing formatter test must still pass exactly as before. If a formatter test changed, something captured a comment that was previously claimed elsewhere: investigate rather than updating the test.

- [ ] **Step 9: Commit**

```bash
git add crates/turing-machine/src/cst.rs crates/turing-machine/src/parser.rs
git commit -m "feat(turing-machine): capture interior comments on list CST nodes

A comment written inside a comma-separated list now rides the enclosing
CST node, keyed by the index of the entry it precedes, with the entry
count meaning \"before the closer\" so a trailing comment has a home too.

Sparse and index-keyed rather than a per-entry wrapper: the entry types
stay trivia-free, so no AST-facing type carries a comment and lower_cst
is untouched. The printer does not read these yet, so formatting is
unchanged."
```

---

### Task 1b: The TM printer half (SAME COMMIT as Task 1)

Make the four CST-owned surfaces print interior comments in place. **This is not
a separate commit** — it lands together with the capture above, for the
data-loss reason stated there. Its steps are listed apart only so the TDD order
stays readable.

**Files:**
- Modify: `crates/turing-machine/src/fmt.rs`
- Test: `crates/turing-machine/tests/fmt_tmc.rs` (corpus-driven; no new file)

**Interfaces:**
- Consumes: `interior` / `sig_interior` from Task 1.
- Produces:
  - `struct Interior<'a> { slots: Vec<Vec<&'a Comment>>, forces_break: bool }`
  - `fn bucket(interior: &[(usize, Comment)], entry_count: usize) -> Interior<'_>`
  - `fn interior_lines(comments: &[&Comment], indent: usize) -> String`

- [ ] **Step 1: Write the failing test**

Add to `crates/turing-machine/tests/fmt_tmc.rs`:

```rust
/// An interior list comment prints where it was written, not relocated
/// below the enclosing item. A trailing comment (`own_line == false`)
/// rides the preceding entry's line; an own-line comment keeps its own
/// line. Either way a LINE comment forces the list multi-line, because
/// nothing can follow `//` on its physical line.
#[test]
fn interior_list_comments_print_in_place() {
    let src = "alphabet bits { '_', // the blank\n  '0', '1' }\n\n\
               machine { tape t: bits; entry state s { [*] -> stop; } }\n";
    let out = format(src).expect("formats");
    let alphabet: Vec<&str> = out
        .lines()
        .take_while(|l| !l.starts_with("machine"))
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        alphabet,
        vec!["alphabet bits {", "  '_', // the blank", "  '0',", "  '1'", "}"],
        "the comment stays on the entry it was written against"
    );
}

/// A comment after the last entry prints before the closer, still inside
/// the list — the position a per-entry scheme could not express.
#[test]
fn a_comment_after_the_last_entry_prints_before_the_closer() {
    let src = "alphabet bits { '_', '0', '1' // the last\n}\n\n\
               machine { tape t: bits; entry state s { [*] -> stop; } }\n";
    let out = format(src).expect("formats");
    let closer = out
        .lines()
        .position(|l| l == "}")
        .expect("the alphabet closes on its own line");
    assert!(
        out.lines().nth(closer - 1).unwrap().contains("// the last"),
        "the comment is the last thing inside the body, got:\n{out}"
    );
}

/// A BLOCK comment with no LINE comment beside it does not force a break:
/// something can follow `*/` on the same physical line.
#[test]
fn an_interior_block_comment_keeps_the_list_on_one_line() {
    let src = "alphabet bits { '_', /* x */ '0', '1' }\n\n\
               machine { tape t: bits; entry state s { [*] -> stop; } }\n";
    let out = format(src).expect("formats");
    assert!(
        out.lines().next().unwrap().contains("/* x */"),
        "the block comment stays inline, got:\n{out}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test fmt_tmc interior -- --nocapture`
Also run: `cargo test -p mtc-turing-machine --test fmt_tmc a_comment_after -- --nocapture`

Expected: FAIL — the comment is currently emitted after the alphabet item, so the assertion on the body lines does not match.

- [ ] **Step 3: Add the bucketing helper**

In `crates/turing-machine/src/fmt.rs`, after `open_trailing_text`:

```rust
/// One list's interior comments, bucketed per slot. `slots` has one entry
/// per position `0..=entry_count`; the last bucket is the tail slot, printed
/// before the closer (docs/tmt/fmt.md (interior comments)).
struct Interior<'a> {
    slots: Vec<Vec<&'a Comment>>,
    /// A LINE comment anywhere in the list forces it multi-line — nothing
    /// can follow `//` on its physical line.
    forces_break: bool,
}

impl Interior<'_> {
    fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_empty())
    }
}

/// Buckets `interior` by slot. An index past `entry_count` is a bug in the
/// parser's bookkeeping; in release it clamps to the tail slot, because a
/// misplaced comment is recoverable and a dropped one is data loss.
fn bucket(interior: &[(usize, Comment)], entry_count: usize) -> Interior<'_> {
    let mut slots: Vec<Vec<&Comment>> = vec![Vec::new(); entry_count + 1];
    let mut forces_break = false;
    for (index, comment) in interior {
        debug_assert!(
            *index <= entry_count,
            "interior comment index {index} exceeds entry count {entry_count}"
        );
        if matches!(comment.kind, CommentKind::Line) {
            forces_break = true;
        }
        slots[(*index).min(entry_count)].push(comment);
    }
    Interior {
        slots,
        forces_break,
    }
}

/// Own-line comments for one slot, each on its own line at `indent`.
fn interior_lines(comments: &[&Comment], indent: usize) -> String {
    let mut out = String::new();
    for c in comments.iter().filter(|c| c.own_line) {
        out.push_str(&comment_line(c, indent));
        out.push('\n');
    }
    out
}

/// The same-line (trailing) comments for one slot, ready to append after a
/// separator. Empty when the slot has only own-line comments.
fn interior_trailing(comments: &[&Comment]) -> String {
    let texts: Vec<String> = comments
        .iter()
        .filter(|c| !c.own_line)
        .map(|c| normalize_comment_text(&c.text))
        .collect();
    if texts.is_empty() {
        String::new()
    } else {
        format!(" {}", texts.join(" "))
    }
}
```

Add `CommentKind` to this file's `use crate::lexer::{...}` list if it is not already imported.

- [ ] **Step 4: Teach `render_alphabet` to use it**

**The indexing rule that governs every renderer in this plan.** A comment
that trails `'_',` is drained from the pending queue *before* `'0'` is
parsed, so it keys to the index of the entry that FOLLOWS it. When
printing, that means:

- slot `i`'s **own-line** comments print above entry `i`;
- slot `i + 1`'s **same-line** comments print at the end of entry `i`'s
  line, after its separator;
- the tail slot's own-line comments print before the closer, and its
  same-line comments have already been consumed by the last entry's line.

Get this backwards and a comment lands one entry late. Every list
renderer below follows the same rule.

In `render_alphabet`, replace the one-line/multi-line decision and the
body loop:

```rust
    let entries: Vec<String> = a.elems.iter().map(alphabet_elem_text).collect();
    let interior = bucket(&a.interior, a.elems.len());
    let one_line = format!("{head} {{ {} }}", entries.join(", "));
    // A comment on the `{`, or any LINE comment inside the body, forces the
    // body onto its own lines whatever the width says.
    if a.open_trailing.is_empty() && interior.is_empty() && one_line.chars().count() <= LINE_WIDTH {
        code.push_str(&one_line);
    } else if a.open_trailing.is_empty()
        && !interior.forces_break
        && one_line.chars().count() <= LINE_WIDTH
    {
        // Block-only interior comments stay inline, each before its entry.
        let mut line = format!("{head} {{ ");
        for (i, entry) in entries.iter().enumerate() {
            for c in interior.slots[i].iter() {
                line.push_str(&normalize_comment_text(&c.text));
                line.push(' ');
            }
            line.push_str(entry);
            if i + 1 < entries.len() {
                line.push_str(", ");
            }
        }
        for c in interior.slots[entries.len()].iter() {
            line.push(' ');
            line.push_str(&normalize_comment_text(&c.text));
        }
        line.push_str(" }");
        code.push_str(&line);
    } else {
        code.push_str(&head);
        code.push_str(" {");
        code.push_str(&open_trailing_text(&a.open_trailing));
        code.push('\n');
        let entry_pad = " ".repeat(indent + INDENT_UNIT);
        for (i, entry) in entries.iter().enumerate() {
            code.push_str(&interior_lines(&interior.slots[i], indent + INDENT_UNIT));
            code.push_str(&entry_pad);
            code.push_str(entry);
            if i + 1 < entries.len() {
                code.push(',');
            }
            // The NEXT slot's same-line comments belong to THIS entry's line
            // — see the indexing rule above.
            code.push_str(&interior_trailing(&interior.slots[i + 1]));
            code.push('\n');
        }
        code.push_str(&interior_lines(
            &interior.slots[entries.len()],
            indent + INDENT_UNIT,
        ));
        code.push_str(&pad);
        code.push('}');
    }
```

Note `interior.slots[i + 1]` is always in range: `slots` has `entries.len() + 1` buckets, and `i` runs to `entries.len() - 1`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test fmt_tmc -- --nocapture`

Expected: the three new tests PASS, and — critically — `every_tmc_source_formats_idempotently` and `formatting_never_changes_a_token` still pass over the whole corpus.

- [ ] **Step 6: Apply the same treatment to `paren_list` and `render_use`**

`paren_list` gains an interior parameter:

```rust
fn paren_list(
    col: usize,
    head: &str,
    entries: &[String],
    tail: &str,
    interior: &Interior<'_>,
) -> String {
    let one_line = format!("{head}({}){tail}", entries.join(", "));
    if (entries.is_empty() || col + one_line.chars().count() <= LINE_WIDTH)
        && interior.is_empty()
    {
        return one_line;
    }
    let entry_pad = " ".repeat(col + INDENT_UNIT);
    let mut out = format!("{head}(\n");
    for (i, entry) in entries.iter().enumerate() {
        out.push_str(&interior_lines(&interior.slots[i], col + INDENT_UNIT));
        out.push_str(&entry_pad);
        out.push_str(entry);
        if i + 1 < entries.len() {
            out.push(',');
        }
        out.push_str(&interior_trailing(&interior.slots[i + 1]));
        out.push('\n');
    }
    out.push_str(&interior_lines(&interior.slots[entries.len()], col + INDENT_UNIT));
    out.push_str(&" ".repeat(col));
    out.push(')');
    out.push_str(tail);
    out
}
```

Every existing `paren_list` call site passes `&bucket(&[], entries.len())` unless it has a real interior list. The signature-rendering site passes `&bucket(&r.sig_interior, r.sig.params.len())`; the graft and bind sites pass their node's `interior`.

`render_use` gains the same treatment, breaking the list across lines when a LINE comment forces it, with continuation lines aligned under the first path (4 columns past the statement indent, clearing `use `).

- [ ] **Step 7: Run the full gates**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 8: Commit**

```bash
git add crates/turing-machine/src/fmt.rs crates/turing-machine/tests/fmt_tmc.rs
git commit -m "feat(turing-machine): print interior list comments in place

The four CST-owned list surfaces — alphabet bodies, signature parameters,
and graft/bind argument lists, plus use paths — now print an interior
comment where its author wrote it instead of relocating it below the
enclosing item.

Placement follows the comment's own_line flag: a trailing comment rides
the preceding entry's line, an own-line comment keeps its own line. A
line comment forces the list multi-line, since nothing can follow // on
its physical line; a block comment with no line comment beside it stays
inline and forces nothing."
```

---

### Task 3: The TM seam surfaces — call binding lists and map pairs

These two lists live inside `RuleCst`'s verbatim `parser::Rule` embed, so they get sparse side-car fields on `RuleCst` rather than touching the AST types.

**Files:**
- Modify: `crates/turing-machine/src/cst.rs`
- Modify: `crates/turing-machine/src/parser.rs`
- Modify: `crates/turing-machine/src/fmt.rs`

**Interfaces:**
- Consumes: `interior_comments`, `bucket`, `interior_lines`, `interior_trailing`, `Interior` from Tasks 1–2.
- Produces: `pub call_args: Vec<(usize, Comment)>` and `pub map_pairs: Vec<(usize, usize, Comment)>` on `RuleCst`.

- [ ] **Step 1: Write the failing test**

Add to `crates/turing-machine/tests/fmt_tmc.rs`:

```rust
/// A comment inside a `call`'s binding list prints in place, not below the
/// rule that carries the call.
#[test]
fn interior_call_binding_comments_print_in_place() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               routine walk(tape t: bits, state done) {\n\
               \x20 entry state g { ['_'] -> goto done; }\n\
               }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call walk(t = m, // the work tape\n\
               \x20                                  done = stop) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let comment_line = out
        .lines()
        .find(|l| l.contains("// the work tape"))
        .expect("the comment survives");
    assert!(
        comment_line.contains("t = m"),
        "it rides the binding it was written against, got: {comment_line:?}"
    );
}

/// A comment inside a `with map` pair list prints in place.
#[test]
fn interior_map_pair_comments_print_in_place() {
    let src = "alphabet bits { '_', '0', '1' }\n\
               alphabet wide { '_', 'x', 'y' }\n\n\
               routine walk(tape t: bits) { entry state g { ['_'] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: wide;\n\
               \x20 entry state s { [*] -> call walk(t = m with map { 'x' -> '0', // low\n\
               \x20                                                   'y' -> '1' }) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let comment_line = out
        .lines()
        .find(|l| l.contains("// low"))
        .expect("the comment survives");
    assert!(
        comment_line.contains("'x' -> '0'"),
        "it rides the pair it was written against, got: {comment_line:?}"
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-turing-machine --test fmt_tmc interior_call -- --nocapture`

Expected: FAIL — the comment currently appears on its own line below the rule.

- [ ] **Step 3: Add the side-car fields**

In `crates/turing-machine/src/cst.rs`, extend `RuleCst`:

```rust
pub struct RuleCst {
    pub rule: Rule,
    pub trailing: Option<Comment>,
    /// Interior comments of a `call` transition's binding list, keyed as
    /// [`AlphabetCst::interior`] is. A SIDE-CAR rather than a field on the
    /// embedded [`Rule`]: that type is handed to the AST verbatim, and the
    /// AST is contractually comment-independent. Empty for any rule whose
    /// transition is not a call.
    pub call_args: Vec<(usize, Comment)>,
    /// Interior comments of a `with map` pair list, keyed by (binding-arg
    /// index, pair index) — map pairs nest one level inside call arguments.
    pub map_pairs: Vec<(usize, usize, Comment)>,
}
```

- [ ] **Step 4: Populate them**

`sym_map` returns its interior list, mirroring `binding_args`:

```rust
    fn sym_map(&mut self) -> Result<(SymMap, Vec<(usize, Comment)>), CompileError> {
        let map_tok = self.expect_kw_tok("map", "`map` after `with`")?;
        self.expect(&TokenKind::LBrace, "`{` to open the map")?;
        let mut pairs: Vec<MapPair> = Vec::new();
        let mut interior: Vec<(usize, Comment)> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBrace) {
            loop {
                self.interior_comments(pairs.len(), &mut interior);
                pairs.push(self.map_pair()?);
                match self.peek().kind {
                    TokenKind::Comma => self.bump(),
                    TokenKind::RBrace => break,
                    _ => return Err(Self::expected(self.peek(), "`,` or `}`")),
                }
            }
        }
        self.interior_comments(pairs.len(), &mut interior);
        let rb = self.expect(&TokenKind::RBrace, "`}` to close the map")?;
        Ok((
            SymMap {
                pairs,
                span: join(map_tok.span(), rb.span()),
            },
            interior,
        ))
    }
```

`binding_arg` threads its map's interior list out alongside the arg; `binding_args` collects them into a `Vec<(usize, usize, Comment)>` by pairing each with the arg index it belongs to. The rule-building site stores the call's own list in `call_args` and the map lists in `map_pairs`. Every non-call rule gets `Vec::new()` for both.

- [ ] **Step 5: Render them**

`transition_text` takes the rule's `call_args` and passes `&bucket(&call_args, args.len())` into `paren_list`.

`sym_map_text` becomes column-aware so a map can break:

```rust
fn sym_map_text(map: &SymMap, col: usize, interior: &Interior<'_>) -> String {
    let pairs: Vec<String> = map
        .pairs
        .iter()
        .map(|pair| {
            let arrow = match pair.arrow {
                MapArrow::Bidirectional => "->",
                MapArrow::ReadOnly => "=>",
            };
            format!("{} {arrow} {}", sym_text(&pair.src), sym_text(&pair.dst))
        })
        .collect();
    if interior.is_empty() {
        return format!("with map {{ {} }}", pairs.join(", "));
    }
    let entry_pad = " ".repeat(col + INDENT_UNIT);
    let mut out = String::from("with map {\n");
    for (i, pair) in pairs.iter().enumerate() {
        out.push_str(&interior_lines(&interior.slots[i], col + INDENT_UNIT));
        out.push_str(&entry_pad);
        out.push_str(pair);
        if i + 1 < pairs.len() {
            out.push(',');
        }
        out.push_str(&interior_trailing(&interior.slots[i + 1]));
        out.push('\n');
    }
    out.push_str(&interior_lines(&interior.slots[pairs.len()], col + INDENT_UNIT));
    out.push_str(&" ".repeat(col));
    out.push('}');
    out
}
```

`binding_value_text` threads `col` and the per-arg map bucket through.

- [ ] **Step 6: Run to verify they pass**

Run: `cargo test -p mtc-turing-machine --test fmt_tmc -- --nocapture`

Expected: all pass, including the corpus idempotence and token-identity guards.

- [ ] **Step 7: Full gates**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 8: Commit**

```bash
git add crates/turing-machine/src/cst.rs crates/turing-machine/src/parser.rs crates/turing-machine/src/fmt.rs crates/turing-machine/tests/fmt_tmc.rs
git commit -m "feat(turing-machine): print interior comments in call and map lists

The two remaining list surfaces live inside the rule node's verbatim AST
embed, so their comments ride side-car fields on the CST rule rather than
the embedded types: the AST is contractually comment-independent, and
wrapping those entry types would have reached the optimizer, the IR, and
codegen for a formatting change.

with map rendering becomes column-aware so a commented map can break
across lines like every other list."
```

---

### Task 4: The PM `use` list

The PM crate has exactly one affected surface.

**Files:**
- Modify: `crates/post-machine/src/cst.rs`
- Modify: `crates/post-machine/src/parser.rs`
- Modify: `crates/post-machine/src/fmt/mod.rs`

**Interfaces:**
- Consumes: nothing from the TM crate — the crates are independent. The design mirrors Tasks 1–2.
- Produces: `pub interior: Vec<(usize, Comment)>` on PM's `UseCst`.

- [ ] **Step 1: Write the failing test**

Add to `crates/post-machine/tests/fmt_programs.rs`:

```rust
/// A comment inside a `use` path list prints in place, not relocated below
/// the statement (docs/pmt/fmt.md (interior comments)).
#[test]
fn interior_use_list_comments_print_in_place() {
    let src = "use std::goToEnd, // walk right\n\
               \x20   std::goToBegin;\n\n\
               main() {\n 1: @goToEnd();\n 2: halt;\n}\n";
    let out = format(src).expect("formats");
    let comment_line = out
        .lines()
        .find(|l| l.contains("// walk right"))
        .expect("the comment survives");
    assert!(
        comment_line.contains("std::goToEnd"),
        "it rides the path it was written against, got: {comment_line:?}"
    );
    assert!(
        !out.trim_end().ends_with("// walk right"),
        "and is not relocated to the end"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mtc-post-machine --test fmt_programs interior_use -- --nocapture`

Expected: FAIL — the comment currently prints on its own line after the `use` statement.

- [ ] **Step 3: Add the field**

In `crates/post-machine/src/cst.rs`, extend `UseCst`:

```rust
    /// Comments written INSIDE the path list, in source order, each keyed by
    /// the index of the path it precedes. An index equal to the path count
    /// means "after the last path, before the `;`".
    ///
    /// Sparse and index-keyed rather than a per-path wrapper, so [`UsePath`]
    /// stays trivia-free and `lower_cst` hands it to the AST unchanged
    /// (docs/pmt/fmt.md (interior comments)).
    pub interior: Vec<(usize, Comment)>,
```

- [ ] **Step 4: Add the parser helper and populate**

In `crates/post-machine/src/parser.rs`, add beside `drain_pending_comments`:

```rust
    /// Drain every pending comment written before entry `index` of the list
    /// being parsed, tagging each with that index. Called at the top of each
    /// list-loop iteration and once more before the closer with
    /// `index = entries.len()`, which is how a comment after the last entry
    /// gets a home (docs/pmt/fmt.md (interior comments)).
    fn interior_comments(&mut self, index: usize, out: &mut Vec<(usize, Comment)>) {
        while self.cpos < self.comments.len() && self.comments[self.cpos].sig_index <= self.pos {
            out.push((index, self.comments[self.cpos].comment.clone()));
            self.cpos += 1;
        }
    }
```

In the `use` loop, call `self.interior_comments(paths.len(), &mut interior);` at the top of each iteration and once after the loop, then add `interior,` to the constructed `UseCst`.

- [ ] **Step 5: Render it**

Replace `print_use`:

```rust
fn print_use(out: &mut String, u: &UseCst, indent: usize) {
    let rendered: Vec<String> = u.paths.iter().map(render_use_path).collect();
    let has_line = u
        .interior
        .iter()
        .any(|(_, c)| matches!(c.kind, CommentKind::Line));
    out.push_str(&" ".repeat(indent));
    out.push_str("use ");
    if u.interior.is_empty() {
        out.push_str(&rendered.join(", "));
    } else {
        // Continuation lines align under the first path, clearing `use `.
        let cont = " ".repeat(indent + 4);
        let slot = |ix: usize, own_line: bool| -> Vec<&Comment> {
            u.interior
                .iter()
                .filter(move |(i, c)| *i == ix && c.own_line == own_line)
                .map(|(_, c)| c)
                .collect()
        };
        for (i, path) in rendered.iter().enumerate() {
            for c in slot(i, true) {
                out.push('\n');
                out.push_str(&cont);
                out.push_str(&normalize_comment_text(&c.text));
                out.push('\n');
                out.push_str(&cont);
            }
            if i > 0 && slot(i, true).is_empty() {
                out.push('\n');
                out.push_str(&cont);
            }
            out.push_str(path);
            if i + 1 < rendered.len() {
                out.push(',');
                // The NEXT slot's same-line comments belong to THIS line:
                // a comment after `a,` is drained before `b` parses, so it
                // keys to the following index.
                for c in slot(i + 1, false) {
                    out.push(' ');
                    out.push_str(&normalize_comment_text(&c.text));
                }
            }
        }
        // A tail-slot own-line comment sits before the `;`.
        for c in slot(rendered.len(), true) {
            out.push('\n');
            out.push_str(&cont);
            out.push_str(&normalize_comment_text(&c.text));
        }
        debug_assert!(has_line || u.interior.iter().all(|(_, c)| !matches!(c.kind, CommentKind::Line)));
    }
    out.push(';');
    if let Some(tc) = &u.trailing {
        out.push(' ');
        out.push_str(&normalize_comment_text(&tc.comment.text));
    }
    out.push('\n');
}
```

Add `CommentKind` and `Comment` to the file's `use crate::lexer::{...}` list if absent.

`has_line` exists only to record that a LINE comment is what forces the break, and the `debug_assert!` keeps it live so clippy does not flag it unused. If clippy objects to the assertion's shape, delete both the binding and the assertion rather than silencing the lint — the multi-line path is taken whenever `interior` is non-empty, so the flag is documentation, not control flow.

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test -p mtc-post-machine --test fmt_programs -- --nocapture`

Expected: the new test PASSES and every existing formatter test still passes — in particular the idempotence and property tests in `fmt_property.rs`.

- [ ] **Step 7: Full gates**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 8: Commit**

```bash
git add crates/post-machine/src/cst.rs crates/post-machine/src/parser.rs crates/post-machine/src/fmt/mod.rs crates/post-machine/tests/fmt_programs.rs
git commit -m "feat(post-machine): print interior use-list comments in place

PM's use path list is its one comma-separated list with this defect, and
it takes the same index-keyed sparse representation the sibling crate
uses: the path type stays trivia-free, so lower_cst is untouched.

Honouring the comment's own_line flag means an own-line comment keeps its
own line here, which the statement comma-group printer does not do. That
difference is deliberate — the comma-group behaviour is shipped and
documented, and is not changed by this."
```

---

### Task 5: Prove the guards fire, and pin the corpus

The repository's own formatter guards should already catch this defect. Make that true rather than assumed, by putting interior comments into a real fixture.

**Files:**
- Modify: `crates/turing-machine/tests/golden/a5_call_across_alphabets.tmc`

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces: no code interface.

- [ ] **Step 1: Record the baseline**

Run and save the output:
```bash
cargo test -p mtc-turing-machine --test golden_programs 2>&1 | tail -3
cargo test -p mtc-turing-machine --test tmc_golden 2>&1 | tail -3
```

Expected: both pass. Note the fixture's current compiled bytes are what must not change.

- [ ] **Step 2: Add interior comments to the fixture**

Edit `crates/turing-machine/tests/golden/a5_call_across_alphabets.tmc` so its lists carry comments in all three positions. It already has an alphabet, a signature, a `use` list, and a `call` with a `with map`, so one file reaches five of the six surfaces:

```
alphabet bits { '_', // the blank
                '0', '1' }
alphabet wide { '_', 'a', 'b', '0', '1' }

namespace mylib {
  export routine plusOne(tape num: bits // the number under the head
  ) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*]   -> write ['1'] return;
    }
  }
}

use mylib::plusOne; // the only import

machine {
  tape ctl:  bits;
  tape data: wide;

  entry state main {
    ['1', *] -> call plusOne(num = data with map { '0'->'0', // low
                                                   '1'->'1' }) then done;
    [*, *]   -> move [>, .] goto main;
  }

  state done { [*, *] -> stop; }
}
```

- [ ] **Step 3: Confirm the compiled output is unchanged**

Run:
```bash
cargo test -p mtc-turing-machine --test golden_programs
cargo test -p mtc-turing-machine --test tmc_golden
```

Expected: PASS with no golden regeneration. Comments are trivia; if a golden moved, something is wrong with the parser change, not the fixture.

- [ ] **Step 4: Confirm the formatter guards now exercise the fixture**

Run: `cargo test -p mtc-turing-machine --test fmt_tmc -- --nocapture`

Expected: `every_tmc_source_formats_idempotently` and `formatting_never_changes_a_token` both PASS over the edited fixture. The second is the one that matters — it re-lexes the formatted text and compares comment text and `own_line` against the original, so a relocated comment fails it.

- [ ] **Step 5: Prove the guard would catch a regression**

Temporarily revert one renderer to its relocating behaviour — the simplest is to make `bucket` return empty slots:

```rust
fn bucket(interior: &[(usize, Comment)], entry_count: usize) -> Interior<'_> {
    let _ = interior;                       // TEMPORARY
    Interior { slots: vec![Vec::new(); entry_count + 1], forces_break: false }
}
```

Run `cargo test -p mtc-turing-machine --test fmt_tmc` and record the failure output. Then restore the real body and confirm the suite passes again. Put the observed failure in your report — a guard nobody has watched fail is not yet a guard.

- [ ] **Step 6: Confirm the compiled stdlib is byte-identical**

Run: `cargo test -p mtc-turing-machine --test stdlib_golden`

Expected: PASS. This is the property that proves the change is text-only.

- [ ] **Step 7: Full gates**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 8: Commit**

```bash
git add crates/turing-machine/tests/golden/a5_call_across_alphabets.tmc
git commit -m "test(turing-machine): exercise interior list comments in the corpus

The formatter's whitespace-only and idempotence guards run over every
.tmc in the repository, and their token signature already compares each
comment's own_line flag — so they encoded this defect all along and
simply never met a fixture that triggered it.

This fixture reaches five of the six affected surfaces in one file: an
alphabet body, a signature parameter list, a use list, a call binding
list, and a with-map pair list. Its compiled output is unchanged, which
is what proves the change is text-only."
```

---

### Task 6: The per-surface matrix

One assertion per surface and position, so a regression names exactly what broke rather than failing a corpus-wide guard.

**Files:**
- Create: `crates/turing-machine/tests/fmt_interior.rs`

**Interfaces:**
- Consumes: `mtc_turing_machine::fmt::format`.
- Produces: no code interface.

- [ ] **Step 1: Write the matrix**

Create `crates/turing-machine/tests/fmt_interior.rs`:

```rust
//! Per-surface coverage for interior list comments: each affected list, at
//! each of the three positions a comment can occupy inside it.
//!
//! The corpus guards in `fmt_tmc.rs` prove the property holds over real
//! sources; these prove WHICH surface broke when it stops holding.

use mtc_turing_machine::fmt::format;

/// Formats `src` and returns the line carrying `needle`, panicking with the
/// whole output when it is absent — a relocated comment is still present in
/// the text, so the useful failure names the line it landed on.
fn line_with<'a>(out: &'a str, needle: &str) -> &'a str {
    out.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` missing from:\n{out}"))
}

const MACHINE: &str = "machine { tape t: bits; entry state s { [*] -> stop; } }\n";

#[test]
fn alphabet_between_entries() {
    let src = format!("alphabet bits {{ '_', // note\n  '0', '1' }}\n\n{MACHINE}");
    let out = format(&src).expect("formats");
    assert!(line_with(&out, "// note").contains("'_'"));
}

#[test]
fn alphabet_before_first_entry() {
    let src = format!("alphabet bits {{\n  // note\n  '_', '0', '1' }}\n\n{MACHINE}");
    let out = format(&src).expect("formats");
    let idx = out.lines().position(|l| l.contains("// note")).unwrap();
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("'_'"),
        "the comment precedes the first entry, got:\n{out}"
    );
}

#[test]
fn alphabet_after_last_entry() {
    let src = format!("alphabet bits {{ '_', '0', '1' // note\n}}\n\n{MACHINE}");
    let out = format(&src).expect("formats");
    let idx = out.lines().position(|l| l.contains("// note")).unwrap();
    assert_eq!(out.lines().nth(idx + 1).unwrap(), "}", "it precedes the closer");
}

#[test]
fn alphabet_block_comment_stays_inline() {
    let src = format!("alphabet bits {{ '_', /* x */ '0', '1' }}\n\n{MACHINE}");
    let out = format(&src).expect("formats");
    assert!(out.lines().next().unwrap().contains("/* x */"));
}

#[test]
fn signature_parameter_list() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               routine walk(tape t: bits, // note\n\
               \x20            state done) {\n\
               \x20 entry state g { ['_'] -> goto done; }\n\
               }\n\n\
               machine { tape m: bits; entry state s { [*] -> call walk(t = m, done = stop) then stop; } }\n";
    let out = format(src).expect("formats");
    assert!(line_with(&out, "// note").contains("tape t: bits"));
}

#[test]
fn graft_argument_list() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               graph walk(tape t: bits, state done) {\n\
               \x20 entry state g { ['_'] -> goto done; }\n\
               }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry graft walk(t = m, // note\n\
               \x20                  done = stop);\n\
               }\n";
    let out = format(src).expect("formats");
    assert!(line_with(&out, "// note").contains("t = m"));
}

#[test]
fn bind_argument_list() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               routine walk(tape t: bits, state done) {\n\
               \x20 entry state g { ['_'] -> goto done; }\n\
               }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> goto s2; }\n\
               \x20 state s2 { [*] -> stop; }\n\
               \x20 bind walk(t = m, // note\n\
               \x20           done = stop) as w;\n\
               }\n";
    let out = format(src).expect("formats");
    assert!(line_with(&out, "// note").contains("t = m"));
}

#[test]
fn use_path_list() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               namespace lib {\n\
               \x20 export routine p(tape t: bits) { entry state g { [*] -> stop; } }\n\
               \x20 export routine q(tape t: bits) { entry state g { [*] -> stop; } }\n\
               }\n\n\
               use lib::p, // note\n\
               \x20   lib::q;\n\n\
               machine { tape m: bits; entry state s { [*] -> call p(t = m) then stop; } }\n";
    let out = format(src).expect("formats");
    assert!(line_with(&out, "// note").contains("lib::p"));
}
```

- [ ] **Step 2: Run the matrix**

Run: `cargo test -p mtc-turing-machine --test fmt_interior -- --nocapture`

Expected: all PASS. Any failure names the exact surface and position.

- [ ] **Step 3: Add the clamp unit test**

No fixture can reach the release-mode clamp in `bucket`, so it needs a unit test. Add to `crates/turing-machine/src/fmt.rs`'s test module (create a `#[cfg(test)] mod tests` at the end of the file if there is none):

```rust
    /// An out-of-range index clamps to the tail slot rather than dropping
    /// the comment: a misplaced comment is a bug, a lost one is data loss.
    #[test]
    fn an_out_of_range_interior_index_clamps_to_the_tail() {
        let comment = Comment {
            text: "// stray".into(),
            kind: CommentKind::Line,
            own_line: false,
        };
        let interior = vec![(99, comment)];
        let bucketed = bucket(&interior, 2);
        assert_eq!(bucketed.slots.len(), 3, "one slot per position 0..=count");
        assert_eq!(bucketed.slots[2].len(), 1, "clamped into the tail slot");
        assert!(bucketed.forces_break, "a line comment still forces the break");
    }
```

Note this test only runs in release-assertion terms; the `debug_assert!` in `bucket` fires first under `cargo test`'s default debug profile. Guard the test accordingly:

```rust
    #[test]
    #[cfg_attr(debug_assertions, ignore = "the debug_assert fires first; this pins release behaviour")]
    fn an_out_of_range_interior_index_clamps_to_the_tail() {
```

- [ ] **Step 4: Run it in release**

Run: `cargo test -p mtc-turing-machine --release --lib fmt::tests::an_out_of_range -- --ignored --nocapture`

Expected: PASS.

- [ ] **Step 5: Full gates**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/tests/fmt_interior.rs crates/turing-machine/src/fmt.rs
git commit -m "test(turing-machine): per-surface matrix for interior list comments

The corpus guards prove the property holds across real sources; these
prove which surface broke when it stops holding, one assertion per list
kind and position.

Adds the one case no fixture can reach: an out-of-range slot index
clamps to the tail rather than dropping the comment, pinned as a release
test because the debug assertion fires first."
```

---

### Task 7: Documentation

**Files:**
- Modify: `docs/tmt/fmt.md`
- Modify: `docs/pmt/fmt.md`

**Interfaces:**
- Consumes: everything from Tasks 1–6.
- Produces: no code interface.

- [ ] **Step 1: Rewrite the TM exception section**

`docs/tmt/fmt.md`'s `### The trivia exception` currently claims a comment inside a `call`/`graft` binding list, a signature parameter list, or an `alphabet` body relocates. That is now false for those three and was always incomplete — `bind` argument lists, `use` path lists, and `with map` pairs behave the same way.

Replace the section with a statement of the rule:

```markdown
### Comments inside a list

A comment written inside a comma-separated list — an `alphabet` body, a
`routine`/`graph` signature, a `call`/`graft`/`bind` argument list, a
`with map` pair list, or a `use` path list — prints where it was written.

Placement follows what the author did. A comment that trails code on its
line stays on that line:

```
alphabet bit { '_', // the blank
               '0', '1' }
```

A comment on its own line keeps its own line, above the entry it
precedes. A `//` comment always forces the list onto multiple lines,
because nothing can follow it on its physical line.

A `/* … */` comment with no `//` beside it stays inline in an `alphabet`
body and a `use` list, which can print their entries inline around it.
The bracketed lists — a signature, a `call`/`graft`/`bind` argument list,
and a `with map` pair list — have no inline-with-comments form, so any
interior comment there breaks the list across lines. The comment still
rides the entry it was written against either way.

A comment after the last entry prints before the closing delimiter, still
inside the list.

**The exception that remains**: a comment inside a pattern, write, or move
vector — `['0', /* here */ '1']` — still reprints as an own-line comment
after the enclosing rule. Those vectors are positional and are walked per
row by the compiler; giving them per-entry trivia is tracked separately.
```

- [ ] **Step 2: Add the PM half**

`docs/pmt/fmt.md` does not mention this behaviour at all, though PM's `use` path list has it. Add after the "Comma groups" section:

```markdown
## Comments inside a `use` list

A comment written inside a `use` path list prints where it was written —
trailing a path if that is where the author put it, on its own line if
not:

```c
use std::goToEnd, // walk right
    std::goToBegin;
```

Continuation lines align under the first path.

Note this differs from a statement's comma group, above, where an own-line
comment is drawn up onto the preceding line. That difference is
deliberate: the comma-group behaviour is long-standing and unchanged.
```

- [ ] **Step 3: Verify every claim against the formatter**

For each example in both pages, run it through the real binary and confirm the documented output:

```bash
cargo build --release
printf 'alphabet bit { %s, // the blank\n %s, %s }\n\nmachine { tape t: bit; entry state s { [*] -> stop; } }\n' "'_'" "'0'" "'1'" \
  | target/release/tmt fmt - --lang tmc
```

Every code block in both pages must be actual output, not a description of it. If the formatter disagrees with the page, **the formatter wins** — fix the page and note the discrepancy in your report.

- [ ] **Step 4: Check for forge references**

Run:
```bash
rg -n 'github\.com|#[0-9]{1,3}\b|docs/superpowers' docs/tmt/fmt.md docs/pmt/fmt.md
```

Expected: no hits.

- [ ] **Step 5: Full gates**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 6: Commit**

```bash
git add docs/tmt/fmt.md docs/pmt/fmt.md
git commit -m "docs(editors): state the interior-comment rule on both fmt pages

The TM page described a relocation that no longer happens, and named
three list kinds as though they were the closed set when six surfaces
behaved that way. It now states the placement rule, and scopes the
remaining exception to the glyph vectors that keep the old behaviour.

The PM page did not mention the behaviour at all, though its own use-path
list has it. It gains that section, including the deliberate difference
from statement comma groups, which are unchanged."
```

---

### Task 8: File the deferred-vector follow-up

**Files:** none — this task creates a tracker issue.

**Interfaces:**
- Consumes: the shipped mechanism from Tasks 1–6.
- Produces: no code interface.

- [ ] **Step 1: Confirm the vectors still relocate**

```bash
cargo build --release
printf "alphabet bits { '_', '0', '1' }\n\nmachine {\n  tape a: bits;\n  tape b: bits;\n  entry state s {\n    ['0', // first tape\n     '1'] -> stop;\n  }\n}\n" \
  | target/release/tmt fmt - --lang tmc
```

Expected: the comment appears below the rule. Record the exact output.

- [ ] **Step 2: File the issue**

```bash
gh issue create --title "fmt: interior comments in pattern, write, and move vectors still relocate" --body "$(cat <<'BODY'
The interior-comment work covered the named-argument, alphabet, and import lists. Three positional glyph-vector surfaces were deliberately left out and still relocate a comment below the enclosing rule:

- pattern cells — `['0', /* here */ '1']`
- write vectors — `write ['1', /* here */ '0']`
- move vectors — `move [>, /* here */ .]`

**The mechanism already shipped extends to them unchanged**: sparse index-keyed `Vec<(usize, Comment)>` side-cars on the CST rule node, exactly as `call_args` and `map_pairs` work today, bucketed by the same printer helper.

**Why they were deferred, so the trade-off is on record**: these three are positional and hot. `Pattern.cells`, `WriteVec`, and `MoveVec` are walked per row by codegen and the optimizer, and a mid-vector comment is vanishingly rare in real sources. The cost of per-cell trivia was judged to exceed the benefit at the time; nothing about the design blocks it.

The formatter pages document this as the one remaining exception, so closing this issue means editing both.
BODY
)"
```

- [ ] **Step 3: Record the issue number**

Note the created issue's number in your report so the round's summary can reference it.

---

## Final verification

- [ ] **All gates green from a clean tree**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **`crates/core` untouched**

```bash
git diff --stat master...HEAD -- crates/core
```

Expected: empty.

- [ ] **No AST type gained a trivia field**

```bash
git diff master...HEAD -- crates/turing-machine/src/parser.rs | rg '^\+.*Comment'
```

Expected: only the `interior_comments` helper and the tuple-returning signatures. No `Comment` field added to `AlphabetElem`, `SigParam`, `BindingArg`, `MapPair`, `Rule`, or `Signature`.

- [ ] **The AST-purity assertion still holds**

```bash
cargo test -p mtc-turing-machine --lib compiler:: 2>&1 | rg 'test result'
```

Expected: pass. That suite contains the assertion that an AST parsed from a comment-carrying stream equals one parsed from a comment-free stream.

- [ ] **The compiled stdlib is byte-identical**

```bash
cargo test -p mtc-turing-machine --test stdlib_golden
```

- [ ] **Goldens unchanged except the deliberately edited fixture**

```bash
git diff --name-only master...HEAD -- crates/turing-machine/tests/golden/
```

Expected: only `a5_call_across_alphabets.tmc`, and its compiled goldens must be unchanged.
