# Glyph-Vector Interior Comments Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A comment written inside a `.tmc` pattern, write, or move vector prints where its author put it, instead of being relocated below the enclosing rule.

**Architecture:** Three sparse `Vec<(usize, Comment)>` side-cars on `RuleCst`, keyed by cell index exactly as the shipped `call_args` is — the vectors live inside the verbatim `parser::Rule` embed, and the AST is contractually comment-independent. The printer splits on comment kind: a block comment stays inline inside the vector so the state-block grid is untouched, while a line comment forces that one rule off the grid onto a multi-line form.

**Tech Stack:** Rust, `crates/turing-machine` only. No new dependencies.

Closes the deferred half of the interior-comment work.

## Global Constraints

- **No new dependencies.** `serde`/`serde_json` only, `proptest` as a dev-dep. No clap.
- **`crates/core` and `crates/post-machine` must not change.** Zero diff. PM has no glyph vectors.
- **No AST type changes.** `Pattern`, `PatternCell`, `WriteVec`, `MoveVec`, `Rule`, `Transition` keep their exact shape. This preserves the assertion at `crates/turing-machine/src/compiler.rs:3577` that an AST parsed from a comment-carrying token stream equals one parsed comment-free.
- **`lower_cst` must not change.**
- **The formatter stays whitespace-only and idempotent.** Re-lexing formatted output must yield the same token stream, comment text and `own_line` included.
- **The compiled stdlib must stay byte-identical**, and committed goldens must not move.
- Code comments cite durable pages by page + parenthetical keyword, e.g. `docs/tmt/fmt.md (interior comments)`. **Never cite `docs/superpowers/`.** No issue numbers, no URLs.
- **No Claude or Claude Code attribution** anywhere, including commit messages.
- Conventional commits with scope.

## The three lessons this plan exists to not relearn

The sibling round that fixed the other seven surfaces produced six plan defects and four shipped data-loss bugs. All of them were caught by running the built binary; none by reading a diff or watching a green suite. Concretely:

1. **Capture and printing must land in ONE commit.** Once the parser drains a comment into a side-car, the old relocation path never sees it — so a capture-only commit does not misplace the comment, it *destroys* it.
2. **Cover slot 0 and the tail slot from the start.** Four defects lived in exactly those positions: a comment immediately after an opening delimiter, and one immediately before the closer. Nothing tested them.
3. **Assert the comment SURVIVES before asserting placement.** A placement-only assertion passes vacuously when the comment is dropped, which is how all four hid.

## The grid, which is what makes this different from the sibling work

`grid_for` (`crates/turing-machine/src/fmt.rs`) computes each column's width from the *rendered text* of every rule in a state, and `render_rule` pads to those widths so columns line up. That is why comment handling here splits on kind:

- A **block** comment can sit inside a vector on one line. `pattern_text` and friends render it, the width calculation naturally includes it, and the grid still aligns.
- A **line** comment cannot — nothing may follow `//` on its physical line. The rule carrying it must render multi-line, and must be **excluded from the grid's width computation**, or it would inflate columns for rules that do not need them.

## File Structure

| File | Responsibility |
|---|---|
| `crates/turing-machine/src/cst.rs` | Three new side-cars on `RuleCst` |
| `crates/turing-machine/src/parser.rs` | The three vector loops populate them |
| `crates/turing-machine/src/fmt.rs` | Inline-block rendering; the off-grid multi-line form; `grid_for` skips line-commented rules |
| `crates/turing-machine/tests/fmt_interior.rs` | Matrix rows for the three surfaces |
| `docs/tmt/fmt.md` | The remaining-exception section becomes a statement of the rule |

---

### Task 1: Capture and print glyph-vector interior comments

**One commit.** Parser capture and printer consumption are inseparable — see lesson 1.

**Files:**
- Modify: `crates/turing-machine/src/cst.rs`
- Modify: `crates/turing-machine/src/parser.rs`
- Modify: `crates/turing-machine/src/fmt.rs`
- Test: `crates/turing-machine/tests/fmt_interior.rs`

**Interfaces:**
- Consumes: the shipped `interior_comments` parser helper, and `Interior` / `bucket` / `interior_lines` / `interior_trailing` in `fmt.rs`.
- Produces on `RuleCst`: `pattern_cells: Vec<(usize, Comment)>`, `write_cells: Vec<(usize, Comment)>`, `move_cells: Vec<(usize, Comment)>`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/turing-machine/tests/fmt_interior.rs`. Note every assertion checks survival first, per lesson 3.

```rust
/// A BLOCK comment inside a pattern vector stays inline, so the state's
/// grid alignment is untouched.
#[test]
fn pattern_block_comment_stays_inline() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               machine {\n  tape a: bits;\n  tape b: bits;\n\
               \x20 entry state s {\n\
               \x20   ['0', /* lo */ '1'] -> stop;\n\
               \x20   [*, *] -> move [>, .] goto s;\n\
               \x20 }\n}\n";
    let out = format(src).expect("formats");
    let line = line_with(&out, "/* lo */");
    assert!(line.contains("['0',") && line.contains("'1']"),
        "the comment stays inside the vector, got: {line:?}");
    assert!(line.contains("->"),
        "and the rule is still one grid row, got: {line:?}");
}

/// A LINE comment inside a pattern vector forces that rule off the grid
/// onto a multi-line form. The comment must survive.
#[test]
fn pattern_line_comment_breaks_the_rule_off_the_grid() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               machine {\n  tape a: bits;\n  tape b: bits;\n\
               \x20 entry state s {\n\
               \x20   ['0', // the low bit\n\
               \x20    '1'] -> stop;\n\
               \x20 }\n}\n";
    let out = format(src).expect("formats");
    let line = line_with(&out, "// the low bit");
    assert!(line.contains("'0'"),
        "it rides the cell it was written against, got: {line:?}");
}

/// The write vector, same two kinds.
#[test]
fn write_vector_comments_print_in_place() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               machine {\n  tape a: bits;\n  tape b: bits;\n\
               \x20 entry state s {\n\
               \x20   [*, *] -> write ['1', /* hi */ '0'] stop;\n\
               \x20 }\n}\n";
    let out = format(src).expect("formats");
    assert!(line_with(&out, "/* hi */").contains("write ['1',"));
}

/// The move vector, same.
#[test]
fn move_vector_comments_print_in_place() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               machine {\n  tape a: bits;\n  tape b: bits;\n\
               \x20 entry state s {\n\
               \x20   [*, *] -> move [>, /* stay */ .] stop;\n\
               \x20 }\n}\n";
    let out = format(src).expect("formats");
    assert!(line_with(&out, "/* stay */").contains("move [>,"));
}

/// Slot 0 — immediately after the opening `[`. This position destroyed
/// comments on four surfaces in the sibling work; it is covered from the
/// start here (lesson 2).
#[test]
fn pattern_slot0_comment_survives() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               machine {\n  tape a: bits;\n  tape b: bits;\n\
               \x20 entry state s {\n\
               \x20   [/* first */ '0', '1'] -> stop;\n\
               \x20 }\n}\n";
    let out = format(src).expect("formats");
    assert!(out.contains("/* first */"), "slot-0 comment survives:\n{out}");
}

/// The tail slot — immediately before the closing `]`. The other half of
/// lesson 2.
#[test]
fn pattern_tail_slot_comment_survives() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               machine {\n  tape a: bits;\n  tape b: bits;\n\
               \x20 entry state s {\n\
               \x20   ['0', '1' /* last */] -> stop;\n\
               \x20 }\n}\n";
    let out = format(src).expect("formats");
    assert!(out.contains("/* last */"), "tail-slot comment survives:\n{out}");
}

/// A line-commented rule must not inflate the grid for its neighbours.
#[test]
fn a_line_commented_rule_does_not_widen_the_grid() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               machine {\n  tape a: bits;\n  tape b: bits;\n\
               \x20 entry state s {\n\
               \x20   [*, *] -> move [>, .] goto s;\n\
               \x20   ['0', // note\n\
               \x20    '1'] -> stop;\n\
               \x20 }\n}\n";
    let out = format(src).expect("formats");
    let neighbour = line_with(&out, "goto s");
    assert!(neighbour.trim_start().starts_with("[*, *] ->"),
        "the uncommented rule keeps tight alignment, got: {neighbour:?}");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p mtc-turing-machine --test fmt_interior pattern_ write_vector move_vector a_line_commented`

Expected: FAIL — comments are currently relocated below the rule, so `line_with` finds them on their own line rather than beside a cell.

- [ ] **Step 3: Add the side-cars**

In `crates/turing-machine/src/cst.rs`, extend `RuleCst` after `map_pairs`:

```rust
    /// Interior comments of the rule's pattern vector, keyed by the index
    /// of the cell each precedes, with the cell count meaning "before the
    /// closing `]`". A SIDE-CAR for the same reason [`Self::call_args`] is:
    /// the vector types are handed to the AST verbatim
    /// (docs/tmt/fmt.md (interior comments)).
    pub pattern_cells: Vec<(usize, Comment)>,
    /// Interior comments of the rule's `write` vector, keyed as
    /// [`Self::pattern_cells`] is. Empty when the rule has no write vector.
    pub write_cells: Vec<(usize, Comment)>,
    /// Interior comments of the rule's `move` vector, keyed as
    /// [`Self::pattern_cells`] is. Empty when the rule has no move vector.
    pub move_cells: Vec<(usize, Comment)>,
```

- [ ] **Step 4: Populate them in the parser**

`pattern`, `write_vec`, and `move_vec` each return a tuple, exactly as `binding_args` and `sym_map` already do. In each, declare `let mut interior: Vec<(usize, Comment)> = Vec::new();` beside the cell vector, call `self.interior_comments(cells.len(), &mut interior);` as the first statement of each loop iteration, and once more **before** consuming the closing `]` — the drain must run while `self.pos` still points at the closer, or it claims what follows. The `use`-loop bug in the sibling round was exactly this.

Update the `rule()` call sites to destructure, and store all three on the `RuleCst` it builds.

- [ ] **Step 5: Render the block-comment case inline**

`pattern_text`, `write_vec_text`, and `move_vec_text` take an `Interior` and emit block comments inline:

```rust
fn pattern_text(pattern: &Pattern, interior: &Interior<'_>) -> String {
    let cells: Vec<String> = pattern.cells.iter().map(pattern_cell_text).collect();
    format!("[{}]", join_cells_with_interior(&cells, interior))
}

/// Joins rendered cells with `, `, splicing each slot's BLOCK comments
/// inline. Slot `i`'s comments precede cell `i`; the tail slot's precede
/// the closing bracket. Only reached when no LINE comment is present —
/// the caller has already sent that case down the multi-line path.
fn join_cells_with_interior(cells: &[String], interior: &Interior<'_>) -> String {
    let mut out = String::new();
    for (i, cell) in cells.iter().enumerate() {
        for c in interior.slots[i].iter() {
            out.push_str(&normalize_comment_text(&c.text));
            out.push(' ');
        }
        out.push_str(cell);
        if i + 1 < cells.len() {
            out.push_str(", ");
        }
    }
    for c in interior.slots[cells.len()].iter() {
        out.push(' ');
        out.push_str(&normalize_comment_text(&c.text));
    }
    out
}
```

- [ ] **Step 6: Send the line-comment case off the grid**

Add a predicate on `RuleCst` and consult it in both `grid_for` and `render_rule`:

```rust
impl RuleCst {
    /// True when any glyph vector carries a LINE comment. Such a rule
    /// cannot be a grid row — nothing may follow `//` on its physical
    /// line — so it renders multi-line and is excluded from the grid's
    /// width computation (docs/tmt/fmt.md (interior comments)).
    fn breaks_the_grid(&self) -> bool {
        [&self.pattern_cells, &self.write_cells, &self.move_cells]
            .iter()
            .any(|v| v.iter().any(|(_, c)| matches!(c.kind, CommentKind::Line)))
    }
}
```

`grid_for` takes `&[&RuleCst]` instead of `&[&Rule]` and skips rules where `breaks_the_grid()` is true. `render_rule` checks it first and, when true, renders the rule with each vector broken across lines — cells at `indent + INDENT_UNIT`, closer at `indent`, using the existing `interior_lines` / `interior_trailing` helpers and the same indexing rule as every other list: slot `i`'s own-line comments print above cell `i`; slot `i + 1`'s same-line comments print at the end of cell `i`'s line.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test fmt_interior`

Expected: PASS, including every pre-existing case — the shared `format_checked` helper puts each through an idempotence check.

- [ ] **Step 8: Verify on the real binary**

A passing test is not the evidence that matters here; the sibling round had unit tests pass twice on code that destroyed comments.

```bash
cargo build --release
printf "alphabet bits { '_', '0', '1' }\n\nmachine {\n  tape a: bits;\n  tape b: bits;\n  entry state s {\n    ['0', /* lo */ '1'] -> stop;\n    [*, *] -> move [>, .] goto s;\n  }\n}\n" | target/release/tmt fmt - --lang tmc
```

Confirm the block comment stays inline and the two rules still align. Then the line-comment form, the slot-0 form, and the tail-slot form. Each output must survive a second `tmt fmt` pass unchanged. Put all four observed outputs in your report.

- [ ] **Step 9: Confirm nothing else moved**

```bash
cargo test -p mtc-turing-machine --test golden_programs
cargo test -p mtc-turing-machine --test tmc_golden
cargo test -p mtc-turing-machine --test stdlib_golden
cargo test -p mtc-turing-machine --test fmt_tmc
```

All must pass with no golden regeneration. Comments are trivia; a moved golden means the parser change is wrong.

- [ ] **Step 10: Full gates and commit**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

```bash
git add crates/turing-machine/src/cst.rs crates/turing-machine/src/parser.rs \
        crates/turing-machine/src/fmt.rs crates/turing-machine/tests/fmt_interior.rs
git commit -m "feat(turing-machine): print glyph-vector interior comments in place

A comment written inside a pattern, write, or move vector now prints where
its author put it instead of being relocated below the enclosing rule,
completing the interior-comment work.

Capture and printing land together deliberately: once the parser drains a
comment into the new side-cars, the old relocation path never sees it, so
a capture-only change would drop the comment rather than merely misplace
it.

These three vectors feed the state-block grid, so handling splits on
comment kind. A block comment stays inline inside the vector, the width
calculation includes it, and the grid still aligns. A line comment cannot
share its line, so the rule carrying one renders multi-line and is
excluded from the grid's width computation, leaving its neighbours'
alignment untouched."
```

---

### Task 2: Documentation

**Files:**
- Modify: `docs/tmt/fmt.md`

- [ ] **Step 1: Rewrite the remaining-exception section**

`docs/tmt/fmt.md`'s "The exception that remains" describes pattern, write, and move vectors as still relocating. That is now false. Replace it with the rule, including the grid interaction, which is the part a reader cannot guess:

```markdown
Inside a pattern, write, or move vector the comment kind decides the
layout, because these vectors are grid columns — a state's rules line
their patterns and actions up in fixed columns.

A `/* … */` comment stays inline inside the vector, so the row keeps its
alignment with its neighbours:

    ['0', /* lo */ '1'] -> stop;
    [*, *]              -> move [>, .] goto s;

A `//` comment cannot share its line, so the rule carrying one leaves the
grid and renders across several lines. Its neighbours keep their own
alignment — a commented rule never widens the columns of the rules around
it.
```

**Every code block must be real output.** Run each through `target/release/tmt fmt` and paste what comes back. The sibling round shipped a wrong CLI flag name across thirteen references by checking pages against pages.

- [ ] **Step 2: Check for forge references**

Run: `rg -n 'github\.com|#[0-9]{1,3}\b|docs/superpowers' docs/tmt/fmt.md`

Expected: no hits.

- [ ] **Step 3: Commit**

```bash
git add docs/tmt/fmt.md
git commit -m "docs(tmt): state the glyph-vector comment rule

The page described pattern, write and move vectors as relocating their
interior comments, which is no longer true. It now states the rule and
the grid interaction a reader cannot infer: a block comment stays inline
and the row keeps its column alignment, while a line comment takes its
rule off the grid without disturbing the rules around it."
```

---

## Final verification

- [ ] **All gates green**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **`crates/core` and `crates/post-machine` untouched**

```bash
git diff --stat master...HEAD -- crates/core crates/post-machine
```

Expected: empty.

- [ ] **No AST type gained trivia**

```bash
git diff master...HEAD -- crates/turing-machine/src/parser.rs | rg '^\+.*pub.*Comment'
```

Expected: only the tuple-returning signatures — no `Comment` field on `Pattern`, `PatternCell`, `WriteVec`, `MoveVec`, or `Rule`.

- [ ] **Goldens unmoved and the stdlib byte-identical**

```bash
cargo test -p mtc-turing-machine --test golden_programs
cargo test -p mtc-turing-machine --test stdlib_golden
git diff --name-only master...HEAD -- crates/turing-machine/tests/golden/
```

Expected: both pass; the last prints nothing.
